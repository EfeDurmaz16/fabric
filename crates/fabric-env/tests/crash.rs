//! Crash tests for materialize (ADR 0009): a child process runs
//! `Ready::execute`, dies by SIGKILL at a named point, and the parent
//! checks what the next resume sees.
//!
//! The child is this test binary started again with `FABRIC_CRASH_ROOT`
//! set: each test runs its child half when the variable is set and its
//! parent half otherwise. The steps are `sh` scripts, so the kill lands at
//! a point the script names rather than at a guess of the clock. Parent
//! and child take the tree's lock in one lock directory, as two resumes on
//! one node do.

use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fabric_env::detect::detect;
use fabric_env::materialize::{
    CacheHealth, MaterializeError, NodeEnv, Plan, SENTINEL, TreeLock, plan,
};

const ROOT_VAR: &str = "FABRIC_CRASH_ROOT";

/// The lock directory of the node both halves play, as Fabric's state would hold it.
fn locks() -> std::path::PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("crash-locks")
}

/// How long the parent waits for a process the test started to reach a point or exit.
const PATIENCE: Duration = Duration::from_secs(20);

type TestResult<T> = Result<T, Box<dyn Error>>;

/// Runs the child half when this process is the child: lock, detect, plan and execute over the parent's workspace.
fn run_child() -> TestResult<bool> {
    let Some(root) = env::var_os(ROOT_VAR) else {
        return Ok(false);
    };
    let lock = TreeLock::acquire(&locks(), Path::new(&root))?;
    let detected = detect(Path::new(&root), None)?;
    plan(&detected, &lock, &NodeEnv::current())?
        .ready()?
        .execute(&lock)?;
    Ok(true)
}

/// Starts the test named `test` again, as the child over `root`.
fn spawn_child(test: &str, root: &Path) -> TestResult<Child> {
    Ok(Command::new(env::current_exe()?)
        .args([test, "--exact", "--test-threads=1"])
        .env(ROOT_VAR, root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?)
}

/// Writes `override_toml` as the workspace's `.fabric/env.toml`.
fn declare(root: &Path, override_toml: &str) -> io::Result<()> {
    fs::create_dir_all(root.join(".fabric"))?;
    fs::write(root.join(".fabric/env.toml"), override_toml)
}

/// Plans the workspace again, as the next resume would, once no step from an earlier resume holds the lock.
fn next_plan(root: &Path) -> TestResult<Plan> {
    let started = Instant::now();
    let lock = loop {
        match TreeLock::acquire(&locks(), root) {
            Err(MaterializeError::InstallerRunning { .. }) if started.elapsed() < PATIENCE => {
                thread::sleep(Duration::from_millis(20));
            }
            other => break other?,
        }
    };
    Ok(plan(&detect(root, None)?, &lock, &NodeEnv::current())?)
}

/// Waits until `path` exists.
fn wait_for(path: &Path) -> TestResult<()> {
    let started = Instant::now();
    while !path.exists() {
        if started.elapsed() > PATIENCE {
            return Err(format!("{} never appeared", path.display()).into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// A resume killed after its step did all the work and before the sentinel
/// was written leaves no proof, so the next resume fills the cache again.
#[test]
fn a_resume_killed_before_the_sentinel_write_leaves_the_cache_to_fill() {
    if run_child().unwrap() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    declare(
        dir.path(),
        r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "mkdir -p out && echo x > out/f && kill -9 $PPID"]
"#,
    )
    .unwrap();
    let status = spawn_child(
        "a_resume_killed_before_the_sentinel_write_leaves_the_cache_to_fill",
        dir.path(),
    )
    .unwrap()
    .wait()
    .unwrap();
    assert_eq!(status.signal(), Some(9));
    assert!(dir.path().join("out/f").is_file());
    assert!(!dir.path().join("out").join(SENTINEL).exists());
    let next = next_plan(dir.path()).unwrap();
    assert_eq!(
        next.caches()[0].health,
        CacheHealth::Incomplete {
            expected: &[SENTINEL]
        }
    );
    assert_eq!(next.steps().len(), 1);
}

/// The sentinel of a whole cache is removed before a step that fills it
/// starts, so a resume killed during that step leaves no proof behind.
#[test]
fn a_resume_killed_during_a_step_removes_the_old_proof() {
    if run_child().unwrap() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("out")).unwrap();
    fs::write(dir.path().join("out/f"), "x").unwrap();
    fs::write(dir.path().join("out").join(SENTINEL), "").unwrap();
    declare(
        dir.path(),
        r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "echo y > out/f && kill -9 $PPID"]
when = "always"
"#,
    )
    .unwrap();
    let status = spawn_child(
        "a_resume_killed_during_a_step_removes_the_old_proof",
        dir.path(),
    )
    .unwrap()
    .wait()
    .unwrap();
    assert_eq!(status.signal(), Some(9));
    assert!(!dir.path().join("out").join(SENTINEL).exists());
    assert_eq!(
        next_plan(dir.path()).unwrap().caches()[0].health,
        CacheHealth::Incomplete {
            expected: &[SENTINEL]
        }
    );
}

/// After a `kill -9` of resume the installer it started keeps running. The
/// next resume refuses to take the tree while it runs, instead of
/// restoring over it or starting a second installer, and proceeds once it
/// has exited. Nothing the workspace does lifts the refusal: the lock is
/// not in the tree, so `rm -rf .fabric`, as `git clean -xfd` does, and a
/// restore that writes the tree again leave it held.
#[test]
fn a_step_orphaned_by_a_killed_resume_blocks_the_next_one_until_it_exits() {
    if run_child().unwrap() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    declare(
        dir.path(),
        r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "touch started; while [ ! -e release ]; do sleep 0.02; done; mkdir -p out; echo x > out/f"]
"#,
    )
    .unwrap();
    let mut child = spawn_child(
        "a_step_orphaned_by_a_killed_resume_blocks_the_next_one_until_it_exits",
        dir.path(),
    )
    .unwrap();
    wait_for(&dir.path().join("started")).unwrap();
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(9));

    let refused = || {
        matches!(
            TreeLock::acquire(&locks(), dir.path()),
            Err(MaterializeError::InstallerRunning { .. })
        )
    };
    assert!(refused());
    fs::remove_dir_all(dir.path().join(".fabric")).unwrap();
    assert!(refused());
    declare(
        dir.path(),
        "[[cache]]\npath = \"out\"\nclass = \"build\"\n\n[[setup]]\nprogram = \"true\"\n",
    )
    .unwrap();
    assert!(refused());

    fs::write(dir.path().join("release"), "").unwrap();
    let next = next_plan(dir.path()).unwrap();
    assert!(dir.path().join("out/f").is_file());
    assert_eq!(
        next.caches()[0].health,
        CacheHealth::Incomplete {
            expected: &[SENTINEL]
        }
    );
    assert_eq!(next.steps().len(), 1);
}
