//! The cooperative move protocol and the steal (ADR 0003).
//!
//! Invariant: a job leaves `Departing` only through a record the move's
//! target wrote for that `move_id`, the target writes exactly one decision per
//! `move_id` and answers every later question with that same record, and the
//! epoch rises by exactly one on `MoveImported` and on `Stolen` and never
//! otherwise. A lost reply, a repeated offer or a repeated receipt changes
//! nothing, which is what makes two runnable copies impossible.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{Epoch, EventId, MoveId, NodeId, RecordHash, SnapshotId};
use crate::lease::{FencingToken, LeaseState};
use crate::ledger::{EventKind, Ledger, LedgerError, Record};

/// What the owner sends after `MoveStarted` is durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveOffer {
    /// The move.
    pub move_id: MoveId,
    /// The owner's head, which is the `MoveStarted` record.
    pub head: RecordHash,
    /// The snapshot the target must hold completely before it imports.
    pub snapshot: SnapshotId,
}

/// The target's answer to a `MoveOffer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferAnswer {
    /// Already decided; the record was persisted earlier and is sent again as is.
    Decided(Record),
    /// Import now: persist this `MoveImported`, accept it, then send it.
    Import(Record),
    /// The offer does not link to this node's head; the sender fetches the suffix and offers again.
    Rejected {
        /// This node's head.
        head: RecordHash,
    },
    /// The offer carries an older epoch than this node knows.
    Stale {
        /// The offer's epoch.
        yours: Epoch,
        /// The epoch this node knows.
        current: Epoch,
    },
}

/// The target's answer to a `MoveAbort`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortAnswer {
    /// Already decided, either way; the record is sent again as is.
    Decided(Record),
    /// Abort now: persist this `MoveAborted`, accept it, then send it.
    Abort(Record),
}

/// The store's word that every chunk of one snapshot is present locally.
///
/// Only a store that has checked its catalog should make one. The move
/// protocol and the steal compare the named snapshot with the one the ledger
/// requires, so a caller cannot check one snapshot and vouch for another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteSnapshot(SnapshotId);

impl CompleteSnapshot {
    /// Names a snapshot the store verified as complete.
    #[must_use]
    pub const fn new(snapshot: SnapshotId) -> Self {
        Self(snapshot)
    }

    /// Returns the snapshot.
    #[must_use]
    pub const fn id(self) -> SnapshotId {
        self.0
    }
}

/// A steal checked against the rules; print `warning`, then persist and accept `record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealProposal {
    /// The `Stolen` record to persist and accept.
    pub record: Record,
    /// What the user is told before the record is persisted.
    pub warning: StealWarning,
}

/// What a steal costs, for the user to read before it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StealWarning {
    /// The owner of record being stolen from.
    pub owner: NodeId,
    /// The `at` of the last record this node holds, in Unix milliseconds.
    pub last_record_at: u64,
    /// The snapshot the thief will resume from.
    pub snapshot: SnapshotId,
}

impl fmt::Display for StealWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stealing from {} (last record at {} ms); resuming from snapshot {}; any work {} did after that snapshot becomes an orphan on its disk",
            self.owner, self.last_record_at, self.snapshot, self.owner
        )
    }
}

/// How one node relates to a job's lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseView {
    /// This node owns the job and may run it with this token.
    Owner(FencingToken),
    /// This node started a move and waits for the target's decision.
    Departing {
        /// The open move.
        move_id: MoveId,
        /// The node that decides it.
        target: NodeId,
    },
    /// This node handed the job to `owner` in the last move; its copy is a warm cache.
    Departed {
        /// The owner of record.
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// This node holds a replica and nothing else.
    Replica {
        /// The owner of record.
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
}

impl Ledger {
    /// Builds the `MoveStarted` that commits `me` to a move toward `target`.
    ///
    /// Requires the head to be `Suspended`: the workspace is in a published
    /// snapshot and the harness is stopped. From the moment the record is
    /// durable, `me` is not runnable.
    pub fn start_move(
        &self,
        me: NodeId,
        target: NodeId,
        move_id: MoveId,
        id: EventId,
        at: u64,
    ) -> Result<Record, LedgerError> {
        self.propose(me, EventKind::MoveStarted { move_id, target }, id, at)
    }

    /// Returns the offer `me` sends while it is the departing source.
    #[must_use]
    pub fn offer(&self, me: NodeId) -> Option<MoveOffer> {
        match self.lease() {
            LeaseState::Departing { node, move_id, .. } if node == me => Some(MoveOffer {
                move_id,
                head: self.head(),
                snapshot: self.manifest().latest_snapshot?,
            }),
            _ => None,
        }
    }

    /// Returns the decision record this node wrote or accepted for `move_id`, if any.
    #[must_use]
    pub fn decision_for(&self, move_id: MoveId) -> Option<&Record> {
        self.records().iter().find(|r| match &r.event().kind {
            EventKind::MoveImported { move_id: m, .. } | EventKind::MoveAborted { move_id: m } => {
                *m == move_id
            }
            _ => false,
        })
    }

    /// Decides an offer by the three rules of ADR 0003, in order.
    ///
    /// `offered_epoch` is the epoch in the message envelope and `complete`
    /// is the store's word on which snapshot it holds whole, if any. The
    /// answer is pure: `Import` and the new record must be persisted and
    /// accepted by the caller before the reply leaves.
    #[must_use]
    pub fn answer_offer(
        &self,
        me: NodeId,
        offer: &MoveOffer,
        offered_epoch: Epoch,
        complete: Option<CompleteSnapshot>,
        id: EventId,
        at: u64,
    ) -> OfferAnswer {
        if let Some(record) = self.decision_for(offer.move_id) {
            return OfferAnswer::Decided(record.clone());
        }
        let current = self.lease().epoch();
        if offered_epoch < current {
            return OfferAnswer::Stale {
                yours: offered_epoch,
                current,
            };
        }
        let LeaseState::Departing {
            node,
            move_id,
            target,
            ..
        } = self.lease()
        else {
            return OfferAnswer::Rejected { head: self.head() };
        };
        let links = offer.head == self.head()
            && offered_epoch == current
            && move_id == offer.move_id
            && target == me
            && self.manifest().latest_snapshot == Some(offer.snapshot);
        if !links || complete.map(CompleteSnapshot::id) != Some(offer.snapshot) {
            return OfferAnswer::Rejected { head: self.head() };
        }
        match self.propose(
            me,
            EventKind::MoveImported {
                move_id,
                from: node,
            },
            id,
            at,
        ) {
            Ok(record) => OfferAnswer::Import(record),
            Err(_) => OfferAnswer::Rejected { head: self.head() },
        }
    }

    /// Decides an abort request for `move_id` on the target.
    pub fn answer_abort(
        &self,
        me: NodeId,
        move_id: MoveId,
        id: EventId,
        at: u64,
    ) -> Result<AbortAnswer, LedgerError> {
        if let Some(record) = self.decision_for(move_id) {
            return Ok(AbortAnswer::Decided(record.clone()));
        }
        match self.lease() {
            LeaseState::Departing {
                move_id: open,
                target,
                ..
            } if open == move_id => {
                if target != me {
                    return Err(LedgerError::NotTarget { move_id });
                }
                Ok(AbortAnswer::Abort(self.propose(
                    me,
                    EventKind::MoveAborted { move_id },
                    id,
                    at,
                )?))
            }
            _ => Err(LedgerError::UnknownMove(move_id)),
        }
    }

    /// Builds the `Stolen` record for `thief`.
    ///
    /// The `--force` gate is the CLI's (ADR 0003); the core refuses a steal
    /// by the owner itself, which includes a departing source, and a steal
    /// when `complete` is not the latest published snapshot: a steal never
    /// resumes from nothing.
    pub fn steal(
        &self,
        thief: NodeId,
        reason: String,
        complete: Option<CompleteSnapshot>,
        id: EventId,
        at: u64,
    ) -> Result<StealProposal, LedgerError> {
        let owner = self.lease().holder();
        if thief == owner {
            return Err(LedgerError::StealBySelf);
        }
        let snapshot = self
            .manifest()
            .latest_snapshot
            .filter(|latest| complete.map(CompleteSnapshot::id) == Some(*latest))
            .ok_or(LedgerError::StealWithoutSnapshot)?;
        let record = self.propose(
            thief,
            EventKind::Stolen {
                by: thief,
                from: owner,
                reason,
            },
            id,
            at,
        )?;
        Ok(StealProposal {
            record,
            warning: StealWarning {
                owner,
                last_record_at: self.manifest().updated_at,
                snapshot,
            },
        })
    }

    /// Says how `me` relates to the job's lease.
    #[must_use]
    pub fn view(&self, me: NodeId) -> LeaseView {
        match self.lease() {
            LeaseState::Owned { node, epoch } if node == me => LeaseView::Owner(FencingToken {
                job_id: self.job_id(),
                epoch,
            }),
            LeaseState::Departing {
                node,
                move_id,
                target,
                ..
            } if node == me => LeaseView::Departing { move_id, target },
            LeaseState::Owned { node, epoch } | LeaseState::Departing { node, epoch, .. } => {
                let departed = self
                    .records()
                    .iter()
                    .rev()
                    .find_map(|r| match &r.event().kind {
                        EventKind::MoveImported { from, .. } => Some(*from == me),
                        EventKind::Stolen { .. } => Some(false),
                        _ => None,
                    });
                if departed == Some(true) {
                    LeaseView::Departed { owner: node, epoch }
                } else {
                    LeaseView::Replica { owner: node, epoch }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::HarnessKind;
    use crate::id::JobId;
    use crate::ledger::Author;

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn eid(b: u8) -> EventId {
        EventId::from_bytes([b; 16])
    }

    fn snap(b: u8) -> SnapshotId {
        SnapshotId::of(&[b])
    }

    fn append(ledger: &mut Ledger, author: NodeId, kind: EventKind, seq: u8) {
        let record = ledger
            .propose(author, kind, eid(seq), u64::from(seq))
            .unwrap();
        ledger.accept(record, Author::Node(author)).unwrap();
    }

    /// A ledger owned by node 1 with two published snapshots, suspended at the second.
    fn suspended_twice(owner: NodeId) -> Ledger {
        let created = Ledger::created(
            JobId::from_bytes([3; 16]),
            "goal".into(),
            HarnessKind::ClaudeCode,
            owner,
            eid(1),
            1,
        )
        .unwrap();
        let mut ledger = Ledger::replay(JobId::from_bytes([3; 16]), [created]).unwrap();
        append(
            &mut ledger,
            owner,
            EventKind::SnapshotPublished {
                snapshot: snap(1),
                parent: None,
            },
            2,
        );
        append(
            &mut ledger,
            owner,
            EventKind::SnapshotPublished {
                snapshot: snap(2),
                parent: Some(snap(1)),
            },
            3,
        );
        append(
            &mut ledger,
            owner,
            EventKind::Suspended { snapshot: snap(2) },
            4,
        );
        ledger
    }

    #[test]
    fn steal_needs_the_witness_for_the_latest_snapshot() {
        let ledger = suspended_twice(node(1));
        let older = Some(CompleteSnapshot::new(snap(1)));
        assert!(matches!(
            ledger.steal(node(2), String::new(), older, eid(9), 9),
            Err(LedgerError::StealWithoutSnapshot)
        ));
        assert!(matches!(
            ledger.steal(node(2), String::new(), None, eid(9), 9),
            Err(LedgerError::StealWithoutSnapshot)
        ));
        let latest = Some(CompleteSnapshot::new(snap(2)));
        let proposal = ledger
            .steal(node(2), "pro asleep".into(), latest, eid(9), 9)
            .unwrap();
        assert_eq!(proposal.warning.snapshot, snap(2));
        assert_eq!(proposal.warning.owner, node(1));
        assert_eq!(proposal.warning.last_record_at, 4);
    }

    /// ADR 0003, Recovery: while a move is open the source is still the owner
    /// of record, so it cannot steal from itself. With two nodes a vanished
    /// target leaves the job frozen until the target answers or a third node
    /// steals; the alternative is recorded in ADR 0003's open questions.
    #[test]
    fn a_departing_source_cannot_steal_its_own_frozen_move() {
        let mut ledger = suspended_twice(node(1));
        append(
            &mut ledger,
            node(1),
            EventKind::MoveStarted {
                move_id: MoveId::from_bytes([7; 16]),
                target: node(2),
            },
            5,
        );
        let latest = Some(CompleteSnapshot::new(snap(2)));
        assert!(matches!(
            ledger.steal(node(1), String::new(), latest, eid(9), 9),
            Err(LedgerError::StealBySelf)
        ));
        assert!(ledger.runnable_by(node(1)).is_none());
        let third = ledger
            .steal(node(3), String::new(), latest, eid(9), 9)
            .unwrap();
        assert!(matches!(
            third.record.event().kind,
            EventKind::Stolen { by, from, .. } if by == node(3) && from == node(1)
        ));
    }
}
