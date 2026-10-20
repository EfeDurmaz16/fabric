//! The `Harness` trait and the values that cross it.
//!
//! Invariant: the core drives every harness through this trait alone. It
//! learns whether the binary is installed, hands over a workspace and a
//! session, and gets back a manifest, a restore report or a running process
//! handle. No vendor type crosses the boundary.

use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use tokio::process::Child;

use crate::error::HarnessError;
use crate::manifest::StateManifest;
use crate::session::SessionId;

/// An installed harness binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The binary that was found.
    pub path: PathBuf,
    /// The version it reported, such as `2.1.294`; `None` when the binary
    /// answered `--version` with no version in it.
    pub version: Option<String>,
    /// The whole first line of its version output.
    pub raw: String,
}

/// How to start a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Start a new session with an opening prompt.
    New {
        /// The first user message.
        prompt: String,
    },
    /// Continue a recorded session.
    Resume {
        /// The session to continue.
        session: SessionId,
        /// A first message to send on resume; required for a headless run.
        prompt: Option<String>,
    },
}

/// How the started process is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Io {
    /// The user's terminal: stdin, stdout and stderr are inherited.
    Terminal,
    /// Headless: no stdin, stdout and stderr captured on the handle.
    Piped,
    /// A protocol client owns stdin and stdout; stderr is inherited.
    Rpc,
}

/// A harness process this node started.
pub struct Running {
    /// The process id.
    pub pid: u32,
    /// The process group id; equal to `pid` because the group is new.
    pub pgid: u32,
    /// Unix milliseconds when it was spawned; informational.
    pub spawned_at_ms: u64,
    /// The session it continues, when known at launch.
    pub session: Option<SessionId>,
    /// The child process; its pipes are set when `Io::Piped` was asked for.
    pub child: Child,
    /// The protocol client, when the harness was started under `Io::Rpc`.
    #[cfg(feature = "acp")]
    pub acp: Option<crate::acp::client::AcpClient>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("pid", &self.pid)
            .field("pgid", &self.pgid)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Running {
    /// Wraps a freshly spawned child.
    #[must_use]
    pub fn new(child: Child, session: Option<SessionId>) -> Self {
        let pid = child.id().unwrap_or(0);
        Running {
            pid,
            pgid: pid,
            spawned_at_ms: crate::process::now_ms(),
            session,
            child,
            #[cfg(feature = "acp")]
            acp: None,
        }
    }
}

/// What a restore placed on this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The manifest that was restored.
    pub manifest: StateManifest,
    /// Every file that was placed, in order.
    pub placed: Vec<PathBuf>,
}

/// An agent that can be captured, restored and started (ADR 0006).
pub trait Harness: Send + Sync {
    /// Which harness this is.
    fn kind(&self) -> HarnessKind;

    /// Finds the binary and its version; `None` when it is not installed.
    fn detect(&self) -> BoxFuture<'_, Result<Option<Installed>, HarnessError>>;

    /// Copies everything `session` needs to continue into `out`, redacted,
    /// and writes the manifest there. The harness must not be running.
    fn capture<'a>(
        &'a self,
        workspace: &'a Path,
        session: &'a SessionId,
        out: &'a Path,
    ) -> BoxFuture<'a, Result<StateManifest, HarnessError>>;

    /// Places a captured state tree where this harness expects it on this
    /// node, mapping paths when the workspace or home differ from capture.
    fn restore<'a>(
        &'a self,
        workspace: &'a Path,
        state: &'a Path,
    ) -> BoxFuture<'a, Result<Restored, HarnessError>>;

    /// Starts the harness in `workspace` with exactly `env`.
    fn launch<'a>(
        &'a self,
        workspace: &'a Path,
        launch: Launch,
        io: Io,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>>;
}

/// Checks that `workspace` is an absolute directory and returns its physical path.
pub(crate) fn physical_workspace(workspace: &Path) -> Result<PathBuf, HarnessError> {
    if !workspace.is_absolute() {
        return Err(HarnessError::BadWorkspace {
            path: workspace.to_path_buf(),
        });
    }
    let physical = std::fs::canonicalize(workspace)
        .map_err(|e| HarnessError::io("canonicalize", workspace, e))?;
    if !physical.is_dir() {
        return Err(HarnessError::BadWorkspace {
            path: workspace.to_path_buf(),
        });
    }
    Ok(physical)
}

/// Converts a physical path into the `AbsPath` a manifest records.
pub(crate) fn abs_path(path: &Path) -> Result<AbsPath, HarnessError> {
    let text = path.to_str().ok_or_else(|| HarnessError::Malformed {
        what: "absolute path",
        path: path.to_path_buf(),
        reason: "not UTF-8".to_owned(),
    })?;
    AbsPath::parse(text).map_err(|e| HarnessError::Malformed {
        what: "absolute path",
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// Refuses a restore whose manifest was captured under another workspace
/// path or home: under ADR 0011 the target root is the captured root, and
/// the harness keys its state to those exact strings.
pub(crate) fn same_place(
    manifest: &StateManifest,
    root: &Path,
    home: &Path,
) -> Result<(), HarnessError> {
    let root = abs_path(root)?;
    if manifest.root != root {
        return Err(HarnessError::RootMismatch {
            from: manifest.root.clone(),
            to: root,
        });
    }
    let home = abs_path(home)?;
    if manifest.home != home {
        return Err(HarnessError::HomeMismatch {
            from: manifest.home.clone(),
            to: home,
        });
    }
    Ok(())
}
