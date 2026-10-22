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
                    out.say(format!(
                        "the open move waits for {target}, which did not answer"
                    ));
                }
                Some(_) => {
                    if let Err(e) = self.resolve_departing(id, &mut out).await {
                        tracing::info!(job = %id, error = %e, "the open move stays open");
                    }
                }
                None => {}
            }
            for line in out.lines {
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
                Err(e) => out.say(format!("did not catch up with {peer}: {e}")),
            }
            if let Err(e) = self.settle_job(id, &mut out).await {
                out.say(format!("left as it was: {e}"));
            }
            for line in out.lines {
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
                out.say(format!(
                    "{} has no address here and no session open with this node, so a move or a steal there is not known here",
                    entry.node()
                ));
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
                Ok((peer, Ok(Err(e)))) => out.say(format!("did not catch up with {peer}: {e}")),
                Ok((peer, Err(_))) => out.say(format!(
                    "{peer} did not answer within {} s, so a move or a steal there is not known here",
                    CHECK_DEADLINE.as_secs()
                )),
                Err(e) => tracing::warn!(error = %e, "a catch-up task ended early"),
            }
        }
        if let Err(e) = self.settle_job(id, out).await {
            out.say(format!("left as it was: {e}"));
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
        for line in out.lines {
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
            out.say(format!(
                "frozen: {}'s ledger at epoch {} does not continue this node's",
                fork.peer, fork.epoch
            ));
        }
        self.settle(job, out).await
    }

    /// The highest epoch in `peer`'s ledger for the job, read from its whole chain.
    async fn peer_epoch(self: &Arc<Self>, id: JobId, peer: NodeId) -> Result<Epoch, NodeError> {
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
            return Ok(Epoch::FIRST);
        }
        chain_epoch(id, records).map_err(|e| NodeError::Peer {
            peer,
            reason: format!("its ledger for job {id} does not replay: {e}"),
        })
    }
}

/// The highest epoch in `records`, a peer's whole ledger for job `id`,
/// once they replay as one ledger: the first a `Created` for this job, and
/// each record after it linked to the one before and allowed by the
/// ledger's rules. Each record alone only proves its own framing, so a
/// record of another job, or a chain with a gap, would otherwise let a
/// faulty peer freeze a valid job here with any epoch it wrote.
fn chain_epoch(id: JobId, records: Vec<Record>) -> Result<Epoch, LedgerError> {
    let ledger = Ledger::replay(id, records)?;
    Ok(ledger
        .records()
        .iter()
        .map(|record| record.event().epoch)
        .max()
        .unwrap_or(Epoch::FIRST))
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
            out.lines
                .iter()
                .any(|line| line.starts_with(&peer) && line.contains("no address")),
            "{:?}",
            out.lines
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
