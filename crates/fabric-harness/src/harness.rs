//! The `Harness` trait and the values that cross it.
//!
//! Invariant: the core drives every harness through this trait alone. It
//! learns whether the binary is installed, hands over a workspace and a
//! session, and gets back a manifest, a restore report or a running process
//! handle. No vendor type crosses the boundary, and no value of these types
//! can describe a process that does not exist.

use std::num::NonZeroU32;
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

/// The most characters a model name may have.
pub const MAX_MODEL_NAME_LEN: usize = 128;

/// Why a model name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelNameError {
    /// The name is empty or longer than `MAX_MODEL_NAME_LEN`.
    #[error("model name must have 1 to {MAX_MODEL_NAME_LEN} characters")]
    Length,
    /// A character outside `[A-Za-z0-9._:/@[]-]`, or a leading `-`.
    #[error("model name has an invalid character at index {index}")]
    BadChar {
        /// Zero-based index of the offending character.
        index: usize,
    },
}

/// A model name passed to a harness, such as `opus` or `o3`: safe on a
/// command line and inside a quoted TOML string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelName(String);

impl ModelName {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, ModelNameError> {
        if text.is_empty() || text.chars().count() > MAX_MODEL_NAME_LEN {
            return Err(ModelNameError::Length);
        }
        let bad = text.char_indices().find(|(i, c)| {
            !(c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | ':' | '/' | '@' | '[' | ']' | '-'))
                || (*i == 0 && *c == '-')
        });
        if let Some((index, _)) = bad {
            return Err(ModelNameError::BadChar { index });
        }
        Ok(ModelName(text.to_owned()))
    }

    /// The name as given.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
}

/// A harness process this node started, in a process group of its own.
/// Only this crate builds one, from a child it spawned, so the pid is
/// real and is also the process group id.
pub struct Running {
    pid: NonZeroU32,
    session: Option<SessionId>,
    child: Child,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("pid", &self.pid)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Running {
    /// Wraps a child that `process::spawn` started in a new process group.
    #[cfg_attr(
        not(any(feature = "acp", feature = "claude", feature = "codex")),
        allow(dead_code, reason = "used by the adapters; none is enabled")
    )]
    pub(crate) fn new(pid: NonZeroU32, session: Option<SessionId>, child: Child) -> Self {
        Running {
            pid,
            session,
            child,
        }
    }

    /// The process id.
    #[must_use]
    pub fn pid(&self) -> NonZeroU32 {
        self.pid
    }

    /// The process group id, equal to the pid because the group was created for it.
    #[must_use]
    pub fn pgid(&self) -> NonZeroU32 {
        self.pid
    }

    /// The session the process continues, when known at launch.
    #[must_use]
    pub fn session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// The child process; its pipes are set when `Io::Piped` was asked for.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// What a restore placed on this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The manifest that was restored.
    pub manifest: StateManifest,
    /// Every file that was placed or found already equal, in order.
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
    /// node. Refuses a workspace or home other than the ones captured
    /// (ADR 0011), any file outside the places the adapter itself
    /// captures, and a restore while the harness writes the same session.
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
#[cfg_attr(
    not(any(feature = "acp", feature = "claude", feature = "codex")),
    allow(dead_code, reason = "used by the adapters; none is enabled")
)]
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
#[cfg_attr(
    not(any(feature = "acp", feature = "claude", feature = "codex")),
    allow(dead_code, reason = "used by the adapters; none is enabled")
)]
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
#[cfg_attr(
    not(any(feature = "acp", feature = "claude", feature = "codex")),
    allow(dead_code, reason = "used by the adapters; none is enabled")
)]
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

/// Refuses an overlay file (settings, MCP configuration) that holds a
/// literal secret: ADR 0006 allows only `${VAR}` references there.
#[cfg_attr(
    not(any(feature = "claude", feature = "codex")),
    allow(dead_code, reason = "used by the Claude adapter")
)]
pub(crate) fn refuse_secret_overlay(path: &Path) -> Result<(), HarnessError> {
    let bytes = std::fs::read(path).map_err(|e| HarnessError::io("read", path, e))?;
    if crate::redact::Redactor::standard()?.matches_bytes(&bytes) {
        return Err(HarnessError::SecretInOverlay {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Runs a blocking capture or restore body off the async runtime.
#[cfg_attr(
    not(any(feature = "acp", feature = "claude", feature = "codex")),
    allow(dead_code, reason = "used by the adapters; none is enabled")
)]
pub(crate) async fn blocking<T, F>(body: F) -> Result<T, HarnessError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HarnessError> + Send + 'static,
{
    tokio::task::spawn_blocking(body)
        .await
        .map_err(|source| HarnessError::BlockingTask { source })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_plain_tokens() {
        for good in [
            "opus",
            "o3",
            "claude-opus-4-1[1m]",
            "gpt-5.1-codex",
            "us.anthropic.x:0",
        ] {
            assert!(ModelName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-p", "a b", "a\"b", "a\\b", "a=b", "x\n"] {
            assert!(ModelName::parse(bad).is_err(), "{bad:?}");
        }
    }
}
