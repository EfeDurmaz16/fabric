//! The kinds of harness and the shape of their carried state.
//!
//! Invariant: the core never looks inside harness state. It knows which kind
//! of adapter owns it and the list of relative paths and hashes that make it
//! up, nothing more.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::snapshot::Entry;

/// Which agent drives a job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HarnessKind {
    /// Claude Code through its CLI.
    ClaudeCode,
    /// Codex through `codex exec resume`.
    Codex,
    /// Any agent that speaks the Agent Client Protocol, started by `command`.
    Acp {
        /// The command that starts the agent.
        command: String,
    },
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HarnessKind::ClaudeCode => f.write_str("claude-code"),
            HarnessKind::Codex => f.write_str("codex"),
            HarnessKind::Acp { command } => write!(f, "acp:{command}"),
        }
    }
}

/// What an adapter's `capture` produced, as paths and hashes only.
///
/// Paths are relative to the state root the adapter declared. The bytes live
/// in the store under their chunk ids; this type never holds them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HarnessState {
    /// The captured files, directories and links.
    pub entries: Vec<Entry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_adapter() {
        assert_eq!(HarnessKind::ClaudeCode.to_string(), "claude-code");
        assert_eq!(HarnessKind::Codex.to_string(), "codex");
        assert_eq!(
            HarnessKind::Acp {
                command: "opencode".into()
            }
            .to_string(),
            "acp:opencode"
        );
    }
}
