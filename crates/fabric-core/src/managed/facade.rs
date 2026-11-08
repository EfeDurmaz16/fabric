//! The facade token (ADR 0027): what the authority mints for an actor,
//! how a bearer is stored (by digest, never in clear) and the check every
//! facade command passes before anything else.
//!
//! Invariant: `check_token` answers in one order, unknown, revoked,
//! expired, job scope, resource scope, so two authorities refuse the same
//! token for the same reason and a caller learns nothing about scope from
//! a token it does not hold.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use ulid::Ulid;

use crate::id::{JobId, NodeId, ParseIdError};

/// How many events a watch holds for a slow reader before it lags.
pub const WATCH_WINDOW: usize = 256;
/// The longest a watch waits for an event, in milliseconds.
pub const WATCH_MAX_WAIT_MS: u32 = 25_000;
/// How long a watch waits when the caller names no wait.
pub const WATCH_DEFAULT_WAIT_MS: u32 = 10_000;
/// The bytes of a bearer token.
pub const BEARER_BYTES: usize = 32;
/// The most characters an actor name has.
pub const MAX_ACTOR: usize = 64;

/// The domain separator of a token digest.
const TOKEN_DOMAIN: &[u8] = b"fabric.facade.token.v1\0";

/// A token's id: a ULID, and the handle revocation takes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenId([u8; 16]);

impl TokenId {
    /// Wraps the 16 raw bytes of a ULID.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        TokenId(bytes)
    }

    /// The 16 raw bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Ulid::from_bytes(self.0), f)
    }
}

impl fmt::Debug for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenId({self})")
    }
}

impl FromStr for TokenId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ulid = Ulid::from_string(s).map_err(|_| ParseIdError::BadUlid)?;
        if ulid.to_string() != s {
            return Err(ParseIdError::BadUlid);
        }
        Ok(TokenId(ulid.to_bytes()))
    }
}

/// The digest of a bearer: what the authority stores and compares.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenDigest([u8; 32]);

impl TokenDigest {
    /// Wraps a 32-byte digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        TokenDigest(bytes)
    }

    /// The 32 raw bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Display for TokenDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for TokenDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenDigest({self})")
    }
}

/// The digest of a bearer: blake3 over the domain separator and the bytes.
#[must_use]
pub fn token_digest(bearer: &[u8; BEARER_BYTES]) -> TokenDigest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(TOKEN_DOMAIN);
    hasher.update(bearer);
    TokenDigest(*hasher.finalize().as_bytes())
}

/// Who holds a token: 1 to 64 characters of `A-Z`, `a-z`, `0-9`, `.`, `_` and `-`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Actor(String);

/// An actor name broke the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an actor has 1 to 64 characters of A-Z, a-z, 0-9, '.', '_' and '-'")]
pub struct ActorError;

impl Actor {
    /// Checks `text` against the rule and wraps it.
    pub fn parse(text: &str) -> Result<Self, ActorError> {
        if text.is_empty()
            || text.len() > MAX_ACTOR
            || !text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(ActorError);
        }
        Ok(Actor(text.to_owned()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Actor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Actor::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Actor({})", self.0)
    }
}

/// Which jobs a token may act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobScope {
    /// Any job the owner has.
    AnyOwned,
    /// These jobs only.
    Jobs(Vec<JobId>),
}

/// Which resources a token may place on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceScope {
    /// Any node.
    Any,
    /// These nodes only.
    Nodes(Vec<NodeId>),
}

/// A minted token as the journal keeps it; never the bearer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacadeTokenRecord {
    /// Its id.
    pub id: TokenId,
    /// The digest of its bearer.
    pub digest: TokenDigest,
    /// Who holds it.
    pub actor: Actor,
    /// Which jobs.
    pub jobs: JobScope,
    /// Which resources.
    pub resources: ResourceScope,
    /// When it was minted, Unix milliseconds.
    pub minted_ms: u64,
    /// When it expires, Unix milliseconds; expired at and after.
    pub expires_ms: u64,
    /// When it was revoked, if it was.
    pub revoked_ms: Option<u64>,
}

/// Which scope a token fell outside of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScopeKind {
    /// The job scope.
    Job,
    /// The resource scope.
    Resource,
}

/// What a token check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenCheck {
    /// The token may act.
    Ok,
    /// No such token.
    Unknown,
    /// Revoked.
    Revoked,
    /// Expired.
    Expired,
    /// Known and live, but not for this job or resource.
    OutOfScope {
        /// Which scope.
        what: ScopeKind,
    },
}

/// Checks a token in one order: unknown, revoked, expired, job scope,
/// resource scope. `job` and `resource` are what the command names;
/// `None` asks nothing of that scope.
#[must_use]
pub fn check_token(
    record: Option<&FacadeTokenRecord>,
    now_ms: u64,
    job: Option<JobId>,
    resource: Option<NodeId>,
) -> TokenCheck {
    let Some(record) = record else {
        return TokenCheck::Unknown;
    };
    if record.revoked_ms.is_some() {
        return TokenCheck::Revoked;
    }
    if now_ms >= record.expires_ms {
        return TokenCheck::Expired;
    }
    if let (Some(job), JobScope::Jobs(jobs)) = (job, &record.jobs)
        && !jobs.contains(&job)
    {
        return TokenCheck::OutOfScope {
            what: ScopeKind::Job,
        };
    }
    if let (Some(node), ResourceScope::Nodes(nodes)) = (resource, &record.resources)
        && !nodes.contains(&node)
    {
        return TokenCheck::OutOfScope {
            what: ScopeKind::Resource,
        };
    }
    TokenCheck::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> FacadeTokenRecord {
        FacadeTokenRecord {
            id: TokenId::from_bytes([1; 16]),
            digest: token_digest(&[7; 32]),
            actor: Actor::parse("ci-bot").unwrap(),
            jobs: JobScope::Jobs(vec![JobId::from_bytes([1; 16])]),
            resources: ResourceScope::Nodes(vec![NodeId::from_bytes([2; 32])]),
            minted_ms: 100,
            expires_ms: 1_000,
            revoked_ms: None,
        }
    }

    #[test]
    fn a_token_is_checked_in_one_order() {
        let job = JobId::from_bytes([1; 16]);
        let other_job = JobId::from_bytes([9; 16]);
        let node = NodeId::from_bytes([2; 32]);
        let other_node = NodeId::from_bytes([3; 32]);
        assert_eq!(check_token(None, 500, None, None), TokenCheck::Unknown);
        let revoked = FacadeTokenRecord {
            revoked_ms: Some(400),
            ..record()
        };
        // Revoked wins over expired and scope.
        assert_eq!(
            check_token(Some(&revoked), 5_000, Some(other_job), None),
            TokenCheck::Revoked
        );
        assert_eq!(
            check_token(Some(&record()), 1_000, Some(other_job), None),
            TokenCheck::Expired
        );
        assert_eq!(
            check_token(Some(&record()), 999, Some(other_job), Some(other_node)),
            TokenCheck::OutOfScope {
                what: ScopeKind::Job
            }
        );
        assert_eq!(
            check_token(Some(&record()), 999, Some(job), Some(other_node)),
            TokenCheck::OutOfScope {
                what: ScopeKind::Resource
            }
        );
        assert_eq!(
            check_token(Some(&record()), 999, Some(job), Some(node)),
            TokenCheck::Ok
        );
        assert_eq!(
            check_token(Some(&record()), 999, None, None),
            TokenCheck::Ok
        );
        let any = FacadeTokenRecord {
            jobs: JobScope::AnyOwned,
            resources: ResourceScope::Any,
            ..record()
        };
        assert_eq!(
            check_token(Some(&any), 999, Some(other_job), Some(other_node)),
            TokenCheck::Ok
        );
    }

    #[test]
    fn actors_and_token_ids_have_one_form() {
        assert!(Actor::parse("ci.bot_1-x").is_ok());
        for bad in ["", "a b", "a/b", &"a".repeat(65)] {
            assert_eq!(Actor::parse(bad), Err(ActorError), "{bad:?}");
        }
        let id = TokenId::from_bytes([0x01, 0x92, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9]);
        assert_eq!(id.to_string().parse::<TokenId>(), Ok(id));
        assert_ne!(token_digest(&[1; 32]), token_digest(&[2; 32]));
        assert_ne!(
            token_digest(&[1; 32]).to_bytes(),
            *blake3::hash(&[1; 32]).as_bytes(),
            "the digest is under its own domain"
        );
    }
}
