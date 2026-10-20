//! The generic Agent Client Protocol adapter.
//!
//! Invariant: any agent that speaks ACP and advertises `session/load` or
//! `session/resume` can be continued in a new workspace by its session id
//! alone; Fabric carries, from the agent's declared state directory, only
//! the files whose path has a component that is the session id or the id
//! plus an extension, as opaque bytes it never
//! parses, and refuses a file that holds a secret-shaped string. An agent
//! whose state directory is not declared cannot be captured or restored,
//! and the adapter says so instead of guessing. Restore accepts only the
//! workspace path and home of the capture (ADR 0011) and only files under
//! the declared state directory whose path names the session. The adapter
//! cannot tell whether a generic agent is writing a session while it
//! restores one; the caller owns that guard, through the handle
//! `launch_acp` returned. A launch whose handshake fails kills the agent's
//! process group before it returns, so no agent runs without a handle.

pub mod client;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::{HarnessKind, ProgramName};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::denylist;
use crate::departure::{self, Departure};
use crate::error::HarnessError;
use crate::files::{self, Carry, Placement, Source};
use crate::harness::{self, Harness, Installed, Io, Launch, Restored, Running};
use crate::manifest::{
    Content, HOME_PREFIX, MANIFEST_FORMAT, Mutability, Redaction, StateManifest, under_home,
};
use crate::process::{self, Wiring};
use crate::redact::RedactionCounts;
use crate::session::SessionId;

use self::client::{AcpClient, AcpError, PermissionPolicy, StopReason};

/// True when a path component is the session's own: the id itself (a
/// directory) or the id followed by an extension (`<sid>.json`). Session
/// `s1` does not own `s10.json` or `xs1y`.
fn names_session(part: &str, session: &SessionId) -> bool {
    part.strip_prefix(session.as_str())
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// An ACP agent started as `program args...`.
#[derive(Debug, Clone)]
pub struct AcpHarness {
    program: ProgramName,
    args: Vec<String>,
    home: PathBuf,
    state_dir: Option<PathBuf>,
    policy: PermissionPolicy,
    path_var: Option<OsString>,
}

/// An ACP agent this node started, with the client that owns its pipes.
/// Dropping the client closes the agent's stdin.
pub struct AcpRunning {
    /// The process handle the supervisor records.
    pub running: Running,
    /// The protocol connection.
    pub client: AcpClient,
    /// The first prompt turn, when `launch_acp` was given a prompt; its
    /// result says how the turn ended.
    pub turn: Option<JoinHandle<Result<StopReason, AcpError>>>,
}

impl std::fmt::Debug for AcpRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpRunning")
            .field("running", &self.running)
            .field("turn_pending", &self.turn.is_some())
            .finish_non_exhaustive()
    }
}

impl AcpHarness {
    /// An adapter for `program args...` under `home`, with no state
    /// directory declared and the deny permission policy.
    #[must_use]
    pub fn new(program: ProgramName, args: Vec<String>, home: PathBuf) -> Self {
        AcpHarness {
            program,
            args,
            home,
            state_dir: None,
            policy: PermissionPolicy::Deny,
            path_var: std::env::var_os("PATH"),
        }
    }

    /// Declares where the agent keeps its sessions; refused when `dir` is
    /// not under the home.
    pub fn with_state_dir(mut self, dir: PathBuf) -> Result<Self, HarnessError> {
        if dir.strip_prefix(&self.home).is_err() {
            return Err(HarnessError::NotUnderHome {
                path: dir,
                home: self.home.clone(),
            });
        }
        self.state_dir = Some(dir);
        Ok(self)
    }

    /// Sets how `session/request_permission` is answered.
    #[must_use]
    pub fn with_policy(mut self, policy: PermissionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Looks the agent up on this `PATH` value instead of the process's.
    #[must_use]
    pub fn with_path_var(mut self, path_var: OsString) -> Self {
        self.path_var = Some(path_var);
        self
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        process::find_on_path(self.program.as_str(), self.path_var.as_deref()).ok_or_else(|| {
            HarnessError::NotInstalled {
                program: self.program.to_string(),
            }
        })
    }

    fn state_rel(&self) -> Result<(PathBuf, Vec<String>), HarnessError> {
        let dir = self
            .state_dir
            .clone()
            .ok_or_else(|| HarnessError::StateDirNotDeclared {
                program: self.program.to_string(),
            })?;
        let rel = dir
            .strip_prefix(&self.home)
            .map_err(|_| HarnessError::NotUnderHome {
                path: dir.clone(),
                home: self.home.clone(),
            })?;
        Ok((dir.clone(), files::rel_components(rel)?))
    }

    /// Where this node records the session's files at capture. An ACP
    /// agent's state is opaque, so every file is treated as rewritable.
    fn departure_path(&self, session: &SessionId) -> PathBuf {
        departure::record_path(&self.home, &format!("acp-{}", self.program), session)
    }

    fn capture_sync(
        &self,
        root: &Path,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let (dir, rel_parts) = self.state_rel()?;
        let mut sources = Vec::new();
        for file in files::walk(&dir)? {
            let file_parts = files::rel_components(&file)?;
            if !file_parts.iter().any(|part| names_session(part, session)) {
                continue;
            }
            let mut parts: Vec<&str> = vec![HOME_PREFIX];
            parts.extend(rel_parts.iter().map(String::as_str));
            parts.extend(file_parts.iter().map(String::as_str));
            sources.push(Source {
                abs: dir.join(&file),
                rel: files::rel_path(&parts)?,
                carry: Carry::OpaqueRefuseSecrets,
                mutability: Mutability::Replaceable,
            });
        }
        if sources.is_empty() {
            return Err(HarnessError::SessionNotFound {
                kind: self.kind(),
                session: session.clone(),
                searched: dir,
            });
        }
        let mut counts = RedactionCounts::new();
        let carried = files::stage(&sources, out, &mut counts)?;
        Departure::of(&sources)?.write(&self.departure_path(session))?;
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind: self.kind(),
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
        info!(%session, agent = %self.program, files = manifest.files.len(), "acp: captured");
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
                warn!(error = %e, agent = %self.program, "acp: version probe failed; manifest records none");
                None
            }
        };
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        harness::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
    }

    /// True when a tree path lies under the declared state directory and
    /// one of its components below it names the session, which is the rule
    /// capture selects files by.
    fn restorable(rel: &str, state_parts: &[String], session: &SessionId) -> bool {
        let Some(rest) = under_home(rel) else {
            return false;
        };
        let mut parts = rest.split('/');
        for expected in state_parts {
            if parts.next() != Some(expected.as_str()) {
                return false;
            }
        }
        parts.any(|part| names_session(part, session))
    }

    fn restore_sync(
        &self,
        root: &Path,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        harness::same_place(&manifest, root, &self.home)?;
        let (_, state_parts) = self.state_rel()?;
        let mut placements = Vec::with_capacity(manifest.files.len());
        for file in &manifest.files {
            let rel = file.rel_path.as_str();
            if let Some(rule) = denylist::check(rel) {
                return Err(HarnessError::Denied {
                    path: rel.to_owned(),
                    rule,
                });
            }
            let rest = under_home(rel)
                .filter(|_| Self::restorable(rel, &state_parts, &manifest.session))
                .filter(|_| file.mutability == Mutability::Replaceable);
            let Some(rest) = rest else {
                return Err(HarnessError::UnexpectedFile {
                    kind: self.kind(),
                    path: rel.to_owned(),
                });
            };
            placements.push(Placement {
                staged: state.join(rel),
                dest: self.home.join(rest),
                content: Content::Opaque,
                mutability: Mutability::Replaceable,
            });
        }
        files::verify(state, &manifest.files)?;
        let departure = Departure::read(&self.departure_path(&manifest.session))?;
        let placed = files::place_all(&placements, &departure)?;
        info!(session = %manifest.session, agent = %self.program, files = placed.len(), "acp: restored");
        Ok(Restored { manifest, placed })
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let manifest = StateManifest::read(state)?;
        if manifest.kind != self.kind() {
            return Err(HarnessError::WrongHarness {
                captured: manifest.kind.clone(),
                restoring: self.kind(),
            });
        }
        let adapter = self.clone();
        let state = state.to_path_buf();
        harness::blocking(move || adapter.restore_sync(&root, &state, manifest)).await
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        let Some(path) = process::find_on_path(self.program.as_str(), self.path_var.as_deref())
        else {
            return Ok(None);
        };
        let raw = process::probe(&path, &["--version"]).await?;
        Ok(Some(Installed {
            version: process::version_token(&raw).map(str::to_owned),
            raw,
            path,
        }))
    }

    /// Starts the agent, completes the handshake, continues or creates the
    /// session and returns the process with its protocol client. A prompt
    /// is sent on a task of its own, whose handle is on the result.
    pub async fn launch_acp(
        &self,
        workspace: &Path,
        launch: Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<AcpRunning, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let (mut child, pid) = process::spawn(&binary, &self.args, &root, env, Wiring::Rpc)?;
        // Until the session is open, an error or a dropped future kills the
        // agent's group, so no agent is left running without a handle.
        let guard = process::GroupGuard::new(pid);
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(HarnessError::Spawn {
                program: self.program.to_string(),
                source: std::io::Error::other("agent started without stdio pipes"),
            });
        };
        let client = AcpClient::attach(stdin, stdout, self.policy);
        let (session, prompt) = self.open_session(&client, &root, launch).await?;
        guard.disarm();
        let turn = prompt.map(|prompt| {
            let client = client.clone();
            let session = session.clone();
            tokio::spawn(async move { client.prompt(&session, &prompt).await })
        });
        Ok(AcpRunning {
            running: Running::new(pid, Some(session), child),
            client,
            turn,
        })
    }

    /// Runs the handshake and opens the session `launch` names; returns
    /// the session and the prompt still to send.
    async fn open_session(
        &self,
        client: &AcpClient,
        root: &Path,
        launch: Launch,
    ) -> Result<(SessionId, Option<String>), HarnessError> {
        let capabilities = client.initialize().await?;
        let cwd = root.to_string_lossy().into_owned();
        match launch {
            Launch::New { prompt } => {
                let session = client.new_session(&cwd).await?;
                info!(%session, agent = %self.program, "acp: new session");
                Ok((session, Some(prompt)))
            }
            Launch::Resume { session, prompt } => {
                if capabilities.resume {
                    client.resume_session(&session, &cwd).await?;
                } else if capabilities.load_session {
                    client.load_session(&session, &cwd).await?;
                } else {
                    return Err(HarnessError::LoadNotSupported {
                        program: self.program.to_string(),
                    });
                }
                info!(%session, agent = %self.program, "acp: session continued");
                Ok((session, prompt))
            }
        }
    }

    /// The trait's headless launch: the prompt turn runs on a task that
    /// owns the client and closes the agent's stdin when the turn ends.
    async fn launch_impl(
        &self,
        workspace: &Path,
        launch: Launch,
        io: Io,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        if matches!(io, Io::Terminal) {
            return Err(HarnessError::NeedsClient {
                program: self.program.to_string(),
            });
        }
        if matches!(launch, Launch::Resume { prompt: None, .. }) {
            return Err(HarnessError::HeadlessNeedsPrompt {
                program: self.program.to_string(),
            });
        }
        let AcpRunning {
            running,
            client,
            turn,
        } = self.launch_acp(workspace, launch, env).await?;
        let program = self.program.to_string();
        tokio::spawn(async move {
            let outcome = match turn {
                Some(turn) => turn.await,
                None => return,
            };
            match outcome {
                Ok(Ok(reason)) => info!(agent = %program, ?reason, "acp: turn ended"),
                Ok(Err(e)) => warn!(agent = %program, error = %e, "acp: turn failed"),
                Err(e) => warn!(agent = %program, error = %e, "acp: turn task ended early"),
            }
            drop(client);
        });
        Ok(running)
    }
}

impl Harness for AcpHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Acp {
            program: self.program.clone(),
            args: self.args.clone(),
        }
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
        Box::pin(self.launch_impl(workspace, launch, io, env))
    }
}
