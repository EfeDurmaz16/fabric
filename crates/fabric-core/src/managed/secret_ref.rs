//! Secret references (ADR 0025): how a job names a secret its tasks need,
//! and the type that holds a resolved value on the one node that resolves it.
//!
//! Invariant: a `SecretValue` has no `Serialize`, no `Clone`, prints
//! redacted and overwrites its bytes with zero when dropped, so a value
//! can reach a task's environment and nothing else: not the journal, not
//! the ledger, not a checkpoint, a package, a receipt or a log.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use super::source::SourceError;

/// The prefix of the environment variables the local resolver reads.
pub const SECRET_ENV_PREFIX: &str = "FABRIC_SECRET_";

/// The most characters a secret name has.
pub const MAX_SECRET_NAME: usize = 64;

/// The name of a secret: 1 to 64 characters of `A-Z`, `0-9` and `_`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SecretName(String);

impl SecretName {
    /// Checks `text` against the rule and wraps it.
    pub fn parse(text: &str) -> Result<Self, SourceError> {
        if text.is_empty()
            || text.len() > MAX_SECRET_NAME
            || !text
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(SourceError::BadSecretName);
        }
        Ok(SecretName(text.to_owned()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The environment variable the local resolver reads for it.
    #[must_use]
    pub fn env_var(&self) -> String {
        format!("{SECRET_ENV_PREFIX}{}", self.0)
    }
}

impl<'de> Deserialize<'de> for SecretName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        SecretName::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretName({})", self.0)
    }
}

/// Which attempts of the job a secret may reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttemptScope {
    /// Any attempt of the job.
    Any,
    /// Only the attempt that is the job's current grant when it is resolved.
    Current,
}

/// A kind of resource a secret may be resolved on; grows with `ResourceRef`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    /// A registered Fabric node.
    Node,
}

/// Where a secret may go.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretScope {
    /// Which attempts.
    pub attempt: AttemptScope,
    /// Which kinds of resource; empty allows none.
    pub resource_kinds: Vec<ResourceKind>,
}

impl SecretScope {
    /// Any attempt on a node: the scope a job gets when it names none.
    #[must_use]
    pub fn node_any() -> SecretScope {
        SecretScope {
            attempt: AttemptScope::Any,
            resource_kinds: vec![ResourceKind::Node],
        }
    }
}

/// A secret a job's tasks need, by name and scope; never a value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    /// Its name, which is also the variable the task reads it under.
    pub name: SecretName,
    /// Where it may go.
    pub scope: SecretScope,
}

/// A resolved secret value. It lives on the node that resolved it and
/// reaches a task's environment; it is never serialized, cloned or shown.
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    /// Wraps the bytes; they belong to the value now.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> SecretValue {
        SecretValue(bytes)
    }

    /// The bytes, for the one place that hands them to a task.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        for b in &mut self.0 {
            // A volatile write would be stronger; the standard library's
            // guarantee is the same as every other hand-rolled zeroing
            // without a new crate, and the point is that no copy is made
            // by this type in the first place.
            *b = 0;
        }
        std::hint::black_box(&self.0);
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_are_upper_snake_and_bounded() {
        assert!(SecretName::parse("GITHUB_TOKEN").is_ok());
        assert!(SecretName::parse("A1_").is_ok());
        assert_eq!(
            SecretName::parse("GITHUB_TOKEN").unwrap().env_var(),
            "FABRIC_SECRET_GITHUB_TOKEN"
        );
        for bad in ["", "github_token", "A B", "A-B", "é", &"A".repeat(65)] {
            assert_eq!(
                SecretName::parse(bad),
                Err(SourceError::BadSecretName),
                "{bad:?}"
            );
        }
        let bytes = postcard::to_stdvec("lower").unwrap();
        assert!(postcard::from_bytes::<SecretName>(&bytes).is_err());
    }

    #[test]
    fn a_secret_value_prints_redacted_and_exposes_on_purpose_only() {
        let value = SecretValue::new(b"hunter2".to_vec());
        assert_eq!(format!("{value:?}"), "<redacted>");
        assert_eq!(value.to_string(), "<redacted>");
        assert_eq!(value.expose(), b"hunter2");
    }
}
