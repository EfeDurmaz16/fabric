//! The environment spec, version 0 (ADR 0007).
//!
//! Invariant: a spec names things and never holds a secret value. A setup
//! command is a program and its arguments, never a shell string. The spec's
//! identity is the hash of its canonical bytes, so an edit is a new spec.

use serde::{Deserialize, Serialize};

use crate::codec::{self, CodecError, VersionedError};
use crate::id::EnvSpecId;
use crate::path::RelPath;

/// The format version this crate writes; the first varint of every encoded spec.
pub const ENV_SPEC_VERSION: u16 = 0;

/// Why an environment spec was refused.
#[derive(Debug, thiserror::Error)]
pub enum EnvSpecError {
    /// The bytes carry a version this crate does not know.
    #[error("environment spec version {0} is not supported")]
    UnknownVersion(u16),
    /// The bytes do not decode, or the value does not encode.
    #[error("environment spec codec: {0}")]
    Codec(#[from] CodecError),
}

/// What a workspace needs before a harness can run in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSpec {
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
    /// The program to run, looked up on `PATH`, never through a shell.
    pub program: String,
    /// Its arguments, passed as they are.
    pub args: Vec<String>,
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

impl From<VersionedError> for EnvSpecError {
    fn from(err: VersionedError) -> Self {
        match err {
            VersionedError::UnknownVersion(v) => EnvSpecError::UnknownVersion(v),
            VersionedError::Codec(c) => EnvSpecError::Codec(c),
        }
    }
}

impl EnvSpec {
    /// Returns the canonical bytes of the spec: the version, then the fields.
    pub fn encode(&self) -> Result<Vec<u8>, EnvSpecError> {
        Ok(codec::encode_versioned(ENV_SPEC_VERSION, self)?)
    }

    /// Decodes bytes written by `encode`, refusing other versions.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvSpecError> {
        Ok(codec::decode_versioned(bytes, ENV_SPEC_VERSION)?)
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
            tools: vec![Tool {
                name: "node".into(),
                version: "v22.23.2".into(),
                source: ToolSource::Detected,
            }],
            setup: vec![SetupStep {
                program: "npm".into(),
                args: vec!["ci".into()],
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
    fn trailing_bytes_are_refused() {
        let mut bytes = spec().encode().unwrap();
        bytes.push(0);
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::Codec(CodecError::TrailingBytes(1)))
        ));
    }

    /// Version 0 is one byte, `0x00`; `0x80 0x00` reads as the same number
    /// and would give the spec a second id.
    #[test]
    fn an_overlong_version_prefix_is_refused() {
        let bytes = spec().encode().unwrap();
        let mut over = vec![0x80, 0x00];
        over.extend_from_slice(&bytes[1..]);
        assert!(matches!(
            EnvSpec::decode(&over),
            Err(EnvSpecError::Codec(CodecError::NonCanonical))
        ));
    }

    #[test]
    fn unknown_version_is_refused() {
        let mut bytes = spec().encode().unwrap();
        bytes[0] = 7;
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::UnknownVersion(7))
        ));
    }
}
