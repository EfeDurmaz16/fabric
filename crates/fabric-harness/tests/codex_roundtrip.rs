//! Codex adapter: a paginated lineage with a compressed prefix is captured
//! under Codex's real writer lock, restored onto a node with the same home
//! and workspace path but no Codex state, under `sessions/fabric/<thread>/`,
//! brought back over the original, and a fake `codex` is launched there.
//! Rollouts that share an id, rollouts that hold a secret, manifest paths
//! that are not rollouts and an active writer are refused.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use fabric_core::env::EnvVarName;
use fabric_core::path::AbsPath;
use fabric_harness::codex::Codex;
use fabric_harness::codex::overrides::{ApprovalPolicy, CodexOverrides, SandboxMode};
use fabric_harness::manifest::StateManifest;
use fabric_harness::{Harness, HarnessError, Launch, SessionId};

const THREAD: &str = "019f233e-bb33-7342-85fa-f2d33b14368a";
const PREFIX: &str = "019f1111-bb33-7342-85fa-f2d33b14368b";
const HEAD_NAME: &str = "rollout-2026-10-19T11-00-00-019f233e-bb33-7342-85fa-f2d33b14368a.jsonl";
const PREFIX_NAME: &str =
    "rollout-2026-10-18T09-00-00-019f1111-bb33-7342-85fa-f2d33b14368b.jsonl.zst";

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

fn turn(text: &str) -> String {
    format!(
        "{{\"timestamp\":\"2026-10-19T12:00:00.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"{text}\"}}]}}}}\n"
    )
}

struct Fixture {
    home: tempfile::TempDir,
    work: PathBuf,
    head: PathBuf,
    prefix: PathBuf,
}

fn build_source() -> Fixture {
    build_source_at("fabric/jobs/j1/work")
}

fn build_source_at(work_rel: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let work = home_path.join(work_rel);
    std::fs::create_dir_all(&work).unwrap();
    let codex = home_path.join(".codex");
    let day = codex.join("sessions/2026/10/19");
    std::fs::create_dir_all(&day).unwrap();
    let head = day.join(HEAD_NAME);
    std::fs::write(
        &head,
        session_meta(THREAD, &work.to_string_lossy(), Some(PREFIX)),
    )
    .unwrap();
    let archived = codex.join("archived_sessions");
    std::fs::create_dir_all(&archived).unwrap();
    let prefix = archived.join(PREFIX_NAME);
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

/// The version the adapter's writer lock rule was checked on.
const LOCKING_VERSION: &str = "0.155.1";

/// Writes a fake `codex` under `dir/bin` that reports `version`, stays
/// alive when its first argument is `hold`, and otherwise logs its working
/// directory and arguments to `$FAKE_LOG`; returns the script and a `PATH`
/// value that finds it.
fn fake_codex_reporting(dir: &Path, version: &str) -> (PathBuf, OsString) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("codex");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) echo \"codex-cli {version}\"; exit 0 ;;\n  hold) sleep 60; exit 0 ;;\nesac\n{{ echo \"cwd=$(pwd -P)\"; for a in \"$@\"; do echo \"arg=$a\"; done; }} > \"$FAKE_LOG\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, bin.into_os_string())
}

fn fake_codex(dir: &Path) -> OsString {
    fake_codex_reporting(dir, LOCKING_VERSION).1
}

/// The adapter for `home`, with a fake `codex` that takes the writer lock.
fn node(home: &Path) -> Codex {
    Codex::new(abs(home), fake_codex(home))
}

fn abs(path: &Path) -> AbsPath {
    AbsPath::parse(path.to_str().unwrap()).unwrap()
}

fn thread() -> SessionId {
    SessionId::parse(THREAD).unwrap()
}

fn lock_dir(home: &Path) -> PathBuf {
    home.join(".codex/thread-writer-locks")
}

#[tokio::test]
async fn capture_follows_the_lineage_and_carries_nothing_else() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    assert_eq!(adapter.latest_session(&src.work).unwrap(), Some(thread()));

    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    let paths: Vec<&str> = manifest
        .files()
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            format!("home/.codex/archived_sessions/{PREFIX_NAME}").as_str(),
            format!("home/.codex/sessions/2026/10/19/{HEAD_NAME}").as_str(),
        ]
    );
    assert_eq!(
        std::fs::read(out.path().join(paths[1])).unwrap(),
        std::fs::read(&src.head).unwrap(),
        "rollout bytes travel verbatim"
    );
    assert!(manifest.redactions().is_empty());
    let locks = lock_dir(&home);
    assert!(
        locks.join(".coordination.lock").is_file(),
        "the coordination lock lives where Codex keeps it"
    );
    assert!(
        !locks.join(format!("{THREAD}.lock")).exists(),
        "the thread lock file is removed on release, as Codex does"
    );
    assert!(!home.join(".codex/.coordination.lock").exists());
}

#[tokio::test]
async fn a_missing_prefix_breaks_the_lineage() {
    let src = build_source();
    std::fs::remove_file(&src.prefix).unwrap();
    let adapter = node(&src.home.path().canonicalize().unwrap());
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::LineageBroken { ref missing, .. } if missing.as_str() == PREFIX),
        "{err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn an_active_writer_blocks_capture_and_restore() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let locks = lock_dir(&home);
    std::fs::create_dir_all(&locks).unwrap();
    let holder = std::fs::File::create(locks.join(format!("{THREAD}.lock"))).unwrap();
    holder.lock().unwrap();
    let adapter = node(&home);
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
    // The capture removed the lock file on release, as Codex does; a new
    // writer opens the path afresh, as Codex's `acquire` does.
    drop(holder);
    let holder = std::fs::File::create(locks.join(format!("{THREAD}.lock"))).unwrap();
    holder.lock().unwrap();
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(matches!(err, HarnessError::ActiveWriter { .. }), "{err}");
    holder.unlock().unwrap();
    adapter.restore(&src.work, out.path()).await.unwrap();
}

/// A running fake `codex`, killed with its process group on drop.
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
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.child.id())])
            .status();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn a_running_codex_too_old_for_the_writer_lock_blocks_capture_and_restore() {
    // A Codex older than the lock rule takes no thread lock, so the lock
    // being free says nothing; the running process must block instead.
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    node(&home)
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    let (script, path_var) = fake_codex_reporting(&home, "0.150.0");
    let adapter = Codex::new(abs(&home), path_var);
    let held = Held::start(&script);
    let pid = held.child.id();
    let again = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), again.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessRunning { ref pids, .. } if pids.contains(&pid)),
        "{err}"
    );
    assert!(!again.path().join("manifest.json").exists());
    let before = std::fs::read(&src.head).unwrap();
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessRunning { ref pids, .. } if pids.contains(&pid)),
        "{err}"
    );
    assert_eq!(std::fs::read(&src.head).unwrap(), before);
}

#[tokio::test]
async fn a_rollout_with_a_secret_is_refused_not_carried() {
    let src = build_source();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&src.head)
        .unwrap();
    file.write_all(turn("token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab").as_bytes())
        .unwrap();
    drop(file);
    let adapter = node(&src.home.path().canonicalize().unwrap());
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

/// Appends a turn whose JSON text is `escaped` to the head rollout and
/// asserts that capture refuses it at that line.
async fn assert_refused_with_turn(escaped: &str) {
    let src = build_source();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&src.head)
        .unwrap();
    file.write_all(turn(escaped).as_bytes()).unwrap();
    drop(file);
    let adapter = node(&src.home.path().canonicalize().unwrap());
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::SecretInState { line: 3, .. }),
        "{escaped}: {err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn an_env_line_after_an_escaped_newline_is_refused() {
    // `cat .env` output as a rollout stores it: `\n` between the lines.
    assert_refused_with_turn(r"NODE_ENV=dev\nDB_PASSWORD=hunter22xyz\nPORT=1").await;
}

#[tokio::test]
async fn a_value_in_escaped_quotes_is_refused() {
    assert_refused_with_turn(r#"export DB_PASSWORD=\"hunter22xyz\""#).await;
}

#[tokio::test]
async fn a_workspace_whose_name_holds_sk_inside_a_word_is_captured() {
    // `disk-usage` holds `sk-`; the rollout's `cwd` and its first message
    // name the path, and neither is a key.
    let src = build_source_at("oss/disk-usage-analyzer-service");
    let adapter = node(&src.home.path().canonicalize().unwrap());
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    assert_eq!(manifest.files().len(), 2);
    assert!(manifest.redactions().is_empty());
}

#[tokio::test]
async fn two_files_for_one_rollout_resolve_by_prefix_or_are_refused() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    let copy_dir = home.join(".codex/sessions/fabric").join(THREAD);
    std::fs::create_dir_all(&copy_dir).unwrap();
    let copy = copy_dir.join(HEAD_NAME);
    let mut longer = std::fs::read_to_string(&src.head).unwrap();
    longer.push_str(&turn("more"));
    std::fs::write(&copy, &longer).unwrap();
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    let head = format!("home/.codex/sessions/fabric/{THREAD}/{HEAD_NAME}");
    assert!(
        manifest.files().iter().any(|f| f.rel_path.as_str() == head),
        "the longer file wins over its prefix"
    );
    std::fs::write(&copy, session_meta(THREAD, "/other", Some(PREFIX))).unwrap();
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::LineageAmbiguous { ref paths, .. } if paths.len() == 2),
        "{err}"
    );
}

#[tokio::test]
async fn a_forked_rollout_of_another_thread_blocks_nothing() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let other = "019f9999-bb33-7342-85fa-f2d33b149999";
    let name = format!("rollout-2026-10-17T08-00-00-{other}.jsonl");
    for (dir, cwd) in [("2026/10/17", "/a"), ("fabric/x", "/b")] {
        let path = home.join(".codex/sessions").join(dir).join(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, session_meta(other, cwd, None)).unwrap();
    }
    let adapter = node(&home);
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    assert_eq!(manifest.files().len(), 2);
    adapter.restore(&src.work, out.path()).await.unwrap();
}

/// Writes `manifest` under `out` with one more entry, for `body` at `rel`,
/// as someone who forged the tree would.
fn forge(out: &Path, manifest: &StateManifest, rel: &str, body: &[u8]) {
    forge_as(out, manifest, rel, body, "Replaceable");
}

/// `forge` with the mutability the entry claims.
fn forge_as(out: &Path, manifest: &StateManifest, rel: &str, body: &[u8], mutability: &str) {
    let mut value = serde_json::to_value(manifest).unwrap();
    let files = value["files"].as_array_mut().unwrap();
    files.push(serde_json::json!({
        "rel_path": rel,
        "content": "Opaque",
        "mutability": mutability,
        "size": body.len(),
        "blake3": blake3::hash(body).to_hex().to_string(),
    }));
    files.sort_by(|a, b| a["rel_path"].as_str().cmp(&b["rel_path"].as_str()));
    std::fs::write(out.join("manifest.json"), value.to_string()).unwrap();
}

/// Moves this node's Codex state aside, as if the job ran on another node
/// with the same home path; returns where it went.
fn set_aside(home: &Path, name: &str) -> PathBuf {
    let aside = home.join(name);
    std::fs::rename(home.join(".codex"), &aside).unwrap();
    aside
}

#[tokio::test]
async fn restore_places_the_lineage_under_sessions_fabric_and_launch_resumes_there() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let out = tempfile::tempdir().unwrap();
    let manifest = node(&home)
        .capture(&src.work, &thread(), out.path())
        .await
        .unwrap();
    set_aside(&home, ".codex-pro");

    let bin = fake_codex(&home);
    let adapter = Codex::new(abs(&home), bin.clone()).with_overrides(CodexOverrides {
        sandbox: SandboxMode::WorkspaceWrite,
        approval: ApprovalPolicy::Never,
        model: None,
        effort: None,
    });
    let installed = adapter.detect().await.unwrap().unwrap();
    assert_eq!(installed.version.as_deref(), Some("0.155.1"));

    // A manifest path that is not a rollout is refused before any write.
    let rel = "home/.codex/skills/evil.md";
    let staged = out.path().join(rel);
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, b"payload").unwrap();
    forge(out.path(), &manifest, rel, b"payload");
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(matches!(err, HarnessError::UnexpectedFile { .. }), "{err}");
    assert!(!home.join(".codex/sessions").exists());

    // So is a well-formed rollout of another thread, off this lineage.
    let other = "019f9999-bb33-7342-85fa-f2d33b149999";
    let rel = format!("home/.codex/sessions/2026/10/17/rollout-2026-10-17T08-00-00-{other}.jsonl");
    let body = session_meta(other, "/a", None);
    let staged = out.path().join(&rel);
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, &body).unwrap();
    forge_as(out.path(), &manifest, &rel, body.as_bytes(), "AppendOnly");
    let err = adapter.restore(&src.work, out.path()).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::UnexpectedFile { ref path, .. } if *path == rel),
        "{err}"
    );
    assert!(!home.join(".codex/sessions").exists());
    manifest.write(out.path()).unwrap();

    let restored = adapter.restore(&src.work, out.path()).await.unwrap();
    let target = home.join(".codex/sessions/fabric").join(THREAD);
    assert_eq!(restored.placed.len(), 2);
    assert!(target.join(HEAD_NAME).is_file());
    assert!(target.join(PREFIX_NAME).is_file());
    assert!(!home.join(".codex/auth.json").exists());
    let again = tempfile::tempdir().unwrap();
    assert_eq!(
        adapter
            .capture(&src.work, &thread(), again.path())
            .await
            .unwrap()
            .files()
            .len(),
        2,
        "the restored lineage resolves on the target"
    );
    adapter.restore(&src.work, out.path()).await.unwrap();

    let log = home.join("log.txt");
    let env = vec![
        (
            EnvVarName::parse("HOME").unwrap(),
            home.to_string_lossy().into_owned(),
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
            &src.work,
            Launch::Headless {
                resume: Some(thread()),
                prompt: "reply with the secret word".to_owned(),
            },
            &env,
        )
        .await
        .unwrap();
    assert!(running.child_mut().wait().await.unwrap().success());
    let seen = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        seen,
        format!(
            "cwd={}\narg=exec\narg=resume\narg={THREAD}\narg=--json\narg=-c\narg=projects.\"{}\".trust_level=trusted\narg=-c\narg=sandbox_mode=\"workspace-write\"\narg=-c\narg=approval_policy=\"never\"\narg=--\narg=reply with the secret word\n",
            src.work.display(),
            src.work.display()
        )
    );
}

#[tokio::test]
async fn a_thread_that_comes_back_lands_over_its_original_file() {
    let src = build_source();
    let home = src.home.path().canonicalize().unwrap();
    let adapter = node(&home);
    let out_1 = tempfile::tempdir().unwrap();
    adapter
        .capture(&src.work, &thread(), out_1.path())
        .await
        .unwrap();
    let pro = set_aside(&home, ".codex-pro");

    adapter.restore(&src.work, out_1.path()).await.unwrap();
    let head_air = home
        .join(".codex/sessions/fabric")
        .join(THREAD)
        .join(HEAD_NAME);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&head_air)
        .unwrap();
    file.write_all(turn("done on the air").as_bytes()).unwrap();
    drop(file);
    let out_2 = tempfile::tempdir().unwrap();
    adapter
        .capture(&src.work, &thread(), out_2.path())
        .await
        .unwrap();
    set_aside(&home, ".codex-air");
    std::fs::rename(&pro, home.join(".codex")).unwrap();

    let restored = adapter.restore(&src.work, out_2.path()).await.unwrap();
    assert!(
        restored.placed.contains(&src.head),
        "the head lands over the original, not beside it: {:?}",
        restored.placed
    );
    assert!(restored.placed.contains(&src.prefix));
    assert!(
        !home.join(".codex/sessions/fabric").exists(),
        "no second copy of the thread was created"
    );
    assert!(
        std::fs::read_to_string(&src.head)
            .unwrap()
            .ends_with(&turn("done on the air"))
    );
}

#[test]
fn the_codex_home_must_be_under_the_home() {
    let err = Codex::new(AbsPath::parse("/h").unwrap(), OsString::new())
        .with_codex_home(&AbsPath::parse("/elsewhere/.codex").unwrap())
        .unwrap_err();
    assert!(matches!(err, HarnessError::NotUnderHome { .. }), "{err}");
    assert!(
        Codex::new(AbsPath::parse("/h").unwrap(), OsString::new())
            .with_codex_home(&AbsPath::parse("/h/custom").unwrap())
            .is_ok()
    );
}
