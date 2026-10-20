//! Claude Code adapter: capture from a fake home, restore onto a node
//! that has the same home and workspace path (ADR 0011) but none of the
//! state, bring the job back, launch a fake `claude` and check what it saw.
//! The denylist, the signed-block rule, the file modes, the restore
//! allowlist and the running-session guard are asserted on real bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use fabric_core::env::EnvVarName;
use fabric_core::path::AbsPath;
use fabric_harness::claude::ClaudeCode;
use fabric_harness::claude::flags::{LaunchFlags, PermissionMode};
use fabric_harness::manifest::StateManifest;
use fabric_harness::{Harness, HarnessError, Launch, SessionId};

mod common;
use common::{abs, fake_claude, project_key};

const SID: &str = "9c9796f0-2682-4546-a963-d314b3ad543f";
const SECRET: &str = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
const GITHUB: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n";

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
    let key = project_key(Path::new(root));
    let signed = signed_block(root);
    format!(
        concat!(
            "{{\"type\":\"user\",\"cwd\":\"{root}\",\"sessionId\":\"{sid}\",\"message\":{{\"role\":\"user\",\"content\":\"fix the test in {root}/tests\"}}}}\n",
            "{{\"type\":\"file-history-snapshot\",\"messageId\":\"m1\",\"snapshot\":{{\"messageId\":\"m1\",\"trackedFileBackups\":{{\"src/lib.rs\":{{\"backupFileName\":\"abc@v1\",\"version\":1,\"backupTime\":\"t\"}},\"{root}/.env\":{{\"backupFileName\":\"def@v1\",\"version\":1,\"backupTime\":\"t\"}}}},\"timestamp\":\"t\"}},\"isSnapshotUpdate\":false}}\n",
            "{{\"type\":\"assistant\",\"cwd\":\"{root}\",\"message\":{{\"content\":[{signed},{{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{{\"file_path\":\"{root}/src/lib.rs\"}}}}]}}}}\n",
            "{{\"type\":\"user\",\"cwd\":\"{root}\",\"toolUseResult\":\"output spilled to {home}/.claude/projects/{key}/{sid}/tool-results/1.txt; token {secret}\"}}\n",
            "{{\"type\":\"assistant\",\"cwd\":\"{root}\",\"message\":{{\"content\":[{{\"type\":\"thinking\",\"thinking\":\"the token is {secret}\",\"signature\":\"AbCd\"}},{{\"type\":\"text\",\"text\":\"noted\"}}]}}}}\n",
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

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn build_source(work_rel: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let work = home_path.join(work_rel);
    std::fs::create_dir_all(&work).unwrap();
    let root = work.to_string_lossy().into_owned();
    let home_text = home_path.to_string_lossy().into_owned();
    let key = project_key(&work);
    let claude = home_path.join(".claude");
    let project = claude.join("projects").join(&key);
    std::fs::create_dir_all(project.join(format!("{SID}/subagents"))).unwrap();
    std::fs::create_dir_all(project.join(format!("{SID}/tool-results"))).unwrap();
    std::fs::create_dir_all(project.join("memory")).unwrap();
    let transcript_path = project.join(format!("{SID}.jsonl"));
    std::fs::write(&transcript_path, transcript(&root, &home_text)).unwrap();
    std::fs::set_permissions(&transcript_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(
        project.join(format!("{SID}/subagents/agent-1.jsonl")),
        format!("{{\"cwd\":\"{root}\",\"text\":\"sub\"}}\n"),
    )
    .unwrap();
    std::fs::write(
        project.join(format!("{SID}/subagents/agent-1.meta.json")),
        format!("{{\n  \"worktreePath\": \"{root}\"\n}}"),
    )
    .unwrap();
    std::fs::write(
        project.join(format!("{SID}/tool-results/1.txt")),
        format!("big output mentioning {root}\nand token {SECRET}\n"),
    )
    .unwrap();
    std::fs::write(
        project.join(format!("{SID}/tool-results/2.txt")),
        format!("cat ~/.ssh/id_ed25519\n{PEM}done\n"),
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
        format!("GITHUB_TOKEN={GITHUB}\n"),
    )
    .unwrap();
    // Claude's backup of the workspace's `.env`, named by hash: the
    // snapshot record in the transcript says what it is a copy of.
    std::fs::write(
        claude.join("file-history").join(SID).join("def@v1"),
        "DATABASE_URL=postgres://admin:hunter2@db.internal/prod\n",
    )
    .unwrap();
    // A backup no record names.
    std::fs::write(
        claude.join("file-history").join(SID).join("0ff@v3"),
        "unknown\n",
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
    std::fs::write(
        claude.join("sessions/123.json"),
        "{\"pid\":123,\"sessionId\":\"another-session\",\"cwd\":\"/elsewhere\"}",
    )
    .unwrap();
    std::fs::write(home_path.join(".claude.json"), "{\"oauthAccount\":{}}").unwrap();
    // Inside the workspace: the workspace snapshot's business, not this
    // adapter's. The `.git` file under `.claude/worktrees/` is what
    // Claude's EnterWorktree leaves behind and what broke capture before.
    std::fs::create_dir_all(work.join(".claude/worktrees/feat-x")).unwrap();
    std::fs::write(
        work.join(".claude/worktrees/feat-x/.git"),
        "gitdir: /elsewhere\n",
    )
    .unwrap();
    std::fs::write(
        work.join(".claude/settings.local.json"),
        format!("{{\"permissions\":{{\"allow\":[\"Read({root}/docs/**)\"]}}}}"),
    )
    .unwrap();
    std::fs::write(work.join("CLAUDE.local.md"), "local notes\n").unwrap();
    std::fs::write(work.join(".claude/.env"), "SECRET=1\n").unwrap();
    Fixture { home, work }
}

/// A running fake `claude`, killed with its process group on drop.
struct Held {
    child: std::process::Child,
}

impl Held {
    /// Starts `script hold` in a process group of its own.
    fn start(script: &Path) -> Self {
        let child = std::process::Command::new(script)
            .arg("hold")
            .process_group(0)
            .spawn()
            .unwrap();
        Held { child }
    }

    /// The process id.
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.child.id())])
            .status();
        let _ = self.child.wait();
    }
}

/// The adapter for `home`, finding the fake `claude` kept under it.
fn node(home: &Path) -> ClaudeCode {
    let (_, path_var) = fake_claude(home);
    ClaudeCode::new(abs(home), path_var)
}

fn session() -> SessionId {
    SessionId::parse(SID).unwrap()
}

fn transcript_under(home: &Path, work: &Path) -> PathBuf {
    home.join(".claude/projects")
        .join(project_key(work))
        .join(format!("{SID}.jsonl"))
}

/// The text sidecars of a capture into `out`: secrets redacted line by line.
fn assert_sidecars_redacted(out: &Path, key: &str) {
    let results = std::fs::read_to_string(out.join(format!(
        "home/.claude/projects/{key}/{SID}/tool-results/1.txt"
    )))
    .unwrap();
    assert!(!results.contains(SECRET));
    assert!(results.contains("[REDACTED:anthropic-key]"));
    let key_results = std::fs::read_to_string(out.join(format!(
        "home/.claude/projects/{key}/{SID}/tool-results/2.txt"
    )))
    .unwrap();
    assert_eq!(
        key_results,
        "cat ~/.ssh/id_ed25519\n[REDACTED:private-key-block]\ndone\n"
    );
    let history =
        std::fs::read_to_string(out.join(format!("home/.claude/file-history/{SID}/abc@v1")))
            .unwrap();
    assert_eq!(history, "GITHUB_TOKEN=[REDACTED:github-token]\n");
}

#[tokio::test]
async fn capture_carries_session_state_only_and_redacts() {
    let src = build_source("fabric/jobs/j1/work");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();

    let paths: Vec<&str> = manifest
        .files()
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    let key = project_key(&src.work);
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
    assert!(
        !paths
            .iter()
            .any(|p| p.ends_with("def@v1") || p.ends_with("0ff@v3")),
        "a backup of .env and an unnamed backup stay behind: {paths:?}"
    );
    assert!(paths.contains(&"home/.claude/tasks/session-9c9796f0/1.json"));
    for denied in [
        ".credentials.json",
        "sessions/",
        ".claude.json",
        ".env",
        "workspace/",
        "settings.local.json",
        "CLAUDE.local.md",
        "worktrees",
    ] {
        assert!(
            !paths.iter().any(|p| p.contains(denied)),
            "{denied} was carried: {paths:?}"
        );
    }
    assert_eq!(manifest.harness_version(), Some("2.1.294"));
    let rules: Vec<(&str, u32)> = manifest
        .redactions()
        .iter()
        .map(|r| (r.rule(), r.count()))
        .collect();
    assert_eq!(
        rules,
        vec![
            ("anthropic-key", 2),
            ("file-backup-not-in-transcript", 1),
            ("file-backup-of-denied-file", 1),
            ("github-token", 1),
            ("private-key-block", 1),
            ("signed-block-dropped", 1)
        ],
        "one match in the transcript, one in the text sidecar, two backups left behind, one in file history, a key over three lines, one block dropped"
    );
    let staged_path = out
        .path()
        .join(format!("home/.claude/projects/{key}/{SID}.jsonl"));
    let staged = std::fs::read_to_string(&staged_path).unwrap();
    assert!(!staged.contains(SECRET));
    assert!(
        staged.ends_with("cut by a cra"),
        "the cut line must be kept"
    );
    assert!(staged.contains(&signed_block(&src.work.to_string_lossy())));
    assert!(staged.contains(r#"[{"type":"text","text":"noted"}]"#));
    assert_eq!(mode(&staged_path), 0o600);
    assert_sidecars_redacted(out.path(), &key);
    for file in manifest.files() {
        let bytes = std::fs::read(out.path().join(file.rel_path.as_str())).unwrap();
        assert_eq!(file.size, bytes.len() as u64);
        assert_eq!(
            file.blake3.to_string(),
            blake3::hash(&bytes).to_hex().as_str()
        );
    }
}

/// Moves this node's Claude state and Fabric's departure records aside,
/// as if the job ran on another node with the same home path, where
/// Claude is installed and idle: its session registry exists and is
/// empty. Returns where the state went.
fn set_aside(home: &Path, name: &str) -> PathBuf {
    let aside = home.join(name);
    std::fs::create_dir_all(&aside).unwrap();
    std::fs::rename(home.join(".claude"), aside.join("claude")).unwrap();
    if home.join(".fabric").exists() {
        std::fs::rename(home.join(".fabric"), aside.join("fabric")).unwrap();
    }
    std::fs::create_dir_all(home.join(".claude/sessions")).unwrap();
    aside
}

/// Puts a node's state set aside by `set_aside` back in place.
fn bring_back(home: &Path, aside: &Path) {
    std::fs::remove_dir_all(home.join(".claude")).unwrap();
    std::fs::rename(aside.join("claude"), home.join(".claude")).unwrap();
    if home.join(".fabric").exists() {
        std::fs::remove_dir_all(home.join(".fabric")).unwrap();
    }
    if aside.join("fabric").exists() {
        std::fs::rename(aside.join("fabric"), home.join(".fabric")).unwrap();
    }
}

fn regular_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn restore_under_another_root_or_home_is_refused_before_any_write() {
    let src = build_source("fabric/jobs/j1/work");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    node(&home)
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    set_aside(&home, ".claude-pro");

    let other_root = home.join("other/place");
    std::fs::create_dir_all(&other_root).unwrap();
    let err = node(&home)
        .restore(&other_root, out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::RootMismatch { .. }), "{err}");

    let other_home_dir = tempfile::tempdir().unwrap();
    let other_home = other_home_dir.path().canonicalize().unwrap();
    let err = node(&other_home)
        .restore(&src.work, out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::HomeMismatch { .. }), "{err}");
    assert!(
        !home.join(".claude/projects").exists(),
        "nothing was placed"
    );
    assert!(
        !other_home.join(".claude/projects").exists(),
        "nothing was placed"
    );
}

#[tokio::test]
async fn restore_on_a_fresh_node_places_the_staged_bytes_and_protects_what_is_there() {
    let src = build_source("fabric/jobs/j1/work");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    let adapter = node(&home);
    let manifest = adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    set_aside(&home, ".claude-pro");

    let restored = adapter.restore(&src.work, out.path()).await.unwrap();
    assert_eq!(restored.placed.len(), manifest.files().len());
    for file in manifest.files() {
        let rest = file.rel_path.as_str().strip_prefix("home/").unwrap();
        let placed = home.join(rest);
        assert_eq!(
            std::fs::read(&placed).unwrap(),
            std::fs::read(out.path().join(file.rel_path.as_str())).unwrap(),
            "{rest} is placed byte for byte"
        );
        assert_eq!(mode(&placed), 0o600);
    }
    assert!(!home.join(".claude/.credentials.json").exists());
    assert!(!home.join(".claude/sessions/123.json").exists());

    // The same bytes again change nothing; a grown transcript is kept.
    let transcript = transcript_under(&home, &src.work);
    adapter.restore(&src.work, out.path()).await.unwrap();
    let text = std::fs::read_to_string(&transcript).unwrap();
    let grown = format!("{text}sh\"}}\n{{\"type\":\"user\"}}\n");
    std::fs::write(&transcript, &grown).unwrap();
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(matches!(err, HarnessError::StateDiverged { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(&transcript).unwrap(), grown);

    // A divergence in the last file of the manifest stops the restore
    // before the first file is written.
    std::fs::write(&transcript, &text).unwrap();
    let last = manifest.files().last().unwrap();
    let last_placed = home.join(last.rel_path.as_str().strip_prefix("home/").unwrap());
    std::fs::write(&last_placed, "edited here\n").unwrap();
    std::fs::remove_file(&transcript).unwrap();
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::StateDiverged { ref path } if path == &last_placed),
        "{err}"
    );
    assert!(
        !transcript.exists(),
        "nothing before the divergent file was placed"
    );
}

/// Writes `manifest` under `out` with one more entry, for `body` at `rel`,
/// as someone who forged the tree would.
fn forge(out: &Path, manifest: &StateManifest, rel: &str, body: &[u8]) {
    let mut value = serde_json::to_value(manifest).unwrap();
    let files = value["files"].as_array_mut().unwrap();
    files.push(serde_json::json!({
        "rel_path": rel,
        "content": "Opaque",
        "mutability": "Replaceable",
        "size": body.len(),
        "blake3": blake3::hash(body).to_hex().to_string(),
    }));
    files.sort_by(|a, b| a["rel_path"].as_str().cmp(&b["rel_path"].as_str()));
    std::fs::write(out.join("manifest.json"), value.to_string()).unwrap();
}

#[tokio::test]
async fn a_manifest_naming_a_file_outside_session_state_is_refused() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    let adapter = node(&home);
    let manifest = adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    set_aside(&home, ".claude-pro");
    for (rel, expect_denied) in [
        ("home/Library/LaunchAgents/evil.plist", false),
        ("home/.zshrc", false),
        ("home/.claude/settings.json", false),
        ("home/.ssh/authorized_keys", true),
    ] {
        let body = b"payload\n";
        let staged = out.path().join(rel);
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, body).unwrap();
        forge(out.path(), &manifest, rel, body);
        let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
        if expect_denied {
            assert!(matches!(err, HarnessError::Denied { .. }), "{err}");
        } else {
            assert!(matches!(err, HarnessError::UnexpectedFile { .. }), "{err}");
        }
        assert!(!home.join(rel.strip_prefix("home/").unwrap()).exists());
        assert!(
            !home.join(".claude/projects").exists(),
            "nothing was placed"
        );
    }
    manifest.write(out.path()).unwrap();
    adapter.restore(&src.work, out.path()).await.unwrap();
}

#[tokio::test]
async fn a_manifest_that_relabels_a_transcript_as_rewritable_is_refused() {
    // A transcript only grows; a manifest that calls it rewritable would
    // let restore replace local bytes outright.
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    let adapter = node(&home);
    let manifest = adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    let transcript = format!(
        "home/.claude/projects/{}/{SID}.jsonl",
        project_key(&src.work)
    );
    let mut value = serde_json::to_value(&manifest).unwrap();
    for file in value["files"].as_array_mut().unwrap() {
        if file["rel_path"] == transcript.as_str() {
            file["mutability"] = "Replaceable".into();
        }
    }
    std::fs::write(out.path().join("manifest.json"), value.to_string()).unwrap();
    set_aside(&home, ".claude-pro");
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::EntryMismatch { ref path, .. } if *path == transcript),
        "{err}"
    );
    assert!(
        !home.join(".claude/projects").exists(),
        "nothing was placed"
    );
}

#[tokio::test]
async fn a_job_that_leaves_and_comes_back_keeps_the_original_and_gains_the_new_turns() {
    // Pro to Air to Pro, same home and root on both. The Pro still holds
    // the unredacted transcript; the copy that comes back was redacted on
    // the way out and grew on the Air.
    let src = build_source("fabric/jobs/j1/work");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    let transcript = transcript_under(&home, &src.work);
    let original = std::fs::read(&transcript).unwrap();
    let out_1 = tempfile::tempdir().unwrap();
    let sent = adapter
        .capture(&src.work, &session(), out_1.path())
        .await
        .unwrap();
    adapter.confirm_departure(&sent).await.unwrap();
    let pro = set_aside(&home, ".claude-pro");

    adapter.restore(&src.work, out_1.path()).await.unwrap();
    let air_suffix = format!(
        "sh\"}}\n{{\"type\":\"assistant\",\"cwd\":\"{}\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"done on the air\"}}]}}}}\n",
        src.work.display()
    );
    let mut on_air = std::fs::read(&transcript).unwrap();
    on_air.extend_from_slice(air_suffix.as_bytes());
    std::fs::write(&transcript, &on_air).unwrap();
    // Claude rewrites memory and task files in place.
    let (memory, task) = rewritable_files(&home, &src.work);
    std::fs::write(&memory, "# memory\nlearned on the air\n").unwrap();
    std::fs::write(&task, "{\"subject\":\"t\",\"status\":\"done\"}").unwrap();
    let out_2 = tempfile::tempdir().unwrap();
    let back = adapter
        .capture(&src.work, &session(), out_2.path())
        .await
        .unwrap();
    adapter.confirm_departure(&back).await.unwrap();
    set_aside(&home, ".claude-air");
    bring_back(&home, &pro);

    adapter.restore(&src.work, out_2.path()).await.unwrap();
    let mut expected = original.clone();
    expected.extend_from_slice(air_suffix.as_bytes());
    assert_eq!(
        std::fs::read(&transcript).unwrap(),
        expected,
        "the Pro keeps every byte it had, secret and signed block included, plus the Air's turn"
    );
    assert_eq!(mode(&transcript), 0o600);
    assert_eq!(
        std::fs::read_to_string(&memory).unwrap(),
        "# memory\nlearned on the air\n",
        "a file the Pro left untouched takes the Air's rewrite"
    );
    assert!(std::fs::read_to_string(&task).unwrap().contains("done"));

    // Restoring again changes nothing, and capturing again yields what the
    // Air sent.
    adapter.restore(&src.work, out_2.path()).await.unwrap();
    assert_eq!(std::fs::read(&transcript).unwrap(), expected);
    let out_3 = tempfile::tempdir().unwrap();
    adapter
        .capture(&src.work, &session(), out_3.path())
        .await
        .unwrap();
    let staged = |out: &Path| {
        std::fs::read(out.join(format!(
            "home/.claude/projects/{}/{SID}.jsonl",
            project_key(&src.work)
        )))
        .unwrap()
    };
    assert_eq!(staged(out_3.path()), staged(out_2.path()));
}

/// The project memory and a task file of the session under `home`.
fn rewritable_files(home: &Path, work: &Path) -> (PathBuf, PathBuf) {
    (
        home.join(".claude/projects")
            .join(project_key(work))
            .join("memory/MEMORY.md"),
        home.join(".claude/tasks/session-9c9796f0/1.json"),
    )
}

#[tokio::test]
async fn a_rewritten_file_edited_here_after_the_job_left_is_not_replaced() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    let out_1 = tempfile::tempdir().unwrap();
    let sent = adapter
        .capture(&src.work, &session(), out_1.path())
        .await
        .unwrap();
    adapter.confirm_departure(&sent).await.unwrap();
    let pro = set_aside(&home, ".claude-pro");
    adapter.restore(&src.work, out_1.path()).await.unwrap();
    let (memory, _) = rewritable_files(&home, &src.work);
    std::fs::write(&memory, "# memory\nfrom the air\n").unwrap();
    let out_2 = tempfile::tempdir().unwrap();
    let back = adapter
        .capture(&src.work, &session(), out_2.path())
        .await
        .unwrap();
    adapter.confirm_departure(&back).await.unwrap();
    set_aside(&home, ".claude-air");
    bring_back(&home, &pro);
    // Someone used the Pro's copy after the job left.
    std::fs::write(&memory, "# memory\nfrom the pro\n").unwrap();
    let transcript = transcript_under(&home, &src.work);
    let before = std::fs::read(&transcript).unwrap();
    let err = adapter.restore(&src.work, out_2.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::StateDiverged { ref path } if path == &memory),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&memory).unwrap(),
        "# memory\nfrom the pro\n"
    );
    assert_eq!(
        std::fs::read(&transcript).unwrap(),
        before,
        "nothing was placed"
    );
}

#[tokio::test]
async fn a_symlink_in_session_state_refuses_the_capture() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let outside = home.join("outside.txt");
    std::fs::write(&outside, "aws_secret_access_key=supersecret\n").unwrap();
    let results = home
        .join(".claude/projects")
        .join(project_key(&src.work))
        .join(format!("{SID}/tool-results/link.txt"));
    std::os::unix::fs::symlink(&outside, &results).unwrap();
    let out = tempfile::tempdir().unwrap();
    let err = node(&home)
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SymlinkInState { .. }), "{err}");
    std::fs::remove_file(&results).unwrap();
    let transcript = transcript_under(&home, &src.work);
    let real = home.join("real.jsonl");
    std::fs::rename(&transcript, &real).unwrap();
    std::os::unix::fs::symlink(&real, &transcript).unwrap();
    let err = node(&home)
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SymlinkInState { .. }), "{err}");
    assert!(regular_files(out.path()).is_empty(), "nothing was staged");
}

#[tokio::test]
async fn a_symlink_on_the_way_to_a_destination_refuses_the_restore() {
    // The target node keeps its project memory in the workspace through a
    // link; restore must not write MEMORY.md through it.
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    let adapter = node(&home);
    adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    set_aside(&home, ".claude-pro");
    let linked = src.work.join("docs/memory");
    std::fs::create_dir_all(&linked).unwrap();
    let project = home.join(".claude/projects").join(project_key(&src.work));
    std::fs::create_dir_all(&project).unwrap();
    std::os::unix::fs::symlink(&linked, project.join("memory")).unwrap();
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::SymlinkInState { ref path } if path == &project.join("memory")),
        "{err}"
    );
    assert!(!linked.join("MEMORY.md").exists());
    assert!(
        !project.join(format!("{SID}.jsonl")).exists(),
        "nothing was placed"
    );
}

#[tokio::test]
async fn a_live_session_blocks_capture_and_restore() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    let out = tempfile::tempdir().unwrap();
    adapter
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    // The registry names this test process as the writer of the session.
    let registry = home.join(".claude/sessions");
    let own = std::process::id();
    std::fs::write(
        registry.join(format!("{own}.json")),
        format!("{{\"pid\":{own},\"sessionId\":\"{SID}\",\"cwd\":\"/w\"}}"),
    )
    .unwrap();
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessRunning { ref pids, .. } if pids == &vec![own]),
        "{err}"
    );
    let out2 = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &session(), out2.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::HarnessRunning { .. }), "{err}");
    // A record of a process that has exited is stale and does not block.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    std::fs::remove_file(registry.join(format!("{own}.json"))).unwrap();
    std::fs::write(
        registry.join(format!("{dead}.json")),
        format!("{{\"pid\":{dead},\"sessionId\":\"{SID}\",\"cwd\":\"/w\"}}"),
    )
    .unwrap();
    adapter.restore(&src.work, out.path()).await.unwrap();
}

#[tokio::test]
async fn a_registry_record_that_does_not_parse_lets_any_running_claude_block() {
    // A registry written by another Claude version: the adapter cannot tell
    // which session the record is for, so every running claude counts.
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let (script, _) = fake_claude(&home);
    std::fs::write(
        home.join(".claude/sessions/999.json"),
        "{\"processId\":999,\"session\":\"x\"}",
    )
    .unwrap();
    let held = Held::start(&script);
    let out = tempfile::tempdir().unwrap();
    let err = node(&home)
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessRunning { ref pids, .. } if pids.contains(&held.pid())),
        "{err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn a_missing_registry_lets_any_running_claude_block() {
    // An older Claude keeps no registry; a running one may be resuming
    // the very session a restore would replace.
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    node(&home)
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    set_aside(&home, ".claude-pro");
    std::fs::remove_dir_all(home.join(".claude/sessions")).unwrap();
    let (script, _) = fake_claude(&home);
    let held = Held::start(&script);
    let err = node(&home)
        .restore(&src.work, out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessRunning { ref pids, .. } if pids.contains(&held.pid())),
        "{err}"
    );
    assert!(
        !home.join(".claude/projects").exists(),
        "nothing was placed"
    );
}

#[tokio::test]
async fn a_truncated_staged_file_is_refused_before_anything_is_placed() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    node(&home)
        .capture(&src.work, &session(), out.path())
        .await
        .unwrap();
    let key = project_key(&src.work);
    let staged = out
        .path()
        .join(format!("home/.claude/projects/{key}/{SID}.jsonl"));
    let bytes = std::fs::read(&staged).unwrap();
    std::fs::write(&staged, &bytes[..bytes.len() / 2]).unwrap();
    set_aside(&home, ".claude-pro");
    let err = node(&home)
        .restore(&src.work, out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::StateCorrupt { .. }), "{err}");
    assert!(
        !home.join(".claude/projects").exists(),
        "nothing was placed"
    );
}

#[tokio::test]
async fn launch_resumes_in_the_workspace_with_exactly_the_given_env() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let work = home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    let (_script, bin_dir) = fake_claude(&home);
    let log = home.join("log.txt");
    let adapter = ClaudeCode::new(abs(&home), bin_dir.clone()).with_flags(LaunchFlags {
        permission_mode: Some(PermissionMode::AcceptEdits),
        ..LaunchFlags::default()
    });

    let installed = adapter.detect().await.unwrap().unwrap();
    assert_eq!(installed.version.as_deref(), Some("2.1.294"));

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
            Launch::Interactive {
                resume: Some(session()),
                prompt: None,
            },
            &env,
        )
        .await
        .unwrap();
    assert_eq!(running.session().cloned(), Some(session()));
    assert_eq!(running.pid(), running.pgid());
    assert_eq!(Some(running.pid().get()), running.child_mut().id());
    let status = running.child_mut().wait().await.unwrap();
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
            Launch::Headless {
                resume: None,
                prompt: "hello".to_owned(),
            },
            &env,
        )
        .await
        .unwrap();
    let fresh = running.session().cloned().unwrap();
    running.child_mut().wait().await.unwrap();
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(
        seen.contains(&format!(
            "arg=-p\narg=--output-format\narg=json\narg=--session-id\narg={fresh}\n"
        )),
        "{seen}"
    );
    assert!(seen.contains("arg=--\narg=hello\n"), "{seen}");
}

#[tokio::test]
async fn a_settings_overlay_with_a_literal_token_refuses_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let work = home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    let overlay = home.join("overlay.json");
    std::fs::write(&overlay, format!("{{\"env\":{{\"GH\":\"{GITHUB}\"}}}}")).unwrap();
    let adapter = node(&home).with_flags(LaunchFlags {
        settings: Some(AbsPath::parse(overlay.to_str().unwrap()).unwrap()),
        ..LaunchFlags::default()
    });
    let resume = || Launch::Headless {
        resume: Some(session()),
        prompt: "go".to_owned(),
    };
    let err = adapter.launch(&work, resume(), &[]).await.unwrap_err();
    assert!(matches!(err, HarnessError::SecretInOverlay { .. }), "{err}");
    std::fs::write(&overlay, r#"{"env":{"X":"A=1\nDB_PASSWORD=hunter22xyz"}}"#).unwrap();
    let err = adapter.launch(&work, resume(), &[]).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::SecretInOverlay { .. }),
        "an escaped newline hides nothing: {err}"
    );
    std::fs::write(&overlay, "{\"env\":{\"GH\":\"${GH_TOKEN}\"}}").unwrap();
    let log = home.join("log.txt");
    let env = [(
        EnvVarName::parse("FAKE_LOG").unwrap(),
        log.to_string_lossy().into_owned(),
    )];
    let mut running = adapter.launch(&work, resume(), &env).await.unwrap();
    running.child_mut().wait().await.unwrap();
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(
        seen.contains(&format!("arg=--settings\narg={}\n", overlay.display())),
        "{seen}"
    );
}

#[tokio::test]
async fn missing_session_and_latest_session_are_reported() {
    let src = build_source("w");
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
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
