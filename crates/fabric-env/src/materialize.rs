//! Materialize: make a restored workspace runnable on this node (ADR 0007).
//!
//! Invariant: `plan` only reads. It never runs a program and never writes
//! a byte, so a dry run is `plan` and nothing more. `Plan::ready` turns a
//! plan into steps that can run, and refuses when the plan shows that one
//! cannot: a program missing from `PATH` or a declared environment variable
//! missing from this node. `Ready::execute` runs exactly those steps, with
//! the program paths and directories the plan resolved. Neither value can
//! be copied, so one plan runs at most once.
//!
//! A `CachesMissing` step runs when what it fills is not whole: a detected
//! install runs when its own cache is not, so a missing `target` never
//! reinstalls a whole `node_modules`; `cargo fetch` runs when `target`,
//! which a build makes from what it fetches, is not; and a step whose
//! cache is outside the workspace, or that the user declared, runs when
//! any cache is not whole or the report has none.
//!
//! A cache is healthy when it is a non-empty directory that holds proof
//! that whatever filled it finished. For npm, pnpm and Yarn that is the
//! marker the installer itself writes as it finishes, chosen by the manager
//! detection recorded, whatever other lockfiles lie next to it. For a cache
//! a setup step fills and whose installer writes no such marker (Bun,
//! `.venv`, a declared cache) it is `SENTINEL`, which materialize removes
//! before the first step that fills the cache and writes after the last
//! one exits 0. A cache no step fills needs only to be non-empty. An
//! install that fails or is killed halfway leaves no proof, so the next
//! plan fills the cache again. A report with no cache paths
//! has nothing that proves warmth, so its `CachesMissing` steps run on
//! every resume; declaring a `[[cache]]` is how a user makes them skip.
//!
//! The restore that runs before materialize writes a carried cache only when
//! its carry key matches this node (ADR 0012), so what is on disk is what
//! this node can use, and materialize trusts the disk.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use fabric_core::env::{EnvVarName, RunWhen, SetupStep};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use tracing::Dispatch;

use crate::detect::{CachePath, Completion, Detection, StepScope};

/// The file materialize writes in a cache once the steps that fill it have exited 0.
///
/// It is removed before the first of those steps starts and written after
/// the last one succeeds, so a step that fails or is killed leaves the
/// cache without it and the next plan fills the cache again.
pub const SENTINEL: &str = ".fabric-installed";

/// What the sentinel file holds; only its presence is read.
const SENTINEL_TEXT: &[u8] =
    b"Written by Fabric after the setup steps that fill this cache exited 0.\n";

/// The `tracing` target that carries every line a setup step prints.
pub const SETUP_LOG_TARGET: &str = "fabric_env::setup";

/// The most bytes of output one log event carries.
const MAX_LINE: u64 = 64 << 10;

/// How long a step's output may stay open after the step exits before `execute` moves on.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);

/// Why materialize could not plan or finish.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    /// The workspace root the detection report names is no longer a directory.
    #[error("workspace root `{}` is not readable", path.display())]
    Root {
        /// The root the detection report names.
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
    /// Programs are missing from `PATH` or declared variables are unset on this node; nothing ran.
    #[error(
        "nothing ran; fix all of this on this node first: {}",
        cannot_run_list(programs, env)
    )]
    CannotRun {
        /// Every planned program that is not on `PATH`, with its step index.
        programs: Vec<(usize, ProgramName)>,
        /// Every declared variable that is not set.
        env: Vec<EnvVarName>,
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
    /// The sentinel of a cache could not be removed before a step or written after it.
    #[error("could not update the completion sentinel of cache `{path}`")]
    Sentinel {
        /// The cache path.
        path: RelPath,
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

fn cannot_run_list(programs: &[(usize, ProgramName)], env: &[EnvVarName]) -> String {
    let mut parts: Vec<String> = programs
        .iter()
        .map(|(index, program)| format!("install `{program}` (setup step {index})"))
        .collect();
    parts.extend(env.iter().map(|name| format!("set {name}")));
    parts.join("; ")
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

/// A step the plan would run, with what the plan resolved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    index: usize,
    step: SetupStep,
    dir: PathBuf,
    program: Option<PathBuf>,
    sentinels: Sentinels,
}

/// The caches whose sentinel a step removes before it runs and writes after it succeeds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Sentinels {
    clear: Vec<RelPath>,
    write: Vec<RelPath>,
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
///
/// A plan is a report. It runs nothing itself and cannot be copied: the
/// one way to run it is `Plan::ready`, which consumes it.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    root: PathBuf,
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

    /// Returns true when running the plan would run nothing.
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }

    /// Turns the plan into steps that can run, or refuses with every missing program and variable named.
    ///
    /// A plan with no steps is ready whatever this node lacks, since nothing will run.
    pub fn ready(self) -> Result<Ready, MaterializeError> {
        if self.steps.is_empty() {
            return Ok(Ready {
                root: self.root,
                steps: Vec::new(),
            });
        }
        let mut steps = Vec::with_capacity(self.steps.len());
        let mut missing = Vec::new();
        for planned in self.steps {
            match planned.program {
                Some(program) => steps.push(ReadyStep {
                    index: planned.index,
                    step: planned.step,
                    dir: planned.dir,
                    program,
                    sentinels: planned.sentinels,
                }),
                None => missing.push((planned.index, planned.step.program)),
            }
        }
        if !missing.is_empty() || !self.missing_env.is_empty() {
            return Err(MaterializeError::CannotRun {
                programs: missing,
                env: self.missing_env,
            });
        }
        Ok(Ready {
            root: self.root,
            steps,
        })
    }
}

/// A step whose program was found, so it can run.
#[derive(Debug)]
struct ReadyStep {
    index: usize,
    step: SetupStep,
    dir: PathBuf,
    program: PathBuf,
    sentinels: Sentinels,
}

/// Steps that passed every check `Plan::ready` makes; running them consumes this value.
#[derive(Debug)]
pub struct Ready {
    root: PathBuf,
    steps: Vec<ReadyStep>,
}

impl Ready {
    /// Runs the steps in order, streaming their output to `tracing`, and stops at the first that fails.
    ///
    /// Takes the value: what the plan read from the disk is stale once a
    /// step has run, so a second run needs a new plan.
    pub fn execute(self) -> Result<(), MaterializeError> {
        for step in &self.steps {
            for path in &step.sentinels.clear {
                clear_sentinel(&self.root, path)?;
            }
            run_step(step)?;
            for path in &step.sentinels.write {
                write_sentinel(&self.root, path)?;
            }
        }
        Ok(())
    }
}

/// Inspects the workspace `detected` describes and returns what materialize would do.
///
/// This is the dry run: it reads the disk, `PATH` and the presence of the
/// spec's environment variables, and changes nothing. It takes the
/// detection report rather than the spec alone because the report knows
/// each cache's ecosystem, which decides the marker that proves it whole,
/// and the root it was built from, so a report cannot be checked against
/// another tree.
pub fn plan(detected: &Detection) -> Result<Plan, MaterializeError> {
    let root = detected.root().to_owned();
    match fs::metadata(&root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(MaterializeError::Root {
                path: root,
                source: io::Error::from(io::ErrorKind::NotADirectory),
            });
        }
        Err(source) => return Err(MaterializeError::Root { path: root, source }),
    }

    let spec = detected.spec();
    let caches = detected
        .caches()
        .iter()
        .map(|cache| {
            Ok(CacheReport {
                path: cache.path.clone(),
                health: inspect(&root, cache, detected.completion(cache))?,
            })
        })
        .collect::<Result<Vec<_>, MaterializeError>>()?;
    let any_unhealthy =
        caches.is_empty() || caches.iter().any(|c| c.health != CacheHealth::Healthy);
    let unhealthy = |path: &RelPath| {
        caches
            .iter()
            .any(|c| &c.path == path && c.health != CacheHealth::Healthy)
    };

    let mut steps = Vec::new();
    for (index, (step, scope)) in spec.setup.iter().zip(detected.step_scopes()).enumerate() {
        let needed = match scope {
            StepScope::Fills(path) | StepScope::Warms(path) => unhealthy(path),
            StepScope::Outside | StepScope::Declared => any_unhealthy,
        };
        if step.when == RunWhen::CachesMissing && !needed {
            continue;
        }
        steps.push(PlannedStep {
            index,
            step: step.clone(),
            dir: step_dir(&root, index, step)?,
            program: find_on_path(env::var_os("PATH").as_deref(), &root, &step.program),
            sentinels: Sentinels::default(),
        });
    }
    assign_sentinels(detected, &mut steps);

    let missing_env = spec
        .env_var_names
        .iter()
        .filter(|name| env::var_os(name.as_str()).is_none())
        .cloned()
        .collect();

    Ok(Plan {
        root,
        caches,
        steps,
        missing_env,
    })
}

/// Gives each sentinel cache to the planned steps that fill it: the first clears it, the last writes it.
fn assign_sentinels(detected: &Detection, steps: &mut [PlannedStep]) {
    let scopes = detected.step_scopes();
    for cache in detected.caches() {
        if detected.completion(cache) != Completion::Sentinel {
            continue;
        }
        // Completion is Sentinel only for a cache some step fills, and for
        // such a cache a declared step always counts as filling it.
        let fills = |planned: &PlannedStep| match scopes.get(planned.index) {
            Some(StepScope::Fills(path)) => path == &cache.path,
            Some(StepScope::Declared) => true,
            Some(StepScope::Warms(_) | StepScope::Outside) | None => false,
        };
        if let Some(first) = steps.iter().position(fills) {
            steps[first].sentinels.clear.push(cache.path.clone());
        }
        if let Some(last) = steps.iter().rposition(fills) {
            steps[last].sentinels.write.push(cache.path.clone());
        }
    }
}

fn inspect(
    root: &Path,
    cache: &CachePath,
    completion: Completion,
) -> Result<CacheHealth, MaterializeError> {
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
    let expected: &'static [&'static str] = match completion {
        Completion::Content => return Ok(CacheHealth::Healthy),
        Completion::Marker(markers) => markers,
        Completion::Sentinel => &[SENTINEL],
    };
    if expected
        .iter()
        .any(|m| fs::symlink_metadata(full.join(m)).is_ok_and(|meta| meta.is_file()))
    {
        Ok(CacheHealth::Healthy)
    } else {
        Ok(CacheHealth::Incomplete { expected })
    }
}

/// Removes the sentinel of the cache at `path`; nothing there is fine.
fn clear_sentinel(root: &Path, path: &RelPath) -> Result<(), MaterializeError> {
    match fs::remove_file(root.join(path.as_str()).join(SENTINEL)) {
        Err(err)
            if err.kind() != io::ErrorKind::NotFound
                && err.kind() != io::ErrorKind::NotADirectory =>
        {
            Err(MaterializeError::Sentinel {
                path: path.clone(),
                source: err,
            })
        }
        _ => Ok(()),
    }
}

/// Writes the sentinel of the cache at `path` when the steps left a real directory there.
///
/// A cache the steps did not create, or one that is a link, gets no
/// sentinel: the next plan sees it missing or torn and fills it again,
/// which costs time and never writes outside the workspace.
fn write_sentinel(root: &Path, path: &RelPath) -> Result<(), MaterializeError> {
    let dir = root.join(path.as_str());
    if !fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
        tracing::warn!(target: SETUP_LOG_TARGET, cache = %path, "the setup steps left no directory at this cache path; it will be filled again on the next resume");
        return Ok(());
    }
    let sentinel_err = |source| MaterializeError::Sentinel {
        path: path.clone(),
        source,
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(SENTINEL))
        .map_err(sentinel_err)?;
    file.write_all(SENTINEL_TEXT).map_err(sentinel_err)
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

/// Finds `program` in the entries of `PATH` that resolve outside the workspace.
///
/// A restored workspace must not supply its own `pnpm`, so relative entries
/// such as `.` and absolute ones under `root`, such as a
/// `node_modules/.bin` a shell hook added, are skipped, and so is a
/// candidate that is a link back into the workspace from an outside
/// directory, such as `~/.local/bin/pnpm -> <root>/node_modules/.bin/pnpm`.
fn find_on_path(path: Option<&OsStr>, root: &Path, program: &ProgramName) -> Option<PathBuf> {
    env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .filter(|dir| !fs::canonicalize(dir).is_ok_and(|dir| dir.starts_with(root)))
        .map(|dir| dir.join(program.as_str()))
        .find(|candidate| {
            fs::canonicalize(candidate).is_ok_and(|resolved| !resolved.starts_with(root))
                && fs::metadata(candidate)
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

fn run_step(planned: &ReadyStep) -> Result<(), MaterializeError> {
    let index = planned.index;
    let name = &planned.step.program;
    let program = &planned.program;
    tracing::info!(
        target: SETUP_LOG_TARGET,
        step = index,
        program = %name,
        when = ?planned.step.when,
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
    // Lines are logged from reader threads, which do not inherit the
    // caller's scoped subscriber; hand it to them.
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let (done, finished) = mpsc::channel();
    let mut open = 0;
    if let Some(out) = child.stdout.take() {
        open += spawn_reader(out, "stdout", index, name, &dispatch, done.clone());
    }
    if let Some(err) = child.stderr.take() {
        open += spawn_reader(err, "stderr", index, name, &dispatch, done.clone());
    }
    drop(done);
    let status = child.wait().map_err(|source| MaterializeError::Wait {
        index,
        program: name.clone(),
        source,
    })?;
    // A pipe reaches EOF only when every process holding it has exited. A
    // step that started a daemon leaves it open, so the readers get a
    // bounded grace after the step itself exits and then keep running on
    // their own; whatever they read later still reaches the log.
    let deadline = Instant::now() + OUTPUT_GRACE;
    while open > 0 {
        match finished.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(()) => open -= 1,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!(
                    target: SETUP_LOG_TARGET,
                    step = index,
                    program = %name,
                    "setup step exited but its output is still open: a process it started is still running"
                );
                break;
            }
        }
    }
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

/// Starts a thread that forwards `pipe` to the log and reports on `done` at EOF; returns how many threads started.
///
/// If the thread cannot start, the pipe is dropped with the closure, so the
/// program's writes to it fail instead of filling a pipe nobody reads.
fn spawn_reader(
    pipe: impl Read + Send + 'static,
    stream: &'static str,
    index: usize,
    program: &ProgramName,
    dispatch: &Dispatch,
    done: Sender<()>,
) -> usize {
    let owned = program.clone();
    let dispatch = dispatch.clone();
    let spawned = thread::Builder::new().spawn(move || {
        forward(pipe, stream, index, &owned, &dispatch);
        // The receiver is gone once run_step has moved on; nothing is owed then.
        done.send(()).ok();
    });
    match spawned {
        Ok(_) => 1,
        Err(err) => {
            tracing::warn!(target: SETUP_LOG_TARGET, step = index, program = %program, stream, error = %err, "could not start a reader; this output is dropped");
            0
        }
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
    /// whether `PATH` names the workspace by a relative entry, an absolute
    /// one, or an outside directory whose `pnpm` is a link back into it.
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
        fs::create_dir(elsewhere.join("links")).unwrap();
        std::os::unix::fs::symlink(
            root.join("node_modules/.bin/pnpm"),
            elsewhere.join("links/pnpm"),
        )
        .unwrap();
        let pnpm = ProgramName::parse("pnpm").unwrap();

        let inside = env::join_paths([
            root.join("node_modules/.bin"),
            PathBuf::from("."),
            elsewhere.join("links"),
        ])
        .unwrap();
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
