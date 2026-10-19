//! Materialize: make a restored workspace runnable on this node (ADR 0007).
//!
//! Invariant: `plan` only reads. It never runs a program and never writes
//! a byte, so a dry run is `plan` and nothing more. `Plan::execute` runs
//! exactly the steps the plan lists, with the program paths and directories
//! the plan resolved, and runs none of them when the plan already shows that
//! one cannot run: a program missing from `PATH` or a declared environment
//! variable missing from this node.
//!
//! A cache is healthy when it is a non-empty directory that holds the
//! marker its installer writes last. The marker comes from the cache's
//! ecosystem in the detection report, so `node_modules` needs the marker of
//! the manager detection chose, whatever other lockfiles lie next to it,
//! `.venv` needs `pyvenv.cfg`, and any other cache needs only to be
//! non-empty. A first install killed halfway leaves a tree without its
//! marker, so the next plan installs again.
//!
//! The restore that runs before materialize writes a carried cache only when
//! its carry key matches this node (ADR 0012), so what is on disk is what
//! this node can use, and materialize trusts the disk.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use fabric_core::env::{EnvVarName, RunWhen, SetupStep};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use tracing::Dispatch;

use crate::detect::{CachePath, Detection};

/// The `tracing` target that carries every line a setup step prints.
pub const SETUP_LOG_TARGET: &str = "fabric_env::setup";

/// The most bytes of output one log event carries.
const MAX_LINE: u64 = 64 << 10;

/// Why materialize could not plan or finish.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    /// The workspace root does not resolve to a directory.
    #[error("workspace root `{}` is not readable", path.display())]
    Root {
        /// The root as given.
        path: PathBuf,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// A cache path could not be inspected.
    #[error("could not inspect cache `{path}`")]
    Inspect {
        /// The cache path.
        path: RelPath,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// A step's directory does not resolve.
    #[error("the directory of setup step {index} does not resolve")]
    StepDir {
        /// Zero-based index of the step in the spec.
        index: usize,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// A step's directory resolves through a link to somewhere outside the workspace.
    #[error("the directory of setup step {index} resolves outside the workspace")]
    StepDirOutsideRoot {
        /// Zero-based index of the step in the spec.
        index: usize,
    },
    /// A step's directory is not a directory.
    #[error("the directory of setup step {index} is not a directory")]
    StepDirNotADirectory {
        /// Zero-based index of the step in the spec.
        index: usize,
    },
    /// Declared environment variables are not set on this node; nothing ran.
    #[error(
        "set these environment variables on this node first: {}",
        names_list(names)
    )]
    MissingEnvVars {
        /// The names that are missing.
        names: Vec<EnvVarName>,
    },
    /// A step's program is not on this node's `PATH`; nothing ran.
    #[error("`{program}` of setup step {index} is not on PATH on this node; install it first")]
    ProgramNotFound {
        /// Zero-based index of the step in the spec.
        index: usize,
        /// The program.
        program: ProgramName,
    },
    /// A step's program did not start.
    #[error("setup step {index} (`{program}`) did not start")]
    Spawn {
        /// Zero-based index of the step in the spec.
        index: usize,
        /// The program.
        program: ProgramName,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// Waiting for a step's program failed.
    #[error("setup step {index} (`{program}`) could not be waited for")]
    Wait {
        /// Zero-based index of the step in the spec.
        index: usize,
        /// The program.
        program: ProgramName,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// A step exited unsuccessfully; the steps after it did not run.
    #[error("setup step {index} (`{program}`) failed: {status}")]
    StepFailed {
        /// Zero-based index of the step in the spec.
        index: usize,
        /// The program.
        program: ProgramName,
        /// How it exited.
        status: ExitStatus,
    },
}

fn names_list(names: &[EnvVarName]) -> String {
    names
        .iter()
        .map(EnvVarName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What materialize found at one cache path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheHealth {
    /// A non-empty directory with its installer's marker, when it has one.
    Healthy,
    /// Nothing at the path.
    Missing,
    /// Something other than a directory at the path.
    NotADirectory,
    /// An empty directory.
    Empty,
    /// A directory with content and none of the markers its installer writes last.
    Incomplete {
        /// The markers any one of which would make it healthy.
        expected: &'static [&'static str],
    },
}

/// One cache path and its health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheReport {
    /// The cache path, relative to the root.
    pub path: RelPath,
    /// What materialize found there.
    pub health: CacheHealth,
}

/// Why a step is in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepReason {
    /// At least one cache is not healthy.
    CachesMissing,
    /// The step runs on every materialize.
    Always,
}

/// A step the plan will run, with what the plan resolved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    index: usize,
    step: SetupStep,
    reason: StepReason,
    dir: PathBuf,
    program: Option<PathBuf>,
}

impl PlannedStep {
    /// Returns the step's zero-based index in the spec.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the step as the spec holds it.
    pub fn step(&self) -> &SetupStep {
        &self.step
    }

    /// Returns why the step runs.
    pub fn reason(&self) -> StepReason {
        self.reason
    }

    /// Returns the directory the step runs in, resolved and inside the workspace.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Returns where the program was found on `PATH`, or `None` when it was not.
    pub fn program(&self) -> Option<&Path> {
        self.program.as_deref()
    }
}

/// What materialize found and what it would run; computing it changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    caches: Vec<CacheReport>,
    steps: Vec<PlannedStep>,
    missing_env: Vec<EnvVarName>,
}

impl Plan {
    /// Returns the health of every cache path in the spec, in spec order.
    pub fn caches(&self) -> &[CacheReport] {
        &self.caches
    }

    /// Returns the steps that would run, in the order they would run.
    pub fn steps(&self) -> &[PlannedStep] {
        &self.steps
    }

    /// Returns the declared environment variables this node does not set.
    pub fn missing_env(&self) -> &[EnvVarName] {
        &self.missing_env
    }

    /// Returns true when executing the plan would run nothing.
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }

    /// Runs the planned steps in order, streaming their output to `tracing`.
    ///
    /// Refuses before running anything when a planned program is missing or
    /// a declared variable is unset. Stops at the first step that fails.
    pub fn execute(&self) -> Result<(), MaterializeError> {
        if self.steps.is_empty() {
            return Ok(());
        }
        if !self.missing_env.is_empty() {
            return Err(MaterializeError::MissingEnvVars {
                names: self.missing_env.clone(),
            });
        }
        let mut resolved = Vec::with_capacity(self.steps.len());
        for planned in &self.steps {
            let program =
                planned
                    .program
                    .as_deref()
                    .ok_or_else(|| MaterializeError::ProgramNotFound {
                        index: planned.index,
                        program: planned.step.program.clone(),
                    })?;
            resolved.push((planned, program));
        }
        for (planned, program) in resolved {
            run_step(planned, program)?;
        }
        Ok(())
    }
}

/// Inspects the workspace at `root` against `detected` and returns what materialize would do.
///
/// This is the dry run: it reads the disk, `PATH` and the presence of the
/// spec's environment variables, and changes nothing. It takes the
/// detection report rather than the spec alone because the report knows
/// each cache's ecosystem, which decides the marker that proves it whole.
pub fn plan(root: &Path, detected: &Detection) -> Result<Plan, MaterializeError> {
    let root = fs::canonicalize(root).map_err(|source| MaterializeError::Root {
        path: root.to_owned(),
        source,
    })?;
    if !root.is_dir() {
        return Err(MaterializeError::Root {
            path: root,
            source: io::Error::from(io::ErrorKind::NotADirectory),
        });
    }

    let spec = detected.spec();
    let caches = detected
        .caches()
        .iter()
        .map(|cache| {
            Ok(CacheReport {
                path: cache.path.clone(),
                health: inspect(&root, cache)?,
            })
        })
        .collect::<Result<Vec<_>, MaterializeError>>()?;
    let any_unhealthy = caches.iter().any(|c| c.health != CacheHealth::Healthy);

    let mut steps = Vec::new();
    for (index, step) in spec.setup.iter().enumerate() {
        let reason = match step.when {
            RunWhen::Always => StepReason::Always,
            RunWhen::CachesMissing if any_unhealthy => StepReason::CachesMissing,
            RunWhen::CachesMissing => continue,
        };
        steps.push(PlannedStep {
            index,
            step: step.clone(),
            reason,
            dir: step_dir(&root, index, step)?,
            program: find_on_path(env::var_os("PATH").as_deref(), &root, &step.program),
        });
    }

    let missing_env = spec
        .env_var_names
        .iter()
        .filter(|name| env::var_os(name.as_str()).is_none())
        .cloned()
        .collect();

    Ok(Plan {
        caches,
        steps,
        missing_env,
    })
}

fn inspect(root: &Path, cache: &CachePath) -> Result<CacheHealth, MaterializeError> {
    let full = root.join(cache.path.as_str());
    let inspect_err = |source| MaterializeError::Inspect {
        path: cache.path.clone(),
        source,
    };
    let meta = match fs::metadata(&full) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(CacheHealth::Missing),
        Err(err) => return Err(inspect_err(err)),
    };
    if !meta.is_dir() {
        return Ok(CacheHealth::NotADirectory);
    }
    if fs::read_dir(&full).map_err(inspect_err)?.next().is_none() {
        return Ok(CacheHealth::Empty);
    }
    let expected = cache.ecosystem.install_markers();
    if expected.is_empty()
        || expected
            .iter()
            .any(|m| fs::symlink_metadata(full.join(m)).is_ok())
    {
        Ok(CacheHealth::Healthy)
    } else {
        Ok(CacheHealth::Incomplete { expected })
    }
}

fn step_dir(root: &Path, index: usize, step: &SetupStep) -> Result<PathBuf, MaterializeError> {
    let Some(cwd) = &step.cwd else {
        return Ok(root.to_owned());
    };
    let dir = fs::canonicalize(root.join(cwd.as_str()))
        .map_err(|source| MaterializeError::StepDir { index, source })?;
    if !dir.starts_with(root) {
        return Err(MaterializeError::StepDirOutsideRoot { index });
    }
    if !dir.is_dir() {
        return Err(MaterializeError::StepDirNotADirectory { index });
    }
    Ok(dir)
}

/// Finds `program` in the entries of `PATH` that lie outside the workspace.
///
/// A restored workspace must not supply its own `pnpm`, so relative entries
/// such as `.` and absolute ones under `root`, such as a
/// `node_modules/.bin` a shell hook added, are skipped.
fn find_on_path(path: Option<&OsStr>, root: &Path, program: &ProgramName) -> Option<PathBuf> {
    env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .filter(|dir| !fs::canonicalize(dir).is_ok_and(|dir| dir.starts_with(root)))
        .map(|dir| dir.join(program.as_str()))
        .find(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

fn run_step(planned: &PlannedStep, program: &Path) -> Result<(), MaterializeError> {
    let index = planned.index;
    let name = &planned.step.program;
    tracing::info!(
        target: SETUP_LOG_TARGET,
        step = index,
        program = %name,
        reason = ?planned.reason,
        dir = %planned.dir.display(),
        "running setup step"
    );
    let mut child = Command::new(program)
        .args(&planned.step.args)
        .current_dir(&planned.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| MaterializeError::Spawn {
            index,
            program: name.clone(),
            source,
        })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Lines are logged from reader threads, which do not inherit the
    // caller's scoped subscriber; hand it to them.
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let status = thread::scope(|scope| {
        if let Some(out) = stdout {
            spawn_reader(scope, out, "stdout", index, name, &dispatch);
        }
        if let Some(err) = stderr {
            spawn_reader(scope, err, "stderr", index, name, &dispatch);
        }
        child.wait()
    })
    .map_err(|source| MaterializeError::Wait {
        index,
        program: name.clone(),
        source,
    })?;
    if !status.success() {
        return Err(MaterializeError::StepFailed {
            index,
            program: name.clone(),
            status,
        });
    }
    tracing::info!(target: SETUP_LOG_TARGET, step = index, program = %name, "setup step finished");
    Ok(())
}

/// Starts a thread that forwards `pipe` to the log.
///
/// If the thread cannot start, the pipe is dropped with the closure, so the
/// program's writes to it fail instead of filling a pipe nobody reads.
fn spawn_reader<'scope, 'env: 'scope>(
    scope: &'scope thread::Scope<'scope, 'env>,
    pipe: impl Read + Send + 'scope,
    stream: &'static str,
    index: usize,
    program: &'env ProgramName,
    dispatch: &'env Dispatch,
) {
    let spawned = thread::Builder::new().spawn_scoped(scope, move || {
        forward(pipe, stream, index, program, dispatch);
    });
    if let Err(err) = spawned {
        tracing::warn!(target: SETUP_LOG_TARGET, step = index, program = %program, stream, error = %err, "could not start a reader; this output is dropped");
    }
}

/// Logs each line of `pipe` as one event, until the program closes it.
///
/// A line longer than `MAX_LINE` bytes is logged in pieces of that size,
/// so a program that never prints a newline cannot grow the buffer without bound.
fn forward(
    pipe: impl Read,
    stream: &'static str,
    index: usize,
    program: &ProgramName,
    dispatch: &Dispatch,
) {
    tracing::dispatcher::with_default(dispatch, || {
        let mut reader = BufReader::new(pipe);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim_end_matches(['\n', '\r']);
                    tracing::info!(target: SETUP_LOG_TARGET, step = index, program = %program, stream, "{text}");
                }
                Err(err) => {
                    tracing::warn!(target: SETUP_LOG_TARGET, step = index, program = %program, stream, error = %err, "stopped reading output");
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A restored workspace that ships its own `pnpm` must not have it run,
    /// whether `PATH` names the workspace by a relative or an absolute entry.
    #[test]
    fn a_program_inside_the_workspace_is_never_found() {
        let root = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(root.path()).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let elsewhere = fs::canonicalize(elsewhere.path()).unwrap();
        executable(&root.join("node_modules/.bin/pnpm"));
        executable(&root.join("pnpm"));
        executable(&elsewhere.join("bin/pnpm"));
        fs::write(elsewhere.join("bin/cargo"), "not executable").unwrap();
        let pnpm = ProgramName::parse("pnpm").unwrap();

        let inside = env::join_paths([root.join("node_modules/.bin"), PathBuf::from(".")]).unwrap();
        assert_eq!(find_on_path(Some(&inside), &root, &pnpm), None);

        let both =
            env::join_paths([root.join("node_modules/.bin"), elsewhere.join("bin")]).unwrap();
        assert_eq!(
            find_on_path(Some(&both), &root, &pnpm),
            Some(elsewhere.join("bin/pnpm"))
        );
        let cargo = ProgramName::parse("cargo").unwrap();
        assert_eq!(find_on_path(Some(&both), &root, &cargo), None);
        assert_eq!(find_on_path(None, &root, &pnpm), None);
    }
}
