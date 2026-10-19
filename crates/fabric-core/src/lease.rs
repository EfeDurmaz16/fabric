//! The lease and the fencing token (ADR 0003).
//!
//! Invariant: a job is always owned by exactly one node of record, the lease
//! is derived from the ledger and nothing else, and no node acts on a job
//! without presenting the epoch it believes is current.

use serde::{Deserialize, Serialize};

use crate::id::{Epoch, JobId, MoveId, NodeId};

/// Which node may run a job right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaseState {
    /// `node` owns the job and may run a harness for it.
    Owned {
        /// The owner.
        node: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// `node` has committed to a move toward `target`; nobody may run the job.
    Departing {
        /// The owner of record, who started the move.
        node: NodeId,
        /// The current epoch, which the move's import will bump.
        epoch: Epoch,
        /// The move in flight.
        move_id: MoveId,
        /// The node that alone decides the move.
        target: NodeId,
    },
}

impl LeaseState {
    /// Returns the current epoch.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        match self {
            LeaseState::Owned { epoch, .. } | LeaseState::Departing { epoch, .. } => *epoch,
        }
    }

    /// Returns the owner of record.
    #[must_use]
    pub const fn holder(&self) -> NodeId {
        match self {
            LeaseState::Owned { node, .. } | LeaseState::Departing { node, .. } => *node,
        }
    }

    /// Returns true when `node` may run a harness for the job.
    #[must_use]
    pub fn runnable_by(&self, node: NodeId) -> bool {
        matches!(self, LeaseState::Owned { node: owner, .. } if *owner == node)
    }
}

/// What a runner presents with every write: the job and the epoch it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FencingToken {
    /// The job the token is for.
    pub job_id: JobId,
    /// The epoch the presenter believes is current.
    pub epoch: Epoch,
}

/// Why a fencing token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FenceError {
    /// The token names another job.
    #[error("token is for job {token}, not {job}")]
    WrongJob {
        /// The job the token names.
        token: JobId,
        /// The job the check was made against.
        job: JobId,
    },
    /// The presenter's epoch is older than the one this node knows.
    #[error("stale epoch {yours}, current is {current}")]
    StaleEpoch {
        /// The presenter's epoch.
        yours: Epoch,
        /// The epoch this node knows.
        current: Epoch,
    },
    /// The presenter's epoch is newer than the one this node knows; this node must fetch first.
    #[error("epoch {yours} is ahead of this node's {current}")]
    AheadOfUs {
        /// The presenter's epoch.
        yours: Epoch,
        /// The epoch this node knows.
        current: Epoch,
    },
}

impl FencingToken {
    /// Accepts the token only when it names `job_id` at exactly `current`.
    pub fn check(&self, job_id: JobId, current: Epoch) -> Result<(), FenceError> {
        if self.job_id != job_id {
            return Err(FenceError::WrongJob {
                token: self.job_id,
                job: job_id,
            });
        }
        if self.epoch < current {
            return Err(FenceError::StaleEpoch {
                yours: self.epoch,
                current,
            });
        }
        if self.epoch > current {
            return Err(FenceError::AheadOfUs {
                yours: self.epoch,
                current,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    #[test]
    fn departing_is_runnable_by_nobody() {
        let lease = LeaseState::Departing {
            node: node(1),
            epoch: Epoch::FIRST,
            move_id: MoveId::from_bytes([0; 16]),
            target: node(2),
        };
        assert!(!lease.runnable_by(node(1)));
        assert!(!lease.runnable_by(node(2)));
        let owned = LeaseState::Owned {
            node: node(1),
            epoch: Epoch::FIRST,
        };
        assert!(owned.runnable_by(node(1)));
        assert!(!owned.runnable_by(node(2)));
    }

    #[test]
    fn token_must_match_job_and_epoch_exactly() {
        let job = JobId::from_bytes([1; 16]);
        let other = JobId::from_bytes([2; 16]);
        let two = Epoch::FIRST.next().unwrap();
        let token = FencingToken {
            job_id: job,
            epoch: Epoch::FIRST,
        };
        assert_eq!(token.check(job, Epoch::FIRST), Ok(()));
        assert_eq!(
            token.check(job, two),
            Err(FenceError::StaleEpoch {
                yours: Epoch::FIRST,
                current: two
            })
        );
        assert_eq!(
            FencingToken {
                job_id: job,
                epoch: two
            }
            .check(job, Epoch::FIRST),
            Err(FenceError::AheadOfUs {
                yours: two,
                current: Epoch::FIRST
            })
        );
        assert_eq!(
            token.check(other, Epoch::FIRST),
            Err(FenceError::WrongJob {
                token: job,
                job: other
            })
        );
    }
}
