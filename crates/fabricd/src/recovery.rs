//! The verifier of a captured checkpoint (ADR 0028): what the authority
//! does between a runner's `Captured` report and the `CheckpointAcknowledged`
//! it commits.
//!
//! Invariant: an acknowledgement is made from bytes this node fetched
//! through the store transfer and verified by content (`fetch_snapshot`
//! keeps a chunk only when it hashes to its id and publishes a manifest
//! only after every object it names is on disk, ADR 0014), never from the
//! runner's word. The cut the runner reported must equal the cut this node
//! computes over the published head manifest; anything else is refused and
//! the checkpoint stays captured, a restart point for nothing. The fetch
//! runs on a tokio task, never on the authority's writer thread; only the
//! commit goes through `Authority::acknowledge`.
//!
//! Until the M5 checkpoint cut (ADR 0024) lands at the restack, the cut
//! of a snapshot is the blake3 digest of its head manifest's canonical
//! bytes, computed the same way by the runner and the authority.

use std::sync::Arc;

use fabric_core::id::{NodeId, SnapshotId};
use fabric_core::managed::{Attempt, CheckpointAck, CutDigest, ResourceRef};
use fabric_store::store::Store;

use crate::node::Node;
use crate::transfer::{fetch_snapshot, with_store};

/// What the fetch brought: the counts for the acknowledgement and the
/// head manifest's bytes for the cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fetched {
    /// Manifests published, each after every object it names was on disk.
    pub manifests: u32,
    /// Chunks the chain names, each kept only after it hashed to its id.
    pub chunks: u64,
    /// The manifests' bytes.
    pub bytes: u64,
    /// The head manifest's canonical bytes.
    pub head: Vec<u8>,
}

/// Why a captured checkpoint was not acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum VerifyError {
    /// The attempt's node did not answer within the bound; retried on the
    /// next heartbeat, never recorded as a refusal.
    #[error("node {node} did not answer the fetch")]
    Unreachable {
        /// The node.
        node: NodeId,
    },
    /// A chunk or manifest did not hash to its id, or the published id is
    /// not the asked id (`fetch_snapshot`'s own refusals).
    #[error("checkpoint {snapshot} did not verify: {reason}")]
    Corrupt {
        /// The snapshot.
        snapshot: SnapshotId,
        /// What the fetch refused.
        reason: String,
    },
    /// The cut the runner reported is not the cut of the bytes fetched.
    #[error("the runner reported cut {reported}, the fetched bytes give {verified}")]
    CutMismatch {
        /// What the runner reported.
        reported: CutDigest,
        /// What this node computed.
        verified: CutDigest,
    },
    /// The store failed.
    #[error("store: {0}")]
    #[allow(
        dead_code,
        reason = "made by the fetch path, driven by the M5 executor at the restack"
    )]
    Store(String),
}

/// The cut of a snapshot: the digest of its head manifest's canonical bytes.
#[must_use]
pub(crate) fn cut_of(head_manifest: &[u8]) -> CutDigest {
    CutDigest::of(head_manifest)
}

/// The pure decision: the acknowledgement to commit for `snapshot` of
/// `attempt`, given what the fetch brought, or why none is made. A fetch
/// that failed acknowledges nothing, whatever the runner reported.
pub(crate) fn acknowledgement(
    attempt: &Attempt,
    snapshot: SnapshotId,
    reported: CutDigest,
    fetched: Result<Fetched, VerifyError>,
) -> Result<CheckpointAck, VerifyError> {
    let fetched = fetched?;
    let verified = cut_of(&fetched.head);
    if verified != reported {
        return Err(VerifyError::CutMismatch { reported, verified });
    }
    let ResourceRef::Node { node } = attempt.resource;
    Ok(CheckpointAck {
        attempt: attempt.id,
        generation: attempt.generation,
        snapshot,
        cut: verified,
        manifests: fetched.manifests,
        chunks: fetched.chunks,
        bytes: fetched.bytes,
        fetched_from: node,
    })
}

/// Walks the published chain of `snapshot` in `store` and counts it.
#[allow(
    dead_code,
    reason = "driven by the M5 executor's Captured reports at the restack"
)]
fn count_chain(store: &mut Store, snapshot: SnapshotId) -> Result<Fetched, VerifyError> {
    let failed = |e: &dyn std::fmt::Display| VerifyError::Store(e.to_string());
    let mut fetched = Fetched {
        manifests: 0,
        chunks: 0,
        bytes: 0,
        head: Vec::new(),
    };
    let mut next = Some(snapshot);
    while let Some(id) = next {
        let manifest = store.manifest(id).map_err(|e| failed(&e))?;
        let (len, ids) = store.manifest_chunks(id).map_err(|e| failed(&e))?;
        if id == snapshot {
            fetched.head = manifest.encode().map_err(|e| failed(&e))?;
        }
        fetched.manifests = fetched.manifests.saturating_add(1);
        fetched.chunks = fetched
            .chunks
            .saturating_add(u64::try_from(ids.len()).unwrap_or(u64::MAX));
        fetched.bytes = fetched.bytes.saturating_add(len);
        next = manifest.parent;
    }
    Ok(fetched)
}

/// For a `Captured` the authority committed: opens a session to the
/// attempt's node, runs `fetch_snapshot` for the chain, reads the head
/// manifest, recomputes the cut, and returns the acknowledgement to
/// commit, or why it cannot.
#[allow(
    dead_code,
    reason = "driven by the M5 executor's Captured reports at the restack"
)]
pub(crate) async fn verify(
    node: &Arc<Node>,
    attempt: &Attempt,
    snapshot: SnapshotId,
    reported: CutDigest,
) -> Result<CheckpointAck, VerifyError> {
    let ResourceRef::Node { node: runner } = attempt.resource;
    let fetched = async {
        let session = node
            .session(runner)
            .await
            .map_err(|_| VerifyError::Unreachable { node: runner })?;
        fetch_snapshot(&node.store, &session, snapshot)
            .await
            .map_err(|e| VerifyError::Corrupt {
                snapshot,
                reason: e.to_string(),
            })?;
        with_store(&node.store, move |store| Ok(count_chain(store, snapshot)))
            .await
            .map_err(|e| VerifyError::Store(e.to_string()))?
    }
    .await;
    acknowledgement(attempt, snapshot, reported, fetched)
}

#[cfg(test)]
mod tests {
    use fabric_core::id::JobId;
    use fabric_core::managed::{
        AttemptId, AttemptStatus, Generation, OperationId, ResourceRef, TaskId,
    };

    use super::*;

    const RUNNER: NodeId = NodeId::from_bytes([7; 32]);

    fn attempt() -> Attempt {
        Attempt {
            id: AttemptId::from_bytes([2; 16]),
            task: TaskId::from_bytes([1; 16]),
            job: JobId::from_bytes([1; 16]),
            resource: ResourceRef::Node { node: RUNNER },
            generation: Generation::FIRST,
            initial_checkpoint: None,
            granted_by: OperationId::from_bytes([3; 16]),
            status: AttemptStatus::Running,
            unavailable_since_ms: None,
        }
    }

    fn fetched(head: &[u8]) -> Fetched {
        Fetched {
            manifests: 2,
            chunks: 5,
            bytes: 1_024,
            head: head.to_vec(),
        }
    }

    #[test]
    fn a_corrupt_chunk_is_never_acknowledged() {
        let snapshot = SnapshotId::of(b"manifest");
        let reported = cut_of(b"manifest bytes");
        // The fetch refused a chunk: nothing is acknowledged, whatever the
        // runner reported, and the reason names the snapshot.
        let refused = acknowledgement(
            &attempt(),
            snapshot,
            reported,
            Err(VerifyError::Corrupt {
                snapshot,
                reason: "chunk did not hash to its id".into(),
            }),
        )
        .unwrap_err();
        assert!(matches!(refused, VerifyError::Corrupt { snapshot: s, .. } if s == snapshot));
        // An unreachable runner is not a refusal either.
        let unreachable = acknowledgement(
            &attempt(),
            snapshot,
            reported,
            Err(VerifyError::Unreachable { node: RUNNER }),
        )
        .unwrap_err();
        assert_eq!(unreachable, VerifyError::Unreachable { node: RUNNER });
        // Bytes that verified but give another cut: a mismatch, recorded
        // with both digests.
        let mismatch = acknowledgement(&attempt(), snapshot, reported, Ok(fetched(b"other bytes")))
            .unwrap_err();
        assert_eq!(
            mismatch,
            VerifyError::CutMismatch {
                reported,
                verified: cut_of(b"other bytes"),
            }
        );
        // The same cut: the acknowledgement names what was fetched and
        // the node it came from.
        let ack = acknowledgement(
            &attempt(),
            snapshot,
            reported,
            Ok(fetched(b"manifest bytes")),
        )
        .unwrap();
        assert_eq!(
            ack,
            CheckpointAck {
                attempt: attempt().id,
                generation: Generation::FIRST,
                snapshot,
                cut: reported,
                manifests: 2,
                chunks: 5,
                bytes: 1_024,
                fetched_from: RUNNER,
            }
        );
    }
}
