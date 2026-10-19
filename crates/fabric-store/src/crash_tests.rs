//! Crash tests. A child re-executes this test binary, stops at a named
//! checkpoint inside publish or restore, and is killed with `SIGKILL`; the
//! parent then asserts what ADR 0004 promises about the store and the target.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use fabric_core::id::{JobId, NodeId, SnapshotId};

use crate::capture::{CaptureRequest, UnmeasuredKeys, capture};
use crate::git::Git;
use crate::objects::ChunkParams;
use crate::policy::IgnorePolicy;
use crate::restore::{
    Durability, RestoreRequest, RestoreState, Verify, restore, restore_state, verify,
};
use crate::store::{CheckDepth, Store};

const JOB: JobId = JobId::from_bytes([7; 16]);
const NODE: NodeId = NodeId::from_bytes([1; 32]);
const CREATED_AT: u64 = 1_760_000_000_000;

/// Makes a repository with staged, unstaged, untracked, ignored and cache files.
fn make_workspace(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("node_modules/pkg"))?;
    fs::write(root.join("src/main.rs"), "fn main() {}\n")?;
    fs::write(root.join(".gitignore"), "*.log\nnode_modules/\n")?;
    let git = Git::new(root, CREATED_AT);
    git.run(&["init", "-q", "-b", "main"])?;
    git.run(&["add", "-A"])?;
    git.run(&["commit", "-q", "-m", "one"])?;
    fs::write(root.join("src/main.rs"), "fn main() { staged(); }\n")?;
    git.run(&["add", "src/main.rs"])?;
    fs::write(
        root.join("src/main.rs"),
        "fn main() { staged(); unstaged(); }\n",
    )?;
    fs::write(root.join("notes.txt"), "untracked\n")?;
    fs::write(root.join("build.log"), "ignored precious\n")?;
    for i in 0..40 {
        fs::write(
            root.join(format!("node_modules/pkg/file{i}.js")),
            format!("module.exports = {i};\n"),
        )?;
    }
    Ok(())
}

fn capture_and_publish(
    store: &mut Store,
    root: &Path,
) -> Result<SnapshotId, Box<dyn std::error::Error>> {
    let policy = IgnorePolicy::default();
    let captured = capture(
        store,
        &CaptureRequest {
            root,
            job_id: JOB,
            created_by: NODE,
            created_at: CREATED_AT,
            parent: None,
            policy: &policy,
            carry_keys: &UnmeasuredKeys,
            harness_state: None,
            env_spec: None,
            max_file_size: None,
            allow_large_untracked: false,
        },
    )?;
    Ok(store.publish(&captured.manifest)?)
}

/// The child side: runs the operation named by the environment until the checkpoint stops it.
#[test]
fn crash_child() {
    let Ok(mode) = std::env::var("FABRIC_STORE_CRASH_MODE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("FABRIC_STORE_CRASH_DIR").unwrap());
    let mut store = Store::open(&dir.join("store")).unwrap();
    match mode.as_str() {
        "publish" => {
            capture_and_publish(&mut store, &dir.join("work")).unwrap();
        }
        "restore" => {
            let snapshot: SnapshotId = std::env::var("FABRIC_STORE_CRASH_SNAPSHOT")
                .unwrap()
                .parse()
                .unwrap();
            restore(
                &store,
                &RestoreRequest {
                    snapshot,
                    target: &dir.join("target"),
                    harness_state: None,
                    verify: Verify::None,
                    durability: Durability::ProcessCrash,
                    carry: &|_| true,
                },
            )
            .unwrap();
        }
        other => panic!("unknown mode {other}"),
    }
    panic!("child did not reach its checkpoint");
}

/// Runs the child until it reaches `checkpoint`, then kills it with SIGKILL.
fn run_child_and_kill(dir: &Path, mode: &str, checkpoint: &str, snapshot: Option<SnapshotId>) {
    let signal = dir.join(format!("signal-{checkpoint}"));
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "crash_tests::crash_child", "--nocapture"])
        .env("FABRIC_STORE_CRASH_MODE", mode)
        .env("FABRIC_STORE_CRASH_DIR", dir)
        .env("FABRIC_STORE_CRASH_AT", checkpoint)
        .env("FABRIC_STORE_CRASH_SIGNAL", &signal)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(snapshot) = snapshot {
        cmd.env("FABRIC_STORE_CRASH_SNAPSHOT", snapshot.to_string());
    }
    let mut child = cmd.spawn().unwrap();
    let started = Instant::now();
    loop {
        if signal.exists() {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "{checkpoint}: child exited with {status} before the checkpoint\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            started.elapsed() < Duration::from_mins(2),
            "{checkpoint}: child did not reach the checkpoint in time"
        );
        thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "{checkpoint}: child was not killed");
}

#[test]
fn publish_killed_at_any_checkpoint_never_leaves_a_dangling_row() {
    for checkpoint in [
        "publish.after_objects",
        "publish.after_fsync",
        "publish.before_commit",
        "publish.after_commit",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        make_workspace(&dir.join("work")).unwrap();
        drop(Store::create(&dir.join("store"), ChunkParams::DEFAULT).unwrap());
        run_child_and_kill(dir, "publish", checkpoint, None);

        let mut store = Store::open(&dir.join("store")).unwrap();
        let committed = checkpoint == "publish.after_commit";
        let report = store.check(CheckDepth::Hashes).unwrap();
        assert!(report.is_clean(), "{checkpoint}: {report:?}");
        assert_eq!(report.snapshots, u64::from(committed), "{checkpoint}");

        let gc = store.gc(SystemTime::now(), Duration::ZERO).unwrap();
        assert_eq!(gc.removed_chunks, 0, "{checkpoint}");
        assert_eq!(gc.removed_orphans > 0, !committed, "{checkpoint}: {gc:?}");

        let snapshot = capture_and_publish(&mut store, &dir.join("work")).unwrap();
        let report = store.check(CheckDepth::Hashes).unwrap();
        assert!(report.is_clean(), "{checkpoint}: {report:?}");
        assert!(store.manifest(snapshot).is_ok());
    }
}

#[test]
fn restore_killed_at_any_checkpoint_is_detectable_and_finishes_on_rerun() {
    for checkpoint in [
        "restore.after_git",
        "restore.after_first_file",
        "restore.after_files",
        "restore.before_rename",
        "restore.after_rename",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        make_workspace(&dir.join("work")).unwrap();
        let snapshot = {
            let mut store = Store::create(&dir.join("store"), ChunkParams::DEFAULT).unwrap();
            capture_and_publish(&mut store, &dir.join("work")).unwrap()
        };
        run_child_and_kill(dir, "restore", checkpoint, Some(snapshot));

        let target = dir.join("target");
        let store = Store::open(&dir.join("store")).unwrap();
        let carry = |_: &fabric_core::snapshot::CacheEntry| true;
        if checkpoint == "restore.after_rename" {
            assert_eq!(
                restore_state(&target),
                RestoreState::Present,
                "{checkpoint}"
            );
            let verified = verify(&store, snapshot, &target, Verify::All, &carry).unwrap();
            assert!(verified > 0, "{checkpoint}");
            continue;
        }
        assert!(
            matches!(restore_state(&target), RestoreState::HalfRestored { .. }),
            "{checkpoint}: {:?}",
            restore_state(&target)
        );
        assert!(!target.exists(), "{checkpoint}: target must not exist yet");
        let report = restore(
            &store,
            &RestoreRequest {
                snapshot,
                target: &target,
                harness_state: None,
                verify: Verify::All,
                durability: Durability::ProcessCrash,
                carry: &carry,
            },
        )
        .unwrap();
        assert!(report.verified > 0, "{checkpoint}");
        assert_eq!(
            restore_state(&target),
            RestoreState::Present,
            "{checkpoint}"
        );
        let git = Git::new(&target, CREATED_AT);
        let status = git
            .run_text(&["status", "--porcelain=v2", "--untracked-files=all"])
            .unwrap();
        let source = Git::new(&dir.join("work"), CREATED_AT)
            .run_text(&["status", "--porcelain=v2", "--untracked-files=all"])
            .unwrap();
        assert_eq!(status, source, "{checkpoint}");
    }
}
