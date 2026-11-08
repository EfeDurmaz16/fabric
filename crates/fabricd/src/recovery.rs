//! The verifier of a captured checkpoint (ADR 0028): what the authority
//! does between a runner's `Captured` report and the `CheckpointAcknowledged`
//! it commits.
//!
//! Invariant: an acknowledgement is made from bytes this node holds,
//! fetched through the store transfer and verified by content
//! (`fetch_snapshot` keeps a chunk only when it hashes to its id and
//! publishes a manifest only after every object it names is on disk, ADR
//! 0014), never from the runner's word; the head manifest read back must
//! hash to the snapshot's id. The job's checkpoint is the snapshot under
//! the M5 cut's digest (`CheckpointCut::digest`, ADR 0024), recorded with
//! the fenced `Checkpointed` report as `CheckpointCaptured`: the cut binds
//! the snapshot id with the receipts, facts and evidence, and the
//! snapshot id binds the bytes, so the acknowledgement names that digest
//! and the domain refuses any other (`acknowledge`). A capture that does
//! not verify stays captured, a restart point for nothing. The fetch runs
//! on a tokio task, never on the authority's writer thread; only the
//! commit goes through `Authority::acknowledge`. The decision of which
//! digest is the job's checkpoint was selected by the implementation
//! agent under Efe's delegation of 2026-11-02.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{Attempt, AttemptId, CheckpointAck, CutDigest, ResourceRef};
use fabric_store::store::Store;

use crate::node::{Node, lock};
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
    /// The head manifest read back does not hash to the snapshot's id.
    #[error("the head manifest of {snapshot} hashes to {got}")]
    WrongHead {
        /// The snapshot.
        snapshot: SnapshotId,
        /// What the bytes hash to.
        got: SnapshotId,
    },
    /// The store failed.
    #[error("store: {0}")]
    Store(String),
}

/// The pure decision: the acknowledgement to commit for `snapshot` of
/// `attempt` under the captured cut `cut`, given what the fetch brought,
/// or why none is made. A fetch that failed acknowledges nothing, and
/// neither does a head manifest that is not the snapshot's.
pub(crate) fn acknowledgement(
    attempt: &Attempt,
    snapshot: SnapshotId,
    cut: CutDigest,
    fetched: Result<Fetched, VerifyError>,
) -> Result<CheckpointAck, VerifyError> {
    let fetched = fetched?;
    let got = SnapshotId::of(&fetched.head);
    if got != snapshot {
        return Err(VerifyError::WrongHead { snapshot, got });
    }
    let ResourceRef::Node { node } = attempt.resource;
    Ok(CheckpointAck {
        attempt: attempt.id,
        generation: attempt.generation,
        snapshot,
        cut,
        manifests: fetched.manifests,
        chunks: fetched.chunks,
        bytes: fetched.bytes,
        fetched_from: node,
    })
}

/// Walks the published chain of `snapshot` in `store` and counts it.
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

/// For a capture the authority committed: opens a session to the
/// attempt's node, runs `fetch_snapshot` for the chain (a capture on this
/// node is in its own store already), reads the head manifest back, and
/// returns the acknowledgement to commit, or why it cannot.
pub(crate) async fn verify(
    node: &Arc<Node>,
    attempt: &Attempt,
    snapshot: SnapshotId,
    cut: CutDigest,
) -> Result<CheckpointAck, VerifyError> {
    let ResourceRef::Node { node: runner } = attempt.resource;
    let fetched = async {
        if runner == node.id {
            return with_store(&node.store, move |store| Ok(count_chain(store, snapshot)))
                .await
                .map_err(|e| VerifyError::Store(e.to_string()))?;
        }
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
    acknowledgement(attempt, snapshot, cut, fetched)
}

/// The verifier on the authority's node: one fetch at a time per captured
/// checkpoint, started when a report recorded a capture and again on each
/// live heartbeat from the attempt's node while the capture is not yet
/// acknowledged, so an unreachable runner is retried without a timer.
#[derive(Debug, Default)]
pub(crate) struct Verifier {
    node: OnceLock<Weak<Node>>,
    in_flight: Mutex<HashSet<(AttemptId, SnapshotId)>>,
}

impl Verifier {
    /// Binds the node; a second bind changes nothing.
    pub(crate) fn bind(&self, node: Weak<Node>) {
        let _ = self.node.set(node);
    }

    /// Starts a verification of every capture of `job` not acknowledged
    /// and not being verified now. Returns at once.
    pub(crate) fn check(&self, job: JobId) {
        let Some(node) = self.node.get().and_then(Weak::upgrade) else {
            return;
        };
        tokio::spawn(async move {
            let state = match node.authority.job(job).await {
                Ok(Some(state)) => state,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(%job, error = %e, "the captures were not read");
                    return;
                }
            };
            for (attempt, _, snapshot) in state.captured_only() {
                let (Some(found), Some(held)) =
                    (state.attempt(attempt), state.checkpoint(snapshot))
                else {
                    continue;
                };
                if !lock(&node.verifier.in_flight).insert((attempt, snapshot)) {
                    continue;
                }
                let (found, cut) = (found.clone(), held.cut);
                let verifying = Arc::clone(&node);
                tokio::spawn(async move {
                    let outcome = verify(&verifying, &found, snapshot, cut).await;
                    match outcome {
                        Ok(ack) => match verifying.authority.acknowledge(job, ack).await {
                            Ok(_) => {
                                tracing::info!(%job, %attempt, %snapshot, "acknowledged a checkpoint");
                            }
                            Err(e) => {
                                tracing::info!(%job, %attempt, %snapshot, error = %e, "a verified checkpoint was not acknowledged");
                            }
                        },
                        Err(VerifyError::Unreachable { node }) => {
                            tracing::info!(%job, %attempt, %node, "the capture's node did not answer; retried on its next heartbeat");
                        }
                        Err(e) => {
                            tracing::warn!(%job, %attempt, %snapshot, error = %e, "a capture did not verify; it stays captured");
                        }
                    }
                    lock(&verifying.verifier.in_flight).remove(&(attempt, snapshot));
                });
            }
        });
    }
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
    fn a_corrupt_chunk_or_a_wrong_head_is_never_acknowledged() {
        let head = b"manifest bytes".to_vec();
        let snapshot = SnapshotId::of(&head);
        let cut = CutDigest::from_bytes([9; 32]);
        // The fetch refused a chunk: nothing is acknowledged, and the
        // reason names the snapshot.
        let refused = acknowledgement(
            &attempt(),
            snapshot,
            cut,
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
            cut,
            Err(VerifyError::Unreachable { node: RUNNER }),
        )
        .unwrap_err();
        assert_eq!(unreachable, VerifyError::Unreachable { node: RUNNER });
        // Bytes that are not the snapshot's head manifest: refused with
        // what they hash to.
        let wrong =
            acknowledgement(&attempt(), snapshot, cut, Ok(fetched(b"other bytes"))).unwrap_err();
        assert_eq!(
            wrong,
            VerifyError::WrongHead {
                snapshot,
                got: SnapshotId::of(b"other bytes"),
            }
        );
        // The snapshot's own head: the acknowledgement names the captured
        // cut, what was fetched and the node it came from.
        let ack = acknowledgement(&attempt(), snapshot, cut, Ok(fetched(&head))).unwrap();
        assert_eq!(
            ack,
            CheckpointAck {
                attempt: attempt().id,
                generation: Generation::FIRST,
                snapshot,
                cut,
                manifests: 2,
                chunks: 5,
                bytes: 1_024,
                fetched_from: RUNNER,
            }
        );
    }
}
