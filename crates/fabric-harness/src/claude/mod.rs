//! The Claude Code adapter.
//!
//! Invariant: a Claude session is carried as the files Claude itself reads
//! on `claude --resume <session-id>`: the transcript and sidecars under
//! every `~/.claude/projects/<key>/` that holds them, the project memory,
//! `file-history`, `tasks` and `uploads` for the session. Files inside the
//! workspace (`.claude/`, `CLAUDE.local.md`) are the workspace snapshot's
//! to carry (ADR 0004), never this adapter's. Credentials, the global
//! `~/.claude.json`, the process registry and the supervisor state never
//! travel (see `denylist`), and a symlink is never followed. Restore
//! accepts only the workspace path and home the state was captured under
//! (ADR 0011), so nothing is rewritten, and places a file only where
//! capture takes one from; any other path in a manifest is refused before
//! anything is written. A capture or restore is refused while a `claude`
//! process registered for the same session is alive, because
//! `place::place_all` cannot protect a file that is being appended to.
//! When the registry cannot answer (it is missing, a record does not
//! parse, or the installed Claude is older than the version it was read
//! on), any running `claude` refuses them instead.
//! The config file is `~/.claude.json`, or `~/.claude/.config.json` when
//! that exists, as Claude 2.1.294 chooses it (`Tt` in its bundle; a
//! temporary home with both files read trust from the second).
//! Login is never copied and no per-job config directory is set:
//! `CLAUDE_CONFIG_DIR` keys the login (ADR 0006, D15). This adapter reads
//! and writes `~/.claude` and `~/.claude.json` only, so `check_env`
//! refuses a launch environment that sets any variable in
//! `MOVES_CONFIG`, whatever its value, and `launch` runs that check
//! first. Read from the 2.1.294 bundle on 2026-10-20: any
//! `CLAUDE_CONFIG_DIR` moves the config file to
//! `$CLAUDE_CONFIG_DIR/.claude.json`, even when it names `~/.claude`,
//! and renames the Keychain item to `Claude Code-credentials-<hash>`; an
//! empty one moves the projects store to a relative path;
//! `CLAUDE_SECURESTORAGE_CONFIG_DIR` renames the Keychain item; and
//! `CLAUDE_CODE_CUSTOM_OAUTH_URL` moves the config file to
//! `.claude-custom-oauth.json` and the login with it.
//!
//! Surfaces used, checked against `claude --help` of 2.1.294 on
//! 2026-10-19: `--resume <id>`, `--session-id <uuid>`, `-p` with
//! `--output-format json`, and the typed flags of `flags::LaunchFlags`.
//! An overlay file that holds a literal secret refuses the launch. The session registry
//! `~/.claude/sessions/<pid>.json` (`pid`, `sessionId`) was read on the
//! owner's machine on 2026-10-20.

pub mod flags;
pub(crate) mod history;
pub(crate) mod slug;
pub(crate) mod trust;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::departure::Records;
use crate::error::{HarnessError, IoOp};
use crate::harness::{Harness, Installed, Launch, Restored, Running};
use crate::jsontext;
use crate::manifest::{HOME_PREFIX, Mutability, StateManifest, under_home};
use crate::pipeline::{self, Capturer, Destination};
use crate::process;
use crate::redact::{RedactionCounts, Redactor};
use crate::session::SessionId;
use crate::stage::{Carry, Source};

use self::flags::LaunchFlags;
pub use self::trust::{LeftReason, LivePids, TrustOutcome};

/// The program name on `PATH`.
pub const PROGRAM: &str = "claude";

/// Variables that make Claude keep its config file, projects store or
/// login somewhere other than `~/.claude.json` and `~/.claude`, with what
/// each moves. Any value counts, the empty string included.
const MOVES_CONFIG: &[(&str, &str)] = &[
    (
        "CLAUDE_CONFIG_DIR",
        "the config file, the projects store and the login",
    ),
    ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "the login"),
    (
        "CLAUDE_CODE_CUSTOM_OAUTH_URL",
        "the config file and the login",
    ),
];

/// The Claude Code adapter for one home directory.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    home: AbsPath,
    config_dir: PathBuf,
    config_json: PathBuf,
    binary: Option<PathBuf>,
    path_var: OsString,
    flags: LaunchFlags,
}

/// The Claude version whose `~/.claude/sessions/<pid>.json` records this
/// adapter was checked against.
const REGISTRY_READ_ON: &[u32] = &[2, 1, 294];

/// What the session registry says about a session.
enum Registry {
    /// These live processes registered the session; empty when none did.
    Answered(Vec<u32>),
    /// The registry cannot be trusted to answer, for this reason.
    CannotAnswer(&'static str),
}

#[derive(Deserialize)]
struct SessionRecord {
    pid: u32,
    #[serde(rename = "sessionId")]
    session_id: String,
}

impl ClaudeCode {
    /// An adapter whose state lives under `home/.claude` and
    /// `home/.claude.json`, finding `claude` on the `PATH` value `path_var`
    /// (the CLI passes the one it launches with, never this process's own).
    #[must_use]
    pub fn new(home: AbsPath, path_var: OsString) -> Self {
        let dir = pipeline::fs_path(&home);
        ClaudeCode {
            config_dir: dir.join(".claude"),
            config_json: dir.join(".claude.json"),
            home,
            binary: None,
            path_var,
            flags: LaunchFlags::default(),
        }
    }

    /// Uses this binary instead of looking `claude` up on `PATH`.
    #[must_use]
    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    /// Flags passed on every launch; resume restores none of them (ADR 0006).
    #[must_use]
    pub fn with_flags(mut self, flags: LaunchFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Refuses a launch environment in which Claude would keep its state
    /// or login somewhere else than this adapter reads and writes: one
    /// that sets any variable in `MOVES_CONFIG`, whatever its value.
    /// `launch` and `preseed_trust` run it on the environment they are
    /// given; the CLI runs it before it captures, which takes none.
    pub fn check_env(env: &[(EnvVarName, String)]) -> Result<(), HarnessError> {
        let set = MOVES_CONFIG
            .iter()
            .find(|(var, _)| env.iter().any(|(name, _)| name.as_str() == *var));
        match set {
            Some(&(var, moves)) => Err(HarnessError::ConfigMoved {
                program: PROGRAM.to_owned(),
                var,
                moves,
            }),
            None => Ok(()),
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

    fn home_dir(&self) -> &Path {
        pipeline::fs_path(&self.home)
    }

    fn key_of(root: &AbsPath) -> Result<String, HarnessError> {
        slug::slug(root).map_err(|e| match e {
            slug::SlugError::TooLong { len } => HarnessError::ProjectKeyTooLong {
                path: PathBuf::from(root.as_str()),
                len,
            },
        })
    }

    /// The newest session whose transcript sits under the workspace's project key.
    pub fn latest_session(&self, workspace: &Path) -> Result<Option<SessionId>, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let dir = self.config_dir.join("projects").join(Self::key_of(&root)?);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &dir, e)),
        };
        let mut newest: Option<(std::time::SystemTime, SessionId)> = None;
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &dir, e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(session) = SessionId::parse(stem) else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .map_err(|e| HarnessError::io(IoOp::Read, &path, e))?;
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, session));
            }
        }
        Ok(newest.map(|(_, s)| s))
    }

    /// Process ids of every `claude` on this node: by name and by the
    /// resolved binary path, so an npm install running as `node` is seen.
    async fn running_claude(&self) -> Result<Vec<u32>, HarnessError> {
        let binary = self.binary().ok();
        process::running_pids(PROGRAM, binary.as_deref()).await
    }

    /// Process ids that may be writing `session` on this node. The session
    /// registry answers when it exists, every record in it parses and the
    /// installed Claude is at least the version the registry was read on.
    /// Otherwise the answer is every `claude` process on the node, so a
    /// registry this adapter cannot read never reads as "no writer".
    async fn session_writers(
        &self,
        session: &SessionId,
        version: Option<&str>,
    ) -> Result<Vec<u32>, HarnessError> {
        match self.registered_writers(session, version).await? {
            Registry::Answered(pids) => Ok(pids),
            Registry::CannotAnswer(reason) => {
                warn!(%session, reason, "claude: the session registry cannot answer; any running claude blocks");
                self.running_claude().await
            }
        }
    }

    /// Live process ids that `~/.claude/sessions/` registers for `session`.
    async fn registered_writers(
        &self,
        session: &SessionId,
        version: Option<&str>,
    ) -> Result<Registry, HarnessError> {
        if !version.is_some_and(|v| process::version_at_least(v, REGISTRY_READ_ON)) {
            return Ok(Registry::CannotAnswer(
                "the claude version is unknown or older than the registry this adapter reads",
            ));
        }
        let dir = self.config_dir.join("sessions");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Registry::CannotAnswer("there is no session registry"));
            }
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &dir, e)),
        };
        let mut pids = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|e| HarnessError::io(IoOp::Walk, &dir, e))?
                .path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                return Ok(Registry::CannotAnswer("a registry record cannot be read"));
            };
            let Ok(record) = serde_json::from_slice::<SessionRecord>(&bytes) else {
                return Ok(Registry::CannotAnswer("a registry record does not parse"));
            };
            if record.session_id == session.as_str() && process::pid_alive(record.pid).await? {
                pids.push(record.pid);
            }
        }
        Ok(Registry::Answered(pids))
    }

    async fn installed_version(&self) -> Option<String> {
        process::installed_version(self.binary().ok(), &self.path_var, PROGRAM).await
    }

    /// Makes sure Claude trusts `workspace`, where Fabric can do so
    /// safely: skips its dialog and applies the workspace's project
    /// settings. The key is the one Claude reads for project settings,
    /// the main checkout of the git repository that holds the workspace,
    /// or the workspace outside a repository (`trust::project_key`).
    /// `~/.claude.json` is read first, and when that key is already
    /// `true`, nothing is written and no process is looked at. Otherwise
    /// `projects["<key>"].hasTrustDialogAccepted` is written while no
    /// `claude` runs; while one runs, or when the file is a symlink or
    /// missing, the file is left to Claude and the outcome says why and
    /// whether a trusted ancestor still skips the dialog, so the caller
    /// can warn and launch. `env` is the environment the caller will launch
    /// with; one that moves Claude's config is refused before anything is
    /// read, because the key would land in a file Claude does not open.
    pub async fn preseed_trust(
        &self,
        workspace: &Path,
        env: &[(EnvVarName, String)],
    ) -> Result<TrustOutcome, HarnessError> {
        self.preseed_trust_with(workspace, env, || self.running_claude())
            .await
    }

    /// `preseed_trust` with the question "which `claude` processes run?"
    /// answered by `running`.
    async fn preseed_trust_with<F, Fut>(
        &self,
        workspace: &Path,
        env: &[(EnvVarName, String)],
        running: F,
    ) -> Result<TrustOutcome, HarnessError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<u32>, HarnessError>>,
    {
        Self::check_env(env)?;
        let root = pipeline::physical_workspace(workspace)?;
        let config_dir = self.config_dir.clone();
        let config_json = self.config_json.clone();
        let (config, key, found) = pipeline::blocking(move || {
            let config = trust::global_config(&config_dir, config_json);
            let key = trust::project_key(&root)?;
            let found = trust::read(&config, &root, &key)?;
            Ok((config, key, found))
        })
        .await?;
        let key_path = pipeline::fs_path(&key).to_path_buf();
        let inherited = match found {
            trust::Found::Exact => return Ok(TrustOutcome::AlreadyTrusted { key: key_path }),
            trust::Found::Ancestor(by) => Some(by),
            trust::Found::Nothing => None,
        };
        let left = |reason| match inherited.clone() {
            Some(by) => TrustOutcome::Inherited { by, reason },
            None => TrustOutcome::LeftToClaude { reason },
        };
        let checked = config.clone();
        if let Some(reason) = pipeline::blocking(move || trust::unwritable(&checked)).await? {
            return Ok(left(reason));
        }
        if let Some(pids) = LivePids::of(running().await?) {
            debug!(
                ?pids,
                key = key.as_str(),
                "claude: trust key not set and claude is running; left to Claude"
            );
            return Ok(left(LeftReason::ClaudeRunning { pids }));
        }
        let changed = pipeline::blocking(move || trust::preseed_file(&config, &key)).await?;
        Ok(if changed {
            TrustOutcome::Written { key: key_path }
        } else {
            TrustOutcome::AlreadyTrusted { key: key_path }
        })
    }

    fn carry_for(path: &Path) -> Carry {
        match path.extension().and_then(|e| e.to_str()) {
            Some("jsonl") => Carry::JsonLines,
            Some("json") => Carry::JsonDocument,
            _ => Carry::Text,
        }
    }

    /// What sits at `path`, without following a symlink there.
    fn kind_at(path: &Path) -> Result<Option<std::fs::FileType>, HarnessError> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(meta.file_type())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(HarnessError::io(IoOp::Read, path, e)),
        }
    }

    fn add_tree(&self, sources: &mut Vec<Source>, dir: &Path) -> Result<(), HarnessError> {
        match Self::kind_at(dir)? {
            Some(kind) if kind.is_symlink() => Err(HarnessError::SymlinkInState {
                path: dir.to_path_buf(),
            }),
            Some(kind) if kind.is_dir() => {
                for file in pipeline::walk(dir)? {
                    self.add_file(sources, &dir.join(&file))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn add_file(&self, sources: &mut Vec<Source>, abs: &Path) -> Result<bool, HarnessError> {
        match Self::kind_at(abs)? {
            Some(kind) if kind.is_symlink() => {
                return Err(HarnessError::SymlinkInState {
                    path: abs.to_path_buf(),
                });
            }
            Some(kind) if kind.is_file() => {}
            _ => return Ok(false),
        }
        let rel_to_home =
            abs.strip_prefix(self.home_dir())
                .map_err(|_| HarnessError::NotUnderHome {
                    path: abs.to_path_buf(),
                    home: self.home_dir().to_path_buf(),
                })?;
        let parts = pipeline::rel_components(rel_to_home)?;
        let mut all: Vec<&str> = vec![HOME_PREFIX];
        all.extend(parts.iter().map(String::as_str));
        let rest: Vec<&str> = parts.iter().map(String::as_str).collect();
        sources.push(Source {
            rel: pipeline::rel_path(&all)?,
            carry: Self::carry_for(abs),
            mutability: Self::mutability_for(&rest),
            abs: abs.to_path_buf(),
        });
        Ok(true)
    }

    /// How Claude changes the file at these components under the home:
    /// transcripts only grow, tool results, file backups and uploads are
    /// written once, and everything else (memory, tasks, sidecar
    /// metadata) is rewritten in place.
    fn mutability_for(parts: &[&str]) -> Mutability {
        let jsonl = |name: &str| Path::new(name).extension().is_some_and(|e| e == "jsonl");
        match parts {
            [".claude", "projects", _, _, "tool-results", ..]
            | [".claude", "file-history" | "uploads", ..] => Mutability::Immutable,
            [".claude", "projects", _, .., name] if jsonl(name) => Mutability::AppendOnly,
            _ => Mutability::Replaceable,
        }
    }

    /// Where this node records the session's rewritable files at capture.
    fn departure(&self, session: &SessionId) -> Records {
        Records::of(self.home_dir(), PROGRAM, session)
    }

    /// True when a tree path names a file this adapter captures for
    /// `session` of the workspace whose key is `root_key`. Restore refuses
    /// every other path, so a manifest cannot place a file elsewhere.
    fn restorable(rel: &str, session: &SessionId, root_key: &str) -> bool {
        let Some(rest) = under_home(rel) else {
            return false;
        };
        let parts: Vec<&str> = rest.split('/').collect();
        let sid = session.as_str();
        let prefix8: String = sid.chars().take(8).collect();
        let tasks = format!("session-{prefix8}");
        let transcript = format!("{sid}.jsonl");
        match parts.as_slice() {
            [".claude", "projects", _, name] => *name == transcript,
            [".claude", "projects", _, dir, _, ..] if *dir == sid => true,
            [".claude", "projects", key, "memory", _, ..] => *key == root_key,
            [".claude", "file-history" | "uploads", dir, _, ..] => *dir == sid,
            [".claude", "tasks", dir, _, ..] => *dir == tasks,
            _ => false,
        }
    }

    /// Adds the backups under `file-history/<sid>/` that the transcripts
    /// already in `sources` name for a file the denylist allows.
    fn add_file_history(
        &self,
        sources: &mut Vec<Source>,
        root: &AbsPath,
        session: &SessionId,
        counts: &mut RedactionCounts,
    ) -> Result<(), HarnessError> {
        let dir = self.config_dir.join("file-history").join(session.as_str());
        match Self::kind_at(&dir)? {
            Some(kind) if kind.is_symlink() => {
                return Err(HarnessError::SymlinkInState { path: dir });
            }
            Some(kind) if kind.is_dir() => {}
            _ => return Ok(()),
        }
        let transcripts: Vec<&Path> = sources
            .iter()
            .filter(|s| s.carry == Carry::JsonLines)
            .map(|s| s.abs.as_path())
            .collect();
        let backups =
            history::Backups::read(&transcripts, self.home_dir(), pipeline::fs_path(root))?;
        for file in pipeline::walk(&dir)? {
            let name = file.to_string_lossy();
            if backups.admit(&name, counts) {
                self.add_file(sources, &dir.join(&file))?;
            }
        }
        Ok(())
    }

    fn sources_for(
        &self,
        root: &AbsPath,
        session: &SessionId,
        counts: &mut RedactionCounts,
    ) -> Result<Vec<Source>, HarnessError> {
        let mut sources = Vec::new();
        let projects = self.config_dir.join("projects");
        let mut found_transcript = false;
        let entries = match std::fs::read_dir(&projects) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &projects, e)),
        };
        if let Some(entries) = entries {
            let mut keys = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &projects, e))?;
                let path = entry.path();
                let kind = entry
                    .file_type()
                    .map_err(|e| HarnessError::io(IoOp::Walk, &path, e))?;
                if kind.is_dir() {
                    keys.push(path);
                } else if kind.is_symlink()
                    && (path.join(format!("{session}.jsonl")).exists()
                        || path.join(session.as_str()).exists())
                {
                    return Err(HarnessError::SymlinkInState { path });
                }
            }
            keys.sort();
            for key_dir in keys {
                let transcript = key_dir.join(format!("{session}.jsonl"));
                if self.add_file(&mut sources, &transcript)? {
                    found_transcript = true;
                }
                self.add_tree(&mut sources, &key_dir.join(session.as_str()))?;
            }
        }
        if !found_transcript {
            return Err(HarnessError::SessionNotFound {
                kind: HarnessKind::ClaudeCode,
                session: session.clone(),
                searched: projects,
            });
        }
        let key = Self::key_of(root)?;
        self.add_tree(&mut sources, &projects.join(&key).join("memory"))?;
        self.add_file_history(&mut sources, root, session, counts)?;
        let prefix8: String = session.as_str().chars().take(8).collect();
        self.add_tree(
            &mut sources,
            &self
                .config_dir
                .join("tasks")
                .join(format!("session-{prefix8}")),
        )?;
        self.add_tree(
            &mut sources,
            &self.config_dir.join("uploads").join(session.as_str()),
        )?;
        Ok(sources)
    }

    fn capture_sync(
        &self,
        root: &AbsPath,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let mut counts = RedactionCounts::new();
        let sources = self.sources_for(root, session, &mut counts)?;
        let records = self.departure(session);
        let capturer = Capturer {
            kind: HarnessKind::ClaudeCode,
            session,
            root,
            home: &self.home,
            version,
            departure: Some(&records),
        };
        let manifest = pipeline::capture(capturer, &sources, counts, out)?;
        info!(%session, files = manifest.files().len(), "claude: captured");
        Ok(manifest)
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let version = self.installed_version().await;
        let pids = self.session_writers(session, version.as_deref()).await?;
        if !pids.is_empty() {
            return Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            });
        }
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        pipeline::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
    }

    /// Where a tree path of `session` lands under the home, and how
    /// capture treats it; `None` for a path capture never takes.
    fn destination(&self, rel: &str, session: &SessionId, root_key: &str) -> Option<Destination> {
        let rest = under_home(rel).filter(|_| Self::restorable(rel, session, root_key))?;
        let parts: Vec<&str> = rest.split('/').collect();
        Some(Destination {
            path: self.home_dir().join(rest),
            content: Self::carry_for(Path::new(rest)).content(),
            mutability: Self::mutability_for(&parts),
        })
    }

    fn restore_sync(
        &self,
        root: &AbsPath,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let root_key = Self::key_of(root)?;
        let session = manifest.session().clone();
        let records = self.departure(&session);
        let restored = pipeline::restore(manifest, state, Some(&records), |rel, _| {
            Ok(self.destination(rel, &session, &root_key))
        })?;
        info!(%session, files = restored.placed.len(), "claude: restored");
        Ok(restored)
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &HarnessKind::ClaudeCode, &root, &self.home)?;
        let version = self.installed_version().await;
        let pids = self
            .session_writers(manifest.session(), version.as_deref())
            .await?;
        if !pids.is_empty() {
            return Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            });
        }
        let adapter = self.clone();
        let state = state.to_path_buf();
        pipeline::blocking(move || adapter.restore_sync(&root, &state, manifest)).await
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        process::detect(self.binary().ok(), &self.path_var).await
    }

    /// The session the process runs and the argument list for `launch`,
    /// without the binary: `--resume` with the session `launch` names, or
    /// `--session-id` with the id `fresh` mints, so the flag and the id
    /// always agree.
    pub(crate) fn args_for(
        &self,
        launch: &Launch,
        fresh: impl FnOnce() -> SessionId,
    ) -> (SessionId, Vec<String>) {
        let mut args = Vec::new();
        if launch.is_headless() {
            args.extend(["-p", "--output-format", "json"].map(str::to_owned));
        }
        let (flag, session) = match launch.resume() {
            Some(session) => ("--resume", session.clone()),
            None => ("--session-id", fresh()),
        };
        args.push(flag.to_owned());
        args.push(session.as_str().to_owned());
        args.extend(self.flags.args());
        if let Some(prompt) = launch.prompt() {
            args.push("--".to_owned());
            args.push(prompt.to_owned());
        }
        (session, args)
    }

    fn fresh_session() -> SessionId {
        SessionId::from_uuid(uuid::Uuid::new_v4())
    }

    fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        Self::check_env(env)?;
        let root = pipeline::physical_workspace(workspace)?;
        let binary = self.binary()?;
        for overlay in self.flags.overlays() {
            refuse_secret_overlay(Path::new(overlay.as_str()))?;
        }
        let (session, args) = self.args_for(launch, Self::fresh_session);
        let (child, pid) =
            process::spawn(&binary, &args, pipeline::fs_path(&root), env, launch.into())?;
        info!(%session, %pid, "claude: launched");
        Ok(Running::new(pid, Some(session), child))
    }
}

/// Refuses an overlay file (settings, MCP configuration) that holds a
/// literal secret: ADR 0006 allows only `${VAR}` references there. The
/// overlay is JSON, so its strings are matched decoded too.
fn refuse_secret_overlay(path: &Path) -> Result<(), HarnessError> {
    let bytes = std::fs::read(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    if jsontext::holds_secret(&bytes, Redactor::standard()?) {
        return Err(HarnessError::SecretInOverlay {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

impl Harness for ClaudeCode {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
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

    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        let records = self.departure(manifest.session());
        let manifest = manifest.clone();
        Box::pin(pipeline::blocking(move || {
            pipeline::confirm_departure(&manifest, &HarnessKind::ClaudeCode, Some(&records))
        }))
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
        Box::pin(async move { self.launch_impl(workspace, &launch, env) })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::Value;

    use super::*;

    /// A temporary home with `.claude.json` holding `config` (none when
    /// `None`) and a workspace directory `<home>/a/work`, both physical.
    struct TrustFixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
        work: PathBuf,
        adapter: ClaudeCode,
    }

    impl TrustFixture {
        fn new(config: Option<&dyn Fn(&Path) -> String>) -> Self {
            Self::named("work", config)
        }

        /// A fixture whose workspace directory is `<home>/a/<name>`.
        fn named(name: &str, config: Option<&dyn Fn(&Path) -> String>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = std::fs::canonicalize(dir.path()).unwrap();
            let work = home.join("a").join(name);
            std::fs::create_dir_all(&work).unwrap();
            if let Some(config) = config {
                std::fs::write(home.join(".claude.json"), config(&work)).unwrap();
            }
            let adapter = ClaudeCode::new(
                AbsPath::parse(home.to_str().unwrap()).unwrap(),
                OsString::new(),
            );
            TrustFixture {
                _dir: dir,
                home,
                work,
                adapter,
            }
        }

        fn config(&self) -> PathBuf {
            self.home.join(".claude.json")
        }

        fn config_bytes(&self) -> Option<Vec<u8>> {
            std::fs::read(self.config()).ok()
        }

        /// Runs `preseed_trust` with `running` as the process answer;
        /// returns the outcome and whether processes were asked about.
        async fn preseed(&self, running: Vec<u32>) -> (Result<TrustOutcome, HarnessError>, bool) {
            self.preseed_at(&self.work, running).await
        }

        /// `preseed` with the workspace given as `workspace`.
        async fn preseed_at(
            &self,
            workspace: &Path,
            running: Vec<u32>,
        ) -> (Result<TrustOutcome, HarnessError>, bool) {
            self.preseed_with(workspace, &[], running).await
        }

        /// `preseed_at` with the launch environment `env`.
        async fn preseed_with(
            &self,
            workspace: &Path,
            env: &[(EnvVarName, String)],
            running: Vec<u32>,
        ) -> (Result<TrustOutcome, HarnessError>, bool) {
            let asked = Cell::new(false);
            let outcome = self
                .adapter
                .preseed_trust_with(workspace, env, || {
                    asked.set(true);
                    async move { Ok(running) }
                })
                .await;
            (outcome, asked.get())
        }
    }

    fn trusting(dir: &Path) -> String {
        serde_json::json!({
            "projects": { dir.to_str().unwrap(): { "hasTrustDialogAccepted": true } }
        })
        .to_string()
    }

    fn trust_at(fixture: &TrustFixture, key: &Path) -> Value {
        let value: Value = serde_json::from_slice(&fixture.config_bytes().unwrap()).unwrap();
        value["projects"][key.to_str().unwrap()][trust::TRUST_KEY].clone()
    }

    fn trust_of(fixture: &TrustFixture) -> Value {
        trust_at(fixture, &fixture.work)
    }

    fn written(key: &Path) -> TrustOutcome {
        TrustOutcome::Written {
            key: key.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn a_trusted_ancestor_alone_is_inherited_and_left_to_a_running_claude() {
        let fixture = TrustFixture::new(Some(&|work: &Path| trusting(work.parent().unwrap())));
        let before = fixture.config_bytes();
        let (outcome, asked) = fixture.preseed(vec![4242]).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::Inherited {
                by: fixture.work.parent().unwrap().to_path_buf(),
                reason: LeftReason::ClaudeRunning {
                    pids: LivePids::of(vec![4242]).unwrap()
                }
            },
            "Claude skips its dialog there but drops the project settings"
        );
        assert!(asked);
        assert_eq!(fixture.config_bytes(), before);
    }

    #[tokio::test]
    async fn a_trusted_ancestor_alone_gets_the_exact_key_while_no_claude_runs() {
        let fixture = TrustFixture::new(Some(&|work: &Path| trusting(work.parent().unwrap())));
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert_eq!(outcome.unwrap(), written(&fixture.work));
        assert!(asked);
        assert_eq!(trust_of(&fixture), Value::Bool(true));
    }

    #[tokio::test]
    async fn a_workspace_reached_through_a_symlink_is_keyed_by_its_physical_path() {
        let fixture = TrustFixture::new(Some(&|work: &Path| trusting(work)));
        let link = fixture.home.join("link");
        std::os::unix::fs::symlink(&fixture.work, &link).unwrap();
        let (outcome, asked) = fixture.preseed_at(&link, Vec::new()).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::AlreadyTrusted {
                key: fixture.work.clone()
            }
        );
        assert!(!asked);
    }

    #[tokio::test]
    async fn a_trusted_directory_above_a_symlink_to_the_workspace_does_not_count() {
        let fixture = TrustFixture::new(Some(&|work: &Path| {
            let links = work.parent().unwrap().parent().unwrap().join("links");
            std::fs::create_dir(&links).unwrap();
            trusting(&links)
        }));
        let link = fixture.home.join("links").join("work");
        std::os::unix::fs::symlink(&fixture.work, &link).unwrap();
        let (outcome, asked) = fixture.preseed_at(&link, Vec::new()).await;
        assert_eq!(
            outcome.unwrap(),
            written(&fixture.work),
            "Claude walks the physical path, so the link's own parent must not count"
        );
        assert!(asked);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_workspace_given_in_other_letter_case_is_keyed_by_its_spelling_on_disk() {
        let fixture =
            TrustFixture::named("Work", Some(&|_: &Path| r#"{"projects":{}}"#.to_owned()));
        let typed = fixture.home.join("a").join("work");
        if !typed.exists() {
            return;
        }
        let (outcome, _) = fixture.preseed_at(&typed, Vec::new()).await;
        assert_eq!(
            outcome.unwrap(),
            written(&fixture.work),
            "Claude keys trust by getcwd, which spells the path as it is stored"
        );
    }

    #[tokio::test]
    async fn a_trusted_workspace_root_writes_nothing() {
        let fixture = TrustFixture::new(Some(&|work: &Path| trusting(work)));
        let before = fixture.config_bytes();
        let (outcome, asked) = fixture.preseed(vec![4242]).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::AlreadyTrusted {
                key: fixture.work.clone()
            }
        );
        assert!(!asked);
        assert_eq!(fixture.config_bytes(), before);
    }

    #[tokio::test]
    async fn a_trusted_directory_above_the_git_root_does_not_count() {
        let fixture = TrustFixture::new(Some(&|work: &Path| {
            std::fs::create_dir(work.join(".git")).unwrap();
            trusting(work.parent().unwrap())
        }));
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert_eq!(outcome.unwrap(), written(&fixture.work));
        assert!(asked);
        assert_eq!(trust_of(&fixture), Value::Bool(true));
    }

    #[tokio::test]
    async fn a_workspace_below_its_git_root_is_keyed_by_the_root() {
        let fixture = TrustFixture::new(Some(&|work: &Path| {
            std::fs::create_dir(work.parent().unwrap().join(".git")).unwrap();
            trusting(work)
        }));
        let repo = fixture.work.parent().unwrap().to_path_buf();
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert_eq!(
            outcome.unwrap(),
            written(&repo),
            "Claude reads project settings trust at the repository root, not the subdirectory"
        );
        assert!(asked);
        assert_eq!(trust_at(&fixture, &repo), Value::Bool(true));
    }

    #[tokio::test]
    async fn a_linked_worktree_is_keyed_by_its_main_checkout() {
        let fixture = TrustFixture::new(Some(&|work: &Path| {
            let main = work.parent().unwrap().parent().unwrap().join("main");
            let gitdir = main.join(".git").join("worktrees").join("work");
            std::fs::create_dir_all(&gitdir).unwrap();
            std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
            std::fs::write(
                gitdir.join("gitdir"),
                format!("{}\n", work.join(".git").display()),
            )
            .unwrap();
            std::fs::write(work.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
            trusting(&main)
        }));
        let main = fixture.home.join("main");
        let before = fixture.config_bytes();
        let (outcome, asked) = fixture.preseed(vec![4242]).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::AlreadyTrusted { key: main },
            "Claude reads a worktree's trust at its main checkout"
        );
        assert!(!asked);
        assert_eq!(fixture.config_bytes(), before);
    }

    #[tokio::test]
    async fn an_untrusted_workspace_is_written_while_no_claude_runs() {
        let fixture = TrustFixture::new(Some(&|_: &Path| r#"{"projects":{}}"#.to_owned()));
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert_eq!(outcome.unwrap(), written(&fixture.work));
        assert!(asked);
        assert_eq!(trust_of(&fixture), Value::Bool(true));
    }

    #[tokio::test]
    async fn an_untrusted_workspace_is_left_to_a_running_claude() {
        let fixture = TrustFixture::new(Some(&|_: &Path| r#"{"projects":{}}"#.to_owned()));
        let before = fixture.config_bytes();
        let (outcome, asked) = fixture.preseed(vec![4242]).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::LeftToClaude {
                reason: LeftReason::ClaudeRunning {
                    pids: LivePids::of(vec![4242]).unwrap()
                }
            }
        );
        assert!(asked);
        assert_eq!(
            fixture.config_bytes(),
            before,
            "Claude owns the file while it runs"
        );
    }

    #[tokio::test]
    async fn a_workspace_stored_in_nfd_is_refused_before_trust_is_read() {
        let decomposed = "masau\u{308}stu\u{308}";
        let fixture = TrustFixture::named(
            decomposed,
            Some(&|_: &Path| r#"{"projects":{}}"#.to_owned()),
        );
        let before = fixture.config_bytes();
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert!(
            matches!(
                outcome,
                Err(HarnessError::Malformed {
                    what: crate::error::Input::WorkspacePath,
                    ..
                })
            ),
            "Claude keys trust by the NFC path, so an NFD root must not be written: {outcome:?}"
        );
        assert!(!asked);
        assert_eq!(fixture.config_bytes(), before);
    }

    #[tokio::test]
    async fn a_symlinked_config_is_never_written_through() {
        let fixture = TrustFixture::new(None);
        let dotfiles = fixture.home.join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let target = dotfiles.join("claude.json");
        std::fs::write(&target, r#"{"projects":{}}"#).unwrap();
        std::os::unix::fs::symlink(&target, fixture.config()).unwrap();
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::LeftToClaude {
                reason: LeftReason::ConfigIsSymlink {
                    path: fixture.config()
                }
            }
        );
        assert!(!asked);
        assert!(
            std::fs::symlink_metadata(fixture.config())
                .unwrap()
                .file_type()
                .is_symlink(),
            "the user's link must survive"
        );
        assert_eq!(std::fs::read(&target).unwrap(), br#"{"projects":{}}"#);
    }

    #[tokio::test]
    async fn a_launch_env_that_moves_the_config_is_refused_before_trust_is_read() {
        let fixture = TrustFixture::new(Some(&|_: &Path| r#"{"projects":{}}"#.to_owned()));
        let before = fixture.config_bytes();
        let env = [(
            EnvVarName::parse("CLAUDE_CONFIG_DIR").unwrap(),
            fixture.home.join(".claude").to_string_lossy().into_owned(),
        )];
        let (outcome, asked) = fixture.preseed_with(&fixture.work, &env, Vec::new()).await;
        assert!(
            matches!(outcome, Err(HarnessError::ConfigMoved { .. })),
            "{outcome:?}"
        );
        assert!(!asked);
        assert_eq!(
            fixture.config_bytes(),
            before,
            "Claude would not read a key written here"
        );
    }

    #[tokio::test]
    async fn a_legacy_config_file_is_the_one_read_and_written() {
        let fixture = TrustFixture::new(Some(&|_: &Path| r#"{"projects":{}}"#.to_owned()));
        let legacy = fixture.home.join(".claude").join(".config.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"projects":{}}"#).unwrap();
        let before = fixture.config_bytes();
        let (outcome, _) = fixture.preseed(Vec::new()).await;
        assert_eq!(outcome.unwrap(), written(&fixture.work));
        let value: Value = serde_json::from_slice(&std::fs::read(&legacy).unwrap()).unwrap();
        assert_eq!(
            value["projects"][fixture.work.to_str().unwrap()][trust::TRUST_KEY],
            Value::Bool(true),
            "Claude reads ~/.claude/.config.json when it exists"
        );
        assert_eq!(fixture.config_bytes(), before);
    }

    #[tokio::test]
    async fn a_malformed_config_is_an_error_and_stays_as_it_was() {
        let fixture = TrustFixture::new(Some(&|_: &Path| r#"{"projects":"#.to_owned()));
        let before = fixture.config_bytes();
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert!(matches!(
            outcome,
            Err(HarnessError::Malformed {
                what: crate::error::Input::ConfigJson,
                ..
            })
        ));
        assert!(!asked);
        assert_eq!(fixture.config_bytes(), before);
    }

    #[tokio::test]
    async fn a_missing_config_is_left_to_claude_and_not_created() {
        let fixture = TrustFixture::new(None);
        let (outcome, asked) = fixture.preseed(Vec::new()).await;
        assert_eq!(
            outcome.unwrap(),
            TrustOutcome::LeftToClaude {
                reason: LeftReason::NoConfig {
                    path: fixture.config()
                }
            }
        );
        assert!(!asked);
        assert!(!fixture.config().exists());
    }

    #[test]
    fn restore_accepts_only_the_places_capture_reads() {
        let sid = SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap();
        let ok = |rel: &str| ClaudeCode::restorable(rel, &sid, "-w");
        for rel in [
            "home/.claude/projects/-w/9c9796f0-2682-4546-a963-d314b3ad543f.jsonl",
            "home/.claude/projects/-other/9c9796f0-2682-4546-a963-d314b3ad543f/tool-results/1.txt",
            "home/.claude/projects/-w/memory/MEMORY.md",
            "home/.claude/file-history/9c9796f0-2682-4546-a963-d314b3ad543f/abc@v1",
            "home/.claude/tasks/session-9c9796f0/1.json",
            "home/.claude/uploads/9c9796f0-2682-4546-a963-d314b3ad543f/a.png",
        ] {
            assert!(ok(rel), "{rel} must be accepted");
        }
        for rel in [
            "home/Library/LaunchAgents/evil.plist",
            "home/.zshrc",
            "home/.claude/settings.json",
            "home/.claude/projects/-w/other-session.jsonl",
            "home/.claude/projects/-other/memory/MEMORY.md",
            "home/.claude/file-history/another/abc@v1",
            "home/.claude/tasks/session-00000000/1.json",
            "home/.claude/projects/-w/9c9796f0-2682-4546-a963-d314b3ad543f",
        ] {
            assert!(!ok(rel), "{rel} must be refused");
        }
    }

    #[test]
    fn args_follow_the_cli_surface() {
        let adapter = ClaudeCode::new(AbsPath::parse("/h").unwrap(), OsString::new()).with_flags(
            LaunchFlags {
                permission_mode: Some(flags::PermissionMode::AcceptEdits),
                ..LaunchFlags::default()
            },
        );
        let session = SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap();
        let resume = Launch::Interactive {
            resume: Some(session.clone()),
            prompt: None,
        };
        assert_eq!(
            adapter.args_for(&resume, ClaudeCode::fresh_session),
            (
                session.clone(),
                vec![
                    "--resume",
                    "9c9796f0-2682-4546-a963-d314b3ad543f",
                    "--permission-mode",
                    "acceptEdits"
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            )
        );
        let headless = Launch::Headless {
            resume: Some(session.clone()),
            prompt: "--not-a-flag".to_owned(),
        };
        assert_eq!(
            adapter.args_for(&headless, ClaudeCode::fresh_session).1,
            vec![
                "-p",
                "--output-format",
                "json",
                "--resume",
                "9c9796f0-2682-4546-a963-d314b3ad543f",
                "--permission-mode",
                "acceptEdits",
                "--",
                "--not-a-flag"
            ]
        );
        let fresh = ClaudeCode::fresh_session();
        let new = Launch::Interactive {
            resume: None,
            prompt: Some("hi".to_owned()),
        };
        assert_eq!(
            adapter.args_for(&new, || fresh.clone()),
            (
                fresh.clone(),
                vec![
                    "--session-id",
                    fresh.as_str(),
                    "--permission-mode",
                    "acceptEdits",
                    "--",
                    "hi"
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            )
        );
    }
}
