//! The Codex adapter.
//!
//! Invariant: a Codex thread is carried as its rollout lineage, byte for
//! byte, under Codex's own writer lock for the whole copy, and restored
//! under `$CODEX_HOME/sessions/fabric/<thread_id>/`, a folder Codex's
//! file-name scan accepts. Nothing is rewritten: Codex retargets the
//! thread to the directory it is resumed from (H2, experiments E2, E3b),
//! and a paginated prefix is referenced by byte offset. `auth.json`, the
//! `SQLite` files, the locks and `config.toml` never travel. `CODEX_HOME` is
//! never set per job because the keyring entry is derived from it (ADR
//! 0006, D15); trust for the new path is passed as a `-c` override on
//! every run.
//!
//! Surfaces used, checked against `codex --help`, `codex resume --help`
//! and `codex exec resume --help` of 0.155.1 on 2026-10-19: `codex resume
//! <id> [PROMPT]` for the terminal and `codex exec resume <id> [PROMPT]`
//! for a headless run, both with `-c key=value` overrides. The app-server
//! JSON-RPC surface is not used (ADR 0006, D14).

pub mod rollout;

use std::ffi::OsString;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use tracing::{info, warn};

use crate::error::HarnessError;
use crate::files::{self, Source};
use crate::harness::{self, Harness, Installed, Io, Launch, Restored, Running};
use crate::manifest::{HOME_PREFIX, MANIFEST_FORMAT, Redaction, Rewrite, StateManifest};
use crate::process;
use crate::rewrite::RedactionCounts;
use crate::session::SessionId;

use self::rollout::RolloutFile;

/// The program name on `PATH`.
pub const PROGRAM: &str = "codex";

/// The folder under `sessions/` where restored lineages are placed.
pub const RESTORE_SUBDIR: &str = "fabric";

/// How long capture waits for Codex's coordination lock.
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

/// Holds Codex's writer lock for a thread while it is copied.
struct WriterLock {
    _coordination: File,
    _thread: File,
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

    /// Reads Codex's state from `dir` instead of `home/.codex`; it must be under home.
    #[must_use]
    pub fn with_codex_home(mut self, dir: PathBuf) -> Self {
        self.store = dir;
        self
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

    /// Where this adapter reads Codex's state.
    #[must_use]
    pub fn codex_home(&self) -> &Path {
        &self.store
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
            .map_err(|_| HarnessError::Malformed {
                what: "codex home",
                path: self.store.clone(),
                reason: format!("not under home {}", self.home.display()),
            })?;
        files::rel_components(rel)
    }

    fn all_rollouts(&self) -> Result<Vec<RolloutFile>, HarnessError> {
        let mut all = rollout::list(&self.store.join("sessions"))?;
        all.extend(rollout::list(&self.store.join("archived_sessions"))?);
        Ok(all)
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
                return Ok(SessionId::parse(&file.thread_id).ok());
            }
        }
        Ok(None)
    }

    /// The rollout lineage of `session`, newest suffix first.
    pub fn lineage(&self, session: &SessionId) -> Result<Vec<RolloutFile>, HarnessError> {
        let all = self.all_rollouts()?;
        let Some(head) = all
            .iter()
            .find(|f| f.thread_id == session.as_str())
            .cloned()
        else {
            return Err(HarnessError::SessionNotFound {
                kind: HarnessKind::Codex,
                session: session.clone(),
                searched: self.store.join("sessions"),
            });
        };
        let mut chain = vec![head];
        loop {
            let last = chain.last().ok_or_else(|| HarnessError::SessionNotFound {
                kind: HarnessKind::Codex,
                session: session.clone(),
                searched: self.store.join("sessions"),
            })?;
            let meta = rollout::read_meta(last)?;
            let Some(prefix_id) = meta.history_base else {
                break;
            };
            if chain.iter().any(|f| f.rollout_id == prefix_id) {
                break;
            }
            let Some(prefix) = all.iter().find(|f| f.rollout_id == prefix_id).cloned() else {
                return Err(HarnessError::LineageBroken {
                    session: session.clone(),
                    missing: prefix_id,
                });
            };
            chain.push(prefix);
        }
        Ok(chain)
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
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(TryLockError::Error(e)) => return Err(HarnessError::io("lock", path, e)),
            }
        }
    }

    fn hold_writer_lock(&self, session: &SessionId) -> Result<WriterLock, HarnessError> {
        let dir = self.store.join("thread-writer-locks");
        std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io("create dir", &dir, e))?;
        let busy = || HarnessError::ActiveWriter {
            kind: HarnessKind::Codex,
            session: session.clone(),
        };
        let coordination =
            Self::try_lock_for(&self.store.join(".coordination.lock"), COORDINATION_WAIT)?
                .ok_or_else(busy)?;
        let thread = Self::try_lock_for(&dir.join(format!("{session}.lock")), Duration::ZERO)?
            .ok_or_else(busy)?;
        Ok(WriterLock {
            _coordination: coordination,
            _thread: thread,
        })
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let home_parts = self.codex_home_parts()?;
        let lock = self.hold_writer_lock(session)?;
        let chain = self.lineage(session)?;
        let mut sources = Vec::with_capacity(chain.len());
        for file in &chain {
            let rel = file
                .path
                .strip_prefix(&self.store)
                .map_err(|_| HarnessError::Malformed {
                    what: "rollout path",
                    path: file.path.clone(),
                    reason: "not under codex home".to_owned(),
                })?;
            let parts = files::rel_components(rel)?;
            let mut all: Vec<&str> = vec![HOME_PREFIX];
            all.extend(home_parts.iter().map(String::as_str));
            all.extend(parts.iter().map(String::as_str));
            sources.push(Source {
                abs: file.path.clone(),
                rel: files::rel_path(&all)?,
                rewrite: Rewrite::None,
            });
        }
        let mut counts = RedactionCounts::new();
        let carried = files::stage(&sources, out, &mut counts)?;
        drop(lock);
        let version = match self.detect_impl().await {
            Ok(Some(installed)) => installed.version,
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, "codex: version probe failed; manifest records none");
                None
            }
        };
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind: HarnessKind::Codex,
            session: session.clone(),
            root: harness::abs_path(&root)?,
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

    fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        harness::physical_workspace(workspace)?;
        let manifest = StateManifest::read(state)?;
        if manifest.kind != HarnessKind::Codex {
            return Err(HarnessError::WrongHarness {
                captured: manifest.kind.clone(),
                restoring: HarnessKind::Codex,
            });
        }
        let target_dir = self
            .store
            .join("sessions")
            .join(RESTORE_SUBDIR)
            .join(manifest.session.as_str());
        let mut placed = Vec::new();
        for file in &manifest.files {
            let src = state.join(file.rel_path.as_str());
            let Some(name) = src.file_name() else {
                continue;
            };
            let bytes = std::fs::read(&src).map_err(|e| HarnessError::io("read", &src, e))?;
            let dest = target_dir.join(name);
            files::place(&dest, &bytes)?;
            placed.push(dest);
        }
        info!(session = %manifest.session, files = placed.len(), "codex: restored");
        Ok(Restored {
            manifest,
            placed,
            rules: Vec::new(),
        })
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
    pub fn args_for(
        &self,
        root: &Path,
        launch: &Launch,
        io: Io,
    ) -> Result<Vec<String>, HarnessError> {
        let headless = !matches!(io, Io::Terminal);
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
            "projects.\"{}\".trust_level=trusted",
            root.to_string_lossy()
        ));
        for value in &self.overrides {
            args.push("-c".to_owned());
            args.push(value.clone());
        }
        match prompt {
            Some(prompt) => args.push(prompt),
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
        let child = process::spawn(&binary, &args, &root, env, io)?;
        info!(?session, pid = child.id(), "codex: launched");
        Ok(Running::new(child, session))
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
        Box::pin(async move { self.restore_impl(workspace, state) })
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
    fn args_follow_the_cli_surface() {
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
            prompt: Some("reply with the secret word".to_owned()),
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
                "reply with the secret word"
            ]
        );
    }
}
