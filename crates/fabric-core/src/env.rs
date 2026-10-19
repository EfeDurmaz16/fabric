//! The environment spec, version 0 (ADR 0007).
//!
//! Invariant: a spec names things and never holds a secret value. Setup
//! commands are argument vectors, never shell strings. The spec's identity is
//! the hash of its canonical bytes, so an edit is a new spec.

use serde::{Deserialize, Serialize};

use crate::id::EnvSpecId;
use crate::path::RelPath;

/// The format version this crate writes.
pub const ENV_SPEC_VERSION: u16 = 0;

/// Why an environment spec was refused.
#[derive(Debug, thiserror::Error)]
pub enum EnvSpecError {
    /// The bytes carry a version this crate does not know.
    #[error("environment spec version {0} is not supported")]
    UnknownVersion(u16),
    /// A setup step has no command.
    #[error("setup step {index} has an empty argv")]
    EmptyArgv {
        /// Zero-based index of the step.
        index: usize,
    },
    /// The bytes do not decode, or the value does not encode.
    #[error("environment spec codec: {0}")]
    Codec(#[from] postcard::Error),
}

/// What a workspace needs before a harness can run in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSpec {
    /// Format version of this struct.
    pub version: u16,
    /// Tools the workspace was seen with; used to warn, never to fail.
    pub tools: Vec<Tool>,
    /// Commands that make the workspace runnable.
    pub setup: Vec<SetupStep>,
    /// Names of environment variables the job needs; values never travel.
    pub env_var_names: Vec<String>,
    /// Paths that count as caches for snapshot policy and warmth checks.
    pub cache_paths: Vec<RelPath>,
}

/// One tool and the exact version string it reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// The tool's name, such as `node` or `rustc`.
    pub name: String,
    /// The tool's own version output, kept whole and never parsed into parts.
    pub version: String,
    /// Where the entry came from.
    pub source: ToolSource,
}

/// How a tool entry got into the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolSource {
    /// Found by detection on the workspace.
    Detected,
    /// Written by the user.
    Declared,
}

/// One command that materialize runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    /// The program and its arguments, run directly without a shell.
    pub argv: Vec<String>,
    /// The directory to run in, relative to the root; `None` is the root itself.
    pub cwd: Option<RelPath>,
    /// When the step runs.
    pub when: RunWhen,
}

/// When a setup step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunWhen {
    /// Only when a cache path is missing, empty, partial or keyed to another node.
    CachesMissing,
    /// On every resume.
    Always,
}

impl EnvSpec {
    /// Checks the spec's own rules.
    pub fn validate(&self) -> Result<(), EnvSpecError> {
        if self.version != ENV_SPEC_VERSION {
            return Err(EnvSpecError::UnknownVersion(self.version));
        }
        if let Some(index) = self.setup.iter().position(|s| s.argv.is_empty()) {
            return Err(EnvSpecError::EmptyArgv { index });
        }
        Ok(())
    }

    /// Returns the canonical postcard bytes of a valid spec.
    pub fn encode(&self) -> Result<Vec<u8>, EnvSpecError> {
        self.validate()?;
        Ok(postcard::to_stdvec(self)?)
    }

    /// Decodes and checks a spec.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvSpecError> {
        let spec: EnvSpec = postcard::from_bytes(bytes)?;
        spec.validate()?;
        Ok(spec)
    }

    /// Returns the spec's identity: blake3 of its canonical bytes.
    pub fn id(&self) -> Result<EnvSpecId, EnvSpecError> {
        Ok(EnvSpecId::of(&self.encode()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> EnvSpec {
        EnvSpec {
            version: ENV_SPEC_VERSION,
            tools: vec![Tool {
                name: "node".into(),
                version: "v22.23.2".into(),
                source: ToolSource::Detected,
            }],
            setup: vec![SetupStep {
                argv: vec!["npm".into(), "ci".into()],
                cwd: None,
                when: RunWhen::CachesMissing,
            }],
            env_var_names: vec!["DATABASE_URL".into()],
            cache_paths: vec![RelPath::parse("node_modules").unwrap()],
        }
    }

    #[test]
    fn id_changes_when_the_spec_changes() {
        let a = spec();
        let mut b = spec();
        b.env_var_names.push("OTHER".into());
        assert_ne!(a.id().unwrap(), b.id().unwrap());
        assert_eq!(a.id().unwrap(), spec().id().unwrap());
    }

    #[test]
    fn decode_round_trips() {
        let bytes = spec().encode().unwrap();
        assert_eq!(EnvSpec::decode(&bytes).unwrap(), spec());
    }

    #[test]
    fn empty_argv_is_refused() {
        let mut s = spec();
        s.setup.push(SetupStep {
            argv: vec![],
            cwd: None,
            when: RunWhen::Always,
        });
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::EmptyArgv { index: 1 })
        ));
    }

    #[test]
    fn unknown_version_is_refused() {
        let mut s = spec();
        s.version = 7;
        assert!(matches!(s.id(), Err(EnvSpecError::UnknownVersion(7))));
    }
}
