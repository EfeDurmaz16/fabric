//! What this node answers its paired peers (ADR 0003, ADR 0013).
//!
//! Invariant: a per-job message from a peer whose epoch is behind this
//! node's is refused with the epoch it lacks, and every record a peer sends
//! passes the ledger rules before it is stored. This node writes exactly
//! one decision per move it is the target of, makes it durable before the
//! answer leaves, and answers every later question about that move with the
//! same record, byte for byte. A pull is a read and is answered whatever
//! the asker's epoch, because the asker is catching up. A pushed page that
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
use crate::local::ForkRecord;
use crate::node::{Node, Out};
use crate::transfer::{fetch_snapshot, is_complete, with_store};

fn internal(err: &impl std::fmt::Display) -> Fault {
    Fault::Internal(FaultText::truncated(&err.to_string()))
}

fn bad(err: &impl std::fmt::Display) -> Fault {
    Fault::BadRequest(FaultText::truncated(&err.to_string()))
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
        Box::pin(async move { self.answer_move_request(peer, token.job_id).await })
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
        _peer: NodeId,
        token: FencingToken,
        move_id: MoveId,
    ) -> JobFuture<'_, RecordBytes> {
        Box::pin(async move { self.answer_move_abort(token.job_id, move_id).await })
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
            Ok(NodeFacts::measure(
                &self.cfg.home,
                &self.cfg.path_var,
                self.cfg.cargo_home.clone(),
            )
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

    async fn answer_move_abort(
        &self,
        id: JobId,
        move_id: MoveId,
    ) -> Result<Fenced<RecordBytes>, Fault> {
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let mut job = job.lock().await;
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
        if let Ok(job) = self.job(id).await {
            let current = job.lock().await.ledger().lease().epoch();
            if token.epoch < current {
                return Ok(Fenced::StaleEpoch {
                    yours: token.epoch,
                    current,
                });
            }
        }
        for bytes in records.records() {
            match self.accept_record(id, peer, bytes.as_bytes()).await {
                Ok(_) => {}
                Err(NodeError::Ledger(LedgerError::PrevMismatch { head })) => {
                    // The sender holds records this node lacks after a
                    // record this node replaced with its own: the chains
                    // forked. The epoch check above passed, so the
                    // sender's epoch is at least this node's.
                    self.freeze_on_push(id, peer, token.epoch, &records).await?;
                    return Ok(Fenced::Answer(Appended::Rejected { head }));
                }
                Err(e) => return Err(bad(&e)),
            }
        }
        let mut out = Out::default();
        self.settle_job(id, &mut out)
            .await
            .map_err(|e| internal(&e))?;
        for line in out.lines {
            tracing::info!(job = %id, "{line}");
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
        id: JobId,
        peer: NodeId,
        epoch: Epoch,
        records: &RecordPage,
    ) -> Result<(), Fault> {
        let Some(last) = records.records().last() else {
            return Ok(());
        };
        let mut out = Out::default();
        self.freeze(
            id,
            &ForkRecord {
                peer,
                epoch,
                head: RecordHash::of(last.as_bytes()),
            },
            &mut out,
        )
        .await
        .map_err(|e| internal(&e))?;
        for line in out.lines {
            tracing::warn!(job = %id, "{line}");
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
        id: JobId,
    ) -> Result<Fenced<MoveRequested>, Fault> {
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let mut job = job.lock().await;
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
        let path_var = self.cfg.path_var.clone();
        self.suspend_locked(&mut job, &path_var, &mut out)
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
        for line in out.lines {
            tracing::info!(job = %id, "{line}");
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
