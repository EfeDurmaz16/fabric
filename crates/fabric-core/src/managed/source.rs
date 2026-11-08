//! The names a git source binding is made of (ADR 0025).
//!
//! Invariant: a `GitUri` never carries a credential and a `CommitId` is a
//! full object id, never a ref name, so a source binding names one tree
//! on every node that reads it and nothing secret travels in the binding.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::git::GitOid;

/// The most bytes a git URI has.
pub const MAX_GIT_URI: usize = 2048;

/// Why a source binding or a secret name was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SourceError {
    /// The URI is not one this crate accepts.
    #[error("the git uri is not accepted: {reason}")]
    BadUri {
        /// What is wrong.
        reason: String,
    },
    /// The URI carries a user name or a password.
    #[error("the git uri carries a credential")]
    CredentialInUri,
    /// The revision is not a full commit id.
    #[error("the revision is not a full commit id of 40 or 64 lowercase hex characters")]
    NotACommitId,
    /// Adoption of an active checkout is not supported in M5.
    #[error("adopting an active checkout as a managed job's workspace is not supported")]
    Adoption,
    /// A repository path on the authority's host names nothing over the facade.
    #[error("a repository path is accepted on the local socket only")]
    RepositoryOverFacade,
    /// A secret name breaks its rule.
    #[error("a secret name has 1 to 64 characters of A-Z, 0-9 and _")]
    BadSecretName,
    /// The source could not be read; the reason is the tool's.
    #[error("the source could not be read: {reason}")]
    Unavailable {
        /// What the clone or the restore said.
        reason: String,
    },
}

/// The schemes a git URI may have.
const SCHEMES: &[&str] = &["https://", "ssh://", "git+ssh://", "file://"];

/// A git URI: 1 to [`MAX_GIT_URI`] bytes, scheme `https`, `ssh`,
/// `git+ssh` or `file`, no user information, no control character.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct GitUri(String);

impl GitUri {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, SourceError> {
        let bad = |reason: &str| SourceError::BadUri {
            reason: reason.to_owned(),
        };
        if text.is_empty() || text.len() > MAX_GIT_URI {
            return Err(bad("empty or longer than 2048 bytes"));
        }
        if text.chars().any(|c| c.is_control() || c == ' ') {
            return Err(bad("a control character or a space"));
        }
        let Some(rest) = SCHEMES.iter().find_map(|scheme| text.strip_prefix(scheme)) else {
            return Err(bad("the scheme is not https, ssh, git+ssh or file"));
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            return Err(SourceError::CredentialInUri);
        }
        if rest.contains(['?', '#']) {
            return Err(bad("a query or a fragment"));
        }
        if !text.starts_with("file://") && authority.is_empty() {
            return Err(bad("no host"));
        }
        Ok(GitUri(text.to_owned()))
    }

    /// The URI's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for GitUri {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        GitUri::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for GitUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for GitUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GitUri({})", self.0)
    }
}

/// A full commit id: 40 or 64 lowercase hex characters; never a ref name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl CommitId {
    /// Checks `text` against the rule and wraps it.
    pub fn parse(text: &str) -> Result<Self, SourceError> {
        GitOid::parse(text)
            .map(|oid| CommitId(oid.as_str().to_owned()))
            .map_err(|_| SourceError::NotACommitId)
    }

    /// The id's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CommitId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        CommitId::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CommitId({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_uris_take_four_schemes_and_no_credential() {
        for ok in [
            "https://github.com/o/r.git",
            "ssh://github.com/o/r",
            "git+ssh://host/r",
            "file:///tmp/repo",
        ] {
            assert!(GitUri::parse(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            GitUri::parse("https://user:tok@github.com/o/r"),
            Err(SourceError::CredentialInUri)
        );
        assert_eq!(
            GitUri::parse("ssh://git@github.com/o/r"),
            Err(SourceError::CredentialInUri)
        );
        for bad in [
            "",
            "git@github.com:o/r.git",
            "http://host/r",
            "https://host/r?token=x",
            "https://host/r#frag",
            "https:///r",
            "https://host/r\n",
        ] {
            assert!(
                matches!(GitUri::parse(bad), Err(SourceError::BadUri { .. })),
                "{bad:?}"
            );
        }
        assert!(matches!(
            GitUri::parse(&format!("https://h/{}", "a".repeat(2048))),
            Err(SourceError::BadUri { .. })
        ));
    }

    #[test]
    fn commit_ids_are_full_lowercase_hex() {
        assert!(CommitId::parse(&"a".repeat(40)).is_ok());
        assert!(CommitId::parse(&"0".repeat(64)).is_ok());
        for bad in ["main", "HEAD", &"A".repeat(40), &"a".repeat(39), "v1.0"] {
            assert_eq!(
                CommitId::parse(bad),
                Err(SourceError::NotACommitId),
                "{bad}"
            );
        }
        let bytes = postcard::to_stdvec("main").unwrap();
        assert!(postcard::from_bytes::<CommitId>(&bytes).is_err());
    }
}
