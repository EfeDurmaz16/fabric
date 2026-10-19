//! The environment spec, version 0 (ADR 0007).
//!
//! Invariant: a spec names things and never holds a secret value: an
//! environment variable appears as a parsed name that cannot hold a `=` or a
//! value. A setup command is a program and its arguments, never a shell
//! string. Every list whose order carries no meaning is strictly sorted, so
//! the spec's identity, the hash of its canonical bytes, is the same for the
//! same content on every node, and an edit is a new spec.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::codec::{self, CodecError, VersionedError};
use crate::harness::ProgramName;
use crate::id::EnvSpecId;
use crate::path::RelPath;
use crate::snapshot::first_out_of_order;

/// The format version this crate writes; the first varint of every encoded spec.
pub const ENV_SPEC_VERSION: u16 = 0;

/// Why an environment spec was refused.
#[derive(Debug, thiserror::Error)]
pub enum EnvSpecError {
    /// A list is not strictly sorted by its key, which also catches a repeated key.
    #[error("{list} entry {index} is not strictly after the one before it")]
    OutOfOrder {
        /// Which list: `tools`, `env_var_names` or `cache_paths`.
        list: &'static str,
        /// Zero-based index of the entry, within its list.
        index: usize,
    },
    /// The bytes carry another version or do not decode, or the value does
    /// not encode. A bad program or variable name on the wire surfaces here too.
    #[error("environment spec codec: {0}")]
    Codec(#[from] VersionedError),
}

impl From<CodecError> for EnvSpecError {
    fn from(err: CodecError) -> Self {
        EnvSpecError::Codec(VersionedError::Codec(err))
    }
}

/// Why an environment variable name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvVarNameError {
    /// The name has no characters.
    #[error("environment variable name is empty")]
    Empty,
    /// A character outside `[A-Za-z0-9_]`, or a leading digit.
    #[error("environment variable name has an invalid character at index {index}")]
    BadChar {
        /// Zero-based index of the offending character.
        index: usize,
    },
}

/// The name of an environment variable: `[A-Za-z_][A-Za-z0-9_]*`.
///
/// A name cannot hold `=`, so a value cannot ride along in it. Checked once,
/// at the boundary, including inside `Deserialize`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EnvVarName(String);

impl EnvVarName {
    /// Checks `text` against the rule and wraps it.
    pub fn parse(text: &str) -> Result<Self, EnvVarNameError> {
        if text.is_empty() {
            return Err(EnvVarNameError::Empty);
        }
        let bad = text
            .bytes()
            .position(|b| !(b == b'_' || b.is_ascii_alphanumeric()));
        if let Some(index) = bad {
            return Err(EnvVarNameError::BadChar { index });
        }
        if text.as_bytes()[0].is_ascii_digit() {
            return Err(EnvVarNameError::BadChar { index: 0 });
        }
        Ok(Self(text.to_owned()))
    }

    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvVarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for EnvVarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvVarName({:?})", self.0)
    }
}

impl FromStr for EnvVarName {
    type Err = EnvVarNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for EnvVarName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// What a workspace needs before a harness can run in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSpec {
    /// Tools the workspace was seen with; used to warn, never to fail. Sorted by name.
    pub tools: Vec<Tool>,
    /// Commands that make the workspace runnable, in the order they run.
    pub setup: Vec<SetupStep>,
    /// Names of environment variables the job needs; values never travel. Sorted.
    pub env_var_names: Vec<EnvVarName>,
    /// Paths that count as caches for snapshot policy and warmth checks. Sorted.
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
    pub program: ProgramName,
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
    /// When a cache path is missing, empty, partial or keyed to another
    /// node, and on every resume when the spec has no cache paths, since
    /// nothing then proves the workspace warm.
    CachesMissing,
    /// On every resume.
    Always,
}

impl EnvSpec {
    /// Checks the spec's own rules: every unordered list is strictly sorted.
    pub fn validate(&self) -> Result<(), EnvSpecError> {
        let lists = [
            (
                "tools",
                first_out_of_order(self.tools.iter().map(|t| &t.name)),
            ),
            (
                "env_var_names",
                first_out_of_order(self.env_var_names.iter()),
            ),
            ("cache_paths", first_out_of_order(self.cache_paths.iter())),
        ];
        for (list, index) in lists {
            if let Some(index) = index {
                return Err(EnvSpecError::OutOfOrder { list, index });
            }
        }
        Ok(())
    }

    /// Returns the canonical bytes of a valid spec: the version, then the fields.
    pub fn encode(&self) -> Result<Vec<u8>, EnvSpecError> {
        self.validate()?;
        Ok(codec::encode_versioned(ENV_SPEC_VERSION, self)?)
    }

    /// Decodes bytes written by `encode`, refusing other versions, and checks the result.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvSpecError> {
        let spec: EnvSpec = codec::decode_versioned(bytes, ENV_SPEC_VERSION)?;
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

    fn var(name: &str) -> EnvVarName {
        EnvVarName::parse(name).unwrap()
    }

    fn spec() -> EnvSpec {
        EnvSpec {
            tools: vec![Tool {
                name: "node".into(),
                version: "v22.23.2".into(),
                source: ToolSource::Detected,
            }],
            setup: vec![SetupStep {
                program: ProgramName::parse("npm").unwrap(),
                args: vec!["ci".into()],
                cwd: None,
                when: RunWhen::CachesMissing,
            }],
            env_var_names: vec![var("DATABASE_URL")],
            cache_paths: vec![RelPath::parse("node_modules").unwrap()],
        }
    }

    #[test]
    fn id_changes_when_the_spec_changes() {
        let a = spec();
        let mut b = spec();
        b.env_var_names.push(var("OTHER"));
        assert_ne!(a.id().unwrap(), b.id().unwrap());
        assert_eq!(a.id().unwrap(), spec().id().unwrap());
    }

    #[test]
    fn decode_round_trips() {
        let bytes = spec().encode().unwrap();
        assert_eq!(EnvSpec::decode(&bytes).unwrap(), spec());
    }

    /// A name cannot carry a value: `DATABASE_URL=postgres://u:p@h/db` is
    /// not a name, on the wire included.
    #[test]
    fn an_env_var_name_is_a_name_and_never_a_value() {
        assert!(EnvVarName::parse("DATABASE_URL").is_ok());
        assert!(EnvVarName::parse("_x9").is_ok());
        assert_eq!(EnvVarName::parse(""), Err(EnvVarNameError::Empty));
        assert_eq!(
            EnvVarName::parse("DATABASE_URL=postgres://u:p@h/db"),
            Err(EnvVarNameError::BadChar { index: 12 })
        );
        assert_eq!(
            EnvVarName::parse("9LIVES"),
            Err(EnvVarNameError::BadChar { index: 0 })
        );
        assert_eq!(
            EnvVarName::parse("A B"),
            Err(EnvVarNameError::BadChar { index: 1 })
        );
        let bytes = postcard::to_stdvec("A=b").unwrap();
        assert!(postcard::from_bytes::<EnvVarName>(&bytes).is_err());
    }

    /// Two specs with the same content in another order would have two ids.
    #[test]
    fn unordered_lists_must_be_strictly_sorted() {
        let mut s = spec();
        s.env_var_names = vec![var("B"), var("A")];
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::OutOfOrder {
                list: "env_var_names",
                index: 1
            })
        ));
        let mut s = spec();
        s.cache_paths.push(s.cache_paths[0].clone());
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::OutOfOrder {
                list: "cache_paths",
                index: 1
            })
        ));
        let mut s = spec();
        s.tools.push(s.tools[0].clone());
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::OutOfOrder {
                list: "tools",
                index: 1
            })
        ));
        // The same rule holds on the wire: bytes of an unsorted spec, made
        // without validate, do not decode.
        let unsorted = EnvSpec {
            env_var_names: vec![var("B"), var("A")],
            ..spec()
        };
        let bytes = codec::encode_versioned(ENV_SPEC_VERSION, &unsorted).unwrap();
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::OutOfOrder {
                list: "env_var_names",
                index: 1
            })
        ));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = spec().encode().unwrap();
        bytes.push(0);
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::Codec(VersionedError::Codec(
                CodecError::TrailingBytes(1)
            )))
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
            Err(EnvSpecError::Codec(VersionedError::Codec(
                CodecError::NonCanonical
            )))
        ));
    }

    #[test]
    fn unknown_version_is_refused() {
        let mut bytes = spec().encode().unwrap();
        bytes[0] = 7;
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::Codec(VersionedError::UnknownVersion(7)))
        ));
    }
}
