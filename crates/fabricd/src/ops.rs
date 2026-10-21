//! What each control request does on the node.
//!
//! Invariant: no operation changes who may run a job except through a
//! ledger record the core accepted, and every record is durable before
//! anything acts on it. A job's lock is held for local steps and released
//! before a network call, so a slow peer never stops this node answering
//! another; each step after a call re-reads the ledger and decides again.
//! A source whose move is open asks the target for its decision before it
//! does anything else with the job, and never becomes runnable again
//! without a record the target wrote (ADR 0003).

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fabric_client::api::{
    ClientEnv, ClosedJob, Decision, DoctorReport, Event, Fact, HarnessSession, JobCreated, JobList,
    JobLog, JobStatus, JobSummary, Lease, LedgerEvent, LogRecord, NodeIdentity, Paired, Peer,
    PeerList, Pong, Process, Reason, Registered, Reply, Request, RunPlan, SessionRecorded,
    SnapshotState, Standing, StealWarning, Step, Suspended, View, Wire, Workspace, WorkspaceState,
};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, MoveId, NodeId, SnapshotId};
use fabric_core::lease::{FenceError, FencingToken};
use fabric_core::ledger::{Author, EventKind, Ledger, LedgerError};
use fabric_core::moves::{CompleteSnapshot, LeaseView, MoveOffer, OfferAnswer};
use fabric_core::session::SessionId;
use fabric_harness::HarnessError;
use fabric_harness::manifest::{MANIFEST_FILE, StateManifest};
use fabric_net::peers::HostPort;
use fabric_net::rpc::Session;
use fabric_net::wire::{
    Appended, Fenced, MoveRequested, MoveState, Offered, Pulled, RecordBytes, RecordPage,
};
use fabric_store::capture::{CaptureRequest, capture};
use fabric_store::policy::IgnorePolicy;
use fabric_store::restore::{
    Durability, RestoreRequest, RestoreState, Verify, restore, restore_state,
};

use crate::clock;
use crate::doctor::{self, Context, Target};
use crate::error::NodeError;
use crate::facts::{MeasuredKeys, NodeFacts, key_difference};
use crate::faults::{self, Action, Point};
use crate::harnesses::{HarnessConfig, adapter, refuse_secret_kind};
use crate::job::Job;
use crate::local::{self, HarnessRecord, Liveness, Marker, MarkerState, SessionRecord};
use crate::node::{Node, Out, is_transport};
use crate::paths::{self, RootChoice, SyncedFolders, check_root, restore_root};
use crate::stamp::TreeStamp;
use crate::tool_env::ToolEnv;
use crate::transfer::{fetch_snapshot, is_complete, with_store};

/// How long a harness gets between `SIGTERM` and `SIGKILL` when a suspend stops it.
pub const STOP_GRACE: Duration = Duration::from_secs(10);

/// How long a fenced harness gets between `SIGTERM` and `SIGKILL`.
pub const FENCE_GRACE: Duration = Duration::from_secs(5);

/// How long a page of records may take, the snapshot fetch it can start included.
pub const APPEND_DEADLINE: Duration = Duration::from_mins(10);

/// How long a move offer or a pull's move request may take.
pub const MOVE_DEADLINE: Duration = Duration::from_mins(10);

/// How long a status question or a ping may take.
pub const SHORT_DEADLINE: Duration = Duration::from_secs(15);

/// The bytes of records `job log` answers with at most: half the control
/// frame cap, which leaves room for the encoding around them.
const LOG_BUDGET: usize = fabric_client::frame::MAX_FRAME / 2;

/// How many times a move asks again after a lost answer before it reports the move frozen.
const MOVE_TRIES: usize = 5;

/// One answer in the offer loop: to the offer itself, or to a later `MoveStatus`.
#[derive(Debug)]
enum Asked {
    Offer(Fenced<Offered>),
    Status(Fenced<MoveState>),
}

impl Node {
    pub(crate) async fn dispatch(
        self: &Arc<Self>,
        request: Request,
        out: &mut Out,
    ) -> Result<Reply, NodeError> {
        self.wait_for_recovery(&request, out).await;
        if let Some(env) = request.env() {
            self.remember_env(env);
        }
        Ok(match request {
            Request::NodeId => Reply::NodeId(NodeIdentity {
                node: self.id,
                listen: self.local_addr()?,
            }),
            Request::NodeAdd { node, addr } => {
                Reply::NodeAdd(self.node_add(node, addr.as_deref()).await?)
            }
            Request::NodeList => Reply::NodeList(self.node_list()),
            Request::NodePing { node } => Reply::NodePing(self.node_ping(node).await?),
            Request::JobNew {
                repo,
                goal,
                harness,
            } => Reply::JobNew(self.job_new(repo.as_path(), goal, harness).await?),
            Request::JobList => Reply::JobList(self.job_list().await),
            Request::JobStatus { job } => Reply::JobStatus(Box::new(self.job_status(job).await?)),
            Request::JobLog { job } => Reply::JobLog(self.job_log(job).await?),
            Request::JobRun { job, env: _ } => {
                self.resolve_departing(job, out).await?;
                self.check_peers(job, out).await;
                Reply::JobRun(self.job_run(job, out).await?)
            }
            Request::JobResume {
                job,
                on,
                harness,
                force,
                env,
            } => {
                let env = self.client_env(env.as_ref());
                Reply::JobResume(
                    Box::pin(self.job_resume(job, on, harness, force, &env, out)).await?,
                )
            }
            Request::HarnessStarted {
                job,
                epoch,
                pid,
                harness,
                session,
            } => Reply::HarnessStarted(
                self.harness_started(job, epoch, pid, harness, session)
                    .await?,
            ),
            Request::HarnessExited { job, pid } => {
                self.harness_exited(job, pid).await?;
                Reply::HarnessExited
            }
            Request::HarnessSession {
                job,
                epoch,
                pid,
                harness,
                session,
            } => Reply::HarnessSession(
                self.harness_session(job, epoch, pid, harness, session)
                    .await?,
            ),
            Request::JobSuspend { job, env } => {
                let env = self.client_env(env.as_ref());
                self.resolve_departing(job, out).await?;
                let snapshot = self.suspend(job, &env, out).await?;
                self.replicate(job, out).await;
                Reply::JobSuspend(Suspended { snapshot })
            }
            Request::JobMove {
                job,
                to,
                force,
                env,
            } => {
                let env = self.client_env(env.as_ref());
                Reply::JobMove(self.job_move(job, to, force, &env, out).await?)
            }
            Request::JobSteal { job, reason, force } => {
                Reply::JobSteal(self.job_steal(job, reason, force, out).await?)
            }
            Request::JobDecide {
                job,
                decision,
                note,
            } => Reply::JobDecide(fabric_client::api::Decided {
                text: self.job_decide(job, decision, &note).await?,
            }),
            Request::Doctor { peer, job, env } => {
                let env = self.client_env(env.as_ref());
                Reply::Doctor {
                    report: self.doctor(peer, job, &env).await?,
                }
            }
        })
    }

    /// Reads answer from local state at once, saying so while start-up
    /// recovery runs; a command that changes a job waits until recovery has
    /// learned what the peers wrote.
    async fn wait_for_recovery(&self, request: &Request, out: &mut Out) {
        let reads = matches!(
            request,
            Request::JobList | Request::JobStatus { .. } | Request::JobLog { .. }
        );
        let node_only = matches!(
            request,
            Request::NodeId
                | Request::NodeAdd { .. }
                | Request::NodeList
                | Request::NodePing { .. }
                | Request::Doctor { .. }
        );
        if reads && !self.is_recovered() {
            out.emit(Event::CatchingUp);
        } else if !reads && !node_only {
            self.recovered().await;
        }
    }

    /// The environment a client sent, or the one the node started with
    /// (ADR 0018, The client's environment).
    fn client_env(&self, env: Option<&ClientEnv>) -> ToolEnv {
        env.map_or_else(|| self.node_env(), ToolEnv::from)
    }

    /// Who owns `id` now, at which epoch.
    #[cfg(test)]
    async fn lease(&self, id: JobId) -> Result<Lease, NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        Ok(lease_of(&job))
    }

    async fn node_add(&self, node: NodeId, addr: Option<&str>) -> Result<Paired, NodeError> {
        let addr: Option<HostPort> = addr
            .map(|text| {
                text.parse()
                    .map_err(|e: fabric_net::peers::SpecError| NodeError::BadAddress {
                        text: text.to_owned(),
                        reason: e.to_string(),
                    })
            })
            .transpose()?;
        let peers = Arc::clone(&self.peers);
        let shown = addr.as_ref().map(ToString::to_string);
        tokio::task::spawn_blocking(move || peers.update(|book| book.pair(node, addr)))
            .await
            .map_err(|e| NodeError::Task(e.to_string()))??;
        Ok(Paired { node, addr: shown })
    }

    fn node_list(&self) -> PeerList {
        let book = self.peers.snapshot();
        PeerList {
            peers: book
                .iter()
                .map(|entry| Peer {
                    node: entry.node(),
                    addrs: entry.addrs().iter().map(ToString::to_string).collect(),
                    connected: self.open_session(entry.node()).is_some(),
                })
                .collect(),
        }
    }

    async fn node_ping(self: &Arc<Self>, node: NodeId) -> Result<Pong, NodeError> {
        let session = self.dial(node).await?;
        let started = tokio::time::Instant::now();
        session
            .ping(SHORT_DEADLINE)
            .await
            .map_err(|e| NodeError::Unreachable {
                peer: node,
                reason: e.to_string(),
            })?;
        Ok(Pong {
            node,
            rtt_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            wire: wire(session.kind()),
            clock_skew_ms: session.clock_skew_ms(),
        })
    }

    async fn job_new(
        &self,
        repo: &Path,
        goal: String,
        harness: HarnessKind,
    ) -> Result<JobCreated, NodeError> {
        refuse_secret_kind(&harness)?;
        let id = clock::job_id();
        let root = paths::job_root(&self.cfg.home, id);
        let synced = synced_folders(&self.cfg.home, &self.node_env()).await;
        check_root(&root, &self.cfg.home, synced)?;
        let parent = root
            .parent()
            .ok_or_else(|| NodeError::io(&root, std::io::Error::other("a root needs a parent")))?;
        crate::durable::create_dir(parent).map_err(|e| NodeError::io(parent, e))?;
        let cloned = tokio::process::Command::new("git")
            .args(["clone", "--quiet", "--"])
            .arg(repo)
            .arg(&root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .await
            .map_err(|e| NodeError::io(repo, e))?;
        if !cloned.status.success() {
            return Err(NodeError::io(
                repo,
                std::io::Error::other(String::from_utf8_lossy(&cloned.stderr).trim().to_owned()),
            ));
        }
        let created = Ledger::created(
            id,
            goal,
            harness.clone(),
            self.id,
            clock::event_id(),
            clock::now_ms(),
        )
        .map_err(fabric_core::ledger::LedgerError::from)?;
        let job = Job::create_owned(
            id,
            self.cfg.state.job(id),
            &created,
            &Marker {
                root: root.clone(),
                state: MarkerState::Live,
                epoch: Epoch::FIRST,
            },
        )?;
        self.jobs
            .lock()
            .await
            .insert(id, Arc::new(tokio::sync::Mutex::new(job)));
        Ok(JobCreated {
            job: id,
            root,
            repo: repo.to_owned(),
            harness,
            epoch: Epoch::FIRST,
        })
    }

    async fn job_list(&self) -> JobList {
        let jobs: Vec<_> = self.jobs.lock().await.values().cloned().collect();
        let mut listed = Vec::with_capacity(jobs.len());
        for job in jobs {
            let job = job.lock().await;
            let manifest = job.ledger().manifest();
            let standing = match job.forked() {
                Ok(Some(_)) => Standing::Frozen,
                Ok(None) => standing(&job.view(self.id)),
                Err(_) => Standing::Unreadable,
            };
            listed.push(JobSummary {
                job: job.id(),
                standing,
                epoch: manifest.lease.epoch(),
                goal: manifest.goal.clone(),
            });
        }
        JobList {
            jobs: listed,
            closed: self
                .broken_jobs()
                .into_iter()
                .map(|(job, error)| ClosedJob { job, error })
                .collect(),
        }
    }

    fn broken_jobs(&self) -> Vec<(JobId, String)> {
        self.broken
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(id, e)| (*id, e.clone()))
            .collect()
    }

    async fn job_status(self: &Arc<Self>, id: JobId) -> Result<JobStatus, NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        let manifest = job.ledger().manifest();
        // A record that does not read is reported in its place, so the
        // status still says everything else: it is what a user runs to
        // find out why a job is broken (review finding on PR #13).
        let forked = job.forked();
        let view = match (job.view(self.id), &forked) {
            (_, Err(e)) => View::Unreadable {
                error: e.to_string(),
            },
            (_, Ok(Some(fork))) => View::Frozen {
                peer: fork.peer,
                epoch: fork.epoch,
            },
            (LeaseView::Owner(_), Ok(None)) => View::Owner,
            (LeaseView::Departing { move_id, target }, Ok(None)) => {
                View::Departing { move_id, target }
            }
            (LeaseView::Departed { owner, epoch }, Ok(None)) => View::Departed { owner, epoch },
            (LeaseView::Replica { owner, epoch }, Ok(None)) => View::Replica { owner, epoch },
        };
        // A fork record that does not read may stand for a fork, so the
        // workspace is judged as a frozen one's: an orphan nothing deletes.
        let frozen = !matches!(forked, Ok(None));
        let (workspace, workspace_unreadable) = match job.marker() {
            Ok(None) => (None, None),
            Ok(Some(marker)) => (
                Some(Workspace {
                    state: workspace_state(&job, self.id, &marker, frozen).await,
                    root: marker.root,
                }),
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        };
        let process = match job.harness_record() {
            Ok(Some(record)) => match record.liveness().await {
                Liveness::Leader => Process::Running {
                    pid: record.pid,
                    epoch: record.epoch,
                },
                Liveness::Group => Process::GroupRemains { pid: record.pid },
                Liveness::Gone => Process::NotRunning,
            },
            Ok(None) => Process::NotRunning,
            Err(e) => Process::Unreadable {
                error: e.to_string(),
            },
        };
        Ok(JobStatus {
            job: id,
            node: self.id,
            goal: manifest.goal.clone(),
            harness: manifest.harness.clone(),
            owner: manifest.lease.holder(),
            epoch: manifest.lease.epoch(),
            view,
            snapshot: manifest.latest_snapshot.map(|id| SnapshotState {
                id,
                suspended: job.suspended_in().is_some(),
            }),
            workspace,
            workspace_unreadable,
            process,
            records: manifest.event_count,
        })
    }

    /// The job's records, the latest ones when all of them would not fit
    /// in one control answer.
    async fn job_log(&self, id: JobId) -> Result<JobLog, NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        let records: Vec<LogRecord> = job
            .ledger()
            .records()
            .iter()
            .map(|record| {
                let event = record.event();
                LogRecord {
                    id: event.id,
                    epoch: event.epoch,
                    at: event.at,
                    event: ledger_event(&event.kind),
                }
            })
            .collect();
        // A record is at most 1 MiB, so the latest one always fits.
        let mut start = records.len();
        let mut bytes = 0;
        while let Some(record) = start.checked_sub(1).map(|i| &records[i]) {
            let size = serde_json::to_vec(record).map_or(usize::MAX, |json| json.len());
            if bytes + size > LOG_BUDGET {
                break;
            }
            bytes += size;
            start -= 1;
        }
        Ok(JobLog {
            left_out: u64::try_from(start).unwrap_or(u64::MAX),
            records: records.into_iter().skip(start).collect(),
        })
    }

    async fn job_run(self: &Arc<Self>, id: JobId, out: &mut Out) -> Result<RunPlan, NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        let marker = job.reconcile_marker()?.ok_or(NodeError::NoWorkspace(id))?;
        if !marker.is_current(token.epoch, job.latest()).await {
            return Err(NodeError::StaleWorkspace {
                job: id,
                root: marker.root,
                epoch: marker.epoch,
            });
        }
        match marker.state {
            MarkerState::Live | MarkerState::Clean { .. } => {}
            MarkerState::Suspending { .. } => {
                return Err(NodeError::Unfinished {
                    job: id,
                    step: Step::Suspend,
                });
            }
            MarkerState::Restoring { .. } => {
                return Err(NodeError::Unfinished {
                    job: id,
                    step: Step::Restore,
                });
            }
        }
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid,
            });
        }
        plan(
            &job,
            token.epoch,
            marker.root,
            None,
            &self.harness_config,
            out,
        )
    }

    /// Records the harness the CLI launched and appends `Resumed`, or refuses.
    ///
    /// It fails closed: a stale epoch, a harness already recorded and alive,
    /// and a process whose start time cannot be read are all refused, and
    /// the CLI then kills the group it launched. The record is written
    /// before `Resumed`, so no harness the ledger says runs is unsupervised.
    pub(crate) async fn harness_started(
        &self,
        id: JobId,
        epoch: Epoch,
        pid: NonZeroU32,
        harness: HarnessKind,
        session: Option<SessionId>,
    ) -> Result<Registered, NodeError> {
        refuse_secret_kind(&harness)?;
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        if token.epoch != epoch {
            return Err(FenceError::StaleEpoch {
                yours: epoch,
                current: token.epoch,
            }
            .into());
        }
        if let Some(record) = job.harness_record()?
            && record.pid != pid
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid,
            });
        }
        if !local::leads_group(pid).await {
            return Err(NodeError::HarnessGone { pid });
        }
        let started = local::process_started(pid)
            .await
            .ok_or(NodeError::HarnessGone { pid })?;
        local::write(
            &job.dir().harness(),
            &HarnessRecord {
                epoch,
                pid,
                started,
            },
        )?;
        let snapshot = job.latest();
        job.append(
            self.id,
            EventKind::Resumed {
                node: self.id,
                snapshot,
                harness: harness.clone(),
            },
        )?;
        if let Some(session) = session {
            local::write(
                &job.dir().session(),
                &SessionRecord {
                    harness: harness.clone(),
                    session,
                },
            )?;
        }
        if let Some(marker) = job.marker()? {
            job.set_marker(&Marker {
                root: marker.root,
                state: MarkerState::Live,
                epoch,
            })?;
        }
        Ok(Registered {
            epoch,
            harness,
            pid,
        })
    }

    /// Records the session a harness named after it started, for the
    /// harness this node recorded: a headless Codex run names a new thread
    /// only once it runs, and a capture without the session would leave the
    /// thread behind.
    ///
    /// It refuses a stale epoch and a pid that is not the recorded
    /// harness's. A suspend, a move or a fence that stopped the harness
    /// removed its record and captured without the session, so a session
    /// recorded later would name one the snapshot does not hold.
    pub(crate) async fn harness_session(
        &self,
        id: JobId,
        epoch: Epoch,
        pid: NonZeroU32,
        harness: HarnessKind,
        session: SessionId,
    ) -> Result<SessionRecorded, NodeError> {
        refuse_secret_kind(&harness)?;
        let job = self.job(id).await?;
        let job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        if token.epoch != epoch {
            return Err(FenceError::StaleEpoch {
                yours: epoch,
                current: token.epoch,
            }
            .into());
        }
        if job.harness_record()?.is_none_or(|record| record.pid != pid) {
            return Err(NodeError::HarnessNotRecorded {
                job: id,
                pid,
                session,
            });
        }
        local::write(
            &job.dir().session(),
            &SessionRecord {
                harness: harness.clone(),
                session: session.clone(),
            },
        )?;
        Ok(SessionRecorded { harness, session })
    }

    async fn harness_exited(&self, id: JobId, pid: NonZeroU32) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        if let Some(record) = job.harness_record()?
            && record.pid == pid
        {
            // The record stands for the group: a background process the
            // harness started keeps it, so a suspend still stops it.
            if record.liveness().await == Liveness::Gone {
                local::remove(&job.dir().harness())?;
            } else {
                tracing::info!(job = %id, pid, "the harness exited and its group still runs");
            }
        }
        Ok(())
    }

    /// Stops the harness, captures, publishes and records the suspend (ADR 0004, Suspend).
    ///
    /// Each step is safe to run again after a kill: a published snapshot
    /// the ledger does not name yet is captured again, and one the ledger
    /// names is finished with `Suspended` and no second capture.
    pub(crate) async fn suspend(
        &self,
        id: JobId,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<SnapshotId, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        self.suspend_locked(&mut job, env, out).await
    }

    pub(crate) async fn suspend_locked(
        &self,
        job: &mut Job,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<SnapshotId, NodeError> {
        let id = job.id();
        let epoch = owner_token(job, self.id)?.epoch;
        let Some(marker) = job.reconcile_marker()? else {
            return job.suspended_in().ok_or(NodeError::NoWorkspace(id));
        };
        if !marker.is_current(epoch, job.latest()).await {
            out.emit(Event::StaleWorkspaceKept {
                job: id,
                root: marker.root.clone(),
                left_at: marker.epoch,
            });
            return job.suspended_in().ok_or(NodeError::NoWorkspace(id));
        }
        if let (MarkerState::Clean { snapshot, .. }, Some(suspended)) =
            (&marker.state, job.suspended_in())
            && *snapshot == suspended
        {
            if marker.is_unchanged().await {
                out.emit(Event::AlreadySuspended {
                    snapshot: suspended,
                });
                return Ok(suspended);
            }
            out.emit(Event::WorkspaceChanged {
                snapshot: suspended,
            });
        }
        if let MarkerState::Restoring { .. } = marker.state {
            return Err(NodeError::Unfinished {
                job: id,
                step: Step::Restore,
            });
        }
        if let Some(record) = job.harness_record()? {
            if record.liveness().await != Liveness::Gone {
                out.emit(Event::StoppingHarness { pid: record.pid });
                record.stop(STOP_GRACE).await?;
            }
            local::remove(&job.dir().harness())?;
        }
        faults::hit(Point::AfterQuiesce).await;

        let last = job
            .ledger()
            .records()
            .last()
            .map(|r| r.event().kind.clone());
        let (snapshot, tree) = if let Some(EventKind::SnapshotPublished { snapshot, .. }) = last {
            out.emit(Event::FinishingSuspend { snapshot });
            let tree = match marker.state {
                MarkerState::Suspending { tree, .. } => tree,
                _ => None,
            };
            (snapshot, tree)
        } else {
            // Measured before the capture, so an edit made while it runs
            // shows as a change at the next suspend.
            let tree = TreeStamp::measure(&marker.root).await;
            job.set_marker(&Marker {
                root: marker.root.clone(),
                state: MarkerState::Suspending {
                    head: job.ledger().head(),
                    tree,
                },
                epoch,
            })?;
            let snapshot = self.capture(job, &marker.root, env, out).await?;
            faults::hit(Point::AfterSnapshotPublish).await;
            job.append(
                self.id,
                EventKind::SnapshotPublished {
                    snapshot,
                    parent: job.latest(),
                },
            )?;
            (snapshot, tree)
        };
        self.confirm_departure(job, env, out).await?;
        job.append(self.id, EventKind::Suspended { snapshot })?;
        job.set_marker(&Marker {
            root: marker.root,
            state: MarkerState::Clean { snapshot, tree },
            epoch,
        })?;
        out.emit(Event::Suspended {
            snapshot,
            epoch: job.ledger().lease().epoch(),
        });
        Ok(snapshot)
    }

    /// Tells the adapter that the snapshot holding its capture is published,
    /// so its departure record counts at the next restore on this node (ADR 0015).
    ///
    /// It runs after `SnapshotPublished` is durable, on the first suspend
    /// and on the one that finishes it after a crash; a second confirmation
    /// changes nothing.
    async fn confirm_departure(
        &self,
        job: &Job,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let Some(record) = job.session()? else {
            return Ok(());
        };
        let state_dir = job.dir().harness_state();
        if !state_dir.join(MANIFEST_FILE).is_file() {
            return Ok(());
        }
        let manifest = StateManifest::read(&state_dir)?;
        let harness = adapter(&record.harness, &self.harness_env(env)?)?;
        match harness.confirm_departure(&manifest).await {
            Ok(()) => Ok(()),
            Err(HarnessError::DepartureNotPending { session }) => {
                out.emit(Event::NoDepartureRecord { session });
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Captures the harness state and the workspace and publishes them as one snapshot.
    async fn capture(
        &self,
        job: &Job,
        root: &Path,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<SnapshotId, NodeError> {
        let state_dir = job.dir().harness_state();
        let mut harness_state = None;
        if let Some(record) = job.session()? {
            let session = record.session;
            let harness = adapter(&record.harness, &self.harness_env(env)?)?;
            job.clear_harness_state()?;
            std::fs::create_dir_all(&state_dir).map_err(|e| NodeError::io(&state_dir, e))?;
            let manifest = harness.capture(root, &session, &state_dir).await?;
            out.emit(Event::SessionCaptured {
                harness: record.harness.clone(),
                session,
                files: u64::try_from(manifest.files().len()).unwrap_or(u64::MAX),
            });
            harness_state = Some(state_dir);
        }
        let facts = NodeFacts::measure(&self.cfg.home, env).await;
        // A workspace whose environment does not detect here would not
        // materialize on the target either, so the snapshot is refused
        // before it is published or moved.
        let detection = fabric_env::detect::detect(root, &[])?;
        let keys = MeasuredKeys::new(facts, Some(&detection));
        let secret_patterns: Vec<String> = detection
            .spec()
            .secret_patterns
            .iter()
            .map(|pattern| pattern.as_str().to_owned())
            .collect();
        let root = root.to_owned();
        let job_id = job.id();
        let me = self.id;
        let parent = job.latest();
        let (snapshot, report) = with_store(&self.store, move |store| {
            let mut policy = IgnorePolicy::default();
            policy.load_fabricignore(&root)?;
            // The job's own patterns from `.fabric/env.toml` widen the
            // built-in list; nothing narrows it (ADR 0004).
            for pattern in &secret_patterns {
                policy.add_secret_pattern(pattern)?;
            }
            let captured = capture(
                store,
                &CaptureRequest {
                    root: &root,
                    job_id,
                    created_by: me,
                    created_at: clock::now_ms(),
                    parent,
                    policy: &policy,
                    carry_keys: &keys,
                    harness_state: harness_state.as_deref(),
                    env_spec: None,
                    allow_large_untracked: false,
                },
            )?;
            let snapshot = store.publish(&captured.manifest)?;
            Ok((snapshot, captured.report))
        })
        .await?;
        out.emit(Event::SnapshotPublished {
            snapshot,
            files: report.files,
            bytes: report.bytes,
            new_objects: report.new_objects,
        });
        for path in report.denied.iter().chain(&report.denied_untracked) {
            out.emit(Event::LeftBehind {
                path: path.to_string(),
            });
        }
        Ok(snapshot)
    }

    /// Pushes the ledger, and through it the latest snapshot, to every paired peer it can reach.
    pub(crate) async fn replicate(self: &Arc<Self>, id: JobId, out: &mut Out) {
        // A peer behind a NAT has no address here and is reached over the
        // session it opened, as every other request reaches it.
        for peer in self.reachable_peers() {
            match self.push_records(id, peer).await {
                Ok(()) => out.emit(Event::Replicated { peer }),
                Err(e) => out.emit(Event::NotReplicated {
                    peer,
                    error: e.to_refusal(),
                }),
            }
        }
    }

    /// Sends the whole ledger to `peer` page by page; the peer keeps what it lacks
    /// and fetches the latest snapshot before it answers the last page.
    pub(crate) async fn push_records(
        self: &Arc<Self>,
        id: JobId,
        peer: NodeId,
    ) -> Result<(), NodeError> {
        let (records, epoch) = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            (
                job.ledger().records().to_vec(),
                job.ledger().lease().epoch(),
            )
        };
        let token = FencingToken { job_id: id, epoch };
        let mut pages: Vec<RecordPage> = Vec::new();
        for record in records {
            let bytes = RecordBytes::new(record.bytes().to_vec())?;
            let unplaced = match pages.last_mut() {
                Some(page) => page.push(bytes).err(),
                None => Some(bytes),
            };
            if let Some(bytes) = unplaced {
                pages.push(RecordPage::new(bytes));
            }
        }
        for page in pages {
            let answer = self
                .call(peer, |session| {
                    let page = page.clone();
                    async move { session.append(token, page, APPEND_DEADLINE).await }
                })
                .await?;
            match answer {
                Fenced::Answer(Appended::Accepted { .. }) => {}
                Fenced::Answer(Appended::Rejected { head }) => {
                    return Err(NodeError::peer_fact(
                        peer,
                        format!(
                            "its ledger does not continue this one (its head is {head}); the chains forked"
                        ),
                        Reason::PeerForked {
                            job: id,
                            peer,
                            head,
                        },
                    ));
                }
                Fenced::StaleEpoch { current, .. } => {
                    return Err(NodeError::peer_fact(
                        peer,
                        format!(
                            "it knows epoch {current}, newer than this node's; run `fabric job resume {id}` to catch up"
                        ),
                        Reason::PeerAhead {
                            job: id,
                            peer,
                            epoch: current,
                        },
                    ));
                }
                Fenced::AheadOfUs { current, .. } => {
                    return Err(NodeError::peer_fact(
                        peer,
                        format!(
                            "it is at epoch {current}, older than this node's, and must catch up first"
                        ),
                        Reason::PeerBehind {
                            job: id,
                            peer,
                            epoch: current,
                        },
                    ));
                }
            }
        }
        Ok(())
    }

    /// Calls `peer` on the kept session, or on a fresh one when the kept one failed at the transport.
    pub(crate) async fn call<T, F, Fut>(
        self: &Arc<Self>,
        peer: NodeId,
        f: F,
    ) -> Result<T, NodeError>
    where
        F: Fn(Arc<Session>) -> Fut,
        Fut: std::future::Future<Output = Result<T, fabric_net::rpc::RpcError>>,
    {
        let session = self.session(peer).await?;
        match f(Arc::clone(&session)).await {
            Ok(value) => Ok(value),
            Err(e) if is_transport(&e) => {
                self.forget(peer, &session);
                let session = self.dial(peer).await?;
                f(session).await.map_err(|e| NodeError::Unreachable {
                    peer,
                    reason: e.to_string(),
                })
            }
            Err(e) => Err(answer_error(peer, &e)),
        }
    }

    /// The push form of the move (ADR 0003): suspend, push, `MoveStarted`, offer, decision.
    /// Moves `id` to `to` and returns the lease the move's decision made.
    ///
    /// The lease is read under the lock that applies the decision, or that
    /// finds it applied: another steal or move may change it once the lock
    /// is released.
    async fn job_move(
        self: &Arc<Self>,
        id: JobId,
        to: NodeId,
        force: bool,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<Lease, NodeError> {
        if to == self.id {
            return Err(NodeError::ledger_fact(
                LedgerError::MoveToSelf,
                Reason::MoveToSelf { job: id },
            ));
        }
        self.resolve_departing(id, out).await?;
        let (harness, root, view) = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            let root = job
                .marker()?
                .map_or_else(|| paths::job_root(&self.cfg.home, id), |m| m.root);
            (job.harness().clone(), root, job.view(self.id))
        };
        match view {
            LeaseView::Owner(_) => {}
            LeaseView::Departing { target, move_id } if target == to => {
                out.emit(Event::MoveReoffered {
                    move_id,
                    target: to,
                });
            }
            LeaseView::Departing { move_id, target } => {
                return Err(NodeError::Departing {
                    job: id,
                    move_id,
                    target,
                });
            }
            // A move asked again after its answer was lost: the target's
            // decision arrived while resolving, and the move is done.
            LeaseView::Departed { owner, epoch } if owner == to => {
                out.emit(Event::AlreadyMoved { owner, epoch });
                return Ok(Lease { owner, epoch });
            }
            LeaseView::Departed { owner, epoch } => {
                return Err(NodeError::Departed {
                    job: id,
                    owner,
                    epoch,
                });
            }
            LeaseView::Replica { owner, epoch } => {
                return Err(NodeError::NotOwner {
                    job: id,
                    owner,
                    epoch,
                });
            }
        }
        self.preflight(to, Some((&harness, &root)), Target::Peer, env, force, out)
            .await?;

        if matches!(view, LeaseView::Owner(_)) {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            self.suspend_locked(&mut job, env, out).await?;
        }
        self.push_records(id, to).await?;
        out.emit(Event::TargetHoldsCopy { target: to });

        let offer = self.depart(id, to, out).await?;
        faults::hit(Point::AfterDepartingWrite).await;
        let Some(offer) = offer else {
            return Err(NodeError::Peer {
                peer: to,
                reason: "the move was decided while it was being offered; see `fabric job status`"
                    .to_owned(),
            });
        };
        self.push_records(id, to).await?;
        self.offer(id, to, offer, out).await
    }

    /// Commits the source to the move with `MoveStarted` and returns the offer to send.
    ///
    /// The job's lock was released while the target copied it, so a run
    /// may have started here since: what was copied is then out of date,
    /// and the job stays here. A move already open toward `to` is offered
    /// again as it is; one toward another node refuses with that node.
    async fn depart(
        &self,
        id: JobId,
        to: NodeId,
        out: &mut Out,
    ) -> Result<Option<MoveOffer>, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        // Another move, typed at the same time, may have departed toward
        // another node while the target copied the job.
        if let LeaseView::Departing { move_id, target } = job.view(self.id)
            && target != to
        {
            return Err(NodeError::Departing {
                job: id,
                move_id,
                target,
            });
        }
        if let LeaseView::Owner(token) = job.view(self.id) {
            let suspended = job.suspended_in();
            if suspended.is_none() {
                return Err(NodeError::RanDuringMove {
                    job: id,
                    target: to,
                });
            }
            // A workspace an earlier tenure left is not the job's and is
            // not what was copied; settle leaves it where it is.
            if let Some(marker) = job.marker()?
                && marker.is_current(token.epoch, job.latest()).await
            {
                match marker.state {
                    MarkerState::Clean { snapshot, .. } if Some(snapshot) == suspended => {
                        if !marker.is_unchanged().await {
                            return Err(NodeError::ChangedDuringMove {
                                job: id,
                                root: marker.root,
                                target: to,
                            });
                        }
                    }
                    _ => {
                        return Err(NodeError::RanDuringMove {
                            job: id,
                            target: to,
                        });
                    }
                }
            }
            let move_id = clock::move_id();
            let record = job.ledger().start_move(
                self.id,
                to,
                move_id,
                clock::event_id(),
                clock::now_ms(),
            )?;
            job.commit(record, Author::Node(self.id))?;
            out.emit(Event::MoveStarted {
                move_id,
                target: to,
            });
        }
        Ok(job.ledger().offer(self.id))
    }

    /// Offers the open move until the target decides, asking again after a lost answer.
    async fn offer(
        self: &Arc<Self>,
        id: JobId,
        to: NodeId,
        offer: MoveOffer,
        out: &mut Out,
    ) -> Result<Lease, NodeError> {
        let epoch = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            job.ledger().lease().epoch()
        };
        let token = FencingToken { job_id: id, epoch };
        let mut last_error = String::new();
        for attempt in 0..MOVE_TRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
            let answer = if attempt == 0 {
                self.call(to, |session| async move {
                    session.offer(epoch, offer, MOVE_DEADLINE).await
                })
                .await
                .map(Asked::Offer)
            } else {
                self.call(to, |session| async move {
                    session
                        .move_status(token, offer.move_id, MOVE_DEADLINE)
                        .await
                })
                .await
                .map(Asked::Status)
            };
            match answer {
                Ok(
                    Asked::Offer(Fenced::Answer(Offered::Decided { record }))
                    | Asked::Status(Fenced::Answer(MoveState::Decided { record })),
                ) => {
                    let twice = faults::hit(Point::AfterImportReceipt).await == Some(Action::Dup);
                    let lease = self.apply_decision(id, to, record.as_bytes(), out).await?;
                    if twice {
                        return self.apply_decision(id, to, record.as_bytes(), out).await;
                    }
                    return Ok(lease);
                }
                Ok(Asked::Status(Fenced::Answer(MoveState::Undecided { .. }))) => {
                    let answer = self
                        .call(to, |session| async move {
                            session.offer(epoch, offer, MOVE_DEADLINE).await
                        })
                        .await;
                    last_error = match answer {
                        Ok(Fenced::Answer(Offered::Decided { record })) => {
                            return self.apply_decision(id, to, record.as_bytes(), out).await;
                        }
                        Ok(answer) => format!("{to} has not decided: {}", offered_words(&answer)),
                        Err(e) => format!("{to} has not decided: {e}"),
                    };
                }
                Ok(Asked::Offer(Fenced::Answer(
                    Offered::SnapshotIncomplete { .. } | Offered::Rejected { .. },
                ))) => {
                    self.push_records(id, to).await?;
                    last_error = format!("{to} lacked records or chunks; pushed them again");
                }
                Ok(
                    Asked::Offer(Fenced::StaleEpoch { current, .. })
                    | Asked::Status(Fenced::StaleEpoch { current, .. }),
                ) => {
                    return Err(NodeError::peer_fact(
                        to,
                        format!("it knows epoch {current}; the move cannot complete"),
                        Reason::PeerAhead {
                            job: id,
                            peer: to,
                            epoch: current,
                        },
                    ));
                }
                Ok(
                    Asked::Offer(Fenced::AheadOfUs { current, .. })
                    | Asked::Status(Fenced::AheadOfUs { current, .. }),
                ) => {
                    last_error = format!("{to} is at epoch {current} and has not caught up");
                }
                Err(e) => {
                    last_error = e.to_string();
                    out.emit(Event::NoAnswer {
                        target: to,
                        error: e.to_refusal(),
                    });
                }
            }
        }
        out.emit(Event::MoveGaveUp { detail: last_error });
        Err(NodeError::Departing {
            job: id,
            move_id: offer.move_id,
            target: to,
        })
    }

    /// Appends the target's decision for an open move; a decision already held changes nothing.
    ///
    /// Returns the lease the decision left, read under the lock that applies it.
    async fn apply_decision(
        self: &Arc<Self>,
        id: JobId,
        from: NodeId,
        bytes: &[u8],
        out: &mut Out,
    ) -> Result<Lease, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let grew = job.accept(bytes, from, self.id)?;
        if grew {
            match job.view(self.id) {
                LeaseView::Departed { owner, epoch } => {
                    out.emit(Event::Imported { owner, epoch });
                }
                LeaseView::Owner(token) => {
                    out.emit(Event::Declined {
                        target: from,
                        epoch: token.epoch,
                    });
                }
                other => out.emit(Event::DecisionLeft {
                    standing: standing(&other),
                }),
            }
        }
        let lease = lease_of(&job);
        self.settle(&mut job, out).await?;
        Ok(lease)
    }

    /// Asks the target of this node's open move for its decision, and applies it.
    pub(crate) async fn resolve_departing(
        self: &Arc<Self>,
        id: JobId,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let (move_id, target, epoch) = {
            let Ok(job) = self.job(id).await else {
                return Ok(());
            };
            let job = job.lock().await;
            match job.view(self.id) {
                LeaseView::Departing { move_id, target } => {
                    (move_id, target, job.ledger().lease().epoch())
                }
                _ => return Ok(()),
            }
        };
        let token = FencingToken { job_id: id, epoch };
        let answer = self
            .call(target, |session| async move {
                session.move_status(token, move_id, SHORT_DEADLINE).await
            })
            .await;
        match answer {
            Ok(Fenced::Answer(MoveState::Decided { record })) => self
                .apply_decision(id, target, record.as_bytes(), out)
                .await
                .map(drop),
            Ok(_) => {
                out.emit(Event::MoveUndecided { move_id, target });
                Ok(())
            }
            Err(e) => {
                out.emit(Event::MoveUnanswered {
                    move_id,
                    target,
                    error: e.to_refusal(),
                });
                Ok(())
            }
        }
    }

    /// Kills a harness this node no longer owns and moves a departed copy out of the root.
    ///
    /// A frozen job has no owner here: its harness is stopped and its
    /// workspace stays where it is, since it may hold work the job lacks.
    pub(crate) async fn settle(&self, job: &mut Job, out: &mut Out) -> Result<(), NodeError> {
        let view = job.view(self.id);
        let forked = job.forked()?.is_some();
        if let Some(record) = job.harness_record()? {
            let owned =
                !forked && matches!(view, LeaseView::Owner(token) if token.epoch == record.epoch);
            if !owned {
                if record.liveness().await != Liveness::Gone {
                    out.emit(Event::Fenced {
                        epoch: record.epoch,
                        pid: record.pid,
                    });
                    record.stop(FENCE_GRACE).await?;
                }
                local::remove(&job.dir().harness())?;
            }
        }
        if forked {
            return Ok(());
        }
        // A copy that still matches a published snapshot holds nothing the
        // job lacks, so it leaves the root; a copy with changes since the
        // last snapshot, by a harness or by hand, stays where it is, an
        // orphan the user decides about.
        if let LeaseView::Departed { epoch, .. } | LeaseView::Replica { epoch, .. } = view
            && let Some(marker) = job.reconcile_marker()?
            && matches!(marker.state, MarkerState::Clean { .. })
            && (marker.root.symlink_metadata().is_err() || marker.is_unchanged().await)
        {
            let dest =
                paths::departed_copy(&marker.root, epoch).ok_or_else(|| no_parent(&marker.root))?;
            if move_aside(&marker.root, &dest)? {
                out.emit(Event::CopyMovedAside { to: dest });
            }
            job.clear_marker()?;
        }
        Ok(())
    }

    pub(crate) async fn settle_job(&self, id: JobId, out: &mut Out) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        job.reconcile_marker()?;
        self.settle(&mut job, out).await
    }

    /// Pulls the job from `holder` if needed, restores it, and returns what to launch.
    async fn job_resume(
        self: &Arc<Self>,
        id: JobId,
        on: Option<NodeId>,
        harness: Option<HarnessKind>,
        force: bool,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<RunPlan, NodeError> {
        if let Some(harness) = &harness {
            refuse_secret_kind(harness)?;
        }
        self.resolve_departing(id, out).await?;
        if self.job(id).await.is_ok() {
            self.check_peers(id, out).await;
        }
        let holder = match self.job(id).await {
            Err(_) => Some(on.ok_or(NodeError::UnknownJob(id))?),
            Ok(job) => match job.lock().await.view(self.id) {
                LeaseView::Owner(_) => None,
                LeaseView::Departing {
                    move_id, target, ..
                } => {
                    return Err(NodeError::Departing {
                        job: id,
                        move_id,
                        target,
                    });
                }
                LeaseView::Departed { owner, .. } | LeaseView::Replica { owner, .. } => {
                    Some(on.unwrap_or(owner))
                }
            },
        };
        if let Some(holder) = holder {
            // The preflight checks the harness that will launch here, so a
            // job this node has never seen is learned before it runs.
            if self.job(id).await.is_err() {
                self.pull_records(id, holder).await?;
            }
            let kind = match &harness {
                Some(kind) => kind.clone(),
                None => self.job(id).await?.lock().await.harness().clone(),
            };
            let root = paths::job_root(&self.cfg.home, id);
            self.preflight(
                holder,
                Some((&kind, root.as_path())),
                Target::Here,
                env,
                force,
                out,
            )
            .await?;
            self.pull(id, holder, out).await?;
        }
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        let root = self
            .restore_if_needed(&mut job, token.epoch, env, out)
            .await?;
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid,
            });
        }
        plan(&job, token.epoch, root, harness, &self.harness_config, out)
    }

    /// The pull form of the move: fetch, ask the owner to start a move toward this node, decide it here.
    async fn pull(
        self: &Arc<Self>,
        id: JobId,
        holder: NodeId,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        out.emit(Event::Pulling { job: id, holder });
        self.pull_records(id, holder).await?;
        self.ensure_latest(id, holder).await?;
        let epoch = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            job.ledger().lease().epoch()
        };
        let token = FencingToken { job_id: id, epoch };
        let answer = self
            .call(holder, |session| async move {
                session.move_request(token, MOVE_DEADLINE).await
            })
            .await?;
        let move_id = match answer {
            Fenced::Answer(MoveRequested::Started { move_id }) => move_id,
            Fenced::Answer(MoveRequested::Rejected { head }) => {
                return Err(NodeError::Peer {
                    peer: holder,
                    reason: format!("it did not start a move; its head is {head}"),
                });
            }
            fenced @ Fenced::StaleEpoch { current, .. } => {
                return Err(NodeError::peer_fact(
                    holder,
                    format!("it did not start a move: {}", fenced_words(&fenced)),
                    Reason::PeerAhead {
                        job: id,
                        peer: holder,
                        epoch: current,
                    },
                ));
            }
            fenced @ Fenced::AheadOfUs { current, .. } => {
                return Err(NodeError::peer_fact(
                    holder,
                    format!("it did not start a move: {}", fenced_words(&fenced)),
                    Reason::PeerBehind {
                        job: id,
                        peer: holder,
                        epoch: current,
                    },
                ));
            }
        };
        out.emit(Event::HolderStartedMove { holder, move_id });
        self.pull_records(id, holder).await?;
        self.ensure_latest(id, holder).await?;
        faults::hit(Point::BeforeImport).await;
        self.import_pulled_move(id, holder, move_id, out).await?;
        // The holder learns the decision now, or from MoveStatus later.
        if let Err(e) = self.push_records(id, holder).await {
            out.emit(Event::HolderNotTold {
                holder,
                error: e.to_refusal(),
            });
        }
        Ok(())
    }

    /// Imports the move `holder` started for `id`, whose records and
    /// snapshot this node now holds, or says why its ledger does not take it.
    async fn import_pulled_move(
        &self,
        id: JobId,
        holder: NodeId,
        move_id: MoveId,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let ledger = job.ledger();
        let snapshot = ledger
            .manifest()
            .latest_snapshot
            .ok_or(NodeError::NoSnapshot(id))?;
        let offer = MoveOffer {
            job_id: id,
            move_id,
            head: ledger.head(),
            snapshot,
        };
        let answer = ledger.answer_offer(
            self.id,
            &offer,
            ledger.lease().epoch(),
            Some(CompleteSnapshot::new(snapshot)),
            clock::event_id(),
            clock::now_ms(),
        )?;
        match answer {
            OfferAnswer::Import(record) => job.commit(record, Author::Node(self.id))?,
            OfferAnswer::Decided(_) => {}
            OfferAnswer::Rejected { head } => {
                return Err(NodeError::peer_fact(
                    holder,
                    format!(
                        "its move does not link to this node's head {head}; run the resume again"
                    ),
                    Reason::MoveUnlinked {
                        job: id,
                        peer: holder,
                        head,
                    },
                ));
            }
            OfferAnswer::SnapshotIncomplete { snapshot } => {
                return Err(NodeError::peer_fact(
                    holder,
                    format!("snapshot {snapshot} did not arrive whole; run the resume again"),
                    Reason::Fetch {
                        snapshot,
                        peer: holder,
                    },
                ));
            }
            OfferAnswer::Stale { current, .. } => {
                return Err(NodeError::peer_fact(
                    holder,
                    format!("the move is at an epoch behind {current}"),
                    Reason::MoveStale {
                        job: id,
                        peer: holder,
                        epoch: current,
                    },
                ));
            }
        }
        out.emit(Event::ImportedHere {
            move_id,
            epoch: job.ledger().lease().epoch(),
        });
        Ok(())
    }

    /// Fetches the records `holder` has after this node's head; makes the job if it is new here.
    pub(crate) async fn pull_records(
        self: &Arc<Self>,
        id: JobId,
        holder: NodeId,
    ) -> Result<(), NodeError> {
        loop {
            let (after, epoch) = match self.job(id).await {
                Ok(job) => {
                    let job = job.lock().await;
                    (Some(job.ledger().head()), job.ledger().lease().epoch())
                }
                Err(_) => (None, Epoch::FIRST),
            };
            let token = FencingToken { job_id: id, epoch };
            let answer = self
                .call(holder, |session| async move {
                    session.pull(token, after, SHORT_DEADLINE).await
                })
                .await?;
            let (page, more) = match answer {
                Fenced::Answer(Pulled::UpToDate) => return Ok(()),
                Fenced::Answer(Pulled::Last(page)) => (page, false),
                Fenced::Answer(Pulled::More(page)) => (page, true),
                Fenced::Answer(Pulled::NotInChain { head }) => {
                    let behind = {
                        let job = self.job(id).await?;
                        let job = job.lock().await;
                        job.ledger().suffix_after(head).is_some()
                    };
                    if behind {
                        return Ok(());
                    }
                    return Err(Box::pin(self.forked_from(id, holder, head)).await);
                }
                fenced => {
                    return Err(NodeError::Peer {
                        peer: holder,
                        reason: format!("it did not answer the pull: {}", fenced_words(&fenced)),
                    });
                }
            };
            if !self.take_page(id, holder, &page).await? {
                // Another pull moved the head while this page was in
                // flight; asking again from the new head either links or
                // is answered NotInChain, which the fork rule above takes.
                continue;
            }
            if !more {
                return Ok(());
            }
        }
    }

    /// Takes a pulled page; false when it no longer links to this node's
    /// head because the head moved after the page was asked for.
    pub(crate) async fn take_page(
        &self,
        id: JobId,
        holder: NodeId,
        page: &RecordPage,
    ) -> Result<bool, NodeError> {
        for bytes in page.records() {
            match self.accept_record(id, holder, bytes.as_bytes()).await {
                Ok(_) => {}
                Err(NodeError::Ledger(fabric_core::ledger::LedgerError::PrevMismatch {
                    ..
                })) => return Ok(false),
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }

    /// Takes one record from `peer`, making the job here from its `Created` record.
    pub(crate) async fn accept_record(
        &self,
        id: JobId,
        peer: NodeId,
        bytes: &[u8],
    ) -> Result<bool, NodeError> {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get(&id).cloned() {
            drop(jobs);
            let mut job = job.lock().await;
            return job.accept(bytes, peer, self.id);
        }
        let record = fabric_core::ledger::Record::decode(bytes)
            .map_err(fabric_core::ledger::LedgerError::from)?;
        let job = Job::create(id, self.cfg.state.job(id), &record)?;
        jobs.insert(id, Arc::new(tokio::sync::Mutex::new(job)));
        Ok(true)
    }

    /// Fetches the latest published snapshot from `peer` unless the store holds it whole.
    pub(crate) async fn ensure_latest(
        self: &Arc<Self>,
        id: JobId,
        peer: NodeId,
    ) -> Result<(), NodeError> {
        let latest = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            job.latest()
        };
        let Some(snapshot) = latest else {
            return Ok(());
        };
        if is_complete(&self.store, snapshot).await? {
            return Ok(());
        }
        let session = self.session(peer).await?;
        match fetch_snapshot(&self.store, &session, snapshot).await {
            Ok(()) => Ok(()),
            Err(first) => {
                self.forget(peer, &session);
                let session = self.dial(peer).await.map_err(|_| first)?;
                fetch_snapshot(&self.store, &session, snapshot).await
            }
        }
    }

    /// Restores the latest snapshot at the root unless the workspace of this
    /// tenure, at `epoch`, is already there.
    ///
    /// A workspace an earlier tenure left is set aside first, with the
    /// session record it continued, so the job never continues from a tree
    /// or a session older than its latest snapshot.
    async fn restore_if_needed(
        &self,
        job: &mut Job,
        epoch: Epoch,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<PathBuf, NodeError> {
        let id = job.id();
        let marker = self.tenure_marker(job, epoch, out).await?;
        match &marker {
            Some(Marker {
                root,
                state: MarkerState::Live | MarkerState::Clean { .. },
                ..
            }) => {
                out.emit(Event::WorkspaceInPlace { root: root.clone() });
                return Ok(root.clone());
            }
            Some(Marker {
                state: MarkerState::Suspending { .. },
                ..
            }) => {
                return Err(NodeError::Unfinished {
                    job: id,
                    step: Step::Suspend,
                });
            }
            _ => {}
        }
        let snapshot = job.latest().ok_or(NodeError::NoSnapshot(id))?;
        let manifest = with_store(&self.store, move |store| Ok(store.manifest(snapshot)?)).await?;
        let choice = restore_root(
            Path::new(manifest.root.as_str()),
            &self.cfg.home,
            id,
            job.dir(),
        );
        out.emit(match &choice {
            RootChoice::Same(root) => Event::RootChosen {
                root: root.clone(),
                recorded: None,
            },
            RootChoice::Fallback { root, captured } => Event::RootChosen {
                root: root.clone(),
                recorded: Some(captured.clone()),
            },
        });
        let root = choice.root().to_owned();
        check_root(
            &root,
            &self.cfg.home,
            synced_folders(&self.cfg.home, env).await,
        )?;
        let resuming = matches!(
            &marker,
            Some(Marker { state: MarkerState::Restoring { snapshot: s }, .. }) if *s == snapshot
        );
        let state = restore_state(&root)?;
        if !resuming {
            if state == RestoreState::Present {
                return Err(NodeError::WorkspaceOccupied { root });
            }
            job.clear_harness_state()?;
            local::remove(&job.dir().session())?;
        }
        job.set_marker(&Marker {
            root: root.clone(),
            state: MarkerState::Restoring { snapshot },
            epoch,
        })?;
        if let Some(parent) = root.parent() {
            crate::durable::create_dir(parent).map_err(|e| NodeError::io(parent, e))?;
        }
        let has_harness_state = !manifest.harness_state.entries.is_empty();
        let state_dir = job.dir().harness_state();
        if !(resuming && state == RestoreState::Present) {
            let harness_dir = has_harness_state.then(|| state_dir.clone());
            self.write_snapshot(snapshot, &manifest.caches, &root, harness_dir, env, out)
                .await?;
        }
        if has_harness_state {
            self.restore_harness(job, &root, env, out).await?;
        }
        let tree = TreeStamp::measure(&root).await;
        job.set_marker(&Marker {
            root: root.clone(),
            state: MarkerState::Clean { snapshot, tree },
            epoch,
        })?;
        Ok(root)
    }

    /// The marker of a workspace this tenure, at `epoch`, can continue, or
    /// `None` when the latest snapshot must be restored.
    ///
    /// A workspace an earlier tenure left is set aside. A clean workspace
    /// holds nothing the latest snapshot lacks, so one removed or moved
    /// outside Fabric is restored from it.
    async fn tenure_marker(
        &self,
        job: &mut Job,
        epoch: Epoch,
        out: &mut Out,
    ) -> Result<Option<Marker>, NodeError> {
        Ok(match job.reconcile_marker()? {
            Some(marker) if !marker.is_current(epoch, job.latest()).await => {
                self.set_orphan_aside(job, &marker, epoch, out).await?;
                None
            }
            Some(Marker {
                root,
                state: MarkerState::Clean { .. },
                ..
            }) if is_missing(&root)? => {
                out.emit(Event::WorkspaceGone { root });
                None
            }
            marker => marker,
        })
    }

    /// Writes `snapshot` into the empty `root`, carrying each cache whose key matches this node's.
    async fn write_snapshot(
        &self,
        snapshot: SnapshotId,
        caches: &[fabric_core::snapshot::CacheEntry],
        root: &Path,
        harness_dir: Option<PathBuf>,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let facts = NodeFacts::measure(&self.cfg.home, env).await;
        report_caches(caches, &facts, out);
        let target = root.to_owned();
        let report = with_store(&self.store, move |store| {
            let carry = |entry: &fabric_core::snapshot::CacheEntry| {
                entry.key.matches(&facts.key_like(&entry.key))
            };
            Ok(restore(
                store,
                &RestoreRequest {
                    snapshot,
                    target: &target,
                    harness_state: harness_dir.as_deref(),
                    verify: Verify::Sample(32),
                    durability: Durability::PowerLoss,
                    carry: &carry,
                },
            )?)
        })
        .await?;
        out.emit(Event::Restored {
            snapshot,
            files: report.files,
            bytes: report.bytes,
        });
        Ok(())
    }

    /// Moves a workspace an earlier tenure left out of the root, next to it
    /// under `orphan/<epoch>`, and forgets it and the session it continued.
    ///
    /// Nothing is deleted. A harness still recorded from that tenure is
    /// stopped first, so nothing writes to the tree while it moves.
    async fn set_orphan_aside(
        &self,
        job: &mut Job,
        marker: &Marker,
        epoch: Epoch,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        self.settle(job, out).await?;
        let base = paths::orphan_copy(&marker.root, marker.epoch)
            .ok_or_else(|| no_parent(&marker.root))?;
        let dest = unused(&base);
        if move_aside(&marker.root, &dest)? {
            out.emit(Event::OrphanSetAside {
                root: marker.root.clone(),
                left_at: marker.epoch,
                epoch,
                to: dest,
            });
        }
        local::remove(&job.dir().session())?;
        job.clear_marker()
    }

    /// Places the captured harness state where its harness expects it and records the session.
    async fn restore_harness(
        &self,
        job: &Job,
        root: &Path,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let state_dir = job.dir().harness_state();
        let state = StateManifest::read(&state_dir)?;
        let harness = adapter(state.kind(), &self.harness_env(env)?)?;
        harness.restore(root, &state_dir).await?;
        local::write(
            &job.dir().session(),
            &SessionRecord {
                harness: state.kind().clone(),
                session: state.session().clone(),
            },
        )?;
        out.emit(Event::SessionRestored {
            harness: state.kind().clone(),
            session: state.session().clone(),
        });
        Ok(())
    }

    /// Steals `id` and returns the lease the steal made.
    ///
    /// The lease is read under the lock that commits the steal: the push to
    /// the old owner waits without it, and another steal or a move may
    /// change the lease meanwhile.
    async fn job_steal(
        self: &Arc<Self>,
        id: JobId,
        reason: String,
        force: bool,
        out: &mut Out,
    ) -> Result<Lease, NodeError> {
        let (owner, complete) = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            // A frozen chain pushed at a higher epoch would freeze the job
            // on the real owner too, for good.
            if let Some(fork) = job.forked()? {
                return Err(NodeError::Forked {
                    job: id,
                    peer: fork.peer,
                    epoch: fork.epoch,
                });
            }
            let latest = job.latest().ok_or(NodeError::NoSnapshot(id))?;
            let complete = is_complete(&self.store, latest)
                .await?
                .then_some(CompleteSnapshot::new(latest));
            (job.ledger().lease().holder(), complete)
        };
        let stolen = {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            let proposal = job
                .ledger()
                .steal(
                    self.id,
                    reason,
                    complete,
                    clock::event_id(),
                    clock::now_ms(),
                )
                .map_err(|e| steal_refused(&job, self.id, e))?;
            let age_ms = clock::now_ms().saturating_sub(proposal.warning.last_record_at);
            let described = proposal.warning.with_age(age_ms);
            let warning = StealWarning {
                owner: proposal.warning.owner,
                snapshot: proposal.warning.snapshot,
                owner_stopped_at_snapshot: proposal.warning.owner_stopped_at_snapshot,
                last_record_at: proposal.warning.last_record_at,
                age_ms,
            };
            if !force {
                return Err(NodeError::StealNeedsForce { warning, described });
            }
            out.emit(Event::StealWarning { warning });
            job.commit(proposal.record, Author::Node(self.id))?;
            job.append(self.id, EventKind::Note { text: described })?;
            let stolen = lease_of(&job);
            out.emit(Event::Stole {
                job: id,
                epoch: stolen.epoch,
            });
            self.settle(&mut job, out).await?;
            stolen
        };
        if let Err(e) = self.push_records(id, owner).await {
            out.emit(Event::OwnerNotTold {
                owner,
                error: e.to_refusal(),
            });
        }
        Ok(stolen)
    }

    async fn job_decide(
        &self,
        id: JobId,
        decision: Decision,
        note: &str,
    ) -> Result<String, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        owner_token(&job, self.id)?;
        let word = match decision {
            Decision::Approve => "approved",
            Decision::Reject => "rejected",
        };
        let text = if note.is_empty() {
            word.to_owned()
        } else {
            format!("{word}: {note}")
        };
        job.append(self.id, EventKind::DecisionRecorded { text: text.clone() })?;
        Ok(text)
    }

    async fn doctor(
        self: &Arc<Self>,
        peer: Option<NodeId>,
        job: Option<JobId>,
        env: &ToolEnv,
    ) -> Result<DoctorReport, NodeError> {
        let Some(peer) = peer else {
            let facts = NodeFacts::measure(&self.cfg.home, env).await;
            return Ok(DoctorReport::Facts {
                facts: facts
                    .iter()
                    .map(|(name, value)| Fact {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    })
                    .collect(),
            });
        };
        let (harness, root) = match job {
            Some(id) => {
                let job = self.job(id).await?;
                let job = job.lock().await;
                let root = job
                    .marker()?
                    .map_or_else(|| paths::job_root(&self.cfg.home, id), |m| m.root);
                (Some(job.harness().clone()), Some(root))
            }
            None => (None, None),
        };
        let rows = self
            .compare(
                peer,
                harness.as_ref().zip(root.as_deref()),
                Target::Peer,
                env,
            )
            .await?;
        Ok(DoctorReport::Compared {
            peer,
            rows: rows.iter().map(doctor::Row::to_wire).collect(),
        })
    }

    /// Compares this node's facts with `peer`'s, reports the rows, and
    /// stops on a refusal `force` does not waive.
    async fn preflight(
        self: &Arc<Self>,
        peer: NodeId,
        job: Option<(&HarnessKind, &Path)>,
        target: Target,
        env: &ToolEnv,
        force: bool,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let rows = self.compare(peer, job, target, env).await?;
        out.emit(Event::Preflight {
            peer,
            rows: rows.iter().map(doctor::Row::to_wire).collect(),
        });
        let blocking = doctor::blocking(&rows, force);
        if blocking.is_empty() {
            Ok(())
        } else {
            Err(NodeError::Preflight(
                blocking.iter().map(|row| row.fact.to_owned()).collect(),
            ))
        }
    }

    /// This node's facts compared with `peer`'s, row by row (ADR 0008).
    async fn compare(
        self: &Arc<Self>,
        peer: NodeId,
        job: Option<(&HarnessKind, &Path)>,
        target: Target,
        env: &ToolEnv,
    ) -> Result<Vec<doctor::Row>, NodeError> {
        let here = NodeFacts::measure(&self.cfg.home, env).await;
        // A session the peer opened is used too: a peer behind a NAT has
        // no address here and is reachable only over the session it dialed.
        let (theirs, clock_skew_ms, wire) = self
            .call(peer, |session| async move {
                let facts = session.facts(SHORT_DEADLINE * 4).await?;
                Ok((facts, session.clock_skew_ms(), session.kind().to_string()))
            })
            .await?;
        let theirs = NodeFacts::from_wire(&theirs);
        Ok(doctor::compare(
            &here,
            &theirs,
            &Context {
                harness: job.map(|(h, _)| h),
                root: job.map(|(_, r)| r),
                target,
                clock_skew_ms: Some(clock_skew_ms),
                wire: Some(wire),
            },
        ))
    }
}

/// What a call that reached `peer` and failed means: a peer that gave no
/// answer in time was not reached, as far as the user can tell, and
/// anything else is the peer's answer.
fn answer_error(peer: NodeId, err: &fabric_net::rpc::RpcError) -> NodeError {
    match err {
        fabric_net::rpc::RpcError::Deadline(_) => NodeError::Unreachable {
            peer,
            reason: err.to_string(),
        },
        _ => NodeError::Peer {
            peer,
            reason: err.to_string(),
        },
    }
}

/// The refusal of a steal `me` asked for and the ledger of `job` did not
/// take, with the facts of the job that explain it.
/// Who owns `job` now, at which epoch; the caller holds its lock.
fn lease_of(job: &Job) -> Lease {
    let lease = job.ledger().lease();
    Lease {
        owner: lease.holder(),
        epoch: lease.epoch(),
    }
}

fn steal_refused(job: &Job, me: NodeId, error: LedgerError) -> NodeError {
    let id = job.id();
    let fact = match (&error, job.view(me)) {
        (LedgerError::StealBySelf, LeaseView::Departing { move_id, target }) => {
            Reason::StealWhileDeparting {
                job: id,
                move_id,
                target,
            }
        }
        (LedgerError::StealBySelf, _) => Reason::StealByOwner { job: id },
        (LedgerError::StealByTarget { move_id }, _) => Reason::StealByTarget {
            job: id,
            move_id: *move_id,
        },
        (LedgerError::StealWithoutSnapshot, _) => match job.latest() {
            Some(snapshot) => Reason::StealWithoutSnapshot { job: id, snapshot },
            None => return NodeError::NoSnapshot(id),
        },
        _ => return error.into(),
    };
    NodeError::ledger_fact(error, fact)
}

/// The owner's fencing token, or the refusal that says who owns the job.
fn owner_token(job: &Job, me: NodeId) -> Result<FencingToken, NodeError> {
    let id = job.id();
    if let Some(fork) = job.forked()? {
        return Err(NodeError::Forked {
            job: id,
            peer: fork.peer,
            epoch: fork.epoch,
        });
    }
    match job.view(me) {
        LeaseView::Owner(token) => Ok(token),
        LeaseView::Departing { move_id, target } => Err(NodeError::Departing {
            job: id,
            move_id,
            target,
        }),
        LeaseView::Departed { owner, epoch } => Err(NodeError::Departed {
            job: id,
            owner,
            epoch,
        }),
        LeaseView::Replica { owner, epoch } => Err(NodeError::NotOwner {
            job: id,
            owner,
            epoch,
        }),
    }
}

/// What to launch for `job` on its owner.
///
/// The workspace is marked live before the plan leaves the node: from here
/// on a harness may write to it, so a move refuses to depart from the
/// snapshot it was copying and a departed copy is never moved aside as if
/// it held nothing new.
fn plan(
    job: &Job,
    epoch: Epoch,
    root: PathBuf,
    harness: Option<HarnessKind>,
    harness_config: &HarnessConfig,
    out: &mut Out,
) -> Result<RunPlan, NodeError> {
    job.set_marker(&Marker {
        root: root.clone(),
        state: MarkerState::Live,
        epoch,
    })?;
    let harness = harness.unwrap_or_else(|| job.harness().clone());
    let session = match job.session()? {
        Some(record) if record.harness == harness => {
            out.emit(Event::ContinuingSession {
                harness: harness.clone(),
                session: record.session.clone(),
            });
            Some(record.session)
        }
        Some(record) => {
            out.emit(Event::NewSession {
                harness: harness.clone(),
                previous: Some(HarnessSession {
                    harness: record.harness,
                    session: record.session,
                }),
            });
            None
        }
        None => {
            out.emit(Event::NewSession {
                harness: harness.clone(),
                previous: None,
            });
            None
        }
    };
    Ok(RunPlan {
        job: job.id(),
        epoch,
        root,
        harness,
        session,
        goal: job.ledger().manifest().goal.clone(),
        harness_config: harness_config.into(),
    })
}

/// Says, one line per cache, whether its bytes are written here or rebuilt, and why (ADR 0008).
fn report_caches(caches: &[fabric_core::snapshot::CacheEntry], facts: &NodeFacts, out: &mut Out) {
    for cache in caches {
        let mine = facts.key_like(&cache.key);
        if cache.key.matches(&mine) && cache.carry == fabric_core::snapshot::CarryState::Carried {
            out.emit(Event::CacheCarried {
                path: cache.path.to_string(),
            });
        } else {
            out.emit(Event::CacheRebuilt {
                path: cache.path.to_string(),
                why: key_difference(&cache.key, &mine),
            });
        }
    }
}

/// Whether nothing, not even a dangling link, is at `path`.
fn is_missing(path: &Path) -> Result<bool, NodeError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(NodeError::io(path, e)),
    }
}

fn no_parent(root: &Path) -> NodeError {
    NodeError::io(root, std::io::Error::other("a root needs a parent"))
}

/// `base`, or the first of `base.1`, `base.2` and on that does not exist yet.
fn unused(base: &Path) -> PathBuf {
    let mut candidate = base.to_owned();
    let mut n = 0_u32;
    while candidate.symlink_metadata().is_ok() {
        n += 1;
        let mut name = base.as_os_str().to_owned();
        name.push(format!(".{n}"));
        candidate = PathBuf::from(name);
    }
    candidate
}

/// Renames the workspace at `root` to `dest` durably; false when there is no workspace at `root`.
fn move_aside(root: &Path, dest: &Path) -> Result<bool, NodeError> {
    if !root.exists() {
        return Ok(false);
    }
    let parent = dest.parent().ok_or_else(|| no_parent(dest))?;
    crate::durable::create_dir(parent).map_err(|e| NodeError::io(parent, e))?;
    std::fs::rename(root, dest).map_err(|e| NodeError::io(root, e))?;
    crate::durable::sync_dir(parent).map_err(|e| NodeError::io(parent, e))?;
    if let Some(root_parent) = root.parent() {
        crate::durable::sync_dir(root_parent).map_err(|e| NodeError::io(root_parent, e))?;
    }
    Ok(true)
}

/// Reads which folders iCloud Drive syncs on this node; off everywhere `defaults` does not exist.
async fn synced_folders(home: &Path, env: &ToolEnv) -> SyncedFolders {
    let path_var = env.path_var().to_owned();
    let read = |key: &'static str| {
        let path_var = path_var.clone();
        let home = home.to_owned();
        async move {
            let Some(binary) = crate::facts::find_on_path("defaults", &path_var) else {
                return false;
            };
            tokio::process::Command::new(binary)
                .args(["read", "com.apple.finder", key])
                .env("HOME", home)
                .output()
                .await
                .is_ok_and(|o| {
                    o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "1"
                })
        }
    };
    let (desktop, documents) =
        tokio::join!(read("FXICloudDriveDesktop"), read("FXICloudDriveDocuments"));
    SyncedFolders { desktop, documents }
}

/// A fenced answer that is not an answer, in words.
fn fenced_words<T>(answer: &Fenced<T>) -> String {
    match answer {
        Fenced::Answer(_) => "it answered".to_owned(),
        Fenced::StaleEpoch { current, .. } => {
            format!("it knows epoch {current}, newer than this node's")
        }
        Fenced::AheadOfUs { current, .. } => {
            format!("it is at epoch {current}, behind this node's, and must catch up first")
        }
    }
}

/// A target's answer to an offer that is not a decision, in words.
fn offered_words(answer: &Fenced<Offered>) -> String {
    match answer {
        Fenced::Answer(Offered::Decided { .. }) => "it decided".to_owned(),
        Fenced::Answer(Offered::Rejected { head }) => {
            format!("the offer does not link to its head {head}")
        }
        Fenced::Answer(Offered::SnapshotIncomplete { snapshot }) => {
            format!("it does not hold snapshot {snapshot} whole")
        }
        fenced => fenced_words(fenced),
    }
}

/// What `status` says about the workspace `marker` names, from the marker
/// and how this node relates to the job together.
async fn workspace_state(job: &Job, me: NodeId, marker: &Marker, frozen: bool) -> WorkspaceState {
    match job.view(me) {
        LeaseView::Owner(token) if !frozen => {
            if marker.is_current(token.epoch, job.latest()).await {
                WorkspaceState::Current {
                    marker: marker_state(&marker.state),
                }
            } else {
                WorkspaceState::Stale {
                    left_at: marker.epoch,
                }
            }
        }
        LeaseView::Departing { target, .. } if !frozen && marker.is_unchanged().await => {
            WorkspaceState::AwaitingDecision { target }
        }
        _ if !frozen && marker.is_unchanged().await => WorkspaceState::CleanCopy,
        _ => WorkspaceState::Orphan,
    }
}

fn standing(view: &LeaseView) -> Standing {
    match view {
        LeaseView::Owner(_) => Standing::Owner,
        LeaseView::Departing { .. } => Standing::Departing,
        LeaseView::Departed { .. } => Standing::Departed,
        LeaseView::Replica { .. } => Standing::Replica,
    }
}

fn marker_state(state: &MarkerState) -> fabric_client::api::Marker {
    use fabric_client::api::Marker as Shown;
    match state {
        MarkerState::Live => Shown::Live,
        MarkerState::Suspending { .. } => Shown::Suspending,
        MarkerState::Clean { snapshot, .. } => Shown::Clean {
            snapshot: *snapshot,
        },
        MarkerState::Restoring { snapshot } => Shown::Restoring {
            snapshot: *snapshot,
        },
    }
}

fn ledger_event(kind: &EventKind) -> LedgerEvent {
    match kind.clone() {
        EventKind::Created {
            goal,
            harness,
            created_by,
            ..
        } => LedgerEvent::Created {
            created_by,
            harness,
            goal,
        },
        EventKind::MoveStarted { move_id, target } => LedgerEvent::MoveStarted { move_id, target },
        EventKind::MoveImported { move_id, from } => LedgerEvent::MoveImported { move_id, from },
        EventKind::MoveAborted { move_id } => LedgerEvent::MoveAborted { move_id },
        EventKind::Stolen { by, from, reason } => LedgerEvent::Stolen { by, from, reason },
        EventKind::SnapshotPublished { snapshot, parent } => {
            LedgerEvent::SnapshotPublished { snapshot, parent }
        }
        EventKind::Suspended { snapshot } => LedgerEvent::Suspended { snapshot },
        EventKind::Resumed {
            node,
            snapshot,
            harness,
        } => LedgerEvent::Resumed {
            node,
            snapshot,
            harness,
        },
        EventKind::EnvSpecChanged { env_spec } => LedgerEvent::EnvSpecChanged { env_spec },
        EventKind::DecisionRecorded { text } => LedgerEvent::DecisionRecorded { text },
        EventKind::Note { text } => LedgerEvent::Note { text },
    }
}

/// The client API's name for the wire a session runs on.
fn wire(kind: fabric_net::transport::TransportKind) -> Wire {
    match kind {
        fabric_net::transport::TransportKind::Quic => Wire::Quic,
        fabric_net::transport::TransportKind::TcpTls => Wire::TcpTls,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    use fabric_net::endpoint::DialOrder;

    use super::*;
    use crate::node::NodeConfig;
    use crate::paths::StateDir;

    /// A node in a fresh directory under `/tmp`, which keeps the socket path short.
    pub(crate) fn node() -> (tempfile::TempDir, Arc<Node>) {
        let dir = tempfile::Builder::new()
            .prefix("fops")
            .tempdir_in("/tmp")
            .unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let node = Node::start(NodeConfig {
            state: StateDir::resolve(Some(root.join("state").as_os_str()), &home).unwrap(),
            home,
            listen: "127.0.0.1:0".parse().unwrap(),
            dial: DialOrder::TcpOnly,
            env: ToolEnv::current(),
        })
        .unwrap();
        (dir, node)
    }

    /// A job made from a one-commit repository, owned by `node` at epoch 1.
    pub(crate) async fn job(dir: &Path, node: &Arc<Node>) -> JobId {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        for args in [
            &["init", "-q"][..],
            &["add", "-A"],
            &[
                "-c",
                "user.name=f",
                "-c",
                "user.email=f@x",
                "commit",
                "-qm",
                "1",
            ],
        ] {
            let status = std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        }
        node.job_new(&repo, "goal".to_owned(), HarnessKind::Codex)
            .await
            .unwrap()
            .job
    }

    fn sleeper() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn pid(child: &std::process::Child) -> NonZeroU32 {
        NonZeroU32::new(child.id()).unwrap()
    }

    async fn register(node: &Arc<Node>, id: JobId, pid: NonZeroU32) -> Result<(), NodeError> {
        node.harness_started(id, Epoch::FIRST, pid, HarnessKind::Codex, None)
            .await
            .map(drop)
    }

    /// Serves `a` and `b` and pairs them, each with the other's address.
    async fn paired(a: &Arc<Node>, b: &Arc<Node>) {
        for (node, peer) in [(a, b), (b, a)] {
            tokio::spawn(Arc::clone(node).run());
            let addr = peer.local_addr().unwrap().to_string();
            node.node_add(peer.id, Some(&addr)).await.unwrap();
        }
    }

    /// A node runs the job, so its marker says live, and a peer that has
    /// caught up with that, with no fork, steals it. The thief works and
    /// suspends; the job comes back. The workspace the first node left is
    /// not the job's: the restore must set it aside and bring the thief's
    /// snapshot and session state, not continue the stale tree.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_job_that_comes_back_sets_the_stale_workspace_aside() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let path = ToolEnv::current();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, out).await.unwrap();
        a.job_run(id, out).await.unwrap();
        let harness = sleeper();
        register(&a, id, pid(&harness)).await.unwrap();
        let reaper = std::thread::spawn(move || {
            let mut harness = harness;
            harness.wait()
        });
        let stale_root = paths::job_root(&a.cfg.home, id);
        std::fs::write(stale_root.join("README.md"), "a's stale edit\n").unwrap();
        local::write(
            &a.cfg.state.job(id).session(),
            &SessionRecord {
                harness: HarnessKind::Codex,
                session: SessionId::parse("stale-session").unwrap(),
            },
        )
        .unwrap();

        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        b.job_steal(id, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        reaper.join().unwrap().unwrap();
        let thief_root = {
            let job = b.job(id).await.unwrap();
            let mut job = job.lock().await;
            b.restore_if_needed(&mut job, Epoch::new(2).unwrap(), &path, out)
                .await
                .unwrap()
        };
        std::fs::write(thief_root.join("thief.txt"), "b's work\n").unwrap();
        b.suspend(id, &path, out).await.unwrap();

        a.pull(id, b.id, out).await.unwrap();
        let job = a.job(id).await.unwrap();
        let mut job = job.lock().await;
        let epoch = owner_token(&job, a.id).unwrap().epoch;
        let root = a
            .restore_if_needed(&mut job, epoch, &path, out)
            .await
            .unwrap();
        let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();
        assert_eq!(read(root.join("thief.txt")), "b's work\n");
        assert_eq!(read(root.join("README.md")), "hello\n");
        let orphan = paths::orphan_copy(&stale_root, Epoch::FIRST).unwrap();
        assert_eq!(read(orphan.join("README.md")), "a's stale edit\n");
        assert!(
            job.session().unwrap().is_none(),
            "the stale session would be continued"
        );
    }

    /// A headless Codex run names its thread only once it runs. The node
    /// records that session for the harness it recorded and for no other
    /// pid, and not once that harness was stopped, because the suspend or
    /// fence that stopped it captured without the session.
    #[tokio::test]
    async fn a_session_named_after_the_start_is_recorded_only_for_the_recorded_harness() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = ToolEnv::current();
        let out = &mut Out::default();
        node.suspend(id, &path, out).await.unwrap();
        node.job_run(id, out).await.unwrap();
        let mut harness = sleeper();
        register(&node, id, pid(&harness)).await.unwrap();
        let thread = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let name_at = |epoch, pid| {
            let (node, thread) = (Arc::clone(&node), thread.clone());
            async move {
                node.harness_session(id, epoch, pid, HarnessKind::Codex, thread)
                    .await
            }
        };
        let name = |pid| name_at(Epoch::FIRST, pid);

        let other = NonZeroU32::new(pid(&harness).get() + 1).unwrap();
        assert!(matches!(
            name(other).await,
            Err(NodeError::HarnessNotRecorded { .. })
        ));
        assert!(
            matches!(
                name_at(Epoch::new(2).unwrap(), pid(&harness)).await,
                Err(NodeError::Fence(_))
            ),
            "a plan of another epoch named the session"
        );
        let recorded = || async {
            let job = node.job(id).await.unwrap();
            let job = job.lock().await;
            job.session().unwrap()
        };
        assert_eq!(recorded().await, None);
        name(pid(&harness)).await.unwrap();
        assert_eq!(
            recorded().await,
            Some(SessionRecord {
                harness: HarnessKind::Codex,
                session: thread.clone(),
            })
        );

        harness.kill().unwrap();
        harness.wait().unwrap();
        node.harness_exited(id, pid(&harness)).await.unwrap();
        assert!(matches!(
            name(pid(&harness)).await,
            Err(NodeError::HarnessNotRecorded { .. })
        ));
    }

    /// Two moves typed at once toward different nodes: the second finds the
    /// job departing toward the first's target and must name that node,
    /// not offer the first move to its own.
    #[tokio::test]
    async fn a_move_toward_another_node_refuses_with_the_open_moves_target() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = ToolEnv::current();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let (first, second) = (NodeId::from_bytes([7; 32]), NodeId::from_bytes([8; 32]));
        node.depart(id, first, &mut Out::default()).await.unwrap();
        let refused = node.depart(id, second, &mut Out::default()).await;
        assert!(
            matches!(refused, Err(NodeError::Departing { target, .. }) if target == first),
            "{refused:?}"
        );
    }

    #[test]
    fn a_peer_that_answers_too_late_counts_as_unreachable() {
        let peer = NodeId::from_bytes([7; 32]);
        let late = answer_error(
            peer,
            &fabric_net::rpc::RpcError::Deadline(Duration::from_secs(15)),
        );
        assert_eq!(
            late.to_refusal().kind(),
            fabric_client::api::RefusalKind::Unreachable
        );
    }

    /// Users read status when a move is frozen; a clean copy waiting for
    /// the target must not be called an orphan with unsaved work.
    #[tokio::test]
    async fn status_calls_a_departing_clean_copy_clean() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = ToolEnv::current();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        node.depart(id, NodeId::from_bytes([7; 32]), &mut Out::default())
            .await
            .unwrap();
        let status = node.job_status(id).await.unwrap();
        let state = status.workspace.unwrap().state;
        assert!(
            matches!(state, WorkspaceState::AwaitingDecision { .. }),
            "{state:?}"
        );
    }

    /// A node frozen after a fork must not steal on its forked chain: the
    /// steal would reach the real owner at a higher epoch and freeze the
    /// job there as well, for good.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_frozen_node_refuses_to_steal() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let path = ToolEnv::current();
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        b.freeze(
            id,
            &crate::local::ForkRecord {
                peer: a.id,
                epoch: Epoch::FIRST,
                head: fabric_core::id::RecordHash::of(b"a head"),
            },
            &mut Out::default(),
        )
        .await
        .unwrap();
        let refused = b
            .job_steal(id, String::new(), true, &mut Out::default())
            .await;
        assert!(
            matches!(refused, Err(NodeError::Forked { .. })),
            "{refused:?}"
        );
    }

    /// A pull of a job this node has never seen, or under another harness,
    /// must be refused when that harness cannot run here, before the job
    /// changes hands. The test home has no Codex or Claude login.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pull_preflights_the_harness_that_will_launch() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let path = ToolEnv::current();
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        for (harness, login) in [
            (None, crate::facts::name::CODEX_LOGIN),
            (
                Some(HarnessKind::ClaudeCode),
                crate::facts::name::CLAUDE_LOGIN,
            ),
        ] {
            let refused = b
                .job_resume(id, Some(a.id), harness, true, &path, &mut Out::default())
                .await;
            assert!(
                matches!(&refused, Err(NodeError::Preflight(facts)) if facts.iter().any(|fact| fact == login)),
                "{refused:?}"
            );
        }
        let job = a.job(id).await.unwrap();
        assert!(matches!(
            job.lock().await.view(a.id),
            LeaseView::Owner(token) if token.epoch == Epoch::FIRST
        ));
    }

    /// A suspend must not publish a snapshot whose `.fabric/env.toml`
    /// does not parse: the target would take the job and then fail to
    /// materialize it.
    #[tokio::test]
    async fn a_suspend_refuses_a_workspace_whose_environment_does_not_detect() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(root.join(".fabric/env.toml"), "not toml [\n").unwrap();
        let path = ToolEnv::current();
        let refused = node.suspend(id, &path, &mut Out::default()).await;
        assert!(matches!(refused, Err(NodeError::Detect(_))), "{refused:?}");
        let job = node.job(id).await.unwrap();
        assert_eq!(job.lock().await.latest(), None);
    }

    /// A secret pattern the job adds in `.fabric/env.toml` keeps a matching
    /// file on this node, as a built-in pattern does.
    #[tokio::test]
    async fn a_suspend_leaves_behind_what_the_jobs_secret_patterns_match() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "secret_patterns = [\"*.sqlite\"]\n",
        )
        .unwrap();
        std::fs::write(root.join("users.sqlite"), "rows\n").unwrap();
        let path = ToolEnv::current();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        assert!(
            out.events
                .iter()
                .any(|event| matches!(event, Event::LeftBehind { path } if path == "users.sqlite")),
            "{:?}",
            out.events
        );
    }

    /// A clean workspace deleted by hand is restored by the next resume,
    /// not handed to the CLI as a root that does not exist.
    #[tokio::test]
    async fn a_resume_restores_a_clean_workspace_that_was_deleted() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = ToolEnv::current();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::remove_dir_all(&root).unwrap();
        let plan = node
            .job_resume(id, None, None, false, &path, &mut Out::default())
            .await
            .unwrap();
        assert_eq!(plan.root, root);
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            "hello\n"
        );
    }

    /// The CLI launches with the configuration the daemon captures and
    /// restores with, the one it read at start, even after the file changed.
    #[tokio::test]
    async fn a_run_plan_carries_the_harness_config_the_node_started_with() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        std::fs::write(
            node.cfg.state.harness_config(),
            "[acp.opencode]\nstate_dir = \"elsewhere\"\n",
        )
        .unwrap();
        let plan = node.job_run(id, &mut Out::default()).await.unwrap();
        let sent: RunPlan = serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        assert!(sent.harness_config.acp.is_empty());
        assert_eq!(
            crate::harnesses::HarnessConfig::from(sent.harness_config),
            node.harness_config
        );
    }

    /// A peer paired without an address, as the Air pairs the Pro, is
    /// reachable over the session it opened, so doctor works from this side.
    #[tokio::test(flavor = "multi_thread")]
    async fn doctor_reaches_an_addressless_peer_over_its_session() {
        let (_dir_a, a) = node();
        let (_dir_b, b) = node();
        tokio::spawn(Arc::clone(&a).run());
        tokio::spawn(Arc::clone(&b).run());
        let addr = b.local_addr().unwrap().to_string();
        a.node_add(b.id, Some(&addr)).await.unwrap();
        b.node_add(a.id, None).await.unwrap();
        a.node_ping(b.id).await.unwrap();
        let path = ToolEnv::current();
        // The two test homes differ, which the table refuses; what matters
        // is that the table was made from the peer's answer.
        let answered = b.doctor(Some(a.id), None, &path).await.unwrap();
        let DoctorReport::Compared { rows, .. } = answered else {
            panic!("{answered:?}");
        };
        assert!(
            rows.iter().any(|row| row.fact == "network"
                && row
                    .peer
                    .as_deref()
                    .is_some_and(|wire| wire.contains("tcp+tls"))),
            "{rows:?}"
        );
    }

    /// Two peers can answer pulls asked from the same head. Once one page
    /// lands, the other's no longer links to this node's head; that page
    /// must lead to the fork rule, not to an error that leaves a
    /// higher-epoch steal on the other chain unseen.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_that_no_longer_links_leads_to_the_fork_rule() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        let (_dir_c, c) = node();
        tokio::spawn(Arc::clone(&a).run());
        tokio::spawn(Arc::clone(&c).run());
        let (at_a, at_c) = (
            a.local_addr().unwrap().to_string(),
            c.local_addr().unwrap().to_string(),
        );
        for (node, peer, addr) in [
            (&a, b.id, None),
            (&a, c.id, Some(&at_c)),
            (&b, a.id, Some(&at_a)),
            (&b, c.id, Some(&at_c)),
            (&c, a.id, Some(&at_a)),
            (&c, b.id, None),
        ] {
            node.node_add(peer, addr.map(String::as_str)).await.unwrap();
        }
        let path = ToolEnv::current();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, out).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        c.pull_records(id, a.id).await.unwrap();
        c.ensure_latest(id, a.id).await.unwrap();
        // The catch-up c's session started must end before the chains split.
        for _ in 0..250 {
            if crate::node::lock(&c.catching_up).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let shared = b.job(id).await.unwrap().lock().await.ledger().head();

        a.job_decide(id, Decision::Approve, "a's own record")
            .await
            .unwrap();
        b.pull_records(id, a.id).await.unwrap();
        c.job_steal(id, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        // c's answer to a pull b asked from the shared head, before a's record landed.
        let token = FencingToken {
            job_id: id,
            epoch: Epoch::FIRST,
        };
        let answer = b
            .call(c.id, |session| async move {
                session.pull(token, Some(shared), SHORT_DEADLINE).await
            })
            .await
            .unwrap();
        let Fenced::Answer(Pulled::Last(page) | Pulled::More(page)) = answer else {
            panic!("{answer:?}");
        };

        assert!(!b.take_page(id, c.id, &page).await.unwrap());
        let pulled = b.pull_records(id, c.id).await;
        assert!(
            matches!(pulled, Err(NodeError::Forked { peer, .. }) if peer == c.id),
            "{pulled:?}"
        );
        let job = b.job(id).await.unwrap();
        assert!(job.lock().await.forked().unwrap().is_some());
    }

    /// A suspend copies the job to a peer paired without an address while
    /// that peer's session is open, so it holds the work if this node goes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_suspend_copies_the_job_to_an_addressless_peer_over_its_session() {
        let (_dir_a, a) = node();
        let (dir_b, b) = node();
        tokio::spawn(Arc::clone(&a).run());
        tokio::spawn(Arc::clone(&b).run());
        let addr = b.local_addr().unwrap().to_string();
        a.node_add(b.id, Some(&addr)).await.unwrap();
        b.node_add(a.id, None).await.unwrap();
        a.node_ping(b.id).await.unwrap();
        let id = job(dir_b.path(), &b).await;
        let path = ToolEnv::current();
        let mut out = Out::default();
        b.suspend(id, &path, &mut out).await.unwrap();
        b.replicate(id, &mut out).await;
        assert!(a.job(id).await.is_ok(), "{:?}", out.events);
    }

    /// A secret in an ACP agent's arguments must be refused before a
    /// ledger record names the harness, since records reach every peer.
    #[tokio::test]
    async fn a_harness_whose_arguments_hold_a_secret_is_never_recorded() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let secret = HarnessKind::Acp {
            program: fabric_core::harness::ProgramName::parse("opencode").unwrap(),
            args: vec!["--api-key".to_owned(), format!("sk-ant-{}", "a".repeat(24))],
        };
        let refused = node
            .job_new(dir.path(), "goal".to_owned(), secret.clone())
            .await;
        assert!(
            matches!(refused, Err(NodeError::HarnessConfig(_))),
            "{refused:?}"
        );
        let mut sleeper = sleeper();
        let refused = node
            .harness_started(id, Epoch::FIRST, pid(&sleeper), secret, None)
            .await;
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
        assert!(
            matches!(refused, Err(NodeError::HarnessConfig(_))),
            "{refused:?}"
        );
        assert_eq!(node.jobs.lock().await.len(), 1);
        let job = node.job(id).await.unwrap();
        assert_eq!(job.lock().await.ledger().records().len(), 1);
    }

    /// A log longer than one control answer can carry still answers, with
    /// the latest records and how many were left out.
    #[tokio::test]
    async fn a_log_past_the_control_cap_answers_with_the_latest_records() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let long_text = "n".repeat(900 << 10);
        for _ in 0..10 {
            node.job_decide(id, Decision::Approve, &long_text)
                .await
                .unwrap();
        }
        let log = node.job_log(id).await.unwrap();
        assert!(log.left_out > 0, "nothing was left out");
        assert_eq!(log.left_out + log.records.len() as u64, 11);
        assert!(matches!(
            log.records.last().map(|record| &record.event),
            Some(LedgerEvent::DecisionRecorded { .. })
        ));
        let answer = fabric_client::api::Answer::Done {
            reply: Reply::JobLog(log),
        };
        assert!(
            fabric_client::frame::encode(&answer).unwrap().len() <= fabric_client::frame::MAX_FRAME
        );
    }

    /// Review finding on PR #13: the capture a peer's pull asks for
    /// measured carry keys in the daemon's environment, while the puller
    /// restores in its CLI's, so with `CARGO_HOME` set in the shell and not
    /// for the daemon a pull rebuilt `target/` where the base carried it.
    /// The holder now captures in the environment its last local client
    /// sent: here a `rustc` and a `CARGO_HOME` only that environment names.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_capture_for_a_peers_pull_measures_in_the_environment_a_client_sent() {
        use std::os::unix::fs::PermissionsExt;

        use fabric_client::api::{PathDir, Roots, SystemPath};
        use fabric_core::path::AbsPath;
        use fabric_core::snapshot::CarryKey;

        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let root = paths::job_root(&a.cfg.home, id);
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target").join("built"), "bytes\n").unwrap();
        let bin = dir_a.path().join("shell-bin");
        std::fs::create_dir(&bin).unwrap();
        let rustc = bin.join("rustc");
        std::fs::write(&rustc, "#!/bin/sh\necho rustc 1.0.0-shell\n").unwrap();
        std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
        a.remember_env(&ClientEnv {
            path: vec![
                PathDir::parse(bin.to_str().unwrap()).unwrap(),
                PathDir::parse("/usr/bin").unwrap(),
                PathDir::parse("/bin").unwrap(),
            ],
            roots: Roots {
                cargo_home: Some(SystemPath::parse("/shell/cargo").unwrap()),
                ..Roots::default()
            },
        });

        b.pull(id, a.id, &mut Out::default()).await.unwrap();
        let snapshot = b.job(id).await.unwrap().lock().await.latest().unwrap();
        let manifest = with_store(&b.store, move |store| Ok(store.manifest(snapshot)?))
            .await
            .unwrap();
        let target = manifest
            .caches
            .iter()
            .find(|cache| cache.path.as_str() == "target")
            .unwrap();
        assert_eq!(
            target.key,
            CarryKey::Rust {
                rustc_vv: "rustc 1.0.0-shell".to_owned(),
                cargo_home: AbsPath::parse("/shell/cargo").unwrap(),
            }
        );
    }

    /// Review finding on PR #13: a node kept the last CLI's `PATH` as its
    /// own and ran tools from it for its peers. With a job's virtualenv
    /// activated in the shell, or direnv's `node_modules/.bin`, a peer's
    /// preflight or pull then had the node run `python3` or `node` from
    /// that workspace, whose files come from the repository and from
    /// peers' snapshots, long after the shell was gone. The node now keeps
    /// no directory inside a job's tree, written there or reached through
    /// a link, and keeps the rest of the shell's environment.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_never_has_the_node_run_a_program_from_a_workspace() {
        use std::os::unix::fs::PermissionsExt;

        use fabric_client::api::{PathDir, Roots, SystemPath};

        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let root = paths::job_root(&a.cfg.home, id);
        let venv = root.join(".venv").join("bin");
        std::fs::create_dir_all(&venv).unwrap();
        let ran = dir_a.path().join("ran");
        for program in ["python3", "node", "rustc", "pnpm"] {
            let path = venv.join(program);
            let script = format!("#!/bin/sh\necho \"$0\" >> '{}'\n", ran.display());
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let linked = dir_a.path().join("linked");
        std::os::unix::fs::symlink(&root, &linked).unwrap();
        let dir = |path: &Path| PathDir::parse(path.to_str().unwrap()).unwrap();
        let system = |path: &Path| SystemPath::parse(path.to_str().unwrap()).unwrap();

        for workspace in [venv.clone(), linked.join(".venv").join("bin")] {
            a.remember_env(&ClientEnv {
                path: vec![
                    dir(&workspace),
                    PathDir::parse("/usr/bin").unwrap(),
                    PathDir::parse("/bin").unwrap(),
                ],
                roots: Roots {
                    cargo_home: Some(SystemPath::parse("/shell/cargo").unwrap()),
                    rustup_home: Some(system(&root.join(".rustup"))),
                    ..Roots::default()
                },
            });
            b.doctor(Some(a.id), None, &ToolEnv::current())
                .await
                .unwrap();
            assert!(
                !ran.exists(),
                "a peer had the node run {}",
                std::fs::read_to_string(&ran).unwrap_or_default()
            );
            let kept = a.node_env();
            assert_eq!(kept.path_var(), "/usr/bin:/bin");
            assert_eq!(
                kept.root(Roots::CARGO_HOME),
                Some(std::ffi::OsStr::new("/shell/cargo"))
            );
            assert_eq!(kept.root(Roots::RUSTUP_HOME), None);
        }
    }

    /// Three running nodes and a job A made and owns at epoch 1, which C,
    /// paired with B alone, stole and pushed to B: B is a replica at epoch
    /// 2 with C as its owner, and A, which knows nothing of the steal, is
    /// still at epoch 1. The directories come first, kept alive.
    async fn stolen_without_a() -> (
        [tempfile::TempDir; 3],
        Arc<Node>,
        Arc<Node>,
        Arc<Node>,
        JobId,
    ) {
        let (dir_a, a) = node();
        let (dir_b, b) = node();
        let (dir_c, c) = node();
        for node in [&a, &b, &c] {
            tokio::spawn(Arc::clone(node).run());
        }
        for (node, peer) in [(&a, &b), (&b, &a), (&b, &c), (&c, &b)] {
            let addr = peer.local_addr().unwrap().to_string();
            node.node_add(peer.id, Some(&addr)).await.unwrap();
        }
        let out = &mut Out::default();
        let env = ToolEnv::current();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &env, out).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        c.pull_records(id, b.id).await.unwrap();
        c.ensure_latest(id, b.id).await.unwrap();
        c.job_steal(id, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        c.push_records(id, b.id).await.unwrap();
        assert_eq!(b.lease(id).await.unwrap().epoch, Epoch::new(2).unwrap());
        assert_eq!(a.lease(id).await.unwrap().epoch, Epoch::FIRST);
        ([dir_a, dir_b, dir_c], a, b, c, id)
    }

    /// Review finding on PR #13: a peer that refused a node's records named
    /// the epoch it knows only in the sentence, as `peer { peer }`. When a
    /// move from A pushes its records to B, B is at epoch 2, and the
    /// refusal says so as data, with the sentence it had.
    #[tokio::test(flavor = "multi_thread")]
    async fn records_pushed_to_a_peer_at_a_newer_epoch_are_refused_with_it() {
        let (_dirs, a, b, _c, id) = stolen_without_a().await;

        let refused = a.push_records(id, b.id).await.unwrap_err().to_refusal();
        assert_eq!(
            refused.reason(),
            &Reason::PeerAhead {
                job: id,
                peer: b.id,
                epoch: Epoch::new(2).unwrap(),
            }
        );
        assert_eq!(refused.kind(), fabric_client::api::RefusalKind::Failed);
        assert!(
            refused.message().starts_with(&format!(
                "{} answered: it knows epoch 2, newer than this node's",
                b.id
            )),
            "{}",
            refused.message()
        );
    }

    /// Review finding on PR #13: the pull form of the move named the
    /// holder's epoch only in the sentence, as `peer { peer }`. B, at
    /// epoch 2, asks A, still at epoch 1, to start a move toward it, as
    /// `fabric job resume --on A` does on B; A answers that B is ahead of
    /// it, and the refusal says A is behind, at epoch 1, as data.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pull_from_a_holder_at_an_older_epoch_is_refused_with_it() {
        let (_dirs, a, b, _c, id) = stolen_without_a().await;

        let refused = b
            .pull(id, a.id, &mut Out::default())
            .await
            .unwrap_err()
            .to_refusal();
        assert_eq!(
            refused.reason(),
            &Reason::PeerBehind {
                job: id,
                peer: a.id,
                epoch: Epoch::FIRST,
            }
        );
        assert_eq!(refused.kind(), fabric_client::api::RefusalKind::Failed);
        assert_eq!(
            refused.message(),
            format!(
                "{} answered: it did not start a move: it is at epoch 1, behind this node's, and must catch up first",
                a.id
            )
        );
        assert_eq!(a.lease(id).await.unwrap().epoch, Epoch::FIRST);
    }

    /// The refusal `node` gives when it asks to steal `id`.
    async fn steal_refusal(node: &Arc<Node>, id: JobId) -> fabric_client::api::Refusal {
        node.job_steal(id, "a sleeps".to_owned(), true, &mut Out::default())
            .await
            .unwrap_err()
            .to_refusal()
    }

    /// Review finding on PR #13: what a ledger rule refuses of a request
    /// came back as `internal`, the reason of a failed file or store, with
    /// the job's facts only in the sentence. A move to the owner itself
    /// names the job, with the rule's sentence as before.
    #[tokio::test]
    async fn a_move_to_the_owner_itself_is_refused_with_the_job() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;

        let refused = node
            .job_move(id, node.id, true, &ToolEnv::current(), &mut Out::default())
            .await
            .unwrap_err()
            .to_refusal();
        assert_eq!(refused.reason(), &Reason::MoveToSelf { job: id });
        assert_eq!(refused.kind(), fabric_client::api::RefusalKind::Failed);
        assert_eq!(refused.message(), "cannot move a job to its owner");
    }

    /// Review finding on PR #13: a steal the ledger refuses came back as
    /// `internal`. Each refusal now names the facts of the job that explain
    /// it, with the rule's sentence as before: the owner asks, B asks
    /// without the latest snapshot whole, the owner asks while a move it
    /// started toward B is open, and B, that move's target, asks.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_steal_the_ledger_refuses_names_the_facts_that_explain_it() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &ToolEnv::current(), &mut Out::default())
            .await
            .unwrap();
        let snapshot = a.job(id).await.unwrap().lock().await.latest().unwrap();

        let refused = steal_refusal(&a, id).await;
        assert_eq!(refused.reason(), &Reason::StealByOwner { job: id });
        assert_eq!(refused.message(), "the owner cannot steal its own job");

        b.pull_records(id, a.id).await.unwrap();
        let refused = steal_refusal(&b, id).await;
        assert_eq!(
            refused.reason(),
            &Reason::StealWithoutSnapshot { job: id, snapshot }
        );
        assert_eq!(
            refused.message(),
            "steal refused: the latest published snapshot is not complete in the local store"
        );

        let move_id = clock::move_id();
        {
            let job = a.job(id).await.unwrap();
            let mut job = job.lock().await;
            let record = job
                .ledger()
                .start_move(a.id, b.id, move_id, clock::event_id(), clock::now_ms())
                .unwrap();
            job.commit(record, Author::Node(a.id)).unwrap();
        }
        let refused = steal_refusal(&a, id).await;
        assert_eq!(
            refused.reason(),
            &Reason::StealWhileDeparting {
                job: id,
                move_id,
                target: b.id,
            }
        );
        assert_eq!(refused.message(), "the owner cannot steal its own job");

        b.pull_records(id, a.id).await.unwrap();
        let refused = steal_refusal(&b, id).await;
        assert_eq!(
            refused.reason(),
            &Reason::StealByTarget { job: id, move_id }
        );
        assert_eq!(
            refused.message(),
            format!("the target of move {move_id} decides it; import or abort instead of stealing")
        );
        assert_eq!(refused.kind(), fabric_client::api::RefusalKind::Failed);
    }

    /// Review finding on PR #13: the reply to a steal read the lease again
    /// after the push to the old owner, which waits without the job's lock,
    /// so a steal or a move in that wait made the reply name a later lease
    /// than the one the steal made. B steals from A while A holds its job's
    /// lock, so B's push waits; meanwhile C steals from B. B's reply is the
    /// lease of its own steal, the one its `stole` event names.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_steal_replies_with_the_lease_it_made() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        let (_dir_c, c) = node();
        for node in [&a, &b, &c] {
            tokio::spawn(Arc::clone(node).run());
        }
        for (node, peer) in [(&a, &b), (&b, &a), (&b, &c), (&c, &b)] {
            let addr = peer.local_addr().unwrap().to_string();
            node.node_add(peer.id, Some(&addr)).await.unwrap();
        }
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &ToolEnv::current(), &mut Out::default())
            .await
            .unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        c.pull_records(id, b.id).await.unwrap();
        c.ensure_latest(id, b.id).await.unwrap();

        let at_a = a.job(id).await.unwrap();
        let held = at_a.lock().await;
        let steal = tokio::spawn({
            let b = Arc::clone(&b);
            async move {
                let request = Request::JobSteal {
                    job: id,
                    reason: "a sleeps".to_owned(),
                    force: true,
                };
                let mut out = Out::default();
                let reply = b.dispatch(request, &mut out).await;
                (reply, out)
            }
        });
        let stolen = Lease {
            owner: b.id,
            epoch: Epoch::new(2).unwrap(),
        };
        for _ in 0..500 {
            if b.lease(id).await.unwrap() == stolen {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(b.lease(id).await.unwrap(), stolen);
        c.pull_records(id, b.id).await.unwrap();
        c.job_steal(id, "b sleeps".to_owned(), true, &mut Out::default())
            .await
            .unwrap();
        assert_eq!(
            b.lease(id).await.unwrap(),
            Lease {
                owner: c.id,
                epoch: Epoch::new(3).unwrap(),
            }
        );
        drop(held);

        let (reply, out) = steal.await.unwrap();
        assert_eq!(reply.unwrap(), Reply::JobSteal(stolen));
        assert!(
            out.events.contains(&Event::Stole {
                job: id,
                epoch: stolen.epoch,
            }),
            "{:?}",
            out.events
        );
    }

    /// Review finding on PR #13: the reply to a move read the lease again
    /// once the move returned, outside the lock that applied the target's
    /// decision. The offer now returns the lease that decision made, B at
    /// epoch 2, as its `imported` event says. The race itself needs an
    /// operation queued on the job's lock while the decision is applied,
    /// which no test here can order.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_decided_move_returns_the_lease_its_decision_made() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let out = &mut Out::default();
        a.suspend(id, &ToolEnv::current(), out).await.unwrap();
        let offer = a.depart(id, b.id, out).await.unwrap().unwrap();
        a.push_records(id, b.id).await.unwrap();

        let moved = Lease {
            owner: b.id,
            epoch: Epoch::new(2).unwrap(),
        };
        assert_eq!(a.offer(id, b.id, offer, out).await.unwrap(), moved);
        let imported = Event::Imported {
            owner: b.id,
            epoch: moved.epoch,
        };
        assert!(out.events.contains(&imported), "{:?}", out.events);
    }

    /// A move typed again after its answer was lost learns the decision and
    /// replies with the lease it made, read with the view that found the
    /// move done.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_typed_again_after_a_lost_answer_replies_with_its_lease() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let out = &mut Out::default();
        a.suspend(id, &ToolEnv::current(), out).await.unwrap();
        let offer = a.depart(id, b.id, out).await.unwrap().unwrap();
        a.push_records(id, b.id).await.unwrap();
        // B imports; its answer never reaches A's ledger.
        let answer = a
            .call(b.id, |session| async move {
                session.offer(Epoch::FIRST, offer, MOVE_DEADLINE).await
            })
            .await
            .unwrap();
        assert!(
            matches!(answer, Fenced::Answer(Offered::Decided { .. })),
            "{answer:?}"
        );

        let request = Request::JobMove {
            job: id,
            to: b.id,
            force: false,
            env: None,
        };
        let moved = Lease {
            owner: b.id,
            epoch: Epoch::new(2).unwrap(),
        };
        assert_eq!(
            a.dispatch(request, out).await.unwrap(),
            Reply::JobMove(moved)
        );
        let done = Event::AlreadyMoved {
            owner: b.id,
            epoch: moved.epoch,
        };
        assert!(out.events.contains(&done), "{:?}", out.events);
    }

    /// Review finding on PR #13: ACP arguments that hold a secret were
    /// refused as `internal`; the refusal names the program.
    #[tokio::test]
    async fn acp_arguments_that_hold_a_secret_are_refused_with_the_program() {
        let (_dir, node) = node();
        let harness = HarnessKind::Acp {
            program: fabric_core::harness::ProgramName::parse("opencode").unwrap(),
            args: vec!["--api-key".to_owned(), format!("sk-ant-{}", "a".repeat(24))],
        };

        let refused = node
            .job_resume(
                clock::job_id(),
                None,
                Some(harness),
                false,
                &ToolEnv::current(),
                &mut Out::default(),
            )
            .await
            .unwrap_err()
            .to_refusal();
        assert_eq!(
            refused.reason(),
            &Reason::SecretInArguments {
                program: "opencode".to_owned()
            }
        );
        assert_eq!(
            refused.message(),
            "the launch arguments of opencode hold a secret; pass it through the environment"
        );
    }

    #[tokio::test]
    async fn a_second_harness_is_refused_while_the_recorded_one_runs() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let (mut first, mut second) = (sleeper(), sleeper());
        register(&node, id, pid(&first)).await.unwrap();
        let refused = register(&node, id, pid(&second)).await;
        assert!(
            matches!(refused, Err(NodeError::AlreadyRunningHarness { pid: p, .. }) if p.get() == first.id()),
            "{refused:?}"
        );
        for child in [&mut first, &mut second] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }

    #[tokio::test]
    async fn a_harness_whose_start_time_cannot_be_read_is_refused() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let mut gone = sleeper();
        let pid = pid(&gone);
        gone.kill().unwrap();
        gone.wait().unwrap();
        let refused = register(&node, id, pid).await;
        assert!(
            matches!(refused, Err(NodeError::HarnessGone { .. })),
            "{refused:?}"
        );
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        assert!(job.harness_record().unwrap().is_none());
        assert!(
            !job.ledger()
                .records()
                .iter()
                .any(|r| matches!(r.event().kind, EventKind::Resumed { .. })),
            "Resumed was appended for a harness nobody supervises"
        );
    }
}
