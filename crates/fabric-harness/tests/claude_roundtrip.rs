//! Claude Code adapter: capture from one fake home, restore into another
//! at a different workspace path, launch a fake `claude` and check what it
//! saw. The denylist and the signed-block rule are asserted on real bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fabric_core::env::EnvVarName;
use fabric_harness::claude::ClaudeCode;
use fabric_harness::claude::slug::slug;
use fabric_harness::{Harness, HarnessError, Io, Launch, SessionId};

const SID: &str = "9c9796f0-2682-4546-a963-d314b3ad543f";
const SECRET: &str = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";

struct Fixture {
    home: tempfile::TempDir,
    work: PathBuf,
}

fn signed_block(path: &str) -> String {
    format!(
        r#"{{"type":"thinking","thinking":"I should read {path}/src/lib.rs first","signature":"EqQBCkYIBRgCKkB"}}"#
    )
}

fn transcript(root: &str, home: &str) -> String {
    let key = slug(Path::new(root)).unwrap();
    let signed = signed_block(root);
    format!(
        concat!(
            "{{\"type\":\"user\",\"cwd\":\"{root}\",\"sessionId\":\"{sid}\",\"message\":{{\"role\":\"user\",\"content\":\"fix the test in {root}/tests\"}}}}\n",
            "{{\"type\":\"assistant\",\"cwd\":\"{root}\",\"message\":{{\"content\":[{signed},{{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{{\"file_path\":\"{root}/src/lib.rs\"}}}}]}}}}\n",
            "{{\"type\":\"user\",\"cwd\":\"{root}\",\"toolUseResult\":\"output spilled to {home}/.claude/projects/{key}/{sid}/tool-results/1.txt; token {secret}\"}}\n",
            "{{\"type\":\"user\",\"cwd\":\"{root}\",\"cut\":\"this line was cut by a cra"
        ),
        root = root,
        sid = SID,
        signed = signed,
        home = home,
        key = key,
        secret = SECRET,
    )
}

fn build_source(work_rel: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let work = home_path.join(work_rel);
    std::fs::create_dir_all(&work).unwrap();
    let root = work.to_string_lossy().into_owned();
    let home_text = home_path.to_string_lossy().into_owned();
    let key = slug(&work).unwrap();
    let claude = home_path.join(".claude");
    let project = claude.join("projects").join(&key);
    std::fs::create_dir_all(project.join(format!("{SID}/subagents"))).unwrap();
    std::fs::create_dir_all(project.join(format!("{SID}/tool-results"))).unwrap();
    std::fs::create_dir_all(project.join("memory")).unwrap();
    std::fs::write(
        project.join(format!("{SID}.jsonl")),
        transcript(&root, &home_text),
    )
    .unwrap();
    std::fs::write(
        project.join(format!("{SID}/subagents/agent-1.jsonl")),
        format!("{{\"cwd\":\"{root}\",\"text\":\"sub\"}}\n"),
    )
    .unwrap();
    std::fs::write(
        project.join(format!("{SID}/subagents/agent-1.meta.json")),
        format!("{{\"worktreePath\":\"{root}\"}}"),
    )
    .unwrap();
    std::fs::write(
        project.join(format!("{SID}/tool-results/1.txt")),
        format!("big output mentioning {root}\n"),
    )
    .unwrap();
    std::fs::write(project.join("memory/MEMORY.md"), "# memory\n").unwrap();
    // A sidecar under another project key, as this very session has.
    let other = claude
        .join("projects")
        .join("-other-key")
        .join(format!("{SID}/workflows"));
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join("run.json"),
        format!("{{\"scriptPath\":\"{root}/x.ts\"}}"),
    )
    .unwrap();
    std::fs::create_dir_all(claude.join("file-history").join(SID)).unwrap();
    std::fs::write(
        claude.join("file-history").join(SID).join("abc@v1"),
        "old bytes",
    )
    .unwrap();
    std::fs::create_dir_all(claude.join("tasks/session-9c9796f0")).unwrap();
    std::fs::write(
        claude.join("tasks/session-9c9796f0/1.json"),
        "{\"subject\":\"t\"}",
    )
    .unwrap();
    // Never carried.
    std::fs::write(claude.join(".credentials.json"), "{\"token\":\"x\"}").unwrap();
    std::fs::create_dir_all(claude.join("sessions")).unwrap();
    std::fs::write(claude.join("sessions/123.json"), "{}").unwrap();
    std::fs::write(home_path.join(".claude.json"), "{\"oauthAccount\":{}}").unwrap();
    // Inside the workspace.
    std::fs::create_dir_all(work.join(".claude")).unwrap();
    std::fs::write(
        work.join(".claude/settings.local.json"),
        format!("{{\"permissions\":{{\"allow\":[\"Read({root}/docs/**)\"]}}}}"),
    )
    .unwrap();
    std::fs::write(work.join("CLAUDE.local.md"), "local notes\n").unwrap();
    Fixture { home, work }
}

fn fake_claude(dir: &Path) -> (PathBuf, OsString) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\n{ echo \"cwd=$(pwd -P)\"; for a in \"$@\"; do echo \"arg=$a\"; done; echo \"home=$HOME\"; echo \"leak=${CARGO_MANIFEST_DIR:-unset}\"; } > \"$FAKE_LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, bin.into_os_string())
}

fn session() -> SessionId {
    SessionId::parse(SID).unwrap()
}

#[tokio::test]
async fn capture_carries_session_state_only_and_redacts() {
    let src = build_source("fabric/jobs/j1/work");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = ClaudeCode::new(home.clone()).with_path_var(OsString::new());
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();

    let paths: Vec<&str> = manifest.files.iter().map(|f| f.rel_path.as_str()).collect();
    let key = slug(&src.work).unwrap();
    assert!(paths.contains(&format!("home/.claude/projects/{key}/{SID}.jsonl").as_str()));
    assert!(
        paths.contains(
            &format!("home/.claude/projects/{key}/{SID}/subagents/agent-1.jsonl").as_str()
        )
    );
    assert!(paths.contains(&format!("home/.claude/projects/{key}/memory/MEMORY.md").as_str()));
    assert!(
        paths.contains(
            &format!("home/.claude/projects/-other-key/{SID}/workflows/run.json").as_str()
        )
    );
    assert!(paths.contains(&format!("home/.claude/file-history/{SID}/abc@v1").as_str()));
    assert!(paths.contains(&"home/.claude/tasks/session-9c9796f0/1.json"));
    assert!(paths.contains(&"workspace/.claude/settings.local.json"));
    assert!(paths.contains(&"workspace/CLAUDE.local.md"));
    for denied in [".credentials.json", "sessions/", ".claude.json", ".env"] {
        assert!(
            !paths.iter().any(|p| p.contains(denied)),
            "{denied} was carried: {paths:?}"
        );
    }
    assert_eq!(manifest.harness_version, None);
    assert_eq!(manifest.redactions.len(), 1);
    assert_eq!(manifest.redactions[0].rule, "anthropic-key");
    assert_eq!(manifest.redactions[0].count, 1);
    let staged = std::fs::read_to_string(
        out.path()
            .join(format!("home/.claude/projects/{key}/{SID}.jsonl")),
    )
    .unwrap();
    assert!(!staged.contains(SECRET));
    assert!(
        staged.ends_with("cut by a cra"),
        "the cut line must be kept"
    );
    assert!(staged.contains(&signed_block(&src.work.to_string_lossy())));
}

#[tokio::test]
async fn capture_of_a_dotenv_in_the_workspace_claude_dir_is_refused_not_skipped() {
    // The denylist is a hard stop on anything an adapter lists; the Claude
    // adapter lists the whole workspace `.claude/` tree, so a `.env` there
    // must make capture fail closed rather than carry or quietly drop it.
    let src = build_source("w");
    let adapter = ClaudeCode::new(src.home.path().canonicalize().unwrap());
    let out = tempfile::tempdir().unwrap();
    assert!(
        adapter
            .capture(&src.work, &session(), out.path())
            .await
            .is_ok()
    );
    std::fs::write(src.work.join(".claude/.env"), "SECRET=1").unwrap();
    let out2 = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &session(), out2.path())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            HarnessError::Denied {
                rule: "dotenv file",
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn restore_into_another_root_rewrites_paths_but_not_signed_blocks() {
    let src = build_source("fabric/jobs/j1/work");
    let home_a = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    ClaudeCode::new(home_a.clone())
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();

    let home_b_dir = tempfile::tempdir().unwrap();
    let home_b = home_b_dir.path().canonicalize().unwrap();
    let work_b = home_b.join("other/place");
    std::fs::create_dir_all(&work_b).unwrap();
    let adapter_b = ClaudeCode::new(home_b.clone());
    let restored = adapter_b.restore(&work_b, out.path()).await.unwrap();
    assert_eq!(restored.rules.len(), 3, "{:?}", restored.rules);

    let old_key = slug(&src.work).unwrap();
    let new_key = slug(&work_b).unwrap();
    let project = home_b.join(".claude/projects").join(&new_key);
    assert!(project.join(format!("{SID}.jsonl")).is_file());
    assert!(!home_b.join(".claude/projects").join(&old_key).exists());
    let text = std::fs::read_to_string(project.join(format!("{SID}.jsonl"))).unwrap();
    let root_a = src.work.to_string_lossy();
    let root_b = work_b.to_string_lossy();
    assert!(text.contains(&format!("\"cwd\":\"{root_b}\"")));
    assert!(text.contains(&format!("\"file_path\":\"{root_b}/src/lib.rs\"")));
    assert!(text.contains(&format!(
        "{}/.claude/projects/{new_key}/{SID}/tool-results/1.txt",
        home_b.display()
    )));
    assert!(
        text.contains(&signed_block(&root_a)),
        "signed block must keep the old path"
    );
    assert_eq!(
        text.matches(root_a.as_ref()).count(),
        1,
        "only the signed block keeps the old root"
    );
    assert!(text.ends_with("cut by a cra"));

    let meta = std::fs::read_to_string(project.join(format!("{SID}/subagents/agent-1.meta.json")))
        .unwrap();
    assert_eq!(meta, format!("{{\"worktreePath\":\"{root_b}\"}}"));
    let other = std::fs::read_to_string(home_b.join(format!(
        ".claude/projects/-other-key/{SID}/workflows/run.json"
    )))
    .unwrap();
    assert_eq!(other, format!("{{\"scriptPath\":\"{root_b}/x.ts\"}}"));
    let results =
        std::fs::read_to_string(project.join(format!("{SID}/tool-results/1.txt"))).unwrap();
    assert!(
        results.contains(root_a.as_ref()),
        "plain text sidecars are not rewritten"
    );
    let local = std::fs::read_to_string(work_b.join(".claude/settings.local.json")).unwrap();
    assert!(local.contains(&format!("Read({root_b}/docs/**)")));
    assert_eq!(
        std::fs::read_to_string(work_b.join("CLAUDE.local.md")).unwrap(),
        "local notes\n"
    );
    assert!(!home_b.join(".claude/.credentials.json").exists());

    // A second restore of the same bytes is a no-op; a grown transcript is
    // accepted; a diverged one is refused before anything is written.
    adapter_b.restore(&work_b, out.path()).await.unwrap();
    let transcript = project.join(format!("{SID}.jsonl"));
    let grown = format!("{text}sh\"}}\n{{\"type\":\"user\",\"cwd\":\"{root_b}\"}}\n");
    std::fs::write(&transcript, &grown).unwrap();
    let err = adapter_b.restore(&work_b, out.path()).await.unwrap_err();
    assert!(matches!(err, HarnessError::StateDiverged { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(&transcript).unwrap(), grown);
}

#[tokio::test]
async fn restore_into_the_same_root_rewrites_nothing() {
    let src = build_source("fabric/jobs/j1/work");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    let adapter = ClaudeCode::new(home.clone());
    adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    let key = slug(&src.work).unwrap();
    let transcript = home
        .join(".claude/projects")
        .join(&key)
        .join(format!("{SID}.jsonl"));
    let before = std::fs::read(&transcript).unwrap();
    // The on-disk file still holds the secret; the staged copy is redacted,
    // so placing it over the original is a divergence and is refused.
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(matches!(err, HarnessError::StateDiverged { .. }), "{err}");
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    // Remove the original: the redacted copy lands under the same key, unmapped.
    std::fs::remove_file(&transcript).unwrap();
    let restored = adapter.restore(&src.work, out.path()).await.unwrap();
    assert!(restored.rules.is_empty());
    let staged = std::fs::read(
        out.path()
            .join(format!("home/.claude/projects/{key}/{SID}.jsonl")),
    )
    .unwrap();
    assert_eq!(std::fs::read(&transcript).unwrap(), staged);
}

#[tokio::test]
async fn launch_resumes_in_the_workspace_with_exactly_the_given_env() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let work = home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    let (_script, bin_dir) = fake_claude(&home);
    let log = home.join("log.txt");
    let adapter = ClaudeCode::new(home.clone())
        .with_path_var(bin_dir.clone())
        .with_flags(vec![
            "--permission-mode".to_owned(),
            "acceptEdits".to_owned(),
        ]);

    let installed = adapter.detect().await;
    assert!(
        matches!(installed, Err(HarnessError::UnexpectedVersionOutput { .. })),
        "{installed:?}"
    );

    // CARGO_MANIFEST_DIR is set in this test process by cargo; the launch
    // env does not list it, so the fake must see it unset.
    let env = vec![
        (
            EnvVarName::parse("HOME").unwrap(),
            home.to_string_lossy().into_owned(),
        ),
        (
            EnvVarName::parse("PATH").unwrap(),
            format!("{}:/usr/bin:/bin", bin_dir.to_string_lossy()),
        ),
        (
            EnvVarName::parse("FAKE_LOG").unwrap(),
            log.to_string_lossy().into_owned(),
        ),
    ];
    let mut running = adapter
        .launch(
            &work,
            Launch::Resume {
                session: session(),
                prompt: None,
            },
            Io::Terminal,
            &env,
        )
        .await
        .unwrap();
    assert_eq!(running.session, Some(session()));
    assert_eq!(running.pid, running.pgid);
    let status = running.child.wait().await.unwrap();
    assert!(status.success());
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(seen.contains(&format!("cwd={}", work.display())), "{seen}");
    assert!(
        seen.contains(&format!(
            "arg=--resume\narg={SID}\narg=--permission-mode\narg=acceptEdits\n"
        )),
        "{seen}"
    );
    assert!(seen.contains(&format!("home={}", home.display())), "{seen}");
    assert!(seen.contains("leak=unset"), "{seen}");

    let mut running = adapter
        .launch(
            &work,
            Launch::New {
                prompt: "hello".to_owned(),
            },
            Io::Piped,
            &env,
        )
        .await
        .unwrap();
    assert!(running.session.is_some());
    running.child.wait().await.unwrap();
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(
        seen.contains("arg=-p\narg=--output-format\narg=json\narg=--session-id\n"),
        "{seen}"
    );
    assert!(
        seen.ends_with("arg=--\narg=hello\nhome=") || seen.contains("arg=--\narg=hello\n"),
        "{seen}"
    );
}

#[tokio::test]
async fn missing_session_and_latest_session_are_reported() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = ClaudeCode::new(home);
    assert_eq!(adapter.latest_session(&src.work).unwrap(), Some(session()));
    let out = tempfile::tempdir().unwrap();
    let missing = SessionId::parse("00000000-0000-0000-0000-000000000000").unwrap();
    let err = adapter
        .capture(&src.work, &missing, out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SessionNotFound { .. }), "{err}");
    let empty = src.home.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(adapter.latest_session(&empty).unwrap(), None);
}
