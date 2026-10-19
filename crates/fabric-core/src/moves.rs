//! The cooperative move protocol and the steal (ADR 0003).
//!
//! Invariant: a job leaves `Departing` only through a record the move's
//! target wrote for that `move_id`, the target writes exactly one decision per
//! `move_id` and answers every later question with that same record, and the
//! epoch rises by exactly one on `MoveImported` and on `Stolen` and never
//! otherwise. A lost reply, a repeated offer or a repeated receipt changes
//! nothing, which is what makes two runnable copies on one chain impossible.
//! A steal can fork the chain: the thief and an owner that kept writing hold
//! two suffixes, and the fork rule of ADR 0003, which a later PR implements,
//! decides which one survives. A departing source is still the owner of
//! record and cannot steal from itself, so a move whose target vanishes
//! stays frozen until the target answers or a third node steals (ADR 0003).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{Epoch, EventId, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use crate::lease::{FencingToken, LeaseState};
use crate::ledger::{EventKind, Ledger, LedgerError, Record};

/// What the owner sends after `MoveStarted` is durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveOffer {
    /// The job the move is for; an offer for another job is refused.
    pub job_id: JobId,
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
    /// Import now: `commit` this `MoveImported`, then send it.
    Import(Record),
    /// The offer does not link to this node's head; the sender pushes the suffix after `head` and offers again.
    Rejected {
        /// This node's head.
        head: RecordHash,
    },
    /// The offer links but the named snapshot is not whole in this node's store; the sender pushes it and offers again.
    SnapshotIncomplete {
        /// The snapshot the sender must push.
        snapshot: SnapshotId,
    },
    /// The offer carries an older epoch than this node knows; the sender fetches first.
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
    /// Abort now: `commit` this `MoveAborted`, then send it.
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

/// A steal checked against the rules; print `warning`, then `commit` `record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealProposal {
    /// The `Stolen` record to commit.
    pub record: Record,
    /// What the user is told before the record is persisted.
    pub warning: StealWarning,
}

/// What a steal costs, for the user to read before it happens.
///
/// `Display` names the owner and the snapshot; the CLI prints
/// `last_record_at` as a date next to it, because the core has no clock
/// and no time zone.
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
            "stealing the job from {owner}; resuming from snapshot {snapshot}; any work {owner} did after that snapshot becomes an orphan on its disk",
            owner = self.owner,
            snapshot = self.snapshot
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
    /// Requires the workspace to be suspended in the latest published
    /// snapshot: the last `Suspended` names it and no `Resumed` followed.
    /// Notes written after the suspend do not block the move. From the
    /// moment the record is durable, `me` is not runnable.
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
                job_id: self.job_id(),
                move_id,
                head: self.head(),
                snapshot: self.manifest().latest_snapshot?,
            }),
            _ => None,
        }
    }

    /// Returns the decision record on this chain for `move_id`, if any.
    ///
    /// There is at most one: the ledger refuses a `MoveStarted` whose id an
    /// earlier move used, and `Departing` has one exit per move.
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
    /// is the store's word on which snapshot it holds whole, if any. Every
    /// `Ok` answer is a protocol step the sender can act on; an `Err` is a
    /// cause the sender cannot fix by retrying, such as an offer for another
    /// job or for a move whose target is another node, and the caller prints
    /// it. The answer is pure: `Import` and the new record must be persisted
    /// through `commit` by the caller before the reply leaves.
    pub fn answer_offer(
        &self,
        me: NodeId,
        offer: &MoveOffer,
        offered_epoch: Epoch,
        complete: Option<CompleteSnapshot>,
        id: EventId,
        at: u64,
    ) -> Result<OfferAnswer, LedgerError> {
        if offer.job_id != self.job_id() {
            return Err(LedgerError::WrongJob {
                found: offer.job_id,
                expected: self.job_id(),
            });
        }
        if let Some(record) = self.decision_for(offer.move_id) {
            return Ok(OfferAnswer::Decided(record.clone()));
        }
        let current = self.lease().epoch();
        if offered_epoch < current {
            return Ok(OfferAnswer::Stale {
                yours: offered_epoch,
                current,
            });
        }
        let LeaseState::Departing {
            node,
            move_id,
            target,
            ..
        } = self.lease()
        else {
            return Ok(OfferAnswer::Rejected { head: self.head() });
        };
        let links = offer.head == self.head()
            && offered_epoch == current
            && move_id == offer.move_id
            && self.manifest().latest_snapshot == Some(offer.snapshot);
        if !links {
            return Ok(OfferAnswer::Rejected { head: self.head() });
        }
        if target != me {
            return Err(LedgerError::NotTarget { move_id });
        }
        if complete.map(CompleteSnapshot::id) != Some(offer.snapshot) {
            return Ok(OfferAnswer::SnapshotIncomplete {
                snapshot: offer.snapshot,
            });
        }
        let record = self.propose(
            me,
            EventKind::MoveImported {
                move_id,
                from: node,
            },
            id,
            at,
        )?;
        Ok(OfferAnswer::Import(record))
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
    /// by the owner itself, which includes a departing source, a steal by
    /// the target of the open move, which must import or abort instead, and
    /// a steal when `complete` is not the latest published snapshot: a steal
    /// never resumes from nothing.
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
        if let LeaseState::Departing {
            move_id, target, ..
        } = self.lease()
            && target == thief
        {
            return Err(LedgerError::StealByTarget { move_id });
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
    use crate::ledger::{Author, MemoryStore};

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn eid(b: u8) -> EventId {
        EventId::from_bytes([b; 16])
    }

    fn snap(b: u8) -> SnapshotId {
        SnapshotId::of(&[b])
    }

    /// Appends through `commit`; these tests read the ledger, not the store.
    fn append(ledger: &mut Ledger, author: NodeId, kind: EventKind, seq: u8) {
        let record = ledger
            .propose(author, kind, eid(seq), u64::from(seq))
            .unwrap();
        ledger
            .commit(&mut MemoryStore::default(), record, Author::Node(author))
            .unwrap();
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

    #[test]
    fn answer_offer_names_each_refusal() {
        let move_id = MoveId::from_bytes([7; 16]);
        let mut on_source = suspended_twice(node(1));
        append(
            &mut on_source,
            node(1),
            EventKind::MoveStarted {
                move_id,
                target: node(2),
            },
            5,
        );
        let offer = on_source.offer(node(1)).unwrap();
        let epoch = on_source.lease().epoch();
        let complete = Some(CompleteSnapshot::new(snap(2)));

        let foreign = MoveOffer {
            job_id: JobId::from_bytes([8; 16]),
            ..offer
        };
        assert!(matches!(
            on_source.answer_offer(node(2), &foreign, epoch, complete, eid(9), 9),
            Err(LedgerError::WrongJob { .. })
        ));

        let behind = suspended_twice(node(1));
        assert!(matches!(
            behind.answer_offer(node(2), &offer, epoch, complete, eid(9), 9),
            Ok(OfferAnswer::Rejected { head }) if head == behind.head()
        ));

        assert!(matches!(
            on_source.answer_offer(node(3), &offer, epoch, complete, eid(9), 9),
            Err(LedgerError::NotTarget { move_id: m }) if m == move_id
        ));

        let older = Some(CompleteSnapshot::new(snap(1)));
        assert!(matches!(
            on_source.answer_offer(node(2), &offer, epoch, older, eid(9), 9),
            Ok(OfferAnswer::SnapshotIncomplete { snapshot }) if snapshot == snap(2)
        ));

        assert!(matches!(
            on_source.answer_offer(node(2), &offer, epoch, complete, eid(9), 9),
            Ok(OfferAnswer::Import(_))
        ));
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

    /// The target holds the two legitimate exits of an open move; a steal by
    /// it would leave the move undecided and the source asking forever.
    #[test]
    fn the_target_of_an_open_move_decides_it_and_cannot_steal() {
        let move_id = MoveId::from_bytes([7; 16]);
        let mut ledger = suspended_twice(node(1));
        append(
            &mut ledger,
            node(1),
            EventKind::MoveStarted {
                move_id,
                target: node(2),
            },
            5,
        );
        let latest = Some(CompleteSnapshot::new(snap(2)));
        assert!(matches!(
            ledger.steal(node(2), String::new(), latest, eid(9), 9),
            Err(LedgerError::StealByTarget { move_id: m }) if m == move_id
        ));
        let relayed = Record::encode(crate::ledger::Event {
            id: eid(9),
            prev: Some(ledger.head()),
            epoch: Epoch::FIRST.next().unwrap(),
            at: 9,
            kind: EventKind::Stolen {
                by: node(2),
                from: node(1),
                reason: String::new(),
            },
        })
        .unwrap();
        assert!(matches!(
            ledger.verify(relayed.bytes(), node(2)),
            Err(LedgerError::StealByTarget { .. })
        ));
    }

    fn imported(ledger: &mut Ledger, source: NodeId, target: NodeId, move_id: MoveId, seq: u8) {
        append(
            ledger,
            source,
            EventKind::MoveStarted { move_id, target },
            seq,
        );
        append(
            ledger,
            target,
            EventKind::MoveImported {
                move_id,
                from: source,
            },
            seq + 1,
        );
    }

    #[test]
    fn view_names_each_side_of_a_move_and_of_the_move_after_it() {
        let (a, b, c) = (node(1), node(2), node(3));
        let move_ab = MoveId::from_bytes([1; 16]);
        let mut ledger = suspended_twice(a);
        assert!(matches!(ledger.view(a), LeaseView::Owner(token) if token.epoch == Epoch::FIRST));
        assert!(matches!(ledger.view(b), LeaseView::Replica { owner, .. } if owner == a));

        append(
            &mut ledger,
            a,
            EventKind::MoveStarted {
                move_id: move_ab,
                target: b,
            },
            5,
        );
        assert!(matches!(
            ledger.view(a),
            LeaseView::Departing { move_id, target } if move_id == move_ab && target == b
        ));
        assert!(matches!(ledger.view(b), LeaseView::Replica { owner, .. } if owner == a));

        append(
            &mut ledger,
            b,
            EventKind::MoveImported {
                move_id: move_ab,
                from: a,
            },
            6,
        );
        let two = Epoch::FIRST.next().unwrap();
        assert!(matches!(
            ledger.view(a),
            LeaseView::Departed { owner, epoch } if owner == b && epoch == two
        ));
        assert!(matches!(ledger.view(b), LeaseView::Owner(token) if token.epoch == two));
        assert!(matches!(ledger.view(c), LeaseView::Replica { owner, .. } if owner == b));

        append(
            &mut ledger,
            b,
            EventKind::SnapshotPublished {
                snapshot: snap(3),
                parent: Some(snap(2)),
            },
            7,
        );
        append(
            &mut ledger,
            b,
            EventKind::Suspended { snapshot: snap(3) },
            8,
        );
        imported(&mut ledger, b, c, MoveId::from_bytes([2; 16]), 9);
        let three = two.next().unwrap();
        assert!(matches!(
            ledger.view(a),
            LeaseView::Replica { owner, epoch } if owner == c && epoch == three
        ));
        assert!(matches!(ledger.view(b), LeaseView::Departed { owner, .. } if owner == c));
        assert!(matches!(ledger.view(c), LeaseView::Owner(_)));
    }

    #[test]
    fn view_after_a_steal_makes_the_old_owner_a_replica() {
        let ledger = suspended_twice(node(1));
        let proposal = ledger
            .steal(
                node(2),
                String::new(),
                Some(CompleteSnapshot::new(snap(2))),
                eid(9),
                9,
            )
            .unwrap();
        let mut ledger = ledger;
        ledger
            .commit(
                &mut MemoryStore::default(),
                proposal.record,
                Author::Node(node(2)),
            )
            .unwrap();
        let two = Epoch::FIRST.next().unwrap();
        assert!(matches!(
            ledger.view(node(1)),
            LeaseView::Replica { owner, epoch } if owner == node(2) && epoch == two
        ));
        assert!(matches!(ledger.view(node(2)), LeaseView::Owner(token) if token.epoch == two));
    }

    #[test]
    fn steal_warning_names_the_owner_and_the_snapshot_in_words() {
        let warning = StealWarning {
            owner: node(1),
            last_record_at: 1_760_000_000_000,
            snapshot: snap(2),
        };
        let text = warning.to_string();
        assert!(text.contains(&node(1).to_string()));
        assert!(text.contains(&snap(2).to_string()));
        assert!(text.contains("orphan"));
        assert!(!text.contains("1760000000000"));
    }
}
