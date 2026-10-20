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
//! path is passed as a `-c` override on every run.
//!
//! The lock protocol follows `codex-rs/rollout/src/writer_lock.rs` at
//! `e4568a3`: `thread-writer-locks/.coordination.lock` is held only while
//! a `<thread_id>.lock` is opened and tried, and again while it is
//! removed; the thread lock is held for the whole copy.
//!
//! Surfaces used, checked against `codex --help`, `codex resume --help`
//! and `codex exec resume --help` of 0.155.1 on 2026-10-19: `codex resume
//! <id> [PROMPT]` for the terminal and `codex exec resume <id> [PROMPT]`
//! for a headless run, both with `-c key=value` overrides. The prompt is
//! passed after `--`, which clap reads as the end of options. The
//! app-server JSON-RPC surface is not used (ADR 0006, D14).

pub mod rollout;

use std::ffi::OsString;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use tracing::{info, warn};

use crate::denylist;
use crate::departure::Departure;
use crate::error::HarnessError;
use crate::files::{self, Carry, Placement, Relation, Source};
use crate::harness::{self, Harness, Installed, Io, Launch, Restored, Running};
use crate::manifest::{
    Content, HOME_PREFIX, MANIFEST_FORMAT, Mutability, Redaction, StateManifest,
};
use crate::process;
use crate::redact::RedactionCounts;
use crate::session::SessionId;

use self::rollout::{RolloutFile, RolloutName};

/// The program name on `PATH`.
pub const PROGRAM: &str = "codex";

/// The folder under `sessions/` where restored lineages are placed when
/// this node holds no file for the rollout yet.
pub const RESTORE_SUBDIR: &str = "fabric";

const LOCK_DIR: &str = "thread-writer-locks";
const COORDINATION_LOCK: &str = ".coordination.lock";

/// How long a lock attempt waits for Codex's coordination lock, which
/// Codex holds only across an open and a `try_lock`.
const COORDINATION_WAIT: Duration = Duration::from_secs(2);

/// The Codex adapter for one `CODEX_HOME` under one home directory.
#[derive(Debug, Clone)]
pub struct Codex {
    home: PathBuf,
    store: PathBuf,
    binary: Option<PathBuf>,
    path_var: Option<OsString>,
    overrides: Vec<String>,
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
        .map_err(|e| HarnessError::io("write", path, e))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                // This runs on a blocking thread (`harness::blocking`), so
                // sleeping here parks no runtime worker.
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::Error(e)) => return Err(HarnessError::io("lock", path, e)),
        }
    }
}

fn lock_coordination(dir: &Path) -> Result<File, HarnessError> {
    std::fs::create_dir_all(dir).map_err(|e| HarnessError::io("create dir", dir, e))?;
    let path = dir.join(COORDINATION_LOCK);
    try_lock_for(&path, COORDINATION_WAIT)?.ok_or_else(|| {
        HarnessError::io(
            "lock",
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
fn toml_quoted_key(root: &Path) -> Result<String, HarnessError> {
    let text = root.to_str().ok_or_else(|| HarnessError::Malformed {
        what: "workspace path",
        path: root.to_path_buf(),
        reason: "not UTF-8".to_owned(),
    })?;
    if text.chars().any(char::is_control) {
        return Err(HarnessError::Malformed {
            what: "workspace path",
            path: root.to_path_buf(),
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

impl Codex {
    /// An adapter whose state lives under `home/.codex`.
    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        Codex {
            store: home.join(".codex"),
            home,
            binary: None,
            path_var: std::env::var_os("PATH"),
            overrides: Vec::new(),
        }
    }

    /// Reads Codex's state from `dir` instead of `home/.codex`; refused
    /// when `dir` is not under the home.
    pub fn with_codex_home(mut self, dir: PathBuf) -> Result<Self, HarnessError> {
        if dir.strip_prefix(&self.home).is_err() {
            return Err(HarnessError::NotUnderHome {
                path: dir,
                home: self.home.clone(),
            });
        }
        self.store = dir;
        Ok(self)
    }

    /// Uses this binary instead of looking `codex` up on `PATH`.
    #[must_use]
    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    /// Looks `codex` up on this `PATH` value instead of the process's.
    #[must_use]
    pub fn with_path_var(mut self, path_var: OsString) -> Self {
        self.path_var = Some(path_var);
        self
    }

    /// `-c key=value` overrides passed on every run, such as
    /// `sandbox_mode="workspace-write"` or `model="o3"`; a cold resume
    /// does not restore them (H2, experiment E3).
    #[must_use]
    pub fn with_overrides(mut self, overrides: Vec<String>) -> Self {
        self.overrides = overrides;
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

    fn codex_home_parts(&self) -> Result<Vec<String>, HarnessError> {
        let rel = self
            .store
            .strip_prefix(&self.home)
            .map_err(|_| HarnessError::NotUnderHome {
                path: self.store.clone(),
                home: self.home.clone(),
            })?;
        files::rel_components(rel)
    }

    /// Every rollout in the store, with files that share a rollout id
    /// reduced to the longest when the shorter ones are byte prefixes of it.
    fn all_rollouts(&self, session: &SessionId) -> Result<Vec<RolloutFile>, HarnessError> {
        let mut all = rollout::list(&self.store.join("sessions"))?;
        all.extend(rollout::list(&self.store.join("archived_sessions"))?);
        let mut kept: Vec<RolloutFile> = Vec::with_capacity(all.len());
        for file in all {
            let Some(index) = kept
                .iter()
                .position(|k| k.name.rollout_id == file.name.rollout_id)
            else {
                kept.push(file);
                continue;
            };
            let existing = &kept[index];
            let keep_existing = match files::compare(&existing.path, &file.path)? {
                Relation::Missing | Relation::Equal => true,
                Relation::ExistingIsPrefix => false,
                Relation::Diverged => match files::compare(&file.path, &existing.path)? {
                    Relation::ExistingIsPrefix => true,
                    Relation::Missing | Relation::Equal | Relation::Diverged => {
                        return Err(HarnessError::LineageAmbiguous {
                            session: session.clone(),
                            rollout: file.name.rollout_id.clone(),
                            paths: vec![existing.path.clone(), file.path],
                        });
                    }
                },
            };
            if !keep_existing {
                kept[index] = file;
            }
        }
        Ok(kept)
    }

    /// The newest thread whose rollout records the workspace as its `cwd`.
    pub fn latest_session(&self, workspace: &Path) -> Result<Option<SessionId>, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let root_text = root.to_string_lossy();
        for file in rollout::list(&self.store.join("sessions"))? {
            let meta = match rollout::read_meta(&file) {
                Ok(meta) => meta,
                Err(e) => {
                    warn!(error = %e, "codex: rollout skipped");
                    continue;
                }
            };
            if meta.cwd.as_deref() == Some(root_text.as_ref()) {
                return Ok(Some(file.name.thread_id));
            }
        }
        Ok(None)
    }

    /// The rollout lineage of `session`, newest suffix first.
    pub fn lineage(&self, session: &SessionId) -> Result<Vec<RolloutFile>, HarnessError> {
        let all = self.all_rollouts(session)?;
        let not_found = || HarnessError::SessionNotFound {
            kind: HarnessKind::Codex,
            session: session.clone(),
            searched: self.store.join("sessions"),
        };
        let head = all
            .iter()
            .find(|f| f.name.thread_id == *session)
            .cloned()
            .ok_or_else(not_found)?;
        let mut chain = vec![head];
        loop {
            let last = chain.last().ok_or_else(not_found)?;
            let meta = rollout::read_meta(last)?;
            let Some(prefix_id) = meta.history_base else {
                break;
            };
            if chain.iter().any(|f| f.name.rollout_id == prefix_id) {
                break;
            }
            let Some(prefix) = all.iter().find(|f| f.name.rollout_id == prefix_id).cloned() else {
                return Err(HarnessError::LineageBroken {
                    session: session.clone(),
                    missing: prefix_id,
                });
            };
            chain.push(prefix);
        }
        Ok(chain)
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

    fn capture_sync(
        &self,
        root: &Path,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let home_parts = self.codex_home_parts()?;
        let lock = self.hold_writer_lock(session)?;
        let chain = self.lineage(session)?;
        let mut sources = Vec::with_capacity(chain.len());
        for file in &chain {
            rollout::refuse_secrets(file)?;
            let rel =
                file.path
                    .strip_prefix(&self.store)
                    .map_err(|_| HarnessError::NotUnderHome {
                        path: file.path.clone(),
                        home: self.store.clone(),
                    })?;
            let parts = files::rel_components(rel)?;
            let mut all: Vec<&str> = vec![HOME_PREFIX];
            all.extend(home_parts.iter().map(String::as_str));
            all.extend(parts.iter().map(String::as_str));
            sources.push(Source {
                abs: file.path.clone(),
                rel: files::rel_path(&all)?,
                carry: Carry::Opaque,
                mutability: Mutability::AppendOnly,
            });
        }
        let mut counts = RedactionCounts::new();
        let carried = files::stage(&sources, out, &mut counts)?;
        drop(lock);
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind: HarnessKind::Codex,
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
        info!(%session, rollouts = chain.len(), "codex: captured");
        Ok(manifest)
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let version = match self.detect_impl().await {
            Ok(Some(installed)) => installed.version,
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, "codex: version probe failed; manifest records none");
                None
            }
        };
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        harness::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
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

    fn restore_sync(
        &self,
        root: &Path,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        harness::same_place(&manifest, root, &self.home)?;
        let lock = self.hold_writer_lock(&manifest.session)?;
        let existing = self.all_rollouts(&manifest.session)?;
        let fallback_dir = self
            .store
            .join("sessions")
            .join(RESTORE_SUBDIR)
            .join(manifest.session.as_str());
        let mut placements = Vec::with_capacity(manifest.files.len());
        for file in &manifest.files {
            let rel = file.rel_path.as_str();
            if let Some(rule) = denylist::check(rel) {
                return Err(HarnessError::Denied {
                    path: rel.to_owned(),
                    rule,
                });
            }
            let restorable = self
                .restorable(rel)?
                .filter(|_| file.mutability == Mutability::AppendOnly);
            let Some((name_text, name)) = restorable else {
                return Err(HarnessError::UnexpectedFile {
                    kind: HarnessKind::Codex,
                    path: rel.to_owned(),
                });
            };
            let staged = state.join(rel);
            let same_id: Vec<&RolloutFile> = existing
                .iter()
                .filter(|f| f.name.rollout_id == name.rollout_id)
                .collect();
            let dest = match same_id.as_slice() {
                [] => fallback_dir.join(name_text),
                [found] if found.name.compressed == name.compressed => found.path.clone(),
                found => {
                    let mut paths: Vec<PathBuf> = found.iter().map(|f| f.path.clone()).collect();
                    paths.push(staged);
                    return Err(HarnessError::LineageAmbiguous {
                        session: manifest.session.clone(),
                        rollout: name.rollout_id,
                        paths,
                    });
                }
            };
            placements.push(Placement {
                staged,
                dest,
                content: Content::Opaque,
                mutability: Mutability::AppendOnly,
            });
        }
        files::verify(state, &manifest.files)?;
        // A rollout only grows, so nothing is ever replaced and no
        // departure record is kept.
        let placed = files::place_all(&placements, &Departure::default())?;
        drop(lock);
        info!(session = %manifest.session, files = placed.len(), "codex: restored");
        Ok(Restored { manifest, placed })
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let manifest = StateManifest::read(state)?;
        if manifest.kind != HarnessKind::Codex {
            return Err(HarnessError::WrongHarness {
                captured: manifest.kind.clone(),
                restoring: HarnessKind::Codex,
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

    /// The argument list for `launch`, without the binary.
    pub(crate) fn args_for(
        &self,
        root: &Path,
        launch: &Launch,
        io: Io,
    ) -> Result<Vec<String>, HarnessError> {
        let headless = matches!(io, Io::Piped);
        let mut args = Vec::new();
        if headless {
            args.push("exec".to_owned());
        }
        let prompt = match launch {
            Launch::New { prompt } => Some(prompt.clone()),
            Launch::Resume { session, prompt } => {
                args.push("resume".to_owned());
                args.push(session.as_str().to_owned());
                prompt.clone()
            }
        };
        if headless {
            args.push("--json".to_owned());
        }
        args.push("-c".to_owned());
        args.push(format!(
            "projects.{}.trust_level=trusted",
            toml_quoted_key(root)?
        ));
        for value in &self.overrides {
            args.push("-c".to_owned());
            args.push(value.clone());
        }
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

    fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        io: Io,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let args = self.args_for(&root, launch, io)?;
        let session = match launch {
            Launch::New { .. } => None,
            Launch::Resume { session, .. } => Some(session.clone()),
        };
        let (child, pid) = process::spawn(&binary, &args, &root, env, io.into())?;
        info!(?session, %pid, "codex: launched");
        Ok(Running::new(pid, session, child))
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
    fn args_follow_the_cli_surface_and_quote_the_root() {
        let adapter = Codex::new(PathBuf::from("/h"))
            .with_overrides(vec!["sandbox_mode=\"workspace-write\"".to_owned()]);
        let root = Path::new("/h/fabric/jobs/j/work");
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let resume = Launch::Resume {
            session: session.clone(),
            prompt: None,
        };
        assert_eq!(
            adapter.args_for(root, &resume, Io::Terminal).unwrap(),
            vec![
                "resume",
                "019f233e-bb33-7342-85fa-f2d33b14368a",
                "-c",
                "projects.\"/h/fabric/jobs/j/work\".trust_level=trusted",
                "-c",
                "sandbox_mode=\"workspace-write\""
            ]
        );
        assert!(matches!(
            adapter.args_for(root, &resume, Io::Piped),
            Err(HarnessError::HeadlessNeedsPrompt { .. })
        ));
        let headless = Launch::Resume {
            session,
            prompt: Some("--dangerously-bypass-approvals-and-sandbox".to_owned()),
        };
        assert_eq!(
            adapter.args_for(root, &headless, Io::Piped).unwrap(),
            vec![
                "exec",
                "resume",
                "019f233e-bb33-7342-85fa-f2d33b14368a",
                "--json",
                "-c",
                "projects.\"/h/fabric/jobs/j/work\".trust_level=trusted",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "--",
                "--dangerously-bypass-approvals-and-sandbox"
            ]
        );
        let quoted = Path::new("/h/say \"hi\"\\there");
        let args = adapter
            .args_for(
                quoted,
                &Launch::New {
                    prompt: "go".to_owned(),
                },
                Io::Terminal,
            )
            .unwrap();
        assert_eq!(
            args[1],
            "projects.\"/h/say \\\"hi\\\"\\\\there\".trust_level=trusted"
        );
        assert!(matches!(
            toml_quoted_key(Path::new("/h/new\nline")),
            Err(HarnessError::Malformed { .. })
        ));
    }
}
