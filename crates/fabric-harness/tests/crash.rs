//! Crash tests: a capture or a restore runs in a child process that the
//! `crashpoints` feature aborts at a named step, after the Nth file it
//! stages or places. After the abort nothing half written may pass for
//! state: no manifest is left that a fresh node would accept, every placed
//! file is whole, and running the same operation again converges. A
//! process that dies between a rename and the directory fsync loses
//! nothing, because the page cache outlives it, so that window needs a
//! power cut, not a kill, and is not tested here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

use fabric_harness::claude::ClaudeCode;
use fabric_harness::codex::Codex;
use fabric_harness::manifest::StateManifest;
use fabric_harness::{Harness, SessionId};

mod common;
use common::{fake_claude, project_key};

const SID: &str = "7a1f2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const THREAD: &str = "019f233e-bb33-7342-85fa-f2d33b14368a";
const PREFIX: &str = "019f1111-bb33-7342-85fa-f2d33b14368b";
const ROLE: &str = "FABRIC_CRASH_ROLE";
const HOME: &str = "FABRIC_CRASH_HOME";
const WORK: &str = "FABRIC_CRASH_WORK";
const OUT: &str = "FABRIC_CRASH_OUT";
const CRASHPOINT: &str = "FABRIC_CRASHPOINT";
const SIDECARS: usize = 3;
/// The transcript and its sidecars.
const FILES: usize = SIDECARS + 1;

fn session() -> SessionId {
    SessionId::parse(SID).unwrap()
}

/// The adapter for `home`, finding the fake `claude` kept under it.
fn node(home: &Path) -> ClaudeCode {
    let (_, path_var) = fake_claude(home);
    ClaudeCode::new(home.to_path_buf(), path_var)
}

/// Moves `.claude` to `aside` and leaves a node where Claude is installed
/// and idle: an empty session registry and no session state.
fn fresh_node(home: &Path, aside: &Path) {
    std::fs::rename(home.join(".claude"), aside).unwrap();
    std::fs::create_dir_all(home.join(".claude/sessions")).unwrap();
}

fn bring_back(home: &Path, aside: &Path) {
    std::fs::remove_dir_all(home.join(".claude")).unwrap();
    std::fs::rename(aside, home.join(".claude")).unwrap();
}

/// A home with one transcript, which holds secrets and signed blocks, and
/// a few text sidecars.
fn build_home(home: &Path) -> PathBuf {
    let work = home.join("fabric/jobs/j1/work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(home.join(".claude/sessions")).unwrap();
    let root = work.to_string_lossy().into_owned();
    let project = home.join(".claude/projects").join(project_key(&work));
    std::fs::create_dir_all(project.join(format!("{SID}/tool-results"))).unwrap();
    let mut transcript = String::new();
    for i in 0..50 {
        writeln!(
            transcript,
            "{{\"type\":\"assistant\",\"cwd\":\"{root}\",\"n\":{i},\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"note ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab\"}},{{\"type\":\"thinking\",\"thinking\":\"look at {root}/src/file_{i}.rs\",\"signature\":\"sig{i}\"}}]}}}}"
        )
        .unwrap();
    }
    std::fs::write(project.join(format!("{SID}.jsonl")), transcript).unwrap();
    for k in 0..SIDECARS {
        let body = format!("output {k} from {root}\n").repeat(100);
        std::fs::write(project.join(format!("{SID}/tool-results/{k}.txt")), body).unwrap();
    }
    work
}

fn transcript_of(home: &Path, work: &Path) -> PathBuf {
    home.join(".claude/projects")
        .join(project_key(work))
        .join(format!("{SID}.jsonl"))
}

fn session_meta(id: &str, cwd: &str, history_base: Option<&str>) -> String {
    let base = history_base.map_or_else(String::new, |b| {
        format!(
            ",\"history_base\":{{\"thread_id\":\"{b}\",\"end_ordinal_exclusive\":2,\"end_byte_offset\":120}}"
        )
    });
    format!(
        "{{\"timestamp\":\"2026-10-19T10:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"session_id\":\"{id}\",\"timestamp\":\"2026-10-19T10:00:00.000Z\",\"cwd\":\"{cwd}\",\"originator\":\"codex_cli_rs\",\"cli_version\":\"0.155.1\"{base}}}}}\n"
    )
}

/// A Codex home with a two-rollout lineage; returns the workspace.
fn build_codex_home(home: &Path) -> PathBuf {
    let work = home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    let cwd = work.to_string_lossy();
    let sessions = home.join(".codex/sessions/2026/10");
    std::fs::create_dir_all(sessions.join("18")).unwrap();
    std::fs::create_dir_all(sessions.join("19")).unwrap();
    std::fs::write(
        sessions.join(format!("18/rollout-2026-10-18T09-00-00-{PREFIX}.jsonl")),
        session_meta(PREFIX, &cwd, None),
    )
    .unwrap();
    std::fs::write(
        sessions.join(format!("19/rollout-2026-10-19T11-00-00-{THREAD}.jsonl")),
        session_meta(THREAD, &cwd, Some(PREFIX)),
    )
    .unwrap();
    work
}

/// Runs `role` in a child process with `crashpoint` set; returns true when
/// the child aborted (SIGABRT), which only the crash point does.
fn run_child(role: &str, home: &Path, work: &Path, out: &Path, crashpoint: &str) -> bool {
    let exe = std::env::current_exe().unwrap();
    let status = std::process::Command::new(exe)
        .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(HOME, home)
        .env(WORK, work)
        .env(OUT, out)
        .env(CRASHPOINT, crashpoint)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    status.signal() == Some(6)
}

/// The child's body; a no-op unless the parent set the role.
#[test]
fn crash_child() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let home = PathBuf::from(std::env::var_os(HOME).unwrap());
    let work = PathBuf::from(std::env::var_os(WORK).unwrap());
    let out = PathBuf::from(std::env::var_os(OUT).unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        match role.as_str() {
            "capture" => {
                node(&home).capture(&work, &session(), &out).await.unwrap();
            }
            "restore" => {
                node(&home).restore(&work, &out).await.unwrap();
            }
            "codex-restore" => {
                Codex::new(home, OsString::new())
                    .restore(&work, &out)
                    .await
                    .unwrap();
            }
            other => panic!("unknown role {other}"),
        }
    });
}

fn regular_files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

fn is_temp(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains(".fabric-tmp-"))
}

/// Every staged file of the capture in `out`, by the place restore puts it
/// under `home`.
fn staged_by_destination(home: &Path, out: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    StateManifest::read(out)
        .unwrap()
        .files()
        .iter()
        .map(|f| {
            let rel = f.rel_path.as_str();
            let dest = home.join(rel.strip_prefix("home/").unwrap());
            (dest, std::fs::read(out.join(rel)).unwrap())
        })
        .collect()
}

#[tokio::test]
async fn a_capture_aborted_after_any_staged_file_leaves_no_manifest() {
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path().canonicalize().unwrap();
    let work = build_home(&home);
    let transcript = transcript_of(&home, &work);
    for index in 0..FILES {
        // The directory already holds a complete earlier capture, and the
        // session has grown since, so the new capture replaces its files.
        let out = tempfile::tempdir().unwrap();
        node(&home)
            .capture(&work, &session(), out.path())
            .await
            .unwrap();
        let mut grown = std::fs::read(&transcript).unwrap();
        grown.extend_from_slice(b"{\"type\":\"user\",\"n\":-1}\n");
        std::fs::write(&transcript, grown).unwrap();

        let point = format!("stage-after-file:{index}");
        assert!(run_child("capture", &home, &work, out.path(), &point));
        assert!(
            !out.path().join("manifest.json").exists(),
            "after {point} no manifest, old or new, is left"
        );

        let source_state = home.join(".claude-source");
        fresh_node(&home, &source_state);
        assert!(node(&home).restore(&work, out.path()).await.is_err());
        assert!(!home.join(".claude/projects").exists());
        bring_back(&home, &source_state);

        // Capturing again into the same directory converges.
        node(&home)
            .capture(&work, &session(), out.path())
            .await
            .unwrap();
        fresh_node(&home, &source_state);
        node(&home).restore(&work, out.path()).await.unwrap();
        bring_back(&home, &source_state);
    }
}

#[tokio::test]
async fn a_restore_aborted_after_any_placed_file_leaves_whole_files_and_converges() {
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path().canonicalize().unwrap();
    let work = build_home(&home);
    let out = tempfile::tempdir().unwrap();
    node(&home)
        .capture(&work, &session(), out.path())
        .await
        .unwrap();
    fresh_node(&home, &home.join(".claude-source"));
    let staged = staged_by_destination(&home, out.path());
    assert_eq!(staged.len(), FILES);
    let projects = home.join(".claude/projects");
    for index in 0..FILES {
        if projects.exists() {
            std::fs::remove_dir_all(&projects).unwrap();
        }
        let point = format!("place-after-file:{index}");
        assert!(run_child("restore", &home, &work, out.path(), &point));
        let after_abort: BTreeMap<PathBuf, Vec<u8>> = regular_files(&projects)
            .into_iter()
            .filter(|(p, _)| !is_temp(p))
            .collect();
        assert_eq!(
            after_abort.len(),
            index + 1,
            "after {point} exactly the files up to it are placed"
        );
        for (path, bytes) in &after_abort {
            assert_eq!(
                staged.get(path),
                Some(bytes),
                "{} is not whole after {point}",
                path.display()
            );
        }
        node(&home).restore(&work, out.path()).await.unwrap();
        let converged: BTreeMap<PathBuf, Vec<u8>> = regular_files(&projects)
            .into_iter()
            .filter(|(p, _)| !is_temp(p))
            .collect();
        assert_eq!(converged, staged);
    }
}

#[tokio::test]
async fn a_codex_restore_aborted_mid_way_frees_the_writer_lock_and_converges() {
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path().canonicalize().unwrap();
    let work = build_codex_home(&home);
    let thread = SessionId::parse(THREAD).unwrap();
    let out = tempfile::tempdir().unwrap();
    let adapter = Codex::new(home.clone(), OsString::new());
    adapter.capture(&work, &thread, out.path()).await.unwrap();
    let source = regular_files(&home.join(".codex/sessions"));
    std::fs::rename(home.join(".codex"), home.join(".codex-source")).unwrap();

    assert!(run_child(
        "codex-restore",
        &home,
        &work,
        out.path(),
        "place-after-file:0"
    ));
    let target = home.join(".codex/sessions/fabric").join(THREAD);
    let placed = regular_files(&target)
        .into_keys()
        .filter(|p| !is_temp(p))
        .count();
    assert_eq!(placed, 1, "only the first rollout was placed");

    // The aborted process held Codex's thread lock; its death released it.
    let restored = adapter.restore(&work, out.path()).await.unwrap();
    assert_eq!(restored.placed.len(), 2);
    for path in &restored.placed {
        let original = source
            .iter()
            .find(|(p, _)| p.file_name() == path.file_name())
            .unwrap()
            .1;
        assert_eq!(&std::fs::read(path).unwrap(), original);
    }
}
