//! Codex adapter: a paginated lineage with a compressed prefix is captured
//! under the writer lock, restored onto a node with the same home and
//! workspace path but no Codex state, under `sessions/fabric/<thread>/`,
//! and a fake `codex` is launched there.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fabric_core::env::EnvVarName;
use fabric_harness::codex::Codex;
use fabric_harness::{Harness, HarnessError, Io, Launch, SessionId};

const THREAD: &str = "019f233e-bb33-7342-85fa-f2d33b14368a";
const PREFIX: &str = "019f1111-bb33-7342-85fa-f2d33b14368b";

fn session_meta(id: &str, cwd: &str, history_base: Option<&str>) -> String {
    let base = match history_base {
        Some(b) => format!(
            ",\"history_base\":{{\"thread_id\":\"{b}\",\"end_ordinal_exclusive\":9,\"end_byte_offset\":120}}"
        ),
        None => String::new(),
    };
    format!(
        "{{\"timestamp\":\"2026-10-19T10:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"session_id\":\"{id}\",\"timestamp\":\"2026-10-19T10:00:00.000Z\",\"cwd\":\"{cwd}\",\"originator\":\"codex_cli_rs\",\"cli_version\":\"0.155.1\",\"history_mode\":\"paginated\"{base}}}}}\n{{\"timestamp\":\"2026-10-19T10:00:01.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"hello from {cwd}\"}}]}}}}\n"
    )
}

struct Fixture {
    home: tempfile::TempDir,
    work: PathBuf,
    head: PathBuf,
    prefix: PathBuf,
}

fn build_source() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let work = home_path.join("fabric/jobs/j1/work");
    std::fs::create_dir_all(&work).unwrap();
    let codex = home_path.join(".codex");
    let day = codex.join("sessions/2026/10/19");
    std::fs::create_dir_all(&day).unwrap();
    let head = day.join(format!("rollout-2026-10-19T11-00-00-{THREAD}.jsonl"));
    std::fs::write(
        &head,
        session_meta(THREAD, &work.to_string_lossy(), Some(PREFIX)),
    )
    .unwrap();
    let archived = codex.join("archived_sessions");
    std::fs::create_dir_all(&archived).unwrap();
    let prefix = archived.join(format!("rollout-2026-10-18T09-00-00-{PREFIX}.jsonl.zst"));
    let mut encoder =
        zstd::stream::write::Encoder::new(std::fs::File::create(&prefix).unwrap(), 3).unwrap();
    encoder
        .write_all(session_meta(PREFIX, "/somewhere/else", None).as_bytes())
        .unwrap();
    encoder.finish().unwrap();
    // An older rollout of another thread in the same cwd, which must lose
    // to the newer head in `latest_session`.
    let older = day.join("rollout-2026-10-19T08-00-00-019f0000-bb33-7342-85fa-f2d33b140000.jsonl");
    std::fs::write(
        &older,
        session_meta(
            "019f0000-bb33-7342-85fa-f2d33b140000",
            &work.to_string_lossy(),
            None,
        ),
    )
    .unwrap();
    // Never carried.
    std::fs::write(codex.join("auth.json"), "{\"tokens\":{}}").unwrap();
    std::fs::write(codex.join("state_5.sqlite"), "sqlite").unwrap();
    std::fs::write(codex.join("config.toml"), "model = \"x\"\n").unwrap();
    Fixture {
        home,
        work,
        head,
        prefix,
    }
}

fn fake_codex(dir: &Path) -> OsString {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("codex");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"codex-cli 0.155.1\"; exit 0; fi\n{ echo \"cwd=$(pwd -P)\"; for a in \"$@\"; do echo \"arg=$a\"; done; } > \"$FAKE_LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin.into_os_string()
}

fn thread() -> SessionId {
    SessionId::parse(THREAD).unwrap()
}

#[tokio::test]
async fn capture_follows_the_lineage_and_carries_nothing_else() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let adapter = Codex::new(home.clone()).with_path_var(OsString::new());
    assert_eq!(adapter.latest_session(&src.work).unwrap(), Some(thread()));

    let chain = adapter.lineage(&thread()).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].path, src.head);
    assert_eq!(chain[1].path, src.prefix);
    assert!(chain[1].compressed);

    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    let paths: Vec<&str> = manifest.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            format!("home/.codex/archived_sessions/rollout-2026-10-18T09-00-00-{PREFIX}.jsonl.zst")
                .as_str(),
            format!("home/.codex/sessions/2026/10/19/rollout-2026-10-19T11-00-00-{THREAD}.jsonl")
                .as_str(),
        ]
    );
    assert_eq!(
        std::fs::read(out.path().join(paths[1])).unwrap(),
        std::fs::read(&src.head).unwrap(),
        "rollout bytes travel verbatim"
    );
    assert!(manifest.redactions.is_empty());
    assert!(
        home.join(".codex/thread-writer-locks")
            .join(format!("{THREAD}.lock"))
            .is_file(),
        "capture leaves the lock file Codex expects"
    );
}

#[tokio::test]
async fn a_missing_prefix_breaks_the_lineage() {
    let src = build_source();
    std::fs::remove_file(&src.prefix).unwrap();
    let adapter = Codex::new(src.home.path().canonicalize().unwrap());
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::LineageBroken { ref missing, .. } if missing == PREFIX),
        "{err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn an_active_writer_blocks_capture() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let locks = home.join(".codex/thread-writer-locks");
    std::fs::create_dir_all(&locks).unwrap();
    let holder = std::fs::File::create(locks.join(format!("{THREAD}.lock"))).unwrap();
    holder.lock().unwrap();
    let adapter = Codex::new(home);
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::ActiveWriter { .. }), "{err}");
    holder.unlock().unwrap();
    adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_rollout_with_a_secret_is_refused_not_carried() {
    let src = build_source();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&src.head)
        .unwrap();
    file.write_all(
        b"{\"type\":\"response_item\",\"payload\":{\"text\":\"token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab\"}}\n",
    )
    .unwrap();
    drop(file);
    let adapter = Codex::new(src.home.path().canonicalize().unwrap());
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::SecretInState { line: 3, .. }),
        "{err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn restore_places_the_lineage_under_sessions_fabric_and_launch_resumes_there() {
    let src = build_source();
    let out = tempfile::tempdir().unwrap();
    let home_b = src.home.path().canonicalize().unwrap();
    Codex::new(home_b.clone())
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    // The other node has the same home and workspace path (ADR 0011) and
    // no Codex state.
    std::fs::rename(home_b.join(".codex"), home_b.join(".codex-departed")).unwrap();
    let work_b = src.work.clone();
    let bin = fake_codex(&home_b);
    let adapter = Codex::new(home_b.clone())
        .with_path_var(bin.clone())
        .with_overrides(vec!["sandbox_mode=\"workspace-write\"".to_owned()]);
    let installed = adapter.detect().await.unwrap().unwrap();
    assert_eq!(installed.version.as_deref(), Some("0.155.1"));

    let restored = adapter.restore(&work_b, out.path()).await.unwrap();
    let target = home_b.join(".codex/sessions/fabric").join(THREAD);
    assert_eq!(restored.placed.len(), 2);
    assert!(
        target
            .join(format!("rollout-2026-10-19T11-00-00-{THREAD}.jsonl"))
            .is_file()
    );
    assert!(
        target
            .join(format!("rollout-2026-10-18T09-00-00-{PREFIX}.jsonl.zst"))
            .is_file()
    );
    assert!(!home_b.join(".codex/auth.json").exists());
    assert_eq!(
        adapter.lineage(&thread()).unwrap().len(),
        2,
        "the restored lineage resolves on the target"
    );
    adapter.restore(&work_b, out.path()).await.unwrap();

    let log = home_b.join("log.txt");
    let env = vec![
        (
            EnvVarName::parse("HOME").unwrap(),
            home_b.to_string_lossy().into_owned(),
        ),
        (
            EnvVarName::parse("PATH").unwrap(),
            format!("{}:/usr/bin:/bin", bin.to_string_lossy()),
        ),
        (
            EnvVarName::parse("FAKE_LOG").unwrap(),
            log.to_string_lossy().into_owned(),
        ),
    ];
    let mut running = adapter
        .launch(
            &work_b,
            Launch::Resume {
                session: thread(),
                prompt: Some("reply with the secret word".to_owned()),
            },
            Io::Piped,
            &env,
        )
        .await
        .unwrap();
    assert!(running.child.wait().await.unwrap().success());
    let seen = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        seen,
        format!(
            "cwd={}\narg=exec\narg=resume\narg={THREAD}\narg=--json\narg=-c\narg=projects.\"{}\".trust_level=trusted\narg=-c\narg=sandbox_mode=\"workspace-write\"\narg=--\narg=reply with the secret word\n",
            work_b.display(),
            work_b.display()
        )
    );
}
