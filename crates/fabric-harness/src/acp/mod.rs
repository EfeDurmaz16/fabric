//! The generic Agent Client Protocol adapter.
//!
//! Invariant: any agent that speaks ACP and advertises `session/load` or
//! `session/resume` can be continued in a new workspace by its session id
//! alone; Fabric carries the agent's state directory as opaque bytes and
//! never parses it. An agent whose state directory is not declared cannot
//! be captured, and the adapter says so instead of guessing.

pub mod client;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::{HarnessKind, ProgramName};
use tracing::info;

use crate::error::HarnessError;
use crate::files::{self, Source};
use crate::harness::{self, Harness, Installed, Io, Launch, Restored, Running};
use crate::manifest::{HOME_PREFIX, MANIFEST_FORMAT, Redaction, Rewrite, StateManifest};
use crate::process;
use crate::redact::RedactionCounts;
use crate::session::SessionId;

use self::client::{AcpClient, PermissionPolicy};

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

    /// Declares where the agent keeps its sessions; must be under `home`.
    #[must_use]
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        self.state_dir = Some(dir);
        self
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
            .ok_or_else(|| HarnessError::Malformed {
                what: "acp state directory",
                path: PathBuf::new(),
                reason: format!("no state directory declared for {}", self.program),
            })?;
        let rel = dir
            .strip_prefix(&self.home)
            .map_err(|_| HarnessError::Malformed {
                what: "acp state directory",
                path: dir.clone(),
                reason: format!("not under home {}", self.home.display()),
            })?;
        Ok((dir.clone(), files::rel_components(rel)?))
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let (dir, rel_parts) = self.state_rel()?;
        let mut sources = Vec::new();
        for file in files::walk(&dir)? {
            let mut parts: Vec<&str> = vec![HOME_PREFIX];
            parts.extend(rel_parts.iter().map(String::as_str));
            let file_parts = files::rel_components(&file)?;
            parts.extend(file_parts.iter().map(String::as_str));
            sources.push(Source {
                abs: dir.join(&file),
                rel: files::rel_path(&parts)?,
                rewrite: Rewrite::None,
            });
        }
        let mut counts = RedactionCounts::new();
        let carried = files::stage(&sources, out, &mut counts)?;
        let version = match self.detect_impl().await {
            Ok(Some(installed)) => installed.version,
            _ => None,
        };
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind: self.kind(),
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
        Ok(manifest)
    }

    fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let manifest = StateManifest::read(state)?;
        if manifest.kind != self.kind() {
            return Err(HarnessError::WrongHarness {
                captured: manifest.kind.clone(),
                restoring: self.kind(),
            });
        }
        harness::same_place(&manifest, &root, &self.home)?;
        let mut placed = Vec::new();
        for file in &manifest.files {
            let Some(rest) = file.rel_path.as_str().strip_prefix("home/") else {
                continue;
            };
            let src = state.join(file.rel_path.as_str());
            let bytes = std::fs::read(&src).map_err(|e| HarnessError::io("read", &src, e))?;
            let dest = self.home.join(rest);
            files::place(&dest, &bytes)?;
            placed.push(dest);
        }
        Ok(Restored { manifest, placed })
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        let Some(path) = process::find_on_path(self.program.as_str(), self.path_var.as_deref())
        else {
            return Ok(None);
        };
        let raw = process::probe(&path, &["--version"])
            .await
            .unwrap_or_default();
        Ok(Some(Installed {
            version: process::version_token(&raw).map(str::to_owned),
            raw,
            path,
        }))
    }

    async fn launch_impl(
        &self,
        workspace: &Path,
        launch: Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let mut child = process::spawn(&binary, &self.args, &root, env, Io::Rpc)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(HarnessError::Spawn {
                program: self.program.to_string(),
                source: std::io::Error::other("agent started without stdio pipes"),
            });
        };
        let client = AcpClient::attach(stdin, stdout, self.policy);
        let capabilities = client.initialize().await?;
        let cwd = root.to_string_lossy().into_owned();
        let session = match launch {
            Launch::New { prompt } => {
                let session = client.new_session(&cwd).await?;
                info!(%session, agent = %self.program, "acp: new session");
                spawn_turn(&client, &session, prompt);
                session
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
                if let Some(prompt) = prompt {
                    spawn_turn(&client, &session, prompt);
                }
                session
            }
        };
        let mut running = Running::new(child, Some(session));
        running.acp = Some(client);
        Ok(running)
    }
}

/// Sends `prompt` on a task of its own so `launch` returns while the turn runs.
fn spawn_turn(client: &AcpClient, session: &SessionId, prompt: String) {
    let client = client.clone();
    let session = session.clone();
    tokio::spawn(async move {
        if let Err(e) = client.prompt(&session, &prompt).await {
            tracing::warn!(error = %e, "acp: prompt turn failed");
        }
    });
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
        Box::pin(async move { self.restore_impl(workspace, state) })
    }

    fn launch<'a>(
        &'a self,
        workspace: &'a Path,
        launch: Launch,
        _io: Io,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(self.launch_impl(workspace, launch, env))
    }
}
