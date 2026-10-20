//! Materialize: make a restored workspace runnable on this node (ADR 0007).
//!
//! Invariant: `plan` only reads. It never runs a program and never writes a
//! byte, so a dry run is `plan` and nothing more. What it reads from the
//! node's environment comes in through `NodeEnv`: `PATH`, and whether each
//! declared variable is set, never a value. `Plan::ready` turns a plan
//! into steps that can run, and refuses when the plan shows that one
//! cannot: a program missing from `PATH` or a declared
//! environment variable missing from this node. `Ready::execute` runs
//! exactly those steps, with the program paths and directories the plan
//! resolved. Neither value can be copied, so one plan runs at most once.
//!
//! A `CachesMissing` step runs when what it fills is not whole: a detected
//! install runs when its own cache is not, and a declared step when one of
//! the caches it fills is not, so a missing `target` never reinstalls a
//! whole `node_modules`; `cargo fetch` runs when `target`, which a build
//! makes from what it fetches, is not; and a step that fills no cache in
//! the workspace runs when any cache is not whole or the report has none.
//!
//! A cache is healthy when it is a non-empty directory that holds proof
//! that whatever filled it finished. For a cache a setup step fills, the
//! detected `node_modules` included, that proof is `SENTINEL`, which
//! materialize removes before the first step that fills the cache and
//! writes after the last one exits 0. An installer's own file is never
//! proof: npm, pnpm and Yarn 1 write theirs before the root package's
//! lifecycle scripts run, so a `postinstall` that fails or is interrupted
//! leaves it over a tree that is not whole. A cache no step fills needs
//! only to be non-empty. An install that fails or is killed halfway
//! leaves no proof, so the next plan fills the cache again. A report with
//! no cache paths has nothing that proves warmth, so its `CachesMissing`
//! steps run on every resume; declaring a `[[cache]]` is how a user makes
//! them skip. A `node_modules` the user installed by hand has no sentinel,
//! so the first resume after it installs once more; that is the price of
//! never trusting a tree no step was seen to finish.
//!
//! The restore that runs before materialize writes a carried cache only when
//! its carry key matches this node (ADR 0012), so what is on disk is what
//! this node can use, and materialize trusts the disk.
//!
//! Trust: setup runs code that comes with the snapshot. `npm ci` and
//! `pnpm install` run the root package's lifecycle scripts and
//! `.pnpmfile.cjs`, `uv sync` builds source distributions, and a declared
//! step can be `sh -c ...`. A restored workspace is therefore trusted as
//! far as the peer that signed its snapshot, and the `PATH` filter below
//! only keeps the workspace from replacing the installer binary itself.
//! The prompt before a spec this node has not run is the CLI's (ADR 0007).
//!
//! One installer per tree: a step is not tied to the `fabric resume`
//! process, so after a `kill -9` of resume the installer keeps running.
//! `TreeLock` is a `flock` on a file in this node's Fabric state, named by
//! the hash of the workspace root, never a file in the workspace: the
//! agent, `git clean -xfd` or a restore in place can delete or replace
//! anything in the tree, and a lock they can unlink guards nothing. The
//! caller takes it before restore writes the tree and holds it through
//! `plan` and `execute`, which take it by reference, so neither can run
//! without it. `execute` hands that same open file to every step as its
//! standard input, an empty file that reads as end of input. The lock
//! belongs to the open file, which the installer and the processes that
//! inherit its input share, so it outlives a killed resume and ends when
//! the last of them exits; until then `TreeLock::acquire` refuses, so a
//! second resume neither restores over nor starts a second installer on a
//! tree the first one is still writing. A process a step leaves behind
//! with that input still open, such as a daemon that did not redirect it,
//! holds the lock too, and resumes refuse until it exits.
//!
//! The reverse gap is open: a process the step starts with its own
//! standard input does not hold the lock. npm runs dependency install
//! scripts, such as a `node-gyp` build, with piped input, so if the
//! installer alone is killed (the OOM killer, a force quit) while such a
//! build still writes, the lock is released and the next resume can start
//! `npm ci`, which deletes `node_modules` under it. Closing this needs the
//! step's process group recorded next to the lock and checked by
//! `acquire`, and asking whether a group is alive takes a system call that
//! the standard library does not wrap and this crate's `forbid(unsafe_code)`
//! rules out calling directly; ADR 0007 records the gap.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use fabric_core::env::{EnvVarName, RunWhen, SetupArg, SetupStep};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use tracing::Dispatch;

use crate::detect::{CachePath, Completion, Detection, StepGate};

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
    /// Another resume, or a setup step one started, holds the tree's lock; nothing was restored, planned or run.
    #[error(
        "another resume, or a setup step one started, holds this workspace; wait for it to finish or stop the process that holds `{}` open",
        lock.display()
    )]
    TreeBusy {
        /// The lock file the holder has open.
        lock: PathBuf,
    },
    /// The lock file could not be opened, created or locked.
    #[error("could not take the lock `{}`", lock.display())]
    Lock {
        /// The lock file, or the directory that holds it.
        lock: PathBuf,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// The directory for locks lies inside the workspace, where the workspace could delete a lock.
    #[error("the lock directory `{}` lies inside the workspace `{}`", locks.display(), root.display())]
    LocksInsideWorkspace {
        /// The lock directory, resolved.
        locks: PathBuf,
        /// The workspace root, resolved.
        root: PathBuf,
    },
    /// The lock passed in was taken for another workspace than the one being planned or run.
    #[error("the lock held is for `{}`, not for `{}`", locked.display(), root.display())]
    WrongTree {
        /// The root the lock was taken for.
        locked: PathBuf,
        /// The root being planned or run.
        root: PathBuf,
    },
    /// The workspace root does not resolve or is not a directory.
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
    /// A non-empty directory with the proof its completion needs, when it needs one.
    Healthy,
    /// Nothing at the path.
    Missing,
    /// Something other than a real directory at the path or on the way to it, a link included.
    NotADirectory,
    /// An empty directory.
    Empty,
    /// A directory with content and no `SENTINEL`, so the steps that fill it may not have finished.
    Incomplete,
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

/// What `plan` reads from this node's environment: `PATH`, and whether each declared variable is set.
///
/// It decides only what `plan` checks. A step runs with the environment
/// of the process that calls `execute`, as ADR 0007 says.
#[derive(Debug, Clone)]
pub struct NodeEnv {
    path: Option<OsString>,
    vars: Vars,
}

/// Where `NodeEnv` looks up whether a variable is set.
#[derive(Debug, Clone)]
enum Vars {
    /// This process's environment, read when `plan` asks.
    Process,
    /// Exactly these names, as the caller resolved them.
    Listed(Vec<EnvVarName>),
}

impl NodeEnv {
    /// Returns this process's environment: its `PATH` now, and its variables when `plan` checks them.
    pub fn current() -> Self {
        Self {
            path: env::var_os("PATH"),
            vars: Vars::Process,
        }
    }

    /// Returns an environment with this `PATH` in which exactly the names in `set` are set.
    pub fn new(path: Option<OsString>, set: Vec<EnvVarName>) -> Self {
        Self {
            path,
            vars: Vars::Listed(set),
        }
    }

    /// Returns true when `name` is set; its value is never kept.
    fn is_set(&self, name: &EnvVarName) -> bool {
        match &self.vars {
            Vars::Process => env::var_os(name.as_str()).is_some(),
            Vars::Listed(set) => set.contains(name),
        }
    }
}

/// The lock that keeps one installer per workspace tree, held in this node's Fabric state.
///
/// Dropping it releases the lock, unless a setup step still holds the
/// same open file as its standard input.
#[derive(Debug)]
pub struct TreeLock {
    root: PathBuf,
    file: fs::File,
}

impl TreeLock {
    /// Takes the lock for the workspace at `root` in `locks`, a directory of this node's Fabric state, creating it when absent.
    ///
    /// The root must exist: the caller creates it, takes the lock, and only
    /// then restores into it. Refuses with `TreeBusy` while another resume
    /// holds the tree, or a step one started still runs in it, whatever has
    /// happened to the files in the tree since.
    pub fn acquire(locks: &Path, root: &Path) -> Result<Self, MaterializeError> {
        let root_err = |source| MaterializeError::Root {
            path: root.to_owned(),
            source,
        };
        let root = fs::canonicalize(root).map_err(root_err)?;
        if !fs::metadata(&root).map_err(root_err)?.is_dir() {
            return Err(root_err(io::Error::from(io::ErrorKind::NotADirectory)));
        }
        let locks_err = |source| MaterializeError::Lock {
            lock: locks.to_owned(),
            source,
        };
        fs::create_dir_all(locks).map_err(locks_err)?;
        let locks = fs::canonicalize(locks).map_err(locks_err)?;
        if locks.starts_with(&root) {
            return Err(MaterializeError::LocksInsideWorkspace { locks, root });
        }
        let name = blake3::hash(root.as_os_str().as_bytes()).to_hex();
        let path = locks.join(format!("{name}.lock"));
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| MaterializeError::Lock {
                lock: path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { root, file }),
            Err(TryLockError::WouldBlock) => Err(MaterializeError::TreeBusy { lock: path }),
            Err(TryLockError::Error(source)) => Err(MaterializeError::Lock { lock: path, source }),
        }
    }

    /// Refuses when the lock was taken for another tree than `root`.
    fn covers(&self, root: &Path) -> Result<(), MaterializeError> {
        if self.root == root {
            Ok(())
        } else {
            Err(MaterializeError::WrongTree {
                locked: self.root.clone(),
                root: root.to_owned(),
            })
        }
    }
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
    /// step has run, so a second run needs a new plan. `lock` must be the
    /// tree's lock the plan was made under; each step gets its open file
    /// as standard input, so the lock lasts as long as the step does.
    pub fn execute(self, lock: &TreeLock) -> Result<(), MaterializeError> {
        lock.covers(&self.root)?;
        for step in &self.steps {
            for path in &step.sentinels.clear {
                clear_sentinel(&self.root, path)?;
            }
            run_step(step, &lock.file)?;
            for path in &step.sentinels.write {
                write_sentinel(&self.root, path)?;
            }
        }
        Ok(())
    }
}

/// Inspects the workspace `detected` describes and returns what materialize would do.
///
/// This is the dry run: it reads the disk, and from `node` the `PATH` and
/// the presence of the spec's environment variables, and changes nothing. `lock` proves no
/// installer from an earlier resume is still writing the tree. It takes the
/// detection report rather than the spec alone because the report knows
/// which steps fill each cache, which decides whether the cache needs
/// `SENTINEL`, and the root it was built from, so a report cannot be
/// checked against another tree.
pub fn plan(
    detected: &Detection,
    lock: &TreeLock,
    node: &NodeEnv,
) -> Result<Plan, MaterializeError> {
    let root = detected.root().to_owned();
    lock.covers(&root)?;
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
                health: inspect(&root, cache)?,
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
    for (index, (step, gate)) in detected.steps().enumerate() {
        let needed = match gate {
            StepGate::Caches(paths) => paths.iter().any(unhealthy),
            StepGate::AnyCache => any_unhealthy,
        };
        if step.when == RunWhen::CachesMissing && !needed {
            continue;
        }
        steps.push(PlannedStep {
            index,
            step: step.clone(),
            dir: step_dir(&root, index, step)?,
            program: find_on_path(node.path.as_deref(), &root, &step.program),
            sentinels: Sentinels::default(),
        });
    }
    assign_sentinels(detected, &mut steps);

    let missing_env = spec
        .env_var_names
        .iter()
        .filter(|name| !node.is_set(name))
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
    for cache in detected.caches() {
        let Completion::Sentinel { filled_by } = &cache.completion else {
            continue;
        };
        let fills = |planned: &PlannedStep| filled_by.contains(&planned.index);
        if let Some(first) = steps.iter().position(fills) {
            steps[first].sentinels.clear.push(cache.path.clone());
        }
        if let Some(last) = steps.iter().rposition(fills) {
            steps[last].sentinels.write.push(cache.path.clone());
        }
    }
}

/// What lies at a cache path, read without following a link in any component.
enum Found {
    /// Nothing at the path or on the way to it.
    Missing,
    /// A link, or something other than a directory, at the path or on the way to it.
    NotADirectory,
    /// A real directory, reached through real directories.
    Dir(PathBuf),
}

/// Walks `path` under `root` component by component, so a linked parent such as `web -> /elsewhere` is never followed.
///
/// A cache reached through a link is not inside the workspace: judging it,
/// or writing its sentinel, would read or write somewhere else.
fn find_dir(root: &Path, path: &RelPath) -> io::Result<Found> {
    let mut full = root.to_path_buf();
    for component in path.as_str().split('/') {
        full.push(component);
        match fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(Found::NotADirectory),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Found::Missing),
            Err(err) => return Err(err),
        }
    }
    Ok(Found::Dir(full))
}

fn inspect(root: &Path, cache: &CachePath) -> Result<CacheHealth, MaterializeError> {
    let inspect_err = |source| MaterializeError::Inspect {
        path: cache.path.clone(),
        source,
    };
    let full = match find_dir(root, &cache.path).map_err(inspect_err)? {
        Found::Missing => return Ok(CacheHealth::Missing),
        Found::NotADirectory => return Ok(CacheHealth::NotADirectory),
        Found::Dir(full) => full,
    };
    if fs::read_dir(&full).map_err(inspect_err)?.next().is_none() {
        return Ok(CacheHealth::Empty);
    }
    match &cache.completion {
        Completion::Content => Ok(CacheHealth::Healthy),
        Completion::Sentinel { .. } => {
            if fs::symlink_metadata(full.join(SENTINEL)).is_ok_and(|meta| meta.is_file()) {
                Ok(CacheHealth::Healthy)
            } else {
                Ok(CacheHealth::Incomplete)
            }
        }
    }
}

/// Removes the sentinel of the cache at `path`; nothing there, or a cache reached through a link, is left alone.
///
/// A linked cache is never judged healthy, so a sentinel beyond the link
/// proves nothing here and is not this workspace's to remove.
fn clear_sentinel(root: &Path, path: &RelPath) -> Result<(), MaterializeError> {
    let sentinel_err = |source| MaterializeError::Sentinel {
        path: path.clone(),
        source,
    };
    let Found::Dir(dir) = find_dir(root, path).map_err(sentinel_err)? else {
        return Ok(());
    };
    match fs::remove_file(dir.join(SENTINEL)) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(sentinel_err(err)),
        _ => Ok(()),
    }
}

/// Writes the sentinel of the cache at `path` when the steps left a real directory there, reached through real directories.
///
/// A cache the steps did not create, or one that is or lies under a link,
/// gets no sentinel: the next plan sees it missing or not a directory and
/// fills it again, which costs time and never writes outside the workspace.
/// A regular file the steps left at the sentinel's name, such as one
/// unpacked from an archive of a whole cache, is kept: the steps exited 0,
/// which is all the sentinel says. Anything else there, a link included,
/// is removed and the sentinel created in its place, so a link is never
/// followed.
fn write_sentinel(root: &Path, path: &RelPath) -> Result<(), MaterializeError> {
    let sentinel_err = |source| MaterializeError::Sentinel {
        path: path.clone(),
        source,
    };
    let Found::Dir(dir) = find_dir(root, path).map_err(sentinel_err)? else {
        tracing::warn!(target: SETUP_LOG_TARGET, cache = %path, "the setup steps left no real directory at this cache path; it will be filled again on the next resume");
        return Ok(());
    };
    let sentinel = dir.join(SENTINEL);
    let create = || {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&sentinel)
    };
    let mut file = match create() {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(&sentinel)
                .map_err(sentinel_err)?
                .is_file()
            {
                return Ok(());
            }
            fs::remove_file(&sentinel).map_err(sentinel_err)?;
            create().map_err(sentinel_err)?
        }
        Err(err) => return Err(sentinel_err(err)),
    };
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
/// A restored workspace must not replace the installer binary, so relative entries
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

fn run_step(planned: &ReadyStep, lock: &fs::File) -> Result<(), MaterializeError> {
    let index = planned.index;
    let name = &planned.step.program;
    let program = &planned.program;
    let spawn_err = |source| MaterializeError::Spawn {
        index,
        program: name.clone(),
        source,
    };
    // The step inherits the locked file itself, not a copy of the lock, so
    // the lock lasts as long as the step does, whatever happens to resume.
    let stdin = lock.try_clone().map_err(spawn_err)?;
    tracing::info!(
        target: SETUP_LOG_TARGET,
        step = index,
        program = %name,
        when = ?planned.step.when,
        dir = %planned.dir.display(),
        "running setup step"
    );
    let mut child = Command::new(program)
        .args(planned.step.args.iter().map(SetupArg::as_str))
        .current_dir(&planned.dir)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
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
