//! Crash tests: a capture or a restore is run in a child process that is
//! killed with SIGKILL part way through. After the kill, nothing half
//! written may pass for state: a manifest that exists describes a tree a
//! fresh node accepts, every placed file is whole, and running the same
//! operation again converges.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_harness::claude::ClaudeCode;
use fabric_harness::claude::slug::slug;
use fabric_harness::{Harness, SessionId, StateManifest};

const SID: &str = "7a1f2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const ROLE: &str = "FABRIC_CRASH_ROLE";
const HOME: &str = "FABRIC_CRASH_HOME";
const WORK: &str = "FABRIC_CRASH_WORK";
const OUT: &str = "FABRIC_CRASH_OUT";
const LINES: usize = 20_000;
const SIDECARS: usize = 6;

fn session() -> SessionId {
    SessionId::parse(SID).unwrap()
}

/// A home with one large transcript and several sidecars, big enough that
/// a capture or restore takes well over a second in a debug build.
fn build_home(home: &Path) -> PathBuf {
    let work = home.join("fabric/jobs/j1/work");
    std::fs::create_dir_all(&work).unwrap();
    let root = work.to_string_lossy().into_owned();
    let project = home.join(".claude/projects").join(slug(&work).unwrap());
    std::fs::create_dir_all(project.join(format!("{SID}/tool-results"))).unwrap();
    let mut transcript = String::new();
    for i in 0..LINES {
        writeln!(
            transcript,
            "{{\"type\":\"assistant\",\"cwd\":\"{root}\",\"n\":{i},\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"edit {root}/src/file_{i}.rs then run cargo test in {root}; note ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab\"}},{{\"type\":\"thinking\",\"thinking\":\"look at {root}/src/file_{i}.rs\",\"signature\":\"sig{i}\"}}]}}}}"
        )
        .unwrap();
    }
    std::fs::write(project.join(format!("{SID}.jsonl")), transcript).unwrap();
    for k in 0..SIDECARS {
        let body = format!("output {k} from {root}\n").repeat(20_000);
        std::fs::write(project.join(format!("{SID}/tool-results/{k}.txt")), body).unwrap();
    }
    work
}

fn run_child(role: &str, home: &Path, work: &Path, out: &Path, kill_after: Duration) -> bool {
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(HOME, home)
        .env(WORK, work)
        .env(OUT, out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + kill_after;
    while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let still_running = child.try_wait().unwrap().is_none();
    if still_running {
        child.kill().unwrap();
    }
    child.wait().unwrap();
    still_running
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
    let adapter = ClaudeCode::new(home);
    runtime.block_on(async {
        match role.as_str() {
            "capture" => {
                adapter.capture(&work, &session(), &out).await.unwrap();
            }
            "restore" => {
                adapter.restore(&work, &out).await.unwrap();
            }
            other => panic!("unknown role {other}"),
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_killed_part_way_leaves_no_tree_a_fresh_node_would_accept_by_mistake() {
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path().canonicalize().unwrap();
    let work = build_home(&home);
    let mut killed_mid_way = 0;
    let mut finished = 0;
    // The last delay lets the capture finish, so the manifest branch runs.
    for delay_ms in [5, 30, 120, 400, 1200, 15_000] {
        let out = tempfile::tempdir().unwrap();
        if run_child(
            "capture",
            &home,
            &work,
            out.path(),
            Duration::from_millis(delay_ms),
        ) {
            killed_mid_way += 1;
        }
        // A fresh node: same home and workspace path, no Claude state.
        let source_state = home.join(".claude-source");
        std::fs::rename(home.join(".claude"), &source_state).unwrap();
        let restore = ClaudeCode::new(home.clone())
            .restore(&work, out.path())
            .await;
        if out.path().join("manifest.json").exists() {
            // The manifest is the last write; everything it lists is whole.
            restore.unwrap();
            let manifest = StateManifest::read(out.path()).unwrap();
            assert_eq!(manifest.files.len(), SIDECARS + 1);
            std::fs::remove_dir_all(home.join(".claude")).unwrap();
            finished += 1;
        } else {
            assert!(restore.is_err(), "no manifest, so nothing to restore");
            assert!(!home.join(".claude").exists());
        }
        std::fs::rename(&source_state, home.join(".claude")).unwrap();
        // Capturing again into a fresh directory is unaffected by what was left.
        let again = tempfile::tempdir().unwrap();
        ClaudeCode::new(home.clone())
            .capture(&work, &session(), again.path())
            .await
            .unwrap();
    }
    assert!(
        killed_mid_way > 0,
        "no child was still running when killed; the fixture is too small"
    );
    assert!(
        finished > 0,
        "no capture finished; the manifest branch never ran"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_killed_part_way_places_only_whole_files_and_converges() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().canonicalize().unwrap();
    let work = build_home(&source);
    let out = tempfile::tempdir().unwrap();
    ClaudeCode::new(source.clone())
        .capture(&work, &session(), out.path())
        .await
        .unwrap();

    // A fresh node: same home and workspace path, no Claude state.
    std::fs::rename(source.join(".claude"), source.join(".claude-source")).unwrap();
    let target = source.clone();
    let target_work = work.clone();
    let adapter = ClaudeCode::new(target.clone());
    let mut killed_mid_way = 0;
    for delay_ms in [5, 30, 120, 400, 1200] {
        if run_child(
            "restore",
            &target,
            &target_work,
            out.path(),
            Duration::from_millis(delay_ms),
        ) {
            killed_mid_way += 1;
        }
        let after_kill = regular_files(&target.join(".claude"));
        adapter.restore(&target_work, out.path()).await.unwrap();
        let converged = regular_files(&target.join(".claude"));
        for (path, bytes) in &after_kill {
            if is_temp(path) {
                continue;
            }
            assert_eq!(
                converged.get(path),
                Some(bytes),
                "{} was not whole after the kill",
                path.display()
            );
        }
        assert_eq!(
            converged.keys().filter(|p| !is_temp(p)).count(),
            SIDECARS + 1
        );
    }
    assert!(
        killed_mid_way > 0,
        "no child was still running when killed; the fixture is too small"
    );
    // A temporary file left by a killed restore is never carried onward.
    let again = tempfile::tempdir().unwrap();
    let manifest = adapter
        .capture(&target_work, &session(), again.path())
        .await
        .unwrap();
    assert!(
        manifest
            .files
            .iter()
            .all(|f| !f.rel_path.as_str().contains(".fabric-tmp-"))
    );
    assert_eq!(manifest.files.len(), SIDECARS + 1);
}
