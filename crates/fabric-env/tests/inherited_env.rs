//! A detected `uv` step over a node whose shell points uv elsewhere.
//!
//! A step runs with the environment of the process that runs `execute`,
//! and this crate never changes its own environment, so the test starts
//! this binary again with uv's variables set, as `crash.rs` does for a
//! crash: the child half resumes over the parent's workspace and the
//! parent checks where the install went.

use std::env;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use fabric_env::detect::detect;
use fabric_env::materialize::{CacheHealth, NodeEnv, TreeLock, plan};

const ROOT_VAR: &str = "FABRIC_UV_ROOT";

type TestResult<T> = Result<T, Box<dyn Error>>;

/// The lock directory of the node the child plays, as Fabric's state would hold it.
fn locks() -> std::path::PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("inherited-env-locks")
}

/// Runs the child half when this process is the child: lock, detect, plan and execute over the parent's workspace.
fn run_child() -> TestResult<bool> {
    let Some(root) = env::var_os(ROOT_VAR) else {
        return Ok(false);
    };
    let lock = TreeLock::acquire(&locks(), Path::new(&root))?;
    let detected = detect(Path::new(&root), &[])?;
    plan(&detected, &lock, &NodeEnv::current())?
        .ready()?
        .execute(&lock)?;
    Ok(true)
}

/// Each variable alone sends a step that honors it outside the workspace:
/// the stand-in `uv` reads them the way uv does, so the install lands in
/// the workspace's `.venv` only when the step runs without all three.
#[test]
fn a_detected_uv_step_never_follows_an_inherited_uv_location() {
    if run_child().unwrap() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"x\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("uv.lock"), "version = 1\n").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    for sub in ["project", "cwd"] {
        fs::create_dir(elsewhere.path().join(sub)).unwrap();
    }
    let bin = tempfile::tempdir().unwrap();
    let uv = bin.path().join("uv");
    fs::write(
        &uv,
        "#!/bin/sh\ncd \"${UV_WORKING_DIR:-.}\" || exit 1\nvenv=\"${UV_PROJECT_ENVIRONMENT:-${UV_PROJECT:-.}/.venv}\"\nmkdir -p \"$venv\" && echo x > \"$venv/installed\"\n",
    )
    .unwrap();
    fs::set_permissions(&uv, fs::Permissions::from_mode(0o755)).unwrap();

    let status = Command::new(env::current_exe().unwrap())
        .args([
            "a_detected_uv_step_never_follows_an_inherited_uv_location",
            "--exact",
            "--test-threads=1",
        ])
        .env(ROOT_VAR, dir.path())
        .env(
            "PATH",
            env::join_paths([bin.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap(),
        )
        .env("UV_PROJECT_ENVIRONMENT", elsewhere.path().join("venv"))
        .env("UV_PROJECT", elsewhere.path().join("project"))
        .env("UV_WORKING_DIR", elsewhere.path().join("cwd"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{status}");

    assert!(dir.path().join(".venv/installed").is_file());
    for outside in ["venv", "project/.venv", "cwd/.venv"] {
        assert!(!elsewhere.path().join(outside).exists(), "{outside}");
    }
    let lock = TreeLock::acquire(&locks(), dir.path()).unwrap();
    let again = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::new(None, Vec::new()),
    )
    .unwrap();
    assert_eq!(again.caches()[0].health, CacheHealth::Healthy);
}
