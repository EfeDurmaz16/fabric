//! Catching up with paired peers: learning the records they wrote while
//! this node was away or asleep, and failing closed when a peer's ledger
//! forked from this one.
//!
//! Invariant: a node that finds a peer's ledger for a job forked from its
//! own at an epoch at least as high freezes the job before it does anything
//! else with it: it records the fork, stops the harness it recorded, and
//! refuses every command that would run the job from then on. The fork rule
//! of ADR 0003 is not built, so a node keeps running a forked job only when
//! the peer's epoch is lower. Catching up runs at start, whenever a session
//! with a peer begins, and before a run; a session that begins while the
//! start-up pass runs is caught up when that pass ends, and one that begins
//! while a catch-up of its peer runs makes that catch-up run once more. A
//! peer that does not answer is skipped for the rest of a pass rather than
//! dialled again for every job.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use fabric_client::api::Event;
use fabric_core::id::{Epoch, JobId, NodeId, RecordHash};
use fabric_core::lease::FencingToken;
use fabric_core::ledger::{Ledger, LedgerError, Record};
use fabric_core::moves::LeaseView;
use fabric_net::wire::{Fenced, Pulled};

use crate::error::NodeError;
use crate::job::Job;
use crate::local::ForkRecord;
use crate::node::{Node, Out, lock};
use crate::ops::SHORT_DEADLINE;

/// How long a run waits for each peer to say whether the job changed hands.
pub const CHECK_DEADLINE: Duration = Duration::from_secs(3);

impl Node {
    /// The peers this node can call now: those with an address it can
    /// dial, and those that hold a session open with it.
    pub(crate) fn reachable_peers(&self) -> Vec<NodeId> {
        self.peers
            .snapshot()
            .iter()
            .filter(|entry| !entry.addrs().is_empty() || self.open_session(entry.node()).is_some())
            .map(fabric_net::peers::PeerEntry::node)
            .collect()
    }

    /// Settles every job after a restart: learns what the paired peers
    /// wrote while this node was away, fences stray harnesses, moves
    /// departed copies aside and asks the target of an open move for its
    /// decision. Each peer is caught up on its own task, and a peer that
    /// could not be reached is not dialled again in this pass.
    pub(crate) async fn recover(self: Arc<Self>) {
        let mut tasks = tokio::task::JoinSet::new();
        for peer in self.reachable_peers() {
            let node = Arc::clone(&self);
            tasks.spawn(async move { (peer, node.catch_up_peer(peer).await) });
        }
        let mut unreachable = BTreeSet::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((_, Ok(()))) => {}
                Ok((peer, Err(e))) => {
                    tracing::info!(%peer, error = %e, "did not catch up with the peer");
                    unreachable.insert(peer);
                }
                Err(e) => tracing::warn!(error = %e, "a catch-up task ended early"),
            }
        }
        let ids: Vec<JobId> = self.jobs.lock().await.keys().copied().collect();
        for id in ids {
            let mut out = Out::default();
            if let Err(e) = self.settle_job(id, &mut out).await {
                tracing::warn!(job = %id, error = %e, "recovery left the job as it was");
            }
            let target = match self.job(id).await {
                Ok(job) => match job.lock().await.view(self.id) {
                    LeaseView::Departing { target, .. } => Some(target),
                    _ => None,
                },
                Err(_) => None,
            };
            match target {
                Some(target) if unreachable.contains(&target) => {
                    out.emit(Event::MoveWaits { target });
                }
                Some(_) => {
                    if let Err(e) = self.resolve_departing(id, &mut out).await {
                        tracing::info!(job = %id, error = %e, "the open move stays open");
                    }
                }
                None => {}
            }
            for line in out.lines() {
                tracing::info!(job = %id, "{line}");
            }
        }
        self.end_recovery();
    }

    /// Marks start-up recovery done and catches up with every peer whose
    /// session began while it ran, such as an addressless peer that dialed
    /// in after recovery listed the reachable peers.
    ///
    /// A peer recovery already caught up with is caught up again. Its
    /// catch-up may have ended before its session began, with a steal
    /// written in between, and a session recovery's own dial opened looks
    /// the same as a later one. The extra pass costs one pull per job.
    ///
    /// The flag is set under the same lock `spawn_catch_up` checks it
    /// under, so a session that begins at any moment is either queued here
    /// or caught up on its own, never neither. A catch-up of the same peer
    /// that already runs then runs once more (`spawn_catch_up`).
    pub(crate) fn end_recovery(self: &Arc<Self>) {
        let waiting = {
            let mut waiting = lock(&self.waiting_catch_up);
            self.recovered.send_replace(true);
            std::mem::take(&mut *waiting)
        };
        for peer in waiting {
            self.spawn_catch_up(peer);
        }
    }

    /// Whether the start-up recovery has run.
    pub(crate) fn is_recovered(&self) -> bool {
        *self.recovered.borrow()
    }

    /// Pulls every job this node knows from `peer`, fetches its latest
    /// snapshot when the store lacks it, and settles each one.
    ///
    /// It stops at the first failure to reach the peer, so an unreachable
    /// peer costs one dial per pass and not one per job. Any other failure
    /// is logged for that job and the next job is tried.
    async fn catch_up_peer(self: &Arc<Self>, peer: NodeId) -> Result<(), NodeError> {
        let ids: Vec<JobId> = self.jobs.lock().await.keys().copied().collect();
        for id in ids {
            let mut out = Out::default();
            // A pull, unlike a push, fetches no snapshot, and the next
            // pull finds the ledger up to date; the latest snapshot is
            // fetched here so the replica can restore or steal the job.
            let caught_up = match self.pull_records(id, peer).await {
                Ok(()) => self.ensure_latest(id, peer).await,
                Err(e) => Err(e),
            };
            match caught_up {
                Ok(()) => {}
                Err(e @ (NodeError::Unreachable { .. } | NodeError::UnknownPeer(_))) => {
                    return Err(e);
                }
                Err(e) => out.emit(Event::NotCaughtUp {
                    peer,
                    error: e.to_refusal(),
                }),
            }
            if let Err(e) = self.settle_job(id, &mut out).await {
                out.emit(Event::LeftAsItWas {
                    error: e.to_refusal(),
                });
            }
            for line in out.lines() {
                tracing::info!(job = %id, "{line}");
            }
        }
        Ok(())
    }

    /// Catches up with `peer` in the background once a session with it
    /// begins. While start-up recovery runs, the peer waits for it
    /// instead, so the catch-up never acts on a ledger recovery has not
    /// settled, and is not lost when recovery listed the reachable peers
    /// before this session began.
    ///
    /// When a catch-up of the peer already runs, it runs once more when it
    /// ends. It may have pulled a job before the peer wrote to it, or be
    /// waiting on a session that died, which an addressless peer cannot be
    /// dialled to replace.
    pub(crate) fn spawn_catch_up(self: &Arc<Self>, peer: NodeId) {
        {
            let mut waiting = lock(&self.waiting_catch_up);
            if !self.is_recovered() {
                waiting.insert(peer);
                return;
            }
        }
        {
            let mut catching_up = lock(&self.catching_up);
            if let Some(again) = catching_up.get_mut(&peer) {
                *again = true;
                return;
            }
            catching_up.insert(peer, false);
        }
        let node = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(e) = node.catch_up_peer(peer).await {
                    tracing::info!(%peer, error = %e, "did not catch up with the peer");
                }
                let mut catching_up = lock(&node.catching_up);
                match catching_up.get_mut(&peer) {
                    Some(again) if *again => *again = false,
                    _ => {
                        catching_up.remove(&peer);
                        return;
                    }
                }
            }
        });
    }

    /// Before a launch: asks each reachable peer, within `CHECK_DEADLINE`,
    /// for the records it holds after this node's head, so a node that
    /// stayed up through a steal or a move learns it before it runs the job.
    /// A peer that does not answer in time is named and skipped, and so is
    /// one this node cannot ask at all: paired with no address and with no
    /// session open, it may have stolen the job while this node was away.
    pub(crate) async fn check_peers(self: &Arc<Self>, id: JobId, out: &mut Out) {
        let reachable = self.reachable_peers();
        for entry in self.peers.snapshot().iter() {
            if !reachable.contains(&entry.node()) {
                out.emit(Event::PeerNotAsked { peer: entry.node() });
            }
        }
        let mut tasks = tokio::task::JoinSet::new();
        for peer in reachable {
            let node = Arc::clone(self);
            tasks.spawn(async move {
                let pulled =
                    tokio::time::timeout(CHECK_DEADLINE, node.pull_records(id, peer)).await;
                (peer, pulled)
            });
        }
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((_, Ok(Ok(())))) => {}
                Ok((peer, Ok(Err(e)))) => out.emit(Event::NotCaughtUp {
                    peer,
                    error: e.to_refusal(),
                }),
                Ok((peer, Err(_))) => out.emit(Event::PeerSilent {
                    peer,
                    waited_s: CHECK_DEADLINE.as_secs(),
                }),
                Err(e) => tracing::warn!(error = %e, "a catch-up task ended early"),
            }
        }
        if let Err(e) = self.settle_job(id, out).await {
            out.emit(Event::LeftAsItWas {
                error: e.to_refusal(),
            });
        }
    }

    /// Handles a peer whose head is not in this node's chain for `id`.
    ///
    /// When the peer's ledger reached an epoch at least as high as this
    /// node's, the job is frozen here and its harness stopped; otherwise the
    /// peer is the side that must stop. The returned error says which.
    pub(crate) async fn forked_from(
        self: &Arc<Self>,
        id: JobId,
        peer: NodeId,
        head: RecordHash,
    ) -> NodeError {
        let theirs = match self.peer_epoch(id, peer).await {
            Ok(epoch) => epoch,
            Err(e) => return e,
        };
        let mine = match self.job(id).await {
            Ok(job) => job.lock().await.ledger().lease().epoch(),
            Err(e) => return e,
        };
        if theirs < mine {
            return NodeError::ForkBehind {
                job: id,
                peer,
                epoch: theirs,
            };
        }
        let mut out = Out::default();
        let frozen = self
            .freeze(
                id,
                &ForkRecord {
                    peer,
                    epoch: theirs,
                    head,
                },
                &mut out,
            )
            .await;
        for line in out.lines() {
            tracing::warn!(job = %id, "{line}");
        }
        match frozen {
            Ok(()) => NodeError::Forked {
                job: id,
                peer,
                epoch: theirs,
            },
            Err(e) => e,
        }
    }

    /// Freezes the job on this node and stops the harness it recorded.
    pub(crate) async fn freeze(
        &self,
        id: JobId,
        fork: &ForkRecord,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        self.freeze_locked(&mut job, fork, out).await
    }

    /// Freezes `job`, whose lock the caller holds, and stops its harness.
    ///
    /// The fork rule is applied again under the lock: a steal or an import
    /// since the caller read this node's epoch can carry it past the
    /// peer's, and then the peer is the side that stops (`ForkBehind`).
    pub(crate) async fn freeze_locked(
        &self,
        job: &mut Job,
        fork: &ForkRecord,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        if fork.epoch < job.ledger().lease().epoch() {
            return Err(NodeError::ForkBehind {
                job: job.id(),
                peer: fork.peer,
                epoch: fork.epoch,
            });
        }
        if job.freeze(fork)? {
            out.emit(Event::Frozen {
                peer: fork.peer,
                epoch: fork.epoch,
            });
        }
        self.settle(job, out).await
    }

    /// Applies the fork rule of ADR 0003 to a job frozen here after a fork
    /// (`fabric job repair`): reads the peer's whole chain, and when the
    /// peer's chain wins, takes it, keeping this node's ledger whole beside
    /// it and pinning the snapshots only that ledger published, then lifts
    /// the freeze and settles the job, so a clean workspace moves aside as
    /// a departed copy and one with changes stays, an orphan to export.
    /// When the chains no longer fork the freeze is lifted. When this
    /// node's chain wins the job stays frozen and the refusal says to
    /// repair on the peer first, which takes this chain; then the chains
    /// no longer fork here.
    ///
    /// The lock is let go while the peer is read, and the repair stops if
    /// the ledger moved meanwhile.
    pub(crate) async fn job_repair(
        self: &Arc<Self>,
        id: JobId,
        out: &mut Out,
    ) -> Result<fabric_client::api::Repaired, NodeError> {
        use fabric_client::api::{RepairOutcome, Repaired};
        let (fork, ours) = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            let fork = job.forked()?.ok_or(NodeError::NotFrozen { job: id })?;
            (fork, job.ledger().records().to_vec())
        };
        let peer = fork.peer;
        let theirs = self
            .peer_chain(id, peer)
            .await?
            .ok_or_else(|| NodeError::Peer {
                peer,
                reason: format!("it holds no record of job {id}"),
            })?;
        let decided = fork_rule(&ours, theirs.records()).map_err(|e| NodeError::Peer {
            peer,
            reason: e.to_string(),
        })?;
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        if ours.last().map(Record::hash) != Some(job.ledger().head()) {
            return Err(NodeError::ChangedDuringRepair { job: id });
        }
        let outcome = match decided {
            Fork::None => {
                job.unfreeze()?;
                RepairOutcome::Unforked
            }
            Fork::After { ours: true, .. } => {
                return Err(NodeError::ForkWon {
                    job: id,
                    peer,
                    epoch: chain_top(&theirs),
                });
            }
            Fork::After { at, ours: false } => {
                if let Some(record) = job.harness_record()?
                    && record.liveness().await != crate::local::Liveness::Gone
                {
                    return Err(NodeError::AlreadyRunningHarness {
                        job: id,
                        pid: record.pid,
                    });
                }
                let orphans: Vec<fabric_core::id::SnapshotId> = ours[at + 1..]
                    .iter()
                    .filter_map(|record| match record.event().kind {
                        fabric_core::ledger::EventKind::SnapshotPublished { snapshot, .. } => {
                            Some(snapshot)
                        }
                        _ => None,
                    })
                    .filter(|snapshot| !theirs.records().iter().any(|r| {
                        matches!(r.event().kind, fabric_core::ledger::EventKind::SnapshotPublished { snapshot: s, .. } if s == *snapshot)
                    }))
                    .collect();
                let kept = job.dir().forked_ledger(ours[at].hash());
                let mut pins = Vec::with_capacity(orphans.len());
                for snapshot in &orphans {
                    pins.push(crate::local::Pin {
                        snapshot: *snapshot,
                        chain: self.bundle_chain(*snapshot).await?,
                        why: crate::local::PinWhy::ForkLost { kept: kept.clone() },
                    });
                }
                job.pin(pins)?;
                job.adopt(&theirs, &kept)?;
                job.reconcile_marker()?;
                self.settle(&mut job, out).await?;
                RepairOutcome::Adopted {
                    epoch: job.ledger().lease().epoch(),
                    kept,
                    orphans,
                }
            }
        };
        drop(job);
        // The chain taken names the peer's latest snapshot, which a restore
        // or a steal here needs; a failure leaves it to the next catch-up.
        if matches!(outcome, RepairOutcome::Adopted { .. })
            && let Err(e) = self.ensure_latest(id, peer).await
        {
            out.emit(Event::NotCaughtUp {
                peer,
                error: e.to_refusal(),
            });
        }
        Ok(Repaired { peer, outcome })
    }

    /// `snapshot` and each bundle base it needs, in order, to its full
    /// bundle, as this node's store holds them; a snapshot the store lacks
    /// ends the chain.
    pub(crate) async fn bundle_chain(
        &self,
        snapshot: fabric_core::id::SnapshotId,
    ) -> Result<Vec<fabric_core::id::SnapshotId>, NodeError> {
        crate::transfer::with_store(&self.store, move |store| {
            let mut chain = vec![snapshot];
            let mut at = snapshot;
            loop {
                let base = match store.manifest(at) {
                    Ok(manifest) => manifest.git.and_then(|git| git.bundle_base),
                    Err(fabric_store::store::StoreError::UnknownSnapshot(_)) => None,
                    Err(e) => return Err(e.into()),
                };
                match base {
                    Some(base) if !chain.contains(&base) => {
                        chain.push(base);
                        at = base;
                    }
                    _ => return Ok(chain),
                }
            }
        })
        .await
    }

    /// The highest epoch in `peer`'s ledger for the job, read from its whole chain.
    async fn peer_epoch(self: &Arc<Self>, id: JobId, peer: NodeId) -> Result<Epoch, NodeError> {
        Ok(self
            .peer_chain(id, peer)
            .await?
            .map_or(Epoch::FIRST, |ledger| chain_top(&ledger)))
    }

    /// `peer`'s whole ledger for the job, replayed as one chain of it, or
    /// `None` when it holds no record of the job.
    pub(crate) async fn peer_chain(
        self: &Arc<Self>,
        id: JobId,
        peer: NodeId,
    ) -> Result<Option<Ledger>, NodeError> {
        let token = FencingToken {
            job_id: id,
            epoch: Epoch::FIRST,
        };
        let mut after = None;
        let mut records = Vec::new();
        loop {
            let answer = self
                .call(peer, |session| async move {
                    session.pull(token, after, SHORT_DEADLINE).await
                })
                .await?;
            let (page, more) = match answer {
                Fenced::Answer(Pulled::UpToDate) => break,
                Fenced::Answer(Pulled::Last(page)) => (page, false),
                Fenced::Answer(Pulled::More(page)) => (page, true),
                _ => {
                    return Err(NodeError::Peer {
                        peer,
                        reason: "its ledger changed while this node read it".to_owned(),
                    });
                }
            };
            for bytes in page.records() {
                let record = Record::decode(bytes.as_bytes()).map_err(LedgerError::from)?;
                after = Some(record.hash());
                records.push(record);
            }
            if !more {
                break;
            }
        }
        if records.is_empty() {
            return Ok(None);
        }
        // Each record alone only proves its own framing, so a record of
        // another job, or a chain with a gap, would otherwise let a faulty
        // peer freeze a valid job here with any epoch it wrote, or give a
        // repair a chain this node's rules refuse.
        Ledger::replay(id, records)
            .map(Some)
            .map_err(|e| NodeError::Peer {
                peer,
                reason: format!("its ledger for job {id} does not replay: {e}"),
            })
    }
}

/// The highest epoch in a chain.
fn chain_top(ledger: &Ledger) -> Epoch {
    ledger
        .records()
        .iter()
        .map(|record| record.event().epoch)
        .max()
        .unwrap_or(Epoch::FIRST)
}

/// The highest epoch in `records`, a peer's whole ledger for job `id`,
/// once they replay as one ledger: the first a `Created` for this job, and
/// each record after it linked to the one before and allowed by the
/// ledger's rules.
#[cfg(test)]
fn chain_epoch(id: JobId, records: Vec<Record>) -> Result<Epoch, LedgerError> {
    Ok(chain_top(&Ledger::replay(id, records)?))
}

/// What the fork rule of ADR 0003 makes of two chains of one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fork {
    /// One holds the other: they do not fork.
    None,
    /// They fork after the record at `at` (0 is `Created`); `ours` says
    /// whether this node's suffix wins.
    After {
        /// The position of the last record the chains share.
        at: usize,
        /// True when this node's chain wins.
        ours: bool,
    },
}

/// Why the fork rule cannot pick a chain: two suffixes the ledger rules
/// say cannot both exist, so one of the chains was not written by Fabric.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the fork rule cannot pick between the chains: {0}")]
pub(crate) struct Undecided(&'static str);

/// How a suffix ranks under the fork rule: its highest epoch-bumping
/// record, `MoveImported` or `Stolen` with its thief, or the fork point's
/// epoch when it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rank {
    /// No epoch bump since the fork point, whose epoch this is.
    None(Epoch),
    /// A steal by this node at this epoch.
    Stolen(Epoch, NodeId),
    /// A consented move at this epoch.
    Imported(Epoch),
}

impl Rank {
    fn of(suffix: &[Record], fork_epoch: Epoch) -> Rank {
        // Epochs only rise along a chain, so the last bump is the highest.
        suffix
            .iter()
            .rev()
            .find_map(|record| match &record.event().kind {
                fabric_core::ledger::EventKind::MoveImported { .. } => {
                    Some(Rank::Imported(record.event().epoch))
                }
                fabric_core::ledger::EventKind::Stolen { by, .. } => {
                    Some(Rank::Stolen(record.event().epoch, *by))
                }
                _ => None,
            })
            .unwrap_or(Rank::None(fork_epoch))
    }

    fn epoch(self) -> Epoch {
        match self {
            Rank::None(epoch) | Rank::Stolen(epoch, _) | Rank::Imported(epoch) => epoch,
        }
    }
}

/// The fork rule of ADR 0003 (Forks), from the two chains alone, so both
/// nodes reach the same answer with no message and no clock: the higher
/// epoch-bumping record wins; at equal epoch `MoveImported` beats
/// `Stolen`; between two `Stolen` the lower thief's node id, compared as
/// bytes, wins. A suffix with no bump counts at the fork point's epoch.
pub(crate) fn fork_rule(ours: &[Record], theirs: &[Record]) -> Result<Fork, Undecided> {
    let shared = ours
        .iter()
        .zip(theirs)
        .take_while(|(a, b)| a.hash() == b.hash())
        .count();
    if shared == 0 {
        return Err(Undecided("the chains share no record, not even Created"));
    }
    if shared == ours.len() || shared == theirs.len() {
        return Ok(Fork::None);
    }
    let at = shared - 1;
    let fork_epoch = ours[at].event().epoch;
    let (mine, other) = (
        Rank::of(&ours[shared..], fork_epoch),
        Rank::of(&theirs[shared..], fork_epoch),
    );
    let ours = match mine.epoch().cmp(&other.epoch()) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (mine, other) {
            (Rank::Imported(_), Rank::Stolen(..) | Rank::None(_))
            | (Rank::Stolen(..), Rank::None(_)) => true,
            (Rank::Stolen(..) | Rank::None(_), Rank::Imported(_))
            | (Rank::None(_), Rank::Stolen(..)) => false,
            (Rank::Stolen(_, a), Rank::Stolen(_, b)) if a != b => a.to_bytes() < b.to_bytes(),
            (Rank::Stolen(..), Rank::Stolen(..)) => {
                return Err(Undecided("one node stole the job twice at one epoch"));
            }
            (Rank::Imported(_), Rank::Imported(_)) => {
                return Err(Undecided("two moves were imported at one epoch"));
            }
            (Rank::None(_), Rank::None(_)) => {
                return Err(Undecided("neither chain raised the epoch after the fork"));
            }
        },
    };
    Ok(Fork::After { at, ours })
}

#[cfg(test)]
mod fork_tests {
    use fabric_core::harness::HarnessKind;
    use fabric_core::id::{EventId, JobId, MoveId, NodeId, SnapshotId};
    use fabric_core::ledger::{Author, EventKind, Ledger, MemoryStore, Record};
    use fabric_core::moves::{CompleteSnapshot, OfferAnswer};

    use super::{Fork, fork_rule};

    const JOB: JobId = JobId::from_bytes([3; 16]);
    const A: NodeId = NodeId::from_bytes([1; 32]);
    const B: NodeId = NodeId::from_bytes([2; 32]);
    const C: NodeId = NodeId::from_bytes([3; 32]);

    fn eid() -> EventId {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut bytes = [0; 16];
        bytes[8..].copy_from_slice(&NEXT.fetch_add(1, Ordering::Relaxed).to_be_bytes());
        EventId::from_bytes(bytes)
    }

    const SNAPSHOT: [u8; 1] = [7];

    /// A owns the job, suspended in a published snapshot.
    fn suspended() -> Ledger {
        let created = Ledger::created(JOB, "goal".into(), HarnessKind::Codex, A, eid(), 1).unwrap();
        let mut ledger = Ledger::replay(JOB, [created]).unwrap();
        let snapshot = SnapshotId::of(&SNAPSHOT);
        for kind in [
            EventKind::SnapshotPublished {
                snapshot,
                parent: None,
            },
            EventKind::Suspended { snapshot },
        ] {
            write(&mut ledger, A, kind);
        }
        ledger
    }

    fn write(ledger: &mut Ledger, author: NodeId, kind: EventKind) {
        let record = ledger.propose(author, kind, eid(), 2).unwrap();
        ledger
            .commit(&mut MemoryStore::default(), record, Author::Node(author))
            .unwrap();
    }

    fn steal(ledger: &mut Ledger, thief: NodeId) {
        let complete = Some(CompleteSnapshot::new(SnapshotId::of(&SNAPSHOT)));
        let proposal = ledger
            .steal(thief, String::new(), complete, eid(), 3)
            .unwrap();
        ledger
            .commit(
                &mut MemoryStore::default(),
                proposal.record,
                Author::Node(thief),
            )
            .unwrap();
    }

    fn import(ledger: &mut Ledger, target: NodeId) {
        let owner = ledger.lease().holder();
        let started = ledger
            .start_move(owner, target, MoveId::from_bytes([9; 16]), eid(), 3)
            .unwrap();
        ledger
            .commit(&mut MemoryStore::default(), started, Author::Node(owner))
            .unwrap();
        let offer = ledger.offer(owner).unwrap();
        let epoch = ledger.lease().epoch();
        let complete = Some(CompleteSnapshot::new(offer.snapshot));
        let OfferAnswer::Import(record) = ledger
            .answer_offer(target, &offer, epoch, complete, eid(), 4)
            .unwrap()
        else {
            panic!("the target did not import");
        };
        ledger
            .commit(&mut MemoryStore::default(), record, Author::Node(target))
            .unwrap();
    }

    fn note(ledger: &mut Ledger) {
        let owner = ledger.lease().holder();
        write(
            ledger,
            owner,
            EventKind::Note {
                text: "work".into(),
            },
        );
    }

    fn records(ledger: &Ledger) -> Vec<Record> {
        ledger.records().to_vec()
    }

    fn winner(ours: &Ledger, theirs: &Ledger) -> Option<bool> {
        match fork_rule(&records(ours), &records(theirs)).unwrap() {
            Fork::None => None,
            Fork::After { ours, .. } => Some(ours),
        }
    }

    /// Rule 1: the higher epoch wins, and a suffix that never raised the
    /// epoch counts at the fork point's: the old owner that kept noting at
    /// epoch one loses to a steal at epoch two, from both sides.
    #[test]
    fn the_higher_epoch_wins() {
        let base = suspended();
        let (mut owner, mut thief) = (base.clone(), base);
        note(&mut owner);
        steal(&mut thief, B);
        assert_eq!(winner(&owner, &thief), Some(false));
        assert_eq!(winner(&thief, &owner), Some(true));
        let mut again = thief.clone();
        steal(&mut again, C);
        let mut moved = owner.clone();
        import(&mut moved, C);
        // Epoch three by a second steal beats epoch two by a move.
        assert_eq!(winner(&moved, &again), Some(false));
    }

    /// Rule 2: at equal epoch `MoveImported` beats `Stolen`, the move being
    /// one the owner of record consented to.
    #[test]
    fn a_move_beats_a_steal_at_one_epoch() {
        let base = suspended();
        let (mut moved, mut stolen) = (base.clone(), base);
        import(&mut moved, C);
        steal(&mut stolen, B);
        assert_eq!(winner(&moved, &stolen), Some(true));
        assert_eq!(winner(&stolen, &moved), Some(false));
    }

    /// Rule 3: between two steals at one epoch, the thief with the lower
    /// node id, compared as bytes, wins, from both sides.
    #[test]
    fn the_lower_thief_wins_two_steals_at_one_epoch() {
        let base = suspended();
        let (mut by_b, mut by_c) = (base.clone(), base);
        steal(&mut by_b, B);
        steal(&mut by_c, C);
        note(&mut by_c);
        assert_eq!(winner(&by_b, &by_c), Some(true));
        assert_eq!(winner(&by_c, &by_b), Some(false));
    }

    /// Chains of which one holds the other do not fork, and two chains
    /// that both never raised the epoch after the fork, which only a node
    /// that does not follow the ledger rules writes, are not decided.
    #[test]
    fn chains_that_hold_each_other_do_not_fork_and_two_without_a_bump_are_not_decided() {
        let base = suspended();
        let mut longer = base.clone();
        note(&mut longer);
        assert_eq!(winner(&base, &longer), None);
        assert_eq!(winner(&longer, &base), None);
        assert_eq!(winner(&base, &base), None);
        let mut other = base.clone();
        write(
            &mut other,
            A,
            EventKind::Note {
                text: "other".into(),
            },
        );
        assert!(fork_rule(&records(&longer), &records(&other)).is_err());
    }
}

#[cfg(test)]
mod tests {
    use crate::node::Out;
    use crate::ops::tests::{job, node, paired};
    use crate::transfer::is_complete;

    /// A peer's ledger is trusted for its epoch only as a whole chain of
    /// this job: a record of another job, framed well on its own, must not
    /// count, or a faulty peer could freeze this job with any epoch.
    #[test]
    fn a_peer_ledger_that_does_not_replay_for_the_job_gives_no_epoch() {
        use fabric_core::harness::HarnessKind;
        use fabric_core::id::{EventId, JobId, NodeId};
        use fabric_core::ledger::Ledger;
        let created = |job: u8| {
            Ledger::created(
                JobId::from_bytes([job; 16]),
                "goal".into(),
                HarnessKind::Codex,
                NodeId::from_bytes([3; 32]),
                EventId::from_bytes([4; 16]),
                1,
            )
            .unwrap()
        };
        let id = JobId::from_bytes([1; 16]);
        assert!(super::chain_epoch(id, vec![created(2)]).is_err());
        assert_eq!(
            super::chain_epoch(id, vec![created(1)]).unwrap(),
            fabric_core::id::Epoch::FIRST
        );
    }

    /// A peer paired with no address, and with no session open, cannot be
    /// asked before a launch whether it stole the job while this node was
    /// away. The launch goes ahead on this node's ledger (ADR 0016), and the
    /// check must say which peer it could not ask, as it does for one that
    /// does not answer.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_check_names_a_peer_it_cannot_ask() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        a.peers.update(|book| book.pair(b.id, None)).unwrap();
        let id = job(dir_a.path(), &a).await;
        let mut out = Out::default();
        a.check_peers(id, &mut out).await;
        let peer = b.id.to_string();
        assert!(
            out.said()
                .iter()
                .any(|line| line.starts_with(&peer) && line.contains("no address")),
            "{:?}",
            out.said()
        );
    }

    /// A replica that learned a suspend by a pull, with no push and so no
    /// fetch, must fetch the snapshot at its next catch-up: the pull then
    /// finds the ledger up to date, and without the snapshot the replica
    /// cannot restore or steal the job once the owner is gone.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_catch_up_fetches_the_snapshot_a_pull_left_out() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let path = crate::tool_env::ToolEnv::current();
        let snapshot = a.suspend(id, &path, &mut Out::default()).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        assert!(!is_complete(&b.store, snapshot).await.unwrap());
        b.catch_up_peer(a.id).await.unwrap();
        assert!(
            is_complete(&b.store, snapshot).await.unwrap(),
            "the catch-up left the replica without the latest snapshot"
        );
    }
}
