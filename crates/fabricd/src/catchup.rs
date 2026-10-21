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
//! start-up pass runs is caught up when that pass ends. A peer that does
//! not answer is skipped for the rest of a pass rather than dialled again
//! for every job.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::id::{Epoch, JobId, NodeId, RecordHash};
use fabric_core::lease::FencingToken;
use fabric_core::ledger::{LedgerError, Record};
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
        let mut caught_up = BTreeSet::new();
        let mut unreachable = BTreeSet::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((peer, Ok(()))) => {
                    caught_up.insert(peer);
                }
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
        self.end_recovery(&caught_up);
    }

    /// Marks start-up recovery done and catches up with every peer whose
    /// session began while it ran and that it did not already catch up
    /// with, such as an addressless peer that dialed in after recovery
    /// listed the reachable peers.
    ///
    /// The flag is set under the same lock `spawn_catch_up` checks it
    /// under, so a session that begins at any moment is either queued here
    /// or caught up on its own, never neither.
    pub(crate) fn end_recovery(self: &Arc<Self>, caught_up: &BTreeSet<NodeId>) {
        let waiting = {
            let mut waiting = lock(&self.waiting_catch_up);
            self.recovered.send_replace(true);
            std::mem::take(&mut *waiting)
        };
        for peer in waiting.difference(caught_up) {
            self.spawn_catch_up(*peer);
        }
    }

    /// Whether the start-up recovery has run.
    pub(crate) fn is_recovered(&self) -> bool {
        *self.recovered.borrow()
    }

    /// Pulls every job this node knows from `peer` and settles each one.
    ///
    /// It stops at the first failure to reach the peer, so an unreachable
    /// peer costs one dial per pass and not one per job. Any other failure
    /// is logged for that job and the next job is tried.
    async fn catch_up_peer(self: &Arc<Self>, peer: NodeId) -> Result<(), NodeError> {
        let ids: Vec<JobId> = self.jobs.lock().await.keys().copied().collect();
        for id in ids {
            let mut out = Out::default();
            match self.pull_records(id, peer).await {
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
    /// begins, unless one already runs for it. While start-up recovery
    /// runs, the peer waits for it instead, so the catch-up never acts on
    /// a ledger recovery has not settled, and is not lost when recovery
    /// listed the reachable peers before this session began.
    pub(crate) fn spawn_catch_up(self: &Arc<Self>, peer: NodeId) {
        {
            let mut waiting = lock(&self.waiting_catch_up);
            if !self.is_recovered() {
                waiting.insert(peer);
                return;
            }
        }
        if !lock(&self.catching_up).insert(peer) {
            return;
        }
        let node = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = node.catch_up_peer(peer).await {
                tracing::info!(%peer, error = %e, "did not catch up with the peer");
            }
            lock(&node.catching_up).remove(&peer);
        });
    }

    /// Before a launch: asks each reachable peer, within `CHECK_DEADLINE`,
    /// for the records it holds after this node's head, so a node that
    /// stayed up through a steal or a move learns it before it runs the job.
    /// A peer that does not answer in time is named and skipped.
    pub(crate) async fn check_peers(self: &Arc<Self>, id: JobId, out: &mut Out) {
        let mut tasks = tokio::task::JoinSet::new();
        for peer in self.reachable_peers() {
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
    pub(crate) async fn freeze_locked(
        &self,
        job: &mut Job,
        fork: &ForkRecord,
        out: &mut Out,
    ) -> Result<(), NodeError> {
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
        let mut epoch = Epoch::FIRST;
        loop {
            let answer = self
                .call(peer, |session| async move {
                    session.pull(token, after, SHORT_DEADLINE).await
                })
                .await?;
            let (page, more) = match answer {
                Fenced::Answer(Pulled::UpToDate) => return Ok(epoch),
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
                epoch = epoch.max(record.event().epoch);
                after = Some(record.hash());
            }
            if !more {
                return Ok(epoch);
            }
        }
    }
}
