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

use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::{FenceError, FencingToken};
use fabric_core::ledger::{Author, EventKind, Ledger};
use fabric_core::moves::{AbortAnswer, CompleteSnapshot, LeaseView, MoveOffer, OfferAnswer};
use fabric_env::materialize::{MaterializeError, TreeLock};
use fabric_harness::codex::Codex;
use fabric_harness::manifest::{MANIFEST_FILE, StateManifest};
use fabric_harness::{HarnessError, SessionId};
use fabric_net::peers::HostPort;
use fabric_net::rpc::Session;
use fabric_net::wire::{
    Appended, Fenced, MoveRequested, MoveState, Offered, Pulled, RecordBytes, RecordPage,
};
use fabric_store::capture::{CaptureRequest, capture};
use fabric_store::policy::IgnorePolicy;
use fabric_store::restore::{
    Durability, RestoreError, RestoreRequest, RestoreState, Verify, restore, restore_state,
};

use crate::clock;
use crate::control::{Decision, Request, RunPlan};
use crate::doctor::{self, Context, Target};
use crate::error::NodeError;
use crate::facts::{MeasuredKeys, NodeFacts, key_difference};
use crate::faults::{self, Action, Point};
use crate::harnesses::{HarnessConfig, HarnessEnv, adapter, refuse_secret_kind};
use crate::job::Job;
use crate::local::{self, HarnessRecord, Liveness, Marker, MarkerState, SessionRecord};
use crate::node::{Node, Out, is_transport};
use crate::paths::{self, SyncedFolders, check_root, restore_root};
use crate::stamp::TreeStamp;
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

/// The bytes of lines `job log` and `job ls` answer with at most: half the
/// control message cap, which leaves room for the encoding around them.
const LINES_BUDGET: usize = crate::control::MAX_CONTROL_MESSAGE / 2;

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
    ) -> Result<Option<RunPlan>, NodeError> {
        // Reads answer from local state at once; a command that changes a
        // job waits until start-up recovery has learned what the peers wrote.
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
            out.say(
                "this node is still catching up with its peers after a restart; what follows may not show what they wrote",
            );
        } else if !reads && !node_only {
            self.recovered().await;
        }
        match request {
            Request::NodeId => {
                out.say(format!("{}", self.id));
                out.say(format!("listening on {}", self.local_addr()?));
            }
            Request::NodeAdd { node, addr } => self.node_add(node, addr.as_deref(), out).await?,
            Request::NodeList => self.node_list(out),
            Request::NodePing { node } => self.node_ping(node, out).await?,
            Request::JobNew {
                repo,
                goal,
                harness,
            } => self.job_new(&repo, goal, harness, out).await?,
            Request::JobList => self.job_list(out).await,
            Request::JobStatus { job } => self.job_status(job, out).await?,
            Request::JobLog { job } => self.job_log(job, out).await?,
            Request::JobRun { job } => {
                self.resolve_departing(job, out).await?;
                self.check_peers(job, out).await;
                return Ok(Some(self.job_run(job, out).await?));
            }
            Request::JobResume {
                job,
                on,
                harness,
                force,
                path_var,
            } => {
                return Ok(Some(
                    Box::pin(self.job_resume(job, on, harness, force, &path_var, out)).await?,
                ));
            }
            Request::HarnessStarted {
                job,
                epoch,
                pid,
                harness,
                session,
            } => {
                self.harness_started(job, epoch, pid, harness, session, out)
                    .await?;
            }
            Request::LaunchCheck { job, epoch, head } => {
                self.launch_check(job, epoch, head).await?;
            }
            Request::HarnessExited { job, pid } => self.harness_exited(job, pid).await?,
            Request::JobSuspend { job, path_var } => {
                self.resolve_departing(job, out).await?;
                self.suspend(job, &path_var, out).await?;
                self.replicate(job, out).await;
            }
            Request::JobMove {
                job,
                to,
                force,
                path_var,
            } => self.job_move(job, to, force, &path_var, out).await?,
            Request::JobSteal { job, reason, force } => {
                self.job_steal(job, reason, force, out).await?;
            }
            Request::JobDecide {
                job,
                decision,
                note,
            } => {
                self.job_decide(job, decision, &note, out).await?;
            }
            Request::Doctor {
                peer,
                job,
                path_var,
            } => self.doctor(peer, job, &path_var, out).await?,
        }
        Ok(None)
    }

    async fn node_add(
        &self,
        node: NodeId,
        addr: Option<&str>,
        out: &mut Out,
    ) -> Result<(), NodeError> {
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
        out.say(match shown {
            Some(addr) => format!("paired {node} at {addr}"),
            None => format!(
                "paired {node} with no address: it may dial this node, and this node never dials it"
            ),
        });
        Ok(())
    }

    fn node_list(&self, out: &mut Out) {
        let book = self.peers.snapshot();
        let mut any = false;
        for entry in book.iter() {
            any = true;
            let connected = if self.open_session(entry.node()).is_some() {
                "connected"
            } else {
                "not connected"
            };
            let addrs: Vec<String> = entry.addrs().iter().map(ToString::to_string).collect();
            out.say(format!(
                "{} {} ({connected})",
                entry.node(),
                if addrs.is_empty() {
                    "-".to_owned()
                } else {
                    addrs.join(",")
                }
            ));
        }
        if !any {
            out.say("no paired nodes; pair one with `fabric node add <node-id> <host:port>`");
        }
    }

    /// Pings `node` over its live session, or a new dial when there is
    /// none or the live one fails: a second dial while a session is live
    /// would replace it and cut the calls other commands make on it.
    async fn node_ping(self: &Arc<Self>, node: NodeId, out: &mut Out) -> Result<(), NodeError> {
        let (took, wire, skew) = self
            .call(node, |session| async move {
                let started = tokio::time::Instant::now();
                session.ping(SHORT_DEADLINE).await?;
                Ok((
                    started.elapsed(),
                    session.kind().to_string(),
                    session.clock_skew_ms(),
                ))
            })
            .await?;
        out.say(format!(
            "{node} answered in {} ms over {wire}; its clock is {skew} ms from this one",
            took.as_millis(),
        ));
        Ok(())
    }

    async fn job_new(
        &self,
        repo: &Path,
        goal: String,
        harness: HarnessKind,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        refuse_secret_kind(&harness)?;
        let id = clock::job_id();
        let root = paths::job_root(&self.cfg.home, id);
        let synced = synced_folders(&self.cfg.home, &self.cfg.path_var).await;
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
        out.say(format!("{id}"));
        out.say(format!(
            "root {}: a clone of {} under ~/fabric/jobs, the same path on every node with this home (ADR 0011)",
            root.display(),
            repo.display()
        ));
        out.say(format!(
            "harness {harness}; this node owns the job at epoch {}",
            Epoch::FIRST
        ));
        Ok(())
    }

    async fn job_list(&self, out: &mut Out) {
        self.job_list_within(LINES_BUDGET, out).await;
    }

    /// Lists every job in one row each, oldest first, keeping the latest
    /// rows that fit in `budget` bytes: each row is bounded, and a node
    /// that made tens of thousands of jobs would otherwise answer past the
    /// control message cap.
    async fn job_list_within(&self, budget: usize, out: &mut Out) {
        let jobs: Vec<_> = self.jobs.lock().await.values().cloned().collect();
        let mut lines = Vec::new();
        for job in jobs {
            let job = job.lock().await;
            let manifest = job.ledger().manifest();
            let word = match job.forked() {
                Ok(Some(_)) => "frozen",
                Ok(None) => view_word(&job.view(self.id)),
                Err(_) => "unreadable",
            };
            lines.push(format!(
                "{} {word} epoch {} {}",
                job.id(),
                manifest.lease.epoch(),
                summary(&manifest.goal)
            ));
        }
        for (id, error) in self.broken_jobs() {
            lines.push(format!("{id} closed: {error}"));
        }
        if lines.is_empty() {
            out.say("no jobs; make one with `fabric job new --repo <path> --goal <text>`");
            return;
        }
        let (left_out, kept) = latest_within(lines, budget);
        if left_out > 0 {
            out.say(format!(
                "the {left_out} oldest jobs are left out; the latest {} fill one answer, and `fabric job status <job>` shows any job",
                kept.len()
            ));
        }
        out.lines.extend(kept);
    }

    fn broken_jobs(&self) -> Vec<(JobId, String)> {
        self.broken
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(id, e)| (*id, e.clone()))
            .collect()
    }

    async fn job_status(self: &Arc<Self>, id: JobId, out: &mut Out) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        let manifest = job.ledger().manifest();
        out.say(format!("job {id}"));
        out.say(format!("goal: {}", manifest.goal));
        out.say(format!("harness: {}", manifest.harness));
        let owner = manifest.lease.holder();
        out.say(format!(
            "owner: {owner}{} at epoch {}",
            if owner == self.id { " (this node)" } else { "" },
            manifest.lease.epoch()
        ));
        let forked = job.forked()?;
        out.say(match (job.view(self.id), &forked) {
            (_, Some(fork)) => format!(
                "this node: frozen; {}'s ledger at epoch {} does not continue this node's, so the job was stolen while this node ran it. This copy never runs again; run `fabric job resume {id}` on {} to continue there",
                fork.peer, fork.epoch, fork.peer
            ),
            (LeaseView::Owner(_), None) => "this node: owner".to_owned(),
            (LeaseView::Departing { move_id, target }, None) => format!(
                "this node: departing to {target} under move {move_id}; frozen until {target} answers. Run `fabric job move {id} --to {target}` to ask again, or `fabric job steal {id} --force` on another node"
            ),
            (LeaseView::Departed { owner, epoch }, None) => format!(
                "this node: departed; {owner} owns the job at epoch {epoch} and this copy never runs"
            ),
            (LeaseView::Replica { owner, .. }, None) => {
                format!("this node: replica; run `fabric job resume {id}` to pull it from {owner}")
            }
        });
        out.say(match (manifest.latest_snapshot, job.suspended_in()) {
            (None, _) => "snapshot: none yet".to_owned(),
            (Some(s), Some(_)) => format!("snapshot: {s} (suspended in it)"),
            (Some(s), None) => format!("snapshot: {s} (resumed since)"),
        });
        out.say(match job.marker()? {
            None => "workspace: none on this node".to_owned(),
            Some(marker) => workspace_line(&job, self.id, &marker, forked.is_some()).await,
        });
        let running = match job.harness_record()? {
            Some(record) => match record.liveness().await {
                Liveness::Leader => format!(
                    "harness: running as pid {} since epoch {}",
                    record.pid, record.epoch
                ),
                Liveness::Group => format!(
                    "harness: pid {} exited and processes it started still run in its group; `fabric job suspend {id}` stops them",
                    record.pid
                ),
                Liveness::Gone => "harness: not running".to_owned(),
                Liveness::Unknown => format!(
                    "harness: pid {} leads a process group and its start time could not be read, so whether it is still the harness is not known",
                    record.pid
                ),
            },
            None => "harness: not running".to_owned(),
        };
        out.say(running);
        out.say(format!("records: {}", manifest.event_count));
        Ok(())
    }

    /// Prints the job's records, the latest ones when all of them would not
    /// fit in one control answer.
    async fn job_log(&self, id: JobId, out: &mut Out) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        let lines: Vec<String> = job
            .ledger()
            .records()
            .iter()
            .enumerate()
            .map(|(n, record)| {
                let event = record.event();
                format!(
                    "{n:>4} epoch {} at {} {}",
                    event.epoch,
                    event.at,
                    describe(&event.kind)
                )
            })
            .collect();
        // A record is at most 1 MiB, so the latest one always fits.
        let (start, kept) = latest_within(lines, LINES_BUDGET);
        if start > 0 {
            out.say(format!(
                "the first {start} records are left out; the latest {} fill one answer",
                kept.len()
            ));
        }
        out.lines.extend(kept);
        Ok(())
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
                    what: "a suspend",
                    command: "suspend",
                });
            }
            MarkerState::Restoring { .. } => {
                return Err(NodeError::Unfinished {
                    job: id,
                    what: "a restore",
                    command: "resume",
                });
            }
        }
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid.get(),
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
    /// the job at the plan's epoch, records a harness that still runs, or
    /// holds records past the head the plan was made at.
    ///
    /// The CLI asks under the tree lock, before any setup step. A launch of
    /// the same workspace registers its harness, appending `Resumed`, before
    /// it lets the lock go, so a plan this passes was not overtaken by one,
    /// even by one whose harness already exited.
    pub(crate) async fn launch_check(
        &self,
        id: JobId,
        epoch: Epoch,
        head: RecordHash,
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
                pid: record.pid.get(),
            });
        }
        if job.ledger().head() != head {
            return Err(NodeError::PlanOvertaken { job: id });
        }
        Ok(())
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
        out: &mut Out,
    ) -> Result<(), NodeError> {
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
                pid: record.pid.get(),
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
        // The session is recorded as pending before `Resumed` and moved into
        // place after it, so a crash between the two does not leave a
        // `Resumed` whose session a later capture would miss.
        let record = session.map(|session| SessionRecord {
            harness: harness.clone(),
            session,
        });
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
        if let Some(marker) = job.marker()? {
            job.set_marker(&Marker {
                root: marker.root,
                state: MarkerState::Live,
                epoch,
            })?;
        }
        out.say(format!(
            "Resumed recorded at epoch {epoch}; {harness} runs as pid {pid}"
        ));
        Ok(())
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
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<SnapshotId, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        self.suspend_locked(&mut job, path_var, out).await
    }

    pub(crate) async fn suspend_locked(
        &self,
        job: &mut Job,
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<SnapshotId, NodeError> {
        let id = job.id();
        let epoch = owner_token(job, self.id)?.epoch;
        let Some(marker) = job.reconcile_marker()? else {
            return job.suspended_in().ok_or(NodeError::NoWorkspace(id));
        };
        if !marker.is_current(epoch, job.latest()).await {
            out.say(format!(
                "the workspace at {} was left at epoch {}, before this node's tenure, so it is not captured; `fabric job resume {id}` sets it aside",
                marker.root.display(),
                marker.epoch
            ));
            return job.suspended_in().ok_or(NodeError::NoWorkspace(id));
        }
        if let (MarkerState::Clean { snapshot, .. }, Some(suspended)) =
            (&marker.state, job.suspended_in())
            && *snapshot == suspended
        {
            if marker.is_unchanged().await {
                out.say(format!("already suspended in snapshot {suspended}"));
                return Ok(suspended);
            }
            out.say(format!(
                "the workspace changed after snapshot {suspended}, by hand or by a harness Fabric did not start; capturing it again"
            ));
        }
        if let MarkerState::Restoring { .. } = marker.state {
            return Err(NodeError::Unfinished {
                job: id,
                what: "a restore",
                command: "resume",
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
        let _tree = self.lock_tree(id, &marker.root)?;
        if let Some(record) = job.harness_record()? {
            if record.liveness().await != Liveness::Gone {
                out.say(format!("stopping the harness, pid {}", record.pid));
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
            out.say(format!("finishing the suspend that published {snapshot}"));
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
            let snapshot = self.capture(job, &marker.root, path_var, out).await?;
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
        self.confirm_departure(job, path_var, out).await?;
        job.append(self.id, EventKind::Suspended { snapshot })?;
        job.set_marker(&Marker {
            root: marker.root,
            state: MarkerState::Clean { snapshot, tree },
            epoch,
        })?;
        out.say(format!(
            "suspended in snapshot {snapshot}; this node keeps the lease at epoch {}",
            job.ledger().lease().epoch()
        ));
        Ok(snapshot)
    }

    /// Takes the lock `fabric job run` and `resume` hold over the tree at
    /// `root` while they run setup steps and launch; `None` when the root is
    /// gone, since nothing can run setup in it.
    fn lock_tree(&self, job: JobId, root: &Path) -> Result<Option<TreeLock>, NodeError> {
        if !root.exists() {
            return Ok(None);
        }
        match TreeLock::acquire(&self.cfg.state.tree_locks(), root) {
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
        path_var: &OsString,
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
        let harness = adapter(&record.harness, &self.harness_env(path_var)?)?;
        match harness.confirm_departure(&manifest).await {
            Ok(()) => Ok(()),
            Err(HarnessError::DepartureNotPending { session }) => {
                out.say(format!(
                    "no departure record waits for session {session}; its rewritable files stay this node's"
                ));
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
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<SnapshotId, NodeError> {
        let state_dir = job.dir().harness_state();
        let mut harness_state = None;
        if let Some(record) = job_session(job, root, &self.cfg.home)? {
            let session = record.session;
            let harness = adapter(&record.harness, &self.harness_env(path_var)?)?;
            job.clear_harness_state()?;
            std::fs::create_dir_all(&state_dir).map_err(|e| NodeError::io(&state_dir, e))?;
            let manifest = harness.capture(root, &session, &state_dir).await?;
            out.say(format!(
                "captured {} session {} ({} files)",
                record.harness,
                session,
                manifest.files().len()
            ));
            harness_state = Some(state_dir);
        }
        let facts = NodeFacts::measure(&self.cfg.home, path_var, self.cfg.cargo_home.clone()).await;
        // A workspace whose environment does not detect here would not
        // materialize on the target either, so the snapshot is refused
        // before it is published or moved.
        // A pattern the job ever added stays: removing its line from
        // `.fabric/env.toml` does not remove it (ADR 0007, Secrets).
        let recorded: Vec<fabric_core::secret::SecretPattern> =
            local::read(&job.dir().secret_patterns())?.unwrap_or_default();
        let detection = fabric_env::detect::detect(root, &recorded)?;
        local::write(
            &job.dir().secret_patterns(),
            &detection.spec().secret_patterns,
        )?;
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
        let parent = job.latest();
        let (snapshot, report) = with_store(&self.store, move |store| {
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
        out.say(format!(
            "snapshot {snapshot}: {} files, {} bytes, {} new objects",
            report.files, report.bytes, report.new_objects
        ));
        report_capture(&report, out);
        Ok(snapshot)
    }

    /// Pushes the ledger, and through it the latest snapshot, to every paired peer it can reach.
    pub(crate) async fn replicate(self: &Arc<Self>, id: JobId, out: &mut Out) {
        // A peer behind a NAT has no address here and is reached over the
        // session it opened, as every other request reaches it.
        for peer in self.reachable_peers() {
            match self.push_records(id, peer).await {
                Ok(()) => out.say(format!("copied the ledger and the snapshot to {peer}")),
                Err(e) => out.say(format!("not copied to {peer}: {e}")),
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
                    return Err(NodeError::Peer {
                        peer,
                        reason: format!(
                            "its ledger does not continue this one (its head is {head}); the chains forked"
                        ),
                    });
                }
                Fenced::StaleEpoch { current, .. } => {
                    return Err(NodeError::Peer {
                        peer,
                        reason: format!(
                            "it knows epoch {current}, newer than this node's; run `fabric job resume {id}` to catch up"
                        ),
                    });
                }
                Fenced::AheadOfUs { current, .. } => {
                    return Err(NodeError::Peer {
                        peer,
                        reason: format!(
                            "it is at epoch {current}, older than this node's, and must catch up first"
                        ),
                    });
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
    async fn job_move(
        self: &Arc<Self>,
        id: JobId,
        to: NodeId,
        force: bool,
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        if to == self.id {
            return Err(fabric_core::ledger::LedgerError::MoveToSelf.into());
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
                out.say(format!(
                    "move {move_id} toward {to} is open; offering it again"
                ));
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
                out.say(format!("{to} already owns the job at epoch {epoch}"));
                return Ok(());
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
        self.preflight(
            to,
            Some((&harness, &root)),
            Target::Peer,
            path_var,
            force,
            out,
        )
        .await?;

        if matches!(view, LeaseView::Owner(_)) {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            self.suspend_locked(&mut job, path_var, out).await?;
        }
        self.push_records(id, to).await?;
        out.say(format!("{to} holds the ledger and the snapshot"));

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
            out.say(format!(
                "MoveStarted {move_id}: this copy does not run until {to} decides"
            ));
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
    ) -> Result<(), NodeError> {
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
                    self.apply_decision(id, to, record.as_bytes(), out).await?;
                    if twice {
                        self.apply_decision(id, to, record.as_bytes(), out).await?;
                    }
                    return Ok(());
                }
                Ok(Asked::Status(Fenced::Answer(MoveState::Undecided { .. }))) => {
                    let answer = self
                        .call(to, |session| async move {
                            session.offer(epoch, offer, MOVE_DEADLINE).await
                        })
                        .await;
                    last_error = match answer {
                        Ok(Fenced::Answer(Offered::Decided { record })) => {
                            self.apply_decision(id, to, record.as_bytes(), out).await?;
                            return Ok(());
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
                    return Err(NodeError::Peer {
                        peer: to,
                        reason: format!("it knows epoch {current}; the move cannot complete"),
                    });
                }
                Ok(
                    Asked::Offer(Fenced::AheadOfUs { current, .. })
                    | Asked::Status(Fenced::AheadOfUs { current, .. }),
                ) => {
                    last_error = format!("{to} is at epoch {current} and has not caught up");
                }
                Err(e) => {
                    out.say(format!(
                        "no answer from {to} ({e}); asking for its decision"
                    ));
                    last_error = e.to_string();
                }
            }
        }
        out.say(format!("last error: {last_error}"));
        Err(NodeError::Departing {
            job: id,
            move_id: offer.move_id,
            target: to,
        })
    }

    /// Appends the target's decision for an open move; a decision already held changes nothing.
    async fn apply_decision(
        self: &Arc<Self>,
        id: JobId,
        from: NodeId,
        bytes: &[u8],
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let grew = job.accept(bytes, from, self.id)?;
        if grew {
            match job.view(self.id) {
                LeaseView::Departed { owner, epoch } => {
                    out.say(format!(
                        "{owner} imported the job and owns it at epoch {epoch}"
                    ));
                }
                LeaseView::Owner(token) => {
                    out.say(format!(
                        "{from} declined the move; this node owns the job again at epoch {}",
                        token.epoch
                    ));
                }
                other => out.say(format!(
                    "the decision left this node as {}",
                    view_word(&other)
                )),
            }
        }
        self.settle(&mut job, out).await
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
            Ok(Fenced::Answer(MoveState::Decided { record })) => {
                self.apply_decision(id, target, record.as_bytes(), out)
                    .await
            }
            Ok(_) => {
                out.say(format!("move {move_id} toward {target} is still undecided"));
                Ok(())
            }
            Err(e) => {
                out.say(format!(
                    "move {move_id} toward {target} is open and {target} did not answer: {e}"
                ));
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
                    out.say(format!(
                        "fenced: epoch {} is over for this node; stopping harness pid {}",
                        record.epoch, record.pid
                    ));
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
                out.say(format!(
                    "this node's copy moved out of the root to {}; it is kept, never run",
                    dest.display()
                ));
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
        path_var: &OsString,
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
                path_var,
                force,
                out,
            )
            .await?;
            // With no harness named, the job's own launches, and the pull
            // must still find the one the preflight checked.
            self.pull(id, holder, harness.is_none().then_some(&kind), out)
                .await?;
        }
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        let root = self
            .restore_if_needed(&mut job, token.epoch, path_var, out)
            .await?;
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid.get(),
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
    /// the move is then aborted, and the holder keeps the job.
    async fn pull(
        self: &Arc<Self>,
        id: JobId,
        holder: NodeId,
        checked: Option<&HarnessKind>,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        out.say(format!("pulling job {id} from {holder}"));
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
            fenced => {
                return Err(NodeError::Peer {
                    peer: holder,
                    reason: format!("it did not start a move: {}", fenced_words(&fenced)),
                });
            }
        };
        out.say(format!("{holder} started move {move_id} toward this node"));
        self.pull_records(id, holder).await?;
        self.ensure_latest(id, holder).await?;
        faults::hit(Point::BeforeImport).await;
        let changed = {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            // The records hold the holder's `MoveStarted`, after which it
            // runs nothing, so the harness they name is the one to launch.
            // A move this node already decided is left to `import`.
            if let Some(checked) = checked
                && job.harness() != checked
                && job.ledger().decision_for(move_id).is_none()
            {
                let change = (checked.clone(), job.harness().clone());
                if let AbortAnswer::Abort(record) = job.ledger().answer_abort(
                    self.id,
                    move_id,
                    clock::event_id(),
                    clock::now_ms(),
                )? {
                    job.commit(record, Author::Node(self.id))?;
                }
                Some(change)
            } else {
                self.import(&mut job, holder, move_id, out)?;
                None
            }
        };
        if let Some((checked, now)) = changed {
            if let Err(e) = self.push_records(id, holder).await {
                out.say(format!(
                    "{holder} has not seen the abort yet ({e}); it will ask for the decision"
                ));
            }
            return Err(NodeError::HarnessChangedDuringPull {
                job: id,
                holder,
                checked: Box::new(checked),
                now: Box::new(now),
            });
        }
        // The holder learns the decision now, or from MoveStatus later.
        if let Err(e) = self.push_records(id, holder).await {
            out.say(format!(
                "{holder} has not seen the import yet ({e}); it will ask for the decision"
            ));
        }
        Ok(())
    }

    /// Imports the open move `move_id` from `holder` into `job`, whose lock the caller holds.
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
                return Err(NodeError::Peer {
                    peer: holder,
                    reason: format!(
                        "its move does not link to this node's head {head}; run the resume again"
                    ),
                });
            }
            OfferAnswer::SnapshotIncomplete { snapshot } => {
                return Err(NodeError::Peer {
                    peer: holder,
                    reason: format!(
                        "snapshot {snapshot} did not arrive whole; run the resume again"
                    ),
                });
            }
            OfferAnswer::Stale { current, .. } => {
                return Err(NodeError::Peer {
                    peer: holder,
                    reason: format!("the move is at an epoch behind {current}"),
                });
            }
        }
        out.say(format!(
            "imported move {move_id}; this node owns the job at epoch {}",
            job.ledger().lease().epoch()
        ));
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
        path_var: &OsString,
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
                out.say(format!("workspace already at {}", root.display()));
                return Ok(root.clone());
            }
            Some(Marker {
                state: MarkerState::Suspending { .. },
                ..
            }) => {
                return Err(NodeError::Unfinished {
                    job: id,
                    what: "a suspend",
                    command: "suspend",
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
        out.say(choice.to_string());
        let root = choice.root().to_owned();
        check_root(
            &root,
            &self.cfg.home,
            synced_folders(&self.cfg.home, path_var).await,
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
            self.write_snapshot(
                snapshot,
                &manifest.caches,
                &root,
                harness_dir,
                path_var,
                out,
            )
            .await?;
        }
        if has_harness_state {
            self.restore_harness(job, &root, path_var, out).await?;
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
                out.say(format!(
                    "the clean workspace at {} is gone; restoring the latest snapshot",
                    root.display()
                ));
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
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let facts = NodeFacts::measure(&self.cfg.home, path_var, self.cfg.cargo_home.clone()).await;
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
        out.say(format!(
            "restored snapshot {snapshot}: {} files, {} bytes",
            report.files, report.bytes
        ));
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
            out.say(format!(
                "the workspace at {} was left at epoch {}, before this node's tenure at epoch {epoch}, and may differ from the job's latest snapshot; it moved to {}, kept, never run",
                marker.root.display(),
                marker.epoch,
                dest.display()
            ));
        }
        job.forget_session()?;
        job.clear_marker()
    }

    /// Places the captured harness state where its harness expects it and records the session.
    async fn restore_harness(
        &self,
        job: &Job,
        root: &Path,
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let state_dir = job.dir().harness_state();
        let state = StateManifest::read(&state_dir)?;
        let harness = adapter(state.kind(), &self.harness_env(path_var)?)?;
        harness.restore(root, &state_dir).await?;
        job.set_session(&SessionRecord {
            harness: state.kind().clone(),
            session: state.session().clone(),
        })?;
        out.say(format!(
            "restored {} session {}",
            state.kind(),
            state.session()
        ));
        Ok(())
    }

    async fn job_steal(
        self: &Arc<Self>,
        id: JobId,
        reason: String,
        force: bool,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        // One hold of the job's lock from the fork check to the commit: a
        // catch-up waiting for it could otherwise freeze the job, or move
        // it to another owner, after the check and before the steal.
        let owner = {
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
            let proposal = job.ledger().steal(
                self.id,
                reason,
                complete,
                clock::event_id(),
                clock::now_ms(),
            )?;
            let warning = format!(
                "{}; the owner's last record is {}",
                proposal.warning,
                age(clock::now_ms().saturating_sub(proposal.warning.last_record_at))
            );
            if !force {
                return Err(NodeError::StealNeedsForce(format!(
                    "a steal would bump the epoch without {owner}'s consent: {warning}. Pass --force to do it"
                )));
            }
            out.say(format!("warning: {warning}"));
            job.commit(proposal.record, Author::Node(self.id))?;
            job.append(self.id, EventKind::Note { text: warning })?;
            out.say(format!(
                "this node owns the job at epoch {}; run `fabric job resume {id}` to restore and launch it here",
                job.ledger().lease().epoch()
            ));
            self.settle(&mut job, out).await?;
            owner
        };
        if let Err(e) = self.push_records(id, owner).await {
            out.say(format!("{owner} has not seen the steal yet: {e}"));
        }
        Ok(())
    }

    async fn job_decide(
        &self,
        id: JobId,
        decision: Decision,
        note: &str,
        out: &mut Out,
    ) -> Result<(), NodeError> {
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
        out.say(format!("recorded: {text}"));
        Ok(())
    }

    async fn doctor(
        self: &Arc<Self>,
        peer: Option<NodeId>,
        job: Option<JobId>,
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let Some(peer) = peer else {
            let facts =
                NodeFacts::measure(&self.cfg.home, path_var, self.cfg.cargo_home.clone()).await;
            for (name, value) in facts.iter() {
                out.say(format!("{name}: {}", value.replace('\n', " / ")));
            }
            return Ok(());
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
        self.preflight(
            peer,
            harness.as_ref().zip(root.as_deref()),
            Target::Peer,
            path_var,
            false,
            out,
        )
        .await
    }

    /// Compares this node's facts with `peer`'s and stops on a refusal `force` does not waive.
    async fn preflight(
        self: &Arc<Self>,
        peer: NodeId,
        job: Option<(&HarnessKind, &Path)>,
        target: Target,
        path_var: &OsString,
        force: bool,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let mut here =
            NodeFacts::measure(&self.cfg.home, path_var, self.cfg.cargo_home.clone()).await;
        // The agent of an ACP job is named per job, so only the target of a
        // pull, this node, can look for it.
        if let (Target::Here, Some((HarnessKind::Acp { program, .. }, _))) = (target, job) {
            here.measure_acp(program.as_str(), path_var, &self.harness_config);
        }
        // A session the peer opened is used too: a peer behind a NAT has
        // no address here and is reachable only over the session it dialed.
        let (theirs, clock_skew_ms, wire) = self
            .call(peer, |session| async move {
                let facts = session.facts(SHORT_DEADLINE * 4).await?;
                Ok((facts, session.clock_skew_ms(), session.kind().to_string()))
            })
            .await?;
        let theirs = NodeFacts::from_wire(&theirs);
        let rows = doctor::compare(
            &here,
            &theirs,
            &Context {
                harness: job.map(|(h, _)| h),
                root: job.map(|(_, r)| r),
                target,
                clock_skew_ms: Some(clock_skew_ms),
                wire: Some(wire),
            },
        );
        for row in &rows {
            out.say(row.to_string());
        }
        let blocking = doctor::blocking(&rows, force);
        if blocking.is_empty() {
            Ok(())
        } else {
            let facts: Vec<&str> = blocking.iter().map(|r| r.fact).collect();
            Err(NodeError::Preflight(facts.join(", ")))
        }
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

/// The job's session: the one recorded for the harness the job runs,
/// else, when that harness is Codex, the thread the job's workspace holds,
/// else the record of the harness it ran before, if any.
///
/// A Codex launched without a thread to resume makes one only once it
/// runs, after its launch was recorded, so no launch can name it. Its
/// rollout records the workspace it started in as its `cwd`, and the
/// workspace is the job's own (`paths::job_root`), so the newest thread
/// with that `cwd` is the job's; it is recorded here, as a launch that
/// named it would have been. Without it, a suspend would carry the
/// workspace and leave the conversation behind, or carry the state of the
/// harness the job ran before it switched to Codex.
fn job_session(job: &Job, root: &Path, home: &Path) -> Result<Option<SessionRecord>, NodeError> {
    let recorded = job.session()?;
    let harness = job.harness();
    if *harness != HarnessKind::Codex
        || recorded
            .as_ref()
            .is_some_and(|record| record.harness == *harness)
    {
        return Ok(recorded);
    }
    let env = HarnessEnv::new(home, OsString::new(), HarnessConfig::default())?;
    let Some(thread) = Codex::new(env.home, env.path_var).latest_session(root)? else {
        return Ok(recorded);
    };
    let record = SessionRecord {
        harness: HarnessKind::Codex,
        session: thread,
    };
    local::write(&job.dir().session(), &record)?;
    Ok(Some(record))
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
    let session = match job_session(job, &root, home)? {
        Some(record) if record.harness == harness => {
            out.say(format!("continuing {harness} session {}", record.session));
            Some(record.session)
        }
        Some(record) => {
            out.say(format!(
                "starting a new {harness} session; the {} session {} stays in the snapshot (cross-harness resume is an experiment, ADR 0006)",
                record.harness, record.session
            ));
            None
        }
        None => {
            out.say(format!("starting a new {harness} session"));
            None
        }
    };
    Ok(RunPlan {
        job: job.id(),
        epoch,
        head: job.ledger().head(),
        root,
        harness_config: harness_config.for_harness(&harness),
        harness,
        session,
        goal: job.ledger().manifest().goal.clone(),
        home: home.to_owned(),
    })
}

/// Says, one line per cache, whether its bytes are written here or rebuilt,
/// and why (ADR 0008); past `REPORT_PATHS` caches it counts the rest, so a
/// resume's answer stays within one control message.
fn report_caches(caches: &[fabric_core::snapshot::CacheEntry], facts: &NodeFacts, out: &mut Out) {
    for cache in caches.iter().take(REPORT_PATHS) {
        let mine = facts.key_like(&cache.key);
        if cache.key.matches(&mine) && cache.carry == fabric_core::snapshot::CarryState::Carried {
            out.say(format!("{}: carried, the keys match", cache.path));
        } else {
            out.say(format!(
                "{}: rebuilt here, {}",
                cache.path,
                key_difference(&cache.key, &mine)
            ));
        }
    }
    if caches.len() > REPORT_PATHS {
        out.say(format!(
            "{} more caches, each carried where the keys match and rebuilt here otherwise",
            caches.len() - REPORT_PATHS
        ));
    }
}

/// The longest goal summary `job ls` prints, in characters.
const SUMMARY_CHARS: usize = 72;

/// A goal's first line, cut to `SUMMARY_CHARS`, so that a list of goals
/// up to the record bound each still fits in one control answer.
/// The last of `lines` that fit in `budget` bytes together, in order, and
/// how many before them were left out.
fn latest_within(mut lines: Vec<String>, budget: usize) -> (usize, Vec<String>) {
    let mut start = lines.len();
    let mut bytes = 0;
    while let Some(line) = start.checked_sub(1).map(|i| &lines[i]) {
        if bytes + line.len() > budget {
            break;
        }
        bytes += line.len();
        start -= 1;
    }
    let kept = lines.split_off(start);
    (start, kept)
}

fn summary(goal: &str) -> String {
    let first = goal.lines().next().unwrap_or_default();
    match first.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}...", &first[..cut]),
        None if first.len() < goal.len() => format!("{first} ..."),
        None => first.to_owned(),
    }
}

/// How many paths or keys of one kind a suspend names before it counts the rest.
const REPORT_PATHS: usize = 20;

/// Says, one line per path or config key, what the snapshot carries that
/// a secret pattern matches, what of the workspace and the harness state it
/// leaves out, and what of the repository's config it changed or left out.
fn report_capture(report: &fabric_store::capture::CaptureReport, out: &mut Out) {
    let denied: Vec<_> = report
        .denied
        .iter()
        .chain(&report.denied_untracked)
        .collect();
    say_each(out, "left behind (secret pattern)", &denied);
    say_each(
        out,
        "left behind (secret pattern, harness state)",
        &report.denied_harness,
    );
    say_each(
        out,
        "travels in the snapshot though a secret pattern matches it (tracked)",
        &report.tracked_secrets,
    );
    say_each(
        out,
        "travels in the snapshot though a secret pattern matches it (stashed)",
        &report.stashed_secrets,
    );
    say_each(
        out,
        "travels as its clean filter's output, as a fresh clone has it (git attributes)",
        &report.filtered,
    );
    say_each(
        out,
        "left behind (not a file, link or directory)",
        &report.special_files,
    );
    say_each(
        out,
        "left behind (not a file, link or directory, harness state)",
        &report.special_harness,
    );
    // In a repository the whole nested tree stays; in a plain directory
    // its files travel and only its `.git` stays.
    let nested = if report.bundle.is_some() {
        "left behind (nested repository)"
    } else {
        "left behind (the .git of a nested repository)"
    };
    say_each(out, nested, &report.nested_repositories);
    say_each(
        out,
        "left behind (the .git of a nested repository, harness state)",
        &report.nested_harness,
    );
    say_each(
        out,
        "travels without its user info (git config)",
        &report.stripped_userinfo,
    );
    say_each(out, "left behind (git config)", &report.dropped_config);
}

/// One line per item under `words`, the first `REPORT_PATHS` of them, then
/// one that counts the rest, so a suspend's answer stays within one control
/// message however many paths a workspace holds.
fn say_each<T: std::fmt::Display>(out: &mut Out, words: &str, items: &[T]) {
    for item in items.iter().take(REPORT_PATHS) {
        out.say(format!("{words}: {item}"));
    }
    if items.len() > REPORT_PATHS {
        out.say(format!("{words}: {} more", items.len() - REPORT_PATHS));
    }
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

/// Reads which folders iCloud Drive syncs on this node; off everywhere `defaults` does not exist.
async fn synced_folders(home: &Path, path_var: &OsString) -> SyncedFolders {
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

/// How long ago something happened, `ms` milliseconds back, in the largest whole unit.
fn age(ms: u64) -> String {
    let minutes = ms / 60_000;
    match minutes {
        0 => "less than a minute old".to_owned(),
        1..60 => format!("{minutes} min old"),
        60..2880 => format!("{} h old", minutes / 60),
        _ => format!("{} days old", minutes / 1440),
    }
}

/// What `status` says about the workspace `marker` names, from the marker
/// and how this node relates to the job together.
async fn workspace_line(job: &Job, me: NodeId, marker: &Marker, frozen: bool) -> String {
    let root = marker.root.display();
    match job.view(me) {
        LeaseView::Owner(token) if !frozen => {
            if marker.is_current(token.epoch, job.latest()).await {
                format!("workspace: {root} ({})", marker_word(&marker.state))
            } else {
                format!(
                    "workspace: {root} was left at epoch {}, before this node's tenure; `fabric job resume {}` sets it aside and restores the latest snapshot",
                    marker.epoch,
                    job.id()
                )
            }
        }
        LeaseView::Departing { target, .. } if !frozen && marker.is_unchanged().await => {
            format!(
                "workspace: {root} is clean at the snapshot offered to {target} and waits for its decision"
            )
        }
        _ if !frozen && marker.is_unchanged().await => format!(
            "workspace: {root} is clean at a published snapshot; this node never runs it and moves it aside when it next settles the job"
        ),
        _ => format!(
            "workspace: {root} is an orphan: it may hold changes made after the last snapshot this node published, and nothing here deletes it"
        ),
    }
}

fn view_word(view: &LeaseView) -> &'static str {
    match view {
        LeaseView::Owner(_) => "owner",
        LeaseView::Departing { .. } => "departing",
        LeaseView::Departed { .. } => "departed",
        LeaseView::Replica { .. } => "replica",
    }
}

fn marker_word(state: &MarkerState) -> String {
    match state {
        MarkerState::Live => {
            "live: the harness may have changed it since the last snapshot".to_owned()
        }
        MarkerState::Suspending { .. } => "a suspend did not finish".to_owned(),
        MarkerState::Clean { snapshot, .. } => format!("clean at snapshot {snapshot}"),
        MarkerState::Restoring { snapshot } => format!("a restore of {snapshot} did not finish"),
    }
}

fn describe(kind: &EventKind) -> String {
    match kind {
        EventKind::Created {
            goal,
            harness,
            created_by,
            ..
        } => format!("Created by {created_by} under {harness}: {goal}"),
        EventKind::MoveStarted { move_id, target } => {
            format!("MoveStarted {move_id} toward {target}")
        }
        EventKind::MoveImported { move_id, from } => format!("MoveImported {move_id} from {from}"),
        EventKind::MoveAborted { move_id } => format!("MoveAborted {move_id}"),
        EventKind::Stolen { by, from, reason } => format!("Stolen by {by} from {from}: {reason}"),
        EventKind::SnapshotPublished { snapshot, .. } => format!("SnapshotPublished {snapshot}"),
        EventKind::Suspended { snapshot } => format!("Suspended in {snapshot}"),
        EventKind::Resumed { node, harness, .. } => format!("Resumed on {node} under {harness}"),
        EventKind::EnvSpecChanged { env_spec } => format!("EnvSpecChanged {env_spec}"),
        EventKind::DecisionRecorded { text } => format!("DecisionRecorded: {text}"),
        EventKind::Note { text } => format!("Note: {text}"),
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
            path_var: std::env::var_os("PATH").unwrap_or_default(),
            cargo_home: None,
        })
        .unwrap();
        (dir, node)
    }

    /// A job made from a one-commit repository, owned by `node` at epoch 1.
    pub(crate) async fn job(dir: &Path, node: &Arc<Node>) -> JobId {
        let repo = repo(dir);
        let mut out = Out::default();
        node.job_new(&repo, "goal".to_owned(), HarnessKind::Codex, &mut out)
            .await
            .unwrap();
        out.lines[0].parse().unwrap()
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

    async fn register(node: &Arc<Node>, id: JobId, pid: NonZeroU32) -> Result<(), NodeError> {
        node.harness_started(
            id,
            Epoch::FIRST,
            pid,
            HarnessKind::Codex,
            None,
            &mut Out::default(),
        )
        .await
    }

    /// A `PATH` whose `codex` reports a version that takes the thread
    /// writer lock, so a capture never waits on a real `codex` running here.
    fn locking_codex(dir: &Path) -> OsString {
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
        std::env::join_paths([bin.as_path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap()
    }

    /// Writes the rollout a Codex started in the job's workspace makes, and
    /// returns its thread.
    fn codex_thread_in(node: &Node, id: JobId) -> SessionId {
        let root = std::fs::canonicalize(paths::job_root(&node.cfg.home, id)).unwrap();
        let thread = "019f233e-bb33-7342-85fa-f2d33b14368a";
        let day = node.cfg.home.join(".codex/sessions/2026/10/22");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join(format!("rollout-2026-10-22T10-00-00-{thread}.jsonl")),
            format!(
                "{{\"timestamp\":\"2026-10-22T10:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"timestamp\":\"2026-10-22T10:00:00.000Z\",\"cwd\":\"{}\",\"originator\":\"codex_cli_rs\",\"cli_version\":\"0.155.1\"}}}}\n",
                root.display()
            ),
        )
        .unwrap();
        SessionId::parse(thread).unwrap()
    }

    /// The session the job records and the one its last capture holds.
    async fn recorded_and_captured(node: &Node, id: JobId) -> (Option<SessionRecord>, SessionId) {
        let job = node.job(id).await.unwrap();
        let job = job.lock().await;
        let state = StateManifest::read(&job.dir().harness_state()).unwrap();
        (job.session().unwrap(), state.session().clone())
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
        };
        assert_eq!(
            recorded_and_captured(&node, id).await,
            (Some(record), thread)
        );
    }

    /// A job that switched from Claude to Codex still records the Claude
    /// session; the suspend carries the Codex thread, the conversation the
    /// job's harness now holds, not the other harness's state.
    #[tokio::test]
    async fn a_suspend_after_a_switch_to_codex_captures_the_codex_thread() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = locking_codex(dir.path());
        local::write(
            &node.cfg.state.job(id).session(),
            &SessionRecord {
                harness: HarnessKind::ClaudeCode,
                session: SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap(),
            },
        )
        .unwrap();
        let thread = codex_thread_in(&node, id);

        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let record = SessionRecord {
            harness: HarnessKind::Codex,
            session: thread.clone(),
        };
        assert_eq!(
            recorded_and_captured(&node, id).await,
            (Some(record), thread)
        );
    }

    /// Serves `a` and `b` and pairs them, each with the other's address.
    pub(crate) async fn paired(a: &Arc<Node>, b: &Arc<Node>) {
        for (node, peer) in [(a, b), (b, a)] {
            tokio::spawn(Arc::clone(node).run());
            let addr = peer.local_addr().unwrap().to_string();
            node.node_add(peer.id, Some(&addr), &mut Out::default())
                .await
                .unwrap();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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

        a.pull(id, b.id, None, out).await.unwrap();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        let out = &mut Out::default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, out).await.unwrap();
        b.pull_records(id, a.id).await.unwrap();
        b.ensure_latest(id, a.id).await.unwrap();
        b.job_steal(id, "a sleeps".to_owned(), true, out)
            .await
            .unwrap();
        // The steal reached a; this record, which b does not push, did not.
        b.job_decide(id, Decision::Approve, "", out).await.unwrap();
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
        b.node_ping(a.id, out).await.unwrap();
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
            node.node_add(peer.id, Some(&addr), &mut Out::default())
                .await
                .unwrap();
        }
        for node in [&a, &b, &c] {
            node.recovered().await;
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        b.job_decide(id, Decision::Approve, "before", out)
            .await
            .unwrap();
        let recovery = tokio::spawn(Arc::clone(&a).recover());
        wait_for(
            head(Arc::clone(&b)).await,
            "recovery did not catch up with b",
        )
        .await;

        // b writes a record a does not have, then begins a new session.
        b.job_decide(id, Decision::Approve, "after", out)
            .await
            .unwrap();
        let want = head(Arc::clone(&b)).await;
        let kept = b.open_session(a.id).unwrap();
        b.forget(a.id, &kept);
        b.node_ping(a.id, out).await.unwrap();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        b.job_decide(first, Decision::Approve, "before", out)
            .await
            .unwrap();
        let b_second = b.job(second).await.unwrap();
        let b_paused = b_second.lock().await;
        a.spawn_catch_up(b.id);
        wait_for(head(Arc::clone(&b)).await, "a did not catch up with b").await;

        // b writes a record a does not have, then begins a new session.
        b.job_decide(first, Decision::Approve, "after", out)
            .await
            .unwrap();
        let want = head(Arc::clone(&b)).await;
        let kept = b.open_session(a.id).unwrap();
        b.forget(a.id, &kept);
        b.node_ping(a.id, out).await.unwrap();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        assert_eq!(late.refusal(), crate::control::Refusal::Unreachable);
    }

    /// Users read status when a move is frozen; a clean copy waiting for
    /// the target must not be called an orphan with unsaved work.
    #[tokio::test]
    async fn status_calls_a_departing_clean_copy_clean() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = std::env::var_os("PATH").unwrap_or_default();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        node.depart(
            id,
            NodeId::from_bytes([7; 32]),
            &HarnessKind::Codex,
            &mut Out::default(),
        )
        .await
        .unwrap();
        let mut out = Out::default();
        node.job_status(id, &mut out).await.unwrap();
        let line = out
            .lines
            .iter()
            .find(|line| line.starts_with("workspace: "))
            .unwrap();
        assert!(line.contains("waits for its decision"), "{line}");
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
                matches!(&refused, Err(NodeError::Preflight(facts)) if facts.contains(login)),
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
            let facts: Vec<&str> = facts.split(", ").collect();
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

    /// A registration appends `Resumed` and then writes the session the
    /// harness started. A daemon that dies between the two, here a session
    /// file that cannot be written, must not lose that session: the next
    /// capture would leave the new conversation out of the snapshot.
    #[tokio::test]
    async fn a_session_whose_write_failed_after_resumed_is_kept() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let session_file = node.cfg.state.job(id).session();
        // A directory where the session goes makes its write fail.
        std::fs::create_dir_all(session_file.join("in-the-way")).unwrap();
        let mut harness = sleeper();
        let session = SessionId::parse("new-session").unwrap();
        let failed = node
            .harness_started(
                id,
                Epoch::FIRST,
                pid(&harness),
                HarnessKind::Codex,
                Some(session.clone()),
                &mut Out::default(),
            )
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
        std::fs::remove_dir_all(&session_file).unwrap();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        let snapshot = node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let manifest = with_store(&node.store, move |store| Ok(store.manifest(snapshot)?))
            .await
            .unwrap();
        let caches: Vec<&str> = manifest.caches.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(caches, [".output"], "{manifest:?}");
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        assert!(
            out.lines
                .iter()
                .any(|line| line == "left behind (secret pattern): users.sqlite"),
            "{:?}",
            out.lines
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        // A child another test forked while the lock was held keeps a copy
        // of its descriptor, and so the lock, until that child execs.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match node.suspend(id, &path, &mut Out::default()).await {
                Err(NodeError::SetupRunning { .. }) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                done => {
                    done.unwrap();
                    break;
                }
            }
        }
    }

    /// A suspend holds the tree lock from before it stops the harness
    /// until the snapshot is recorded, so a `fabric job run` that starts
    /// meanwhile refuses instead of running setup steps under the capture.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_suspend_holds_the_tree_lock_while_it_stops_the_harness() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let sent: RunPlan = postcard::from_bytes(&postcard::to_stdvec(&plan).unwrap()).unwrap();
        assert_eq!(sent.harness_config, node.harness_config);
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
        a.node_add(b.id, Some(&addr), &mut Out::default())
            .await
            .unwrap();
        b.node_add(a.id, None, &mut Out::default()).await.unwrap();
        a.node_ping(b.id, &mut Out::default()).await.unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut out = Out::default();
        // The two test homes differ, which the table refuses; what matters
        // is that the table was made from the peer's answer.
        let answered = b.doctor(Some(a.id), None, &path, &mut out).await;
        assert!(
            matches!(answered, Ok(()) | Err(NodeError::Preflight(_))),
            "{answered:?}"
        );
        assert!(
            out.lines
                .iter()
                .any(|line| line.contains(" network ") && line.contains("tcp+tls")),
            "{:?}",
            out.lines
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
            node.node_add(peer, addr.map(String::as_str), &mut Out::default())
                .await
                .unwrap();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
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

        a.job_decide(id, Decision::Approve, "a's own record", out)
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
        a.node_add(b.id, Some(&addr), &mut Out::default())
            .await
            .unwrap();
        b.node_add(a.id, None, &mut Out::default()).await.unwrap();
        a.node_ping(b.id, &mut Out::default()).await.unwrap();
        let id = job(dir_b.path(), &b).await;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut out = Out::default();
        b.suspend(id, &path, &mut out).await.unwrap();
        b.replicate(id, &mut out).await;
        assert!(a.job(id).await.is_ok(), "{:?}", out.lines);
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
            .job_new(
                dir.path(),
                "goal".to_owned(),
                secret.clone(),
                &mut Out::default(),
            )
            .await;
        assert!(
            matches!(refused, Err(NodeError::HarnessConfig(_))),
            "{refused:?}"
        );
        let mut sleeper = sleeper();
        let refused = node
            .harness_started(
                id,
                Epoch::FIRST,
                pid(&sleeper),
                secret,
                None,
                &mut Out::default(),
            )
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
    /// the latest records and a line that says how many were left out.
    #[tokio::test]
    async fn a_log_past_the_control_cap_answers_with_the_latest_records() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let long_text = "n".repeat(900 << 10);
        for _ in 0..5 {
            node.job_decide(id, Decision::Approve, &long_text, &mut Out::default())
                .await
                .unwrap();
        }
        let mut out = Out::default();
        node.job_log(id, &mut out).await.unwrap();
        let lines = out.lines;
        let answer = crate::control::Response::Done {
            lines: lines.clone(),
        };
        let encoded = postcard::to_stdvec(&answer).unwrap();
        assert!(encoded.len() <= crate::control::MAX_CONTROL_MESSAGE);
        assert!(
            lines[0].starts_with("the first 4 records are left out"),
            "{}",
            &lines[0]
        );
        assert!(lines.last().unwrap().starts_with("   5 epoch"));
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        std::fs::write(root.join(".fabric/env.toml"), "").unwrap();
        std::fs::write(root.join("users.sqlite"), "rows\n").unwrap();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        assert!(
            out.lines
                .iter()
                .any(|line| line == "left behind (secret pattern): users.sqlite"),
            "{:?}",
            out.lines
        );
    }

    #[test]
    fn an_age_reads_in_the_largest_whole_unit() {
        assert_eq!(age(59_000), "less than a minute old");
        assert_eq!(age(3 * 60_000), "3 min old");
        assert_eq!(age(3 * 3_600_000), "3 h old");
        assert_eq!(age(3 * 86_400_000), "3 days old");
    }

    #[tokio::test]
    async fn a_second_harness_is_refused_while_the_recorded_one_runs() {
        let (dir, node) = node();
        let id = job(dir.path(), &node).await;
        let (mut first, mut second) = (sleeper(), sleeper());
        register(&node, id, pid(&first)).await.unwrap();
        let refused = register(&node, id, pid(&second)).await;
        assert!(
            matches!(refused, Err(NodeError::AlreadyRunningHarness { pid: p, .. }) if p == first.id()),
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
        let refused = node
            .job_new(&repo, goal, HarnessKind::Codex, &mut Out::default())
            .await;
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        node.suspend(id, &path, &mut Out::default()).await.unwrap();
        let harness = sleeper();
        node.harness_started(
            id,
            Epoch::FIRST,
            pid(&harness),
            HarnessKind::ClaudeCode,
            None,
            &mut Out::default(),
        )
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let mut out = Out::default();
        node.job_new(&repo, "goal".to_owned(), HarnessKind::Codex, &mut out)
            .await
            .unwrap();
        let id: JobId = out.lines[0].parse().unwrap();
        let root = paths::job_root(&node.cfg.home, id);
        std::fs::create_dir(root.join("vendored")).unwrap();
        let made = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.join("vendored"))
            .status()
            .unwrap();
        assert!(made.success());
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        for wanted in [
            "travels in the snapshot though a secret pattern matches it (tracked): .env",
            "left behind (nested repository): vendored",
        ] {
            assert!(
                out.lines.iter().any(|line| line == wanted),
                "no line {wanted:?} in {:?}",
                out.lines
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
            node.job_new(&repo, goal.clone(), HarnessKind::Codex, &mut Out::default())
                .await
                .unwrap();
        }
        let mut out = Out::default();
        node.job_list(&mut out).await;
        assert_eq!(out.lines.len(), 5);
        let bytes: usize = out.lines.iter().map(String::len).sum();
        assert!(
            bytes < crate::control::MAX_CONTROL_MESSAGE,
            "{bytes} bytes of lines"
        );
        assert!(
            out.lines
                .iter()
                .all(|line| line.ends_with(" first line ...")),
            "{:?}",
            out.lines
        );
    }

    /// Each row of `job ls` is bounded, and enough jobs still add up past
    /// the control message cap. The answer keeps the latest rows that fit
    /// its budget and says how many older ones it left out.
    #[tokio::test]
    async fn a_job_list_past_its_budget_keeps_the_latest_rows() {
        let (dir, node) = node();
        let repo = repo(dir.path());
        let mut ids = Vec::new();
        for _ in 0..3 {
            let mut out = Out::default();
            node.job_new(&repo, "goal".to_owned(), HarnessKind::Codex, &mut out)
                .await
                .unwrap();
            ids.push(out.lines[0].clone());
        }
        // Rows are in id order, and an id begins with its time.
        ids.sort();
        let mut all = Out::default();
        node.job_list(&mut all).await;
        assert_eq!(all.lines.len(), 3, "{:?}", all.lines);
        let row = all.lines[2].len();
        let mut out = Out::default();
        node.job_list_within(2 * row, &mut out).await;
        assert_eq!(out.lines.len(), 3, "{:?}", out.lines);
        assert!(
            out.lines[0].starts_with("the 1 oldest jobs are left out"),
            "{:?}",
            out.lines
        );
        assert!(out.lines[1].starts_with(&ids[1]), "{:?}", out.lines);
        assert!(out.lines[2].starts_with(&ids[2]), "{:?}", out.lines);
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
        node.launch_check(id, first.epoch, first.head)
            .await
            .unwrap();
        let mut harness = sleeper();
        register(&node, id, pid(&harness)).await.unwrap();
        let refused = node.launch_check(id, second.epoch, second.head).await;
        assert!(
            matches!(refused, Err(NodeError::AlreadyRunningHarness { pid: p, .. }) if p == harness.id()),
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        let id = job(dir_a.path(), &a).await;
        a.suspend(id, &path, &mut Out::default()).await.unwrap();
        let harness = sleeper();
        a.harness_started(
            id,
            Epoch::FIRST,
            pid(&harness),
            HarnessKind::ClaudeCode,
            None,
            &mut Out::default(),
        )
        .await
        .unwrap();
        let reaper = std::thread::spawn(move || {
            let mut harness = harness;
            harness.wait()
        });
        let pulled = b
            .pull(id, a.id, Some(&HarnessKind::Codex), &mut Out::default())
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        for wanted in [
            "travels without its user info (git config): remote.origin.url",
            "left behind (git config): remote.tokened.url",
        ] {
            assert!(
                out.lines.iter().any(|line| line == wanted),
                "no line {wanted:?} in {:?}",
                out.lines
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
            .job_new(
                &repo,
                "goal".to_owned(),
                HarnessKind::Codex,
                &mut Out::default(),
            )
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
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut out = Out::default();
        node.suspend(id, &path, &mut out).await.unwrap();
        let named = out
            .lines
            .iter()
            .filter(|line| line.starts_with("left behind (secret pattern): .env."))
            .count();
        assert_eq!(named, REPORT_PATHS, "{:?}", out.lines.len());
        let rest = format!("left behind (secret pattern): {} more", 500 - REPORT_PATHS);
        assert!(out.lines.contains(&rest), "no line {rest:?}");
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
        node.launch_check(id, first.epoch, first.head)
            .await
            .unwrap();
        let mut harness = sleeper();
        let harness_pid = pid(&harness);
        register(&node, id, harness_pid).await.unwrap();
        harness.kill().unwrap();
        harness.wait().unwrap();
        node.harness_exited(id, harness_pid).await.unwrap();
        let refused = node.launch_check(id, second.epoch, second.head).await;
        assert!(
            matches!(refused, Err(NodeError::PlanOvertaken { .. })),
            "{refused:?}"
        );
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
        let path = std::env::var_os("PATH").unwrap_or_default();
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
    /// must stay within one control message, so the cache report names
    /// the first of them and counts the rest.
    #[test]
    fn a_resume_names_a_bounded_number_of_caches() {
        let caches: Vec<fabric_core::snapshot::CacheEntry> = (0..500)
            .map(|n| fabric_core::snapshot::CacheEntry {
                path: fabric_core::path::RelPath::parse(&format!("caches/{n}")).unwrap(),
                class: fabric_core::snapshot::CacheClass::ToolCache,
                carry: fabric_core::snapshot::CarryState::NotCarried,
                key: fabric_core::snapshot::CarryKey::Unmeasured,
            })
            .collect();
        let mut out = Out::default();
        report_caches(&caches, &NodeFacts::default(), &mut out);
        assert_eq!(out.lines.len(), REPORT_PATHS + 1, "{:?}", out.lines.last());
        assert_eq!(
            out.lines.last().map(String::as_str),
            Some("480 more caches, each carried where the keys match and rebuilt here otherwise")
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
            path_var: std::env::var_os("PATH").unwrap_or_default(),
            cargo_home: None,
        })
        .unwrap();
        let repo = repo(&root);
        for (harness, wanted) in [("acp:opencode", vec!["opencode"]), ("codex", vec![])] {
            let mut out = Out::default();
            node.job_new(
                &repo,
                "goal".to_owned(),
                crate::harnesses::parse_harness(harness).unwrap(),
                &mut out,
            )
            .await
            .unwrap();
            let id: JobId = out.lines[0].parse().unwrap();
            let plan = node.job_run(id, &mut Out::default()).await.unwrap();
            let carried: Vec<&str> = plan.harness_config.acp.keys().map(String::as_str).collect();
            assert_eq!(carried, wanted, "{harness}");
        }
    }
}
