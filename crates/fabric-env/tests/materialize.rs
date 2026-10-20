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
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fabric_core::env::{EnvVarName, RunWhen};
use fabric_env::detect::{Detection, detect};
use fabric_env::materialize::{CacheHealth, MaterializeError, plan};
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

/// Writes `override_toml` as the workspace's `.fabric/env.toml` and detects.
fn declared(root: &Path, override_toml: &str) -> TestResult<Detection> {
    write(root, &[(".fabric/env.toml", override_toml)])?;
    Ok(detect(root, None)?)
}

/// A dry run of a fresh checkout plans the install and leaves the tree as it was.
#[test]
fn a_fresh_pnpm_workspace_plans_its_install_and_the_dry_run_changes_nothing() {
    let dir = copy_fixture("node-pnpm").unwrap();
    let detected = detect(dir.path(), None).unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
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
    write(
        dir.path(),
        &[
            ("node_modules/.modules.yaml", "storeDir: x\n"),
            ("node_modules/a/package.json", "{}"),
        ],
    )
    .unwrap();
    let detected = detect(dir.path(), None).unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
    assert_eq!(planned.caches()[0].health, CacheHealth::Healthy);
    assert!(planned.is_noop());
    assert!(planned.execute().is_ok());
}

/// An install killed halfway leaves packages without the marker the
/// manager writes last; the next plan installs again.
#[test]
fn a_torn_install_runs_again() {
    let dir = copy_fixture("node-pnpm").unwrap();
    write(dir.path(), &[("node_modules/a/package.json", "{}")]).unwrap();
    let detected = detect(dir.path(), None).unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
    assert_eq!(
        planned.caches()[0].health,
        CacheHealth::Incomplete {
            expected: &[".modules.yaml"]
        }
    );
    assert_eq!(planned.steps().len(), 1);
}

/// One row per rule: the marker follows the manager detection chose, not
/// the lockfiles on disk; `.venv` needs `pyvenv.cfg`; other caches need content.
#[test]
fn health_follows_the_cache_kind() {
    let cases: [(Files, CacheHealth); 10] = [
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("package-lock.json", ""),
                ("node_modules/.package-lock.json", "{}"),
            ],
            CacheHealth::Healthy,
        ),
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("package-lock.json", ""),
                ("node_modules/.modules.yaml", ""),
            ],
            CacheHealth::Incomplete {
                expected: &[".package-lock.json"],
            },
        ),
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("yarn.lock", ""),
                (".yarnrc.yml", ""),
                ("node_modules/.yarn-state.yml", ""),
            ],
            CacheHealth::Healthy,
        ),
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("bun.lock", ""),
                ("node_modules/a/package.json", "{}"),
            ],
            CacheHealth::Healthy,
        ),
        // A leftover npm lockfile next to a Bun tree must not demand npm's marker.
        (
            &[
                ("package.json", r#"{"packageManager":"bun@1.2.0"}"#),
                ("bun.lock", ""),
                ("package-lock.json", ""),
                ("node_modules/a/package.json", "{}"),
            ],
            CacheHealth::Healthy,
        ),
        (
            &[("uv.lock", ""), (".venv/bin/python", "")],
            CacheHealth::Incomplete {
                expected: &["pyvenv.cfg"],
            },
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
    ];
    for (files, health) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), files).unwrap();
        let detected = detect(dir.path(), None).unwrap();
        let planned = plan(dir.path(), &detected).unwrap();
        assert_eq!(planned.caches().len(), 1, "{files:?}");
        assert_eq!(planned.caches()[0].health, health, "{files:?}");
    }
}

#[test]
fn an_always_step_runs_on_a_warm_workspace() {
    let dir = tempfile::tempdir().unwrap();
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
    let planned = plan(dir.path(), &detected).unwrap();
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].index(), 1);
    assert_eq!(planned.steps()[0].step().when, RunWhen::Always);
}

#[test]
fn steps_run_in_order_and_stop_at_the_first_failure() {
    let dir = tempfile::tempdir().unwrap();
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
    let planned = plan(dir.path(), &detected).unwrap();
    match planned.execute() {
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
    let unset = "FABRIC_TEST_UNSET_VARIABLE_7Q2X";
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
    let planned = plan(dir.path(), &detected).unwrap();
    assert!(planned.steps()[0].program().is_some());
    assert_eq!(planned.steps()[1].program(), None);
    let missing: Vec<&str> = planned
        .missing_env()
        .iter()
        .map(EnvVarName::as_str)
        .collect();
    assert_eq!(missing, [unset]);
    let err = planned.execute().unwrap_err();
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
    symlink(outside.path(), dir.path().join("web")).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\ncwd = \"web\"\nwhen = \"always\"\n",
    )
    .unwrap();
    assert!(matches!(
        plan(dir.path(), &detected),
        Err(MaterializeError::StepDirOutsideRoot { index: 0 })
    ));

    fs::create_dir(dir.path().join("app")).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\ncwd = \"app\"\nwhen = \"always\"\n",
    )
    .unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
    let expected: PathBuf = fs::canonicalize(dir.path().join("app")).unwrap();
    assert_eq!(planned.steps()[0].dir(), expected);
}

/// A pipe closes only when every holder exits. A step that leaves a
/// process behind, as daemon-starting postinstall scripts do, must not hold
/// the resume until that process ends.
#[test]
fn a_step_that_leaves_a_process_behind_does_not_hold_the_resume() {
    let dir = tempfile::tempdir().unwrap();
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
    let planned = plan(dir.path(), &detected).unwrap();
    let started = Instant::now();
    planned.execute().unwrap();
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
    write(
        dir.path(),
        &[("go.mod", "module example.com/x\n\ngo 1.22.0\n")],
    )
    .unwrap();
    let detected = detect(dir.path(), None).unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
    assert!(planned.caches().is_empty());
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].step().program.as_str(), "go");

    let dir = tempfile::tempdir().unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"touch\"\nargs = [\"ran\"]\n",
    )
    .unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
    assert_eq!(planned.steps().len(), 1);
    planned.execute().unwrap();
    assert!(dir.path().join("ran").exists());
}
