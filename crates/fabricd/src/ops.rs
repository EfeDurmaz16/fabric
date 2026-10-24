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
    CaptureGroup, CaptureNote, CarryFor, CarryLine, CarryReport, ClientEnv, ClosedJob, Decision,
    DoctorReport, Event, Fact, HarnessSession, JobCreated, JobList, JobLog, JobStatus, JobSummary,
    Lease, LedgerEvent, LogRecord, NodeIdentity, Paired, Peer, PeerList, Pong, Process, Reason,
    Refusal, Registered, Reply, Request, RestoredClasses, RunPlan, SessionRecorded,
    SnapshotSession, SnapshotState, Standing, StealWarning, Step, Suspended, Verdict, VerdictCount,
    View, Wire, Workspace, WorkspaceState,
};
use fabric_core::env::{EnvSpec, OVERRIDE_FILE};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::{FenceError, FencingToken};
use fabric_core::ledger::{Author, EventKind, Ledger, LedgerError};
use fabric_core::moves::{AbortAnswer, CompleteSnapshot, LeaseView, MoveOffer, OfferAnswer};
use fabric_core::session::SessionId;
use fabric_core::snapshot::{LeaveReason, SnapshotManifest};
use fabric_core::version::HarnessVersion;
use fabric_env::materialize::{MaterializeError, TreeLock};
use fabric_harness::HarnessError;
use fabric_harness::codex::Codex;
use fabric_harness::manifest::{MANIFEST_FILE, StateManifest};
use fabric_net::peers::HostPort;
use fabric_net::rpc::Session;
use fabric_net::wire::{
    Appended, Fenced, KnownWriter, MoveRequested, MoveState, Offered, Pulled, RecordBytes,
    RecordPage,
};
use fabric_store::capture::{CaptureRequest, capture};
use fabric_store::policy::IgnorePolicy;
use fabric_store::restore::{
    Durability, RestoreError, RestoreReport, RestoreRequest, RestoreState, Verify, Written,
    restore, restore_state,
};

use crate::carry::{self, PathDecision};
use crate::clock;
use crate::doctor::{self, Context, Target};
use crate::error::NodeError;
use crate::facts::{MeasuredKeys, NodeFacts, Probes, Reuse, Tool, stayed_behind};
use crate::faults::{self, Action, Point};
use crate::harnesses::{HarnessConfig, HarnessEnv, adapter, refuse_secret_kind};
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

/// The bytes of JSON `job log`, `job ls` and `node ls` answer with at
/// most: half the control frame cap, which leaves room for the encoding
/// around them.
const LIST_BUDGET: usize = fabric_client::frame::MAX_FRAME / 2;

/// How many times a move asks again after a lost answer before it reports the move frozen.
const MOVE_TRIES: usize = 5;

/// One answer in the offer loop: to the offer itself, or to a later `MoveStatus`.
#[derive(Debug)]
enum Asked {
    Offer(Fenced<Offered>),
    Status(Fenced<MoveState>),
}

impl Node {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per op, so how every request is served reads in one place"
    )]
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
                planned,
                version,
            } => Reply::HarnessStarted(
                self.harness_started(StartedHarness {
                    job,
                    epoch,
                    pid,
                    harness,
                    session,
                    planned,
                    version,
                })
                .await?,
            ),
            Request::LaunchCheck {
                job,
                epoch,
                head,
                harness,
                session,
            } => {
                self.launch_check(job, epoch, head, &harness, session.as_ref())
                    .await?;
                Reply::LaunchCheck
            }
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
                    report: self.doctor(peer, job, &env, out).await?,
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
        self.node_list_within(LIST_BUDGET)
    }

    /// Lists every paired node in the book's order, keeping the last that
    /// fit in `budget` bytes of JSON: an entry is bounded by the addresses
    /// the book keeps for a peer, but the book does not bound how many
    /// peers it holds, and enough of them would otherwise answer past the
    /// frame cap.
    fn node_list_within(&self, budget: usize) -> PeerList {
        let book = self.peers.snapshot();
        let peers = book
            .iter()
            .map(|entry| Peer {
                node: entry.node(),
                addrs: entry.addrs().iter().map(ToString::to_string).collect(),
                connected: self.open_session(entry.node()).is_some(),
            })
            .collect();
        let (left_out, peers) = latest_within(peers, budget);
        PeerList {
            peers,
            left_out: count(left_out),
        }
    }

    /// Pings `node` over its live session, or a new dial when there is
    /// none or the live one fails: a second dial while a session is live
    /// would replace it and cut the calls other commands make on it.
    async fn node_ping(self: &Arc<Self>, node: NodeId) -> Result<Pong, NodeError> {
        let (took, wire, clock_skew_ms) = self
            .call(node, |session| async move {
                let started = tokio::time::Instant::now();
                session.ping(SHORT_DEADLINE).await?;
                Ok((
                    started.elapsed(),
                    wire(session.kind()),
                    session.clock_skew_ms(),
                ))
            })
            .await?;
        Ok(Pong {
            node,
            rtt_ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
            wire,
            clock_skew_ms,
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
        let synced = self.synced_folders(&self.node_env()).await;
        check_root(&root, &self.cfg.home, synced)?;
        // Built before the clone: a goal past the record bound refuses
        // here, not after a clone that no job and no printed id names.
        let created = Ledger::created(
            id,
            goal,
            harness.clone(),
            self.id,
            clock::event_id(),
            clock::now_ms(),
        )
        .map_err(fabric_core::ledger::LedgerError::from)?;
        let parent = root
            .parent()
            .ok_or_else(|| NodeError::io(&root, std::io::Error::other("a root needs a parent")))?;
        // The workspace holds what the agent writes, ignored secrets
        // included; its job directory is this user's alone.
        crate::durable::create_private_dir(parent).map_err(|e| NodeError::io(parent, e))?;
        // From here on, a failure leaves no job and no printed id that
        // names the directory, so it goes; its id is fresh, so nothing
        // else is in it.
        let discard = |e: NodeError| {
            if let Err(left) = std::fs::remove_dir_all(parent) {
                tracing::warn!(path = %parent.display(), error = %left, "a failed job new left its directory");
            }
            e
        };
        let cloned = tokio::process::Command::new("git")
            .args(["clone", "--quiet", "--"])
            .arg(repo)
            .arg(&root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .await
            .map_err(|e| discard(NodeError::io(repo, e)))?;
        if !cloned.status.success() {
            return Err(discard(NodeError::io(
                repo,
                std::io::Error::other(String::from_utf8_lossy(&cloned.stderr).trim().to_owned()),
            )));
        }
        let job = Job::create_owned(
            id,
            self.cfg.state.job(id),
            &created,
            &Marker {
                root: root.clone(),
                state: MarkerState::Live,
                epoch: Epoch::FIRST,
            },
        )
        .map_err(discard)?;
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
        self.job_list_within(LIST_BUDGET).await
    }

    /// Lists every job, oldest first, then each job whose ledger did not
    /// open, keeping the latest entries that fit in `budget` bytes of JSON:
    /// an entry is bounded, since a goal is listed by its first line cut to
    /// `SUMMARY_CHARS`, and a node that made tens of thousands of jobs
    /// would otherwise answer past the frame cap.
    async fn job_list_within(&self, budget: usize) -> JobList {
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
            listed.push(Listed::Job(JobSummary {
                job: job.id(),
                standing,
                epoch: manifest.lease.epoch(),
                goal: summary(&manifest.goal),
            }));
        }
        listed.extend(
            self.broken_jobs()
                .into_iter()
                .map(|(job, error)| Listed::Closed(ClosedJob { job, error })),
        );
        let (left_out, kept) = latest_within(listed, budget);
        let mut list = JobList {
            jobs: Vec::new(),
            closed: Vec::new(),
            left_out: count(left_out),
        };
        for entry in kept {
            match entry {
                Listed::Job(job) => list.jobs.push(job),
                Listed::Closed(closed) => list.closed.push(closed),
            }
        }
        list
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
        let snapshot = match manifest.latest_snapshot {
            Some(id) => {
                let read = with_store(&self.store, move |store| Ok(store.manifest(id)?)).await;
                Some(SnapshotState {
                    id,
                    suspended: job.suspended_in().is_some(),
                    session: Some(match read {
                        Ok(manifest) if manifest.harness_state.entries.is_empty() => {
                            SnapshotSession::NotCaptured
                        }
                        Ok(_) => SnapshotSession::Captured,
                        Err(e) => SnapshotSession::Unreadable {
                            error: e.to_string(),
                        },
                    }),
                })
            }
            None => None,
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
                Liveness::Unknown => Process::Unconfirmed { pid: record.pid },
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
            snapshot,
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
        let (start, records) = latest_within(records, LIST_BUDGET);
        Ok(JobLog {
            left_out: count(start),
            records,
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
            (&self.harness_config, &self.cfg.home),
            out,
        )
    }

    /// Refuses a launch whose plan no longer holds: the node no longer owns
    /// the job at the plan's epoch, records a harness that still runs,
    /// holds records past the head the plan was made at, or records another
    /// session for `harness` than the `session` the plan continues.
    ///
    /// The CLI asks under the tree lock, before any setup step. A launch of
    /// the same workspace registers its harness, appending `Resumed`, before
    /// it lets the lock go, so a plan this passes was not overtaken by one,
    /// even by one whose harness already exited. A headless turn can name
    /// its session after its harness exited, which moves no head, so the
    /// session is compared as well, here and again at registration.
    pub(crate) async fn launch_check(
        &self,
        id: JobId,
        epoch: Epoch,
        head: RecordHash,
        harness: &HarnessKind,
        session: Option<&SessionId>,
    ) -> Result<(), NodeError> {
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
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid,
            });
        }
        if job.ledger().head() != head {
            return Err(NodeError::PlanOvertaken { job: id });
        }
        refuse_moved_session(&job, harness, session)
    }

    /// Records the harness the CLI launched and appends `Resumed`, or refuses.
    ///
    /// It fails closed: a stale epoch, a harness already recorded and alive,
    /// a session that moved since the plan, and a process whose start time
    /// cannot be read are all refused, and the CLI then kills the group it
    /// launched. The record is written before `Resumed`, so no harness the
    /// ledger says runs is unsupervised.
    /// The session record keeps the newer of its known writer and the
    /// version the launch started, because a Codex that resumes a thread
    /// leaves no version in it, and the capture's own version check runs
    /// from the daemon, where a version manager's shim can pick another
    /// Codex than the one the workspace pins.
    pub(crate) async fn harness_started(
        &self,
        started: StartedHarness,
    ) -> Result<Registered, NodeError> {
        let StartedHarness {
            job: id,
            epoch,
            pid,
            harness,
            session,
            planned,
            version,
        } = started;
        faults::hit(Point::BeforeHarnessRecord).await;
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
        // The setup steps ran after the CLI's check, and an earlier
        // headless turn may have named its session meanwhile.
        refuse_moved_session(&job, &harness, planned.as_ref())?;
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
        // The session is recorded as pending before `Resumed` and moved into
        // place after it, so a crash between the two does not leave a
        // `Resumed` whose session a later capture would miss.
        let record = session
            .map(|session| continued(&job, harness.clone(), session, version))
            .transpose()?;
        job.begin_session(epoch, record.as_ref())?;
        let snapshot = job.latest();
        job.append(
            self.id,
            EventKind::Resumed {
                node: self.id,
                snapshot,
                harness: harness.clone(),
            },
        )?;
        if record.is_some() {
            job.session()?;
        }
        // The threads a fresh Codex plan noted count only once its launch is
        // recorded: a plan the CLI never launched, as when the user said no
        // to its setup steps, must not make a thread a later `codex` typed
        // by hand in the workspace the job's.
        let planned = job.dir().fresh_codex_planned();
        if harness == HarnessKind::Codex
            && record.is_none()
            && let Some(before) = local::read::<Vec<SessionId>>(&planned)?
        {
            local::write(&job.dir().fresh_codex(), &before)?;
        }
        local::remove(&planned)?;
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
    /// removed its record and captured the session the job held then, so a
    /// session recorded later would name one the snapshot does not hold. A
    /// fresh Codex's thread is not lost to that: the capture takes it from
    /// the job's workspace (`job_session`).
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
        let record = continued(&job, harness.clone(), session.clone(), None)?;
        local::write(&job.dir().session(), &record)?;
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
        // The CLI holds the tree's lock from its first setup step until
        // the harness it launched is recorded, and a setup step holds it
        // while it runs, so neither shows as a harness to stop. Taking the
        // lock refuses a capture of a tree an installer is still writing,
        // and holding it to the end keeps a run from starting setup while
        // the capture reads. A link at the root would lock, capture and
        // replicate whatever it names, so it is refused first.
        refuse_replaced_root(id, &marker.root)?;
        let _tree = self.lock_tree(id, &marker.root).await?;
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

    /// Takes the lock `fabric job run` and `resume` hold over the tree at
    /// `root` while they run setup steps and launch; `None` when the root is
    /// gone, since nothing can run setup in it.
    ///
    /// A busy lock is retried for a second before it counts as held (ADR
    /// 0007), so it is taken on a blocking thread.
    async fn lock_tree(&self, job: JobId, root: &Path) -> Result<Option<TreeLock>, NodeError> {
        if !root.exists() {
            return Ok(None);
        }
        let locks = self.cfg.state.tree_locks();
        let tree = root.to_owned();
        let taken = tokio::task::spawn_blocking(move || TreeLock::acquire(&locks, &tree))
            .await
            .map_err(|e| NodeError::Task(e.to_string()))?;
        match taken {
            Ok(lock) => Ok(Some(lock)),
            Err(MaterializeError::TreeBusy { lock }) => Err(NodeError::SetupRunning {
                job,
                root: root.to_owned(),
                lock,
            }),
            Err(e) => Err(e.into()),
        }
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
        let harness = adapter(&record.harness, &self.harness_env(env)?, None)?;
        match harness.confirm_departure(&manifest).await {
            Ok(()) => Ok(()),
            Err(HarnessError::DepartureNotPending { session }) => {
                out.emit(Event::NoDepartureRecord { session });
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Captures the session of the harness the job ran last into its harness
    /// state directory, and returns that directory, or `None` when there is
    /// no session to carry.
    ///
    /// The capture is written beside the last one and replaces it only once
    /// it succeeds, so a capture that fails leaves the state the job's last
    /// snapshot carries where it was.
    async fn capture_harness(
        &self,
        job: &Job,
        root: &Path,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<Option<PathBuf>, NodeError> {
        let state_dir = job.dir().harness_state();
        let next = job.dir().harness_state_next();
        let old = job.dir().harness_state_old();
        finish_state_swap(&state_dir, &next, &old)?;
        let Some(record) = job_session(job, root, &self.cfg.home, job.harness())? else {
            return Ok(None);
        };
        let session = record.session.clone();
        // The capture records at least the newest writer this node knows,
        // so a node that never ran the session cannot lower it.
        let harness = adapter(
            &record.harness,
            &self.harness_env(env)?,
            record.known_writer.clone(),
        )?;
        let carried = state_names(&state_dir, &record.harness, &session);
        std::fs::create_dir_all(&next).map_err(|e| NodeError::io(&next, e))?;
        let captured = harness.capture(root, &session, &next).await;
        if captured.is_err() {
            remove_dir_if_any(&next)?;
        }
        match captured {
            Ok(manifest) => {
                if std::fs::symlink_metadata(&state_dir).is_ok() {
                    std::fs::rename(&state_dir, &old).map_err(|e| NodeError::io(&state_dir, e))?;
                }
                std::fs::rename(&next, &state_dir).map_err(|e| NodeError::io(&next, e))?;
                remove_dir_if_any(&old)?;
                out.emit(Event::SessionCaptured {
                    harness: record.harness.clone(),
                    session: session.clone(),
                    files: count(manifest.files().len()),
                });
                let known_writer = writer_after(record.known_writer.clone(), &manifest);
                local::write(
                    &job.dir().session(),
                    &SessionRecord {
                        known_writer,
                        ..record
                    },
                )?;
                Ok(Some(state_dir))
            }
            // A launch records its session before the harness writes any
            // of it, and a harness stopped at once, as by a signal that
            // ended the run while the node recorded it, never does. Nothing
            // of it exists to carry, and no snapshot carries it, since this
            // node never captured or restored it, so the record goes and
            // the capture goes on. The state this node last captured or
            // restored, of the session the job held before, stays in the
            // snapshot and stays the job's session.
            Err(HarnessError::SessionNotFound { .. }) if !carried => {
                let before = StateManifest::read(&state_dir).ok();
                let kept = if let Some(state) = &before {
                    job.set_session(&SessionRecord {
                        harness: state.kind().clone(),
                        session: state.session().clone(),
                        known_writer: known_writer(state),
                    })?;
                    Some(HarnessSession {
                        harness: state.kind().clone(),
                        session: state.session().clone(),
                    })
                } else {
                    job.forget_session()?;
                    job.clear_harness_state()?;
                    None
                };
                out.emit(Event::SessionNeverStarted {
                    harness: record.harness,
                    session,
                    kept,
                });
                Ok(before.map(|_| state_dir))
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
        let harness_state = self.capture_harness(job, root, env, out).await?;
        // A workspace whose environment does not detect here would not
        // materialize on the target either, so the snapshot is refused
        // before it is published or moved.
        // A pattern the job ever added stays: removing its line from
        // `.fabric/env.toml` does not remove it (ADR 0007, Secrets). The
        // node that recorded it may be another one; the spec of the job's
        // latest snapshot travels with the job and holds what it recorded.
        let parent = job.latest();
        let mut recorded: Vec<fabric_core::secret::SecretPattern> =
            local::read(&job.dir().secret_patterns())?.unwrap_or_default();
        if let Some(parent) = parent {
            let travelled = with_store(&self.store, move |store| {
                let manifest = store.manifest(parent)?;
                Ok(manifest
                    .env_spec
                    .map(|spec| store.env_spec(spec))
                    .transpose()?)
            })
            .await?;
            if let Some(spec) = travelled {
                recorded.extend(spec.secret_patterns);
                recorded.sort();
                recorded.dedup();
            }
        }
        let detection = fabric_env::detect::detect(root, &recorded)?;
        // Only what the keys read is measured: no login, and no tool of an
        // ecosystem the workspace does not use (issue #26).
        let probes = MeasuredKeys::probes(&detection);
        let facts = self
            .facts
            .measure(&self.cfg.home, env, probes, Reuse::Cached)
            .await;
        local::write(
            &job.dir().secret_patterns(),
            &detection.spec().secret_patterns,
        )?;
        let spec = detection.spec().clone();
        let keys = MeasuredKeys::new(facts, Some(&detection));
        let secret_patterns: Vec<String> = detection
            .spec()
            .secret_patterns
            .iter()
            .map(|pattern| pattern.as_str().to_owned())
            .collect();
        // A cache the job declares has a name no built-in rule knows, so
        // the policy is told its path: the snapshot records it with its
        // carry key, and a target whose key differs rebuilds it.
        let declared: Vec<(
            fabric_core::snapshot::CacheClass,
            fabric_core::path::RelPath,
        )> = detection
            .caches()
            .iter()
            .filter(|cache| cache.origin == fabric_env::detect::CacheOrigin::Declared)
            .map(|cache| (cache.class, cache.path.clone()))
            .collect();
        let root = root.to_owned();
        let job_id = job.id();
        let me = self.id;
        let captured = with_store(&self.store, move |store| {
            let mut policy = IgnorePolicy::default();
            policy.load_fabricignore(&root)?;
            // The job's own patterns from `.fabric/env.toml` widen the
            // built-in list; nothing narrows it (ADR 0004).
            for pattern in &secret_patterns {
                policy.add_secret_pattern(pattern)?;
            }
            for (class, path) in &declared {
                policy.add_declared_cache(*class, path);
            }
            // The snapshot names the spec and the store keeps its bytes, so
            // a peer that fetches the snapshot fetches the spec (ADR 0007).
            let env_spec = store.put_env_spec(&spec)?;
            let mut captured = capture(
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
                    env_spec: Some(env_spec),
                    allow_large_untracked: false,
                },
            )?;
            keys.forget_lost_remakes(&mut captured.manifest);
            let snapshot = store.publish(&captured.manifest)?;
            let has_git = captured.manifest.git.is_some();
            Ok((
                snapshot,
                captured.report,
                has_git,
                captured.manifest.left_behind,
            ))
        })
        .await?;
        let (snapshot, report, has_git, left_behind) = captured;
        // `files` counts what is outside git; the bundle carries the rest,
        // tracked and untracked, and its bytes are in `bytes`.
        out.emit(Event::SnapshotPublished {
            snapshot,
            files: report.files,
            bytes: report.bytes,
            new_objects: report.new_objects,
            git_bundle: has_git,
        });
        report_capture(&report, &left_behind, out);
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
                let session = self.session(peer).await?;
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
        let (harness, root, view, floor) = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            let root = job
                .marker()?
                .map_or_else(|| paths::job_root(&self.cfg.home, id), |m| m.root);
            (
                job.harness().clone(),
                root,
                job.view(self.id),
                session_floor(&job)?,
            )
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
        let (here, theirs) = self
            .preflight(
                to,
                (&harness, &root, floor.as_ref()),
                Target::Peer,
                env,
                force,
                out,
            )
            .await?;

        let owner = matches!(view, LeaseView::Owner(_));
        let snapshot = if owner {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            Some(self.suspend_locked(&mut job, env, out).await?)
        } else {
            let job = self.job(id).await?;
            let job = job.lock().await;
            job.latest()
        };
        if let Some(snapshot) = snapshot {
            // Once MoveStarted is written, a refusal could only freeze the
            // job here; a move asked again after it goes on.
            self.carry_for_move(to, snapshot, (&here, &theirs), owner.then_some(force), out)
                .await?;
        }
        self.push_records(id, to).await?;
        out.emit(Event::TargetHoldsCopy { target: to });

        let offer = self.depart(id, to, &harness, out).await?;
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

    /// Reports what `to`, which measured `theirs`, does with each ignored
    /// path of `snapshot`, and whether it has the programs of the steps the
    /// snapshot's spec runs on every resume, as this node, which measured
    /// `here`, sees them. Stops on a path it can neither use nor make again,
    /// and on such a program it lacks, unless `force` says so. With `force`
    /// absent nothing stops.
    async fn carry_for_move(
        &self,
        to: NodeId,
        snapshot: SnapshotId,
        (here, theirs): (&NodeFacts, &NodeFacts),
        force: Option<bool>,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let (manifest, spec) = self.manifest_and_spec(snapshot).await?;
        let decisions = carry::decide_all(&manifest, theirs);
        if !decisions.is_empty() {
            out.emit(carry_decided(
                CarryFor::Move { target: to },
                snapshot,
                &decisions,
            ));
        }
        let rows = setup_rows((&manifest, spec.as_ref()), here, theirs, Target::Peer);
        if !rows.is_empty() {
            out.emit(Event::Preflight {
                peer: to,
                rows: rows.iter().map(doctor::Row::to_wire).collect(),
            });
        }
        if manifest.harness_state.entries.is_empty() {
            out.emit(Event::NoSessionCaptured { snapshot });
        }
        match force {
            Some(force) => {
                refuse_rows(&rows, force)?;
                refuse_carry(&decisions, force, out)
            }
            None => Ok(()),
        }
    }

    /// The manifest of `snapshot` and the spec it names, which the store
    /// holds with it.
    async fn manifest_and_spec(
        &self,
        snapshot: SnapshotId,
    ) -> Result<(fabric_core::snapshot::SnapshotManifest, Option<EnvSpec>), NodeError> {
        with_store(&self.store, move |store| {
            let manifest = store.manifest(snapshot)?;
            let spec = manifest
                .env_spec
                .map(|spec| store.env_spec(spec))
                .transpose()?;
            Ok((manifest, spec))
        })
        .await
    }

    /// Commits the source to the move with `MoveStarted` and returns the offer to send.
    ///
    /// The job's lock was released while the target copied it, so a run
    /// may have started here since: what was copied is then out of date,
    /// and the job stays here. The same holds for a resume under another
    /// harness than `checked`, the one the preflight checked the target
    /// for. A move already open toward `to` is offered again as it is; one
    /// toward another node refuses with that node.
    async fn depart(
        &self,
        id: JobId,
        to: NodeId,
        checked: &HarnessKind,
        out: &mut Out,
    ) -> Result<Option<MoveOffer>, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        // A catch-up may have frozen the job while the target copied it;
        // the target must not import and run a frozen chain.
        if let Some(fork) = job.forked()? {
            return Err(NodeError::Forked {
                job: id,
                peer: fork.peer,
                epoch: fork.epoch,
            });
        }
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
            if job.harness() != checked {
                return Err(NodeError::HarnessChangedDuringMove {
                    job: id,
                    checked: Box::new(checked.clone()),
                    now: Box::new(job.harness().clone()),
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
            && (is_missing(&marker.root)? || marker.is_unchanged().await)
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
            let (kind, same) = {
                let job = self.job(id).await?;
                let job = job.lock().await;
                let kind = harness.clone().unwrap_or_else(|| job.harness().clone());
                let same = &kind == job.harness();
                (kind, same)
            };
            // The known writer is the holder's: a record this node kept
            // from an earlier tenure may be older than the holder's.
            let floor = if same {
                self.owners_floor(id, holder, out).await
            } else {
                None
            };
            let root = paths::job_root(&self.cfg.home, id);
            let (_, theirs) = self
                .preflight(
                    holder,
                    (&kind, root.as_path(), floor.as_ref()),
                    Target::Here,
                    env,
                    force,
                    out,
                )
                .await?;
            // With no harness named, the job's own launches, and the pull
            // must still find the one the preflight checked.
            let checked = harness.is_none().then_some(&kind);
            self.pull(id, holder, (checked, force, &theirs), env, out)
                .await?;
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
        plan(
            &job,
            token.epoch,
            root,
            harness,
            (&self.harness_config, &self.cfg.home),
            out,
        )
    }

    /// The pull form of the move: fetch, ask the owner to start a move toward this node, decide it here.
    ///
    /// `checked` is the harness the preflight checked this node for, when
    /// the job's own harness will launch. The holder may have resumed under
    /// another one after that check and before it suspended for the move;
    /// the move is then aborted, and the holder keeps the job. `theirs` are
    /// the facts the holder gave the preflight.
    async fn pull(
        self: &Arc<Self>,
        id: JobId,
        holder: NodeId,
        (checked, force, theirs): (Option<&HarnessKind>, bool, &NodeFacts),
        env: &ToolEnv,
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
        let move_id = started_move(id, holder, answer)?;
        out.emit(Event::HolderStartedMove { holder, move_id });
        self.pull_records(id, holder).await?;
        self.ensure_latest(id, holder).await?;
        // A path this node can neither use nor make again, or a program
        // of a step that runs on every resume it lacks, stops the pull
        // before it imports, as it stops a push before `MoveStarted`.
        let refused = if force {
            None
        } else {
            self.carry_refusal_here(id, (holder, theirs), env, out)
                .await?
        };
        faults::hit(Point::BeforeImport).await;
        let stopped = {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            // The records hold the holder's `MoveStarted`, after which it
            // runs nothing, so the harness they name is the one to launch.
            // A move this node already decided is left to `import`.
            let undecided = job.ledger().decision_for(move_id).is_none();
            let stop = match checked {
                Some(checked) if undecided && job.harness() != checked => {
                    Some(NodeError::HarnessChangedDuringPull {
                        job: id,
                        holder,
                        checked: Box::new(checked.clone()),
                        now: Box::new(job.harness().clone()),
                    })
                }
                _ if undecided => refused,
                _ => None,
            };
            if stop.is_some() {
                if let AbortAnswer::Abort(record) = job.ledger().answer_abort(
                    self.id,
                    move_id,
                    clock::event_id(),
                    clock::now_ms(),
                )? {
                    job.commit(record, Author::Node(self.id))?;
                }
            } else {
                self.import(&mut job, holder, move_id, out)?;
            }
            stop
        };
        if let Some(stop) = stopped {
            if let Err(e) = self.push_records(id, holder).await {
                out.emit(Event::AbortNotTold {
                    holder,
                    error: e.to_refusal(),
                });
            }
            return Err(stop);
        }
        // The holder learns the decision now, or from MoveStatus later.
        if let Err(e) = self.push_records(id, holder).await {
            out.emit(Event::HolderNotTold {
                holder,
                error: e.to_refusal(),
            });
        }
        Ok(())
    }

    /// Decides here, with this node's facts, each ignored path of the job's
    /// latest snapshot, checks this node has the programs of the steps its
    /// spec runs on every resume, and returns the refusal to stop a pull
    /// with when a path or a program is refused, after reporting them.
    /// `theirs`, the holder's facts, fill the holder's side of the rows.
    async fn carry_refusal_here(
        &self,
        id: JobId,
        (holder, theirs): (NodeId, &NodeFacts),
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<Option<NodeError>, NodeError> {
        let latest = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            job.latest()
        };
        let Some(snapshot) = latest else {
            return Ok(None);
        };
        let (manifest, spec) = self.manifest_and_spec(snapshot).await?;
        let probes = spec.as_ref().map_or(Probes::NONE, doctor::setup_probes);
        let probes = probes.union(carry::probes(&manifest));
        let facts = self
            .facts
            .measure(&self.cfg.home, env, probes, Reuse::Cached)
            .await;
        let rows = setup_rows((&manifest, spec.as_ref()), &facts, theirs, Target::Here);
        if !rows.is_empty() {
            out.emit(Event::Preflight {
                peer: holder,
                rows: rows.iter().map(doctor::Row::to_wire).collect(),
            });
        }
        let decisions = carry::decide_all(&manifest, &facts);
        let refusal = refuse_carry(&decisions, false, &mut Out::default()).err();
        if refusal.is_some() {
            out.emit(carry_decided(CarryFor::Pull, snapshot, &decisions));
        }
        Ok(refuse_rows(&rows, false).err().or(refusal))
    }

    /// Imports the open move `move_id` from `holder` into `job`, whose lock
    /// the caller holds, or says why its ledger does not take it.
    fn import(
        &self,
        job: &mut Job,
        holder: NodeId,
        move_id: MoveId,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let id = job.id();
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
                let session = self.session(peer).await.map_err(|_| first)?;
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
        check_root(&root, &self.cfg.home, self.synced_folders(env).await)?;
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
            job.forget_session()?;
        }
        // The marker says `Restoring` from before the staging directory
        // exists, so a tree here may be another process's, written into
        // the empty root after a kill. Only the snapshot's own tree, which
        // the restore renamed into place before a kill took the marker's
        // last write, is accepted.
        if resuming && state == RestoreState::Present && !self.holds(snapshot, &root).await? {
            return Err(NodeError::WorkspaceOccupied { root });
        }
        // The setup steps this node approved run code from the workspace
        // that no spec id or install digest covers, such as a script a
        // declared step names; another node's snapshot may hold the
        // agent's edits of it, so its steps are approved again (ADR 0007).
        if manifest.created_by != self.id {
            local::remove(&job.dir().env_spec_ran())?;
        }
        job.set_marker(&Marker {
            root: root.clone(),
            state: MarkerState::Restoring { snapshot },
            epoch,
        })?;
        if let Some(parent) = root.parent() {
            crate::durable::create_private_dir(parent).map_err(|e| NodeError::io(parent, e))?;
        }
        let has_harness_state = !manifest.harness_state.entries.is_empty();
        let state_dir = job.dir().harness_state();
        if !(resuming && state == RestoreState::Present) {
            let harness_dir = has_harness_state.then(|| state_dir.clone());
            self.write_snapshot(snapshot, &manifest, &root, harness_dir, env, out)
                .await?;
        }
        // A cache written with holes that setup steps fill keeps no
        // sentinel: it would call the tree whole, and materialize would
        // trust it and never fill them. Without it those steps run first,
        // and the carry decision refused a move here unless this node can
        // run them (issue #24). A tree nothing is known to make again is
        // the user's files, carried whatever this node can run, so it stays
        // as it left, holes and sentinel both: without the sentinel
        // materialize would run a step no decision checked, and a job moved
        // here could not resume. This runs after a restore a kill cut short
        // too, before the marker says the tree is clean.
        for cache in &manifest.caches {
            if cache.carry == fabric_core::snapshot::CarryState::Partial
                && matches!(cache.remake, fabric_core::snapshot::Remake::Setup { .. })
            {
                fabric_env::materialize::clear_sentinel(&root, &cache.path)?;
            }
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

    /// Whether the tree at `root` holds every file of `snapshot` outside
    /// its caches, read back against the store.
    async fn holds(&self, snapshot: SnapshotId, root: &Path) -> Result<bool, NodeError> {
        let root = root.to_owned();
        with_store(
            &self.store,
            move |store| match fabric_store::restore::verify(
                store,
                snapshot,
                &root,
                Verify::All,
                &|_| false,
            ) {
                Ok(_) => Ok(true),
                Err(
                    RestoreError::VerifyFailed { .. }
                    | RestoreError::Git(_)
                    | RestoreError::Write(_),
                ) => Ok(false),
                Err(e) => Err(e.into()),
            },
        )
        .await
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
            // A root deleted or moved away while the marker says live
            // holds nothing to continue, and a directory made there later
            // would be launched in as if it were the workspace. So the
            // latest snapshot is restored, once no harness of the tenure
            // still runs in the old tree.
            Some(Marker {
                root,
                state: MarkerState::Live,
                ..
            }) if is_missing(&root)? => {
                if let Some(record) = job.harness_record()?
                    && record.liveness().await != Liveness::Gone
                {
                    return Err(NodeError::AlreadyRunningHarness {
                        job: job.id(),
                        pid: record.pid,
                    });
                }
                out.emit(Event::LiveWorkspaceGone { root });
                None
            }
            marker => marker,
        })
    }

    /// Writes `snapshot` into the empty `root`, writing each cache the
    /// carry decision says this node writes, and says what it wrote.
    async fn write_snapshot(
        &self,
        snapshot: SnapshotId,
        manifest: &fabric_core::snapshot::SnapshotManifest,
        root: &Path,
        harness_dir: Option<PathBuf>,
        env: &ToolEnv,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let facts = self
            .facts
            .measure(&self.cfg.home, env, carry::probes(manifest), Reuse::Cached)
            .await;
        let decisions = carry::decide_all(manifest, &facts);
        if !decisions.is_empty() {
            out.emit(carry_decided(CarryFor::Restore, snapshot, &decisions));
        }
        let writes: std::collections::BTreeSet<fabric_core::path::RelPath> = decisions
            .into_iter()
            .filter(|d| d.decision.writes())
            .map(|d| d.path)
            .collect();
        let has_git = manifest.git.is_some();
        let target = root.to_owned();
        let report = with_store(&self.store, move |store| {
            let carry = |entry: &fabric_core::snapshot::CacheEntry| writes.contains(&entry.path);
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
        out.emit(restored(snapshot, &report, has_git));
        report_restore(&report, out);
        Ok(())
    }

    /// Reads which folders iCloud Drive syncs on this node, as the
    /// `defaults` of `env` answers; off everywhere it does not exist.
    async fn synced_folders(&self, env: &ToolEnv) -> SyncedFolders {
        let facts = self
            .facts
            .measure(
                &self.cfg.home,
                env,
                Probes::NONE.with(Tool::ICloud),
                Reuse::Cached,
            )
            .await;
        SyncedFolders {
            desktop: facts.syncs(crate::facts::name::ICLOUD_DESKTOP),
            documents: facts.syncs(crate::facts::name::ICLOUD_DOCUMENTS),
        }
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
        job.forget_session()?;
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
        let harness = adapter(state.kind(), &self.harness_env(env)?, None)?;
        harness.restore(root, &state_dir).await?;
        let previous = job
            .session()?
            .filter(|record| record.harness == *state.kind() && record.session == *state.session())
            .and_then(|record| record.known_writer);
        job.set_session(&SessionRecord {
            harness: state.kind().clone(),
            session: state.session().clone(),
            known_writer: writer_after(previous, &state),
        })?;
        out.emit(Event::SessionRestored {
            harness: state.kind().clone(),
            session: state.session().clone(),
        });
        Ok(())
    }

    /// Steals `id` and returns the lease the steal made.
    ///
    /// One hold of the job's lock runs from the fork check to the commit,
    /// and the lease is read under it: a catch-up waiting for the lock
    /// could otherwise freeze the job, or move it to another owner, after
    /// the check and before the steal. The push to the old owner waits
    /// without it, and another steal or a move may change the lease then.
    async fn job_steal(
        self: &Arc<Self>,
        id: JobId,
        reason: String,
        force: bool,
        out: &mut Out,
    ) -> Result<Lease, NodeError> {
        let (owner, stolen) = {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
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
            // The holder the steal takes the job from, who must learn of it.
            let owner = job.ledger().lease().holder();
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
            (owner, stolen)
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
        out: &mut Out,
    ) -> Result<DoctorReport, NodeError> {
        let Some(peer) = peer else {
            let facts = self
                .facts
                .measure(&self.cfg.home, env, Probes::ALL, Reuse::Fresh)
                .await;
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
        let (harness, root, floor, latest) = match job {
            Some(id) => {
                let (harness, root, owner, kept, latest) = {
                    let job = self.job(id).await?;
                    let job = job.lock().await;
                    let root = job
                        .marker()?
                        .map_or_else(|| paths::job_root(&self.cfg.home, id), |m| m.root);
                    let (owner, kept) = match job.view(self.id) {
                        LeaseView::Owner(_) | LeaseView::Departing { .. } => {
                            (None, session_floor(&job)?)
                        }
                        LeaseView::Departed { owner, .. } | LeaseView::Replica { owner, .. } => {
                            (Some(owner), None)
                        }
                    };
                    (job.harness().clone(), root, owner, kept, job.latest())
                };
                let floor = match owner {
                    Some(owner) => self.owners_floor(id, owner, out).await,
                    None => kept,
                };
                (Some(harness), Some(root), floor, latest)
            }
            None => (None, None, None, None),
        };
        // The doctor shows every fact as each tool answers now.
        let (mut rows, here, theirs) = self
            .compare(
                peer,
                harness
                    .as_ref()
                    .zip(root.as_deref())
                    .map(|(harness, root)| (harness, root, floor.as_ref())),
                Target::Peer,
                env,
                (Probes::ALL, Reuse::Fresh),
            )
            .await?;
        let carry = match (job, latest) {
            (Some(_), Some(snapshot)) => {
                let (manifest, spec) = self.manifest_and_spec(snapshot).await?;
                rows.extend(setup_rows(
                    (&manifest, spec.as_ref()),
                    &here,
                    &theirs,
                    Target::Peer,
                ));
                let decisions = carry::decide_all(&manifest, &theirs);
                Some(carry_report(Some(&manifest), snapshot, &decisions))
            }
            (Some(_), None) => Some(CarryReport::default()),
            (None, _) => None,
        };
        Ok(DoctorReport::Compared {
            peer,
            rows: rows.iter().map(doctor::Row::to_wire).collect(),
            carry,
        })
    }

    /// The newest version of the job's harness known to have written its
    /// session, asked of `owner`, which alone holds a current record of
    /// it: a node the job left keeps the record of its own tenure, and a
    /// node that never ran it holds none. An owner that does not answer,
    /// or answers with no version this node reads, leaves it unknown, and
    /// the preflight then judges the target against the source alone.
    async fn owners_floor(
        self: &Arc<Self>,
        id: JobId,
        owner: NodeId,
        out: &mut Out,
    ) -> Option<HarnessVersion> {
        let epoch = match self.job(id).await {
            Ok(job) => job.lock().await.ledger().lease().epoch(),
            Err(_) => Epoch::FIRST,
        };
        let token = FencingToken { job_id: id, epoch };
        let asked = self
            .call(owner, |session| async move {
                session.known_writer(token, SHORT_DEADLINE).await
            })
            .await;
        let text = match asked {
            Ok(Fenced::Answer(KnownWriter { version })) => version?,
            Ok(fenced) => {
                let reason = match fenced {
                    Fenced::StaleEpoch { current, .. } => Reason::PeerAhead {
                        job: id,
                        peer: owner,
                        epoch: current,
                    },
                    Fenced::AheadOfUs { current, .. } => Reason::PeerBehind {
                        job: id,
                        peer: owner,
                        epoch: current,
                    },
                    Fenced::Answer(_) => Reason::Peer { peer: owner },
                };
                out.emit(Event::KnownWriterUnknown {
                    owner,
                    error: Refusal::new(reason, fenced_words(&fenced)),
                });
                return None;
            }
            Err(e) => {
                out.emit(Event::KnownWriterUnknown {
                    owner,
                    error: e.to_refusal(),
                });
                return None;
            }
        };
        match HarnessVersion::parse(text.as_str()) {
            Ok(version) => Some(version),
            Err(e) => {
                out.emit(Event::KnownWriterUnreadable {
                    owner,
                    text: text.as_str().to_owned(),
                    error: e.to_string(),
                });
                None
            }
        }
    }

    /// Compares this node's facts with `peer`'s, reports the rows, stops
    /// on a refusal `force` does not waive, and returns this node's facts
    /// and the peer's.
    /// A named job brings its harness, the root the target restores into,
    /// and the newest version of its harness known to have written its
    /// session; this node then measures only what the rows of that job
    /// read (`doctor::probes`).
    async fn preflight(
        self: &Arc<Self>,
        peer: NodeId,
        job: (&HarnessKind, &Path, Option<&HarnessVersion>),
        target: Target,
        env: &ToolEnv,
        force: bool,
        out: &mut Out,
    ) -> Result<(NodeFacts, NodeFacts), NodeError> {
        let probes = (doctor::probes(job.0, target), Reuse::Cached);
        let (rows, here, theirs) = self.compare(peer, Some(job), target, env, probes).await?;
        out.emit(Event::Preflight {
            peer,
            rows: rows.iter().map(doctor::Row::to_wire).collect(),
        });
        refuse_rows(&rows, force)?;
        Ok((here, theirs))
    }

    /// Measures this node's `probes` while `peer` measures its own,
    /// compares them row by row (ADR 0008), and returns the rows, this
    /// node's facts and the peer's.
    async fn compare(
        self: &Arc<Self>,
        peer: NodeId,
        job: Option<(&HarnessKind, &Path, Option<&HarnessVersion>)>,
        target: Target,
        env: &ToolEnv,
        (probes, reuse): (Probes, Reuse),
    ) -> Result<(Vec<doctor::Row>, NodeFacts, NodeFacts), NodeError> {
        // Both nodes measure at once (issue #26). A session the peer
        // opened is used too: a peer behind a NAT has no address here and
        // is reachable only over the session it dialed.
        let (mut here, theirs) = tokio::join!(
            self.facts.measure(&self.cfg.home, env, probes, reuse),
            self.call(peer, |session| async move {
                let facts = session.facts(SHORT_DEADLINE * 4).await?;
                Ok((facts, session.clock_skew_ms(), session.kind().to_string()))
            }),
        );
        let (theirs, clock_skew_ms, wire) = theirs?;
        // The agent of an ACP job is named per job, so only the target of a
        // pull, this node, can look for it.
        if let (Target::Here, Some((HarnessKind::Acp { program, .. }, _, _))) = (target, job) {
            here.measure_acp(program, &self.harness_env(env)?);
        }
        let theirs = NodeFacts::from_wire(&theirs);
        let rows = doctor::compare(
            &here,
            &theirs,
            &Context {
                harness: job.map(|(h, _, _)| h),
                root: job.map(|(_, r, _)| r),
                target,
                clock_skew_ms: Some(clock_skew_ms),
                wire: Some(wire),
                floor: job.and_then(|(_, _, floor)| floor),
            },
        );
        Ok((rows, here, theirs))
    }
}

/// The move `holder` started toward this node at its request, or why it did not.
fn started_move(
    id: JobId,
    holder: NodeId,
    answer: Fenced<MoveRequested>,
) -> Result<MoveId, NodeError> {
    match answer {
        Fenced::Answer(MoveRequested::Started { move_id }) => Ok(move_id),
        Fenced::Answer(MoveRequested::Rejected { head }) => Err(NodeError::Peer {
            peer: holder,
            reason: format!("it did not start a move; its head is {head}"),
        }),
        fenced @ Fenced::StaleEpoch { current, .. } => Err(NodeError::peer_fact(
            holder,
            format!("it did not start a move: {}", fenced_words(&fenced)),
            Reason::PeerAhead {
                job: id,
                peer: holder,
                epoch: current,
            },
        )),
        fenced @ Fenced::AheadOfUs { current, .. } => Err(NodeError::peer_fact(
            holder,
            format!("it did not start a move: {}", fenced_words(&fenced)),
            Reason::PeerBehind {
                job: id,
                peer: holder,
                epoch: current,
            },
        )),
    }
}

/// The rows for the programs of the steps `spec`, the spec of the
/// snapshot `manifest` describes, runs on every resume
/// (`doctor::setup_rows`); none for a snapshot that names no spec, and
/// none when `.fabric/env.toml` stayed on the capturing node.
///
/// The target plans its steps from its own detection over the restored
/// tree (ADR 0007), not from this spec. The override file's declared
/// steps replace the detected ones, and without the file that detection
/// never sees them, so a program they name is not one the target needs,
/// and refusing a move for it would stop one that resumes. A file the
/// target already holds at that path, which a restore leaves alone, is
/// the target's own, and resume names a program it lacks. The detected
/// steps whose program a node measures each run only when a cache of the
/// workspace is not whole, which the carry decision judges per path
/// (review of PR #27).
fn setup_rows(
    (manifest, spec): (&SnapshotManifest, Option<&EnvSpec>),
    here: &NodeFacts,
    peer: &NodeFacts,
    target: Target,
) -> Vec<doctor::Row> {
    if stayed_behind(&manifest.left_behind, OVERRIDE_FILE) {
        return Vec::new();
    }
    spec.map(|spec| doctor::setup_rows(spec, here, peer, target))
        .unwrap_or_default()
}

/// Stops on a row `force` does not waive.
fn refuse_rows(rows: &[doctor::Row], force: bool) -> Result<(), NodeError> {
    let blocking = doctor::blocking(rows, force);
    if blocking.is_empty() {
        Ok(())
    } else {
        Err(NodeError::Preflight(
            blocking.iter().map(|row| row.fact.to_owned()).collect(),
        ))
    }
}

/// Stops on a path the target can neither use nor make again, unless
/// `force`, which carries it as it is (gate finding F6).
///
/// The first `REPORT_PATHS` refused paths are named and the rest counted,
/// so the answer stays bounded however many caches a snapshot holds.
fn refuse_carry(decisions: &[PathDecision], force: bool, out: &mut Out) -> Result<(), NodeError> {
    let (paths, more) = refused_paths(decisions);
    if paths.is_empty() {
        return Ok(());
    }
    if force {
        out.emit(Event::CarriedByForce { paths, more });
        return Ok(());
    }
    Err(NodeError::CarryRefused { paths, more })
}

/// The first `REPORT_PATHS` paths of `decisions` the target can neither
/// use nor make again, and how many more there are.
fn refused_paths(decisions: &[PathDecision]) -> (Vec<String>, u64) {
    let refused: Vec<&PathDecision> = decisions.iter().filter(|d| d.decision.refuses()).collect();
    let paths = refused
        .iter()
        .take(REPORT_PATHS)
        .map(|d| d.path.to_string())
        .collect();
    (paths, count(refused.len().saturating_sub(REPORT_PATHS)))
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

/// The session record for `session` of `harness`, keeping the known
/// writer of the record it replaces when that names the same session, so
/// a launch or a thread named while a turn runs never forgets it, raised
/// to `started`, the version a launch started on the session.
fn continued(
    job: &Job,
    harness: HarnessKind,
    session: SessionId,
    started: Option<HarnessVersion>,
) -> Result<SessionRecord, NodeError> {
    let kept = job
        .session()?
        .filter(|record| record.harness == harness && record.session == session)
        .and_then(|record| record.known_writer);
    let known_writer = kept.into_iter().chain(started).max();
    Ok(SessionRecord {
        harness,
        session,
        known_writer,
    })
}

/// The newest version of the job's harness known to have written its
/// session, from the session record this node holds, when that record is
/// of the job's own harness. Only the owner's record is current: a node
/// the job left keeps the one from its own tenure (`Node::owners_floor`).
pub(crate) fn session_floor(job: &Job) -> Result<Option<HarnessVersion>, NodeError> {
    Ok(job
        .session()?
        .filter(|record| &record.harness == job.harness())
        .and_then(|record| record.known_writer))
}

/// The newest harness version known to have written the session a
/// capture holds, read from its manifest, for a Codex session only; `None`,
/// logged, when a Codex manifest records none that parses, and the session
/// record then keeps the known writer it held (`writer_after`). A Codex
/// resume writes no version into the rollout, so the job carries this
/// one. A Claude transcript records the version of every Claude that
/// wrote a line, and an ACP agent's state is opaque, so for them the
/// version a capture records is only the one installed where it ran,
/// which no guard reads (ADR 0006).
fn known_writer(manifest: &StateManifest) -> Option<HarnessVersion> {
    if *manifest.kind() != HarnessKind::Codex {
        return None;
    }
    let text = manifest.harness_version()?;
    match HarnessVersion::parse(text) {
        Ok(version) => Some(version),
        Err(e) => {
            tracing::warn!(session = %manifest.session(), error = %e, "the captured harness version is not a version");
            None
        }
    }
}

/// The known writer the session record keeps after a capture or a
/// restore of `manifest`: the version the manifest records, or, when it
/// records none that parses, `previous`, the one the record held for the
/// same session; `None` for any harness but Codex. A version that does not
/// parse is ignored and the job's previous known writer stays the floor:
/// it cannot be ordered, so it cannot raise one, and failing the capture
/// would keep the job from being suspended or moved off the node for the
/// Codex installed there (decided 2026-10-22 by the owner, ADR 0006). A
/// Codex capture records at least the known writer it was given, so after
/// a capture this keeps it or raises it.
fn writer_after(
    previous: Option<HarnessVersion>,
    manifest: &StateManifest,
) -> Option<HarnessVersion> {
    if *manifest.kind() != HarnessKind::Codex {
        return None;
    }
    known_writer(manifest).or(previous)
}

/// Refuses a launch planned to continue `planned` when the session a plan
/// for `harness` would continue now is another: an earlier headless turn
/// named its session after the plan was made.
fn refuse_moved_session(
    job: &Job,
    harness: &HarnessKind,
    planned: Option<&SessionId>,
) -> Result<(), NodeError> {
    let now = job.session()?.filter(|record| record.harness == *harness);
    if now.as_ref().map(|record| &record.session) != planned {
        return Err(NodeError::SessionMoved { job: job.id() });
    }
    Ok(())
}

/// Whether the harness state in `state_dir`, the last this node captured
/// or restored for the job, is `session` of `kind`, so a snapshot carries
/// it. A state that cannot be read counts as that session, so a session
/// is never dropped for a file this node cannot read.
fn state_names(state_dir: &Path, kind: &HarnessKind, session: &SessionId) -> bool {
    if !state_dir.join(MANIFEST_FILE).is_file() {
        return false;
    }
    StateManifest::read(state_dir).map_or(true, |state| {
        state.kind() == kind && state.session() == session
    })
}

/// The job's session for `harness`, the one a capture takes or a plan
/// launches: the one recorded for it, else, when both `harness` and the
/// harness the job ran last are Codex, the thread a fresh Codex launched
/// here made, else the record of the harness it ran before, if any.
///
/// A Codex launched without a thread to resume makes one only once it
/// runs, after its launch was recorded, so no launch can name it. Its
/// rollout records the workspace it started in as its `cwd`, and the
/// workspace is the job's own (`paths::job_root`). The plan of such a
/// launch notes the threads the workspace held then (`JobDir::fresh_codex`),
/// so the launch's thread is the oldest one outside that note: an older
/// thread is the job's from before, as one a Codex run here made before
/// the job switched to Claude, and a newer one was made by something the
/// launch ran, a `codex exec`, a sub-agent or a review. It is recorded
/// here, as a launch that named it would have been. Without it, a suspend
/// would carry the workspace and leave the conversation behind, or carry
/// the state of the harness the job ran before it switched to Codex. With
/// no such plan since the job last arrived, or no thread from it, the
/// record stands. A plan for another harness leaves the record alone:
/// after a fresh Codex the record still names the session of the harness
/// before it, which a resume under that harness continues.
fn job_session(
    job: &Job,
    root: &Path,
    home: &Path,
    harness: &HarnessKind,
) -> Result<Option<SessionRecord>, NodeError> {
    let recorded = job.session()?;
    if *harness != HarnessKind::Codex
        || *job.harness() != HarnessKind::Codex
        || recorded
            .as_ref()
            .is_some_and(|record| record.harness == *harness)
    {
        return Ok(recorded);
    }
    let Some(before) = local::read::<Vec<SessionId>>(&job.dir().fresh_codex())? else {
        return Ok(recorded);
    };
    let Some(thread) = codex_threads(root, home)?
        .into_iter()
        .find(|thread| !before.contains(thread))
    else {
        return Ok(recorded);
    };
    // The thread's rollout records the Codex that created it, which the
    // resume guard reads, and no launch has resumed it yet.
    let record = SessionRecord {
        harness: HarnessKind::Codex,
        session: thread,
        known_writer: None,
    };
    local::write(&job.dir().session(), &record)?;
    local::remove(&job.dir().fresh_codex())?;
    Ok(Some(record))
}

/// The Codex threads whose rollouts under `home` record `root` as their `cwd`, oldest first.
fn codex_threads(root: &Path, home: &Path) -> Result<Vec<SessionId>, NodeError> {
    let env = HarnessEnv::new(home, std::ffi::OsString::new(), HarnessConfig::default())?;
    Ok(Codex::new(env.home, env.path_var).threads_in(root)?)
}

/// What to launch for `job` on its owner.
///
/// The workspace is marked live before the plan leaves the node: from here
/// on a harness may write to it, so a move refuses to depart from the
/// snapshot it was copying and a departed copy is never moved aside as if
/// it held nothing new. The plan carries the node's harness configuration
/// and home, so the CLI launches under what the node captures with.
fn plan(
    job: &Job,
    epoch: Epoch,
    root: PathBuf,
    harness: Option<HarnessKind>,
    (harness_config, home): (&HarnessConfig, &Path),
    out: &mut Out,
) -> Result<RunPlan, NodeError> {
    // The CLI resolves a link at the root and would run setup and the
    // harness in whatever it names, so only a real directory is handed out.
    refuse_replaced_root(job.id(), &root)?;
    job.set_marker(&Marker {
        root: root.clone(),
        state: MarkerState::Live,
        epoch,
    })?;
    let harness = harness.unwrap_or_else(|| job.harness().clone());
    let (session, known_writer) = match job_session(job, &root, home, &harness)? {
        // The CLI says it again once the harness has started, with the
        // settings the launch carried from the session (gate finding F10).
        Some(record) if record.harness == harness => {
            out.emit(Event::ContinuingSession {
                harness: harness.clone(),
                session: record.session.clone(),
            });
            (Some(record.session), record.known_writer)
        }
        Some(record) => {
            out.emit(Event::NewSession {
                harness: harness.clone(),
                previous: Some(HarnessSession {
                    harness: record.harness,
                    session: record.session,
                }),
            });
            (None, None)
        }
        None => {
            out.emit(Event::NewSession {
                harness: harness.clone(),
                previous: None,
            });
            (None, None)
        }
    };
    if harness == HarnessKind::Codex && session.is_none() {
        local::write(
            &job.dir().fresh_codex_planned(),
            &codex_threads(&root, home)?,
        )?;
    }
    Ok(RunPlan {
        job: job.id(),
        epoch,
        head: job.ledger().head(),
        root,
        harness_config: (&harness_config.for_harness(&harness)).into(),
        harness,
        session,
        known_writer,
        goal: job.ledger().manifest().goal.clone(),
        home: home.to_owned(),
    })
}

/// The carry decisions `decisions` as one event: the first
/// `REPORT_PATHS` of them, then the rest counted by verdict, so a report
/// stays bounded however many paths a snapshot names (ADR 0008).
fn carry_decided(context: CarryFor, snapshot: SnapshotId, decisions: &[PathDecision]) -> Event {
    let (paths, more) = carry_lines(decisions);
    Event::CarryDecided {
        context,
        snapshot,
        paths,
        more,
    }
}

/// What a peer would do with each ignored path of `snapshot`, whose
/// manifest is `manifest`, as `doctor` reports it.
fn carry_report(
    manifest: Option<&fabric_core::snapshot::SnapshotManifest>,
    snapshot: SnapshotId,
    decisions: &[PathDecision],
) -> CarryReport {
    let (paths, more) = carry_lines(decisions);
    let (refused, refused_more) = refused_paths(decisions);
    CarryReport {
        snapshot: Some(snapshot),
        paths,
        more,
        refused,
        refused_more,
        session_captured: manifest.is_some_and(|m| !m.harness_state.entries.is_empty()),
    }
}

/// The first `REPORT_PATHS` decisions as lines of the client API, and the
/// rest counted by verdict, in the order each verdict first comes.
fn carry_lines(decisions: &[PathDecision]) -> (Vec<CarryLine>, Vec<VerdictCount>) {
    let paths = decisions
        .iter()
        .take(REPORT_PATHS)
        .map(|decision| CarryLine {
            path: decision.path.to_string(),
            verdict: verdict(&decision.decision),
            why: decision.decision.to_string(),
        })
        .collect();
    let mut more: Vec<VerdictCount> = Vec::new();
    for decision in decisions.get(REPORT_PATHS..).unwrap_or_default() {
        let verdict = verdict(&decision.decision);
        match more.iter_mut().find(|count| count.verdict == verdict) {
            Some(count) => count.paths += 1,
            None => more.push(VerdictCount { verdict, paths: 1 }),
        }
    }
    (paths, more)
}

/// A carry decision's verdict, as the client API names it.
fn verdict(decision: &carry::CarryDecision) -> Verdict {
    match decision {
        carry::CarryDecision::Carry { .. } => Verdict::Carry,
        carry::CarryDecision::Rebuild { .. } => Verdict::Rebuild,
        carry::CarryDecision::Refuse { .. } => Verdict::Refuse,
        carry::CarryDecision::LeaveBehind { .. } => Verdict::Leave,
    }
}

/// The longest goal summary `job ls` lists, in characters.
const SUMMARY_CHARS: usize = 72;

/// The last of `items` that fit in `budget` bytes of JSON together, in
/// order, and how many before them were left out.
fn latest_within<T: serde::Serialize>(mut items: Vec<T>, budget: usize) -> (usize, Vec<T>) {
    let mut start = items.len();
    let mut bytes = 0_usize;
    while let Some(item) = start.checked_sub(1).map(|i| &items[i]) {
        let size = serde_json::to_vec(item).map_or(usize::MAX, |json| json.len());
        if bytes.saturating_add(size) > budget {
            break;
        }
        bytes += size;
        start -= 1;
    }
    let kept = items.split_off(start);
    (start, kept)
}

/// A goal's first line, cut to `SUMMARY_CHARS`, so that a list of goals
/// up to the record bound each still fits in one answer.
fn summary(goal: &str) -> String {
    let first = goal.lines().next().unwrap_or_default();
    match first.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}...", &first[..cut]),
        None if first.len() < goal.len() => format!("{first} ..."),
        None => first.to_owned(),
    }
}

/// `n` as a count of the client API.
fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// One entry of `job ls`: a job whose ledger opened, or one that did not.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum Listed {
    Job(JobSummary),
    Closed(ClosedJob),
}

/// A harness a client launched, for `Node::harness_started`.
pub(crate) struct StartedHarness {
    /// The job.
    pub(crate) job: JobId,
    /// The epoch the plan was made under.
    pub(crate) epoch: Epoch,
    /// The harness process, which leads its own group.
    pub(crate) pid: NonZeroU32,
    /// The harness that runs.
    pub(crate) harness: HarnessKind,
    /// The session it continues or created, when known.
    pub(crate) session: Option<SessionId>,
    /// The session the plan continues; a new one when `None`.
    pub(crate) planned: Option<SessionId>,
    /// The version the launch asked of the harness it started, when it asked.
    pub(crate) version: Option<HarnessVersion>,
}

/// How many paths or keys of one kind a suspend names before it counts the rest.
const REPORT_PATHS: usize = 20;

/// Says what the snapshot carries that a secret pattern matches, what of
/// the workspace and the harness state it leaves out, and what of the
/// repository's config it changed or left out, one group per kind, the
/// first `REPORT_PATHS` of each named and the rest counted. The workspace
/// paths a secret pattern or `.fabricignore` kept here are the snapshot's
/// own `left_behind` list, so the suspend names them as a target will
/// (`crate::carry`).
fn report_capture(
    report: &fabric_store::capture::CaptureReport,
    left_behind: &[fabric_core::snapshot::LeftBehind],
    out: &mut Out,
) {
    let because = |reason: LeaveReason| -> Vec<&fabric_core::path::RelPath> {
        left_behind
            .iter()
            .filter(|left| left.reason == reason)
            .map(|left| &left.path)
            .collect()
    };
    // In a repository the whole nested tree stays; in a plain directory
    // its files travel and only its `.git` stays.
    let nested = if report.bundle.is_some() {
        CaptureNote::NestedRepository
    } else {
        CaptureNote::NestedGit
    };
    let groups: Vec<CaptureGroup> = [
        group(CaptureNote::Secret, &because(LeaveReason::Secret)),
        group(CaptureNote::Excluded, &because(LeaveReason::Excluded)),
        group(CaptureNote::SecretHarness, &report.denied_harness),
        group(CaptureNote::TrackedSecret, &report.tracked_secrets),
        group(CaptureNote::StashedSecret, &report.stashed_secrets),
        group(CaptureNote::Filtered, &report.filtered),
        group(CaptureNote::Special, &report.special_files),
        group(CaptureNote::SpecialHarness, &report.special_harness),
        group(CaptureNote::Unreadable, &report.unreadable),
        group(nested, &report.nested_repositories),
        group(CaptureNote::NestedGitHarness, &report.nested_harness),
        group(CaptureNote::StrippedUserinfo, &report.stripped_userinfo),
        group(CaptureNote::DroppedConfig, &report.dropped_config),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !groups.is_empty() {
        out.emit(Event::CaptureReport { groups });
    }
}

/// The group of `items` under `note`, the first `REPORT_PATHS` named and
/// the rest counted; `None` when there are none.
fn group<T: std::fmt::Display>(note: CaptureNote, items: &[T]) -> Option<CaptureGroup> {
    (!items.is_empty()).then(|| CaptureGroup {
        note,
        items: items
            .iter()
            .take(REPORT_PATHS)
            .map(ToString::to_string)
            .collect(),
        more: count(items.len().saturating_sub(REPORT_PATHS)),
    })
}

/// Refuses a root that exists and is not a directory, such as a link put
/// in place of the directory Fabric wrote; a missing root is left to the caller.
fn refuse_replaced_root(job: JobId, root: &Path) -> Result<(), NodeError> {
    if std::fs::symlink_metadata(root).is_ok_and(|meta| !meta.is_dir()) {
        return Err(NodeError::RootNotADirectory {
            job,
            root: root.to_owned(),
        });
    }
    Ok(())
}

/// Finishes or undoes a swap of the harness state a capture killed midway
/// left: a new state not yet in place goes, and the last state set aside
/// comes back when nothing took its place.
fn finish_state_swap(state_dir: &Path, next: &Path, old: &Path) -> Result<(), NodeError> {
    remove_dir_if_any(next)?;
    if is_missing(state_dir)? && !is_missing(old)? {
        std::fs::rename(old, state_dir).map_err(|e| NodeError::io(old, e))
    } else {
        remove_dir_if_any(old)
    }
}

fn remove_dir_if_any(dir: &Path) -> Result<(), NodeError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(NodeError::io(dir, e)),
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

/// Warns of what a restore wrote that a harness cannot take as it is: the
/// submodules it left as empty directories and the links that point
/// outside the tree they were restored into, the first `REPORT_PATHS` of
/// each named and the rest counted, so the run plan that follows never
/// stands for a clean restore that was not one.
fn report_restore(report: &RestoreReport, out: &mut Out) {
    let named = |paths: &[fabric_core::path::RelPath]| {
        let first = paths
            .iter()
            .take(REPORT_PATHS)
            .map(ToString::to_string)
            .collect();
        (first, count(paths.len().saturating_sub(REPORT_PATHS)))
    };
    if !report.submodules.is_empty() {
        let (paths, more) = named(&report.submodules);
        out.emit(Event::SubmodulesLeftEmpty { paths, more });
    }
    if !report.escaping_symlinks.is_empty() {
        let (paths, more) = named(&report.escaping_symlinks);
        out.emit(Event::EscapingLinksRestored { paths, more });
    }
}

/// What a restore wrote, in total and by class (gate finding F8).
fn restored(snapshot: SnapshotId, report: &RestoreReport, git: bool) -> Event {
    let written = |w: Written| fabric_client::api::Written {
        files: w.files,
        bytes: w.bytes,
    };
    let total = report.total();
    Event::Restored {
        snapshot,
        files: total.files,
        bytes: total.bytes,
        classes: Some(RestoredClasses {
            git,
            tracked: written(report.tracked),
            untracked: written(report.untracked),
            ignored: written(report.ignored),
            harness: written(report.harness),
        }),
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
///
/// Only a root that is not there counts as no workspace: one that cannot
/// be looked at, behind a parent without search permission, is an error.
fn move_aside(root: &Path, dest: &Path) -> Result<bool, NodeError> {
    if is_missing(root)? {
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
        let node = node_at(&root.join("home"), &root.join("state"), ToolEnv::current());
        (dir, node)
    }

    /// A node with its state in `state` and the home `home`, which other
    /// nodes of a test may share as the gate Macs share one home path,
    /// started in `env`.
    fn node_at(home: &Path, state: &Path, env: ToolEnv) -> Arc<Node> {
        std::fs::create_dir_all(home).unwrap();
        Node::start(NodeConfig {
            state: StateDir::resolve(Some(state.as_os_str()), home).unwrap(),
            home: home.to_owned(),
            listen: "127.0.0.1:0".parse().unwrap(),
            dial: DialOrder::TcpOnly,
            env,
        })
        .unwrap()
    }

    /// This process's environment, as the CLI sends its own.
    fn env() -> ToolEnv {
        ToolEnv::current()
    }

    /// A job made from a one-commit repository, owned by `node` at epoch 1.
    pub(crate) async fn job(dir: &Path, node: &Arc<Node>) -> JobId {
        let repo = repo(dir);
        node.job_new(&repo, "goal".to_owned(), HarnessKind::Codex)
            .await
            .unwrap()
            .job
    }

    /// A one-commit repository at `dir/repo`.
    fn repo(dir: &Path) -> PathBuf {
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
        repo
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

    /// Asks `node` whether `plan` still holds, as the CLI does under the tree lock.
    async fn check(node: &Node, plan: &RunPlan) -> Result<(), NodeError> {
        node.launch_check(
            plan.job,
            plan.epoch,
            plan.head,
            &plan.harness,
            plan.session.as_ref(),
        )
        .await
    }

    async fn register(node: &Arc<Node>, id: JobId, pid: NonZeroU32) -> Result<(), NodeError> {
        node.harness_started(StartedHarness {
            job: id,
            epoch: Epoch::FIRST,
            pid,
            harness: HarnessKind::Codex,
            session: None,
            planned: None,
            version: None,
        })
        .await
        .map(drop)
    }

    /// A `PATH` whose `codex` reports a version that takes the thread
    /// writer lock, so a capture never waits on a real `codex` running here.
    fn locking_codex(dir: &Path) -> ToolEnv {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let status = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                r#"printf '#!/bin/sh\necho codex-cli 0.155.1\n' > "$1/codex" && chmod 755 "$1/codex""#,
                "sh",
            ])
            .arg(&bin)
            .status()
            .unwrap();
        assert!(status.success());
        on_path(
            &std::env::join_paths([bin.as_path(), Path::new("/usr/bin"), Path::new("/bin")])
                .unwrap(),
        )
    }

    /// An environment of `path` alone, with no tool root set.
    fn on_path(path: &std::ffi::OsStr) -> ToolEnv {
        ToolEnv::from_vars(|name| (name == "PATH").then(|| path.to_owned()))
    }

    /// Writes the rollout a Codex started in the job's workspace makes, and
    /// returns its thread.
    fn codex_thread_in(node: &Node, id: JobId) -> SessionId {
        codex_rollout_in(node, id, "019f233e-bb33-7342-85fa-f2d33b14368a", "10:00")
    }

    /// Writes the rollout of `thread`, started in the job's workspace at
    /// `time` (`HH:MM`) on 2026-10-22, and returns the thread.
    fn codex_rollout_in(node: &Node, id: JobId, thread: &str, time: &str) -> SessionId {
        let root = std::fs::canonicalize(paths::job_root(&node.cfg.home, id)).unwrap();
        let day = node.cfg.home.join(".codex/sessions/2026/10/22");
        std::fs::create_dir_all(&day).unwrap();
        let name_time = time.replace(':', "-");
        std::fs::write(
            day.join(format!("rollout-2026-10-22T{name_time}-00-{thread}.jsonl")),
            format!(
                "{{\"timestamp\":\"2026-10-22T{time}:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"timestamp\":\"2026-10-22T{time}:00.000Z\",\"cwd\":\"{}\",\"originator\":\"codex_cli_rs\",\"cli_version\":\"0.155.1\"}}}}\n",
                root.display()
            ),
        )
        .unwrap();
        SessionId::parse(thread).unwrap()
    }

    /// Plans a Codex launch with no thread to resume, records it and lets
    /// its harness exit; `made` runs while it runs, as the threads that
    /// launch makes are written then.
    async fn fresh_codex_run(node: &Arc<Node>, id: JobId, made: impl FnOnce()) {
        let out = &mut Out::default();
        let plan = node.job_run(id, out).await.unwrap();
        assert_eq!(plan.session, None, "{:?}", out.said());
        let mut harness = sleeper();
        register(node, id, pid(&harness)).await.unwrap();
        made();
        harness.kill().unwrap();
        harness.wait().unwrap();
        node.harness_exited(id, pid(&harness)).await.unwrap();
    }

    /// The session a capture of the job would take now.
    async fn session_to_capture(node: &Node, id: JobId) -> Option<SessionRecord> {
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        assert_eq!(*job.harness(), HarnessKind::Codex);
        let root = paths::job_root(&node.cfg.home, id);
        job_session(&job, &root, &node.cfg.home, job.harness()).unwrap()
    }

    /// The session the job records and the one its last capture holds.
    async fn recorded_and_captured(node: &Node, id: JobId) -> (Option<SessionRecord>, SessionId) {
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        let state = StateManifest::read(&job.dir().harness_state()).unwrap();
        (job.session().unwrap(), state.session().clone())
    }

    /// Records a harness launched to continue `session`, which exits at once.
    async fn ran_briefly(node: &Arc<Node>, id: JobId, session: &SessionId) {
        let out = &mut Out::default();
        let plan = node.job_run(id, out).await.unwrap();
        let mut harness = sleeper();
        node.harness_started(StartedHarness {
            job: id,
            epoch: Epoch::FIRST,
            pid: pid(&harness),
            harness: HarnessKind::Codex,
            session: Some(session.clone()),
            planned: plan.session,
            version: None,
        })
        .await
        .unwrap();
        harness.kill().unwrap();
        harness.wait().unwrap();
        node.harness_exited(id, pid(&harness)).await.unwrap();
    }

    /// A launch whose harness was stopped before it wrote anything, as by a
    /// signal while the node recorded it, leaves a recorded session with no
    /// state here. The suspend says so and goes on without harness state,
    /// instead of failing every suspend and move of the job from then on.
    #[tokio::test]
    async fn a_suspend_goes_on_when_the_recorded_session_never_started() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        ran_briefly(&node, id, &session).await;

        let out = &mut Out::default();
        node.suspend(id, &path, out).await.unwrap();
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        assert_eq!(job.session().unwrap(), None);
        assert!(!job.dir().harness_state().join(MANIFEST_FILE).exists());
        assert!(
            out.said()
                .iter()
                .any(|line| line.contains(session.as_str())),
            "{:?}",
            out.said()
        );
    }

    /// A launch of a new session that never started, after the job's
    /// last session was captured, must not drop that session: the snapshot
    /// keeps the state the job held before, and the job continues it.
    #[tokio::test]
    async fn a_session_that_never_started_leaves_the_one_before_in_the_snapshot() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let mut thread = None;
        fresh_codex_run(&node, id, || thread = Some(codex_thread_in(&node, id))).await;
        let thread = thread.unwrap();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let never = SessionId::parse("019f2340-0000-7000-8000-0000000000ff").unwrap();
        assert_ne!(never, thread);
        ran_briefly(&node, id, &never).await;

        let out = &mut Out::default();
        node.suspend(id, &path, out).await.unwrap();
        let before = SessionRecord {
            harness: HarnessKind::Codex,
            session: thread.clone(),
            // The capture records the version `locking_codex` reports.
            known_writer: Some(HarnessVersion::parse("0.155.1").unwrap()),
        };
        let (recorded, captured) = recorded_and_captured(&node, id).await;
        assert_eq!(recorded, Some(before), "{:?}", out.said());
        assert_eq!(captured, thread);
    }

    /// A session this node captured, whose files are gone when it is to be
    /// captured again, still fails the suspend: the job's snapshots carry
    /// that conversation, and a capture without it would drop it.
    #[tokio::test]
    async fn a_suspend_fails_when_a_captured_session_is_gone() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let mut thread = None;
        fresh_codex_run(&node, id, || thread = Some(codex_thread_in(&node, id))).await;
        let thread = thread.unwrap();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        ran_briefly(&node, id, &thread).await;
        std::fs::remove_dir_all(node.cfg.home.join(".codex/sessions")).unwrap();

        let err = node
            .suspend(id, &path, &mut Out::default())
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                NodeError::Harness(HarnessError::SessionNotFound { .. })
            ),
            "{err}"
        );
        let job = node.job(id).await.unwrap();
        let record = job.lock().await.session().unwrap();
        assert_eq!(record.map(|record| record.session), Some(thread));
    }

    /// A Codex started with no thread to resume makes its thread only once
    /// it runs, so its launch records no session. The suspend takes the
    /// thread the job's workspace holds, records it and captures it, so the
    /// conversation travels with the workspace.
    #[tokio::test]
    async fn a_suspend_captures_the_thread_a_fresh_codex_started() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        let out = &mut Out::default();
        node.suspend(id, &path, out).await.unwrap();
        node.job_run(id, out).await.unwrap();
        let mut harness = sleeper();
        register(&node, id, pid(&harness)).await.unwrap();
        let thread = codex_thread_in(&node, id);
        harness.kill().unwrap();
        harness.wait().unwrap();
        node.harness_exited(id, pid(&harness)).await.unwrap();

        node.suspend(id, &path, out).await.unwrap();
        let record = SessionRecord {
            harness: HarnessKind::Codex,
            session: thread.clone(),
            // The capture records the version `locking_codex` reports.
            known_writer: Some(HarnessVersion::parse("0.155.1").unwrap()),
        };
        assert_eq!(
            recorded_and_captured(&node, id).await,
            (Some(record), thread)
        );
    }

    /// A fresh headless Codex names its thread only once it runs, and the
    /// CLI sends it to the node then. A suspend that takes the job first
    /// stops the harness and drops its record, so the late session is
    /// refused; the suspend must still carry the thread, which it finds in
    /// the job's workspace as for any fresh Codex (Codex review of PR #10,
    /// 2026-10-22).
    #[tokio::test]
    async fn a_suspend_before_a_late_session_record_still_carries_the_thread() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        let out = &mut Out::default();
        node.suspend(id, &path, out).await.unwrap();
        node.job_run(id, out).await.unwrap();
        let harness = sleeper();
        let harness_pid = pid(&harness);
        register(&node, id, harness_pid).await.unwrap();
        let thread = codex_thread_in(&node, id);

        // The suspend stops the harness, which is reaped as the CLI would.
        let reaper = std::thread::spawn(move || {
            let mut harness = harness;
            harness.wait()
        });
        node.suspend(id, &path, out).await.unwrap();
        reaper.join().unwrap().unwrap();
        let late = node
            .harness_session(
                id,
                Epoch::FIRST,
                harness_pid,
                HarnessKind::Codex,
                thread.clone(),
            )
            .await;
        assert!(
            matches!(late, Err(NodeError::HarnessNotRecorded { .. })),
            "{late:?}"
        );
        let record = SessionRecord {
            harness: HarnessKind::Codex,
            session: thread.clone(),
            // The capture records the version `locking_codex` reports.
            known_writer: Some(HarnessVersion::parse("0.155.1").unwrap()),
        };
        assert_eq!(
            recorded_and_captured(&node, id).await,
            (Some(record), thread),
            "{:?}",
            out.said()
        );
    }

    /// A Codex that resumes a thread writes no version into it, so the
    /// version the launch started with is the session's known writer from
    /// then on: the node keeps the newer of it and the one it knew, and a
    /// launch that reports none or an older one never lowers it.
    #[tokio::test]
    async fn a_started_harness_raises_the_known_writer_and_never_lowers_it() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        let out = &mut Out::default();
        node.suspend(id, &path, out).await.unwrap();
        node.job_run(id, out).await.unwrap();
        let thread = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let v = |text| HarnessVersion::parse(text).unwrap();
        let start = |version: Option<HarnessVersion>| {
            let (node, thread) = (Arc::clone(&node), thread.clone());
            async move {
                // The plan continues whatever session the job records then.
                let planned = {
                    let job = node.job(id).await.unwrap();
                    let job = job.lock().await;
                    job.session().unwrap().map(|record| record.session)
                };
                let mut harness = sleeper();
                let started = StartedHarness {
                    job: id,
                    epoch: Epoch::FIRST,
                    pid: pid(&harness),
                    harness: HarnessKind::Codex,
                    session: Some(thread),
                    planned,
                    version,
                };
                let result = node.harness_started(started).await;
                harness.kill().unwrap();
                harness.wait().unwrap();
                node.harness_exited(id, pid(&harness)).await.unwrap();
                result
            }
        };
        let known = || async {
            let job = node.job(id).await.unwrap();
            let job = job.lock().await;
            job.session()
                .unwrap()
                .and_then(|record| record.known_writer)
        };
        start(Some(v("0.160.0"))).await.unwrap();
        assert_eq!(known().await, Some(v("0.160.0")));
        start(Some(v("0.155.1"))).await.unwrap();
        assert_eq!(
            known().await,
            Some(v("0.160.0")),
            "an older start lowered it"
        );
        start(None).await.unwrap();
        assert_eq!(
            known().await,
            Some(v("0.160.0")),
            "a start with no version dropped it"
        );
        start(Some(v("0.163.0-alpha.10.2"))).await.unwrap();
        assert_eq!(known().await, Some(v("0.163.0-alpha.10.2")));
    }

    /// A capture records the newer of the installed Codex and the known
    /// writer, and the session record keeps what the capture recorded, so
    /// a run on this node after the snapshot is held to the same floor as
    /// a node that restores the snapshot.
    #[tokio::test]
    async fn a_capture_raises_the_known_writer_to_the_version_it_records() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let thread = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap().join("home");
        let sessions = home.join(".codex/sessions/2026/10/21");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("rollout-2026-10-21T10-00-00-{thread}.jsonl")),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"cwd\":\"/w\",\"cli_version\":\"0.155.1\"}}}}\n"
            ),
        )
        .unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let codex = bin.join("codex");
        std::fs::write(&codex, "#!/bin/sh\necho 'codex-cli 0.160.0'\n").unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut path = bin.into_os_string();
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let path = on_path(&path);
        {
            let job = node.job(id).await.unwrap();
            let job = job.lock().await;
            let record = SessionRecord {
                harness: HarnessKind::Codex,
                session: thread.clone(),
                known_writer: Some(HarnessVersion::parse("0.155.1").unwrap()),
            };
            local::write(&job.dir().session(), &record).unwrap();
        }
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        let manifest = StateManifest::read(&job.dir().harness_state()).unwrap();
        assert_eq!(manifest.harness_version(), Some("0.160.0"));
        assert_eq!(
            job.session()
                .unwrap()
                .and_then(|record| record.known_writer),
            Some(HarnessVersion::parse("0.160.0").unwrap())
        );
    }

    /// A Codex snapshot whose recorded version does not parse, as one a
    /// capture wrote before the known writer, which recorded the installed
    /// `--version` token as it was, gives no floor of its own. The restore
    /// neither fails nor drops the floor: the known writer this node held
    /// for the session stays (decided 2026-10-22 by the owner, ADR 0006).
    #[tokio::test]
    async fn a_restore_of_a_version_that_does_not_parse_keeps_the_known_writer() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        let thread = codex_thread_in(&node, id);
        let known = HarnessVersion::parse("0.160.0").unwrap();
        let job = node.job(id).await.unwrap();
        job.lock()
            .await
            .set_session(&SessionRecord {
                harness: HarnessKind::Codex,
                session: thread,
                known_writer: Some(known.clone()),
            })
            .unwrap();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let job = job.lock().await;
        let manifest = job.dir().harness_state().join(MANIFEST_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(value["harness_version"], "0.160.0");
        value["harness_version"] = "0.155".into();
        std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();

        let root = paths::job_root(&node.cfg.home, id);
        node.restore_harness(&job, &root, &path, &mut Out::default())
            .await
            .unwrap();
        assert_eq!(
            job.session()
                .unwrap()
                .and_then(|record| record.known_writer),
            Some(known)
        );
    }

    /// Only a Codex capture gives the job a known writer: for Claude the
    /// version a capture records is only the one installed where it ran,
    /// not one known to have written the session, and nothing guards on
    /// it, so a restore or a capture of a Claude session records none.
    #[test]
    fn only_a_codex_capture_gives_a_known_writer() {
        let manifest = |kind: HarnessKind, version: &str| -> StateManifest {
            serde_json::from_value(serde_json::json!({
                "format": fabric_harness::manifest::MANIFEST_FORMAT,
                "kind": kind,
                "session": "s1",
                "root": "/w",
                "home": "/h",
                "harness_version": version,
                "captured_at_ms": 0,
                "files": [],
                "redactions": [],
            }))
            .unwrap()
        };
        assert_eq!(
            known_writer(&manifest(HarnessKind::Codex, "0.160.0")),
            Some(HarnessVersion::parse("0.160.0").unwrap())
        );
        assert_eq!(
            known_writer(&manifest(HarnessKind::ClaudeCode, "2.1.294")),
            None
        );
    }

    /// A job that ran Codex, then Claude, then a fresh Codex that made no
    /// thread, as one not logged in on this node, still holds its Claude
    /// session: the thread of the first Codex run is older than the launch
    /// and is not taken in its place, so a capture carries the Claude
    /// session and a resume does not continue the stale thread.
    #[tokio::test]
    async fn a_fresh_codex_that_made_no_thread_takes_no_older_one() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        node.suspend(id, &locking_codex(dir.path()), &mut Out::default())
            .await
            .unwrap();
        codex_thread_in(&node, id);
        let claude = SessionRecord {
            harness: HarnessKind::ClaudeCode,
            session: SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap(),
            known_writer: None,
        };
        local::write(&node.cfg.state.job(id).session(), &claude).unwrap();

        fresh_codex_run(&node, id, || {}).await;
        assert_eq!(session_to_capture(&node, id).await, Some(claude));
    }

    /// A fresh Codex plan the CLI never launched, as when the user said no
    /// to its setup steps, starts no thread of the job's: a `codex` typed
    /// by hand in the workspace afterwards must not be taken for it.
    #[tokio::test]
    async fn a_fresh_codex_plan_never_launched_takes_no_thread() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        node.suspend(id, &locking_codex(dir.path()), &mut Out::default())
            .await
            .unwrap();
        let plan = node.job_run(id, &mut Out::default()).await.unwrap();
        assert_eq!(plan.session, None);
        codex_thread_in(&node, id);
        assert_eq!(session_to_capture(&node, id).await, None);
    }

    /// A fresh Codex's thread is the first the workspace gains after its
    /// plan; a thread something it ran made later, as a `codex exec` the
    /// agent ran in the workspace or a sub-agent, is not the job's.
    #[tokio::test]
    async fn a_fresh_codex_takes_its_own_thread_not_a_later_one() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        node.suspend(id, &locking_codex(dir.path()), &mut Out::default())
            .await
            .unwrap();
        let mut own = None;
        fresh_codex_run(&node, id, || {
            own = Some(codex_thread_in(&node, id));
            codex_rollout_in(&node, id, "019f2340-0000-7000-8000-000000000001", "10:05");
        })
        .await;
        let own = SessionRecord {
            harness: HarnessKind::Codex,
            session: own.unwrap(),
            known_writer: None,
        };
        assert_eq!(session_to_capture(&node, id).await, Some(own));
    }

    /// A run planned after a fresh Codex exited, with no suspend in
    /// between, continues the thread that Codex made rather than start
    /// another one beside it.
    #[tokio::test]
    async fn a_run_after_a_fresh_codex_continues_its_thread() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        node.suspend(id, &locking_codex(dir.path()), &mut Out::default())
            .await
            .unwrap();
        let mut thread = None;
        fresh_codex_run(&node, id, || thread = Some(codex_thread_in(&node, id))).await;

        let plan = node.job_run(id, &mut Out::default()).await.unwrap();
        assert_eq!(plan.session, thread);
    }

    /// A job that switched from Claude to Codex still records the Claude
    /// session; the suspend carries the Codex thread, the conversation the
    /// job's harness now holds, not the other harness's state.
    #[tokio::test]
    async fn a_suspend_after_a_switch_to_codex_captures_the_codex_thread() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        local::write(
            &node.cfg.state.job(id).session(),
            &SessionRecord {
                harness: HarnessKind::ClaudeCode,
                session: SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap(),
                known_writer: None,
            },
        )
        .unwrap();
        let mut thread = None;
        fresh_codex_run(&node, id, || thread = Some(codex_thread_in(&node, id))).await;
        let thread = thread.unwrap();

        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let record = SessionRecord {
            harness: HarnessKind::Codex,
            session: thread.clone(),
            // The capture records the version `locking_codex` reports.
            known_writer: Some(HarnessVersion::parse("0.155.1").unwrap()),
        };
        assert_eq!(
            recorded_and_captured(&node, id).await,
            (Some(record), thread)
        );
    }

    /// A job ran Claude, then a fresh Codex, which recorded no session, so
    /// the Claude one is still the job's. A resume under Claude continues
    /// that session; the Codex thread the workspace holds neither replaces
    /// the record nor is continued, since the plan launches Claude.
    #[tokio::test]
    async fn a_resume_under_claude_after_a_fresh_codex_continues_the_claude_session() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let claude = SessionRecord {
            harness: HarnessKind::ClaudeCode,
            session: SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap(),
            known_writer: None,
        };
        local::write(&node.cfg.state.job(id).session(), &claude).unwrap();
        codex_thread_in(&node, id);

        let path = env();
        let plan = node
            .job_resume(
                id,
                None,
                Some(HarnessKind::ClaudeCode),
                false,
                &path,
                &mut Out::default(),
            )
            .await
            .unwrap();
        assert_eq!(plan.session.as_ref(), Some(&claude.session));
        let job = node.job(id).await.unwrap();
        assert_eq!(job.lock().await.session().unwrap(), Some(claude));
    }

    /// A job that ran Codex, then Claude, keeps its Claude session when a
    /// resume switches it back to Codex: the older Codex thread in the
    /// workspace does not take the record's place. The Codex launch may
    /// never be recorded, and the next capture would then carry that old
    /// thread for the job's Claude run.
    #[tokio::test]
    async fn a_resume_under_codex_after_claude_leaves_the_claude_record() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let id = node
            .job_new(&repo, "goal".to_owned(), HarnessKind::ClaudeCode)
            .await
            .unwrap()
            .job;
        let claude = SessionRecord {
            harness: HarnessKind::ClaudeCode,
            session: SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap(),
            known_writer: None,
        };
        local::write(&node.cfg.state.job(id).session(), &claude).unwrap();
        codex_thread_in(&node, id);

        let path = env();
        let plan = node
            .job_resume(
                id,
                None,
                Some(HarnessKind::Codex),
                false,
                &path,
                &mut Out::default(),
            )
            .await
            .unwrap();
        assert_eq!(plan.session, None);
        let job = node.job(id).await.unwrap();
        assert_eq!(job.lock().await.session().unwrap(), Some(claude));
    }

    /// Serves `a` and `b` and pairs them, each with the other's address.
    pub(crate) async fn paired(a: &Arc<Node>, b: &Arc<Node>) {
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
                known_writer: None,
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

        a.pull(id, b.id, (None, false, &NodeFacts::default()), &env(), out)
            .await
            .unwrap();
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
    /// fence that stopped it captured without the session. Naming the same
    /// session again keeps the newest writer the node knows of it, and
    /// naming another one does not hand it that writer.
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
                known_writer: None,
            })
        );
        // Naming the session again keeps the newest writer the node knows
        // of it, which the resume guard and the next capture need.
        let known = HarnessVersion::parse("0.160.0").unwrap();
        {
            let job = node.job(id).await.unwrap();
            let job = job.lock().await;
            let record = SessionRecord {
                harness: HarnessKind::Codex,
                session: thread.clone(),
                known_writer: Some(known.clone()),
            };
            local::write(&job.dir().session(), &record).unwrap();
        }
        name(pid(&harness)).await.unwrap();
        assert_eq!(
            recorded().await.and_then(|record| record.known_writer),
            Some(known)
        );
        // A new thread, as `codex exec resume` answers with when it forks,
        // was written by none of the versions that wrote the old one, so it
        // starts with no known writer; keeping the old one's would refuse
        // the Codex that created the new thread on its next resume.
        let forked = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368b").unwrap();
        node.harness_session(
            id,
            Epoch::FIRST,
            pid(&harness),
            HarnessKind::Codex,
            forked.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            recorded().await,
            Some(SessionRecord {
                harness: HarnessKind::Codex,
                session: forked,
                known_writer: None,
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

    /// The setup steps a node approved for a job run code from the
    /// workspace: a script a declared step names, a file a lifecycle
    /// script runs, a Yarn release. A workspace another node captured may
    /// hold the agent's edits of any of them, which no spec id or install
    /// digest covers, so restoring it here drops the approval and the next
    /// launch that runs a step asks again.
    #[tokio::test]
    async fn a_restore_of_a_peers_snapshot_drops_the_setup_approval() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let path = env();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, out).await.unwrap();
        b.pull(id, a.id, (None, false, &NodeFacts::default()), &env(), out)
            .await
            .unwrap();
        let approved = b.cfg.state.job(id).env_spec_ran();
        std::fs::write(&approved, "spec inputs\n").unwrap();

        let job = b.job(id).await.unwrap();
        let mut job = job.lock().await;
        let epoch = owner_token(&job, b.id).unwrap().epoch;
        b.restore_if_needed(&mut job, epoch, &path, out)
            .await
            .unwrap();
        assert!(!approved.exists(), "{:?}", out.said());
    }

    /// An addressless peer can dial in after start-up recovery listed the
    /// reachable peers and before it ended. That session must still be
    /// caught up once recovery ends: a record the peer wrote while this
    /// node was away, such as a steal, would otherwise stay unknown here
    /// until the next session or job command.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_that_begins_during_recovery_is_caught_up_after_it() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        a.recovered().await;
        b.recovered().await;
        let path = env();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, out).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        b.job_steal(id, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        // The steal reached a; this record, which b does not push, did not.
        b.job_decide(id, Decision::Approve, "").await.unwrap();
        let head = |node: Arc<Node>| async move {
            node.job(id).await.unwrap().lock().await.ledger().head()
        };
        let want = head(Arc::clone(&b)).await;
        assert_ne!(head(Arc::clone(&a)).await, want);

        // a restarts: recovery runs, and b's session begins after
        // recovery listed the peers it would catch up with.
        a.recovered.send_replace(false);
        let kept = b.open_session(a.id).unwrap();
        b.forget(a.id, &kept);
        b.node_ping(a.id).await.unwrap();
        a.end_recovery();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while head(Arc::clone(&a)).await != want {
            assert!(
                tokio::time::Instant::now() < deadline,
                "a never caught up with the session b opened during recovery"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Start-up recovery catches each peer up on a task of its own, so it
    /// can be done with one peer while another holds the pass open. A
    /// session that peer begins after its catch-up must be caught up when
    /// the pass ends, or a record it wrote in between, such as a steal,
    /// stays unknown here while this node's stale harness runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_recovery_caught_up_is_caught_up_again_for_a_later_session() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        let (_dir_c, c) = node();
        paired(&a, &b).await;
        tokio::spawn(Arc::clone(&c).run());
        for (node, peer) in [(&a, &c), (&c, &a)] {
            let addr = peer.local_addr().unwrap().to_string();
            node.node_add(peer.id, Some(&addr)).await.unwrap();
        }
        for node in [&a, &b, &c] {
            node.recovered().await;
        }
        let path = env();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, out).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        c.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        b.job_steal(id, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        let head = |node: Arc<Node>| async move {
            node.job(id).await.unwrap().lock().await.ledger().head()
        };
        let wait_for = |want, what: &'static str| {
            let a = Arc::clone(&a);
            async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                while head(Arc::clone(&a)).await != want {
                    assert!(tokio::time::Instant::now() < deadline, "{what}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        };

        // a restarts. c answers no pull while the test holds its job, so
        // the pass stays open after it caught up with b.
        a.recovered.send_replace(false);
        let c_job = c.job(id).await.unwrap();
        let c_paused = c_job.lock().await;
        b.job_decide(id, Decision::Approve, "before").await.unwrap();
        let recovery = tokio::spawn(Arc::clone(&a).recover());
        wait_for(
            head(Arc::clone(&b)).await,
            "recovery did not catch up with b",
        )
        .await;

        // b writes a record a does not have, then begins a new session.
        b.job_decide(id, Decision::Approve, "after").await.unwrap();
        let want = head(Arc::clone(&b)).await;
        let kept = b.open_session(a.id).unwrap();
        b.forget(a.id, &kept);
        b.node_ping(a.id).await.unwrap();
        drop(c_paused);
        recovery.await.unwrap();
        assert!(a.is_recovered());
        wait_for(
            want,
            "a never caught up with the session b began after its catch-up",
        )
        .await;
    }

    /// A session can begin while a catch-up of the same peer still runs and
    /// has gone past the job the peer then wrote to, as when the peer
    /// changed networks and dialed in again. That catch-up must run once
    /// more when it ends, or a steal the peer wrote in between stays
    /// unknown here until the next session while this node's stale
    /// harness runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_that_begins_during_a_catch_up_of_its_peer_is_caught_up_after_it() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        a.recovered().await;
        b.recovered().await;
        let path = env();
        let out = &mut Out::default();
        let mut ids = [
            job(&dir_a.path().join("one"), &a).await,
            job(&dir_a.path().join("two"), &a).await,
        ];
        // A catch-up pulls the jobs in this order.
        ids.sort();
        let [first, second] = ids;
        for id in ids {
            a.suspend(id, &path, out).await.unwrap();
            b.pull_records(id, a.id).await.unwrap();
        }
        b.ensure_latest(first, a.id).await.unwrap();
        b.job_steal(first, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        let head = |node: Arc<Node>| async move {
            node.job(first).await.unwrap().lock().await.ledger().head()
        };
        let wait_for = |want, what: &'static str| {
            let a = Arc::clone(&a);
            async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                while head(Arc::clone(&a)).await != want {
                    assert!(tokio::time::Instant::now() < deadline, "{what}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !crate::node::lock(&a.catching_up).is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the catch-ups the pairing began did not end"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // a catches up with b, which answers no pull of the second job
        // while the test holds it, so the pass stays open after the first.
        b.job_decide(first, Decision::Approve, "before")
            .await
            .unwrap();
        let b_second = b.job(second).await.unwrap();
        let b_paused = b_second.lock().await;
        a.spawn_catch_up(b.id);
        wait_for(head(Arc::clone(&b)).await, "a did not catch up with b").await;

        // b writes a record a does not have, then begins a new session.
        b.job_decide(first, Decision::Approve, "after")
            .await
            .unwrap();
        let want = head(Arc::clone(&b)).await;
        let kept = b.open_session(a.id).unwrap();
        b.forget(a.id, &kept);
        b.node_ping(a.id).await.unwrap();
        drop(b_paused);
        wait_for(
            want,
            "a never caught up with the session b began during its catch-up",
        )
        .await;
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
        node.depart(id, first, &HarnessKind::Codex, &mut Out::default())
            .await
            .unwrap();
        let refused = node
            .depart(id, second, &HarnessKind::Codex, &mut Out::default())
            .await;
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
        node.depart(
            id,
            NodeId::from_bytes([7; 32]),
            &HarnessKind::Codex,
            &mut Out::default(),
        )
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

    /// A path the target can neither use nor rebuild stops a move that is
    /// not forced, by name; a forced one goes on and says what it carries.
    #[test]
    fn a_refused_path_stops_the_move_unless_forced() {
        let refused = PathDecision {
            path: fabric_core::path::RelPath::parse("node_modules").unwrap(),
            decision: carry::CarryDecision::Refuse {
                reason: "pnpm is not installed on the target".to_owned(),
            },
        };
        let mut out = Out::default();
        let err = refuse_carry(std::slice::from_ref(&refused), false, &mut out).unwrap_err();
        assert!(
            matches!(&err, NodeError::CarryRefused { paths, more: 0 } if paths == &["node_modules"]),
            "{err}"
        );
        refuse_carry(&[refused], true, &mut out).unwrap();
        assert_eq!(
            out.said(),
            ["--force: node_modules travels as it is, and the target writes it"]
        );
    }

    /// A monorepo may hold any number of refused trees, and the refusal
    /// must stay within one control message, so it names the first of
    /// them and counts the rest.
    #[test]
    fn a_refusal_names_a_bounded_number_of_paths() {
        let refused: Vec<PathDecision> = (0..25)
            .map(|n| PathDecision {
                path: fabric_core::path::RelPath::parse(&format!("p{n}/node_modules")).unwrap(),
                decision: carry::CarryDecision::Refuse {
                    reason: "pnpm is not installed on the target".to_owned(),
                },
            })
            .collect();
        let err = refuse_carry(&refused, false, &mut Out::default()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("p19/node_modules and 5 more;"), "{text}");
        assert!(!text.contains("p20/"), "{text}");
    }

    #[test]
    fn the_restore_summary_counts_every_class() {
        let report = RestoreReport {
            tracked: Written {
                files: 5,
                bytes: 114,
            },
            untracked: Written {
                files: 1,
                bytes: 10,
            },
            ignored: Written {
                files: 3,
                bytes: 85,
            },
            harness: Written {
                files: 2,
                bytes: 771,
            },
            ..RestoreReport::default()
        };
        let snapshot = SnapshotId::from_bytes([7; 32]);
        let summary = |report: &RestoreReport, git: bool| {
            fabric_client::sentences::event(&restored(snapshot, report, git)).join("\n")
        };
        assert_eq!(
            summary(&report, true),
            format!(
                "restored snapshot {snapshot}: 11 files, 980 bytes (tracked: 5 files, 114 bytes; untracked: 1 file, 10 bytes; ignored: 3 files, 85 bytes; harness state: 2 files, 771 bytes)"
            )
        );
        assert_eq!(
            summary(&RestoreReport::default(), false),
            format!(
                "restored snapshot {snapshot}: 0 files, 0 bytes (workspace: 0 files, 0 bytes; harness state: 0 files, 0 bytes)"
            )
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

    /// A pull of an ACP job must find the agent on the pulling command's
    /// `PATH` and a `state_dir` for it in this node's `harness.toml` before
    /// it takes the job. Otherwise the job would be owned by a node that
    /// cannot launch it, or cannot restore and capture its session, and
    /// `--force` does not waive either.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pull_refuses_an_acp_agent_this_node_cannot_run() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let path = env();
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        // The two test nodes have different homes, which refuses too.
        for (agent, agent_found) in [
            ("acp:fabric-no-such-agent", false),
            // `sh` is on every PATH, and no harness.toml here names it.
            ("acp:sh", true),
        ] {
            let harness = crate::harnesses::parse_harness(agent).unwrap();
            let refused = b
                .job_resume(
                    id,
                    Some(a.id),
                    Some(harness),
                    true,
                    &path,
                    &mut Out::default(),
                )
                .await;
            let Err(NodeError::Preflight(facts)) = &refused else {
                panic!("{agent}: {refused:?}");
            };
            let facts: Vec<&str> = facts.iter().map(String::as_str).collect();
            assert!(facts.contains(&"acp.state_dir"), "{agent}: {facts:?}");
            assert_eq!(
                facts.contains(&"acp.agent"),
                !agent_found,
                "{agent}: {facts:?}"
            );
        }
        let job = a.job(id).await.unwrap();
        assert!(matches!(
            job.lock().await.view(a.id),
            LeaseView::Owner(token) if token.epoch == Epoch::FIRST
        ));
    }

    /// A workspace deleted while its marker says live holds nothing to
    /// continue. The next resume must restore the latest snapshot, not
    /// hand out a root that does not exist, where a directory made later
    /// would be launched in as the workspace.
    #[tokio::test]
    async fn a_live_workspace_whose_root_is_gone_is_restored() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let plan = node.job_run(id, &mut Out::default()).await.unwrap();
        std::fs::remove_dir_all(&plan.root).unwrap();
        let out = &mut Out::default();
        let resumed = node
            .job_resume(id, None, None, false, &path, out)
            .await
            .unwrap();
        assert_eq!(resumed.root, plan.root);
        assert!(
            resumed.root.join(".git").is_dir(),
            "the root was not restored: {:?}",
            out.said()
        );
    }

    /// A `state_dir` that harness.toml names but capture and restore refuse,
    /// under `~/.fabric` or reached through a link, is no `state_dir`: a
    /// pull must refuse it before ownership moves, or the job would be
    /// owned by a node that cannot restore or capture its session.
    #[tokio::test]
    async fn a_pull_refuses_an_acp_state_dir_capture_would_refuse() {
        let (dir_a, a) = node();
        let dir_b = tempfile::Builder::new()
            .prefix("fops")
            .tempdir_in("/tmp")
            .unwrap();
        let root = std::fs::canonicalize(dir_b.path()).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(&root, home.join("linked")).unwrap();
        let state = StateDir::resolve(Some(root.join("state").as_os_str()), &home).unwrap();
        std::fs::create_dir_all(state.harness_config().parent().unwrap()).unwrap();
        std::fs::write(
            state.harness_config(),
            "[acp.sh]\nstate_dir = \".fabric/sh\"\n\n[acp.cat]\nstate_dir = \"linked/sessions\"\n\n[acp.true]\nstate_dir = \"sessions\"\n",
        )
        .unwrap();
        let b = Node::start(NodeConfig {
            state,
            home,
            listen: "127.0.0.1:0".parse().unwrap(),
            dial: DialOrder::TcpOnly,
            env: ToolEnv::current(),
        })
        .unwrap();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let path = env();
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        // The two test nodes have different homes, which refuses every pull.
        for (agent, usable) in [("acp:sh", false), ("acp:cat", false), ("acp:true", true)] {
            let harness = crate::harnesses::parse_harness(agent).unwrap();
            let refused = b
                .job_resume(
                    id,
                    Some(a.id),
                    Some(harness),
                    true,
                    &path,
                    &mut Out::default(),
                )
                .await;
            let Err(NodeError::Preflight(facts)) = &refused else {
                panic!("{agent}: {refused:?}");
            };
            let facts: Vec<&str> = facts.iter().map(String::as_str).collect();
            assert_eq!(
                facts.contains(&"acp.state_dir"),
                !usable,
                "{agent}: {facts:?}"
            );
        }
    }

    /// A registration appends `Resumed` and then writes the session the
    /// harness started. A daemon that dies between the two, here a session
    /// file that cannot be written, must not lose that session: the next
    /// capture would leave the new conversation out of the snapshot.
    #[tokio::test]
    async fn a_session_whose_write_failed_after_resumed_is_kept() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let session_file = node.cfg.state.job(id).session();
        // A directory at the name the session is written under before its
        // rename makes the write fail, while the registration's check of
        // the session it continues still reads none.
        let mut temp_name = session_file.file_name().unwrap().to_os_string();
        temp_name.push(format!(".{}.tmp", std::process::id()));
        let in_the_way = session_file.with_file_name(temp_name);
        std::fs::create_dir_all(in_the_way.join("in-the-way")).unwrap();
        let mut harness = sleeper();
        let session = SessionId::parse("new-session").unwrap();
        let failed = node
            .harness_started(StartedHarness {
                job: id,
                epoch: Epoch::FIRST,
                pid: pid(&harness),
                harness: HarnessKind::Codex,
                session: Some(session.clone()),
                planned: None,
                version: None,
            })
            .await;
        harness.kill().unwrap();
        harness.wait().unwrap();
        assert!(failed.is_err(), "{failed:?}");
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        assert!(matches!(
            job.ledger().records().last().unwrap().event().kind,
            EventKind::Resumed { .. }
        ));
        // As after a restart: the session file was never written.
        assert!(!session_file.exists());
        std::fs::remove_dir_all(&in_the_way).unwrap();
        let kept = job.session().unwrap().map(|record| record.session);
        assert_eq!(kept, Some(session));
    }

    /// A cache `.fabric/env.toml` declares, with a name no built-in rule
    /// knows, must reach the snapshot as a cache with its carry key. As
    /// the user's own files it would be written on every target, platform
    /// or not, sentinel included, and the setup step that rebuilds it
    /// would never run there.
    #[tokio::test]
    async fn a_suspend_records_a_declared_cache_as_a_cache() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "[[cache]]\npath = \".output\"\nclass = \"build\"\n",
        )
        .unwrap();
        std::fs::write(root.join(".gitignore"), ".output/\n").unwrap();
        std::fs::create_dir_all(root.join(".output/server")).unwrap();
        std::fs::write(root.join(".output/server/index.mjs"), "export {}\n").unwrap();
        let path = env();
        let snapshot = node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let manifest = with_store(&node.store, move |store| Ok(store.manifest(snapshot)?))
            .await
            .unwrap();
        let caches: Vec<&str> = manifest.caches.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(caches, [".output"], "{manifest:?}");
    }

    /// A lockfile `.fabricignore` keeps on this node is missing on the
    /// target, whose detection then plans no install for `node_modules`,
    /// so a row that still promised a rebuild would lose the tree there.
    /// The snapshot records that nothing makes it again, and the carry
    /// decision carries it (review of PR #11).
    #[tokio::test]
    async fn a_suspend_whose_lockfile_stays_behind_records_no_rebuild() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::write(root.join("package.json"), "{\"name\": \"x\"}\n").unwrap();
        std::fs::write(root.join("package-lock.json"), "{}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(root.join(".fabricignore"), "package-lock.json\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "1;\n").unwrap();
        let path = env();
        let snapshot = node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let manifest = with_store(&node.store, move |store| Ok(store.manifest(snapshot)?))
            .await
            .unwrap();
        assert!(
            manifest
                .left_behind
                .iter()
                .any(|left| left.path.as_str() == "package-lock.json"),
            "{:?}",
            manifest.left_behind
        );
        let tree = manifest
            .caches
            .iter()
            .find(|cache| cache.path.as_str() == "node_modules")
            .unwrap();
        assert_eq!(tree.remake, fabric_core::snapshot::Remake::Nothing);
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
        let left = CaptureGroup {
            note: CaptureNote::Secret,
            items: vec!["users.sqlite".to_owned()],
            more: 0,
        };
        assert!(
            out.events.iter().any(
                |event| matches!(event, Event::CaptureReport { groups } if groups.contains(&left))
            ),
            "{:?}",
            out.events
        );
    }

    /// `fabric job run` holds the tree's lock from its setup steps until
    /// the harness it launched is recorded, so no harness shows yet. A
    /// suspend in that window must refuse, not publish a tree an installer
    /// is still writing.
    #[tokio::test]
    async fn a_suspend_refuses_while_setup_holds_the_tree() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        let setup = TreeLock::acquire(&node.cfg.state.tree_locks(), &root).unwrap();
        let path = env();
        let refused = node.suspend(id, &path, &mut Out::default()).await;
        assert!(
            matches!(refused, Err(NodeError::SetupRunning { .. })),
            "{refused:?}"
        );
        // A process a setup step left running can hold the lock for good,
        // so the refusal names the file `lsof` finds it by.
        let lock = std::fs::read_dir(node.cfg.state.tree_locks())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let said = refused.unwrap_err().to_string();
        assert!(said.contains(&lock.display().to_string()), "{said}");
        assert_eq!(node.job(id).await.unwrap().lock().await.latest(), None);
        drop(setup);
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
    }

    /// A busy tree lock is retried for a second before the suspend refuses
    /// (ADR 0007); the node's other work must go on meanwhile, here a timer
    /// on the same single-threaded runtime.
    #[tokio::test]
    async fn a_suspend_waiting_on_a_held_tree_does_not_stall_the_node() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        let _setup = TreeLock::acquire(&node.cfg.state.tree_locks(), &root).unwrap();
        let path = env();
        let mut out = Out::default();
        let started = std::time::Instant::now();
        let (refused, ticked) = tokio::join!(node.suspend(id, &path, &mut out), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            started.elapsed()
        });
        assert!(
            matches!(refused, Err(NodeError::SetupRunning { .. })),
            "{refused:?}"
        );
        assert!(
            ticked < Duration::from_millis(500),
            "a 20 ms timer fired after {ticked:?} while the suspend waited on the tree"
        );
    }

    /// A suspend holds the tree lock from before it stops the harness
    /// until the snapshot is recorded, so a `fabric job run` that starts
    /// meanwhile refuses instead of running setup steps under the capture.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_suspend_holds_the_tree_lock_while_it_stops_the_harness() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        node.job_run(id, &mut Out::default()).await.unwrap();
        // Once asked to stop, the harness says so and takes two seconds to exit.
        let asked = dir.path().join("asked");
        let harness = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#"trap 'touch "$0"; sleep 2; exit 0' TERM; while :; do sleep 0.05; done"#)
            .arg(&asked)
            .process_group(0)
            .spawn()
            .unwrap();
        register(&node, id, pid(&harness)).await.unwrap();
        let reaper = std::thread::spawn(move || {
            let mut harness = harness;
            harness.wait()
        });
        let suspending = tokio::spawn({
            let node = Arc::clone(&node);
            async move { node.suspend(id, &path, &mut Out::default()).await }
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !asked.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the suspend never asked the harness to stop"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let root = paths::job_root(&node.cfg.home, id);
        let setup = TreeLock::acquire(&node.cfg.state.tree_locks(), &root);
        assert!(
            matches!(setup, Err(MaterializeError::TreeBusy { .. })),
            "a run took the tree while the suspend stopped its harness: {setup:?}"
        );
        suspending.await.unwrap().unwrap();
        reaper.join().unwrap().unwrap();
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

    /// A restore leaves a submodule as an empty directory and writes a
    /// link whose target leaves the workspace as it was captured. Both
    /// must be named before the run plan returns, the first
    /// `REPORT_PATHS` of each and the rest counted, or the harness starts
    /// in a workspace that reads as cleanly restored with source missing
    /// or a way out of it (Codex review of #8, issue #23).
    #[tokio::test]
    async fn a_restore_names_the_submodules_it_left_empty_and_the_links_that_escape() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let head = git_out(&repo, &["rev-parse", "HEAD"]);
        git_out(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},sub"),
            ],
        );
        git_out(
            &repo,
            &[
                "-c",
                "user.name=f",
                "-c",
                "user.email=f@x",
                "commit",
                "-qm",
                "2",
            ],
        );
        let id = node
            .job_new(&repo, "goal".to_owned(), HarnessKind::Codex)
            .await
            .unwrap()
            .job;
        let root = paths::job_root(&node.cfg.home, id);
        let links = REPORT_PATHS + 2;
        for n in 0..links {
            std::os::unix::fs::symlink("../outside", root.join(format!("out{n:02}"))).unwrap();
        }
        let path = ToolEnv::current();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        let mut out = Out::default();
        node.job_resume(id, None, None, false, &path, &mut out)
            .await
            .unwrap();
        assert!(root.join("sub").is_dir());
        let said = out.said();
        let restored = said
            .iter()
            .position(|line| line.starts_with("restored snapshot"))
            .unwrap_or_else(|| panic!("no restore: {said:#?}"));
        let warned = |start: &str| {
            said.iter()
                .skip(restored)
                .find(|line| line.starts_with(start))
                .unwrap_or_else(|| panic!("no line {start:?} after the restore: {said:#?}"))
        };
        let submodules = warned("warning: submodules left empty");
        assert!(
            submodules.contains(": sub; run `git submodule update --init`"),
            "{submodules}"
        );
        let escaping = warned("warning: links restored that point outside");
        assert!(escaping.contains("out00, out01,"), "{escaping}");
        assert!(escaping.contains("out19 and 2 more;"), "{escaping}");
        assert!(!escaping.contains("out20"), "{escaping}");
    }

    /// Runs git in `repo` and returns what it printed, trimmed.
    fn git_out(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
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
        assert_eq!(sent.home, node.cfg.home);
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
        let answered = b
            .doctor(Some(a.id), None, &path, &mut Out::default())
            .await
            .unwrap();
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
            .harness_started(StartedHarness {
                job: id,
                epoch: Epoch::FIRST,
                pid: pid(&sleeper),
                harness: secret,
                session: None,
                planned: None,
                version: None,
            })
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

        b.pull(
            id,
            a.id,
            (None, false, &NodeFacts::default()),
            &ToolEnv::current(),
            &mut Out::default(),
        )
        .await
        .unwrap();
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
            b.doctor(Some(a.id), None, &ToolEnv::current(), &mut Out::default())
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
            .pull(
                id,
                a.id,
                (None, false, &NodeFacts::default()),
                &ToolEnv::current(),
                &mut Out::default(),
            )
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
        let offer = a
            .depart(id, b.id, &HarnessKind::Codex, out)
            .await
            .unwrap()
            .unwrap();
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
        let offer = a
            .depart(id, b.id, &HarnessKind::Codex, out)
            .await
            .unwrap()
            .unwrap();
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

    /// A secret pattern removed from `.fabric/env.toml` still keeps its
    /// matches on the node at the next capture.
    #[tokio::test]
    async fn a_removed_secret_pattern_still_applies() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "secret_patterns = [\"*.sqlite\"]\n",
        )
        .unwrap();
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        std::fs::write(root.join(".fabric/env.toml"), "").unwrap();
        std::fs::write(root.join("users.sqlite"), "rows\n").unwrap();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        assert!(
            out.said()
                .iter()
                .any(|line| line == "left behind (secret pattern): users.sqlite"),
            "{:?}",
            out.said()
        );
    }

    /// A secret pattern the job's spec recorded travels with the job: the
    /// node that pulls it keeps leaving behind what the pattern matches
    /// after the line left `.fabric/env.toml`, as the node that recorded
    /// it does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_recorded_secret_pattern_travels_with_the_job() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let path = env();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        let root = paths::job_root(&a.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "secret_patterns = [\"*.sqlite\"]\n",
        )
        .unwrap();
        a.suspend(id, &path, out).await.unwrap();
        std::fs::write(root.join(".fabric/env.toml"), "").unwrap();
        a.suspend(id, &path, out).await.unwrap();

        b.pull(id, a.id, (None, false, &NodeFacts::default()), &env(), out)
            .await
            .unwrap();
        let root = {
            let job = b.job(id).await.unwrap();
            let mut job = job.lock().await;
            let epoch = owner_token(&job, b.id).unwrap().epoch;
            b.restore_if_needed(&mut job, epoch, &path, out)
                .await
                .unwrap()
        };
        std::fs::write(root.join("users.sqlite"), "rows\n").unwrap();
        let mut out = Out::default();
        b.suspend(id, &path, &mut out).await.unwrap();
        assert!(
            out.said()
                .iter()
                .any(|line| line == "left behind (secret pattern): users.sqlite"),
            "{:?}",
            out.said()
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

    /// A goal past the ledger's record bound still fits in a control
    /// message, so it reaches the node. The node must refuse it before it
    /// clones: once the record fails, no job and no id point at the clone.
    #[tokio::test]
    async fn a_goal_too_large_for_a_record_is_refused_before_the_clone() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let goal = "x".repeat(fabric_core::ledger::MAX_RECORD_LEN);
        let refused = node.job_new(&repo, goal, HarnessKind::Codex).await;
        assert!(matches!(refused, Err(NodeError::Ledger(_))), "{refused:?}");
        let jobs = node.cfg.home.join("fabric").join("jobs");
        let left: Vec<PathBuf> = std::fs::read_dir(&jobs)
            .map(|dir| dir.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "the clone stayed behind: {left:?}");
    }

    /// A departed clean copy that cannot be looked at, here because its
    /// parent lost search permission, is still there. Settling must fail
    /// and keep the marker, not take the copy for gone and forget it: a
    /// later resume would then find the root occupied by files no marker
    /// names.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_copy_that_cannot_be_looked_at_keeps_its_marker() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let path = env();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        let root = paths::job_root(&a.cfg.home, id);
        let parent = root.parent().unwrap().to_owned();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        b.job_steal(id, "a sleeps".to_owned(), true, &mut Out::default())
            .await
            .unwrap();
        a.settle_job(id, &mut Out::default()).await.unwrap_err();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        a.settle_job(id, &mut Out::default()).await.unwrap();
        let departed = paths::departed_copy(&root, Epoch::new(2).unwrap()).unwrap();
        assert!(
            departed.join("README.md").is_file() && !root.exists(),
            "the copy stayed in the root with no marker"
        );
    }

    /// A catch-up can freeze the job while a move's target copies it. The
    /// move must not then commit `MoveStarted` on the frozen chain, which
    /// the target would import and run.
    #[tokio::test]
    async fn a_move_does_not_depart_from_a_job_frozen_while_it_was_copied() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        node.freeze(
            id,
            &crate::local::ForkRecord {
                peer: NodeId::from_bytes([8; 32]),
                epoch: Epoch::FIRST,
                head: fabric_core::id::RecordHash::of(b"their head"),
            },
            &mut Out::default(),
        )
        .await
        .unwrap();
        let refused = node
            .depart(
                id,
                NodeId::from_bytes([7; 32]),
                &HarnessKind::Codex,
                &mut Out::default(),
            )
            .await;
        assert!(
            matches!(refused, Err(NodeError::Forked { .. })),
            "{refused:?}"
        );
        let job = node.job(id).await.unwrap();
        assert!(
            !job.lock()
                .await
                .ledger()
                .records()
                .iter()
                .any(|r| matches!(r.event().kind, EventKind::MoveStarted { .. })),
            "MoveStarted was appended on a frozen chain"
        );
    }

    /// A catch-up that waits for the job's lock while a steal checks the
    /// fork can freeze the job between the steal's check and its commit.
    /// The steal must not then append a new epoch on the frozen chain.
    /// The lock is fair, so a freeze queued behind the steal runs at the
    /// steal's first release of the lock.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_steal_appends_nothing_after_a_freeze_it_did_not_see() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let path = env();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        let job = b.job(id).await.unwrap();
        let held = job.lock().await;
        let thief = Arc::clone(&b);
        let steal = tokio::spawn(async move {
            thief
                .job_steal(id, "a sleeps".to_owned(), true, &mut Out::default())
                .await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let frozen_job = Arc::clone(&job);
        let a_id = a.id;
        let freeze = tokio::spawn(async move {
            let job = frozen_job.lock().await;
            job.freeze(&crate::local::ForkRecord {
                peer: a_id,
                epoch: Epoch::new(2).unwrap(),
                head: fabric_core::id::RecordHash::of(b"a head"),
            })
            .unwrap();
            job.ledger().records().len()
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(held);
        let at_freeze = freeze.await.unwrap();
        let stolen = steal.await.unwrap();
        let after = job.lock().await.ledger().records().len();
        assert_eq!(
            after, at_freeze,
            "the steal appended to the chain after it was frozen ({stolen:?})"
        );
    }

    /// A `resume --harness` can register another harness while a move's
    /// preflight checks the target for the old one. The move must not then
    /// hand the job, now under the new harness, to a target never checked
    /// for it.
    #[tokio::test]
    async fn a_move_does_not_depart_under_a_harness_its_preflight_did_not_check() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let harness = sleeper();
        node.harness_started(StartedHarness {
            job: id,
            epoch: Epoch::FIRST,
            pid: pid(&harness),
            harness: HarnessKind::ClaudeCode,
            session: None,
            planned: None,
            version: None,
        })
        .await
        .unwrap();
        let reaper = std::thread::spawn(move || {
            let mut harness = harness;
            harness.wait()
        });
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        reaper.join().unwrap().unwrap();
        let refused = node
            .depart(
                id,
                NodeId::from_bytes([7; 32]),
                &HarnessKind::Codex,
                &mut Out::default(),
            )
            .await;
        assert!(
            matches!(
                &refused,
                Err(NodeError::HarnessChangedDuringMove { checked, now, .. })
                    if **checked == HarnessKind::Codex && **now == HarnessKind::ClaudeCode
            ),
            "{refused:?}"
        );
        let job = node.job(id).await.unwrap();
        assert!(
            !job.lock()
                .await
                .ledger()
                .records()
                .iter()
                .any(|r| matches!(r.event().kind, EventKind::MoveStarted { .. })),
            "MoveStarted was appended under an unchecked harness"
        );
    }

    /// A clean workspace whose root was replaced with a link to another
    /// directory is not missing, so a resume does not restore it. Neither
    /// a run nor a resume may then hand out the root: the CLI resolves the
    /// link and would run setup and the harness in the other directory.
    #[tokio::test]
    async fn a_root_replaced_with_a_link_is_never_launched_in() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let root = paths::job_root(&node.cfg.home, id);
        let elsewhere = dir.path().join("elsewhere");
        std::fs::rename(&root, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();
        let resumed = node
            .job_resume(id, None, None, false, &path, &mut Out::default())
            .await;
        assert!(
            matches!(&resumed, Err(NodeError::RootNotADirectory { root: r, .. }) if *r == root),
            "{resumed:?}"
        );
        let ran = node.job_run(id, &mut Out::default()).await;
        assert!(
            matches!(&ran, Err(NodeError::RootNotADirectory { .. })),
            "{ran:?}"
        );
    }

    /// A restore that died after its marker said `Restoring` and before
    /// it wrote anything leaves the root empty, and another process can
    /// fill it before the next resume. The resume must not take those
    /// files for the restored snapshot; a tree the restore did finish,
    /// before a kill took the marker's last write, is still accepted.
    #[tokio::test]
    async fn a_resumed_restore_accepts_only_the_snapshots_own_tree() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let root = paths::job_root(&node.cfg.home, id);
        let job = node.job(id).await.unwrap();
        let mut job = job.lock().await;
        let restoring = Marker {
            root: root.clone(),
            state: MarkerState::Restoring {
                snapshot: job.latest().unwrap(),
            },
            epoch: Epoch::FIRST,
        };

        job.set_marker(&restoring).unwrap();
        let restored = node
            .restore_if_needed(&mut job, Epoch::FIRST, &path, &mut Out::default())
            .await;
        assert_eq!(
            restored.unwrap(),
            root,
            "the snapshot's own tree was refused"
        );

        let theirs = dir.path().join("theirs");
        std::fs::rename(&root, &theirs).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("README.md"), "not the job's\n").unwrap();
        job.set_marker(&restoring).unwrap();
        let refused = node
            .restore_if_needed(&mut job, Epoch::FIRST, &path, &mut Out::default())
            .await;
        assert!(
            matches!(&refused, Err(NodeError::WorkspaceOccupied { root: r }) if *r == root),
            "{refused:?}"
        );
        assert!(matches!(
            job.marker().unwrap().unwrap().state,
            MarkerState::Restoring { .. }
        ));
    }

    /// A suspend must say what its snapshot carries that a secret pattern
    /// matches, and what of the workspace it leaves out: a tracked `.env`
    /// travels in the bundle, and a nested repository stays behind.
    /// Before, only ignored and untracked secrets were named.
    #[tokio::test]
    async fn a_suspend_names_tracked_secrets_and_nested_repositories() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        std::fs::write(repo.join(".env"), "TOKEN=x\n").unwrap();
        for args in [
            &["add", "-f", ".env"][..],
            &[
                "-c",
                "user.name=f",
                "-c",
                "user.email=f@x",
                "commit",
                "-qm",
                "2",
            ],
        ] {
            let status = std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap();
            assert!(status.success());
        }
        let id = node
            .job_new(&repo, "goal".to_owned(), HarnessKind::Codex)
            .await
            .unwrap()
            .job;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::create_dir(root.join("vendored")).unwrap();
        let made = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.join("vendored"))
            .status()
            .unwrap();
        assert!(made.success());
        let path = env();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        for wanted in [
            "travels in the snapshot though a secret pattern matches it (tracked): .env",
            "left behind (nested repository): vendored",
        ] {
            assert!(
                out.said().iter().any(|line| line == wanted),
                "no line {wanted:?} in {:?}",
                out.said()
            );
        }
    }

    /// Goals near the record bound, five of them, add up past the control
    /// message cap. `job ls` must still answer: it names each goal by a
    /// short summary, and `job status` shows one goal whole.
    #[tokio::test]
    async fn a_job_list_fits_one_answer_whatever_the_goals() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let goal = format!("first line\n{}", "x".repeat(900 << 10));
        for _ in 0..5 {
            node.job_new(&repo, goal.clone(), HarnessKind::Codex)
                .await
                .unwrap();
        }
        let list = node.job_list().await;
        assert_eq!((list.jobs.len(), list.left_out), (5, 0));
        let bytes = serde_json::to_vec(&list).unwrap().len();
        assert!(bytes < LIST_BUDGET, "{bytes} bytes of jobs");
        assert!(
            list.jobs.iter().all(|job| job.goal == "first line ..."),
            "{:?}",
            list.jobs
        );
    }

    /// Each peer of `node ls` is bounded, and the peer book does not bound
    /// how many peers it holds. The answer keeps the last peers that fit
    /// its budget and says how many it left out.
    #[tokio::test]
    async fn a_node_list_past_its_budget_keeps_the_last_rows() {
        let (_dir, node) = node();
        for byte in 1..=3_u8 {
            node.node_add(
                NodeId::from_bytes([byte; 32]),
                Some(&format!("10.0.0.{byte}:1")),
            )
            .await
            .unwrap();
        }
        let all = node.node_list();
        assert_eq!((all.peers.len(), all.left_out), (3, 0));
        let row = serde_json::to_vec(&all.peers[2]).unwrap().len();
        let cut = node.node_list_within(2 * row);
        assert_eq!(cut.left_out, 1);
        assert_eq!(cut.peers, all.peers[1..]);
    }

    /// Each job of `job ls` is bounded, and enough jobs still add up past
    /// the frame cap. The answer keeps the latest jobs that fit its budget
    /// and says how many older ones it left out.
    #[tokio::test]
    async fn a_job_list_past_its_budget_keeps_the_latest_rows() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let mut ids = Vec::new();
        for _ in 0..3 {
            let created = node
                .job_new(&repo, "goal".to_owned(), HarnessKind::Codex)
                .await
                .unwrap();
            ids.push(created.job);
        }
        // Jobs are in id order, and an id begins with its time.
        ids.sort();
        let all = node.job_list().await;
        assert_eq!((all.jobs.len(), all.left_out), (3, 0));
        let row = serde_json::to_vec(&all.jobs[2]).unwrap().len();
        let cut = node.job_list_within(2 * row).await;
        assert_eq!(cut.left_out, 1);
        let kept: Vec<JobId> = cut.jobs.iter().map(|job| job.job).collect();
        assert_eq!(kept, ids[1..]);
    }

    /// Two runs can both get a plan before either launches. Once the first
    /// registered its harness and let the tree lock go, the second must be
    /// refused before its setup steps run in the workspace the first
    /// harness writes to.
    #[tokio::test]
    async fn a_plan_another_launch_overtook_is_refused_before_setup() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let first = node.job_run(id, &mut Out::default()).await.unwrap();
        let second = node.job_run(id, &mut Out::default()).await.unwrap();
        check(&node, &first).await.unwrap();
        let mut harness = sleeper();
        register(&node, id, pid(&harness)).await.unwrap();
        let refused = check(&node, &second).await;
        assert!(
            matches!(refused, Err(NodeError::AlreadyRunningHarness { pid: p, .. }) if p.get() == harness.id()),
            "{refused:?}"
        );
        harness.kill().unwrap();
        harness.wait().unwrap();
    }

    /// A workspace holds what the agent writes there, ignored secrets
    /// included, so the job directory that holds it is this user's alone,
    /// whether `job new` made it or a restore did.
    #[tokio::test]
    async fn a_job_directory_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        let parent = root.parent().unwrap().to_owned();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&parent), 0o700, "job new");
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        std::fs::remove_dir_all(&parent).unwrap();
        node.job_resume(id, None, None, false, &path, &mut Out::default())
            .await
            .unwrap();
        assert!(root.join("README.md").is_file());
        assert_eq!(mode(&parent), 0o700, "restore");
    }

    /// A pull checks this node for the harness the job will launch. A
    /// `resume --harness` on the holder can register another harness after
    /// that check and before the holder suspends for the move, and the
    /// pulled records then name it. The pull must abort the move, leaving
    /// the job with the holder, not import it under a harness never checked
    /// here.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pull_aborts_when_the_holder_changed_harness_after_the_check() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let path = env();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        let harness = sleeper();
        a.harness_started(StartedHarness {
            job: id,
            epoch: Epoch::FIRST,
            pid: pid(&harness),
            harness: HarnessKind::ClaudeCode,
            session: None,
            planned: None,
            version: None,
        })
        .await
        .unwrap();
        let reaper = std::thread::spawn(move || {
            let mut harness = harness;
            harness.wait()
        });
        let pulled = b
            .pull(
                id,
                a.id,
                (Some(&HarnessKind::Codex), false, &NodeFacts::default()),
                &env(),
                &mut Out::default(),
            )
            .await;
        reaper.join().unwrap().unwrap();
        assert!(
            matches!(
                &pulled,
                Err(NodeError::HarnessChangedDuringPull { checked, now, .. })
                    if **checked == HarnessKind::Codex && **now == HarnessKind::ClaudeCode
            ),
            "{pulled:?}"
        );
        let held = a.job(id).await.unwrap();
        assert!(
            matches!(held.lock().await.view(a.id), LeaseView::Owner(_)),
            "the holder did not get the job back"
        );
        let pulled = b.job(id).await.unwrap();
        assert!(!matches!(
            pulled.lock().await.view(b.id),
            LeaseView::Owner(_)
        ));
    }

    /// A pull decides each ignored path of the snapshot with this node's
    /// facts before it imports, as a push does before `MoveStarted`. A
    /// locked tree this node can neither rebuild nor use, here a pnpm tree
    /// no finished install vouches for on a node whose `PATH` has no pnpm,
    /// aborts the move and leaves the job with the holder; imported first,
    /// the job would own a tree its resume cannot install (review of PR #11).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pull_aborts_when_this_node_cannot_use_or_rebuild_a_path() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let root = paths::job_root(&a.cfg.home, id);
        std::fs::write(root.join("package.json"), "{\"name\": \"x\"}\n").unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "1;\n").unwrap();
        a.suspend(id, &env(), &mut Out::default()).await.unwrap();
        let no_pnpm = on_path(std::ffi::OsStr::new("/usr/bin:/bin"));
        let mut out = Out::default();
        let pulled = b
            .pull(
                id,
                a.id,
                (None, false, &NodeFacts::default()),
                &no_pnpm,
                &mut out,
            )
            .await;
        assert!(
            matches!(&pulled, Err(NodeError::CarryRefused { paths, .. }) if paths == &["node_modules"]),
            "{pulled:?} {:?}",
            out.said()
        );
        assert!(
            out.said()
                .iter()
                .any(|line| line.starts_with("refuse  node_modules")),
            "{:?}",
            out.said()
        );
        let held = a.job(id).await.unwrap();
        assert!(
            matches!(held.lock().await.view(a.id), LeaseView::Owner(_)),
            "the holder did not get the job back"
        );
        let pulled = b.job(id).await.unwrap();
        assert!(!matches!(
            pulled.lock().await.view(b.id),
            LeaseView::Owner(_)
        ));
    }

    /// A step declared `when = "always"` runs after every restore, whatever
    /// the carry decision did with each cache, and materialize refuses one
    /// whose program is missing before anything runs. A move to a node
    /// without that program stops before the job changes hands, pushed
    /// (after its suspend, before `MoveStarted`) or pulled (before the
    /// import); it stopped only for a path (issue #24).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_stops_where_a_step_run_on_every_resume_cannot_run() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let root = paths::job_root(&a.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "[[setup]]\nprogram = \"uv\"\nargs = [\"run\", \"codegen.py\"]\nwhen = \"always\"\n",
        )
        .unwrap();
        a.suspend(id, &env(), &mut Out::default()).await.unwrap();
        let snapshot = a.job(id).await.unwrap().lock().await.latest().unwrap();
        let uv = |version: Option<&'static str>| {
            NodeFacts::from_pairs(version.map(|v| (crate::facts::name::UV, v)))
        };
        let mut out = Out::default();
        let pushed = a
            .carry_for_move(
                b.id,
                snapshot,
                (&uv(Some("uv 0.9.5")), &uv(None)),
                Some(false),
                &mut out,
            )
            .await;
        assert!(
            matches!(&pushed, Err(NodeError::Preflight(facts)) if facts == &["uv.version"]),
            "{pushed:?}"
        );
        assert!(
            out.said()
                .iter()
                .any(|line| line.starts_with("refuse  uv.version")),
            "{:?}",
            out.said()
        );
        a.carry_for_move(
            b.id,
            snapshot,
            (&uv(Some("uv 0.9.5")), &uv(Some("uv 0.9.5"))),
            Some(false),
            &mut Out::default(),
        )
        .await
        .unwrap();
        let no_uv = on_path(std::ffi::OsStr::new("/usr/bin:/bin"));
        let pulled = b
            .pull(
                id,
                a.id,
                (None, false, &uv(Some("uv 0.9.5"))),
                &no_uv,
                &mut Out::default(),
            )
            .await;
        assert!(
            matches!(&pulled, Err(NodeError::Preflight(facts)) if facts == &["uv.version"]),
            "{pulled:?}"
        );
        let held = a.job(id).await.unwrap();
        assert!(
            matches!(held.lock().await.view(a.id), LeaseView::Owner(_)),
            "the holder did not get the job back"
        );
        let pulled = b.job(id).await.unwrap();
        assert!(!matches!(
            pulled.lock().await.view(b.id),
            LeaseView::Owner(_)
        ));
    }

    /// The target plans its steps from its own detection over the restored
    /// tree. With `.fabric/env.toml` kept on the source by `.fabricignore`,
    /// the `always` step it declares never runs there, so a move to a node
    /// without that step's program goes ahead; it was refused for a step
    /// the target would not run (review of PR #27).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_ignores_the_steps_of_an_override_file_left_behind() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let root = paths::job_root(&a.cfg.home, id);
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "[[setup]]\nprogram = \"uv\"\nargs = [\"run\", \"codegen.py\"]\nwhen = \"always\"\n",
        )
        .unwrap();
        std::fs::write(root.join(".fabricignore"), ".fabric/env.toml\n").unwrap();
        a.suspend(id, &env(), &mut Out::default()).await.unwrap();
        let snapshot = a.job(id).await.unwrap().lock().await.latest().unwrap();
        let uv = |version: Option<&'static str>| {
            NodeFacts::from_pairs(version.map(|v| (crate::facts::name::UV, v)))
        };
        let mut out = Out::default();
        let pushed = a
            .carry_for_move(
                b.id,
                snapshot,
                (&uv(Some("uv 0.9.5")), &uv(None)),
                Some(false),
                &mut out,
            )
            .await;
        assert!(pushed.is_ok(), "{pushed:?} {:?}", out.said());
    }

    /// A cache written with holes arrives without its sentinel. Here a
    /// pnpm tree, one file of which `.fabricignore` kept on the source, is
    /// written on a node with no pnpm, as a refusal is without a move. With
    /// the sentinel, the lockfile and `package.json` that travelled proved
    /// it whole, and materialize would never fill the hole (issue #24).
    #[tokio::test]
    async fn a_cache_restored_with_holes_is_not_called_whole() {
        use fabric_env::materialize::{CacheHealth, SENTINEL, cache_health, sentinel_text};
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::write(root.join("package.json"), "{\"name\": \"x\"}\n").unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(root.join(".fabricignore"), "node_modules/pkg/notes.txt\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "1;\n").unwrap();
        std::fs::write(root.join("node_modules/pkg/notes.txt"), "kept here\n").unwrap();
        let node_modules = fabric_core::path::RelPath::parse("node_modules").unwrap();
        let health = |root: &Path| {
            let detection = fabric_env::detect::detect(root, &[]).unwrap();
            cache_health(&detection, &node_modules).unwrap()
        };
        let detection = fabric_env::detect::detect(&root, &[]).unwrap();
        std::fs::write(
            root.join("node_modules").join(SENTINEL),
            sentinel_text(&detection, &node_modules).unwrap(),
        )
        .unwrap();
        assert_eq!(health(&root), Some(CacheHealth::Healthy));
        node.suspend(id, &env(), &mut Out::default()).await.unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        let no_pnpm = on_path(std::ffi::OsStr::new("/usr/bin:/bin"));
        let mut out = Out::default();
        node.job_resume(id, None, None, false, &no_pnpm, &mut out)
            .await
            .unwrap();
        assert!(
            root.join("node_modules/pkg/index.js").is_file(),
            "the tree was not written: {:?}",
            out.said()
        );
        assert!(!root.join("node_modules/pkg/notes.txt").exists());
        assert_eq!(health(&root), Some(CacheHealth::Incomplete));
    }

    /// A tree with holes that travels as the user's files arrives as it
    /// left, sentinel included. Here `.fabric/env.toml` stayed behind
    /// beside a pnpm tree with a hole, so the capture forgot the tree's
    /// rebuild, and the carry decision carries it to a node without pnpm
    /// and checks no step. Without the sentinel, the lockfile that
    /// travelled would make materialize there run `pnpm install` and
    /// refuse it, after a move had handed the job over (review of PR #27).
    #[tokio::test]
    async fn a_tree_carried_as_the_users_files_keeps_its_sentinel() {
        use fabric_core::snapshot::{CarryState, Remake};
        use fabric_env::materialize::{CacheHealth, SENTINEL, cache_health, sentinel_text};
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::write(root.join("package.json"), "{\"name\": \"x\"}\n").unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(root.join(".fabric/env.toml"), "").unwrap();
        std::fs::write(
            root.join(".fabricignore"),
            ".fabric/env.toml\nnode_modules/pkg/notes.txt\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "1;\n").unwrap();
        std::fs::write(root.join("node_modules/pkg/notes.txt"), "kept here\n").unwrap();
        let node_modules = fabric_core::path::RelPath::parse("node_modules").unwrap();
        let health = |root: &Path| {
            let detection = fabric_env::detect::detect(root, &[]).unwrap();
            cache_health(&detection, &node_modules).unwrap()
        };
        let detection = fabric_env::detect::detect(&root, &[]).unwrap();
        std::fs::write(
            root.join("node_modules").join(SENTINEL),
            sentinel_text(&detection, &node_modules).unwrap(),
        )
        .unwrap();
        let snapshot = node.suspend(id, &env(), &mut Out::default()).await.unwrap();
        let (manifest, _) = node.manifest_and_spec(snapshot).await.unwrap();
        let row = manifest
            .caches
            .iter()
            .find(|cache| cache.path == node_modules)
            .unwrap();
        assert_eq!(
            (row.carry, &row.remake),
            (CarryState::Partial, &Remake::Nothing)
        );
        std::fs::remove_dir_all(&root).unwrap();
        let no_pnpm = on_path(std::ffi::OsStr::new("/usr/bin:/bin"));
        let mut out = Out::default();
        node.job_resume(id, None, None, false, &no_pnpm, &mut out)
            .await
            .unwrap();
        assert!(
            root.join("node_modules/pkg/index.js").is_file(),
            "the tree was not written: {:?}",
            out.said()
        );
        assert_eq!(health(&root), Some(CacheHealth::Healthy));
    }

    /// A workspace root replaced with a link to another directory must not
    /// be captured: the snapshot would publish, and replicate to every
    /// peer, whatever the link names.
    #[tokio::test]
    async fn a_suspend_never_captures_through_a_root_that_is_a_link() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("private.txt"), "not the job's\n").unwrap();
        let aside = dir.path().join("aside");
        std::fs::rename(&root, &aside).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();
        let path = env();
        let refused = node.suspend(id, &path, &mut Out::default()).await;
        assert!(
            matches!(&refused, Err(NodeError::RootNotADirectory { root: r, .. }) if *r == root),
            "{refused:?}"
        );
        let job = node.job(id).await.unwrap();
        assert!(
            !job.lock()
                .await
                .ledger()
                .records()
                .iter()
                .any(|r| matches!(r.event().kind, EventKind::SnapshotPublished { .. })),
            "a snapshot was published through the link"
        );
    }

    /// A suspend must say what of the repository's config it changed or
    /// left out: a remote URL loses its user info, and a URL with a query
    /// is left behind.
    #[tokio::test]
    async fn a_suspend_names_the_git_config_it_changed_or_left_out() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        for args in [
            &[
                "remote",
                "set-url",
                "origin",
                "https://me:pw@example.com/r.git",
            ][..],
            &[
                "remote",
                "add",
                "tokened",
                "https://example.com/r.git?token=x",
            ],
        ] {
            let status = std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        }
        let path = env();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        for wanted in [
            "travels without its user info (git config): remote.origin.url",
            "left behind (git config): remote.tokened.url",
        ] {
            assert!(
                out.said().iter().any(|line| line == wanted),
                "no line {wanted:?} in {:?}",
                out.said()
            );
        }
    }

    /// A clone can fail after it made the workspace, as when its checkout
    /// cannot read a file. No job and no printed id then name the workspace,
    /// so `job new` must take it away with the job directory it made.
    #[tokio::test]
    async fn a_clone_that_fails_leaves_no_job_directory() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let blob = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "HEAD:README.md"])
            .output()
            .unwrap();
        let blob = String::from_utf8(blob.stdout).unwrap();
        let blob = blob.trim();
        std::fs::remove_file(repo.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
        let refused = node
            .job_new(&repo, "goal".to_owned(), HarnessKind::Codex)
            .await;
        assert!(matches!(refused, Err(NodeError::Io { .. })), "{refused:?}");
        let jobs = node.cfg.home.join("fabric").join("jobs");
        let left: Vec<PathBuf> = std::fs::read_dir(&jobs)
            .map(|dir| dir.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "the failed clone stayed behind: {left:?}");
    }

    /// A workspace can hold any number of paths a secret pattern leaves
    /// behind. The suspend's answer must stay within one control message,
    /// so it names the first of them and counts the rest.
    #[tokio::test]
    async fn a_suspend_names_a_bounded_number_of_paths() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let root = paths::job_root(&node.cfg.home, id);
        for n in 0..500 {
            std::fs::write(root.join(format!(".env.{n}")), "TOKEN=x\n").unwrap();
        }
        let path = env();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        let named = out
            .said()
            .iter()
            .filter(|line| line.starts_with("left behind (secret pattern): .env."))
            .count();
        assert_eq!(named, REPORT_PATHS, "{:?}", out.said().len());
        let rest = format!("left behind (secret pattern): {} more", 500 - REPORT_PATHS);
        assert!(out.said().contains(&rest), "no line {rest:?}");
    }

    /// A launch that overtook a plan can register and exit before the
    /// delayed CLI takes the tree lock: no harness runs, and the epoch is
    /// the same. The delayed plan still predates that launch's `Resumed`
    /// and its session, and launching it would record a stale session over
    /// the new one, so it is refused too.
    #[tokio::test]
    async fn a_plan_a_finished_launch_overtook_is_refused_before_setup() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let first = node.job_run(id, &mut Out::default()).await.unwrap();
        let second = node.job_run(id, &mut Out::default()).await.unwrap();
        check(&node, &first).await.unwrap();
        let mut harness = sleeper();
        let harness_pid = pid(&harness);
        register(&node, id, harness_pid).await.unwrap();
        harness.kill().unwrap();
        harness.wait().unwrap();
        node.harness_exited(id, harness_pid).await.unwrap();
        let refused = check(&node, &second).await;
        assert!(
            matches!(refused, Err(NodeError::PlanOvertaken { .. })),
            "{refused:?}"
        );
    }

    /// A headless turn can name its session after its harness exited,
    /// while the CLI still reads the turn's output, and that moves no
    /// ledger head. A plan made in between would continue the session from
    /// before the turn and leave the turn's out, so it is refused under the
    /// tree lock, and at registration when the session arrives while the
    /// setup steps run.
    #[tokio::test]
    async fn a_plan_made_before_a_late_session_is_refused() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        node.job_run(id, &mut Out::default()).await.unwrap();
        let mut turn = sleeper();
        let turn_pid = pid(&turn);
        register(&node, id, turn_pid).await.unwrap();
        turn.kill().unwrap();
        turn.wait().unwrap();
        // The CLI reaped the turn and reads the rest of its output.
        let next = node.job_run(id, &mut Out::default()).await.unwrap();
        assert_eq!(next.session, None);
        check(&node, &next).await.unwrap();
        let thread = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        node.harness_session(
            id,
            Epoch::FIRST,
            turn_pid,
            HarnessKind::Codex,
            thread.clone(),
        )
        .await
        .unwrap();

        let refused = check(&node, &next).await;
        assert!(
            matches!(refused, Err(NodeError::SessionMoved { .. })),
            "{refused:?}"
        );
        let mut launched = sleeper();
        let refused = node
            .harness_started(StartedHarness {
                job: id,
                epoch: next.epoch,
                pid: pid(&launched),
                harness: next.harness.clone(),
                session: None,
                planned: next.session.clone(),
                version: None,
            })
            .await;
        launched.kill().unwrap();
        launched.wait().unwrap();
        assert!(
            matches!(refused, Err(NodeError::SessionMoved { .. })),
            "{refused:?}"
        );
        let job = node.job(id).await.unwrap();
        let recorded = job.lock().await.session().unwrap();
        assert_eq!(recorded.map(|record| record.session), Some(thread));
    }

    /// `forked_from` reads this node's epoch, then freezes under the job's
    /// lock. A steal or import in between can carry this node past the
    /// peer's epoch, and then the peer is the side that stops: the freeze
    /// must compare again under its lock and leave the job running here.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_fork_behind_this_nodes_epoch_never_freezes_the_job() {
        let (dir_a, a) = node();
        let (_dir_b, b) = node();
        paired(&a, &b).await;
        let id = job(dir_a.path(), &a).await;
        let path = env();
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        b.job_steal(id, "a sleeps".to_owned(), true, &mut Out::default())
            .await
            .unwrap();
        let refused = b
            .freeze(
                id,
                &crate::local::ForkRecord {
                    peer: a.id,
                    epoch: Epoch::FIRST,
                    head: fabric_core::id::RecordHash::of(b"a head at epoch 1"),
                },
                &mut Out::default(),
            )
            .await;
        assert!(
            matches!(refused, Err(NodeError::ForkBehind { .. })),
            "{refused:?}"
        );
        let job = b.job(id).await.unwrap();
        assert!(
            job.lock().await.forked().unwrap().is_none(),
            "a fork behind this node's epoch froze the job"
        );
    }

    /// A suspend that failed after its marker said `Suspending` finished
    /// nothing. A `Suspended` from before that suspend began must not
    /// settle the marker: the tree holds the work done since, and calling
    /// it clean at the older snapshot would let a later restore or move
    /// treat that work as already in a snapshot.
    #[tokio::test]
    async fn a_failed_suspend_is_not_settled_by_an_earlier_one() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = env();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        node.job_run(id, &mut Out::default()).await.unwrap();
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::write(root.join("README.md"), "work after the snapshot\n").unwrap();
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(root.join(".fabric/env.toml"), "not toml [\n").unwrap();
        let refused = node.suspend(id, &path, &mut Out::default()).await;
        assert!(matches!(refused, Err(NodeError::Detect(_))), "{refused:?}");
        let job = node.job(id).await.unwrap();
        let marker = job.lock().await.reconcile_marker().unwrap().unwrap();
        assert!(
            matches!(marker.state, MarkerState::Suspending { .. }),
            "{:?}",
            marker.state
        );
    }

    /// A snapshot may name any number of cache paths, and a resume's answer
    /// must stay within one control message, so the decision lines name
    /// the first of them and count the rest by what the target does.
    #[test]
    fn a_resume_names_a_bounded_number_of_caches() {
        let decisions: Vec<PathDecision> = (0..500)
            .map(|n| PathDecision {
                path: fabric_core::path::RelPath::parse(&format!("caches/{n}")).unwrap(),
                decision: if n < 490 {
                    carry::CarryDecision::LeaveBehind {
                        reason: carry::LeaveBehindReason::NothingCaptured,
                    }
                } else {
                    carry::CarryDecision::Rebuild {
                        by: carry::RebuildBy::NextBuild,
                        because: "rustc -vV differs".to_owned(),
                    }
                },
            })
            .collect();
        let mut out = Out::default();
        out.emit(carry_decided(
            CarryFor::Move {
                target: NodeId::from_bytes([9; 32]),
            },
            SnapshotId::from_bytes([7; 32]),
            &decisions,
        ));
        assert_eq!(
            out.said().len(),
            REPORT_PATHS + 2,
            "{:?}",
            out.said().last()
        );
        assert_eq!(
            out.said().last().map(String::as_str),
            Some("480 more paths: leave 470, rebuild 10")
        );
    }

    /// `harness.toml` may configure any number of ACP agents. A run plan
    /// carries only the configuration of the harness it launches, so the
    /// plan stays within one control message whatever the file holds.
    #[tokio::test]
    async fn a_run_plan_carries_only_its_own_harness_config() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .prefix("fops")
            .tempdir_in("/tmp")
            .unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            state.join("harness.toml"),
            "[acp.opencode]\nstate_dir = \"oc\"\n[acp.other]\nstate_dir = \"other\"\n",
        )
        .unwrap();
        let node = Node::start(NodeConfig {
            state: StateDir::resolve(Some(state.as_os_str()), &home).unwrap(),
            home,
            listen: "127.0.0.1:0".parse().unwrap(),
            dial: DialOrder::TcpOnly,
            env: ToolEnv::current(),
        })
        .unwrap();
        let repo = repo(&root);
        for (harness, wanted) in [("acp:opencode", vec!["opencode"]), ("codex", vec![])] {
            let id = node
                .job_new(
                    &repo,
                    "goal".to_owned(),
                    crate::harnesses::parse_harness(harness).unwrap(),
                )
                .await
                .unwrap()
                .job;
            let plan = node.job_run(id, &mut Out::default()).await.unwrap();
            let carried: Vec<&str> = plan.harness_config.acp.keys().map(String::as_str).collect();
            assert_eq!(carried, wanted, "{harness}");
        }
    }

    /// A job on `node` whose workspace holds a `node_modules` an npm
    /// lockfile describes, and the workspace.
    async fn npm_job(dir: &Path, node: &Arc<Node>) -> (JobId, PathBuf) {
        let id = job(dir, node).await;
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::write(root.join("package.json"), "{\"name\": \"x\"}\n").unwrap();
        std::fs::write(root.join("package-lock.json"), "{}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "1;\n").unwrap();
        (id, root)
    }

    /// A suspend measures only what its carry keys read: no login and no
    /// tool of an ecosystem the workspace does not use. For an npm tree
    /// that is node and npm, and python3, whose facts key a `.venv`
    /// detection does not know (issue #26).
    #[tokio::test]
    async fn a_suspend_probes_only_what_its_keys_read() {
        let (dir, node) = node();
        let tools = crate::facts::tests::CountingTools::new(dir.path());
        let (id, _) = npm_job(dir.path(), &node).await;
        node.suspend(id, &tools.env, &mut Out::default())
            .await
            .unwrap();
        let ran = tools.ran();
        assert!(
            !ran.iter()
                .any(|probe| probe.contains("auth") || probe.contains("login")),
            "a suspend asked for a login: {ran:?}"
        );
        assert_eq!(
            ran,
            [
                "node --version",
                "node -p process.versions.modules",
                "npm --version",
                "python3 --version",
                "python3 -c import sys; print(sys.base_prefix)",
            ]
        );
    }

    /// A second suspend reuses the answers of the first and starts no
    /// probe, until a tool is written again; that tool alone is then
    /// probed (issue #26).
    #[tokio::test]
    async fn a_second_suspend_probes_only_a_tool_written_since() {
        let (dir, node) = node();
        let tools =
            crate::facts::tests::CountingTools::only(dir.path(), &["node", "npm", "python3"]);
        let (id, root) = npm_job(dir.path(), &node).await;
        node.suspend(id, &tools.env, &mut Out::default())
            .await
            .unwrap();
        assert_eq!(tools.ran().len(), 5, "the first suspend");
        std::fs::write(root.join("README.md"), "edited\n").unwrap();
        node.suspend(id, &tools.env, &mut Out::default())
            .await
            .unwrap();
        assert_eq!(tools.ran(), Vec::<String>::new(), "the second suspend");
        tools.touch("npm");
        std::fs::write(root.join("README.md"), "edited again\n").unwrap();
        node.suspend(id, &tools.env, &mut Out::default())
            .await
            .unwrap();
        assert_eq!(tools.ran(), ["npm --version"], "after npm changed");
    }

    /// The preflight of a move and of a pull measures this node while the
    /// peer measures itself (issue #26). Each node's `sw_vers` waits up to
    /// five seconds for the other's to start and says whether it did, so a
    /// node that measured only after the other finished reads `alone`.
    /// Both moves are refused, as the two test nodes have different homes,
    /// after the rows are reported.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_preflight_measures_here_and_on_the_peer_at_once() {
        let dir = tempfile::Builder::new()
            .prefix("fops")
            .tempdir_in("/tmp")
            .unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let meet = root.join("meet");
        std::fs::create_dir(&meet).unwrap();
        let tools = |me: &str, other: &str| {
            let tools = crate::facts::tests::CountingTools::only(&root.join(me), &[]);
            let (mine, theirs) = (meet.join(me), meet.join(other));
            tools.install(
                "sw_vers",
                &format!(
                    "touch '{mine}'\ni=0\nwhile [ ! -f '{theirs}' ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i + 1)); done\nif [ -f '{theirs}' ]; then echo together; else echo alone; fi",
                    mine = mine.display(),
                    theirs = theirs.display()
                ),
            );
            tools
        };
        let (tools_a, tools_b) = (tools("a", "b"), tools("b", "a"));
        let a = node_at(
            &root.join("a/home"),
            &root.join("a/state"),
            tools_a.env.clone(),
        );
        let b = node_at(
            &root.join("b/home"),
            &root.join("b/state"),
            tools_b.env.clone(),
        );
        // Every measure probes, so each preflight shows its own.
        a.facts.set_ttl(Duration::ZERO);
        b.facts.set_ttl(Duration::ZERO);
        paired(&a, &b).await;
        let id = job(&root, &a).await;
        let macos_row = |out: &Out| {
            out.events
                .iter()
                .find_map(|event| match event {
                    Event::Preflight { rows, .. } => {
                        rows.iter().find(|row| row.fact == "macos.version").cloned()
                    }
                    _ => None,
                })
                .map(|row| (row.here, row.peer))
        };
        let together = Some((Some("together".to_owned()), Some("together".to_owned())));

        let mut out = Out::default();
        let pushed = a.job_move(id, b.id, false, &tools_a.env, &mut out).await;
        assert!(matches!(pushed, Err(NodeError::Preflight(_))), "{pushed:?}");
        assert_eq!(macos_row(&out), together, "the move");

        for marker in ["a", "b"] {
            std::fs::remove_file(meet.join(marker)).unwrap();
        }
        let mut out = Out::default();
        let pulled = b
            .job_resume(id, Some(a.id), None, false, &tools_b.env, &mut out)
            .await;
        assert!(matches!(pulled, Err(NodeError::Preflight(_))), "{pulled:?}");
        assert_eq!(macos_row(&out), together, "the pull");
    }
}
