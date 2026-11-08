//! The facade tokens (ADR 0027): rows of `facade_tokens`, authority state
//! beside the job history.
//!
//! Invariant: the journal holds a bearer's digest and never the bearer. A
//! token id and a digest are each unique, so a mint that repeats either
//! is refused rather than overwritten, and a revoke writes the time once:
//! a second revoke of the same token answers `false` and changes nothing.
//! The scopes are stored in a one-byte-tagged form of their own, so a
//! future scope kind is a new tag and an old row still reads.

use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::facade::{
    Actor, FacadeTokenRecord, JobScope, ResourceScope, TokenDigest, TokenId,
};
use rusqlite::{OptionalExtension, params};

use crate::project::{get_u64, sql_u64};
use crate::{CommitError, Journal};

/// `JobScope::AnyOwned` and `ResourceScope::Any`: one byte.
const TAG_ANY: u8 = 0;
/// `JobScope::Jobs` and `ResourceScope::Nodes`: the tag, then the ids back to back.
const TAG_LIST: u8 = 1;

fn encode_jobs(scope: &JobScope) -> Vec<u8> {
    match scope {
        JobScope::AnyOwned => vec![TAG_ANY],
        JobScope::Jobs(jobs) => {
            let mut out = Vec::with_capacity(1 + jobs.len() * 16);
            out.push(TAG_LIST);
            for job in jobs {
                out.extend_from_slice(&job.to_bytes());
            }
            out
        }
    }
}

fn encode_resources(scope: &ResourceScope) -> Vec<u8> {
    match scope {
        ResourceScope::Any => vec![TAG_ANY],
        ResourceScope::Nodes(nodes) => {
            let mut out = Vec::with_capacity(1 + nodes.len() * 32);
            out.push(TAG_LIST);
            for node in nodes {
                out.extend_from_slice(&node.to_bytes());
            }
            out
        }
    }
}

fn corrupt(reason: &str) -> CommitError {
    CommitError::Corrupt {
        job: None,
        reason: format!("facade_tokens: {reason}"),
    }
}

fn decode_jobs(bytes: &[u8]) -> Result<JobScope, CommitError> {
    match bytes.split_first() {
        Some((&TAG_ANY, [])) => Ok(JobScope::AnyOwned),
        Some((&TAG_LIST, rest)) if rest.len().is_multiple_of(16) => Ok(JobScope::Jobs(
            rest.chunks_exact(16)
                .map(|chunk| {
                    let mut id = [0u8; 16];
                    id.copy_from_slice(chunk);
                    JobId::from_bytes(id)
                })
                .collect(),
        )),
        _ => Err(corrupt("jobs scope")),
    }
}

fn decode_resources(bytes: &[u8]) -> Result<ResourceScope, CommitError> {
    match bytes.split_first() {
        Some((&TAG_ANY, [])) => Ok(ResourceScope::Any),
        Some((&TAG_LIST, rest)) if rest.len().is_multiple_of(32) => Ok(ResourceScope::Nodes(
            rest.chunks_exact(32)
                .map(|chunk| {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(chunk);
                    NodeId::from_bytes(id)
                })
                .collect(),
        )),
        _ => Err(corrupt("resources scope")),
    }
}

impl Journal {
    /// Stores a minted token. Refuses a token id or a digest the table
    /// already holds, as `CommitError::Domain` would a used key: nothing
    /// is overwritten.
    pub fn token_mint(&mut self, record: &FacadeTokenRecord) -> Result<(), CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let known: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM facade_tokens WHERE token_id = ?1 OR digest = ?2",
                params![record.id.to_bytes(), record.digest.to_bytes()],
                |r| r.get(0),
            )
            .optional()?;
        if known.is_some() {
            return Err(CommitError::Encode(format!(
                "facade token {} or its digest is already minted",
                record.id
            )));
        }
        tx.execute(
            "INSERT INTO facade_tokens (token_id, digest, actor, jobs, resources, minted_ms, expires_ms, revoked_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id.to_bytes(),
                record.digest.to_bytes(),
                record.actor.as_str(),
                encode_jobs(&record.jobs),
                encode_resources(&record.resources),
                sql_u64(record.minted_ms)?,
                sql_u64(record.expires_ms)?,
                record.revoked_ms.map(sql_u64).transpose()?,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The token whose bearer has this digest, if any.
    pub fn token_by_digest(
        &self,
        digest: &TokenDigest,
    ) -> Result<Option<FacadeTokenRecord>, CommitError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT token_id, actor, jobs, resources, minted_ms, expires_ms, revoked_ms
             FROM facade_tokens WHERE digest = ?1",
        )?;
        let row = stmt
            .query_row(params![digest.to_bytes()], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    get_u64(r, 4)?,
                    get_u64(r, 5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })
            .optional()?;
        let Some((id, actor, jobs, resources, minted_ms, expires_ms, revoked)) = row else {
            return Ok(None);
        };
        let id: [u8; 16] = id.try_into().map_err(|_| corrupt("token_id"))?;
        let revoked_ms = revoked
            .map(|ms| u64::try_from(ms).map_err(|_| corrupt("revoked_ms")))
            .transpose()?;
        Ok(Some(FacadeTokenRecord {
            id: TokenId::from_bytes(id),
            digest: *digest,
            actor: Actor::parse(&actor).map_err(|_| corrupt("actor"))?,
            jobs: decode_jobs(&jobs)?,
            resources: decode_resources(&resources)?,
            minted_ms,
            expires_ms,
            revoked_ms,
        }))
    }

    /// Revokes a token at `at_ms`: `true` when it was live, `false` when
    /// the id is unknown or already revoked, which changes nothing.
    pub fn token_revoke(&mut self, id: TokenId, at_ms: u64) -> Result<bool, CommitError> {
        let changed = self.conn.execute(
            "UPDATE facade_tokens SET revoked_ms = ?2 WHERE token_id = ?1 AND revoked_ms IS NULL",
            params![id.to_bytes(), sql_u64(at_ms)?],
        )?;
        Ok(changed == 1)
    }
}

#[cfg(test)]
mod tests {
    use fabric_core::managed::facade::token_digest;

    use super::*;

    fn journal() -> (tempfile::TempDir, Journal) {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::create(
            &dir.path().join(crate::FILE_NAME),
            NodeId::from_bytes([9; 32]),
            1,
        )
        .unwrap();
        (dir, journal)
    }

    fn record(id: u8, bearer: u8) -> FacadeTokenRecord {
        FacadeTokenRecord {
            id: TokenId::from_bytes([id; 16]),
            digest: token_digest(&[bearer; 32]),
            actor: Actor::parse("ci-bot").unwrap(),
            jobs: JobScope::Jobs(vec![JobId::from_bytes([1; 16]), JobId::from_bytes([2; 16])]),
            resources: ResourceScope::Nodes(vec![NodeId::from_bytes([3; 32])]),
            minted_ms: 100,
            expires_ms: 1_000,
            revoked_ms: None,
        }
    }

    /// A minted token reads back whole by its digest, a repeated id or
    /// digest is refused, a revoke writes once, and a journal reopened
    /// still holds the rows: they are state, not history.
    #[test]
    fn tokens_are_minted_once_read_by_digest_and_revoked_once() {
        let (dir, mut journal) = journal();
        let first = record(1, 7);
        journal.token_mint(&first).unwrap();
        assert_eq!(
            journal.token_by_digest(&first.digest).unwrap(),
            Some(first.clone())
        );
        assert_eq!(
            journal.token_by_digest(&token_digest(&[8; 32])).unwrap(),
            None
        );
        assert!(journal.token_mint(&record(1, 8)).is_err(), "the id is used");
        assert!(
            journal.token_mint(&record(2, 7)).is_err(),
            "the digest is used"
        );
        let any = FacadeTokenRecord {
            jobs: JobScope::AnyOwned,
            resources: ResourceScope::Any,
            ..record(2, 8)
        };
        journal.token_mint(&any).unwrap();
        assert_eq!(
            journal.token_by_digest(&any.digest).unwrap(),
            Some(any.clone())
        );

        assert!(journal.token_revoke(first.id, 500).unwrap());
        assert!(
            !journal.token_revoke(first.id, 600).unwrap(),
            "revoked once"
        );
        assert!(
            !journal
                .token_revoke(TokenId::from_bytes([5; 16]), 600)
                .unwrap()
        );
        let revoked = journal.token_by_digest(&first.digest).unwrap().unwrap();
        assert_eq!(revoked.revoked_ms, Some(500));

        drop(journal);
        let reopened = Journal::open(&dir.path().join(crate::FILE_NAME)).unwrap();
        assert_eq!(
            reopened
                .token_by_digest(&first.digest)
                .unwrap()
                .unwrap()
                .revoked_ms,
            Some(500)
        );
        reopened.verify().unwrap();
    }
}
