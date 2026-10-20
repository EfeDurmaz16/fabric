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
use fabric_env::materialize::{CacheHealth, MaterializeError, SENTINEL, plan};
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
    let planned = plan(&detected).unwrap();
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
    let planned = plan(&detected).unwrap();
    assert_eq!(planned.caches()[0].health, CacheHealth::Healthy);
    assert!(planned.is_noop());
    assert!(planned.ready().unwrap().execute().is_ok());
}

/// An install killed halfway leaves packages without the marker the
/// manager writes as it finishes; the next plan installs again.
#[test]
fn a_torn_install_runs_again() {
    let dir = copy_fixture("node-pnpm").unwrap();
    write(dir.path(), &[("node_modules/a/package.json", "{}")]).unwrap();
    let detected = detect(dir.path(), None).unwrap();
    let planned = plan(&detected).unwrap();
    assert_eq!(
        planned.caches()[0].health,
        CacheHealth::Incomplete {
            expected: &[".modules.yaml"]
        }
    );
    assert_eq!(planned.steps().len(), 1);
}

/// One row per rule: the marker follows the manager detection chose, not
/// the lockfiles on disk; a cache a step fills and whose installer writes
/// no marker needs the sentinel; a cache no step fills needs content.
#[test]
fn health_follows_the_cache_kind() {
    let sentinel = &[SENTINEL][..];
    let cases: [(Files, CacheHealth); 13] = [
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
        // Bun writes no marker, so content alone could be a torn tree.
        (
            &[
                ("package.json", PACKAGE_JSON),
                ("bun.lock", ""),
                ("node_modules/a/package.json", "{}"),
            ],
            CacheHealth::Incomplete { expected: sentinel },
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
        // A leftover npm lockfile next to a Bun tree must not demand npm's marker.
        (
            &[
                ("package.json", r#"{"packageManager":"bun@1.2.0"}"#),
                ("bun.lock", ""),
                ("package-lock.json", ""),
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
            CacheHealth::Incomplete { expected: sentinel },
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
        let detected = detect(dir.path(), None).unwrap();
        let planned = plan(&detected).unwrap();
        assert_eq!(planned.caches().len(), 1, "{files:?}");
        assert_eq!(planned.caches()[0].health, health, "{files:?}");
    }
}

/// A declared cache keyed `node` holds a build's output, never npm's
/// marker: without a declared step only content counts, and with one the
/// sentinel does.
#[test]
fn a_declared_node_cache_is_proven_by_content_or_its_sentinel() {
    let warm_npm = [
        ("package.json", PACKAGE_JSON),
        ("package-lock.json", ""),
        ("node_modules/.package-lock.json", "{}"),
        (".output/index.js", ""),
    ];
    let cache = "[[cache]]\npath = \".output\"\nclass = \"build\"\necosystem = \"node\"\n";
    let with_step = format!("{cache}[[setup]]\nprogram = \"true\"\n");
    let cases: [(&str, Option<&str>, CacheHealth); 3] = [
        (cache, None, CacheHealth::Healthy),
        (
            &with_step,
            None,
            CacheHealth::Incomplete {
                expected: &[SENTINEL],
            },
        ),
        (
            &with_step,
            Some(".output/.fabric-installed"),
            CacheHealth::Healthy,
        ),
    ];
    for (override_toml, sentinel, health) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &warm_npm).unwrap();
        if let Some(sentinel) = sentinel {
            write(dir.path(), &[(sentinel, "")]).unwrap();
        }
        let detected = declared(dir.path(), override_toml).unwrap();
        let planned = plan(&detected).unwrap();
        let output = planned
            .caches()
            .iter()
            .find(|c| c.path.as_str() == ".output")
            .unwrap();
        assert_eq!(output.health, health, "{override_toml} {sentinel:?}");
    }
}

/// Declared steps that fill a declared cache in an npm workspace leave its
/// sentinel behind, so the next resume runs nothing: before, the cache was
/// asked for npm's marker, stayed torn forever, and a declared `npm ci`
/// deleted a healthy carried `node_modules` on every resume.
#[test]
fn declared_steps_over_a_declared_node_cache_run_once() {
    let dir = tempfile::tempdir().unwrap();
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
args = ["-c", "mkdir -p node_modules && echo {} > node_modules/.package-lock.json"]

[[setup]]
program = "sh"
args = ["-c", "mkdir -p .output && echo x > .output/index.js"]
"#,
    )
    .unwrap();
    let first = plan(&detected).unwrap();
    assert_eq!(first.steps().len(), 2);
    first.ready().unwrap().execute().unwrap();
    let again = plan(&detect(dir.path(), None).unwrap()).unwrap();
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

/// The sentinel is written only after the steps that fill a cache exit 0,
/// and removed before they start, so a failed or killed run of a step,
/// even over a cache that was whole, leaves the cache to be filled again.
#[test]
fn the_sentinel_marks_a_cache_whole_only_after_its_steps_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let fill = r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "mkdir -p out && echo x > out/f"]
"#;
    let detected = declared(dir.path(), fill).unwrap();
    plan(&detected).unwrap().ready().unwrap().execute().unwrap();
    assert!(dir.path().join("out").join(SENTINEL).is_file());
    let again = plan(&detect(dir.path(), None).unwrap()).unwrap();
    assert_eq!(again.caches()[0].health, CacheHealth::Healthy);
    assert!(again.is_noop());

    let torn = r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "echo y > out/f; exit 3"]
when = "always"
"#;
    let detected = declared(dir.path(), torn).unwrap();
    assert!(matches!(
        plan(&detected).unwrap().ready().unwrap().execute(),
        Err(MaterializeError::StepFailed { .. })
    ));
    assert!(!dir.path().join("out").join(SENTINEL).exists());
    let after = plan(&detect(dir.path(), None).unwrap()).unwrap();
    assert_eq!(
        after.caches()[0].health,
        CacheHealth::Incomplete {
            expected: &[SENTINEL]
        }
    );
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
    let planned = plan(&detected).unwrap();
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
    let planned = plan(&detected).unwrap();
    match planned.ready().unwrap().execute() {
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
    let planned = plan(&detected).unwrap();
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
    symlink(outside.path(), dir.path().join("web")).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\ncwd = \"web\"\nwhen = \"always\"\n",
    )
    .unwrap();
    assert!(matches!(
        plan(&detected),
        Err(MaterializeError::StepDirOutsideRoot { index: 0 })
    ));

    fs::create_dir(dir.path().join("app")).unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"true\"\ncwd = \"app\"\nwhen = \"always\"\n",
    )
    .unwrap();
    let planned = plan(&detected).unwrap();
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
    let planned = plan(&detected).unwrap();
    let started = Instant::now();
    planned.ready().unwrap().execute().unwrap();
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
    let planned = plan(&detected).unwrap();
    assert!(planned.caches().is_empty());
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].step().program.as_str(), "go");

    let dir = tempfile::tempdir().unwrap();
    let detected = declared(
        dir.path(),
        "[[setup]]\nprogram = \"touch\"\nargs = [\"ran\"]\n",
    )
    .unwrap();
    let planned = plan(&detected).unwrap();
    assert_eq!(planned.steps().len(), 1);
    planned.ready().unwrap().execute().unwrap();
    assert!(dir.path().join("ran").exists());
}

/// A missing `target` is a build away and says nothing about
/// `node_modules`; reinstalling a whole carried tree because of it would
/// throw away the carry on every resume. Only the fetch it gates runs.
#[test]
fn a_missing_target_does_not_reinstall_a_whole_node_modules() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("package.json", PACKAGE_JSON),
            ("package-lock.json", ""),
            ("node_modules/.package-lock.json", "{}"),
            ("Cargo.toml", ""),
        ],
    )
    .unwrap();
    let detected = detect(dir.path(), None).unwrap();
    let planned = plan(&detected).unwrap();
    let programs: Vec<&str> = planned
        .steps()
        .iter()
        .map(|s| s.step().program.as_str())
        .collect();
    assert_eq!(programs, ["cargo"]);
}

/// Point 14's rule holds for declared steps too: no declared step fills a
/// detected `target`, so its absence must not rerun a declared `npm ci`
/// over a healthy carried `node_modules`. A torn `node_modules` still does.
#[test]
fn a_missing_target_does_not_run_a_declared_install() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("package.json", PACKAGE_JSON),
            ("package-lock.json", ""),
            ("node_modules/a/package.json", "{}"),
            ("node_modules/.package-lock.json", "{}"),
            ("Cargo.toml", ""),
        ],
    )
    .unwrap();
    let override_toml = "[[setup]]\nprogram = \"npm\"\nargs = [\"ci\"]\n";
    let planned = plan(&declared(dir.path(), override_toml).unwrap()).unwrap();
    assert!(planned.is_noop(), "{:?}", planned.steps());

    fs::remove_file(dir.path().join("node_modules/.package-lock.json")).unwrap();
    let planned = plan(&declared(dir.path(), override_toml).unwrap()).unwrap();
    assert_eq!(planned.steps().len(), 1);
}

/// A cache under a linked directory lies outside the workspace: it is never
/// judged healthy, and its sentinel is neither written nor removed there.
#[test]
fn a_cache_under_a_linked_directory_gets_no_sentinel() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), &[("out/x", ""), ("kept/x", "")]).unwrap();
    write(outside.path(), &[("kept/.fabric-installed", "")]).unwrap();
    let dir = tempfile::tempdir().unwrap();
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
program = "true"
"#,
    )
    .unwrap();
    let planned = plan(&detected).unwrap();
    assert!(
        planned
            .caches()
            .iter()
            .all(|c| c.health == CacheHealth::NotADirectory),
        "{:?}",
        planned.caches()
    );
    planned.ready().unwrap().execute().unwrap();
    assert!(!outside.path().join("out").join(SENTINEL).exists());
    assert!(outside.path().join("kept").join(SENTINEL).exists());
}
