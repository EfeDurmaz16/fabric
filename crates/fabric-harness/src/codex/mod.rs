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
//! would keep appending to a rollout that restore renamed away. The
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
//! typed `overrides::CodexOverrides`, never from free text. The prompt is
//! passed after `--`, which clap reads as the end of options. The
//! app-server JSON-RPC surface is not used (ADR 0006, D14).

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
use tracing::{info, warn};

use crate::error::{HarnessError, Input, IoOp};
use crate::harness::{Harness, Installed, Launch, Restored, Running, TurnReport};
use crate::manifest::{Content, HOME_PREFIX, Mutability, StateManifest};
use crate::pipeline::{self, Capturer, Destination};
use crate::place::{self, Relation};
use crate::process;
use crate::redact::RedactionCounts;
use crate::session::SessionId;
use crate::stage::{Carry, Source};

use self::overrides::CodexOverrides;
use self::rollout::{RolloutFile, RolloutName};

/// The program name on `PATH`.
pub const PROGRAM: &str = "codex";

/// The folder under `sessions/` where restored lineages are placed when
/// this node holds no file for the rollout yet.
pub const RESTORE_SUBDIR: &str = "fabric";

const LOCK_DIR: &str = "thread-writer-locks";
const COORDINATION_LOCK: &str = ".coordination.lock";

/// The Codex version whose thread writer lock this adapter was checked against.
const LOCK_READ_ON: &[u32] = &[0, 155, 1];

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
}

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
        }
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
    /// not restore them (H2, experiment E3).
    #[must_use]
    pub fn with_overrides(mut self, overrides: CodexOverrides) -> Self {
        self.overrides = Some(overrides);
        self
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
        let mut all = rollout::list(&self.store.join("sessions"))?;
        all.extend(rollout::list(&self.store.join("archived_sessions"))?);
        Ok(all)
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

    /// The rollout lineage of `session` in this store, newest suffix first.
    pub(crate) fn lineage(&self, session: &SessionId) -> Result<Vec<RolloutFile>, HarnessError> {
        lineage_in(&self.all_rollouts()?, session, &self.store.join("sessions"))
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
        if version.is_some_and(|v| process::version_at_least(v, LOCK_READ_ON)) {
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
        let version = process::installed_version(self.binary().ok(), &self.path_var, PROGRAM).await;
        self.refuse_lockless_writers(version.as_deref()).await?;
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
        let version = process::installed_version(self.binary().ok(), &self.path_var, PROGRAM).await;
        self.refuse_lockless_writers(version.as_deref()).await?;
        let adapter = self.clone();
        let state = state.to_path_buf();
        pipeline::blocking(move || adapter.restore_sync(&state, manifest)).await
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        process::detect(self.binary().ok(), &self.path_var).await
    }

    /// The argument list for `launch`, without the binary.
    pub(crate) fn args_for(
        &self,
        root: &AbsPath,
        launch: &Launch,
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
        for pair in self.overrides.iter().flat_map(CodexOverrides::pairs) {
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
        let args = self.args_for(&root, launch)?;
        let session = launch.resume().cloned();
        let spawned =
            process::spawn(&binary, &args, pipeline::fs_path(&root), env, launch.into()).await?;
        info!(?session, pid = %spawned.pid, held = spawned.held, "codex: launched");
        Ok(Running::new(session, spawned))
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
        let adapter = Codex::new(AbsPath::parse("/h").unwrap(), OsString::new()).with_overrides(
            CodexOverrides {
                sandbox: overrides::SandboxMode::WorkspaceWrite,
                approval: overrides::ApprovalPolicy::OnRequest,
                model: Some(crate::harness::ModelName::parse("o3").unwrap()),
                effort: Some(overrides::Effort::High),
            },
        );
        let root = &AbsPath::parse("/h/fabric/jobs/j/work").unwrap();
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let resume = Launch::Interactive {
            resume: Some(session.clone()),
            prompt: None,
        };
        assert_eq!(
            adapter.args_for(root, &resume).unwrap(),
            vec![
                "resume",
                "019f233e-bb33-7342-85fa-f2d33b14368a",
                "-c",
                "projects.\"/h/fabric/jobs/j/work\".trust_level=trusted",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "-c",
                "approval_policy=\"on-request\"",
                "-c",
                "model=\"o3\"",
                "-c",
                "model_reasoning_effort=\"high\""
            ]
        );
        let headless = Launch::Headless {
            resume: Some(session),
            prompt: "--dangerously-bypass-approvals-and-sandbox".to_owned(),
        };
        assert_eq!(
            adapter.args_for(root, &headless).unwrap(),
            vec![
                "exec",
                "resume",
                "019f233e-bb33-7342-85fa-f2d33b14368a",
                "--json",
                "-c",
                "projects.\"/h/fabric/jobs/j/work\".trust_level=trusted",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "-c",
                "approval_policy=\"on-request\"",
                "-c",
                "model=\"o3\"",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--",
                "--dangerously-bypass-approvals-and-sandbox"
            ]
        );
        let quoted = &AbsPath::parse("/h/say \"hi\"\\there").unwrap();
        let args = adapter
            .args_for(
                quoted,
                &Launch::Interactive {
                    resume: None,
                    prompt: Some("go".to_owned()),
                },
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
}
