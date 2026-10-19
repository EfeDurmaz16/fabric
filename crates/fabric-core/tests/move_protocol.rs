//! Model-based tests of the move protocol over three in-memory nodes.
//!
//! Every interleaving of lost replies, duplicated receipts, repeated offers,
//! aborts, status questions and replica syncs must keep the lease invariants
//! of ADR 0003. The named interleaving is the T3 Code bug: the target
//! imports, the reply is lost, the source retries, and both must converge on
//! the one `MoveImported` without the source ever being runnable.

use std::collections::HashMap;
use std::error::Error;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, EventId, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::{FenceError, FencingToken, LeaseState};
use fabric_core::ledger::{
    Author, EventKind, Ledger, LedgerError, LedgerStore, MemoryStore, Record,
};
use fabric_core::moves::{AbortAnswer, CompleteSnapshot, MoveOffer, OfferAnswer};
use proptest::prelude::*;

const NODES: usize = 3;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

#[derive(Debug, Clone)]
enum Op {
    Suspend {
        node: usize,
    },
    Resume {
        node: usize,
    },
    Note {
        node: usize,
    },
    StartMove {
        src: usize,
        dst: usize,
    },
    Deliver {
        idx: usize,
        snapshot_complete: bool,
    },
    Drop {
        idx: usize,
    },
    Duplicate {
        idx: usize,
    },
    Abort {
        node: usize,
    },
    AskStatus {
        node: usize,
    },
    Sync {
        from: usize,
        to: usize,
    },
    Steal {
        thief: usize,
        snapshot_complete: bool,
    },
}

#[derive(Debug, Clone)]
enum Msg {
    Offer {
        from: usize,
        to: usize,
        offer: MoveOffer,
        epoch: Epoch,
    },
    Decision {
        from: usize,
        to: usize,
        frame: Vec<u8>,
    },
    Abort {
        from: usize,
        to: usize,
        move_id: MoveId,
    },
    Status {
        from: usize,
        to: usize,
        move_id: MoveId,
    },
}

struct Node {
    id: NodeId,
    ledger: Ledger,
    store: MemoryStore,
}

impl Node {
    fn commit(&mut self, record: Record, author: Author) -> Fallible {
        self.ledger.commit(&mut self.store, record, author)?;
        Ok(())
    }

    fn owns(&self) -> bool {
        self.ledger.runnable_by(self.id).is_some()
    }

    fn departing_toward(&self) -> Option<(MoveId, NodeId)> {
        match self.ledger.lease() {
            LeaseState::Departing {
                node,
                move_id,
                target,
                ..
            } if node == self.id => Some((move_id, target)),
            _ => None,
        }
    }
}

struct World {
    nodes: Vec<Node>,
    mail: Vec<Msg>,
    snapshots: u8,
    at: u64,
    steals: u32,
    /// The node that wrote each record, so a sync can deliver writer-direct.
    writers: HashMap<RecordHash, NodeId>,
}

impl World {
    fn new() -> Fallible<World> {
        let job = JobId::from_bytes([42; 16]);
        let ids: Vec<NodeId> = (1..=NODES)
            .map(|i| Ok(NodeId::from_bytes([u8::try_from(i)?; 32])))
            .collect::<Fallible<_>>()?;
        let created = Ledger::created(
            job,
            "goal".into(),
            HarnessKind::ClaudeCode,
            ids[0],
            EventId::from_bytes([0; 16]),
            1,
        )?;
        let mut nodes = Vec::with_capacity(NODES);
        for &id in &ids {
            let mut store = MemoryStore::default();
            store.append(&created)?;
            let ledger = Ledger::open(job, &store)?;
            nodes.push(Node { id, ledger, store });
        }
        let mut writers = HashMap::new();
        writers.insert(created.hash(), ids[0]);
        Ok(World {
            nodes,
            mail: Vec::new(),
            snapshots: 0,
            at: 1,
            steals: 0,
            writers,
        })
    }

    fn tick(&mut self) -> u64 {
        self.at += 1;
        self.at
    }

    /// A fresh id from the world's counter; the edge would use a ULID.
    fn next_id(&mut self) -> [u8; 16] {
        let n = self.tick();
        let mut bytes = [0; 16];
        bytes[8..].copy_from_slice(&n.to_be_bytes());
        bytes
    }

    fn index_of(&self, id: NodeId) -> Fallible<usize> {
        self.nodes
            .iter()
            .position(|n| n.id == id)
            .ok_or_else(|| format!("unknown node {id}").into())
    }

    /// Commits a record `node` wrote itself and remembers it as the writer.
    fn commit_own(&mut self, node: usize, record: Record) -> Fallible {
        let me = self.nodes[node].id;
        self.writers.insert(record.hash(), me);
        self.nodes[node].commit(record, Author::Node(me))
    }

    fn append_local(&mut self, node: usize, kind: EventKind) -> Fallible {
        let at = self.tick();
        let id = EventId::from_bytes(self.next_id());
        let n = &mut self.nodes[node];
        let record = n.ledger.propose(n.id, kind, id, at)?;
        self.commit_own(node, record)
    }

    fn apply(&mut self, op: &Op) -> Fallible {
        match *op {
            Op::Suspend { node } => self.suspend(node % NODES),
            Op::Resume { node } => self.resume(node % NODES),
            Op::Note { node } => {
                let node = node % NODES;
                if self.nodes[node].owns() {
                    self.append_local(node, EventKind::Note { text: "n".into() })?;
                }
                Ok(())
            }
            Op::StartMove { src, dst } => self.start_move(src % NODES, dst % NODES),
            Op::Deliver {
                idx,
                snapshot_complete,
            } => {
                if self.mail.is_empty() {
                    return Ok(());
                }
                let msg = self.mail.remove(idx % self.mail.len());
                self.deliver(msg, snapshot_complete)
            }
            Op::Drop { idx } => {
                if !self.mail.is_empty() {
                    let i = idx % self.mail.len();
                    self.mail.remove(i);
                }
                Ok(())
            }
            Op::Duplicate { idx } => {
                if !self.mail.is_empty() {
                    let m = self.mail[idx % self.mail.len()].clone();
                    self.mail.push(m);
                }
                Ok(())
            }
            Op::Abort { node } => self.abort(node % NODES),
            Op::AskStatus { node } => {
                let node = node % NODES;
                if let Some((move_id, target)) = self.nodes[node].departing_toward() {
                    let to = self.index_of(target)?;
                    self.mail.push(Msg::Status {
                        from: node,
                        to,
                        move_id,
                    });
                }
                Ok(())
            }
            Op::Sync { from, to } => {
                let (from, to) = (from % NODES, to % NODES);
                if from != to {
                    self.sync(from, to)?;
                }
                Ok(())
            }
            Op::Steal {
                thief,
                snapshot_complete,
            } => self.steal(thief % NODES, snapshot_complete),
        }
    }

    fn suspend(&mut self, node: usize) -> Fallible {
        if !self.nodes[node].owns() {
            return Ok(());
        }
        self.snapshots = self.snapshots.wrapping_add(1);
        let snapshot = SnapshotId::of(&[self.snapshots]);
        let parent = self.nodes[node].ledger.manifest().latest_snapshot;
        self.append_local(node, EventKind::SnapshotPublished { snapshot, parent })?;
        self.append_local(node, EventKind::Suspended { snapshot })?;
        Ok(())
    }

    fn resume(&mut self, node: usize) -> Fallible {
        if !self.nodes[node].owns() {
            return Ok(());
        }
        let snapshot = self.nodes[node].ledger.manifest().latest_snapshot;
        let me = self.nodes[node].id;
        self.append_local(
            node,
            EventKind::Resumed {
                node: me,
                snapshot,
                harness: HarnessKind::Codex,
            },
        )?;
        Ok(())
    }

    fn start_move(&mut self, src: usize, dst: usize) -> Fallible {
        if src == dst || !self.nodes[src].owns() {
            return Ok(());
        }
        let me = self.nodes[src].id;
        let target = self.nodes[dst].id;
        let at = self.tick();
        let move_id = MoveId::from_bytes(self.next_id());
        let id = EventId::from_bytes(self.next_id());
        match self.nodes[src]
            .ledger
            .start_move(me, target, move_id, id, at)
        {
            Ok(record) => {
                self.commit_own(src, record)?;
                assert!(!self.nodes[src].owns(), "source runnable after MoveStarted");
                self.send_offer(src, dst);
                Ok(())
            }
            Err(LedgerError::NotSuspended) => Ok(()),
            Err(e) => Err(format!("unexpected start_move error: {e}").into()),
        }
    }

    fn abort(&mut self, node: usize) -> Fallible {
        let me = self.nodes[node].id;
        if let Some((move_id, target)) = self.nodes[node].departing_toward() {
            let to = self.index_of(target)?;
            self.mail.push(Msg::Abort {
                from: node,
                to,
                move_id,
            });
            return Ok(());
        }
        if let LeaseState::Departing {
            move_id, target, ..
        } = self.nodes[node].ledger.lease()
            && target == me
        {
            let at = self.tick();
            let id = EventId::from_bytes(self.next_id());
            match self.nodes[node].ledger.answer_abort(me, move_id, id, at) {
                Ok(AbortAnswer::Abort(record)) => {
                    self.commit_own(node, record)?;
                }
                Ok(AbortAnswer::Decided(_)) | Err(LedgerError::UnknownMove(_)) => {}
                Err(e) => return Err(format!("unexpected abort error: {e}").into()),
            }
        }
        Ok(())
    }

    fn send_offer(&mut self, src: usize, dst: usize) {
        let me = self.nodes[src].id;
        if let Some(offer) = self.nodes[src].ledger.offer(me) {
            let epoch = self.nodes[src].ledger.lease().epoch();
            self.mail.push(Msg::Offer {
                from: src,
                to: dst,
                offer,
                epoch,
            });
        }
    }

    /// `to` fetches the suffix it lacks from `from`, and each record is
    /// delivered as if its writer sent it, so the writer rule is exercised
    /// on every replicated record. A fork leaves both as they are.
    fn sync(&mut self, from: usize, to: usize) -> Fallible {
        let head = self.nodes[to].ledger.head();
        let Some(suffix) = self.nodes[from].ledger.suffix_after(head) else {
            return Ok(());
        };
        let suffix = suffix.to_vec();
        for record in suffix {
            let writer = *self
                .writers
                .get(&record.hash())
                .ok_or("record with no known writer")?;
            let checked = self.nodes[to].ledger.verify(record.bytes(), writer)?;
            self.nodes[to].commit(checked, Author::Node(writer))?;
        }
        assert_eq!(
            self.nodes[to].ledger.lease(),
            self.nodes[from].ledger.lease(),
            "synced replicas disagree on the lease"
        );
        Ok(())
    }

    fn deliver(&mut self, msg: Msg, snapshot_complete: bool) -> Fallible {
        match msg {
            Msg::Offer {
                from,
                to,
                offer,
                epoch,
            } => self.deliver_offer(from, to, offer, epoch, snapshot_complete),
            Msg::Decision { from, to, frame } => self.deliver_decision(from, to, &frame),
            Msg::Abort { from, to, move_id } => {
                let me = self.nodes[to].id;
                let at = self.tick();
                let id = EventId::from_bytes(self.next_id());
                match self.nodes[to].ledger.answer_abort(me, move_id, id, at) {
                    Ok(AbortAnswer::Abort(record)) => {
                        self.commit_own(to, record.clone())?;
                        self.reply(to, from, &record);
                    }
                    Ok(AbortAnswer::Decided(record)) => self.reply(to, from, &record),
                    Err(LedgerError::UnknownMove(_)) => {}
                    Err(e) => return Err(format!("unexpected abort error: {e}").into()),
                }
                Ok(())
            }
            Msg::Status { from, to, move_id } => {
                if let Some(record) = self.nodes[to].ledger.decision_for(move_id).cloned() {
                    self.reply(to, from, &record);
                }
                Ok(())
            }
        }
    }

    fn reply(&mut self, from: usize, to: usize, record: &Record) {
        self.mail.push(Msg::Decision {
            from,
            to,
            frame: record.bytes().to_vec(),
        });
    }

    fn deliver_offer(
        &mut self,
        from: usize,
        to: usize,
        offer: MoveOffer,
        epoch: Epoch,
        snapshot_complete: bool,
    ) -> Fallible {
        let me = self.nodes[to].id;
        let current = self.nodes[to].ledger.lease().epoch();
        let at = self.tick();
        let id = EventId::from_bytes(self.next_id());
        let complete = snapshot_complete.then_some(CompleteSnapshot::new(offer.snapshot));
        let answer = self.nodes[to]
            .ledger
            .answer_offer(me, &offer, epoch, complete, id, at)?;
        if epoch < current && self.nodes[to].ledger.decision_for(offer.move_id).is_none() {
            assert!(
                matches!(answer, OfferAnswer::Stale { .. }),
                "stale offer not refused"
            );
        }
        match answer {
            OfferAnswer::Import(record) => {
                self.commit_own(to, record.clone())?;
                self.reply(to, from, &record);
            }
            OfferAnswer::Decided(record) => self.reply(to, from, &record),
            OfferAnswer::Rejected { .. } | OfferAnswer::SnapshotIncomplete { .. } => {
                // The source pushes what the target lacks and offers again.
                let still_open = self.nodes[from].departing_toward().map(|(m, _)| m);
                if still_open == Some(offer.move_id) {
                    self.sync(from, to)?;
                    self.send_offer(from, to);
                }
            }
            OfferAnswer::Stale { .. } => {}
        }
        Ok(())
    }

    /// A decision is accepted when it links; the only refusal the protocol
    /// allows is `PrevMismatch`, for a receipt already applied or a chain that
    /// a steal forked. Any other refusal is a bug in the rules.
    fn deliver_decision(&mut self, from: usize, to: usize, frame: &[u8]) -> Fallible {
        let writer = self.nodes[from].id;
        let err = match self.nodes[to].ledger.verify(frame, writer) {
            Ok(record) => return self.nodes[to].commit(record, Author::Node(writer)),
            Err(err) => err,
        };
        let decoded = Record::decode(frame)?;
        let move_id = match &decoded.event().kind {
            EventKind::MoveImported { move_id, .. } | EventKind::MoveAborted { move_id } => {
                *move_id
            }
            other => return Err(format!("decision carries {}", other.name()).into()),
        };
        let already = self.nodes[to].ledger.decision_for(move_id).is_some();
        let forked = self.steals > 0;
        assert!(
            already || forked,
            "a fresh decision on a common chain was refused: {err}"
        );
        assert!(
            matches!(err, LedgerError::PrevMismatch { .. }),
            "a duplicate or forked decision was refused for another reason: {err}"
        );
        Ok(())
    }

    /// `snapshot_complete` models a store that holds the latest snapshot whole.
    fn steal(&mut self, thief: usize, snapshot_complete: bool) -> Fallible {
        let me = self.nodes[thief].id;
        let before = self.nodes[thief].ledger.lease();
        let head = self.nodes[thief].ledger.head();
        let complete = self.nodes[thief]
            .ledger
            .manifest()
            .latest_snapshot
            .filter(|_| snapshot_complete)
            .map(CompleteSnapshot::new);
        let at = self.tick();
        let id = EventId::from_bytes(self.next_id());
        let result = self.nodes[thief]
            .ledger
            .steal(me, "test".into(), complete, id, at);
        match result {
            Ok(proposal) => {
                assert!(snapshot_complete);
                assert_ne!(before.holder(), me);
                self.commit_own(thief, proposal.record)?;
                self.steals += 1;
                let after = self.nodes[thief].ledger.lease();
                let next = before.epoch().next().ok_or("epoch exhausted")?;
                assert_eq!(
                    after,
                    LeaseState::Owned {
                        node: me,
                        epoch: next
                    }
                );
                assert_eq!(proposal.warning.owner, before.holder());
            }
            Err(LedgerError::StealBySelf) => assert_eq!(before.holder(), me),
            Err(LedgerError::StealByTarget { .. }) => {
                assert!(matches!(before, LeaseState::Departing { target, .. } if target == me));
            }
            Err(LedgerError::StealWithoutSnapshot) => {
                let none = self.nodes[thief]
                    .ledger
                    .manifest()
                    .latest_snapshot
                    .is_none();
                assert!(!snapshot_complete || none);
            }
            Err(e) => return Err(format!("unexpected steal error: {e}").into()),
        }
        if self.nodes[thief].ledger.head() == head {
            assert_eq!(
                self.nodes[thief].ledger.lease(),
                before,
                "a refused steal changed the lease"
            );
        }
        Ok(())
    }

    fn check_invariants(&self, allow_steal: bool) -> Fallible {
        let owners: Vec<usize> = (0..NODES).filter(|&i| self.nodes[i].owns()).collect();
        if !allow_steal {
            assert!(owners.len() <= 1, "two runnable copies: {owners:?}");
        }
        for (i, a) in self.nodes.iter().enumerate() {
            for (j, b) in self.nodes.iter().enumerate() {
                if i == j {
                    continue;
                }
                let common_chain = a.ledger.suffix_after(b.ledger.head()).is_some()
                    || b.ledger.suffix_after(a.ledger.head()).is_some();
                if common_chain && a.owns() && b.owns() {
                    assert_ne!(
                        a.ledger.lease().epoch(),
                        b.ledger.lease().epoch(),
                        "two owners at one epoch on a common chain"
                    );
                }
                let mine = a.ledger.lease().epoch();
                let theirs = b.ledger.lease().epoch();
                if theirs < mine {
                    let token = FencingToken {
                        job_id: b.ledger.job_id(),
                        epoch: theirs,
                    };
                    assert_eq!(
                        a.ledger.fence(token),
                        Err(FenceError::StaleEpoch {
                            yours: theirs,
                            current: mine
                        })
                    );
                }
            }
        }
        for node in &self.nodes {
            check_chain(&node.ledger)?;
        }
        Ok(())
    }

    /// Delivers everything, and for a frozen move does what the user would:
    /// offer again under the same move id until the target has decided.
    fn settle(&mut self) -> Fallible {
        for _ in 0..8 {
            while !self.mail.is_empty() {
                let msg = self.mail.remove(0);
                self.deliver(msg, true)?;
            }
            for src in 0..NODES {
                if let Some((_, target)) = self.nodes[src].departing_toward() {
                    let dst = self.index_of(target)?;
                    self.sync(src, dst)?;
                    self.send_offer(src, dst);
                }
            }
            for from in 0..NODES {
                for to in 0..NODES {
                    if from != to {
                        self.sync(from, to)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Epochs never go back, every bump is exactly one and happens once per
/// epoch, every move has at most one decision, and replay agrees with the
/// live manifest.
fn check_chain(ledger: &Ledger) -> Fallible {
    let records = ledger.records();
    let mut epoch = Epoch::FIRST;
    let mut seen_moves = Vec::new();
    let mut bumps = Vec::new();
    for record in records {
        let event = record.event();
        assert!(event.epoch >= epoch, "epoch went backwards");
        if event.kind.bumps_epoch() {
            let next = epoch.next().ok_or("epoch exhausted")?;
            assert_eq!(event.epoch, next, "bump is not exactly one");
            assert!(!bumps.contains(&event.epoch), "two bumps at one epoch");
            bumps.push(event.epoch);
        }
        epoch = event.epoch;
        if let EventKind::MoveImported { move_id, .. } | EventKind::MoveAborted { move_id } =
            &event.kind
        {
            assert!(!seen_moves.contains(move_id), "two decisions for one move");
            seen_moves.push(*move_id);
        }
    }
    let replayed = Ledger::replay(ledger.job_id(), records.iter().cloned())?;
    assert_eq!(replayed.manifest(), ledger.manifest());
    Ok(())
}

fn op_strategy(allow_steal: bool) -> BoxedStrategy<Op> {
    let n = 0..NODES;
    let base = prop_oneof![
        n.clone().prop_map(|node| Op::Suspend { node }),
        n.clone().prop_map(|node| Op::Resume { node }),
        n.clone().prop_map(|node| Op::Note { node }),
        (n.clone(), n.clone()).prop_map(|(src, dst)| Op::StartMove { src, dst }),
        (any::<usize>(), any::<bool>()).prop_map(|(idx, snapshot_complete)| Op::Deliver {
            idx,
            snapshot_complete
        }),
        any::<usize>().prop_map(|idx| Op::Drop { idx }),
        any::<usize>().prop_map(|idx| Op::Duplicate { idx }),
        n.clone().prop_map(|node| Op::Abort { node }),
        n.clone().prop_map(|node| Op::AskStatus { node }),
        (n.clone(), n.clone()).prop_map(|(from, to)| Op::Sync { from, to }),
    ];
    if allow_steal {
        prop_oneof![
            8 => base,
            1 => (n, any::<bool>()).prop_map(|(thief, snapshot_complete)| {
                Op::Steal { thief, snapshot_complete }
            }),
        ]
        .boxed()
    } else {
        base.boxed()
    }
}

fn run(ops: &[Op], allow_steal: bool) -> Fallible<World> {
    let mut world = World::new()?;
    for op in ops {
        world.apply(op)?;
        world.check_invariants(allow_steal)?;
    }
    Ok(world)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn never_two_runnable_copies_without_a_steal(ops in proptest::collection::vec(op_strategy(false), 1..80)) {
        let mut world = run(&ops, false).unwrap();
        world.settle().unwrap();
        world.check_invariants(false).unwrap();
        let heads: Vec<_> = world.nodes.iter().map(|n| n.ledger.head()).collect();
        prop_assert!(heads.iter().all(|h| *h == heads[0]), "replicas did not converge");
        let owners = world.nodes.iter().filter(|n| n.owns()).count();
        prop_assert_eq!(owners, 1, "exactly one owner after settling");
    }

    #[test]
    fn steal_bumps_the_epoch_exactly_once_and_stale_is_refused(ops in proptest::collection::vec(op_strategy(true), 1..80)) {
        run(&ops, true).unwrap();
    }
}

/// The T3 Code interleaving: the target imports, the reply is lost, the
/// source retries under the same move id and never runs in between.
#[test]
fn lost_decision_converges_on_the_one_import() {
    let mut world = run(
        &[
            Op::Suspend { node: 0 },
            Op::StartMove { src: 0, dst: 1 },
            Op::Sync { from: 0, to: 1 },
            Op::Deliver {
                idx: 0,
                snapshot_complete: true,
            },
        ],
        false,
    )
    .unwrap();
    assert!(!world.nodes[0].owns());
    assert!(world.nodes[1].owns(), "target imported");
    assert_eq!(world.mail.len(), 1, "one decision in flight");
    world.apply(&Op::Drop { idx: 0 }).unwrap();
    assert!(
        !world.nodes[0].owns(),
        "source stays frozen after a lost reply"
    );
    world.apply(&Op::AskStatus { node: 0 }).unwrap();
    world
        .apply(&Op::Deliver {
            idx: 0,
            snapshot_complete: true,
        })
        .unwrap();
    world
        .apply(&Op::Deliver {
            idx: 0,
            snapshot_complete: true,
        })
        .unwrap();
    assert_eq!(world.nodes[0].ledger.lease(), world.nodes[1].ledger.lease());
    assert_eq!(world.nodes[0].ledger.head(), world.nodes[1].ledger.head());
    assert!(!world.nodes[0].owns());
    assert!(world.nodes[1].owns());
    assert_eq!(
        world.nodes[1].ledger.lease().epoch(),
        Epoch::FIRST.next().unwrap()
    );
    world.check_invariants(false).unwrap();
}

/// An abort after an import is answered with the import, never a second decision.
#[test]
fn abort_after_import_returns_the_import() {
    let mut world = run(
        &[
            Op::Suspend { node: 0 },
            Op::StartMove { src: 0, dst: 1 },
            Op::Sync { from: 0, to: 1 },
            Op::Deliver {
                idx: 0,
                snapshot_complete: true,
            },
            Op::Drop { idx: 0 },
            Op::Abort { node: 0 },
            Op::Deliver {
                idx: 0,
                snapshot_complete: true,
            },
            Op::Deliver {
                idx: 0,
                snapshot_complete: true,
            },
        ],
        false,
    )
    .unwrap();
    assert!(world.nodes[1].owns());
    assert!(!world.nodes[0].owns());
    let imported = world.nodes[1]
        .ledger
        .records()
        .iter()
        .find_map(|r| match &r.event().kind {
            EventKind::MoveImported { move_id, .. } => Some(*move_id),
            _ => None,
        })
        .unwrap();
    assert!(world.nodes[0].ledger.decision_for(imported).is_some());
    world.check_invariants(false).unwrap();
    world.settle().unwrap();
}

/// ADR 0003, The steal: the stolen-from owner is offered the `Stolen` record,
/// accepts it through the writer rule because the thief wrote it, learns the
/// higher epoch and is fenced; its old token is stale everywhere.
#[test]
fn an_owner_that_receives_stolen_through_verify_is_fenced() {
    let mut world = run(
        &[
            Op::Suspend { node: 0 },
            Op::Sync { from: 0, to: 1 },
            Op::Steal {
                thief: 1,
                snapshot_complete: true,
            },
        ],
        true,
    )
    .unwrap();
    assert!(
        world.nodes[0].owns(),
        "the owner has not heard of the steal"
    );
    assert!(world.nodes[1].owns(), "the thief owns at epoch 2");
    let old_token = world.nodes[0]
        .ledger
        .runnable_by(world.nodes[0].id)
        .unwrap();
    let stolen = world.nodes[1].ledger.records().last().unwrap().clone();
    assert!(matches!(stolen.event().kind, EventKind::Stolen { .. }));
    let thief = world.nodes[1].id;
    assert!(
        world.nodes[0]
            .ledger
            .verify(stolen.bytes(), world.nodes[0].id)
            .is_err(),
        "a Stolen record is accepted only from the thief"
    );
    let checked = world.nodes[0].ledger.verify(stolen.bytes(), thief).unwrap();
    world.nodes[0].commit(checked, Author::Node(thief)).unwrap();
    assert!(!world.nodes[0].owns(), "the old owner is fenced");
    assert_eq!(
        world.nodes[0].ledger.fence(old_token),
        Err(FenceError::StaleEpoch {
            yours: Epoch::FIRST,
            current: Epoch::FIRST.next().unwrap()
        })
    );
    assert_eq!(world.nodes[0].ledger.lease(), world.nodes[1].ledger.lease());
    world.check_invariants(true).unwrap();
}
