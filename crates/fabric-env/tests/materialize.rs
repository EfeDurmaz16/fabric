//! Materialize over real directories: which steps a cache state selects,
//! what a dry run leaves untouched and what refuses before anything runs.
//! How a step's output reaches the log is in `step_output.rs`, alone in
//! its own process.
//!
//! Each test builds its detection report the way a user would, by writing
//! `.fabric/env.toml` into the workspace and running `detect`, so the plan
//! sees exactly what resume will see.

use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fabric_core::env::{EnvVarName, RunWhen};
use fabric_env::detect::{Detection, detect};
use fabric_env::materialize::{
    CacheHealth, MaterializeError, NodeEnv, Plan, SENTINEL, TreeLock, plan,
};
use tempfile::TempDir;

type TestResult<T> = Result<T, Box<dyn Error>>;

/// Files to write, as `(path, contents)`; a path ending in `/` is an empty directory.
type Files<'a> = &'a [(&'a str, &'a str)];

const PACKAGE_JSON: &str = r#"{"name":"x","version":"1.0.0"}"#;

/// Copies a fixture workspace into a fresh directory, so a test never writes into the source tree.
fn copy_fixture(name: &str) -> io::Result<TempDir> {
    let from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let dir = tempfile::tempdir()?;
    copy_tree(&from, dir.path())?;
    Ok(dir)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Writes `files` under `root`, making directories as needed.
fn write(root: &Path, files: Files) -> io::Result<()> {
    for (path, text) in files {
        let full = root.join(path);
        if path.ends_with('/') {
            fs::create_dir_all(full)?;
            continue;
        }
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full, text)?;
    }
    Ok(())
}

/// Takes the tree's lock in this test binary's own lock directory, as resume does before restore.
fn take_lock(root: &Path) -> TestResult<TreeLock> {
    Ok(TreeLock::acquire(
        &Path::new(env!("CARGO_TARGET_TMPDIR")).join("locks"),
        root,
    )?)
}

/// Plans `detected` under its tree's lock with this process's environment, for a test that runs nothing.
fn dry_run(detected: &Detection) -> TestResult<Plan> {
    let lock = take_lock(detected.root())?;
    Ok(plan(detected, &lock, &NodeEnv::current())?)
}

/// Writes `override_toml` as the workspace's `.fabric/env.toml` and detects.
fn declared(root: &Path, override_toml: &str) -> TestResult<Detection> {
    write(root, &[(".fabric/env.toml", override_toml)])?;
    Ok(detect(root, &[])?)
}

/// A dry run of a fresh checkout plans the install and leaves the tree as it was.
#[test]
fn a_fresh_pnpm_workspace_plans_its_install_and_the_dry_run_changes_nothing() {
    let dir = copy_fixture("node-pnpm").unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let detected = detect(dir.path(), &[]).unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    assert_eq!(planned.caches()[0].health, CacheHealth::Missing);
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].index(), 0);
    assert_eq!(planned.steps()[0].step().when, RunWhen::CachesMissing);
    assert_eq!(planned.steps()[0].step().program.as_str(), "pnpm");
    assert_eq!(
        planned.steps()[0].dir(),
        fs::canonicalize(dir.path()).unwrap()
    );
    assert!(!dir.path().join("node_modules").exists());
}

#[test]
fn a_finished_install_runs_nothing() {
    let dir = copy_fixture("node-pnpm").unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[
            ("node_modules/.fabric-installed", ""),
            ("node_modules/a/package.json", "{}"),
        ],
    )
    .unwrap();
    let detected = detect(dir.path(), &[]).unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    assert_eq!(planned.caches()[0].health, CacheHealth::Healthy);
    assert!(planned.is_noop());
    assert!(planned.ready().unwrap().execute(&lock).is_ok());
}

/// A declared variable is the job's, not only its setup's: a warm resume
/// that runs no step refuses without it, as a cold one does.
#[test]
fn a_plan_with_no_steps_still_needs_the_declared_variables() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let unset = "FABRIC_TEST_UNSET_FOR_A_NOOP_PLAN";
    let detected = declared(dir.path(), &format!("env = [\"{unset}\"]\nsetup = []\n")).unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::new(None, Vec::new())).unwrap();
    assert!(planned.is_noop());
    match planned.ready() {
        Err(MaterializeError::CannotRun { programs, env }) => {
            assert!(programs.is_empty());
            assert_eq!(env.len(), 1);
            assert_eq!(env[0].as_str(), unset);
        }
        other => panic!("{other:?}"),
    }
}

/// npm, pnpm and Yarn 1 write their own marker before the root package's
/// lifecycle scripts run, so an install whose `postinstall` fails leaves
/// the marker over a tree that is not whole. Only the sentinel, written
/// after the step exits 0, makes the next plan skip the install.
#[test]
fn an_installer_marker_left_by_a_failed_install_is_not_proof() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[("package.json", PACKAGE_JSON), ("package-lock.json", "")],
    )
    .unwrap();
    let bin = tempfile::tempdir().unwrap();
    let npm = bin.path().join("npm");
    fs::write(
        &npm,
        "#!/bin/sh\nmkdir -p node_modules/a && echo {} > node_modules/.package-lock.json && [ -e postinstall-ok ]\n",
    )
    .unwrap();
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).unwrap();
    let node = NodeEnv::new(Some(bin.path().as_os_str().to_owned()), Vec::new());
    let resume = || plan(&detect(dir.path(), &[]).unwrap(), &lock, &node).unwrap();

    assert!(matches!(
        resume().ready().unwrap().execute(&lock),
        Err(MaterializeError::StepFailed { .. })
    ));
    assert!(dir.path().join("node_modules/.package-lock.json").is_file());
    let after_failure = resume();
    assert_eq!(after_failure.caches()[0].health, CacheHealth::Incomplete);
    assert_eq!(after_failure.steps().len(), 1);

    write(dir.path(), &[("postinstall-ok", "")]).unwrap();
    after_failure.ready().unwrap().execute(&lock).unwrap();
    assert!(resume().is_noop());
}

/// One row per rule: a cache a step fills needs the sentinel, whatever
/// else its installer left there; a cache no step fills needs content.
#[test]
fn health_follows_the_cache_kind() {
    let cases: [(Files, CacheHealth); 9] = [
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("package-lock.json", ""),
                ("node_modules/a/package.json", "{}"),
            ],
            CacheHealth::Incomplete,
        ),
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("bun.lock", ""),
                ("node_modules/a/package.json", "{}"),
                ("node_modules/.fabric-installed", ""),
            ],
            CacheHealth::Healthy,
        ),
        // `uv venv` writes pyvenv.cfg before any package, so it proves nothing.
        (
            &[
                ("requirements.txt", ""),
                (".venv/pyvenv.cfg", "home = /x\n"),
            ],
            CacheHealth::Incomplete,
        ),
        (
            &[
                ("uv.lock", ""),
                (".venv/pyvenv.cfg", ""),
                (".venv/.fabric-installed", ""),
            ],
            CacheHealth::Healthy,
        ),
        (
            &[("Cargo.toml", ""), ("target/debug/x", "")],
            CacheHealth::Healthy,
        ),
        (
            &[("Cargo.toml", ""), ("target", "")],
            CacheHealth::NotADirectory,
        ),
        (&[("Cargo.toml", "")], CacheHealth::Missing),
        (&[("Cargo.toml", ""), ("target/", "")], CacheHealth::Empty),
        // A declared cache that no step fills is whole when it has content.
        (
            &[
                (
                    ".fabric/env.toml",
                    "setup = []\n[[cache]]\npath = \"out\"\nclass = \"build\"\n",
                ),
                ("out/x", ""),
            ],
            CacheHealth::Healthy,
        ),
    ];
    for (files, health) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), files).unwrap();
        let planned = dry_run(&detect(dir.path(), &[]).unwrap()).unwrap();
        assert_eq!(planned.caches().len(), 1, "{files:?}");
        assert_eq!(planned.caches()[0].health, health, "{files:?}");
    }
}

/// A declared cache keyed `node` holds a build's output: without a
/// declared step only content counts, and with one the sentinel does.
#[test]
fn a_declared_node_cache_is_proven_by_content_or_its_sentinel() {
    let warm_npm = [
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        ("node_modules/.fabric-installed", ""),
        (".output/index.js", ""),
    ];
    let cache = "[[cache]]\npath = \".output\"\nclass = \"build\"\necosystem = \"node\"\n";
    let with_step = format!("{cache}[[setup]]\nprogram = \"true\"\n");
    let cases: [(&str, Option<&str>, CacheHealth); 3] = [
        (cache, None, CacheHealth::Healthy),
        (&with_step, None, CacheHealth::Incomplete),
        (
            &with_step,
            Some(".output/.fabric-installed"),
            CacheHealth::Healthy,
        ),
    ];
    for (override_toml, sentinel, health) in cases {
        let dir = tempfile::tempdir().unwrap();
        let lock = take_lock(dir.path()).unwrap();
        write(dir.path(), &warm_npm).unwrap();
        if let Some(sentinel) = sentinel {
            write(dir.path(), &[(sentinel, "")]).unwrap();
        }
        let detected = declared(dir.path(), override_toml).unwrap();
        let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
        let output = planned
            .caches()
            .iter()
            .find(|c| c.path.as_str() == ".output")
            .unwrap();
        assert_eq!(output.health, health, "{override_toml} {sentinel:?}");
    }
}

/// Declared steps that fill a declared cache in an npm workspace leave the
/// sentinel in every cache they fill, so the next resume runs nothing and
/// a declared `npm ci` never deletes the `node_modules` it made.
#[test]
fn declared_steps_over_a_declared_node_cache_run_once() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[("package.json", PACKAGE_JSON), ("package-lock.json", "")],
    )
    .unwrap();
    let detected = declared(
        dir.path(),
        r#"
[[cache]]
path = ".output"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "mkdir -p node_modules/a && echo {} > node_modules/a/package.json"]

[[setup]]
program = "sh"
args = ["-c", "mkdir -p .output && echo x > .output/index.js"]
"#,
    )
    .unwrap();
    let first = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    assert_eq!(first.steps().len(), 2);
    first.ready().unwrap().execute(&lock).unwrap();
    let again = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert!(
        again
            .caches()
            .iter()
            .all(|c| c.health == CacheHealth::Healthy),
        "{:?}",
        again.caches()
    );
    assert!(again.is_noop());
}

/// The steps that fill a cache leave the sentinel once they exit 0, so the
/// next plan finds the cache whole and runs nothing. That the sentinel is
/// removed before a step starts is `crash.rs`'s to show.
#[test]
fn the_sentinel_marks_a_cache_whole_once_its_steps_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let fill = r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "mkdir -p out && echo x > out/f"]
"#;
    let detected = declared(dir.path(), fill).unwrap();
    plan(&detected, &lock, &NodeEnv::current())
        .unwrap()
        .ready()
        .unwrap()
        .execute(&lock)
        .unwrap();
    assert!(dir.path().join("out").join(SENTINEL).is_file());
    let again = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert_eq!(again.caches()[0].health, CacheHealth::Healthy);
    assert!(again.is_noop());
}

#[test]
fn an_always_step_runs_on_a_warm_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(dir.path(), &[("Cargo.toml", ""), ("target/debug/x", "")]).unwrap();
    let detected = declared(
        dir.path(),
        r#"
[[setup]]
program = "cargo"
args = ["fetch"]

[[setup]]
program = "true"
when = "always"
"#,
    )
    .unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].index(), 1);
    assert_eq!(planned.steps()[0].step().when, RunWhen::Always);
}

#[test]
fn steps_run_in_order_and_stop_at_the_first_failure() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let detected = declared(
        dir.path(),
        r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "echo one >> log"]

[[setup]]
program = "false"

[[setup]]
program = "sh"
args = ["-c", "echo three >> log"]
"#,
    )
    .unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    match planned.ready().unwrap().execute(&lock) {
        Err(MaterializeError::StepFailed {
            index,
            program,
            status,
        }) => {
            assert_eq!(index, 1);
            assert_eq!(program.as_str(), "false");
            assert!(!status.success());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(dir.path().join("log")).unwrap(), "one\n");
}

/// A plan that cannot finish does not start, and one error names every
/// missing program and variable, so the user fixes them in one round.
#[test]
fn missing_programs_and_variables_refuse_before_anything_runs_and_are_all_named() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let unset = "FABRIC_TEST_UNSET";
    let detected = declared(
        dir.path(),
        &format!(
            r#"
env = ["PATH", "{unset}"]

[[cache]]
path = "out"
class = "build"

[[setup]]
program = "touch"
args = ["ran"]

[[setup]]
program = "fabric-test-no-such-program"

[[setup]]
program = "fabric-test-no-such-program-either"
"#
        ),
    )
    .unwrap();
    let node = NodeEnv::new(
        std::env::var_os("PATH"),
        vec![EnvVarName::parse("PATH").unwrap()],
    );
    let planned = plan(&detected, &lock, &node).unwrap();
    assert!(planned.steps()[0].program().is_some());
    assert_eq!(planned.steps()[1].program(), None);
    let missing: Vec<&str> = planned
        .missing_env()
        .iter()
        .map(EnvVarName::as_str)
        .collect();
    assert_eq!(missing, [unset]);
    let err = planned.ready().unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("fabric-test-no-such-program-either"),
        "{text}"
    );
    assert!(text.contains(unset), "{text}");
    match err {
        MaterializeError::CannotRun { programs, env } => {
            let programs: Vec<(usize, &str)> =
                programs.iter().map(|(i, p)| (*i, p.as_str())).collect();
            assert_eq!(
                programs,
                [
                    (1, "fabric-test-no-such-program"),
                    (2, "fabric-test-no-such-program-either")
                ]
            );
            assert_eq!(env.len(), 1);
            assert_eq!(env[0].as_str(), unset);
        }
        other => panic!("{other:?}"),
    }
    assert!(!dir.path().join("ran").exists());
}

#[test]
fn a_step_directory_may_not_leave_the_workspace() {
    let outside = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    symlink(outside.path(), dir.path().join("web")).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\ncwd = \"web\"\nwhen = \"always\"\n",
    )
    .unwrap();
    assert!(matches!(
        plan(&detected, &lock, &NodeEnv::current()),
        Err(MaterializeError::StepDirOutsideRoot { index: 0 })
    ));

    fs::create_dir(dir.path().join("app")).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\ncwd = \"app\"\nwhen = \"always\"\n",
    )
    .unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    let expected: PathBuf = fs::canonicalize(dir.path().join("app")).unwrap();
    assert_eq!(planned.steps()[0].dir(), expected);
}

/// A pipe closes only when every holder exits. A step that leaves a
/// process behind, as daemon-starting postinstall scripts do, must not hold
/// the resume until that process ends.
#[test]
fn a_step_that_leaves_a_process_behind_does_not_hold_the_resume() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let detected = declared(
        dir.path(),
        r#"
[[setup]]
program = "sh"
args = ["-c", "sleep 4 & echo started"]
when = "always"
"#,
    )
    .unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    let started = Instant::now();
    planned.ready().unwrap().execute(&lock).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

/// With no cache path nothing proves the workspace warm, so the steps run:
/// detection's `go mod download`, whose cache is outside the workspace, and
/// a `[[setup]]` declared without a `[[cache]]`.
#[test]
fn a_report_without_caches_runs_its_steps_on_every_resume() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[("go.mod", "module example.com/x\n\ngo 1.22.0\n")],
    )
    .unwrap();
    let detected = detect(dir.path(), &[]).unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    assert!(planned.caches().is_empty());
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].step().program.as_str(), "go");

    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"touch\"\nargs = [\"ran\"]\n",
    )
    .unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    assert_eq!(planned.steps().len(), 1);
    planned.ready().unwrap().execute(&lock).unwrap();
    assert!(dir.path().join("ran").exists());
}

/// A missing `target` is a build away and says nothing about
/// `node_modules`; reinstalling a whole carried tree because of it would
/// throw away the carry on every resume. Only the fetch it gates runs.
#[test]
fn a_missing_target_does_not_reinstall_a_whole_node_modules() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[
            ("package.json", PACKAGE_JSON),
            ("package-lock.json", ""),
            ("node_modules/.fabric-installed", ""),
            ("Cargo.toml", ""),
        ],
    )
    .unwrap();
    let detected = detect(dir.path(), &[]).unwrap();
    let planned = plan(&detected, &lock, &NodeEnv::current()).unwrap();
    let programs: Vec<&str> = planned
        .steps()
        .iter()
        .map(|s| s.step().program.as_str())
        .collect();
    assert_eq!(programs, ["cargo"]);
}

/// A declared step does not fill a detected `target`, so a missing
/// `target` never reruns a declared `npm ci` over a healthy
/// `node_modules`. A torn `node_modules` still does.
#[test]
fn a_missing_target_does_not_run_a_declared_install() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[
            ("package.json", PACKAGE_JSON),
            ("package-lock.json", ""),
            ("node_modules/a/package.json", "{}"),
            ("node_modules/.fabric-installed", ""),
            ("Cargo.toml", ""),
        ],
    )
    .unwrap();
    let override_toml = "[[setup]]\nprogram = \"npm\"\nargs = [\"ci\"]\n";
    let planned = plan(
        &declared(dir.path(), override_toml).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert!(planned.is_noop(), "{:?}", planned.steps());

    fs::remove_file(dir.path().join("node_modules").join(SENTINEL)).unwrap();
    let planned = plan(
        &declared(dir.path(), override_toml).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert_eq!(planned.steps().len(), 1);
}

/// A cache under a linked directory lies outside the workspace. A step
/// that fills it would follow the link, as `uv venv` does with a linked
/// `.venv`, so the plan refuses and nothing beyond the link changes.
#[test]
fn a_step_never_fills_a_cache_through_a_link() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), &[("out/x", ""), ("kept/x", "")]).unwrap();
    write(outside.path(), &[("kept/.fabric-installed", "")]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    symlink(outside.path(), dir.path().join("web")).unwrap();
    let detected = declared(
        dir.path(),
        r#"
[[cache]]
path = "web/out"
class = "build"

[[cache]]
path = "web/kept"
class = "build"

[[setup]]
program = "touch"
args = ["web/out/ran"]
"#,
    )
    .unwrap();
    match plan(&detected, &lock, &NodeEnv::current()) {
        Err(MaterializeError::CacheNotADirectory { path }) => {
            assert_eq!(path.as_str(), "web/kept");
        }
        other => panic!("{other:?}"),
    }
    assert!(!outside.path().join("out/ran").exists());
    assert!(!outside.path().join("out").join(SENTINEL).exists());
    assert!(outside.path().join("kept").join(SENTINEL).exists());

    symlink(outside.path().join("out"), dir.path().join(".venv")).unwrap();
    fs::remove_dir_all(dir.path().join(".fabric")).unwrap();
    fs::remove_file(dir.path().join("web")).unwrap();
    write(dir.path(), &[("requirements.txt", "")]).unwrap();
    let detected = detect(dir.path(), &[]).unwrap();
    assert!(matches!(
        plan(&detected, &lock, &NodeEnv::current()),
        Err(MaterializeError::CacheNotADirectory { .. })
    ));
}

/// A lock directory inside the workspace is one the workspace can delete,
/// which is the failure the lock exists to prevent.
#[test]
fn a_lock_directory_inside_the_workspace_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        TreeLock::acquire(&dir.path().join(".fabric/locks"), dir.path()),
        Err(MaterializeError::LocksInsideWorkspace { .. })
    ));
}

/// A lock proves nothing about a tree it was not taken for.
#[test]
fn a_lock_for_another_tree_does_not_plan_or_run_this_one() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\nwhen = \"always\"\n",
    )
    .unwrap();
    let wrong = take_lock(other.path()).unwrap();
    assert!(matches!(
        plan(&detected, &wrong, &NodeEnv::current()),
        Err(MaterializeError::WrongTree { .. })
    ));
    let lock = take_lock(dir.path()).unwrap();
    let ready = plan(&detected, &lock, &NodeEnv::current())
        .unwrap()
        .ready()
        .unwrap();
    assert!(matches!(
        ready.execute(&wrong),
        Err(MaterializeError::WrongTree { .. })
    ));
}

/// A step that leaves the sentinel itself, as one that unpacks an archive
/// of a whole cache does, succeeds; a link it leaves there is replaced by
/// a real sentinel and never followed.
#[test]
fn a_sentinel_the_steps_left_is_kept_and_a_link_there_is_replaced() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("target"), "outside").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    let link = format!(
        "ln -s {} built/{SENTINEL}",
        outside.path().join("target").display()
    );
    let override_toml = format!(
        r#"
[[cache]]
path = "out"
class = "build"

[[cache]]
path = "built"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "mkdir -p out built && echo x > out/f && echo x > built/f && touch out/{SENTINEL} && {link}"]
"#
    );
    let detected = declared(dir.path(), &override_toml).unwrap();
    plan(&detected, &lock, &NodeEnv::current())
        .unwrap()
        .ready()
        .unwrap()
        .execute(&lock)
        .unwrap();
    let built = fs::symlink_metadata(dir.path().join("built").join(SENTINEL)).unwrap();
    assert!(built.is_file());
    assert_eq!(
        fs::read_to_string(outside.path().join("target")).unwrap(),
        "outside"
    );
    let again = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert!(again.is_noop(), "{:?}", again.caches());
}
