//! The fence (ADR 0028): the grant a runner presents with every covered
//! write, the covered write set, and the one rule every covered path runs
//! before it writes anything.
//!
//! Invariant: `fence` is pure and reads only the job's projection. It
//! answers `Live` for exactly one attempt of a job at a time, the live
//! grant at the job's generation in status `granted` or `running`, seen
//! from the node it was granted to; every other token gets a verdict that
//! names why, and the caller writes nothing for it.

use serde::{Deserialize, Serialize};

use super::ids::{AttemptId, Generation};
use super::model::{AttemptStatus, ManagedState, ResourceRef};
use crate::id::{JobId, NodeId};

/// What a runner presents with every covered write for a managed job: the
/// grant it acts under. The managed counterpart of `FencingToken`; an
/// epoch is never written for a managed job (ADR 0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GrantToken {
    /// The job.
    pub job: JobId,
    /// The attempt.
    pub attempt: AttemptId,
    /// The generation of its grant.
    pub generation: Generation,
}

/// The covered write set. Exactly these four; a test lists them and fails
/// when a variant is added without a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoveredPath {
    /// An `Append` to a managed job's ledger (fabric-net, per-job request).
    LedgerAppend,
    /// A bundle publish the authority accepts as the job's checkpoint
    /// (the `Captured` report and the verifier's fetch).
    BundlePublish,
    /// A journal operation an executor reports: every observation,
    /// acknowledgement and operation mark.
    JournalOperation,
    /// The outcome of a facade command (ADR 0027).
    FacadeOutcome,
}

impl CoveredPath {
    /// Every covered path, in the order the specification lists them.
    pub const ALL: [CoveredPath; 4] = [
        CoveredPath::LedgerAppend,
        CoveredPath::BundlePublish,
        CoveredPath::JournalOperation,
        CoveredPath::FacadeOutcome,
    ];

    /// The name the specification and the evidence use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            CoveredPath::LedgerAppend => "ledger_append",
            CoveredPath::BundlePublish => "bundle_publish",
            CoveredPath::JournalOperation => "journal_operation",
            CoveredPath::FacadeOutcome => "facade_outcome",
        }
    }
}

/// What the fence says about a token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FenceVerdict {
    /// The token names the job's live attempt at the job's generation, in
    /// status `granted` or `running`, from the node it was granted to.
    Live,
    /// The token's generation is older than the job's.
    Stale {
        /// The job's generation.
        current: Generation,
    },
    /// The token's attempt was superseded; `by` is the generation that
    /// took its grant, `None` when it was taken and nothing granted yet.
    Superseded {
        /// The generation granted in its place, if one was.
        by: Option<Generation>,
    },
    /// The attempt is the job's but may not write: `unknown`, or ended.
    NotWritable {
        /// Its status.
        status: AttemptStatus,
    },
    /// The write came over the node link from a node other than the
    /// attempt's resource.
    WrongNode {
        /// The node the attempt was granted to.
        granted: NodeId,
        /// The node the write came from.
        from: NodeId,
    },
    /// The job has no such attempt, or the token's generation is not the attempt's.
    UnknownAttempt,
    /// No managed job with the token's id.
    UnknownJob,
}

impl FenceVerdict {
    /// `Live` alone.
    #[must_use]
    pub fn allows(&self) -> bool {
        matches!(self, FenceVerdict::Live)
    }

    /// `Stale` and `Superseded`: the executor stops the attempt's group.
    #[must_use]
    pub fn is_obsolete(&self) -> bool {
        matches!(
            self,
            FenceVerdict::Stale { .. } | FenceVerdict::Superseded { .. }
        )
    }

    /// The name the evidence uses.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            FenceVerdict::Live => "live",
            FenceVerdict::Stale { .. } => "stale",
            FenceVerdict::Superseded { .. } => "superseded",
            FenceVerdict::NotWritable { .. } => "not_writable",
            FenceVerdict::WrongNode { .. } => "wrong_node",
            FenceVerdict::UnknownAttempt => "unknown_attempt",
            FenceVerdict::UnknownJob => "unknown_job",
        }
    }
}

/// The one rule every covered path runs before it writes anything. `from`
/// is the node the write came from; with `None` the node is not checked.
/// The authority never passes `None`: a write made on its own host is
/// checked as from its own node, so an attempt granted elsewhere is not
/// written by a local path either (ADR 0028).
#[must_use]
pub fn fence(
    state: Option<&ManagedState>,
    token: &GrantToken,
    from: Option<NodeId>,
) -> FenceVerdict {
    let Some(state) = state else {
        return FenceVerdict::UnknownJob;
    };
    if state.job.id != token.job {
        return FenceVerdict::UnknownJob;
    }
    let Some(attempt) = state.attempt(token.attempt) else {
        return FenceVerdict::UnknownAttempt;
    };
    if attempt.generation != token.generation {
        return FenceVerdict::UnknownAttempt;
    }
    if let AttemptStatus::Superseded { by } = attempt.status {
        return FenceVerdict::Superseded { by };
    }
    if token.generation < state.job.generation {
        return FenceVerdict::Stale {
            current: state.job.generation,
        };
    }
    match attempt.status {
        AttemptStatus::Granted | AttemptStatus::Running => {}
        ref status => {
            return FenceVerdict::NotWritable {
                status: status.clone(),
            };
        }
    }
    let ResourceRef::Node { node: granted } = attempt.resource;
    match from {
        Some(from) if from != granted => FenceVerdict::WrongNode { granted, from },
        _ => FenceVerdict::Live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_covered_write_set_is_exactly_four() {
        // A new variant fails to compile here until it is placed in ALL and
        // given a path in the journal, the wire and the authority.
        for path in CoveredPath::ALL {
            let placed = match path {
                CoveredPath::LedgerAppend
                | CoveredPath::BundlePublish
                | CoveredPath::JournalOperation
                | CoveredPath::FacadeOutcome => true,
            };
            assert!(placed);
        }
        assert_eq!(CoveredPath::ALL.len(), 4);
        let names: Vec<_> = CoveredPath::ALL.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            [
                "ledger_append",
                "bundle_publish",
                "journal_operation",
                "facade_outcome"
            ]
        );
    }

    #[test]
    fn only_live_allows_and_only_stale_and_superseded_are_obsolete() {
        let verdicts = [
            FenceVerdict::Live,
            FenceVerdict::Stale {
                current: Generation::FIRST,
            },
            FenceVerdict::Superseded { by: None },
            FenceVerdict::NotWritable {
                status: AttemptStatus::Unknown,
            },
            FenceVerdict::WrongNode {
                granted: NodeId::from_bytes([1; 32]),
                from: NodeId::from_bytes([2; 32]),
            },
            FenceVerdict::UnknownAttempt,
            FenceVerdict::UnknownJob,
        ];
        let allowed: Vec<_> = verdicts.iter().map(FenceVerdict::allows).collect();
        assert_eq!(allowed, [true, false, false, false, false, false, false]);
        let obsolete: Vec<_> = verdicts.iter().map(FenceVerdict::is_obsolete).collect();
        assert_eq!(obsolete, [false, true, true, false, false, false, false]);
    }

    #[test]
    fn no_job_is_unknown_job() {
        let token = GrantToken {
            job: JobId::from_bytes([1; 16]),
            attempt: AttemptId::from_bytes([2; 16]),
            generation: Generation::FIRST,
        };
        assert_eq!(fence(None, &token, None), FenceVerdict::UnknownJob);
    }
}
