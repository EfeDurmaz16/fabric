//! What this node answers its paired peers (ADR 0003, ADR 0013).
//!
//! Invariant: a per-job message from a peer whose epoch is behind this
//! node's is refused with the epoch it lacks, checked under the job's lock
//! so the epoch cannot change before the message acts; a move request or
//! an abort from a peer ahead of this node is refused too, since this node
//! must catch up before it acts on one. Every record a peer sends passes
//! the ledger rules before it is stored. This node writes exactly one
//! decision per move it is the target of, makes it durable before the
//! answer leaves, and answers every later question about that move with the
//! same record, byte for byte. A pull and a move status question are reads
//! and are answered whatever the asker's epoch: the asker of a pull is
//! catching up, and the source asking for a decision is one epoch behind
//! once the target imported. A pushed page that
//! does not continue this node's chain, from a peer at an epoch at least as
//! high, freezes the job here before the refusal is answered. Manifests and
//! chunks are named by their hashes and served to any paired peer.

use fabric_core::BoxFuture;
use fabric_core::id::{ChunkId, Epoch, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::FencingToken;
use fabric_core::ledger::{Author, LedgerError};
use fabric_core::moves::{AbortAnswer, CompleteSnapshot, LeaseView, MoveOffer, OfferAnswer};
use fabric_net::rpc::{JobFuture, Service};
use fabric_net::wire::{
    Appended, Facts, Fault, FaultText, Fenced, ManifestChunks, MoveRequested, MoveState, Offered,
    Pulled, RecordBytes, RecordPage,
};
use fabric_store::objects::ObjectError;
use fabric_store::store::StoreError;

use crate::clock;
use crate::error::NodeError;
use crate::facts::NodeFacts;
use crate::faults::{self, Action, Point};
use crate::job::Job;
use crate::local::ForkRecord;
use crate::node::{Node, Out};
use crate::transfer::{fetch_snapshot, is_complete, with_store};

fn internal(err: &impl std::fmt::Display) -> Fault {
    Fault::Internal(FaultText::truncated(&err.to_string()))
}

fn bad(err: &impl std::fmt::Display) -> Fault {
    Fault::BadRequest(FaultText::truncated(&err.to_string()))
}

/// The ADR 0003 epoch check for a message that acts on the job: `None`
/// when the sender's epoch is this node's, else the refusal to answer.
fn fence<T>(token: &FencingToken, current: Epoch) -> Option<Fenced<T>> {
    match token.epoch.cmp(&current) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some(Fenced::StaleEpoch {
            yours: token.epoch,
            current,
        }),
        std::cmp::Ordering::Greater => Some(Fenced::AheadOfUs {
            yours: token.epoch,
            current,
        }),
    }
}

fn record_bytes(bytes: &[u8]) -> Result<RecordBytes, Fault> {
    RecordBytes::new(bytes.to_vec()).map_err(|e| internal(&e))
}

impl Service for Node {
    fn pull(
        &self,
        _peer: NodeId,
        token: FencingToken,
        after: Option<RecordHash>,
    ) -> JobFuture<'_, Pulled> {
        Box::pin(async move { self.answer_pull(token.job_id, after).await })
    }

    fn append(
        &self,
        peer: NodeId,
        token: FencingToken,
        records: RecordPage,
    ) -> JobFuture<'_, Appended> {
        Box::pin(async move { self.answer_append(peer, token, records).await })
    }

    fn move_request(&self, peer: NodeId, token: FencingToken) -> JobFuture<'_, MoveRequested> {
        Box::pin(async move { self.answer_move_request(peer, token).await })
    }

    fn offer(&self, peer: NodeId, token: FencingToken, offer: MoveOffer) -> JobFuture<'_, Offered> {
        Box::pin(async move { self.answer_offer(peer, token, offer).await })
    }

    fn move_status(
        &self,
        _peer: NodeId,
        token: FencingToken,
        move_id: MoveId,
    ) -> JobFuture<'_, MoveState> {
        Box::pin(async move { self.answer_move_status(token.job_id, move_id).await })
    }

    fn move_abort(
        &self,
        peer: NodeId,
        token: FencingToken,
        move_id: MoveId,
    ) -> JobFuture<'_, RecordBytes> {
        Box::pin(async move { self.answer_move_abort(peer, token, move_id).await })
    }

    fn manifest(
        &self,
        _peer: NodeId,
        snapshot: SnapshotId,
    ) -> BoxFuture<'_, Result<ManifestChunks, Fault>> {
        Box::pin(async move {
            let listed = with_store(&self.store, move |store| {
                Ok(store.manifest_chunks(snapshot)?)
            })
            .await;
            match listed {
                Ok((len, ids)) => ManifestChunks::new(len, ids).map_err(|e| internal(&e)),
                Err(NodeError::Store(StoreError::UnknownSnapshot(_))) => {
                    Err(Fault::UnknownSnapshot)
                }
                Err(e) => Err(internal(&e)),
            }
        })
    }

    fn chunk(&self, _peer: NodeId, id: ChunkId) -> BoxFuture<'_, Result<Vec<u8>, Fault>> {
        Box::pin(async move {
            let read = with_store(&self.store, move |store| {
                Ok(store.objects().read_stored(id).map_err(StoreError::from)?)
            })
            .await;
            match read {
                Ok(frame) => Ok(frame),
                Err(NodeError::Store(StoreError::Object(ObjectError::Missing { .. }))) => {
                    Err(Fault::UnknownChunk(id))
                }
                Err(e) => Err(internal(&e)),
            }
        })
    }

    fn facts(&self, _peer: NodeId) -> BoxFuture<'_, Result<Facts, Fault>> {
        Box::pin(async move {
            Ok(NodeFacts::measure(&self.cfg.home, &self.cfg.env)
                .await
                .to_wire())
        })
    }
}

impl Node {
    async fn answer_move_status(
        &self,
        id: JobId,
        move_id: MoveId,
    ) -> Result<Fenced<MoveState>, Fault> {
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let job = job.lock().await;
        Ok(Fenced::Answer(match job.ledger().decision_for(move_id) {
            Some(record) => MoveState::Decided {
                record: record_bytes(record.bytes())?,
            },
            None => MoveState::Undecided { move_id },
        }))
    }

    /// Decides against `move_id` for the source of the move, at the source's epoch.
    ///
    /// A move already decided is answered with its decision at any epoch,
    /// since the decision is what the source lacks.
    async fn answer_move_abort(
        &self,
        peer: NodeId,
        token: FencingToken,
        move_id: MoveId,
    ) -> Result<Fenced<RecordBytes>, Fault> {
        let job = self
            .job(token.job_id)
            .await
            .map_err(|_| Fault::UnknownJob)?;
        let mut job = job.lock().await;
        if let Some(record) = job.ledger().decision_for(move_id) {
            return Ok(Fenced::Answer(record_bytes(record.bytes())?));
        }
        if let Some(fenced) = fence(&token, job.ledger().lease().epoch()) {
            return Ok(fenced);
        }
        let source = job.ledger().lease().holder();
        if peer != source {
            return Err(bad(&format!(
                "only {source}, the source of move {move_id}, may abort it"
            )));
        }
        let answer = job
            .ledger()
            .answer_abort(self.id, move_id, clock::event_id(), clock::now_ms())
            .map_err(|e| bad(&e))?;
        let record = match answer {
            AbortAnswer::Decided(record) => record,
            AbortAnswer::Abort(record) => {
                job.commit(record.clone(), Author::Node(self.id))
                    .map_err(|e| internal(&e))?;
                record
            }
        };
        Ok(Fenced::Answer(record_bytes(record.bytes())?))
    }

    async fn answer_pull(
        &self,
        id: JobId,
        after: Option<RecordHash>,
    ) -> Result<Fenced<Pulled>, Fault> {
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let job = job.lock().await;
        let ledger = job.ledger();
        let suffix = match after {
            None => ledger.records(),
            Some(after) => match ledger.suffix_after(after) {
                Some(suffix) => suffix,
                None => {
                    return Ok(Fenced::Answer(Pulled::NotInChain {
                        head: ledger.head(),
                    }));
                }
            },
        };
        let mut rest = suffix.iter();
        let Some(first) = rest.next() else {
            return Ok(Fenced::Answer(Pulled::UpToDate));
        };
        let mut page = RecordPage::new(record_bytes(first.bytes())?);
        for record in rest {
            if page.push(record_bytes(record.bytes())?).is_err() {
                return Ok(Fenced::Answer(Pulled::More(page)));
            }
        }
        Ok(Fenced::Answer(Pulled::Last(page)))
    }

    async fn answer_append(
        &self,
        peer: NodeId,
        token: FencingToken,
        records: RecordPage,
    ) -> Result<Fenced<Appended>, Fault> {
        let id = token.job_id;
        let mut out = Out::default();
        // A job new here is made from the page's first record, which must be its `Created`.
        let known = if let Ok(job) = self.job(id).await {
            job
        } else {
            let first = records
                .records()
                .first()
                .ok_or_else(|| bad(&"an empty page"))?;
            self.accept_record(id, peer, first.as_bytes())
                .await
                .map_err(|e| bad(&e))?;
            self.job(id).await.map_err(|_| Fault::UnknownJob)?
        };
        {
            let mut job = known.lock().await;
            let current = job.ledger().lease().epoch();
            if token.epoch < current {
                return Ok(Fenced::StaleEpoch {
                    yours: token.epoch,
                    current,
                });
            }
            for bytes in records.records() {
                match job.accept(bytes.as_bytes(), peer, self.id) {
                    Ok(_) => {}
                    Err(NodeError::Ledger(LedgerError::PrevMismatch { head })) => {
                        // The sender holds records this node lacks after a
                        // record this node replaced with its own: the chains
                        // forked. The epoch check above ran under this lock,
                        // so the sender's epoch is at least this node's.
                        self.freeze_on_push(&mut job, peer, token.epoch, &records)
                            .await?;
                        return Ok(Fenced::Answer(Appended::Rejected { head }));
                    }
                    Err(e) => return Err(bad(&e)),
                }
            }
            job.reconcile_marker().map_err(|e| internal(&e))?;
            self.settle(&mut job, &mut out)
                .await
                .map_err(|e| internal(&e))?;
        }
        for event in out.events {
            tracing::info!(job = %id, ?event, "peer request");
        }
        let (head, latest) = {
            let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
            let job = job.lock().await;
            (job.ledger().head(), job.latest())
        };
        if let Some(snapshot) = latest {
            self.fetch_from(peer, snapshot).await?;
        }
        Ok(Fenced::Answer(Appended::Accepted { head }))
    }

    /// Freezes the job after a pushed page that does not continue this
    /// node's chain, from a peer at an epoch at least as high (ADR 0016, Forks).
    async fn freeze_on_push(
        &self,
        job: &mut Job,
        peer: NodeId,
        epoch: Epoch,
        records: &RecordPage,
    ) -> Result<(), Fault> {
        let Some(last) = records.records().last() else {
            return Ok(());
        };
        let mut out = Out::default();
        self.freeze_locked(
            job,
            &ForkRecord {
                peer,
                epoch,
                head: RecordHash::of(last.as_bytes()),
            },
            &mut out,
        )
        .await
        .map_err(|e| internal(&e))?;
        for event in out.events {
            tracing::warn!(job = %job.id(), ?event, "fork");
        }
        Ok(())
    }

    /// Fetches `snapshot` from `peer` over the session the peer opened, unless it is here whole.
    async fn fetch_from(&self, peer: NodeId, snapshot: SnapshotId) -> Result<(), Fault> {
        if is_complete(&self.store, snapshot)
            .await
            .map_err(|e| internal(&e))?
        {
            return Ok(());
        }
        let session = self
            .open_session(peer)
            .ok_or_else(|| internal(&"no session with the sender to fetch the snapshot over"))?;
        fetch_snapshot(&self.store, &session, snapshot)
            .await
            .map_err(|e| internal(&e))
    }

    async fn answer_move_request(
        &self,
        peer: NodeId,
        token: FencingToken,
    ) -> Result<Fenced<MoveRequested>, Fault> {
        let id = token.job_id;
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let mut job = job.lock().await;
        if let Some(fenced) = fence(&token, job.ledger().lease().epoch()) {
            return Ok(fenced);
        }
        match job.view(self.id) {
            LeaseView::Owner(_) => {}
            LeaseView::Departing { move_id, target } if target == peer => {
                return Ok(Fenced::Answer(MoveRequested::Started { move_id }));
            }
            LeaseView::Departing { target, .. } => {
                return Err(bad(&format!("a move toward {target} is open")));
            }
            LeaseView::Departed { owner, .. } | LeaseView::Replica { owner, .. } => {
                return Err(bad(&format!(
                    "{} does not own the job; {owner} does",
                    self.id
                )));
            }
        }
        let mut out = Out::default();
        let env = self.cfg.env.clone();
        self.suspend_locked(&mut job, &env, &mut out)
            .await
            .map_err(|e| internal(&e))?;
        let move_id = clock::move_id();
        let record = job
            .ledger()
            .start_move(self.id, peer, move_id, clock::event_id(), clock::now_ms())
            .map_err(|e| bad(&e))?;
        job.commit(record, Author::Node(self.id))
            .map_err(|e| internal(&e))?;
        drop(job);
        faults::hit(Point::AfterDepartingWrite).await;
        for event in out.events {
            tracing::info!(job = %id, ?event, "peer request");
        }
        Ok(Fenced::Answer(MoveRequested::Started { move_id }))
    }

    async fn answer_offer(
        &self,
        peer: NodeId,
        token: FencingToken,
        offer: MoveOffer,
    ) -> Result<Fenced<Offered>, Fault> {
        for _ in 0..2 {
            let complete = is_complete(&self.store, offer.snapshot)
                .await
                .map_err(|e| internal(&e))?
                .then_some(CompleteSnapshot::new(offer.snapshot));
            let job = self
                .job(offer.job_id)
                .await
                .map_err(|_| Fault::UnknownJob)?;
            let mut job = job.lock().await;
            let answer = job
                .ledger()
                .answer_offer(
                    self.id,
                    &offer,
                    token.epoch,
                    complete,
                    clock::event_id(),
                    clock::now_ms(),
                )
                .map_err(|e| bad(&e))?;
            match answer {
                OfferAnswer::Decided(record) => {
                    return Ok(Fenced::Answer(Offered::Decided {
                        record: record_bytes(record.bytes())?,
                    }));
                }
                OfferAnswer::Import(record) => {
                    job.commit(record.clone(), Author::Node(self.id))
                        .map_err(|e| internal(&e))?;
                    drop(job);
                    tracing::info!(job = %offer.job_id, move_id = %offer.move_id, "imported");
                    if faults::hit(Point::AfterImportReceipt).await == Some(Action::Drop) {
                        tracing::warn!(job = %offer.job_id, "fault: dropping the answer to the offer");
                        if let Some(session) = self.open_session(peer) {
                            session.close();
                        }
                        return Err(internal(&"the answer was dropped"));
                    }
                    return Ok(Fenced::Answer(Offered::Decided {
                        record: record_bytes(record.bytes())?,
                    }));
                }
                OfferAnswer::Rejected { head } => {
                    return Ok(Fenced::Answer(Offered::Rejected { head }));
                }
                OfferAnswer::Stale { yours, current } => {
                    return Ok(Fenced::StaleEpoch { yours, current });
                }
                OfferAnswer::SnapshotIncomplete { snapshot } => {
                    drop(job);
                    self.fetch_from(peer, snapshot).await?;
                }
            }
        }
        Ok(Fenced::Answer(Offered::SnapshotIncomplete {
            snapshot: offer.snapshot,
        }))
    }
}

#[cfg(test)]
mod tests {
    use fabric_core::ledger::EventKind;

    use super::*;
    use crate::ops::tests::{job, node};

    /// A move request from a peer whose epoch is ahead of this node's must
    /// not suspend the job or write `MoveStarted`: this node has to catch
    /// up first, and the request may come from a peer the ledger moved past.
    #[tokio::test]
    async fn a_move_request_at_another_epoch_changes_nothing() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let ahead = FencingToken {
            job_id: id,
            epoch: Epoch::new(2).unwrap(),
        };
        let answer = node
            .answer_move_request(NodeId::from_bytes([9; 32]), ahead)
            .await
            .unwrap();
        assert!(matches!(answer, Fenced::AheadOfUs { .. }), "{answer:?}");
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        assert!(
            job.ledger()
                .records()
                .iter()
                .all(|r| matches!(r.event().kind, EventKind::Created { .. })),
            "the request wrote to the ledger"
        );
    }
}
