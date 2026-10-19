//! The kinds of harness and the shape of their carried state.
//!
//! Invariant: the core never looks inside harness state. It knows which kind
//! of adapter owns it and the list of relative paths and hashes that make it
//! up, nothing more. A program the core names is a bare name for a `PATH`
//! lookup, so what a peer sends can never be a path or an option.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::snapshot::Entry;

/// Why a program name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramNameError {
    /// The name has no characters.
    #[error("program name is empty")]
    Empty,
    /// The name holds a slash, so it is a path and not a name to look up on `PATH`.
    #[error("program name contains a slash")]
    Slash,
    /// The name holds a NUL byte.
    #[error("program name contains a NUL byte")]
    Nul,
    /// The name starts with `-`, which an argument parser would read as an option.
    #[error("program name starts with `-`")]
    LeadingDash,
}

/// The name of a program to look up on `PATH` and start without a shell.
///
/// Never a path, never an option: no slash, no NUL, no leading `-`, not
/// empty. Checked once, at the boundary, including inside `Deserialize`, so
/// a spec or a record from a peer cannot name `/tmp/x`, `../bin/x` or
/// `-rf`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ProgramName(String);

impl ProgramName {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, ProgramNameError> {
        if text.is_empty() {
            return Err(ProgramNameError::Empty);
        }
        if text.contains('/') {
            return Err(ProgramNameError::Slash);
        }
        if text.contains('\0') {
            return Err(ProgramNameError::Nul);
        }
        if text.starts_with('-') {
            return Err(ProgramNameError::LeadingDash);
        }
        Ok(Self(text.to_owned()))
    }

    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProgramName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ProgramName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProgramName({:?})", self.0)
    }
}

impl FromStr for ProgramName {
    type Err = ProgramNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for ProgramName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Which agent drives a job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HarnessKind {
    /// Claude Code through its CLI.
    ClaudeCode,
    /// Codex through `codex exec resume`.
    Codex,
    /// Any agent that speaks the Agent Client Protocol, started as `program` with `args`.
    Acp {
        /// The program that starts the agent, looked up on `PATH`, never through a shell.
        program: ProgramName,
        /// Its arguments, passed as they are.
        args: Vec<String>,
    },
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HarnessKind::ClaudeCode => f.write_str("claude-code"),
            HarnessKind::Codex => f.write_str("codex"),
            HarnessKind::Acp { program, args } => {
                write!(f, "acp:{program}")?;
                for arg in args {
                    write!(f, " {arg}")?;
                }
                Ok(())
            }
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
                program: ProgramName::parse("opencode").unwrap(),
                args: vec!["acp".into()],
            }
            .to_string(),
            "acp:opencode acp"
        );
    }

    #[test]
    fn program_name_is_a_bare_name_for_a_path_lookup() {
        assert!(ProgramName::parse("opencode").is_ok());
        assert!(ProgramName::parse("claude.cmd").is_ok());
        assert_eq!(ProgramName::parse(""), Err(ProgramNameError::Empty));
        assert_eq!(ProgramName::parse("/tmp/x"), Err(ProgramNameError::Slash));
        assert_eq!(ProgramName::parse("../bin/x"), Err(ProgramNameError::Slash));
        assert_eq!(ProgramName::parse("a\0b"), Err(ProgramNameError::Nul));
        assert_eq!(
            ProgramName::parse("-rf"),
            Err(ProgramNameError::LeadingDash)
        );
        let bytes = postcard::to_stdvec("").unwrap();
        assert!(postcard::from_bytes::<ProgramName>(&bytes).is_err());
        let bytes = postcard::to_stdvec("npm").unwrap();
        assert_eq!(
            postcard::from_bytes::<ProgramName>(&bytes).unwrap(),
            ProgramName::parse("npm").unwrap()
        );
    }
}
