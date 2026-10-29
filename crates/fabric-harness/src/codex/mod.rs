//! The Codex adapter.
//!
//! Invariant: a Codex thread is carried as its rollout lineage, byte for
//! byte, under Codex's own thread writer lock for the whole copy, and
//! placed back, under the workspace path and home it was captured under
//! (ADR 0011), either over the file that already holds the same rollout on
//! this node or under `$CODEX_HOME/sessions/fabric/<thread_id>/`, a folder
//! Codex's file-name scan accepts; a manifest path that is not a rollout
//! under `sessions/` or `archived_sessions/` is refused. Nothing is
//! rewritten: Codex retargets
//! the thread to the directory it is resumed from (H2, experiments E2,
//! E3b), and a paginated prefix is referenced by byte offset. Because the
//! bytes travel verbatim, a rollout that holds a secret is refused. Two
//! files that share one rollout id are accepted only when one is a byte
//! prefix of the other, so a thread that went to another node and came
//! back never forks silently. `auth.json`, the `SQLite` files, the locks and
//! `config.toml` never travel. `CODEX_HOME` is never set per job because
//! the keyring entry is derived from it (ADR 0006, D15); trust for the new
//! path is passed as a `-c` override on every run. `check_env` refuses a
//! launch environment under which Codex would use another directory than
//! the store, because Codex would then write the thread and look for its
//! login there while capture reads the store, and could carry a stale
//! copy of the same thread. Codex treats an empty `CODEX_HOME` as unset,
//! canonicalizes a set one, and keys its keyring entry by the canonical
//! path (`codex-rs/utils/home-dir/src/lib.rs` and
//! `codex-rs/login/src/auth/storage.rs` at `e4568a3`), so a value that
//! names the store under another spelling is accepted.
//!
//! The lock protocol follows `codex-rs/rollout/src/writer_lock.rs` at
//! `e4568a3`: `thread-writer-locks/.coordination.lock` is held only while
//! a `<thread_id>.lock` is opened and tried, and again while it is
//! removed; the thread lock is held for the whole copy. The lock is
//! trusted only when the installed Codex is at least the version it was
//! checked on; with an older or unknown version, any running `codex`
//! refuses the capture or restore, because a Codex that takes no lock
//! would keep appending to a rollout that restore renamed away. Both ask
//! the installed Codex from the daemon's directory, since a running one
//! may have started anywhere; a workspace pin this node lacks, whose
//! shim answers no version in the workspace, does not block them. The
//! installed version says nothing about a `codex` started before the
//! binary on disk was upgraded, so capture and restore also refuse, under
//! the lock, while any process holds one of the thread's rollout files
//! open: Codex's rollout writer keeps the file open while the thread is
//! loaded (`RolloutWriterState::writer` in
//! `codex-rs/rollout/src/recorder.rs` at `e4568a3`). A writer that has the
//! file closed at that moment, as Codex's has after a failed write until
//! it retries, is not seen this way.
//!
//! Surfaces used, checked against `codex --help`, `codex resume --help`
//! and `codex exec resume --help` of 0.155.1 on 2026-10-19: `codex resume
//! <id> [PROMPT]` for the terminal and `codex exec resume <id> [PROMPT]`
//! for a headless run, both with `-c key=value` overrides built from the
//! typed `overrides::CodexOverrides`, never from free text, placed after
//! the subcommand: a `-c` before `exec` is ignored by 0.155.1 (seen on the
//! gate, 2026-10-21). The prompt is passed after `--`, which clap reads as
//! the end of options. The app-server JSON-RPC surface is not used (ADR
//! 0006, D14).
//!
//! A resume carries the job's settings from the session itself (gate
//! finding F10a): Codex resumes with the model of this node's
//! `config.toml`, not the one the thread ran with, so the launch reads
//! the last `turn_context` of the lineage and passes its model, with the
//! provider that served it, effort, sandbox and, on a terminal run,
//! approval policy and approvals reviewer, each one the adapter was
//! configured with winning (`overrides::effective`); `codex exec` sets its own approval policy
//! and records the `never` it forced, so a terminal run does not carry
//! a recorded `never`. Before anything starts it asks the binary it
//! would run for its version and refuses when that is older than the
//! newest Codex known to have written the thread, or unreadable
//! (`version::refuse_older`, gate finding F10b). That is the highest
//! `cli_version` the lineage records or the known writer the job
//! carried (`with_known_writer`), whichever is newer: Codex writes
//! `session_meta`, with its version, only when it creates a rollout,
//! and a resume appends with no version at all
//! (`codex-rs/rollout/src/recorder.rs` at `rust-v0.155.1`,
//! `RolloutRecorderParams::Resume` with `meta: None`), so the rollout
//! of a thread created by 0.142.4 and continued by 0.155.1 names only
//! 0.142.4. A resume reports the version the guard asked in the
//! workspace (`Running::version`), the Codex that now runs the thread,
//! and the node raises the known writer to it; a capture records the
//! installed version, asked in the workspace as the guard asks it, or the
//! known writer when that is newer, so the job carries the newest Codex
//! that ran the thread under Fabric to the next node. The capture runs in
//! the daemon, with its environment and the CLI's `PATH`, so a shim that
//! picks the version by an environment variable the CLI set can still
//! answer it with another Codex than the launch's; the directory, which
//! `.tool-versions` and `mise.toml` pin by, is the same. A Codex that resumed the thread outside Fabric, such
//! as a desktop build newer than the CLI on the same node, leaves no
//! version anywhere and is not seen. A thread with no rollout on this
//! node has no bytes to protect, so it is launched as before and Codex
//! says it found none. A rollout of the thread that only a symlink under
//! `sessions/` or `archived_sessions/` reaches is not "no rollout": Codex
//! opens it through the link, from the path its database holds, so the
//! guard refuses it (`SymlinkInState`) by the walk capture uses
//! (`lineage`), as capture does. A launch with no session reads nothing
//! and passes only what was configured.

pub mod overrides;
pub(crate) mod rollout;

use std::ffi::OsString;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use tracing::{info, warn};

use crate::error::{HarnessError, Input, IoOp};
use crate::harness::{Harness, Installed, Launch, Restored, Running, TurnReport};
use crate::manifest::{Content, HOME_PREFIX, Mutability, StateManifest};
use crate::pipeline::{self, Capturer, Destination};
use crate::place::{self, Relation};
use crate::process;
use crate::redact::RedactionCounts;
use crate::stage::{Carry, Source};
use crate::version::HarnessVersion;

use self::overrides::{CodexOverrides, LaunchOverrides};
use self::rollout::{RecordedSettings, RolloutFile, RolloutName};

/// The program name on `PATH`.
pub const PROGRAM: &str = "codex";

/// The folder under `sessions/` where restored lineages are placed when
/// this node holds no file for the rollout yet.
pub const RESTORE_SUBDIR: &str = "fabric";

const LOCK_DIR: &str = "thread-writer-locks";
const COORDINATION_LOCK: &str = ".coordination.lock";

/// The Codex version whose thread writer lock this adapter was checked against.
const LOCK_READ_ON: &str = "0.155.1";

/// How long a lock attempt waits for Codex's coordination lock, which
/// Codex holds only across an open and a `try_lock`.
const COORDINATION_WAIT: Duration = Duration::from_secs(2);

/// The variable that names Codex's home directory.
const HOME_VAR: &str = "CODEX_HOME";

/// The Codex adapter for one `CODEX_HOME` under one home directory.
#[derive(Debug, Clone)]
pub struct Codex {
    home: AbsPath,
    store: PathBuf,
    binary: Option<PathBuf>,
    path_var: OsString,
    overrides: Option<CodexOverrides>,
    known_writer: Option<HarnessVersion>,
    /// The installed version, asked ahead for a restore (`with_installed_ahead`).
    installed_ahead: Option<VersionAhead>,
    /// The resume guard's `--version`, asked ahead (`with_guard_ahead`).
    guard_ahead: Option<VersionAhead>,
}

pub use crate::version::VersionAhead;

/// Holds Codex's writer lock for a thread; removes the lock file on drop
/// under the coordination lock, as Codex does.
struct WriterLock {
    dir: PathBuf,
    path: PathBuf,
    file: Option<File>,
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        match lock_coordination(&self.dir) {
            Ok(coordination) => {
                drop(self.file.take());
                if let Err(e) = std::fs::remove_file(&self.path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(path = %self.path.display(), error = %e, "codex: thread lock not removed");
                }
                drop(coordination);
            }
            Err(e) => {
                warn!(error = %e, "codex: coordination lock not taken; thread lock file left");
            }
        }
    }
}

fn try_lock_for(path: &Path, wait: Duration) -> Result<Option<File>, HarnessError> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| HarnessError::io(IoOp::Write, path, e))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                // This runs on a blocking thread (`pipeline::blocking`), so
                // sleeping here parks no runtime worker.
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::Error(e)) => return Err(HarnessError::io(IoOp::Lock, path, e)),
        }
    }
}

fn lock_coordination(dir: &Path) -> Result<File, HarnessError> {
    std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(IoOp::CreateDir, dir, e))?;
    let path = dir.join(COORDINATION_LOCK);
    try_lock_for(&path, COORDINATION_WAIT)?.ok_or_else(|| {
        HarnessError::io(
            IoOp::Lock,
            path,
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "coordination lock held for longer than expected",
            ),
        )
    })
}

/// A TOML quoted key for `root`, so `projects."<root>".trust_level` names
/// exactly this path.
fn toml_quoted_key(root: &AbsPath) -> Result<String, HarnessError> {
    let text = root.as_str();
    if text.chars().any(char::is_control) {
        return Err(HarnessError::Malformed {
            what: Input::WorkspacePath,
            path: PathBuf::from(text),
            reason: "holds a control character".to_owned(),
        });
    }
    let mut key = String::with_capacity(text.len() + 2);
    key.push('"');
    for c in text.chars() {
        match c {
            '"' => key.push_str("\\\""),
            '\\' => key.push_str("\\\\"),
            other => key.push(other),
        }
    }
    key.push('"');
    Ok(key)
}

/// The file that holds `rollout` among `all`. Files that share the id are
/// reduced to the longest when every other one is a byte prefix of it, so
/// a thread that went to another node and came back never forks silently;
/// otherwise the lineage of `session` is ambiguous. Only the ids a lineage
/// reaches are resolved, so a forked rollout of another thread blocks
/// nothing here.
fn resolve(
    session: &SessionId,
    rollout: &SessionId,
    all: &[RolloutFile],
) -> Result<Option<RolloutFile>, HarnessError> {
    let mut longest: Option<&RolloutFile> = None;
    for file in all.iter().filter(|f| f.name.rollout_id == *rollout) {
        let Some(kept) = longest else {
            longest = Some(file);
            continue;
        };
        longest = Some(match place::compare(&kept.path, &file.path)? {
            Relation::Missing | Relation::Equal => kept,
            Relation::ExistingIsPrefix => file,
            Relation::Diverged => match place::compare(&file.path, &kept.path)? {
                Relation::ExistingIsPrefix => kept,
                Relation::Missing | Relation::Equal | Relation::Diverged => {
                    return Err(HarnessError::LineageAmbiguous {
                        session: session.clone(),
                        rollout: rollout.clone(),
                        paths: vec![kept.path.clone(), file.path.clone()],
                    });
                }
            },
        });
    }
    Ok(longest.cloned())
}

/// The rollout lineage of `session` among `all`, newest suffix first: the
/// newest rollout of the thread, then each prefix its `history_base` names.
fn lineage_in(
    all: &[RolloutFile],
    session: &SessionId,
    searched: &Path,
) -> Result<Vec<RolloutFile>, HarnessError> {
    let (chain, missing) = walk_lineage(all, session)?;
    if let Some(missing) = missing {
        return Err(HarnessError::LineageBroken {
            session: session.clone(),
            missing,
        });
    }
    if chain.is_empty() {
        return Err(HarnessError::SessionNotFound {
            kind: HarnessKind::Codex,
            session: session.clone(),
            searched: searched.to_path_buf(),
        });
    }
    Ok(chain)
}

/// As much of the lineage of `session` among `all` as is there, newest
/// suffix first, and the first prefix it names that is not; an empty chain
/// when `all` holds no rollout of the thread.
fn walk_lineage(
    all: &[RolloutFile],
    session: &SessionId,
) -> Result<(Vec<RolloutFile>, Option<SessionId>), HarnessError> {
    let Some(head_id) = all
        .iter()
        .find(|f| f.name.thread_id == *session)
        .map(|f| f.name.rollout_id.clone())
    else {
        return Ok((Vec::new(), None));
    };
    let Some(head) = resolve(session, &head_id, all)? else {
        return Ok((Vec::new(), None));
    };
    let mut chain = vec![head];
    while let Some(last) = chain.last() {
        let Some(prefix_id) = rollout::read_meta(last)?.history_base else {
            break;
        };
        if chain.iter().any(|f| f.name.rollout_id == prefix_id) {
            break;
        }
        let Some(prefix) = resolve(session, &prefix_id, all)? else {
            return Ok((chain, Some(prefix_id)));
        };
        chain.push(prefix);
    }
    Ok((chain, None))
}

impl Codex {
    /// An adapter whose state lives under `home/.codex`, finding `codex`
    /// on the `PATH` value `path_var`.
    #[must_use]
    pub fn new(home: AbsPath, path_var: OsString) -> Self {
        Codex {
            store: pipeline::fs_path(&home).join(".codex"),
            home,
            binary: None,
            path_var,
            overrides: None,
            known_writer: None,
            installed_ahead: None,
            guard_ahead: None,
        }
    }

    /// Starts asking the installed `codex` its version, from this
    /// process's directory, as a restore asks it before it places a
    /// thread; hand the answer to the adapter that restores with
    /// `with_installed_ahead`.
    #[must_use]
    pub fn ask_installed_ahead(&self) -> VersionAhead {
        VersionAhead::installed(self.binary().ok(), self.path_var.clone(), PROGRAM)
    }

    /// Takes the installed version from `ahead` when it asked what this
    /// adapter would ask, and asks again otherwise.
    #[must_use]
    pub fn with_installed_ahead(mut self, ahead: VersionAhead) -> Self {
        self.installed_ahead = Some(ahead);
        self
    }

    /// Starts the resume guard's `--version` of the `codex` a launch in
    /// `workspace` with `env` would start, run there with that environment,
    /// as the guard runs it; hand it to the launching adapter with
    /// `with_guard_ahead`.
    pub fn ask_guard_ahead(
        &self,
        workspace: &Path,
        env: &[(EnvVarName, String)],
    ) -> Result<VersionAhead, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        Ok(VersionAhead::reported(
            self.binary()?,
            pipeline::fs_path(&root).to_path_buf(),
            env.to_vec(),
        ))
    }

    /// Takes the resume guard's `--version` from `ahead` when it asked the
    /// binary, directory and environment the launch uses, and asks again
    /// otherwise.
    #[must_use]
    pub fn with_guard_ahead(mut self, ahead: VersionAhead) -> Self {
        self.guard_ahead = Some(ahead);
        self
    }

    /// Reads Codex's state from `dir` instead of `home/.codex`; refused
    /// when `dir` is not under the home.
    pub fn with_codex_home(mut self, dir: &AbsPath) -> Result<Self, HarnessError> {
        let dir = pipeline::fs_path(dir).to_path_buf();
        if dir.strip_prefix(self.home_dir()).is_err() {
            return Err(HarnessError::NotUnderHome {
                path: dir,
                home: self.home_dir().to_path_buf(),
            });
        }
        self.store = dir;
        Ok(self)
    }

    fn home_dir(&self) -> &Path {
        pipeline::fs_path(&self.home)
    }

    /// Refuses a launch environment under which Codex would keep its
    /// sessions and login in another directory than the store: a
    /// non-empty `CODEX_HOME` naming another directory, or none while the
    /// store is not `<home>/.codex`. `launch` runs it first.
    pub fn check_env(&self, env: &[(EnvVarName, String)]) -> Result<(), HarnessError> {
        let dir = env
            .iter()
            .find(|(name, value)| name.as_str() == HOME_VAR && !value.is_empty())
            .map_or_else(
                || self.home_dir().join(".codex"),
                |(_, value)| PathBuf::from(value),
            );
        let same = match (
            std::fs::canonicalize(&dir),
            std::fs::canonicalize(&self.store),
        ) {
            (Ok(dir), Ok(store)) => dir == store,
            _ => dir == self.store,
        };
        if same {
            Ok(())
        } else {
            Err(HarnessError::CodexHomeMismatch {
                dir,
                store: self.store.clone(),
            })
        }
    }

    /// Uses this binary instead of looking `codex` up on `PATH`. The
    /// path is absolute: the version probe runs in this process's
    /// directory and the launch in the workspace, so a relative path would
    /// name one file to the probe and another to the launch.
    ///
    /// ```
    /// # use fabric_core::path::AbsPath;
    /// # use fabric_harness::codex::Codex;
    /// let home = AbsPath::parse("/h").unwrap();
    /// let binary = AbsPath::parse("/opt/codex/bin/codex").unwrap();
    /// let _ = Codex::new(home, Default::default()).with_binary(&binary);
    /// ```
    ///
    /// A relative path does not type-check:
    ///
    /// ```compile_fail,E0308
    /// # use fabric_core::path::AbsPath;
    /// # use fabric_harness::codex::Codex;
    /// let home = AbsPath::parse("/h").unwrap();
    /// let binary = std::path::PathBuf::from("./bin/codex");
    /// let _ = Codex::new(home, Default::default()).with_binary(&binary);
    /// ```
    #[must_use]
    pub fn with_binary(mut self, binary: &AbsPath) -> Self {
        self.binary = Some(pipeline::fs_path(binary).to_path_buf());
        self
    }

    /// Settings passed as `-c` overrides on every run; a cold resume does
    /// not restore them (H2, experiment E3). On a resume each field set
    /// here wins over the one the session records.
    #[must_use]
    pub fn with_overrides(mut self, overrides: CodexOverrides) -> Self {
        self.overrides = Some(overrides);
        self
    }

    /// The newest Codex known to have written the session this adapter
    /// captures or resumes, beyond what its rollouts record: the version
    /// the job's last capture recorded, which travels with the job
    /// (`StateManifest::harness_version`), or one a resume on this node
    /// started with since (`Running::version`). A Codex that resumes a
    /// thread appends to its rollout and writes no `session_meta`, so a
    /// rollout names only the Codex that created it. The resume guard
    /// refuses a Codex older than this, and a capture records at least this
    /// version, so a node that never ran the session cannot lower it.
    #[must_use]
    pub fn with_known_writer(mut self, version: Option<HarnessVersion>) -> Self {
        self.known_writer = version;
        self
    }

    /// The version a capture records: the installed one, as a launch in the
    /// workspace would run it, or the known writer when that is newer or
    /// the installed one cannot be read.
    fn captured_version(&self, installed: Option<String>) -> Option<String> {
        let Some(known) = &self.known_writer else {
            return installed;
        };
        match installed.as_deref().map(HarnessVersion::parse) {
            Some(Ok(local)) if local >= *known => installed,
            _ => Some(known.as_str().to_owned()),
        }
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        if let Some(binary) = &self.binary {
            return Ok(binary.clone());
        }
        process::find_on_path(PROGRAM, &self.path_var).ok_or_else(|| HarnessError::NotInstalled {
            program: PROGRAM.to_owned(),
        })
    }

    fn codex_home_parts(&self) -> Result<Vec<String>, HarnessError> {
        let rel =
            self.store
                .strip_prefix(self.home_dir())
                .map_err(|_| HarnessError::NotUnderHome {
                    path: self.store.clone(),
                    home: self.home_dir().to_path_buf(),
                })?;
        pipeline::rel_components(rel)
    }

    /// Every rollout file in the store, duplicates included.
    fn all_rollouts(&self) -> Result<Vec<RolloutFile>, HarnessError> {
        Ok(self.scan_store()?.0)
    }

    /// Every rollout file in the store, duplicates included, and the
    /// symlinks the walk met and did not follow.
    fn scan_store(&self) -> Result<(Vec<RolloutFile>, Vec<PathBuf>), HarnessError> {
        let (mut all, mut links) = rollout::scan(&self.store.join("sessions"))?;
        let (archived, archived_links) = rollout::scan(&self.store.join("archived_sessions"))?;
        all.extend(archived);
        links.extend(archived_links);
        Ok((all, links))
    }

    /// Where this node's `lsof` is, or `None` when it has none. Capture
    /// and restore ask it whether a Codex process holds a thread's rollout
    /// open, so without it every capture of a thread fails.
    #[must_use]
    pub fn lsof() -> Option<&'static Path> {
        process::system_tool("lsof", process::LSOF).ok()
    }

    /// Every thread whose rollout records the workspace as its `cwd`, each
    /// once, oldest first by the time in its rollout's name and then by
    /// rollout id, which Codex mints in time order.
    ///
    /// A Codex launched without a thread to resume makes its thread when it
    /// starts, before anything it runs can make another: a `codex exec` the
    /// agent runs, a sub-agent or a review thread. So of the threads that
    /// appeared after such a launch was planned, the oldest is the
    /// launch's own.
    pub fn threads_in(&self, workspace: &Path) -> Result<Vec<SessionId>, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let mut threads: Vec<SessionId> = Vec::new();
        for file in rollout::list(&self.store.join("sessions"))?
            .into_iter()
            .rev()
        {
            let meta = match rollout::read_meta(&file) {
                Ok(meta) => meta,
                Err(e) => {
                    warn!(error = %e, "codex: rollout skipped");
                    continue;
                }
            };
            if meta.cwd.as_deref() == Some(root.as_str()) && !threads.contains(&file.name.thread_id)
            {
                threads.push(file.name.thread_id);
            }
        }
        Ok(threads)
    }

    /// The newest thread whose rollout records the workspace as its `cwd`.
    pub fn latest_session(&self, workspace: &Path) -> Result<Option<SessionId>, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        for file in rollout::list(&self.store.join("sessions"))? {
            let meta = match rollout::read_meta(&file) {
                Ok(meta) => meta,
                Err(e) => {
                    warn!(error = %e, "codex: rollout skipped");
                    continue;
                }
            };
            if meta.cwd.as_deref() == Some(root.as_str()) {
                return Ok(Some(file.name.thread_id));
            }
        }
        Ok(None)
    }

    /// The rollout lineage of `session` in this store, newest suffix
    /// first. A rollout of the thread reached through a symlink under
    /// `sessions/` or `archived_sessions/` is refused (`SymlinkInState`):
    /// Codex opens it through the link, from the path its database holds,
    /// so capture and the resume guard, which both read the lineage here,
    /// never skip it and find no rollout, or an older one.
    pub(crate) fn lineage(&self, session: &SessionId) -> Result<Vec<RolloutFile>, HarnessError> {
        let (all, links) = self.scan_store()?;
        if let Some(path) = rollout::link_reaching(&links, session)? {
            return Err(HarnessError::SymlinkInState { path });
        }
        lineage_in(&all, session, &self.store.join("sessions"))
    }

    /// Takes the thread writer lock as Codex's `acquire` does: the
    /// coordination lock only around opening and trying the thread lock.
    fn hold_writer_lock(&self, session: &SessionId) -> Result<WriterLock, HarnessError> {
        let dir = self.store.join(LOCK_DIR);
        let coordination = lock_coordination(&dir)?;
        let path = dir.join(format!("{session}.lock"));
        let thread = try_lock_for(&path, Duration::ZERO)?;
        drop(coordination);
        let Some(file) = thread else {
            return Err(HarnessError::ActiveWriter {
                kind: HarnessKind::Codex,
                session: session.clone(),
            });
        };
        Ok(WriterLock {
            dir,
            path,
            file: Some(file),
        })
    }

    /// Refuses while a `codex` runs that may not take the thread writer
    /// lock: one older than `LOCK_READ_ON`, or one whose version is unknown.
    async fn refuse_lockless_writers(&self, version: Option<&str>) -> Result<(), HarnessError> {
        if version.is_some_and(|v| crate::version::at_least(v, LOCK_READ_ON)) {
            return Ok(());
        }
        warn!(
            ?version,
            "codex: the writer lock may not be taken by this version; any running codex blocks"
        );
        let binary = self.binary().ok();
        let pids = process::running_pids(PROGRAM, binary.as_deref()).await?;
        if pids.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            })
        }
    }

    /// Refuses while any process holds one of `files` open: a Codex that
    /// takes no writer lock, whatever the binary on disk says, still holds
    /// the rollout of a thread it has loaded.
    fn refuse_open(files: &[PathBuf]) -> Result<(), HarnessError> {
        let pids = process::holding(files)?;
        if pids.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            })
        }
    }

    fn capture_sync(
        &self,
        root: &AbsPath,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let home_parts = self.codex_home_parts()?;
        // Listing rollouts and taking the lock start at the store and would
        // follow it, or a directory above it, if it were a link.
        pipeline::refuse_symlinks(self.home_dir(), &self.store)?;
        let lock = self.hold_writer_lock(session)?;
        let chain = self.lineage(session)?;
        let files: Vec<PathBuf> = chain.iter().map(|file| file.path.clone()).collect();
        Self::refuse_open(&files)?;
        let mut sources = Vec::with_capacity(chain.len());
        for file in &chain {
            rollout::refuse_secrets(file)?;
            sources.push(self.source_of(file, &home_parts)?);
        }
        // A rollout only grows, so no departure record is kept.
        let capturer = Capturer {
            kind: HarnessKind::Codex,
            session,
            root,
            home: &self.home,
            version,
            departure: None,
        };
        let manifest = pipeline::capture(capturer, &sources, RedactionCounts::new(), out)?;
        drop(lock);
        info!(%session, rollouts = chain.len(), "codex: captured");
        Ok(manifest)
    }

    /// The capture source of one rollout file under this store.
    fn source_of(&self, file: &RolloutFile, home_parts: &[String]) -> Result<Source, HarnessError> {
        let rel = file
            .path
            .strip_prefix(&self.store)
            .map_err(|_| HarnessError::NotUnderHome {
                path: file.path.clone(),
                home: self.store.clone(),
            })?;
        let parts = pipeline::rel_components(rel)?;
        let mut all: Vec<&str> = vec![HOME_PREFIX];
        all.extend(home_parts.iter().map(String::as_str));
        all.extend(parts.iter().map(String::as_str));
        Ok(Source {
            abs: file.path.clone(),
            rel: pipeline::rel_path(&all)?,
            carry: Carry::Opaque,
            mutability: Mutability::AppendOnly,
        })
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        // A running `codex` may have started in any directory, so it is
        // judged by the Codex the daemon's directory picks, as restore
        // judges it. A workspace pin this node lacks runs no Codex there,
        // and taking its shim's failure for a writer of unknown version
        // would make any `codex` on the node, such as a desktop app's,
        // block every capture of the job. The version recorded is asked in
        // the workspace, where a version manager's shim picks the Codex a
        // launch there runs, not the one it picks for the daemon. The two
        // questions are asked at once (issue #26).
        let (anywhere, installed) = tokio::join!(
            process::installed_version(self.binary().ok(), &self.path_var, PROGRAM),
            process::installed_version_in(
                self.binary().ok(),
                Some(pipeline::fs_path(&root)),
                &self.path_var,
                PROGRAM,
            ),
        );
        self.refuse_lockless_writers(anywhere.as_deref()).await?;
        let version = self.captured_version(installed);
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        pipeline::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
    }

    /// The rollout file name a tree path carries, when the path lies under
    /// this store's `sessions/` or `archived_sessions/`, the only places
    /// capture takes a rollout from.
    fn restorable<'a>(&self, rel: &'a str) -> Result<Option<(&'a str, RolloutName)>, HarnessError> {
        let store = self.codex_home_parts()?;
        let mut parts = rel.split('/');
        if parts.next() != Some(HOME_PREFIX) {
            return Ok(None);
        }
        for expected in &store {
            if parts.next() != Some(expected.as_str()) {
                return Ok(None);
            }
        }
        if !matches!(parts.next(), Some("sessions" | "archived_sessions")) {
            return Ok(None);
        }
        let Some(name) = parts.next_back() else {
            return Ok(None);
        };
        Ok(rollout::parse_name(name).map(|parsed| (name, parsed)))
    }

    /// Where one rollout of `session` lands: over the file that already
    /// holds that rollout on this node, else under `fallback_dir`.
    fn destination(
        session: &SessionId,
        staged: &RolloutFile,
        name_text: &str,
        existing: &[RolloutFile],
        fallback_dir: &Path,
    ) -> Result<Destination, HarnessError> {
        let name = &staged.name;
        let path = match resolve(session, &name.rollout_id, existing)? {
            None => fallback_dir.join(name_text),
            Some(found) if found.name.compressed == name.compressed => found.path,
            Some(found) => {
                return Err(HarnessError::LineageAmbiguous {
                    session: session.clone(),
                    rollout: name.rollout_id.clone(),
                    paths: vec![found.path, staged.path.clone()],
                });
            }
        };
        Ok(Destination {
            path,
            content: Content::Opaque,
            mutability: Mutability::AppendOnly,
        })
    }

    fn restore_sync(
        &self,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let session = manifest.session().clone();
        // Only the rollouts on the thread's own lineage, derived from the
        // staged files, may be placed; any other rollout in the manifest
        // is refused like any other unexpected file.
        let mut staged = Vec::new();
        for file in manifest.files() {
            let rel = file.rel_path.as_str();
            if let Some((_, name)) = self.restorable(rel)? {
                staged.push(RolloutFile {
                    path: state.join(rel),
                    name,
                });
            }
        }
        let chain = lineage_in(&staged, &session, state)?;
        pipeline::refuse_symlinks(self.home_dir(), &self.store)?;
        let lock = self.hold_writer_lock(&session)?;
        let existing = self.all_rollouts()?;
        let lineage: Vec<PathBuf> = existing
            .iter()
            .filter(|file| {
                chain
                    .iter()
                    .any(|staged| staged.name.rollout_id == file.name.rollout_id)
            })
            .map(|file| file.path.clone())
            .collect();
        Self::refuse_open(&lineage)?;
        let fallback_dir = self
            .store
            .join("sessions")
            .join(RESTORE_SUBDIR)
            .join(session.as_str());
        // The thread's rollouts here, as far as its lineage reaches: one
        // the incoming lineage does not place was written after the job left.
        let home_parts = self.codex_home_parts()?;
        let held = walk_lineage(&existing, &session)?
            .0
            .iter()
            .map(|file| self.source_of(file, &home_parts))
            .collect::<Result<Vec<Source>, HarnessError>>()?;
        // A rollout only grows, so nothing is ever replaced and no
        // departure record is kept.
        let restored = pipeline::restore(manifest, state, None, &held, |rel, _| {
            let Some((name_text, _)) = self.restorable(rel)? else {
                return Ok(None);
            };
            let path = state.join(rel);
            let Some(on_lineage) = chain.iter().find(|f| f.path == path) else {
                return Ok(None);
            };
            Self::destination(&session, on_lineage, name_text, &existing, &fallback_dir).map(Some)
        })?;
        drop(lock);
        info!(%session, files = restored.placed.len(), "codex: restored");
        Ok(restored)
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &HarnessKind::Codex, &root, &self.home)?;
        let version = crate::version::installed_version(
            self.installed_ahead.as_ref(),
            self.binary().ok(),
            &self.path_var,
            PROGRAM,
        )
        .await;
        self.refuse_lockless_writers(version.as_deref()).await?;
        let adapter = self.clone();
        let state = state.to_path_buf();
        pipeline::blocking(move || adapter.restore_sync(&state, manifest)).await
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        process::detect(self.binary().ok(), &self.path_var).await
    }

    /// What the lineage of `session` records, read before a resume, and the
    /// version of the binary that will run it in `root`, when there was a
    /// writer to compare it with; refused when that binary is older than
    /// the newest writer, the highest `cli_version` of the lineage or the
    /// known writer, or does not say its version, so nothing starts.
    async fn read_session(
        &self,
        root: &AbsPath,
        binary: &Path,
        env: &[(EnvVarName, String)],
        session: &SessionId,
    ) -> Result<(RecordedSettings, Option<HarnessVersion>), HarnessError> {
        let adapter = self.clone();
        let id = session.clone();
        // A thread with no rollout here records nothing to carry or to
        // guard; Codex then says itself that it found none. One that only
        // a symlink reaches is refused by `lineage`, not found missing.
        let read = pipeline::blocking(move || match adapter.lineage(&id) {
            Ok(lineage) => rollout::recorded(&lineage).map(Some),
            Err(HarnessError::SessionNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        })
        .await?;
        let Some(recorded) = read else {
            return Ok((RecordedSettings::default(), None));
        };
        let newest = recorded.writer.iter().chain(&self.known_writer).max();
        let runs = match newest {
            Some(writer) => {
                let cwd = pipeline::fs_path(root);
                Some(
                    crate::version::refuse_older(
                        PROGRAM,
                        (binary, cwd, env),
                        self.guard_ahead.as_ref(),
                        session,
                        writer,
                    )
                    .await?,
                )
            }
            None => None,
        };
        Ok((recorded, runs))
    }

    /// The argument list for `launch` with `overrides`, without the binary.
    pub(crate) fn args_for(
        root: &AbsPath,
        launch: &Launch,
        overrides: &LaunchOverrides,
    ) -> Result<Vec<String>, HarnessError> {
        let mut args = Vec::new();
        if launch.is_headless() {
            args.push("exec".to_owned());
        }
        if let Some(session) = launch.resume() {
            args.push("resume".to_owned());
            args.push(session.as_str().to_owned());
        }
        if launch.is_headless() {
            args.push("--json".to_owned());
        }
        args.push("-c".to_owned());
        args.push(format!(
            "projects.{}.trust_level=trusted",
            toml_quoted_key(root)?
        ));
        for pair in overrides.pairs() {
            args.push("-c".to_owned());
            args.push(pair);
        }
        if let Some(prompt) = launch.prompt() {
            args.push("--".to_owned());
            args.push(prompt.to_owned());
        }
        Ok(args)
    }

    /// Reads one line of what `codex exec --json` printed into `report`.
    /// The events, from `codex-rs/exec/src/exec_events.rs`, unchanged from
    /// 0.142.0 to 0.163.0: `thread.started` names the thread, the same id
    /// for `exec resume`; each `item.completed` whose item is an
    /// `agent_message` carries an answer, the last one being the turn's;
    /// `turn.failed` carries the failure. An `error` event can be a retry
    /// and ends nothing, so it is left alone, as is any other line.
    /// Returns true when the line named a thread other than the one
    /// `report` held: a new thread, or a resume Codex answered with a new
    /// thread because it found no match.
    pub fn read_headless_line(report: &mut TurnReport, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            return false;
        };
        match event["type"].as_str() {
            Some("thread.started") => return report.note_session(event["thread_id"].as_str()),
            Some("item.completed") if event["item"]["type"] == "agent_message" => {
                if let Some(text) = event["item"]["text"].as_str() {
                    report.answer = Some(text.to_owned());
                }
            }
            Some("turn.failed") => {
                report.failure = Some(
                    event["error"]["message"]
                        .as_str()
                        .unwrap_or("the turn failed")
                        .to_owned(),
                );
            }
            _ => {}
        }
        false
    }

    async fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        self.check_env(env)?;
        let root = pipeline::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let (recorded, runs) = match launch.resume() {
            Some(session) => {
                let (recorded, runs) = self.read_session(&root, &binary, env, session).await?;
                (Some(recorded), runs)
            }
            None => (None, None),
        };
        let (overrides, settings) = overrides::effective(
            self.overrides.as_ref(),
            recorded.as_ref(),
            launch.is_headless(),
        );
        let args = Self::args_for(&root, launch, &overrides)?;
        let session = launch.resume().cloned();
        let spawned =
            process::spawn(&binary, &args, pipeline::fs_path(&root), env, launch.into()).await?;
        info!(?session, pid = %spawned.pid, "codex: launched, held until released");
        Ok(Running::new(session, spawned)
            .with_settings(settings)
            .with_version(runs))
    }
}

impl Harness for Codex {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
    }

    fn detect(&self) -> BoxFuture<'_, Result<Option<Installed>, HarnessError>> {
        Box::pin(self.detect_impl())
    }

    fn capture<'a>(
        &'a self,
        workspace: &'a Path,
        session: &'a SessionId,
        out: &'a Path,
    ) -> BoxFuture<'a, Result<StateManifest, HarnessError>> {
        Box::pin(self.capture_impl(workspace, session, out))
    }

    /// A rollout only grows, so Codex keeps no departure record and this
    /// only checks that the manifest is a Codex one.
    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        Box::pin(std::future::ready(pipeline::confirm_departure(
            manifest,
            &HarnessKind::Codex,
            None,
        )))
    }

    fn restore<'a>(
        &'a self,
        workspace: &'a Path,
        state: &'a Path,
    ) -> BoxFuture<'a, Result<Restored, HarnessError>> {
        Box::pin(self.restore_impl(workspace, state))
    }

    fn launch<'a>(
        &'a self,
        workspace: &'a Path,
        launch: Launch,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(async move { self.launch_impl(workspace, &launch, env).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_follow_the_cli_surface_and_quote_the_root() {
        let set = CodexOverrides {
            sandbox: Some(overrides::SandboxMode::WorkspaceWrite),
            approval: Some(overrides::ApprovalPolicy::OnRequest),
            reviewer: Some(overrides::ApprovalsReviewer::User),
            model: Some(crate::harness::ModelName::parse("o3").unwrap()),
            effort: Some(overrides::Effort::High),
        };
        let set = LaunchOverrides {
            settings: set,
            provider: None,
        };
        let root = &AbsPath::parse("/h/fabric/jobs/j/work").unwrap();
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let resume = Launch::Interactive {
            resume: Some(session.clone()),
            prompt: None,
        };
        assert_eq!(
            Codex::args_for(root, &resume, &set).unwrap(),
            vec![
                "resume",
                "019f233e-bb33-7342-85fa-f2d33b14368a",
                "-c",
                "projects.\"/h/fabric/jobs/j/work\".trust_level=trusted",
                "-c",
                "model=\"o3\"",
                "-c",
                "model_reasoning_effort=\"high\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "-c",
                "approval_policy=\"on-request\"",
                "-c",
                "approvals_reviewer=\"user\""
            ]
        );
        let headless = Launch::Headless {
            resume: Some(session),
            prompt: "--dangerously-bypass-approvals-and-sandbox".to_owned(),
        };
        assert_eq!(
            Codex::args_for(root, &headless, &set).unwrap(),
            vec![
                "exec",
                "resume",
                "019f233e-bb33-7342-85fa-f2d33b14368a",
                "--json",
                "-c",
                "projects.\"/h/fabric/jobs/j/work\".trust_level=trusted",
                "-c",
                "model=\"o3\"",
                "-c",
                "model_reasoning_effort=\"high\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "-c",
                "approval_policy=\"on-request\"",
                "-c",
                "approvals_reviewer=\"user\"",
                "--",
                "--dangerously-bypass-approvals-and-sandbox"
            ]
        );
        let quoted = &AbsPath::parse("/h/say \"hi\"\\there").unwrap();
        let args = Codex::args_for(
            quoted,
            &Launch::Interactive {
                resume: None,
                prompt: Some("go".to_owned()),
            },
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(
            args[1],
            "projects.\"/h/say \\\"hi\\\"\\\\there\".trust_level=trusted"
        );
        assert!(matches!(
            toml_quoted_key(&AbsPath::parse("/h/new\nline").unwrap()),
            Err(HarnessError::Malformed { .. })
        ));
    }

    #[test]
    fn headless_events_give_the_thread_and_the_last_answer() {
        let thread = "019f233e-bb33-7342-85fa-f2d33b14368a";
        let mut report = TurnReport::default();
        let lines = [
            format!("{{\"type\":\"thread.started\",\"thread_id\":\"{thread}\"}}"),
            "{\"type\":\"turn.started\"}".to_owned(),
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"item_0\",\"type\":\"agent_message\",\"text\":\"first\"}}".to_owned(),
            "{\"type\":\"error\",\"message\":\"Reconnecting... 1/5\"}".to_owned(),
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"item_1\",\"type\":\"reasoning\",\"text\":\"thinking\"}}".to_owned(),
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"item_2\",\"type\":\"agent_message\",\"text\":\"last\"}}".to_owned(),
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"cached_input_tokens\":0,\"output_tokens\":1}}".to_owned(),
        ];
        let named: Vec<bool> = lines
            .iter()
            .map(|line| Codex::read_headless_line(&mut report, line))
            .collect();
        assert_eq!(named, [true, false, false, false, false, false, false]);
        assert_eq!(
            report,
            TurnReport {
                session: Some(SessionId::parse(thread).unwrap()),
                answer: Some("last".to_owned()),
                failure: None,
            }
        );
        Codex::read_headless_line(
            &mut report,
            "{\"type\":\"turn.failed\",\"error\":{\"message\":\"usage limit\"}}",
        );
        assert_eq!(report.failure.as_deref(), Some("usage limit"));
    }

    /// A capture whose own version check cannot read the installed Codex,
    /// such as a version manager's shim with no version set outside the
    /// workspace, or a `--version` line that is not a version, records the
    /// known writer, so the next node's guard keeps the floor.
    #[test]
    fn a_capture_that_cannot_read_the_installed_codex_records_the_known_writer() {
        let adapter = Codex::new(AbsPath::parse("/h").unwrap(), OsString::new())
            .with_known_writer(Some(HarnessVersion::parse("0.160.0").unwrap()));
        for installed in [None, Some("0.155")] {
            assert_eq!(
                adapter
                    .captured_version(installed.map(str::to_owned))
                    .as_deref(),
                Some("0.160.0"),
                "{installed:?}"
            );
        }
    }

    const THREAD: &str = "019f233e-bb33-7342-85fa-f2d33b14368a";

    /// A home whose store holds one rollout of `THREAD`, written by Codex
    /// 0.155.1 with gpt-6-sol, and a fake `codex` that answers
    /// `--version` by running the shell text `version` and otherwise
    /// appends a record to the rollout, as 0.142.4 did, and writes its
    /// arguments to `ran`.
    struct Resume {
        _dir: tempfile::TempDir,
        home: PathBuf,
        rollout: PathBuf,
        adapter: Codex,
        env: Vec<(EnvVarName, String)>,
    }

    impl Resume {
        /// A fake `codex` that answers `--version` with `version_line`.
        fn new(version_line: &str) -> Self {
            Self::answering(&format!("echo '{version_line}'"))
        }

        fn answering(version: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let home = std::fs::canonicalize(dir.path()).unwrap();
            std::fs::create_dir_all(home.join("work")).unwrap();
            let sessions = home.join(".codex/sessions/2026/10/21");
            std::fs::create_dir_all(&sessions).unwrap();
            let rollout = sessions.join(format!("rollout-2026-10-21T10-00-00-{THREAD}.jsonl"));
            std::fs::write(
                &rollout,
                format!(
                    "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{THREAD}\",\"cwd\":\"/w\",\"cli_version\":\"0.155.1\"}}}}\n\
                     {{\"type\":\"turn_context\",\"payload\":{{\"model\":\"gpt-6-sol\",\"effort\":\"xhigh\",\"approval_policy\":\"on-request\",\"sandbox_policy\":{{\"type\":\"workspace-write\"}}}}}}\n"
                ),
            )
            .unwrap();
            let binary = home.join("codex");
            std::fs::write(
                &binary,
                format!(
                    "#!/bin/sh\n\
                     if [ \"$1\" = --version ]; then {version}; exit 0; fi\n\
                     echo '{{\"type\":\"event_msg\",\"payload\":{{}}}}' >> '{}'\n\
                     printf '%s\\n' \"$@\" > \"$HOME/ran\"\n",
                    rollout.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let adapter = Codex::new(
                AbsPath::parse(home.to_str().unwrap()).unwrap(),
                OsString::new(),
            )
            .with_binary(&AbsPath::parse(binary.to_str().unwrap()).unwrap());
            let env = vec![(
                EnvVarName::parse("HOME").unwrap(),
                home.to_str().unwrap().to_owned(),
            )];
            Resume {
                _dir: dir,
                home,
                rollout,
                adapter,
                env,
            }
        }

        async fn launch(&self, resume: bool) -> Result<Running, HarnessError> {
            let launch = Launch::Headless {
                resume: resume.then(|| SessionId::parse(THREAD).unwrap()),
                prompt: "next".to_owned(),
            };
            self.adapter
                .launch(&self.home.join("work"), launch, &self.env)
                .await
        }

        fn ran(&self) -> Option<Vec<String>> {
            let text = std::fs::read_to_string(self.home.join("ran")).ok()?;
            Some(text.lines().map(str::to_owned).collect())
        }
    }

    /// An older codex on this node is refused before it starts, so the
    /// rollout a newer one wrote keeps its bytes (gate finding F10b).
    #[tokio::test]
    async fn an_older_codex_is_refused_before_it_starts_and_the_rollout_is_untouched() {
        let fixture = Resume::new("codex-cli 0.142.4");
        let before = std::fs::read(&fixture.rollout).unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        let HarnessError::HarnessOlderThanSession {
            local, recorded, ..
        } = &refused
        else {
            panic!("expected HarnessOlderThanSession, got {refused}");
        };
        assert_eq!((local.as_str(), recorded.as_str()), ("0.142.4", "0.155.1"));
        assert!(
            refused
                .to_string()
                .contains("an older harness cannot run this session"),
            "{refused}"
        );
        assert_eq!(std::fs::read(&fixture.rollout).unwrap(), before);
        assert_eq!(fixture.ran(), None, "the older codex ran");
    }

    /// The version is asked in the workspace the launch starts codex in:
    /// a version manager's shim picks the tool version by directory, so a
    /// probe from anywhere else asks another binary than the one that runs.
    #[tokio::test]
    async fn the_version_is_asked_in_the_workspace_codex_starts_in() {
        let fixture = Resume::answering(
            "if [ -f .tool-versions ]; then echo 'codex-cli 0.142.4'; else echo 'codex-cli 0.155.1'; fi",
        );
        std::fs::write(fixture.home.join("work/.tool-versions"), "codex 0.142.4\n").unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(
                &refused,
                HarnessError::HarnessOlderThanSession { local, .. } if local.as_str() == "0.142.4"
            ),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None, "the pinned older codex ran");
    }

    /// The version is asked with the environment the launch starts codex
    /// with: a version manager's shim also picks the tool version from it
    /// (`ASDF_CODEX_VERSION`), so a probe without it asks another binary
    /// than the one that runs.
    #[tokio::test]
    async fn the_version_is_asked_with_the_environment_codex_starts_with() {
        let mut fixture = Resume::answering(
            "if [ -n \"$ASDF_CODEX_VERSION\" ]; then echo \"codex-cli $ASDF_CODEX_VERSION\"; else echo 'codex-cli 0.155.1'; fi",
        );
        fixture.env.push((
            EnvVarName::parse("ASDF_CODEX_VERSION").unwrap(),
            "0.142.4".to_owned(),
        ));
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(
                &refused,
                HarnessError::HarnessOlderThanSession { local, .. } if local.as_str() == "0.142.4"
            ),
            "{refused}"
        );
        assert_eq!(
            fixture.ran(),
            None,
            "the older codex the environment picks ran"
        );
    }

    /// A resume reports the version the guard asked in the workspace, the
    /// Codex that now runs the thread, so the node can remember it: that
    /// Codex writes no version into the rollout, and a version check run
    /// anywhere else may find another Codex than a shim picks there. A new
    /// thread asks nothing and reports nothing.
    #[tokio::test]
    async fn a_resume_reports_the_version_it_started() {
        let fixture = Resume::answering(
            "if [ -f .tool-versions ]; then echo 'codex-cli 0.160.0'; else echo 'codex-cli 0.155.1'; fi",
        );
        std::fs::write(fixture.home.join("work/.tool-versions"), "codex 0.160.0\n").unwrap();
        let mut running = fixture.launch(true).await.unwrap();
        running.release().unwrap();
        running.child_mut().wait().await.unwrap();
        assert_eq!(
            running.version().map(HarnessVersion::as_str),
            Some("0.160.0")
        );
        let mut running = fixture.launch(false).await.unwrap();
        running.release().unwrap();
        running.child_mut().wait().await.unwrap();
        assert_eq!(running.version(), None);
    }

    /// A thread created by one Codex and continued by a newer one names
    /// only its creator in the rollout, because a resume writes no
    /// `session_meta`; the known writer the job carried refuses the older
    /// Codex anyway, before it starts.
    #[tokio::test]
    async fn an_older_codex_is_refused_a_thread_a_newer_one_continued() {
        let mut fixture = Resume::new("codex-cli 0.155.1");
        let newer = HarnessVersion::parse("0.160.0").unwrap();
        fixture.adapter = fixture.adapter.clone().with_known_writer(Some(newer));
        let before = std::fs::read(&fixture.rollout).unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(
                &refused,
                HarnessError::HarnessOlderThanSession { local, recorded, .. }
                    if local.as_str() == "0.155.1" && recorded.as_str() == "0.160.0"
            ),
            "{refused}"
        );
        assert_eq!(std::fs::read(&fixture.rollout).unwrap(), before);
        assert_eq!(fixture.ran(), None, "the older codex ran");
    }

    /// The floor is the higher of the two writers either way: a known
    /// writer older than the rollout's `cli_version`, such as one a daemon
    /// read from another Codex than a workspace pinned, does not lower it.
    #[tokio::test]
    async fn a_known_writer_older_than_the_rollout_does_not_lower_the_floor() {
        let mut fixture = Resume::new("codex-cli 0.158.0");
        let rollout = std::fs::read_to_string(&fixture.rollout).unwrap();
        let newer = rollout.replace("\"cli_version\":\"0.155.1\"", "\"cli_version\":\"0.160.0\"");
        assert_ne!(newer, rollout);
        std::fs::write(&fixture.rollout, &newer).unwrap();
        let older = HarnessVersion::parse("0.155.1").unwrap();
        fixture.adapter = fixture.adapter.clone().with_known_writer(Some(older));
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(
                &refused,
                HarnessError::HarnessOlderThanSession { local, recorded, .. }
                    if local.as_str() == "0.158.0" && recorded.as_str() == "0.160.0"
            ),
            "{refused}"
        );
        assert_eq!(std::fs::read_to_string(&fixture.rollout).unwrap(), newer);
        assert_eq!(fixture.ran(), None, "the older codex ran");
    }

    /// A rollout of the thread reached through a symlink under
    /// `sessions/`, a day directory or the file itself, as a folder synced
    /// from another Mac makes it, refuses the resume and the capture alike.
    /// Codex resumes it through the link, from the path its state database
    /// holds, while a walk that skips the link finds no rollout, so a
    /// guard that skipped it would let an older codex append to what a
    /// newer one wrote, with no version check and the known writer
    /// dropped too.
    #[tokio::test]
    async fn a_rollout_behind_a_symlink_refuses_the_resume_and_the_capture() {
        let mut fixture = Resume::new("codex-cli 0.142.4");
        let newer = HarnessVersion::parse("0.160.0").unwrap();
        fixture.adapter = fixture.adapter.clone().with_known_writer(Some(newer));
        let before = std::fs::read(&fixture.rollout).unwrap();
        let year = fixture.home.join(".codex/sessions/2026");
        let synced = fixture.home.join("synced-2026");
        std::fs::rename(&year, &synced).unwrap();
        std::os::unix::fs::symlink(&synced, &year).unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(&refused, HarnessError::SymlinkInState { path } if *path == year),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None, "the older codex ran");
        assert_eq!(std::fs::read(&fixture.rollout).unwrap(), before);

        // The rollout file itself a link, in a real day directory.
        std::fs::remove_file(&year).unwrap();
        std::fs::create_dir_all(fixture.rollout.parent().unwrap()).unwrap();
        let shared = synced
            .join("10/21")
            .join(fixture.rollout.file_name().unwrap());
        std::os::unix::fs::symlink(&shared, &fixture.rollout).unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(&refused, HarnessError::SymlinkInState { path } if *path == fixture.rollout),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None, "the older codex ran");
        assert_eq!(std::fs::read(&shared).unwrap(), before);

        // Capture refuses the same layout by the same rule. A codex at
        // the lock's floor, so no running codex on the machine blocks it.
        let capturing = Resume::new("codex-cli 0.155.1");
        let day = capturing.rollout.parent().unwrap().to_path_buf();
        let synced = capturing.home.join("synced-day");
        std::fs::rename(&day, &synced).unwrap();
        std::os::unix::fs::symlink(&synced, &day).unwrap();
        let out = capturing.home.join("out");
        let refused = capturing
            .adapter
            .capture(
                &capturing.home.join("work"),
                &SessionId::parse(THREAD).unwrap(),
                &out,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&refused, HarnessError::SymlinkInState { path } if *path == day),
            "{refused}"
        );
    }

    /// A thread that ran on another provider than this node's default
    /// resumes with that provider beside its model, so Codex does not send
    /// the model to a provider that does not serve it, and the report
    /// names both.
    #[tokio::test]
    async fn a_resume_passes_the_provider_the_session_s_model_ran_on() {
        let fixture = Resume::new("codex-cli 0.155.1");
        let rollout = std::fs::read_to_string(&fixture.rollout).unwrap();
        let on_azure = rollout.replace(
            "\"cli_version\":\"0.155.1\"",
            "\"cli_version\":\"0.155.1\",\"model_provider\":\"azure\"",
        );
        assert_ne!(on_azure, rollout);
        std::fs::write(&fixture.rollout, &on_azure).unwrap();
        let mut running = fixture.launch(true).await.unwrap();
        running.release().unwrap();
        running.child_mut().wait().await.unwrap();
        let ran = fixture.ran().unwrap();
        let at = ran.iter().position(|a| a == "model=\"gpt-6-sol\"").unwrap();
        assert_eq!(ran[at + 1..at + 3], ["-c", "model_provider=\"azure\""]);
        let passed: Vec<String> = running
            .settings()
            .passed
            .iter()
            .map(|p| format!("{} {}", p.name, p.value))
            .collect();
        assert_eq!(passed[..2], ["model gpt-6-sol", "model provider azure"]);
    }

    /// A codex whose version cannot be read is refused too: deny by default.
    #[tokio::test]
    async fn a_codex_whose_version_cannot_be_read_is_refused() {
        let fixture = Resume::new("codex-cli");
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(refused, HarnessError::HarnessVersionUnknown { .. }),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None);
    }

    /// A `--version` run that fails, such as a version manager's shim with
    /// no version set for the workspace, which prints to stderr and exits
    /// 126, refuses the resume: deny by default.
    #[tokio::test]
    async fn a_codex_whose_version_run_fails_is_refused() {
        let fixture = Resume::answering("echo 'No version is set for command codex' >&2; exit 126");
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(&refused, HarnessError::HarnessVersionUnknown { reason, .. } if reason.contains("126")),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None, "codex ran unguarded");
    }

    /// A thread whose recorded version or lineage cannot be read refuses
    /// the resume before codex starts, rather than running it unguarded
    /// and without its settings.
    #[tokio::test]
    async fn a_thread_whose_writer_cannot_be_read_is_refused() {
        let fixture = Resume::new("codex-cli 0.155.1");
        let rollout = std::fs::read_to_string(&fixture.rollout).unwrap();
        let unreadable =
            rollout.replace("\"cli_version\":\"0.155.1\"", "\"cli_version\":\"0.155\"");
        assert_ne!(unreadable, rollout);
        std::fs::write(&fixture.rollout, &unreadable).unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(
                &refused,
                HarnessError::Malformed {
                    what: Input::RolloutLine,
                    ..
                }
            ),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None, "codex ran unguarded");

        // A paginated suffix whose prefix is missing: the newest writer of
        // the lineage cannot be known.
        let broken = rollout.replace(
            "\"cli_version\":\"0.155.1\"",
            "\"cli_version\":\"0.155.1\",\"history_base\":{\"thread_id\":\"019f233e-bb33-7342-85fa-f2d33b14368b\"}",
        );
        assert_ne!(broken, rollout);
        std::fs::write(&fixture.rollout, &broken).unwrap();
        let refused = fixture.launch(true).await.unwrap_err();
        assert!(
            matches!(&refused, HarnessError::LineageBroken { .. }),
            "{refused}"
        );
        assert_eq!(fixture.ran(), None, "codex ran unguarded");
    }

    /// A terminal resume passes the approvals reviewer the session ran
    /// with together with its approval policy, so the node's own reviewer
    /// does not answer what the session's user answered. After a headless
    /// turn, the `never` that `codex exec` recorded is not carried, and
    /// neither is the reviewer that goes with it: the node's policy then
    /// asks, and the node's reviewer answers.
    #[tokio::test]
    async fn a_terminal_resume_carries_the_reviewer_only_with_the_policy() {
        let fixture = Resume::new("codex-cli 0.155.1");
        let rollout = std::fs::read_to_string(&fixture.rollout).unwrap();
        let resume = async |approval: &str| {
            let ran_with = rollout.replace(
                "\"approval_policy\":\"on-request\"",
                &format!("\"approval_policy\":\"{approval}\",\"approvals_reviewer\":\"user\""),
            );
            assert_ne!(ran_with, rollout);
            std::fs::write(&fixture.rollout, ran_with).unwrap();
            let launch = Launch::Interactive {
                resume: Some(SessionId::parse(THREAD).unwrap()),
                prompt: None,
            };
            let mut running = fixture
                .adapter
                .launch(&fixture.home.join("work"), launch, &fixture.env)
                .await
                .unwrap();
            running.release().unwrap();
            running.child_mut().wait().await.unwrap();
            let settings = running.settings();
            let passed: Vec<String> = settings
                .passed
                .iter()
                .map(|p| format!("{} {}", p.name, p.value))
                .collect();
            let left: Vec<String> = settings
                .not_carried
                .iter()
                .map(|n| format!("{} {}", n.name, n.value))
                .collect();
            (fixture.ran().unwrap(), passed, left)
        };

        let (ran, passed, left) = resume("on-request").await;
        assert!(
            ran.iter().any(|a| a == "approvals_reviewer=\"user\"")
                && ran.iter().any(|a| a == "approval_policy=\"on-request\""),
            "{ran:?}"
        );
        assert_eq!(
            passed,
            [
                "model gpt-6-sol",
                "effort xhigh",
                "sandbox workspace-write",
                "approval on-request",
                "approvals reviewer user"
            ]
        );
        assert!(left.is_empty(), "{left:?}");

        let (ran, passed, left) = resume("never").await;
        assert!(
            !ran.iter().any(|a| a.starts_with("approvals_reviewer="))
                && !ran.iter().any(|a| a.starts_with("approval_policy=")),
            "{ran:?}"
        );
        assert_eq!(
            passed,
            ["model gpt-6-sol", "effort xhigh", "sandbox workspace-write"]
        );
        assert_eq!(left, ["approval never", "approvals reviewer user"]);
    }

    /// A terminal resume that passes the thread's model passes its effort
    /// too, whatever effort codex names: with `-c model` set, codex 0.155.1
    /// stops restoring the thread's own model and effort and takes this
    /// node's effort, so an effort left out would drop an `ultra` thread to
    /// the node's default.
    #[tokio::test]
    async fn a_terminal_resume_passes_every_effort_codex_names_with_the_model() {
        let fixture = Resume::new("codex-cli 0.155.1");
        let rollout = std::fs::read_to_string(&fixture.rollout).unwrap();
        for effort in ["ultra", "max", "minimal", "none", "persistent"] {
            let ran_at =
                rollout.replace("\"effort\":\"xhigh\"", &format!("\"effort\":\"{effort}\""));
            assert_ne!(ran_at, rollout);
            std::fs::write(&fixture.rollout, &ran_at).unwrap();
            let launch = Launch::Interactive {
                resume: Some(SessionId::parse(THREAD).unwrap()),
                prompt: None,
            };
            let mut running = fixture
                .adapter
                .launch(&fixture.home.join("work"), launch, &fixture.env)
                .await
                .unwrap();
            running.release().unwrap();
            running.child_mut().wait().await.unwrap();
            let ran = fixture.ran().unwrap();
            let pairs: Vec<&str> = ran
                .iter()
                .filter(|a| a.starts_with("model"))
                .map(String::as_str)
                .collect();
            let expected = format!("model_reasoning_effort=\"{effort}\"");
            assert_eq!(pairs, ["model=\"gpt-6-sol\"", expected.as_str()], "{ran:?}");
            assert!(running.settings().not_carried.is_empty(), "{effort}");
        }
    }

    /// A headless resume passes the model, effort and sandbox the session
    /// ran with and says so, and leaves the approval policy to `codex
    /// exec`; a new thread reads nothing and passes nothing (gate finding
    /// F10a).
    #[tokio::test]
    async fn a_resume_passes_the_settings_the_session_ran_with() {
        let fixture = Resume::new("codex-cli 0.155.1");
        let mut running = fixture.launch(true).await.unwrap();
        running.release().unwrap();
        running.child_mut().wait().await.unwrap();
        let trust = format!(
            "projects.\"{}\".trust_level=trusted",
            fixture.home.join("work").display()
        );
        assert_eq!(
            fixture.ran().unwrap(),
            [
                "exec",
                "resume",
                THREAD,
                "--json",
                "-c",
                &trust,
                "-c",
                "model=\"gpt-6-sol\"",
                "-c",
                "model_reasoning_effort=\"xhigh\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "--",
                "next"
            ]
        );
        let passed: Vec<String> = running
            .settings()
            .passed
            .iter()
            .map(|p| format!("{} {}", p.name, p.value))
            .collect();
        assert_eq!(
            passed,
            ["model gpt-6-sol", "effort xhigh", "sandbox workspace-write"]
        );
        // `codex exec` sets its own approval policy, so a headless resume
        // passes none and says so.
        let left: Vec<String> = running
            .settings()
            .not_carried
            .iter()
            .map(|n| format!("{} {}", n.name, n.value))
            .collect();
        assert_eq!(left, ["approval on-request"]);

        let mut running = fixture.launch(false).await.unwrap();
        running.release().unwrap();
        running.child_mut().wait().await.unwrap();
        assert_eq!(
            fixture.ran().unwrap(),
            ["exec", "--json", "-c", &trust, "--", "next"]
        );
        assert_eq!(
            running.settings(),
            &crate::harness::LaunchSettings::default()
        );
    }
}
