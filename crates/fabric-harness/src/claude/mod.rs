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
//! `files::place_all` cannot protect a file that is being appended to.
//! When the registry cannot answer (it is missing, a record does not
//! parse, or the installed Claude is older than the version it was read
//! on), any running `claude` refuses them instead.
//! Login is never copied and no per-job config directory is set:
//! `CLAUDE_CONFIG_DIR` keys the login (ADR 0006, D15).
//!
//! Surfaces used, checked against `claude --help` of 2.1.294 on
//! 2026-10-19: `--resume <id>`, `--session-id <uuid>`, `-p` with
//! `--output-format json`, and pass-through flags such as `--settings`,
//! `--mcp-config` and `--permission-mode`. The session registry
//! `~/.claude/sessions/<pid>.json` (`pid`, `sessionId`) was read on the
//! owner's machine on 2026-10-20.

pub(crate) mod history;
pub mod slug;
pub(crate) mod trust;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use serde::Deserialize;
use tracing::{info, warn};

use crate::denylist;
use crate::departure::{self, Departure};
use crate::error::HarnessError;
use crate::files::{self, Carry, Placement, Source};
use crate::harness::{self, Harness, Installed, Io, Launch, Restored, Running};
use crate::manifest::{
    HOME_PREFIX, MANIFEST_FORMAT, Mutability, Redaction, StateManifest, under_home,
};
use crate::process;
use crate::redact::RedactionCounts;
use crate::session::SessionId;

/// The program name on `PATH`.
pub const PROGRAM: &str = "claude";

/// The Claude Code adapter for one home directory.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    home: PathBuf,
    config_dir: PathBuf,
    config_json: PathBuf,
    binary: Option<PathBuf>,
    path_var: Option<OsString>,
    flags: Vec<String>,
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
    /// An adapter whose state lives under `home/.claude` and `home/.claude.json`.
    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        ClaudeCode {
            config_dir: home.join(".claude"),
            config_json: home.join(".claude.json"),
            home,
            binary: None,
            path_var: std::env::var_os("PATH"),
            flags: Vec::new(),
        }
    }

    /// Uses this binary instead of looking `claude` up on `PATH`.
    #[must_use]
    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    /// Looks `claude` up on this `PATH` value instead of the process's.
    #[must_use]
    pub fn with_path_var(mut self, path_var: OsString) -> Self {
        self.path_var = Some(path_var);
        self
    }

    /// Flags passed on every launch, such as `--settings <file>`,
    /// `--mcp-config <file>` or `--permission-mode acceptEdits`; resume
    /// restores none of them (ADR 0006).
    #[must_use]
    pub fn with_flags(mut self, flags: Vec<String>) -> Self {
        self.flags = flags;
        self
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        if let Some(binary) = &self.binary {
            return Ok(binary.clone());
        }
        process::find_on_path(PROGRAM, self.path_var.as_deref()).ok_or_else(|| {
            HarnessError::NotInstalled {
                program: PROGRAM.to_owned(),
            }
        })
    }

    fn key_of(root: &Path) -> Result<String, HarnessError> {
        slug::slug(root).map_err(|e| match e {
            slug::SlugError::TooLong { len } => HarnessError::ProjectKeyTooLong {
                path: root.to_path_buf(),
                len,
            },
            slug::SlugError::NotUtf8 => HarnessError::Malformed {
                what: "workspace path",
                path: root.to_path_buf(),
                reason: e.to_string(),
            },
        })
    }

    /// The newest session whose transcript sits under the workspace's project key.
    pub fn latest_session(&self, workspace: &Path) -> Result<Option<SessionId>, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let dir = self.config_dir.join("projects").join(Self::key_of(&root)?);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(HarnessError::io("walk", &dir, e)),
        };
        let mut newest: Option<(std::time::SystemTime, SessionId)> = None;
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io("walk", &dir, e))?;
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
                .map_err(|e| HarnessError::io("read", &path, e))?;
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
            Err(e) => return Err(HarnessError::io("walk", &dir, e)),
        };
        let mut pids = Vec::new();
        for entry in entries {
            let path = entry.map_err(|e| HarnessError::io("walk", &dir, e))?.path();
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

    /// The installed version, or `None` when it cannot be learned; a
    /// failed probe is logged, never fatal.
    async fn installed_version(&self) -> Option<String> {
        match self.detect_impl().await {
            Ok(Some(installed)) => installed.version,
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, "claude: version probe failed");
                None
            }
        }
    }

    /// Sets `projects["<root>"].hasTrustDialogAccepted` in `~/.claude.json`
    /// while no `claude` process runs. Returns true when the file changed.
    pub async fn preseed_trust(&self, workspace: &Path) -> Result<bool, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let pids = self.running_claude().await?;
        if !pids.is_empty() {
            return Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            });
        }
        let config_json = self.config_json.clone();
        harness::blocking(move || trust::preseed_file(&config_json, &root)).await
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
            Err(e) => Err(HarnessError::io("read", path, e)),
        }
    }

    fn add_tree(&self, sources: &mut Vec<Source>, dir: &Path) -> Result<(), HarnessError> {
        match Self::kind_at(dir)? {
            Some(kind) if kind.is_symlink() => Err(HarnessError::SymlinkInState {
                path: dir.to_path_buf(),
            }),
            Some(kind) if kind.is_dir() => {
                for file in files::walk(dir)? {
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
        let rel_to_home = abs
            .strip_prefix(&self.home)
            .map_err(|_| HarnessError::NotUnderHome {
                path: abs.to_path_buf(),
                home: self.home.clone(),
            })?;
        let parts = files::rel_components(rel_to_home)?;
        let mut all: Vec<&str> = vec![HOME_PREFIX];
        all.extend(parts.iter().map(String::as_str));
        let rest: Vec<&str> = parts.iter().map(String::as_str).collect();
        sources.push(Source {
            rel: files::rel_path(&all)?,
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
    fn departure_path(&self, session: &SessionId) -> PathBuf {
        departure::record_path(&self.home, PROGRAM, session)
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
        let backups = history::Backups::read(&transcripts)?;
        for file in files::walk(&dir)? {
            let name = file.to_string_lossy();
            if backups.admit(&name, counts) {
                self.add_file(sources, &dir.join(&file))?;
            }
        }
        Ok(())
    }

    fn sources_for(
        &self,
        root: &Path,
        session: &SessionId,
        counts: &mut RedactionCounts,
    ) -> Result<Vec<Source>, HarnessError> {
        let mut sources = Vec::new();
        let projects = self.config_dir.join("projects");
        let mut found_transcript = false;
        let entries = match std::fs::read_dir(&projects) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(HarnessError::io("walk", &projects, e)),
        };
        if let Some(entries) = entries {
            let mut keys = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| HarnessError::io("walk", &projects, e))?;
                let path = entry.path();
                let kind = entry
                    .file_type()
                    .map_err(|e| HarnessError::io("walk", &path, e))?;
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
        self.add_file_history(&mut sources, session, counts)?;
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
        root: &Path,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let mut counts = RedactionCounts::new();
        let sources = self.sources_for(root, session, &mut counts)?;
        let carried = files::stage(&sources, out, &mut counts)?;
        Departure::of(&sources)?.write(&self.departure_path(session))?;
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind: HarnessKind::ClaudeCode,
            session: session.clone(),
            root: harness::abs_path(root)?,
            home: harness::abs_path(&self.home)?,
            harness_version: version,
            captured_at_ms: process::now_ms(),
            files: carried,
            redactions: counts
                .into_iter()
                .map(|(rule, count)| Redaction {
                    rule: rule.to_owned(),
                    count,
                })
                .collect(),
        };
        manifest.write(out)?;
        info!(%session, files = manifest.files.len(), "claude: captured");
        Ok(manifest)
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
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
        harness::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
    }

    fn restore_sync(
        &self,
        root: &Path,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let root_key = Self::key_of(root)?;
        let mut placements = Vec::with_capacity(manifest.files.len());
        for file in &manifest.files {
            let rel = file.rel_path.as_str();
            if let Some(rule) = denylist::check(rel) {
                return Err(HarnessError::Denied {
                    path: rel.to_owned(),
                    rule,
                });
            }
            let rest =
                under_home(rel).filter(|_| Self::restorable(rel, &manifest.session, &root_key));
            let Some(rest) = rest else {
                return Err(HarnessError::UnexpectedFile {
                    kind: HarnessKind::ClaudeCode,
                    path: rel.to_owned(),
                });
            };
            let parts: Vec<&str> = rest.split('/').collect();
            if file.mutability != Self::mutability_for(&parts) {
                return Err(HarnessError::Malformed {
                    what: "manifest",
                    path: state.join(rel),
                    reason: format!("{rel} is not {:?}", file.mutability),
                });
            }
            placements.push(Placement {
                staged: state.join(rel),
                dest: self.home.join(rest),
                content: file.content,
                mutability: file.mutability,
            });
        }
        files::verify(state, &manifest.files)?;
        let departure = Departure::read(&self.departure_path(&manifest.session))?;
        let placed = files::place_all(&placements, &departure)?;
        info!(session = %manifest.session, files = placed.len(), "claude: restored");
        Ok(Restored { manifest, placed })
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let manifest = StateManifest::read(state)?;
        if manifest.kind != HarnessKind::ClaudeCode {
            return Err(HarnessError::WrongHarness {
                captured: manifest.kind.clone(),
                restoring: HarnessKind::ClaudeCode,
            });
        }
        harness::same_place(&manifest, &root, &self.home)?;
        let version = self.installed_version().await;
        let pids = self
            .session_writers(&manifest.session, version.as_deref())
            .await?;
        if !pids.is_empty() {
            return Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            });
        }
        let adapter = self.clone();
        let state = state.to_path_buf();
        harness::blocking(move || adapter.restore_sync(&root, &state, manifest)).await
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        let path = match self.binary() {
            Ok(path) => path,
            Err(HarnessError::NotInstalled { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        let raw = process::probe(&path, &["--version"]).await?;
        let version =
            process::version_token(&raw).ok_or_else(|| HarnessError::UnexpectedVersionOutput {
                program: PROGRAM.to_owned(),
                output: raw.clone(),
            })?;
        Ok(Some(Installed {
            path,
            version: Some(version.to_owned()),
            raw,
        }))
    }

    /// The argument list for `launch`, without the binary. `session` is
    /// the id to start under for `Launch::New`.
    pub(crate) fn args_for(
        &self,
        session: &SessionId,
        launch: &Launch,
        io: Io,
    ) -> Result<Vec<String>, HarnessError> {
        let headless = matches!(io, Io::Piped);
        let mut args = Vec::new();
        if headless {
            args.push("-p".to_owned());
            args.push("--output-format".to_owned());
            args.push("json".to_owned());
        }
        let prompt = match launch {
            Launch::New { prompt } => {
                args.push("--session-id".to_owned());
                args.push(session.as_str().to_owned());
                Some(prompt.clone())
            }
            Launch::Resume { session, prompt } => {
                args.push("--resume".to_owned());
                args.push(session.as_str().to_owned());
                prompt.clone()
            }
        };
        args.extend(self.flags.iter().cloned());
        match prompt {
            Some(prompt) => {
                args.push("--".to_owned());
                args.push(prompt);
            }
            None if headless => {
                return Err(HarnessError::HeadlessNeedsPrompt {
                    program: PROGRAM.to_owned(),
                });
            }
            None => {}
        }
        Ok(args)
    }

    fn fresh_session() -> Result<SessionId, HarnessError> {
        let text = uuid::Uuid::new_v4().to_string();
        SessionId::parse(&text).map_err(|e| HarnessError::Malformed {
            what: "session id",
            path: PathBuf::from(text),
            reason: e.to_string(),
        })
    }

    fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        io: Io,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let session = match launch {
            Launch::New { .. } => Self::fresh_session()?,
            Launch::Resume { session, .. } => session.clone(),
        };
        let args = self.args_for(&session, launch, io)?;
        let (child, pid) = process::spawn(&binary, &args, &root, env, io.into())?;
        info!(%session, %pid, "claude: launched");
        Ok(Running::new(pid, Some(session), child))
    }
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
        io: Io,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(async move { self.launch_impl(workspace, &launch, io, env) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let adapter = ClaudeCode::new(PathBuf::from("/h")).with_flags(vec![
            "--permission-mode".to_owned(),
            "acceptEdits".to_owned(),
        ]);
        let session = SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap();
        let resume = Launch::Resume {
            session: session.clone(),
            prompt: None,
        };
        assert_eq!(
            adapter.args_for(&session, &resume, Io::Terminal).unwrap(),
            vec![
                "--resume",
                "9c9796f0-2682-4546-a963-d314b3ad543f",
                "--permission-mode",
                "acceptEdits"
            ]
        );
        assert!(matches!(
            adapter.args_for(&session, &resume, Io::Piped),
            Err(HarnessError::HeadlessNeedsPrompt { .. })
        ));
        let with_prompt = Launch::Resume {
            session: session.clone(),
            prompt: Some("--not-a-flag".to_owned()),
        };
        assert_eq!(
            adapter.args_for(&session, &with_prompt, Io::Piped).unwrap(),
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
        let fresh = ClaudeCode::fresh_session().unwrap();
        let new = adapter
            .args_for(
                &fresh,
                &Launch::New {
                    prompt: "hi".to_owned(),
                },
                Io::Terminal,
            )
            .unwrap();
        assert_eq!(
            new,
            vec![
                "--session-id",
                fresh.as_str(),
                "--permission-mode",
                "acceptEdits",
                "--",
                "hi"
            ]
        );
    }
}
