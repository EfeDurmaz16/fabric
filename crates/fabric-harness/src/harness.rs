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
    /// Its whole `--version` output, trimmed.
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

/// How to start a session, and where its input and output go. A headless
/// run reads its first message from the command line, so it cannot be
/// built without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// In the user's terminal: stdin, stdout and stderr are inherited.
    Interactive {
        /// The session to continue; a new one when `None`.
        resume: Option<SessionId>,
        /// A first message to send, when there is one.
        prompt: Option<String>,
    },
    /// Without a terminal: no stdin, stdout and stderr captured on the
    /// handle. An ACP agent speaks its protocol on these pipes, so for it
    /// the protocol client owns them and they are not on the handle.
    Headless {
        /// The session to continue; a new one when `None`.
        resume: Option<SessionId>,
        /// The first message.
        prompt: String,
    },
}

impl Launch {
    /// The session to continue, if any.
    #[must_use]
    pub fn resume(&self) -> Option<&SessionId> {
        match self {
            Launch::Interactive { resume, .. } | Launch::Headless { resume, .. } => resume.as_ref(),
        }
    }

    /// The first message, if any.
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Launch::Interactive { prompt, .. } => prompt.as_deref(),
            Launch::Headless { prompt, .. } => Some(prompt),
        }
    }

    /// True for a run without a terminal.
    #[must_use]
    pub fn is_headless(&self) -> bool {
        matches!(self, Launch::Headless { .. })
    }
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
    #[cfg(any(feature = "acp", feature = "claude", feature = "codex"))]
    pub(crate) fn new(pid: NonZeroU32, session: Option<SessionId>, child: Child) -> Self {
        Running {
            pid,
            session,
            child,
        }
    }

    /// The process id, which is also the id of the process group the
    /// process leads.
    #[must_use]
    pub fn pid(&self) -> NonZeroU32 {
        self.pid
    }

    /// The session the process continues, when known at launch.
    #[must_use]
    pub fn session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// The child process. Its stdout and stderr are set for a
    /// `Launch::Headless` run, except for an ACP agent, whose pipes the
    /// protocol client holds (`AcpHarness::launch_acp` returns that client).
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
///
/// `capture` and `restore` copy files on a blocking thread. Dropping their
/// future (a timeout, a Ctrl-C) does not stop that copy, so a caller must
/// await them to completion and must not `launch` the same session until
/// they have returned: a restore still renaming files over a transcript
/// would race the harness that appends to it.
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

    /// Records that the capture which produced `manifest` has left this
    /// node. The core calls it once the snapshot that holds the capture is
    /// published. Until then restore treats this node's rewritable files
    /// as never having left, so a failed publish followed by a restore of
    /// an older snapshot cannot overwrite them. Confirming twice is a no-op.
    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>>;

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
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>>;
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
