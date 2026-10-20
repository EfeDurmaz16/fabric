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
use fabric_core::id::{Epoch, JobId, NodeId, SnapshotId};
use fabric_core::lease::{FenceError, FencingToken};
use fabric_core::ledger::{Author, EventKind, Ledger};
use fabric_core::moves::{CompleteSnapshot, LeaseView, MoveOffer, OfferAnswer};
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
    Durability, RestoreRequest, RestoreState, Verify, restore, restore_state,
};

use crate::clock;
use crate::control::{Request, RunPlan};
use crate::doctor::{self, Context, Target};
use crate::error::NodeError;
use crate::facts::{MeasuredKeys, NodeFacts, key_difference};
use crate::faults::{self, Action, Point};
use crate::harnesses::adapter;
use crate::job::Job;
use crate::local::{self, HarnessRecord, Liveness, Marker, MarkerState, SessionRecord};
use crate::node::{Node, Out, is_transport};
use crate::paths::{self, SyncedFolders, check_root, restore_root};
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
        if !matches!(
            request,
            Request::NodeId
                | Request::NodeAdd { .. }
                | Request::NodeList
                | Request::NodePing { .. }
                | Request::Doctor { .. }
        ) {
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
            Request::JobRun { job, harness } => {
                self.resolve_departing(job, out).await?;
                return Ok(Some(self.job_run(job, harness, out).await?));
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
            Request::JobDecide { job, approve, note } => {
                self.job_decide(job, approve, &note, out).await?;
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
                text.parse().map_err(|e| NodeError::Peer {
                    peer: node,
                    reason: format!("{text} is not host:port: {e}"),
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

    async fn node_ping(self: &Arc<Self>, node: NodeId, out: &mut Out) -> Result<(), NodeError> {
        let session = self.dial(node).await?;
        let started = tokio::time::Instant::now();
        session
            .ping(SHORT_DEADLINE)
            .await
            .map_err(|e| NodeError::Unreachable {
                peer: node,
                reason: e.to_string(),
            })?;
        out.say(format!(
            "{node} answered in {} ms over {}; its clock is {} ms from this one",
            started.elapsed().as_millis(),
            session.kind(),
            session.clock_skew_ms()
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
        let id = clock::job_id();
        let root = paths::job_root(&self.cfg.home, id);
        let synced = synced_folders(&self.cfg.home, &self.cfg.path_var).await;
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
        let job = Job::create(id, self.cfg.state.job(id), &created)?;
        job.set_marker(&Marker {
            root: root.clone(),
            state: MarkerState::Live,
        })?;
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
            "harness {harness}; this node owns the job at epoch 1"
        ));
        Ok(())
    }

    async fn job_list(&self, out: &mut Out) {
        let jobs: Vec<_> = self.jobs.lock().await.values().cloned().collect();
        for job in jobs {
            let job = job.lock().await;
            let manifest = job.ledger().manifest();
            out.say(format!(
                "{} {} epoch {} {}",
                job.id(),
                view_word(&job.view(self.id)),
                manifest.lease.epoch(),
                manifest.goal
            ));
        }
        for (id, error) in self.broken_jobs() {
            out.say(format!("{id} closed: {error}"));
        }
        if out.lines.is_empty() {
            out.say("no jobs; make one with `fabric job new --repo <path> --goal <text>`");
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

    async fn job_status(self: &Arc<Self>, id: JobId, out: &mut Out) -> Result<(), NodeError> {
        self.resolve_departing(id, out).await?;
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
        out.say(match job.view(self.id) {
            LeaseView::Owner(_) => "this node: owner".to_owned(),
            LeaseView::Departing { move_id, target } => format!(
                "this node: departing to {target} under move {move_id}; frozen until {target} answers. Run `fabric job move {id} --to {target}` to ask again, or `fabric job steal {id} --force` on another node"
            ),
            LeaseView::Departed { owner, epoch } => format!(
                "this node: departed; {owner} owns the job at epoch {epoch} and this copy never runs"
            ),
            LeaseView::Replica { owner, .. } => {
                format!("this node: replica; run `fabric job resume {id}` to pull it from {owner}")
            }
        });
        out.say(match (manifest.latest_snapshot, job.suspended_in()) {
            (None, _) => "snapshot: none yet".to_owned(),
            (Some(s), Some(_)) => format!("snapshot: {s} (suspended in it)"),
            (Some(s), None) => format!("snapshot: {s} (resumed since)"),
        });
        let owner_here = matches!(job.view(self.id), LeaseView::Owner(_));
        out.say(match job.marker()? {
            None => "workspace: none on this node".to_owned(),
            Some(marker) if !owner_here => format!(
                "workspace: {} is an orphan: it holds changes made after the last snapshot this node published, and nothing here deletes it",
                marker.root.display()
            ),
            Some(marker) => format!(
                "workspace: {} ({})",
                marker.root.display(),
                marker_word(&marker.state)
            ),
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
            },
            None => "harness: not running".to_owned(),
        };
        out.say(running);
        out.say(format!("records: {}", manifest.event_count));
        Ok(())
    }

    async fn job_log(&self, id: JobId, out: &mut Out) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        for (n, record) in job.ledger().records().iter().enumerate() {
            let event = record.event();
            out.say(format!(
                "{n:>4} epoch {} at {} {}",
                event.epoch,
                event.at,
                describe(&event.kind)
            ));
        }
        Ok(())
    }

    async fn job_run(
        self: &Arc<Self>,
        id: JobId,
        harness: Option<HarnessKind>,
        out: &mut Out,
    ) -> Result<RunPlan, NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        let marker = job.reconcile_marker()?.ok_or(NodeError::NoWorkspace(id))?;
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
        plan(&job, token.epoch, marker.root, harness, out)
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
        session: Option<String>,
        out: &mut Out,
    ) -> Result<(), NodeError> {
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
        if !local::leads_group(pid) {
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
            })?;
        }
        out.say(format!(
            "Resumed recorded at epoch {epoch}; {harness} runs as pid {pid}"
        ));
        Ok(())
    }

    async fn harness_exited(&self, id: JobId, pid: u32) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        if let Some(record) = job.harness_record()?
            && record.pid.get() == pid
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
        owner_token(job, self.id)?;
        let Some(marker) = job.reconcile_marker()? else {
            return job.suspended_in().ok_or(NodeError::NoWorkspace(id));
        };
        if let (MarkerState::Clean { snapshot }, Some(suspended)) =
            (&marker.state, job.suspended_in())
            && *snapshot == suspended
        {
            out.say(format!("already suspended in snapshot {suspended}"));
            return Ok(suspended);
        }
        if let MarkerState::Restoring { .. } = marker.state {
            return Err(NodeError::Unfinished {
                job: id,
                what: "a restore",
                command: "resume",
            });
        }
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
        let snapshot = if let Some(EventKind::SnapshotPublished { snapshot, .. }) = last {
            out.say(format!("finishing the suspend that published {snapshot}"));
            snapshot
        } else {
            job.set_marker(&Marker {
                root: marker.root.clone(),
                state: MarkerState::Suspending {
                    head: job.ledger().head(),
                },
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
            snapshot
        };
        self.confirm_departure(job, path_var, out).await?;
        job.append(self.id, EventKind::Suspended { snapshot })?;
        job.set_marker(&Marker {
            root: marker.root,
            state: MarkerState::Clean { snapshot },
        })?;
        out.say(format!(
            "suspended in snapshot {snapshot}; this node keeps the lease at epoch {}",
            job.ledger().lease().epoch()
        ));
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
        if let Some(record) = job.session()? {
            let session = SessionId::parse(&record.session).map_err(|e| NodeError::Peer {
                peer: self.id,
                reason: format!("the recorded session id is not valid: {e}"),
            })?;
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
        let detection = fabric_env::detect::detect(root, &[]).ok();
        let keys = MeasuredKeys::new(facts, detection.as_ref());
        let root = root.to_owned();
        let job_id = job.id();
        let me = self.id;
        let parent = job.latest();
        let (snapshot, report) = with_store(&self.store, move |store| {
            let mut policy = IgnorePolicy::default();
            policy.load_fabricignore(&root)?;
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
        for path in report.denied.iter().chain(&report.denied_untracked) {
            out.say(format!("left behind (secret pattern): {path}"));
        }
        Ok(snapshot)
    }

    /// Pushes the ledger, and through it the latest snapshot, to every paired peer it can reach.
    pub(crate) async fn replicate(self: &Arc<Self>, id: JobId, out: &mut Out) {
        let peers: Vec<NodeId> = self
            .peers
            .snapshot()
            .iter()
            .filter(|entry| !entry.addrs().is_empty())
            .map(fabric_net::peers::PeerEntry::node)
            .collect();
        for peer in peers {
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
                let session = self.dial(peer).await?;
                f(session).await.map_err(|e| NodeError::Unreachable {
                    peer,
                    reason: e.to_string(),
                })
            }
            Err(e) => Err(NodeError::Peer {
                peer,
                reason: e.to_string(),
            }),
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
    /// and the job stays here. A move already open is offered again as it is.
    async fn depart(
        &self,
        id: JobId,
        to: NodeId,
        out: &mut Out,
    ) -> Result<Option<MoveOffer>, NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        if let LeaseView::Owner(_) = job.view(self.id) {
            let suspended = job.suspended_in();
            let unchanged = suspended.is_some()
                && job.marker()?.is_none_or(|marker| {
                    matches!(marker.state, MarkerState::Clean { snapshot } if Some(snapshot) == suspended)
                });
            if !unchanged {
                return Err(NodeError::RanDuringMove {
                    job: id,
                    target: to,
                });
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
                    if let Ok(Fenced::Answer(Offered::Decided { record })) = answer {
                        self.apply_decision(id, to, record.as_bytes(), out).await?;
                        return Ok(());
                    }
                    last_error = format!("{to} has not decided: {answer:?}");
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
    pub(crate) async fn settle(&self, job: &mut Job, out: &mut Out) -> Result<(), NodeError> {
        let view = job.view(self.id);
        if let Some(record) = job.harness_record()? {
            let owned = matches!(view, LeaseView::Owner(token) if token.epoch == record.epoch);
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
        // A copy that matches a published snapshot holds nothing the job
        // lacks, so it leaves the root; a copy with changes since the last
        // snapshot stays where it is, an orphan the user decides about.
        if let LeaseView::Departed { epoch, .. } | LeaseView::Replica { epoch, .. } = view
            && let Some(marker) = job.reconcile_marker()?
            && matches!(marker.state, MarkerState::Clean { .. })
        {
            let dest = departed_path(&marker.root, epoch)?;
            if marker.root.exists() {
                let parent = dest
                    .parent()
                    .ok_or_else(|| NodeError::io(&dest, std::io::Error::other("no parent")))?;
                crate::durable::create_dir(parent).map_err(|e| NodeError::io(parent, e))?;
                std::fs::rename(&marker.root, &dest).map_err(|e| NodeError::io(&marker.root, e))?;
                crate::durable::sync_dir(parent).map_err(|e| NodeError::io(parent, e))?;
                if let Some(root_parent) = marker.root.parent() {
                    crate::durable::sync_dir(root_parent)
                        .map_err(|e| NodeError::io(root_parent, e))?;
                }
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
        self.resolve_departing(id, out).await?;
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
            let kind = match self.job(id).await {
                Ok(job) => Some(job.lock().await.harness().clone()),
                Err(_) => None,
            };
            let root = paths::job_root(&self.cfg.home, id);
            self.preflight(
                holder,
                kind.as_ref().map(|kind| (kind, root.as_path())),
                Target::Here,
                path_var,
                force,
                out,
            )
            .await?;
            self.pull(id, holder, out).await?;
        }
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        let token = owner_token(&job, self.id)?;
        let root = self.restore_if_needed(&mut job, path_var, out).await?;
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid.get(),
            });
        }
        plan(&job, token.epoch, root, harness, out)
    }

    /// The pull form of the move: fetch, ask the owner to start a move toward this node, decide it here.
    async fn pull(
        self: &Arc<Self>,
        id: JobId,
        holder: NodeId,
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
        let Fenced::Answer(MoveRequested::Started { move_id }) = answer else {
            return Err(NodeError::Peer {
                peer: holder,
                reason: format!("it did not start a move: {answer:?}"),
            });
        };
        out.say(format!("{holder} started move {move_id} toward this node"));
        self.pull_records(id, holder).await?;
        self.ensure_latest(id, holder).await?;
        {
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
                other => {
                    return Err(NodeError::Peer {
                        peer: holder,
                        reason: format!("the move does not link here: {other:?}"),
                    });
                }
            }
            out.say(format!(
                "imported move {move_id}; this node owns the job at epoch {}",
                job.ledger().lease().epoch()
            ));
        }
        // The holder learns the decision now, or from MoveStatus later.
        if let Err(e) = self.push_records(id, holder).await {
            out.say(format!(
                "{holder} has not seen the import yet ({e}); it will ask for the decision"
            ));
        }
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
                    return Err(NodeError::Peer {
                        peer: holder,
                        reason: format!(
                            "its ledger forked from this one (its head is {head}); the chains cannot be joined"
                        ),
                    });
                }
                other => {
                    return Err(NodeError::Peer {
                        peer: holder,
                        reason: format!("unexpected answer to a pull: {other:?}"),
                    });
                }
            };
            for bytes in page.records() {
                self.accept_record(id, holder, bytes.as_bytes()).await?;
            }
            if !more {
                return Ok(());
            }
        }
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

    /// Restores the latest snapshot at the root unless the workspace is already there.
    async fn restore_if_needed(
        &self,
        job: &mut Job,
        path_var: &OsString,
        out: &mut Out,
    ) -> Result<PathBuf, NodeError> {
        let id = job.id();
        let marker = job.reconcile_marker()?;
        match &marker {
            Some(Marker {
                root,
                state: MarkerState::Live | MarkerState::Clean { .. },
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
        }
        job.set_marker(&Marker {
            root: root.clone(),
            state: MarkerState::Restoring { snapshot },
        })?;
        if let Some(parent) = root.parent() {
            crate::durable::create_dir(parent).map_err(|e| NodeError::io(parent, e))?;
        }
        let has_harness_state = !manifest.harness_state.entries.is_empty();
        let state_dir = job.dir().harness_state();
        if !(resuming && state == RestoreState::Present) {
            let facts =
                NodeFacts::measure(&self.cfg.home, path_var, self.cfg.cargo_home.clone()).await;
            report_caches(&manifest.caches, &facts, out);
            let target = root.clone();
            let harness_dir = has_harness_state.then(|| state_dir.clone());
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
        }
        if has_harness_state {
            self.restore_harness(job, &root, path_var, out).await?;
        }
        job.set_marker(&Marker {
            root: root.clone(),
            state: MarkerState::Clean { snapshot },
        })?;
        Ok(root)
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
        local::write(
            &job.dir().session(),
            &SessionRecord {
                harness: state.kind().clone(),
                session: state.session().as_str().to_owned(),
            },
        )?;
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
        let (owner, complete) = {
            let job = self.job(id).await?;
            let job = job.lock().await;
            let latest = job.latest().ok_or(NodeError::NoSnapshot(id))?;
            let complete = is_complete(&self.store, latest)
                .await?
                .then_some(CompleteSnapshot::new(latest));
            (job.ledger().lease().holder(), complete)
        };
        {
            let job = self.job(id).await?;
            let mut job = job.lock().await;
            let proposal = job.ledger().steal(
                self.id,
                reason,
                complete,
                clock::event_id(),
                clock::now_ms(),
            )?;
            let warning = format!(
                "{}; the owner's last record is from {} ms since 1970",
                proposal.warning, proposal.warning.last_record_at
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
        }
        if let Err(e) = self.push_records(id, owner).await {
            out.say(format!("{owner} has not seen the steal yet: {e}"));
        }
        Ok(())
    }

    async fn job_decide(
        &self,
        id: JobId,
        approve: bool,
        note: &str,
        out: &mut Out,
    ) -> Result<(), NodeError> {
        let job = self.job(id).await?;
        let mut job = job.lock().await;
        owner_token(&job, self.id)?;
        let word = if approve { "approved" } else { "rejected" };
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
        let here = NodeFacts::measure(&self.cfg.home, path_var, self.cfg.cargo_home.clone()).await;
        let session = self.dial(peer).await?;
        let theirs =
            session
                .facts(SHORT_DEADLINE * 4)
                .await
                .map_err(|e| NodeError::Unreachable {
                    peer,
                    reason: e.to_string(),
                })?;
        let theirs = NodeFacts::from_wire(&theirs);
        let rows = doctor::compare(
            &here,
            &theirs,
            &Context {
                harness: job.map(|(h, _)| h),
                root: job.map(|(_, r)| r),
                target,
                clock_skew_ms: Some(session.clock_skew_ms()),
                wire: Some(session.kind().to_string()),
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

/// The owner's fencing token, or the refusal that says who owns the job.
fn owner_token(job: &Job, me: NodeId) -> Result<FencingToken, NodeError> {
    let id = job.id();
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
    out: &mut Out,
) -> Result<RunPlan, NodeError> {
    job.set_marker(&Marker {
        root: root.clone(),
        state: MarkerState::Live,
    })?;
    let harness = harness.unwrap_or_else(|| job.harness().clone());
    let session = match job.session()? {
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
        root,
        harness,
        session,
        goal: job.ledger().manifest().goal.clone(),
    })
}

/// Says, one line per cache, whether its bytes are written here or rebuilt, and why (ADR 0008).
fn report_caches(caches: &[fabric_core::snapshot::CacheEntry], facts: &NodeFacts, out: &mut Out) {
    for cache in caches {
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
}

/// Where a departed copy goes: beside the root, so the rename never crosses a filesystem.
fn departed_path(root: &Path, epoch: Epoch) -> Result<PathBuf, NodeError> {
    let parent = root
        .parent()
        .ok_or_else(|| NodeError::io(root, std::io::Error::other("a root needs a parent")))?;
    Ok(parent.join("departed").join(epoch.get().to_string()))
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
        MarkerState::Clean { snapshot } => format!("clean at snapshot {snapshot}"),
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
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    use fabric_net::endpoint::DialOrder;

    use super::*;
    use crate::node::NodeConfig;
    use crate::paths::StateDir;

    /// A node in a fresh directory under `/tmp`, which keeps the socket path short.
    fn node() -> (tempfile::TempDir, Arc<Node>) {
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
    async fn job(dir: &Path, node: &Arc<Node>) -> JobId {
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
        let mut out = Out::default();
        node.job_new(&repo, "goal".to_owned(), HarnessKind::Codex, &mut out)
            .await
            .unwrap();
        out.lines[0].parse().unwrap()
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
}
