//! Materialize over real directories: which steps a cache state selects,
//! what a dry run leaves untouched, what refuses before anything runs, and
//! how a step's output reaches the log.

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fabric_core::env::{EnvSpec, EnvVarName, RunWhen, SetupStep};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use fabric_env::detect::detect;
use fabric_env::materialize::{CacheHealth, MaterializeError, StepReason, plan};
use tempfile::TempDir;

type TestResult<T> = Result<T, Box<dyn Error>>;

/// Files to write, as `(path, contents)`.
type Files<'a> = &'a [(&'a str, &'a str)];

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
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full, text)?;
    }
    Ok(())
}

fn step(program: &str, args: &[&str], when: RunWhen) -> TestResult<SetupStep> {
    Ok(SetupStep {
        program: ProgramName::parse(program)?,
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        cwd: None,
        when,
    })
}

fn spec(setup: Vec<SetupStep>, caches: &[&str], env: &[&str]) -> TestResult<EnvSpec> {
    Ok(EnvSpec {
        tools: Vec::new(),
        setup,
        env_var_names: env
            .iter()
            .map(|n| EnvVarName::parse(n))
            .collect::<Result<_, _>>()?,
        cache_paths: caches
            .iter()
            .map(|p| RelPath::parse(p))
            .collect::<Result<_, _>>()?,
    })
}

/// A dry run of a fresh checkout plans the install and leaves the tree as it was.
#[test]
fn a_fresh_pnpm_workspace_plans_its_install_and_the_dry_run_changes_nothing() {
    let dir = copy_fixture("node-pnpm").unwrap();
    let detected = detect(dir.path()).unwrap();
    let planned = plan(dir.path(), &detected.spec).unwrap();
    assert_eq!(planned.caches()[0].health, CacheHealth::Missing);
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].index(), 0);
    assert_eq!(planned.steps()[0].reason(), StepReason::CachesMissing);
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
    let detected = detect(dir.path()).unwrap();
    let planned = plan(dir.path(), &detected.spec).unwrap();
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
    let detected = detect(dir.path()).unwrap();
    let planned = plan(dir.path(), &detected.spec).unwrap();
    assert_eq!(
        planned.caches()[0].health,
        CacheHealth::Incomplete {
            expected: vec![".modules.yaml"]
        }
    );
    assert_eq!(planned.steps().len(), 1);
}

/// One row per rule: the marker comes from the lockfile next to
/// `node_modules`, `.venv` needs `pyvenv.cfg`, other caches need content.
#[test]
fn health_follows_the_cache_kind() {
    let cases: [(Files, &str, CacheHealth); 8] = [
        (
            &[
                ("package-lock.json", ""),
                ("node_modules/.package-lock.json", "{}"),
            ],
            "node_modules",
            CacheHealth::Healthy,
        ),
        (
            &[
                ("package-lock.json", ""),
                ("node_modules/.modules.yaml", ""),
            ],
            "node_modules",
            CacheHealth::Incomplete {
                expected: vec![".package-lock.json"],
            },
        ),
        (
            &[("yarn.lock", ""), ("node_modules/.yarn-state.yml", "")],
            "node_modules",
            CacheHealth::Healthy,
        ),
        (
            &[("bun.lock", ""), ("node_modules/a/package.json", "{}")],
            "node_modules",
            CacheHealth::Healthy,
        ),
        (
            &[(".venv/bin/python", "")],
            ".venv",
            CacheHealth::Incomplete {
                expected: vec!["pyvenv.cfg"],
            },
        ),
        (&[("target/debug/x", "")], "target", CacheHealth::Healthy),
        (&[("target", "")], "target", CacheHealth::NotADirectory),
        (&[], "target", CacheHealth::Missing),
    ];
    for (files, cache, health) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), files).unwrap();
        let planned = plan(dir.path(), &spec(Vec::new(), &[cache], &[]).unwrap()).unwrap();
        assert_eq!(planned.caches()[0].health, health, "{files:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("target")).unwrap();
    let planned = plan(dir.path(), &spec(Vec::new(), &["target"], &[]).unwrap()).unwrap();
    assert_eq!(planned.caches()[0].health, CacheHealth::Empty);
}

#[test]
fn an_always_step_runs_on_a_warm_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("target/debug/x", "")]).unwrap();
    let setup = vec![
        step("cargo", &["fetch"], RunWhen::CachesMissing).unwrap(),
        step("true", &[], RunWhen::Always).unwrap(),
    ];
    let planned = plan(dir.path(), &spec(setup, &["target"], &[]).unwrap()).unwrap();
    assert_eq!(planned.steps().len(), 1);
    assert_eq!(planned.steps()[0].index(), 1);
    assert_eq!(planned.steps()[0].reason(), StepReason::Always);
}

#[test]
fn steps_run_in_order_and_stop_at_the_first_failure() {
    let dir = tempfile::tempdir().unwrap();
    let setup = vec![
        step("sh", &["-c", "echo one >> log"], RunWhen::CachesMissing).unwrap(),
        step("false", &[], RunWhen::CachesMissing).unwrap(),
        step("sh", &["-c", "echo three >> log"], RunWhen::CachesMissing).unwrap(),
    ];
    let planned = plan(dir.path(), &spec(setup, &["out"], &[]).unwrap()).unwrap();
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

/// A plan that cannot finish does not start: the first step would
/// otherwise run before the missing program or variable is found.
#[test]
fn a_missing_program_or_variable_refuses_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let missing_program = vec![
        step("touch", &["ran"], RunWhen::CachesMissing).unwrap(),
        step("fabric-test-no-such-program", &[], RunWhen::CachesMissing).unwrap(),
    ];
    let planned = plan(dir.path(), &spec(missing_program, &["out"], &[]).unwrap()).unwrap();
    assert!(planned.steps()[0].program().is_some());
    assert_eq!(planned.steps()[1].program(), None);
    assert!(matches!(
        planned.execute(),
        Err(MaterializeError::ProgramNotFound { index: 1, .. })
    ));

    let unset = "FABRIC_TEST_UNSET_VARIABLE_7Q2X";
    let setup = vec![step("touch", &["ran"], RunWhen::CachesMissing).unwrap()];
    let planned = plan(
        dir.path(),
        &spec(setup, &["out"], &["PATH", unset]).unwrap(),
    )
    .unwrap();
    let missing: Vec<&str> = planned
        .missing_env()
        .iter()
        .map(EnvVarName::as_str)
        .collect();
    assert_eq!(missing, [unset]);
    match planned.execute() {
        Err(MaterializeError::MissingEnvVars { names }) => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].as_str(), unset);
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
    let mut install = step("true", &[], RunWhen::Always).unwrap();
    install.cwd = Some(RelPath::parse("web").unwrap());
    assert!(matches!(
        plan(dir.path(), &spec(vec![install], &[], &[]).unwrap()),
        Err(MaterializeError::StepDirOutsideRoot { index: 0 })
    ));

    fs::create_dir(dir.path().join("app")).unwrap();
    let mut inside = step("true", &[], RunWhen::Always).unwrap();
    inside.cwd = Some(RelPath::parse("app").unwrap());
    let planned = plan(dir.path(), &spec(vec![inside], &[], &[]).unwrap()).unwrap();
    let expected: PathBuf = fs::canonicalize(dir.path().join("app")).unwrap();
    assert_eq!(planned.steps()[0].dir(), expected);
}

/// A buffer the log writes into, shared with the test.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = self
            .0
            .lock()
            .map_err(|_| io::Error::other("log buffer poisoned"))?;
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Output is logged from reader threads, which would drop it if they did
/// not carry the caller's subscriber.
#[test]
fn step_output_reaches_the_callers_log_line_by_line() {
    let dir = tempfile::tempdir().unwrap();
    let setup = vec![
        step(
            "sh",
            &["-c", "echo first-out; echo first-err >&2; echo second-out"],
            RunWhen::Always,
        )
        .unwrap(),
    ];
    let planned = plan(dir.path(), &spec(setup, &[], &[]).unwrap()).unwrap();
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::with_default(subscriber, || planned.execute()).unwrap();
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("fabric_env::setup") && line.contains("stream="))
        .collect();
    assert_eq!(lines.len(), 3, "{log}");
    let first_out = lines.iter().position(|l| l.contains("first-out")).unwrap();
    let second_out = lines.iter().position(|l| l.contains("second-out")).unwrap();
    assert!(first_out < second_out);
    assert!(lines[first_out].contains("stream=\"stdout\""));
    let err = lines.iter().find(|l| l.contains("first-err")).unwrap();
    assert!(err.contains("stream=\"stderr\""));
}
