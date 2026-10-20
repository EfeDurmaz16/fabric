//! What this node answers its paired peers (ADR 0003, ADR 0013).
//!
//! Invariant: a per-job message from a peer whose epoch is behind this
//! node's is refused with the epoch it lacks, and every record a peer sends
//! passes the ledger rules before it is stored. This node writes exactly
//! one decision per move it is the target of, makes it durable before the
//! answer leaves, and answers every later question about that move with the
//! same record, byte for byte. A pull is a read and is answered whatever
//! the asker's epoch, because the asker is catching up. Manifests and
//! chunks are named by their hashes and served to any paired peer.

use fabric_core::BoxFuture;
use fabric_core::id::{ChunkId, JobId, NodeId, SnapshotId};
use fabric_core::lease::FencingToken;
use fabric_core::ledger::{Author, LedgerError};
use fabric_core::moves::{AbortAnswer, CompleteSnapshot, LeaseView, OfferAnswer};
use fabric_net::rpc::Service;
use fabric_net::wire::{
    Facts, Fault, FaultText, JobRequest, JobResponse, ManifestChunks, OfferBody, RecordBytes,
    RecordPage,
};
use fabric_store::objects::ObjectError;
use fabric_store::store::StoreError;

use crate::clock;
use crate::error::NodeError;
use crate::facts::NodeFacts;
use crate::faults::{self, Action, Point};
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
    fn job(
        &self,
        peer: NodeId,
        token: FencingToken,
        request: JobRequest,
    ) -> BoxFuture<'_, Result<JobResponse, Fault>> {
        Box::pin(async move { self.answer_job(peer, token, request).await })
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
    async fn answer_job(
        &self,
        peer: NodeId,
        token: FencingToken,
        request: JobRequest,
    ) -> Result<JobResponse, Fault> {
        let id = token.job_id;
        match request {
            JobRequest::Pull { after } => self.answer_pull(id, after).await,
            JobRequest::Append { records } => self.answer_append(peer, token, records).await,
            JobRequest::MoveRequest => self.answer_move_request(peer, id).await,
            JobRequest::MoveOffer(body) => self.answer_offer(peer, token, body).await,
            JobRequest::MoveStatus { move_id } => {
                let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
                let job = job.lock().await;
                Ok(match job.ledger().decision_for(move_id) {
                    Some(record) => JobResponse::MoveDecision {
                        record: record_bytes(record.bytes())?,
                    },
                    None => JobResponse::MoveUndecided { move_id },
                })
            }
            JobRequest::MoveAbort { move_id } => {
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
                Ok(JobResponse::MoveDecision {
                    record: record_bytes(record.bytes())?,
                })
            }
        }
    }

    async fn answer_pull(
        &self,
        id: JobId,
        after: fabric_core::id::RecordHash,
    ) -> Result<JobResponse, Fault> {
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let job = job.lock().await;
        let ledger = job.ledger();
        // A head this chain does not hold means the asker has nothing yet,
        // or forked; either way it gets the chain from the start and its
        // ledger rules decide what links.
        let suffix = ledger.suffix_after(after).unwrap_or(ledger.records());
        let mut records = RecordPage::default();
        let mut more = false;
        for record in suffix {
            if records.push(record_bytes(record.bytes())?).is_err() {
                more = true;
                break;
            }
        }
        Ok(JobResponse::Records { records, more })
    }

    async fn answer_append(
        &self,
        peer: NodeId,
        token: FencingToken,
        records: RecordPage,
    ) -> Result<JobResponse, Fault> {
        let id = token.job_id;
        if let Ok(job) = self.job(id).await {
            let current = job.lock().await.ledger().lease().epoch();
            if token.epoch < current {
                return Ok(JobResponse::StaleEpoch {
                    yours: token.epoch,
                    current,
                });
            }
        }
        for bytes in records.records() {
            match self.accept_record(id, peer, bytes.as_bytes()).await {
                Ok(_) => {}
                Err(NodeError::Ledger(LedgerError::PrevMismatch { head })) => {
                    return Ok(JobResponse::Rejected { head });
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
        Ok(JobResponse::Accepted { head })
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

    async fn answer_move_request(&self, peer: NodeId, id: JobId) -> Result<JobResponse, Fault> {
        let job = self.job(id).await.map_err(|_| Fault::UnknownJob)?;
        let mut job = job.lock().await;
        match job.view(self.id) {
            LeaseView::Owner(_) => {}
            LeaseView::Departing { move_id, target } if target == peer => {
                return Ok(JobResponse::MoveStarted { move_id });
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
        Ok(JobResponse::MoveStarted { move_id })
    }

    async fn answer_offer(
        &self,
        peer: NodeId,
        token: FencingToken,
        body: OfferBody,
    ) -> Result<JobResponse, Fault> {
        let offer = body.for_job(token.job_id);
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
                    return Ok(JobResponse::MoveDecision {
                        record: record_bytes(record.bytes())?,
                    });
                }
                OfferAnswer::Import(record) => {
                    job.commit(record.clone(), Author::Node(self.id))
                        .map_err(|e| internal(&e))?;
                    drop(job);
                    tracing::info!(job = %offer.job_id, move_id = %offer.move_id, "imported");
                    if faults::hit(Point::AfterImportReceipt).await == Some(Action::Drop) {
                        if let Some(session) = self.open_session(peer) {
                            session.close();
                        }
                        return Err(internal(&"the answer was dropped"));
                    }
                    return Ok(JobResponse::MoveDecision {
                        record: record_bytes(record.bytes())?,
                    });
                }
                OfferAnswer::Rejected { head } => return Ok(JobResponse::Rejected { head }),
                OfferAnswer::Stale { yours, current } => {
                    return Ok(JobResponse::StaleEpoch { yours, current });
                }
                OfferAnswer::SnapshotIncomplete { snapshot } => {
                    drop(job);
                    self.fetch_from(peer, snapshot).await?;
                }
            }
        }
        Ok(JobResponse::SnapshotIncomplete {
            snapshot: offer.snapshot,
        })
    }
}
