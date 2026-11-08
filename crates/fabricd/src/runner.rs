//! The attempts this node runs for an authority (docs/managed-jobs.md,
//! "Launch path", the node side): a `Launch` takes a grant, an `Instruct`
//! asks the running attempt to cut, stop or spawn, and every outcome goes
//! back as a `Report`, in order, one at a time per attempt.
//!
//! Invariant: a node acts on a grant only while it is the job's live
//! grant. A launch of a generation older than one this node knows is
//! refused `Superseded`; a heartbeat answered `Stale` or `Unknown`, or a
//! report refused `StaleGeneration`, stops every group of the attempt and
//! reports nothing more. A launch is idempotent by attempt: its record is
//! written before the attempt starts, so a launch sent again, even after
//! this node restarted, is answered `AlreadyLaunched`. An answer says the
//! node took the message, never what came of it: the outcome is a report.
//! Nothing read from a message is trusted past its check: the request's
//! own bounds and every program are checked again here.
//!
//! What the runner counts: launches taken, reports sent and refused,
//! heartbeats and stale verdicts obeyed. Time is the Perf step's alone.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::harness::HarnessKind;
use fabric_core::id::NodeId;
use fabric_core::managed::launch::{
    AfterCut, GrantVerdict, HEARTBEAT_MS, Heartbeat, HeartbeatAnswer, InstructAnswer,
    InstructRefusal, Instruction, LaunchAnswer, LaunchRefusal, LaunchRequest, Report, ReportAnswer,
    ReportRefusal, TaskLaunch,
};
use fabric_core::managed::{
    AttemptId, DrainBudget, ExitOutcome, Fidelity, GrantToken, Observation, Program, ReceiptStatus,
    TaskClass, TaskReceipt,
};
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, mpsc, watch};

use crate::executor::{Executor, ExecutorError};
use crate::local;

/// What a prepared root offers the harness task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepared {
    /// The root the attempt's tasks run in.
    pub root: AbsPath,
    /// The fidelities this node can offer a continuation from it, as
    /// `offered_fidelity` gives them; empty for a launch that continues nothing.
    pub offered: Vec<Fidelity>,
    /// The restored session, for a native-session continuation.
    pub session: Option<SessionId>,
}

/// What the harness task is started with (the launch path, step 4).
#[derive(Debug, Clone)]
pub(crate) struct HarnessPlan {
    /// The attempt.
    pub attempt: AttemptId,
    /// The harness.
    pub harness: HarnessKind,
    /// The root it runs in.
    pub root: AbsPath,
    /// The session to continue, for a native-session continuation.
    pub resume: Option<SessionId>,
    /// The first message.
    pub prompt: String,
    /// The job's secrets, under their names. Held only until the launch.
    pub env: Vec<(String, String)>,
}

/// A harness process its adapter started held in a group of its own.
pub(crate) trait HarnessProcess: Send {
    /// The pid, which leads its group.
    fn pid(&self) -> u32;
    /// Lets the harness run, once the executor recorded it.
    fn release(&mut self) -> Result<(), String>;
    /// Waits for it to end; its exit code, `None` for a signal.
    fn wait(self: Box<Self>) -> BoxFuture<'static, Option<i32>>;
}

/// What the runner needs of the node it runs on: the root, the harness
/// and the link to the authority. The node wires the real one
/// ([`NodeHost`]); the tests a fake.
pub(crate) trait Host: Send + Sync {
    /// Whether this node can start `harness`.
    fn has_harness(&self, harness: &HarnessKind) -> BoxFuture<'_, bool>;
    /// Prepares the attempt's root (the launch path, step 2).
    fn prepare<'a>(&'a self, request: &'a LaunchRequest)
    -> BoxFuture<'a, Result<Prepared, String>>;
    /// Starts the harness task, held.
    fn launch_harness(
        &self,
        plan: HarnessPlan,
    ) -> BoxFuture<'_, Result<Box<dyn HarnessProcess>, String>>;
    /// Builds a build or test task's command in `root`.
    fn task_command(&self, root: &AbsPath, program: &Program) -> std::process::Command;
    /// Sends a report to the authority `to` (`None`: this node). `Err` is
    /// a link that did not carry it.
    fn report(
        &self,
        to: Option<NodeId>,
        report: Report,
    ) -> BoxFuture<'_, Result<ReportAnswer, String>>;
    /// Sends a heartbeat to the authority `to`.
    fn heartbeat(
        &self,
        to: Option<NodeId>,
        beat: Heartbeat,
    ) -> BoxFuture<'_, Result<HeartbeatAnswer, String>>;
    /// Called once an attempt's root is known and before anything starts in
    /// it, and once it ended (`None`), so the capturer can find it.
    fn attempt_root(&self, attempt: AttemptId, root: Option<(&LaunchRequest, &AbsPath)>);
}

/// What the runner counted since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the tests; the Perf step reads the same atomics"
    )
)]
pub(crate) struct Counters {
    /// Launches taken.
    pub launches: u64,
    /// Reports answered by the authority.
    pub reports_sent: u64,
    /// Reports the authority refused.
    pub reports_refused: u64,
    /// Heartbeats answered.
    pub heartbeats: u64,
    /// Stale or unknown verdicts obeyed by a stop.
    pub stale_obeyed: u64,
}

/// The durable record of a launch taken, which answers a launch sent again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Taken {
    token: GrantToken,
}

/// Work the attempt's own task does on an instruction.
#[derive(Debug)]
enum Work {
    Checkpoint(AfterCut),
    Stop(String),
}

/// One attempt this node runs.
struct Ctl {
    token: GrantToken,
    /// The authority that launched it; only it instructs it.
    from: Option<NodeId>,
    work: mpsc::UnboundedSender<Work>,
    /// Set while an instruction's work runs; a second one is `Busy`.
    busy: AtomicBool,
    /// The root, once prepared.
    root: OnceLock<AbsPath>,
}

/// The attempts this node runs for authorities.
pub(crate) struct Runner {
    executor: Arc<Executor>,
    host: OnceLock<Arc<dyn Host>>,
    /// Where the records of the launches taken live.
    taken: PathBuf,
    attempts: Mutex<BTreeMap<AttemptId, Arc<Ctl>>>,
    heartbeat: Duration,
    launches: AtomicU64,
    reports_sent: AtomicU64,
    reports_refused: AtomicU64,
    heartbeats: AtomicU64,
    stale_obeyed: AtomicU64,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Runner({})", self.taken.display())
    }
}

fn refused(reason: LaunchRefusal) -> LaunchAnswer {
    LaunchAnswer::Refused { reason }
}

fn instruct_refused(reason: InstructRefusal) -> InstructAnswer {
    InstructAnswer::Refused { reason }
}

/// The harness task's receipt when it never started.
fn blocked(task: fabric_core::managed::TaskId, reason: String) -> TaskReceipt {
    TaskReceipt {
        task,
        class: TaskClass::Harness,
        status: ReceiptStatus::Blocked { reason },
        exit: None,
        interrupted: false,
        workspace: None,
    }
}

/// Why an attempt's loop ended early.
enum Ended {
    /// The authority named the grant stale; everything was stopped.
    Stale,
}

impl Runner {
    /// A runner whose tasks the executor runs and whose launch records
    /// live under `taken`. The host is bound once the node exists.
    pub(crate) fn new(executor: Arc<Executor>, taken: PathBuf) -> Self {
        Runner {
            executor,
            host: OnceLock::new(),
            taken,
            attempts: Mutex::new(BTreeMap::new()),
            heartbeat: Duration::from_millis(HEARTBEAT_MS),
            launches: AtomicU64::new(0),
            reports_sent: AtomicU64::new(0),
            reports_refused: AtomicU64::new(0),
            heartbeats: AtomicU64::new(0),
            stale_obeyed: AtomicU64::new(0),
        }
    }

    /// Binds the host; a second bind changes nothing.
    pub(crate) fn bind(&self, host: Arc<dyn Host>) {
        let _ = self.host.set(host);
    }

    /// The executor that runs the attempts' tasks.
    #[cfg(test)]
    pub(crate) fn executor(&self) -> &Arc<Executor> {
        &self.executor
    }

    #[cfg(test)]
    fn with_heartbeat(mut self, every: Duration) -> Self {
        self.heartbeat = every;
        self
    }

    /// What the runner counted.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the tests"))]
    pub(crate) fn counters(&self) -> Counters {
        Counters {
            launches: self.launches.load(Ordering::Relaxed),
            reports_sent: self.reports_sent.load(Ordering::Relaxed),
            reports_refused: self.reports_refused.load(Ordering::Relaxed),
            heartbeats: self.heartbeats.load(Ordering::Relaxed),
            stale_obeyed: self.stale_obeyed.load(Ordering::Relaxed),
        }
    }

    fn taken_path(&self, attempt: AttemptId) -> PathBuf {
        self.taken.join(format!("{attempt}.json"))
    }

    /// Takes a grant: checks it, writes the launch record, answers, and
    /// runs the attempt in a task of its own. `from` is the authority
    /// (`None`: this node).
    pub(crate) async fn launch(
        self: &Arc<Self>,
        from: Option<NodeId>,
        request: LaunchRequest,
    ) -> LaunchAnswer {
        let Some(host) = self.host.get().cloned() else {
            return refused(LaunchRefusal::Internal {
                reason: "the runner is not bound to its node yet".into(),
            });
        };
        if let Err(reason) = request.check() {
            return refused(reason);
        }
        let token = request.token;
        for launch in &request.tasks {
            if !matches!(launch.class, TaskClass::Build | TaskClass::Test) {
                return refused(LaunchRefusal::Inconsistent {
                    reason: format!(
                        "task {} is of class {}, which runs no program",
                        launch.task,
                        launch.class.name()
                    ),
                });
            }
        }
        let mut attempts = self.attempts.lock().await;
        if attempts.contains_key(&token.attempt) {
            return refused(LaunchRefusal::AlreadyLaunched);
        }
        match local::read::<Taken>(&self.taken_path(token.attempt)) {
            Ok(Some(_)) => return refused(LaunchRefusal::AlreadyLaunched),
            Ok(None) => {}
            Err(e) => {
                return refused(LaunchRefusal::Internal {
                    reason: e.to_string(),
                });
            }
        }
        if let Some(current) = self.executor.current(token.job).await
            && current > token.generation
        {
            return refused(LaunchRefusal::Superseded { current });
        }
        if !host.has_harness(&request.intent.harness).await {
            return refused(LaunchRefusal::HarnessMissing {
                harness: request.intent.harness.clone(),
            });
        }
        if let Some(name) = self
            .executor
            .unresolved(&request.policy.secrets)
            .into_iter()
            .next()
        {
            return refused(LaunchRefusal::SecretUnresolved { name });
        }
        if let Err(e) = std::fs::create_dir_all(&self.taken)
            .map_err(|source| local::LocalError::Io {
                path: self.taken.clone(),
                source,
            })
            .and_then(|()| local::write(&self.taken_path(token.attempt), &Taken { token }))
        {
            return refused(LaunchRefusal::Internal {
                reason: e.to_string(),
            });
        }
        let (work, rx) = mpsc::unbounded_channel();
        let ctl = Arc::new(Ctl {
            token,
            from,
            work,
            busy: AtomicBool::new(false),
            root: OnceLock::new(),
        });
        attempts.insert(token.attempt, Arc::clone(&ctl));
        drop(attempts);
        self.launches.fetch_add(1, Ordering::Relaxed);
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            runner.run_attempt(host, from, ctl, request, rx).await;
        });
        LaunchAnswer::Accepted
    }

    /// Takes an instruction for an attempt this node runs from the
    /// authority `from` that launched it; answers at once.
    pub(crate) async fn instruct(
        &self,
        from: Option<NodeId>,
        token: GrantToken,
        instruction: Instruction,
    ) -> InstructAnswer {
        let Some(ctl) = self.attempts.lock().await.get(&token.attempt).cloned() else {
            return instruct_refused(InstructRefusal::NotRunning);
        };
        if ctl.from != from {
            // Another authority than the one that granted it names nothing
            // this node runs for it.
            return instruct_refused(InstructRefusal::NotRunning);
        }
        if let Some(current) = self.executor.current(token.job).await
            && current > token.generation
        {
            return instruct_refused(InstructRefusal::Superseded { current });
        }
        if ctl.token != token {
            return instruct_refused(InstructRefusal::NotRunning);
        }
        match instruction {
            Instruction::Spawn(launch) => self.spawn(&ctl, &launch).await,
            Instruction::Checkpoint { then } => {
                if ctl.busy.swap(true, Ordering::AcqRel) {
                    return instruct_refused(InstructRefusal::Busy {
                        what: "another instruction".into(),
                    });
                }
                if ctl.work.send(Work::Checkpoint(then)).is_err() {
                    return instruct_refused(InstructRefusal::NotRunning);
                }
                InstructAnswer::Accepted
            }
            Instruction::Stop { reason } => {
                if ctl.busy.swap(true, Ordering::AcqRel) {
                    return instruct_refused(InstructRefusal::Busy {
                        what: "another instruction".into(),
                    });
                }
                if ctl.work.send(Work::Stop(reason)).is_err() {
                    return instruct_refused(InstructRefusal::NotRunning);
                }
                InstructAnswer::Accepted
            }
        }
    }

    /// Spawns one build or test task of a running attempt.
    async fn spawn(&self, ctl: &Ctl, launch: &TaskLaunch) -> InstructAnswer {
        if let Err(error) = launch.program.check() {
            return instruct_refused(InstructRefusal::BadProgram(error));
        }
        if !matches!(launch.class, TaskClass::Build | TaskClass::Test) {
            return instruct_refused(InstructRefusal::Internal {
                reason: format!("a {} task runs no program", launch.class.name()),
            });
        }
        let (Some(root), Some(host)) = (ctl.root.get(), self.host.get()) else {
            return instruct_refused(InstructRefusal::Busy {
                what: "the root is being prepared".into(),
            });
        };
        let command = host.task_command(root, &launch.program);
        match self
            .executor
            .spawn(ctl.token.attempt, launch.task, launch.class, command)
            .await
        {
            Ok(_) => InstructAnswer::Accepted,
            Err(ExecutorError::Frozen { .. }) => instruct_refused(InstructRefusal::Frozen),
            Err(ExecutorError::NotRunning { .. }) => instruct_refused(InstructRefusal::NotRunning),
            Err(e) => instruct_refused(InstructRefusal::Internal {
                reason: e.to_string(),
            }),
        }
    }

    /// Forgets an attempt whose loop ended; its launch record stays.
    async fn forget(&self, attempt: AttemptId) {
        self.attempts.lock().await.remove(&attempt);
    }

    /// The runner loop of one attempt (the launch path, steps 1 to 10).
    #[expect(
        clippy::too_many_lines,
        reason = "the loop's steps read in their order in one place"
    )]
    async fn run_attempt(
        self: Arc<Self>,
        host: Arc<dyn Host>,
        from: Option<NodeId>,
        ctl: Arc<Ctl>,
        request: LaunchRequest,
        mut work: mpsc::UnboundedReceiver<Work>,
    ) {
        let token = ctl.token;
        let attempt = token.attempt;
        let budget = DrainBudget::new(request.policy.drain_budget_ms).unwrap_or_default();
        // Step 7's heartbeat runs beside everything else: a verdict that
        // the grant is stale reaches every wait below.
        let (stale_tx, stale) = watch::channel(false);
        let retry = Arc::new(Notify::new());
        let beat = tokio::spawn(heartbeat_loop(
            Arc::clone(&self),
            Arc::clone(&host),
            from,
            token,
            stale_tx,
            Arc::clone(&retry),
        ));
        let reporter = Reporter {
            runner: &self,
            host: &*host,
            from,
            token,
            retry,
            stale,
        };
        let outcome = async {
            // Step 2: the root.
            let prepared = match host.prepare(&request).await {
                Ok(prepared) => prepared,
                Err(reason) => {
                    reporter.fail_to_start(request.task, reason).await?;
                    return Ok(());
                }
            };
            // Step 1: the executor's attempt, with its root and secrets.
            self.executor
                .begin(
                    attempt,
                    token.job,
                    token.generation,
                    prepared.root.clone(),
                    request.policy.secrets.clone(),
                )
                .await;
            let _ = ctl.root.set(prepared.root.clone());
            host.attempt_root(attempt, Some((&request, &prepared.root)));
            // Step 3: what a continuation achieves; a refusal is never lowered.
            let achieved = match &request.continuation {
                Some(ask) => match ask.allowed.highest(&prepared.offered) {
                    Ok(achieved) => Some(achieved),
                    Err(refusal) => {
                        reporter
                            .fail_to_start(
                                request.task,
                                format!(
                                    "no allowed fidelity is offered here: {}",
                                    refusal
                                        .offered
                                        .iter()
                                        .map(|f| f.name())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            )
                            .await?;
                        return Ok(());
                    }
                },
                None => None,
            };
            if achieved == Some(Fidelity::WorkspaceOnly) {
                // Workspace only starts no harness: the workspace is the work.
                reporter
                    .send(Observation::Started {
                        attempt,
                        generation: token.generation,
                    })
                    .await?;
                if let Some(ask) = &request.continuation {
                    reporter
                        .send(Observation::Continued {
                            attempt,
                            generation: token.generation,
                            from: ask.from,
                            achieved: Fidelity::WorkspaceOnly,
                            package: None,
                        })
                        .await?;
                }
                reporter
                    .send(Observation::Exited {
                        attempt,
                        generation: token.generation,
                        outcome: ExitOutcome::Succeeded,
                    })
                    .await?;
                return Ok(());
            }
            // Step 4: the harness task, through its adapter.
            let with_package = achieved == Some(Fidelity::NewNativeSessionWithPackage);
            let mut prompt = request.intent.goal.clone();
            if with_package && let Some(package) = &request.package {
                let path = package_path(&prepared.root, attempt);
                if let Err(e) = std::fs::write(&path, package.canonical_json()) {
                    reporter
                        .fail_to_start(request.task, format!("the package was not written: {e}"))
                        .await?;
                    return Ok(());
                }
                let _ = std::fmt::Write::write_fmt(
                    &mut prompt,
                    format_args!(
                        "\n\nThe work so far is described in the work package at {}.",
                        path.display()
                    ),
                );
            }
            let env = match self.executor.secret_env(attempt).await {
                Ok(env) => env,
                Err(e) => {
                    reporter.fail_to_start(request.task, e.to_string()).await?;
                    return Ok(());
                }
            };
            let plan = HarnessPlan {
                attempt,
                harness: request.intent.harness.clone(),
                root: prepared.root.clone(),
                resume: if achieved == Some(Fidelity::NativeSession) {
                    prepared.session.clone()
                } else {
                    None
                },
                prompt,
                env,
            };
            let mut harness = match host.launch_harness(plan).await {
                Ok(harness) => harness,
                Err(reason) => {
                    reporter.fail_to_start(request.task, reason).await?;
                    return Ok(());
                }
            };
            if let Err(e) = self
                .executor
                .adopt(attempt, request.task, harness.pid(), None)
                .await
            {
                drop(harness);
                reporter.fail_to_start(request.task, e.to_string()).await?;
                return Ok(());
            }
            if let Err(reason) = harness.release() {
                let _ = self.executor.stop(attempt, budget).await;
                reporter.fail_to_start(request.task, reason).await?;
                return Ok(());
            }
            let mut exit = harness.wait();
            // Step 5: started, and what a continuation achieved.
            reporter
                .send(Observation::Started {
                    attempt,
                    generation: token.generation,
                })
                .await?;
            if let (Some(ask), Some(achieved)) = (&request.continuation, achieved) {
                reporter
                    .send(Observation::Continued {
                        attempt,
                        generation: token.generation,
                        from: ask.from,
                        achieved,
                        package: if with_package {
                            request.package.as_ref().map(fabric_core::managed::WorkPackage::digest)
                        } else {
                            None
                        },
                    })
                    .await?;
            }
            // Step 6: the build and test tasks the launch names; one that
            // does not start is reported `Blocked` with why, as a spawn
            // instruction that fails is refused with why.
            let mut unstarted = Vec::new();
            for launch in &request.tasks {
                let command = host.task_command(&prepared.root, &launch.program);
                if let Err(e) = self
                    .executor
                    .spawn(attempt, launch.task, launch.class, command)
                    .await
                {
                    tracing::warn!(%attempt, task = %launch.task, error = %e, "a task of the launch did not start");
                    unstarted.push(TaskReceipt {
                        class: launch.class,
                        ..blocked(launch.task, e.to_string())
                    });
                }
            }
            if !unstarted.is_empty() {
                reporter
                    .send(Observation::Receipts {
                        attempt,
                        generation: token.generation,
                        receipts: unstarted,
                    })
                    .await?;
            }
            // Steps 7 to 10.
            let mut stale = reporter.stale.clone();
            loop {
                tokio::select! {
                    changed = stale.changed() => {
                        if changed.is_err() || *stale.borrow() {
                            return Err(Ended::Stale);
                        }
                    }
                    code = &mut exit => {
                        // Step 8: the harness ended; what still runs is stopped.
                        let mut receipts = self.executor.stop(attempt, budget).await.unwrap_or_default();
                        for receipt in &mut receipts {
                            if receipt.task == request.task {
                                receipt.status = ReceiptStatus::Exited;
                                receipt.exit = code;
                                receipt.interrupted = false;
                            }
                        }
                        reporter.send(Observation::Receipts { attempt, generation: token.generation, receipts }).await?;
                        let outcome = if code == Some(0) {
                            ExitOutcome::Succeeded
                        } else {
                            ExitOutcome::Failed { code }
                        };
                        reporter.send(Observation::Exited { attempt, generation: token.generation, outcome }).await?;
                        return Ok(());
                    }
                    next = work.recv() => {
                        let Some(next) = next else { return Ok(()); };
                        let done = match next {
                            Work::Checkpoint(then) => self.checkpoint(&reporter, budget, then).await?,
                            Work::Stop(_reason) => {
                                // Step 10.
                                let receipts = self.executor.stop(attempt, budget).await.unwrap_or_default();
                                reporter.send(Observation::Receipts { attempt, generation: token.generation, receipts }).await?;
                                reporter.send(Observation::Exited { attempt, generation: token.generation, outcome: ExitOutcome::Cancelled }).await?;
                                true
                            }
                        };
                        ctl.busy.store(false, Ordering::Release);
                        if done {
                            // The harness process is the caller's to reap.
                            let _ = tokio::time::timeout(Duration::from_secs(5), &mut exit).await;
                            return Ok(());
                        }
                    }
                }
            }
        }
        .await;
        if let Err(Ended::Stale) = outcome {
            self.stale_obeyed.fetch_add(1, Ordering::Relaxed);
            let _ = self.executor.stop(attempt, budget).await;
        }
        beat.abort();
        let _ = self.executor.end(attempt).await;
        host.attempt_root(attempt, None);
        self.forget(attempt).await;
    }

    /// Step 9: quiesce, then a cut reported, or the blockers reported.
    /// True when the attempt ended with it.
    async fn checkpoint(
        &self,
        reporter: &Reporter<'_>,
        budget: DrainBudget,
        then: AfterCut,
    ) -> Result<bool, Ended> {
        let token = reporter.token;
        let attempt = token.attempt;
        let quiescence = match self.executor.quiesce(attempt, budget).await {
            Ok(quiescence) => quiescence,
            Err(e) => {
                tracing::warn!(%attempt, error = %e, "the quiescence failed");
                return Ok(false);
            }
        };
        tracing::debug!(%attempt, blockers = quiescence.blockers.len(), receipts = quiescence.receipts.len(), "quiesced");
        if !quiescence.blockers.is_empty() {
            reporter
                .send(Observation::HandoffBlocked {
                    attempt,
                    generation: token.generation,
                    blockers: quiescence.blockers,
                })
                .await?;
            let _ = self.executor.resume(attempt).await;
            return Ok(false);
        }
        let cut = match self.executor.cut(attempt, &quiescence).await {
            Ok(cut) => cut,
            Err(e) => {
                reporter
                    .send(Observation::HandoffBlocked {
                        attempt,
                        generation: token.generation,
                        blockers: vec![fabric_core::managed::HandoffBlocker::CheckpointFailed {
                            task: quiescence
                                .receipts
                                .first()
                                .map_or(fabric_core::managed::TaskId::from_bytes([0; 16]), |r| {
                                    r.task
                                }),
                            reason: e.to_string(),
                        }],
                    })
                    .await?;
                let _ = self.executor.resume(attempt).await;
                return Ok(false);
            }
        };
        tracing::debug!(%attempt, snapshot = %cut.snapshot, "cut");
        // A cut that ends the attempt stops what still runs before it is
        // reported: the authority places the replacement as soon as the
        // cut is recorded, so nothing of this attempt may run by then.
        let stopped = match then {
            AfterCut::End => Some(self.executor.stop(attempt, budget).await.unwrap_or_default()),
            AfterCut::Resume => None,
        };
        let answer = reporter
            .send(Observation::Checkpointed {
                attempt,
                generation: token.generation,
                snapshot: cut.snapshot,
                cut,
            })
            .await?;
        if let Some(receipts) = stopped {
            if !matches!(answer, ReportAnswer::Recorded { .. }) {
                // The cut was refused and nothing runs any more: the
                // attempt's end is reported as a failure, so its task is
                // free for another continue.
                reporter
                    .send(Observation::Receipts {
                        attempt,
                        generation: token.generation,
                        receipts,
                    })
                    .await?;
                reporter
                    .send(Observation::Exited {
                        attempt,
                        generation: token.generation,
                        outcome: ExitOutcome::Failed { code: None },
                    })
                    .await?;
            }
            return Ok(true);
        }
        // Resumed, or refused for another reason than a stale grant: the
        // attempt goes on.
        let _ = self.executor.resume(attempt).await;
        Ok(false)
    }
}

/// Where a continuation's package is written: beside the root.
fn package_path(root: &AbsPath, attempt: AttemptId) -> PathBuf {
    let root = std::path::Path::new(root.as_str());
    root.parent()
        .unwrap_or(root)
        .join(format!("package-{attempt}.json"))
}

/// Step 7: a heartbeat every interval; a `Live` answer lets a waiting
/// report go again, `Stale` or `Unknown` ends the attempt.
async fn heartbeat_loop(
    runner: Arc<Runner>,
    host: Arc<dyn Host>,
    to: Option<NodeId>,
    token: GrantToken,
    stale: watch::Sender<bool>,
    retry: Arc<Notify>,
) {
    let mut interval = tokio::time::interval(runner.heartbeat);
    interval.tick().await;
    loop {
        interval.tick().await;
        let Ok(answer) = host
            .heartbeat(
                to,
                Heartbeat {
                    attempts: vec![token],
                },
            )
            .await
        else {
            continue;
        };
        runner.heartbeats.fetch_add(1, Ordering::Relaxed);
        let verdict = answer
            .verdicts
            .iter()
            .find(|(attempt, _)| *attempt == token.attempt)
            .map(|(_, verdict)| *verdict);
        match verdict {
            Some(GrantVerdict::Stale { .. } | GrantVerdict::Unknown) => {
                let _ = stale.send(true);
                return;
            }
            // Live, or an answer that names nothing of this grant, which
            // says nothing of it: a waiting report goes again.
            _ => retry.notify_one(),
        }
    }
}

/// The reports of one attempt: in order, one in flight; a link failure or
/// `Unavailable` waits for the next heartbeat answer and sends again.
struct Reporter<'a> {
    runner: &'a Runner,
    host: &'a dyn Host,
    from: Option<NodeId>,
    token: GrantToken,
    retry: Arc<Notify>,
    stale: watch::Receiver<bool>,
}

impl Reporter<'_> {
    async fn send(&self, observation: Observation) -> Result<ReportAnswer, Ended> {
        let mut stale = self.stale.clone();
        loop {
            if *stale.borrow() {
                return Err(Ended::Stale);
            }
            let report = Report {
                token: self.token,
                observation: observation.clone(),
            };
            match self.host.report(self.from, report).await {
                Ok(ReportAnswer::Refused {
                    reason: ReportRefusal::Unavailable,
                })
                | Err(_) => {
                    tokio::select! {
                        () = self.retry.notified() => {}
                        _ = stale.changed() => {}
                    }
                }
                Ok(answer) => {
                    self.runner.reports_sent.fetch_add(1, Ordering::Relaxed);
                    if let ReportAnswer::Refused { reason } = &answer {
                        self.runner.reports_refused.fetch_add(1, Ordering::Relaxed);
                        tracing::info!(attempt = %self.token.attempt, ?reason, "a report was refused");
                        if matches!(reason, ReportRefusal::StaleGeneration { .. }) {
                            return Err(Ended::Stale);
                        }
                    }
                    return Ok(answer);
                }
            }
        }
    }

    /// Steps 2 and 3 failing: the harness receipt `Blocked`, then `Failed`.
    async fn fail_to_start(
        &self,
        task: fabric_core::managed::TaskId,
        reason: String,
    ) -> Result<(), Ended> {
        self.send(Observation::Receipts {
            attempt: self.token.attempt,
            generation: self.token.generation,
            receipts: vec![blocked(task, reason)],
        })
        .await?;
        self.send(Observation::Exited {
            attempt: self.token.attempt,
            generation: self.token.generation,
            outcome: ExitOutcome::Failed { code: None },
        })
        .await?;
        Ok(())
    }
}

/// How long a report or a heartbeat waits for the authority's answer.
const LINK_DEADLINE: Duration = Duration::from_secs(10);

/// The node a runner runs on: its sources, store, adapters and links.
pub(crate) struct NodeHost {
    node: std::sync::Weak<crate::node::Node>,
    capturer: Arc<crate::capturer::NodeCapturer>,
}

impl NodeHost {
    pub(crate) fn new(
        node: std::sync::Weak<crate::node::Node>,
        capturer: Arc<crate::capturer::NodeCapturer>,
    ) -> Self {
        NodeHost { node, capturer }
    }

    fn node(&self) -> Result<Arc<crate::node::Node>, String> {
        self.node
            .upgrade()
            .ok_or_else(|| "the node is stopping".to_owned())
    }
}

/// A harness its adapter started, which this node reaps.
struct AdapterHarness(fabric_harness::Running);

impl HarnessProcess for AdapterHarness {
    fn pid(&self) -> u32 {
        self.0.pid().get()
    }
    fn release(&mut self) -> Result<(), String> {
        self.0.release().map_err(|e| e.to_string())
    }
    fn wait(self: Box<Self>) -> BoxFuture<'static, Option<i32>> {
        let mut running = self.0;
        Box::pin(async move {
            running
                .child_mut()
                .wait()
                .await
                .ok()
                .and_then(|status| status.code())
        })
    }
}

/// Moves a root a previous attempt left aside, so the new attempt's
/// preparation starts from an empty directory and nothing is lost.
fn set_aside(root: &std::path::Path, attempt: AttemptId) -> Result<(), String> {
    let Ok(mut entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    if entries.next().is_none() {
        return Ok(());
    }
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no name", root.display()))?;
    let aside = root.with_file_name(format!("{name}.before-{attempt}"));
    std::fs::rename(root, &aside).map_err(|e| format!("{}: {e}", root.display()))
}

impl Host for NodeHost {
    fn has_harness(&self, harness: &HarnessKind) -> BoxFuture<'_, bool> {
        let harness = harness.clone();
        Box::pin(async move {
            let Ok(node) = self.node() else {
                return false;
            };
            let env = node.node_env();
            if let HarnessKind::Acp { program, .. } = &harness {
                return crate::facts::find_on_path(program.as_str(), env.path_var()).is_some();
            }
            let Ok(harness_env) = node.harness_env(&env) else {
                return false;
            };
            match crate::harnesses::adapter(&harness, &harness_env, None) {
                Ok(adapter) => matches!(adapter.detect().await, Ok(Some(_))),
                Err(_) => false,
            }
        })
    }

    fn prepare<'a>(
        &'a self,
        request: &'a LaunchRequest,
    ) -> BoxFuture<'a, Result<Prepared, String>> {
        Box::pin(async move {
            let node = self.node()?;
            let job = request.token.job;
            let root = crate::sources::root_for(&node, job);
            set_aside(&root, request.token.attempt)?;
            let source = match request.initial_checkpoint {
                Some(snapshot) => {
                    let complete = crate::transfer::is_complete(&node.store, snapshot)
                        .await
                        .map_err(|e| e.to_string())?;
                    if !complete
                        && let Some(peer) = request.snapshot_at
                        && peer != node.id
                    {
                        let session = node.session(peer).await.map_err(|e| e.to_string())?;
                        crate::transfer::fetch_snapshot(&node.store, &session, snapshot)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    fabric_core::managed::SourceBinding::Snapshot { snapshot }
                }
                None => request.source.clone(),
            };
            let prepared = crate::sources::prepare(&node, job, &source)
                .await
                .map_err(|e| e.to_string())?;
            let root = match prepared {
                crate::sources::Prepared::Cloned { root, .. }
                | crate::sources::Prepared::Restored { root, .. }
                | crate::sources::Prepared::Empty { root } => root,
            };
            let offered = if request.continuation.is_some() {
                let ask = fabric_net::rpc::Advertise {
                    job,
                    harness: request.intent.harness.clone(),
                    root: root.clone(),
                    snapshot: request.initial_checkpoint,
                    secrets: Vec::new(),
                };
                let advert = crate::placement::advert_from_wire(&node.own_advert(&ask).await);
                // A restore of the harness's own state is not part of this
                // launch, so the layout is never called the same and no
                // native session is offered (docs/managed-jobs.md, "Launch
                // path", as built).
                fabric_core::managed::offered_fidelity(
                    &request.intent.harness,
                    None,
                    &advert,
                    false,
                )
            } else {
                Vec::new()
            };
            Ok(Prepared {
                root,
                offered,
                session: None,
            })
        })
    }

    fn launch_harness(
        &self,
        plan: HarnessPlan,
    ) -> BoxFuture<'_, Result<Box<dyn HarnessProcess>, String>> {
        Box::pin(async move {
            let node = self.node()?;
            let tool_env = node.node_env();
            let harness_env = node.harness_env(&tool_env).map_err(|e| e.to_string())?;
            let adapter = crate::harnesses::adapter(&plan.harness, &harness_env, None)
                .map_err(|e| e.to_string())?;
            // The daemon's environment, as a CLI run passes the user's,
            // without the node's secrets; the job's own under their names.
            let mut env: Vec<(fabric_core::env::EnvVarName, String)> = std::env::vars_os()
                .filter_map(|(name, value)| {
                    let name = name.to_str()?;
                    if name.starts_with(fabric_core::managed::SECRET_ENV_PREFIX) {
                        return None;
                    }
                    let name = fabric_core::env::EnvVarName::parse(name).ok()?;
                    Some((name, value.into_string().ok()?))
                })
                .collect();
            for (name, value) in plan.env {
                if let Ok(name) = fabric_core::env::EnvVarName::parse(&name) {
                    env.retain(|(have, _)| *have != name);
                    env.push((name, value));
                }
            }
            let launch = fabric_harness::Launch::Headless {
                resume: plan.resume,
                prompt: plan.prompt,
            };
            let mut running = adapter
                .launch(std::path::Path::new(plan.root.as_str()), launch, &env)
                .await
                .map_err(|e| e.to_string())?;
            drop(env);
            // A headless harness prints its turn; it is read for the
            // session and dropped, so a full pipe never stops the harness.
            let read_line: Option<fn(&mut fabric_harness::TurnReport, &str) -> bool> =
                match plan.harness {
                    HarnessKind::ClaudeCode => {
                        Some(fabric_harness::claude::ClaudeCode::read_headless_line)
                    }
                    HarnessKind::Codex => Some(fabric_harness::codex::Codex::read_headless_line),
                    HarnessKind::Acp { .. } => None,
                };
            let capturer = Arc::clone(&self.capturer);
            let attempt = plan.attempt;
            if let Some(stdout) = running.child_mut().stdout.take() {
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = tokio::io::BufReader::new(stdout).lines();
                    let mut report = fabric_harness::TurnReport::default();
                    let mut named = false;
                    while let Ok(Some(line)) = lines.next_line().await {
                        if let Some(read) = read_line {
                            read(&mut report, &line);
                            if !named && let Some(session) = report.session.clone() {
                                capturer.set_session(attempt, session);
                                named = true;
                            }
                        }
                    }
                });
            }
            if let Some(stderr) = running.child_mut().stderr.take() {
                tokio::spawn(async move {
                    let mut stderr = stderr;
                    let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
                });
            }
            if let Some(mut turn) = running.take_turn() {
                let capturer = Arc::clone(&self.capturer);
                tokio::spawn(async move {
                    while let Some(event) = turn.next().await {
                        if let fabric_harness::TurnEvent::Session(session) = event {
                            capturer.set_session(attempt, session);
                        }
                    }
                });
            }
            Ok(Box::new(AdapterHarness(running)) as Box<dyn HarnessProcess>)
        })
    }

    fn task_command(&self, root: &AbsPath, program: &Program) -> std::process::Command {
        let mut command = std::process::Command::new(&program.argv[0]);
        command.args(&program.argv[1..]);
        let dir = match &program.cwd {
            Some(cwd) => std::path::Path::new(root.as_str()).join(cwd.as_str()),
            None => PathBuf::from(root.as_str()),
        };
        command.current_dir(dir);
        // Of the node's environment only `PATH`, `HOME` and what the
        // program names; the executor adds the job's secrets.
        command.env_clear();
        if let Some(node) = self.node.upgrade() {
            command.env("PATH", node.node_env().path_var());
            command.env("HOME", &node.cfg.home);
        }
        for name in &program.env_allowlist {
            if !name
                .as_str()
                .starts_with(fabric_core::managed::SECRET_ENV_PREFIX)
                && let Some(value) = std::env::var_os(name.as_str())
            {
                command.env(name.as_str(), value);
            }
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command
    }

    fn report(
        &self,
        to: Option<NodeId>,
        report: Report,
    ) -> BoxFuture<'_, Result<ReportAnswer, String>> {
        Box::pin(async move {
            let node = self.node()?;
            match to {
                None => Ok(node.accept_report(None, report).await),
                Some(peer) => {
                    let session = node.session(peer).await.map_err(|e| e.to_string())?;
                    session
                        .report(report, LINK_DEADLINE)
                        .await
                        .map_err(|e| e.to_string())
                }
            }
        })
    }

    fn heartbeat(
        &self,
        to: Option<NodeId>,
        beat: Heartbeat,
    ) -> BoxFuture<'_, Result<HeartbeatAnswer, String>> {
        Box::pin(async move {
            let node = self.node()?;
            match to {
                None => Ok(node.accept_heartbeat(None, &beat).await),
                Some(peer) => {
                    let session = node.session(peer).await.map_err(|e| e.to_string())?;
                    session
                        .heartbeat(beat, LINK_DEADLINE)
                        .await
                        .map_err(|e| e.to_string())
                }
            }
        })
    }

    fn attempt_root(&self, attempt: AttemptId, root: Option<(&LaunchRequest, &AbsPath)>) {
        self.capturer.set(
            attempt,
            root.map(|(request, root)| crate::capturer::Attempt {
                job: request.token.job,
                root: root.clone(),
                harness: request.intent.harness.clone(),
                export: request.policy.export_allowlist.clone(),
                session: None,
            }),
        );
    }
}

/// The adapters' suspend path, for the capturer.
pub(crate) struct AdapterSuspend {
    node: std::sync::Weak<crate::node::Node>,
}

impl AdapterSuspend {
    pub(crate) fn new(node: std::sync::Weak<crate::node::Node>) -> Self {
        AdapterSuspend { node }
    }
}

impl crate::capturer::Suspend for AdapterSuspend {
    fn suspend<'a>(
        &'a self,
        attempt: &'a crate::capturer::Attempt,
        session: &'a SessionId,
        out: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let node = self
                .node
                .upgrade()
                .ok_or_else(|| "the node is stopping".to_owned())?;
            let env = node
                .harness_env(&node.node_env())
                .map_err(|e| e.to_string())?;
            let adapter = crate::harnesses::adapter(&attempt.harness, &env, None)
                .map_err(|e| e.to_string())?;
            adapter
                .capture(std::path::Path::new(attempt.root.as_str()), session, out)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}

/// The node's facts a cut records, measured when it is taken.
pub(crate) struct NodeFactsNow {
    node: std::sync::Weak<crate::node::Node>,
}

impl NodeFactsNow {
    pub(crate) fn new(node: std::sync::Weak<crate::node::Node>) -> Self {
        NodeFactsNow { node }
    }
}

impl crate::capturer::Facts for NodeFactsNow {
    fn measure(&self) -> BoxFuture<'_, Vec<(String, String)>> {
        Box::pin(async move {
            let Some(node) = self.node.upgrade() else {
                return Vec::new();
            };
            let facts = crate::facts::NodeFacts::measure(
                &node.cfg.home,
                &node.node_env(),
                crate::facts::Probes::NONE,
            )
            .await;
            let mut facts: Vec<(String, String)> = facts
                .iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect();
            facts.sort();
            facts
        })
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::os::unix::process::CommandExt;
    use std::sync::Mutex as StdMutex;

    use fabric_core::id::{JobId, SnapshotId};
    use fabric_core::managed::launch::ContinuationAsk;
    use fabric_core::managed::{
        AllowedFidelity, EventDigest, Generation, Intent, JobPolicy, RestartPolicy, SourceBinding,
        TaskId, WorkPackage, WorkRevision, WorkspaceRevision,
    };

    use super::*;
    use crate::executor::Capturer;
    use crate::secrets::LocalResolver;

    struct NoCapture;

    impl Capturer for NoCapture {
        fn checkpoint_harness(
            &self,
            attempt: AttemptId,
            _task: fabric_core::managed::TaskId,
        ) -> BoxFuture<'_, Result<SnapshotId, String>> {
            Box::pin(async move { Ok(SnapshotId::of(&attempt.to_bytes())) })
        }
        fn capture(&self, attempt: AttemptId) -> BoxFuture<'_, Result<SnapshotId, String>> {
            Box::pin(async move { Ok(SnapshotId::of(&[attempt.to_bytes(), [1; 16]].concat())) })
        }
        fn facts(&self) -> Vec<(String, String)> {
            vec![("os".into(), "test".into())]
        }
    }

    /// A harness that is `sleep` in a group of its own.
    struct Sleeper(std::process::Child);

    impl HarnessProcess for Sleeper {
        fn pid(&self) -> u32 {
            self.0.id()
        }
        fn release(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn wait(self: Box<Self>) -> BoxFuture<'static, Option<i32>> {
            let mut child = self.0;
            Box::pin(async move {
                tokio::task::spawn_blocking(move || child.wait().ok().and_then(|s| s.code()))
                    .await
                    .ok()
                    .flatten()
            })
        }
    }

    /// How the fake authority answers.
    #[derive(Default)]
    struct Script {
        /// The verdict every heartbeat gets.
        verdict: Option<GrantVerdict>,
        /// Reports refused as stale.
        refuse_stale: bool,
        /// How many reports fail on the link before one goes through.
        link_failures: u32,
        /// The preparation fails with this.
        prepare_fails: Option<String>,
        /// What the node offers a continuation.
        offered: Vec<Fidelity>,
        /// The harness exits on its own after this many seconds.
        harness_secs: u32,
    }

    #[derive(Default)]
    struct FakeHost {
        script: StdMutex<Script>,
        reports: StdMutex<Vec<Observation>>,
        /// What happened, in order: `report`, `link-fail`, `heartbeat`.
        log: StdMutex<Vec<&'static str>>,
        harness_pids: StdMutex<Vec<u32>>,
        /// Whether the last harness ran when each cut was reported.
        alive_at_cut: StdMutex<Vec<bool>>,
        dir: PathBuf,
    }

    impl Host for FakeHost {
        fn has_harness(&self, _harness: &HarnessKind) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }
        fn prepare<'a>(
            &'a self,
            request: &'a LaunchRequest,
        ) -> BoxFuture<'a, Result<Prepared, String>> {
            Box::pin(async move {
                let script = self.script.lock().unwrap();
                if let Some(reason) = &script.prepare_fails {
                    return Err(reason.clone());
                }
                let root = self.dir.join(request.token.attempt.to_string());
                std::fs::create_dir_all(&root).unwrap();
                Ok(Prepared {
                    root: AbsPath::parse(root.to_str().unwrap()).unwrap(),
                    offered: script.offered.clone(),
                    session: None,
                })
            })
        }
        fn launch_harness(
            &self,
            _plan: HarnessPlan,
        ) -> BoxFuture<'_, Result<Box<dyn HarnessProcess>, String>> {
            let secs = self.script.lock().unwrap().harness_secs.max(1);
            Box::pin(async move {
                let mut command = std::process::Command::new("sh");
                command
                    .arg("-c")
                    .arg(format!("exec sleep {secs}"))
                    .process_group(0)
                    .stdin(std::process::Stdio::null());
                let child = command.spawn().map_err(|e| e.to_string())?;
                self.harness_pids.lock().unwrap().push(child.id());
                Ok(Box::new(Sleeper(child)) as Box<dyn HarnessProcess>)
            })
        }
        fn task_command(&self, root: &AbsPath, program: &Program) -> std::process::Command {
            let mut command = std::process::Command::new(&program.argv[0]);
            command
                .args(&program.argv[1..])
                .current_dir(root.as_str())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            command
        }
        fn report(
            &self,
            _to: Option<NodeId>,
            report: Report,
        ) -> BoxFuture<'_, Result<ReportAnswer, String>> {
            Box::pin(async move {
                let mut script = self.script.lock().unwrap();
                if script.link_failures > 0 {
                    script.link_failures -= 1;
                    self.log.lock().unwrap().push("link-fail");
                    return Err("the link is down".into());
                }
                self.log.lock().unwrap().push("report");
                if matches!(report.observation, Observation::Checkpointed { .. }) {
                    let harness = self.harness_pids.lock().unwrap().last().copied();
                    self.alive_at_cut
                        .lock()
                        .unwrap()
                        .push(harness.is_some_and(alive));
                }
                self.reports.lock().unwrap().push(report.observation);
                if script.refuse_stale {
                    return Ok(ReportAnswer::Refused {
                        reason: ReportRefusal::StaleGeneration {
                            current: Generation::FIRST.next().unwrap(),
                        },
                    });
                }
                Ok(ReportAnswer::Recorded {
                    revision: WorkRevision::new(2).unwrap(),
                })
            })
        }
        fn heartbeat(
            &self,
            _to: Option<NodeId>,
            beat: Heartbeat,
        ) -> BoxFuture<'_, Result<HeartbeatAnswer, String>> {
            Box::pin(async move {
                self.log.lock().unwrap().push("heartbeat");
                let verdict = self
                    .script
                    .lock()
                    .unwrap()
                    .verdict
                    .unwrap_or(GrantVerdict::Live);
                Ok(HeartbeatAnswer {
                    verdicts: beat.attempts.iter().map(|t| (t.attempt, verdict)).collect(),
                })
            })
        }
        fn attempt_root(&self, _attempt: AttemptId, _root: Option<(&LaunchRequest, &AbsPath)>) {}
    }

    struct Bench {
        dir: tempfile::TempDir,
        runner: Arc<Runner>,
        host: Arc<FakeHost>,
    }

    fn bench(script: Script) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let resolver = LocalResolver::with_environment(dir.path(), Vec::new());
        let executor = Arc::new(Executor::new(
            dir.path().join("records"),
            resolver,
            Arc::new(NoCapture),
        ));
        let runner = Arc::new(
            Runner::new(executor, dir.path().join("taken"))
                .with_heartbeat(Duration::from_millis(50)),
        );
        let host = Arc::new(FakeHost {
            script: StdMutex::new(script),
            dir: dir.path().join("roots"),
            ..FakeHost::default()
        });
        runner.bind(host.clone());
        Bench { dir, runner, host }
    }

    const JOB: JobId = JobId::from_bytes([1; 16]);

    fn token(n: u8) -> GrantToken {
        GrantToken {
            job: JOB,
            attempt: AttemptId::from_bytes([n; 16]),
            generation: Generation::FIRST,
        }
    }

    fn request(n: u8) -> LaunchRequest {
        let mut policy = JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly);
        policy.drain_budget_ms = DrainBudget::MIN_MS;
        LaunchRequest {
            token: token(n),
            task: TaskId::from_bytes([9; 16]),
            intent: Intent {
                goal: "make it work".into(),
                harness: HarnessKind::Codex,
                acceptance: Vec::new(),
            },
            source: SourceBinding::Empty,
            policy,
            initial_checkpoint: None,
            snapshot_at: None,
            tasks: Vec::new(),
            continuation: None,
            package: None,
        }
    }

    fn test_task(n: u8, argv: &[&str]) -> TaskLaunch {
        TaskLaunch {
            task: TaskId::from_bytes([n; 16]),
            class: TaskClass::Test,
            program: Program {
                argv: argv.iter().map(|a| (*a).to_owned()).collect(),
                cwd: None,
                env_allowlist: Vec::new(),
            },
        }
    }

    fn package() -> WorkPackage {
        WorkPackage {
            version: fabric_core::managed::PACKAGE_VERSION,
            job: JOB,
            revision: WorkRevision::new(3).unwrap(),
            intent: Intent {
                goal: "make it work".into(),
                harness: HarnessKind::Codex,
                acceptance: Vec::new(),
            },
            decisions: Vec::new(),
            tasks: Vec::new(),
            workspace: WorkspaceRevision {
                bundle_head: SnapshotId::of(b"head"),
                ledger_head: EventDigest::from_bytes([2; 32]),
            },
            evidence: Vec::new(),
            questions: Vec::new(),
            artifacts: Vec::new(),
            next_actions: Vec::new(),
        }
    }

    fn alive(pid: u32) -> bool {
        NonZeroU32::new(pid).is_some_and(|pid| {
            std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .is_ok_and(|o| {
                    let s = String::from_utf8_lossy(&o.stdout);
                    !s.trim().is_empty() && !s.trim_start().starts_with('Z')
                })
        })
    }

    /// The observations' names, in the order the authority got them.
    fn names(host: &FakeHost) -> Vec<&'static str> {
        host.reports
            .lock()
            .unwrap()
            .iter()
            .map(|o| match o {
                Observation::Started { .. } => "started",
                Observation::Checkpointed { .. } => "checkpointed",
                Observation::Receipts { .. } => "receipts",
                Observation::HandoffBlocked { .. } => "handoff_blocked",
                Observation::Continued { .. } => "continued",
                Observation::Exited { .. } => "exited",
                _ => "other",
            })
            .collect()
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while !done() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn ended(b: &Bench, n: u8) {
        let runner = Arc::clone(&b.runner);
        let attempt = token(n).attempt;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while runner.attempts.lock().await.contains_key(&attempt) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the attempt did not end"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn exited(host: &FakeHost) -> Option<ExitOutcome> {
        host.reports.lock().unwrap().iter().find_map(|o| match o {
            Observation::Exited { outcome, .. } => Some(*outcome),
            _ => None,
        })
    }

    #[tokio::test]
    async fn a_launch_is_idempotent_by_attempt() {
        let b = bench(Script::default());
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            LaunchAnswer::Accepted
        );
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            refused(LaunchRefusal::AlreadyLaunched)
        );
        until("started", || names(&b.host).contains(&"started")).await;
        // A runner made again over the same records, as after a restart,
        // still knows the attempt was taken.
        let again = Arc::new(Runner::new(
            Arc::clone(b.runner.executor()),
            b.dir.path().join("taken"),
        ));
        again.bind(b.host.clone());
        assert_eq!(
            again.launch(None, request(1)).await,
            refused(LaunchRefusal::AlreadyLaunched)
        );
        assert_eq!(b.runner.counters().launches, 1);
        assert_eq!(
            b.host.harness_pids.lock().unwrap().len(),
            1,
            "one harness started"
        );
        b.runner
            .instruct(
                None,
                token(1),
                Instruction::Stop {
                    reason: "done".into(),
                },
            )
            .await;
        ended(&b, 1).await;
    }

    #[tokio::test]
    async fn a_launch_of_a_superseded_generation_is_refused() {
        let b = bench(Script::default());
        let newer = Generation::FIRST.next().unwrap();
        b.runner.executor().supersede(JOB, newer).await;
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            refused(LaunchRefusal::Superseded { current: newer })
        );
        assert!(b.host.harness_pids.lock().unwrap().is_empty());
        assert_eq!(b.runner.counters().launches, 0);
    }

    #[tokio::test]
    async fn a_stale_or_unknown_heartbeat_verdict_stops_the_attempt() {
        for verdict in [
            GrantVerdict::Stale {
                current: Generation::FIRST.next().unwrap(),
            },
            GrantVerdict::Unknown,
        ] {
            let b = bench(Script {
                harness_secs: 30,
                ..Script::default()
            });
            let mut req = request(1);
            req.tasks.push(test_task(2, &["sleep", "30"]));
            assert_eq!(b.runner.launch(None, req).await, LaunchAnswer::Accepted);
            until("started", || names(&b.host).contains(&"started")).await;
            let harness = b.host.harness_pids.lock().unwrap()[0];
            assert!(alive(harness));
            b.host.script.lock().unwrap().verdict = Some(verdict);
            ended(&b, 1).await;
            assert!(!alive(harness), "the harness's group was stopped");
            assert_eq!(names(&b.host), vec!["started"], "nothing more is reported");
            assert_eq!(b.runner.counters().stale_obeyed, 1);
        }
    }

    #[tokio::test]
    async fn a_checkpoint_instruction_cuts_then_resumes_or_ends() {
        let b = bench(Script {
            harness_secs: 30,
            ..Script::default()
        });
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            LaunchAnswer::Accepted
        );
        until("started", || names(&b.host).contains(&"started")).await;
        let harness = b.host.harness_pids.lock().unwrap()[0];
        assert_eq!(
            b.runner
                .instruct(
                    None,
                    token(1),
                    Instruction::Checkpoint {
                        then: AfterCut::Resume
                    }
                )
                .await,
            InstructAnswer::Accepted
        );
        until("the cut", || names(&b.host).contains(&"checkpointed")).await;
        let cut = b.host.reports.lock().unwrap().iter().find_map(|o| match o {
            Observation::Checkpointed { cut, .. } => Some(cut.clone()),
            _ => None,
        });
        let cut = cut.unwrap();
        assert_eq!(
            cut.receipts.len(),
            1,
            "the cut carries the harness's receipt"
        );
        assert!(matches!(
            cut.receipts[0].status,
            ReceiptStatus::Checkpointed { .. }
        ));
        assert!(alive(harness), "a resume leaves the harness running");
        // Resumed: a spawn is taken again.
        until("resumed", || {
            !b.runner.attempts.try_lock().is_ok_and(|a| {
                a.get(&token(1).attempt)
                    .is_some_and(|c| c.busy.load(Ordering::Acquire))
            })
        })
        .await;
        assert_eq!(
            b.runner
                .instruct(None, token(1), Instruction::Spawn(test_task(3, &["true"])))
                .await,
            InstructAnswer::Accepted
        );
        assert_eq!(
            b.runner
                .instruct(
                    None,
                    token(1),
                    Instruction::Checkpoint {
                        then: AfterCut::End
                    }
                )
                .await,
            InstructAnswer::Accepted
        );
        ended(&b, 1).await;
        assert_eq!(
            names(&b.host),
            vec!["started", "checkpointed", "checkpointed"]
        );
        assert!(!alive(harness), "an end stops what still runs");
        assert_eq!(
            *b.host.alive_at_cut.lock().unwrap(),
            vec![true, false],
            "an end's cut is reported only once the harness is stopped"
        );
    }

    /// A task of the launch whose program does not start is reported
    /// `Blocked` with why, so it is never left open with no record.
    #[tokio::test]
    async fn a_launch_task_that_does_not_start_is_reported_blocked() {
        let b = bench(Script {
            harness_secs: 30,
            ..Script::default()
        });
        let mut req = request(1);
        req.tasks
            .push(test_task(2, &["/nonexistent/fabric-no-such-program"]));
        assert_eq!(b.runner.launch(None, req).await, LaunchAnswer::Accepted);
        until("the receipt", || names(&b.host).contains(&"receipts")).await;
        let receipts = b.host.reports.lock().unwrap().iter().find_map(|o| match o {
            Observation::Receipts { receipts, .. } => Some(receipts.clone()),
            _ => None,
        });
        let receipts = receipts.unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].task, TaskId::from_bytes([2; 16]));
        assert_eq!(receipts[0].class, TaskClass::Test);
        assert!(matches!(receipts[0].status, ReceiptStatus::Blocked { .. }));
        b.runner
            .instruct(
                None,
                token(1),
                Instruction::Stop {
                    reason: "done".into(),
                },
            )
            .await;
        ended(&b, 1).await;
    }

    #[tokio::test]
    async fn a_stop_instruction_reports_every_task_interrupted_then_cancelled() {
        let b = bench(Script {
            harness_secs: 30,
            ..Script::default()
        });
        let mut req = request(1);
        req.tasks.push(test_task(2, &["sleep", "30"]));
        assert_eq!(b.runner.launch(None, req).await, LaunchAnswer::Accepted);
        until("started", || names(&b.host).contains(&"started")).await;
        until("the test runs", || {
            b.runner
                .executor()
                .pids(token(1).attempt)
                .now_or_never_len()
                == 2
        })
        .await;
        assert_eq!(
            b.runner
                .instruct(
                    None,
                    token(1),
                    Instruction::Stop {
                        reason: "cancelled".into()
                    }
                )
                .await,
            InstructAnswer::Accepted
        );
        ended(&b, 1).await;
        assert_eq!(names(&b.host), vec!["started", "receipts", "exited"]);
        let receipts = b.host.reports.lock().unwrap().iter().find_map(|o| match o {
            Observation::Receipts { receipts, .. } => Some(receipts.clone()),
            _ => None,
        });
        let receipts = receipts.unwrap();
        assert_eq!(receipts.len(), 2);
        assert!(
            receipts
                .iter()
                .all(|r| r.status == ReceiptStatus::Interrupted && r.interrupted),
            "{receipts:?}"
        );
        assert_eq!(exited(&b.host), Some(ExitOutcome::Cancelled));
        assert_eq!(
            b.runner
                .instruct(
                    None,
                    token(1),
                    Instruction::Stop {
                        reason: "again".into()
                    }
                )
                .await,
            instruct_refused(InstructRefusal::NotRunning)
        );
    }

    #[tokio::test]
    async fn a_report_refused_stale_stops_the_attempt() {
        let b = bench(Script {
            harness_secs: 30,
            refuse_stale: true,
            ..Script::default()
        });
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            LaunchAnswer::Accepted
        );
        ended(&b, 1).await;
        let harness = b.host.harness_pids.lock().unwrap()[0];
        assert!(!alive(harness));
        assert_eq!(names(&b.host), vec!["started"]);
        assert_eq!(b.runner.counters().reports_refused, 1);
        assert_eq!(b.runner.counters().stale_obeyed, 1);
    }

    #[tokio::test]
    async fn a_preparation_failure_is_reported_blocked_then_failed() {
        let b = bench(Script {
            prepare_fails: Some("git clone failed".into()),
            ..Script::default()
        });
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            LaunchAnswer::Accepted
        );
        ended(&b, 1).await;
        assert_eq!(names(&b.host), vec!["receipts", "exited"]);
        let receipts = b.host.reports.lock().unwrap().iter().find_map(|o| match o {
            Observation::Receipts { receipts, .. } => Some(receipts.clone()),
            _ => None,
        });
        assert_eq!(
            receipts.unwrap()[0].status,
            ReceiptStatus::Blocked {
                reason: "git clone failed".into()
            }
        );
        assert_eq!(exited(&b.host), Some(ExitOutcome::Failed { code: None }));
        assert!(
            b.host.harness_pids.lock().unwrap().is_empty(),
            "nothing started"
        );
    }

    #[tokio::test]
    async fn an_unallowed_fidelity_starts_nothing() {
        let b = bench(Script {
            offered: vec![
                Fidelity::NewNativeSessionWithPackage,
                Fidelity::WorkspaceOnly,
            ],
            ..Script::default()
        });
        let mut req = request(1);
        req.continuation = Some(ContinuationAsk {
            from: AttemptId::from_bytes([5; 16]),
            allowed: AllowedFidelity {
                native_session: true,
                new_native_session_with_package: false,
                workspace_only: false,
            },
        });
        req.package = Some(package());
        assert_eq!(b.runner.launch(None, req).await, LaunchAnswer::Accepted);
        ended(&b, 1).await;
        assert_eq!(names(&b.host), vec!["receipts", "exited"], "never lowered");
        assert!(b.host.harness_pids.lock().unwrap().is_empty());
        assert_eq!(exited(&b.host), Some(ExitOutcome::Failed { code: None }));

        // An allowed fidelity starts the harness with the package and says so.
        let b = bench(Script {
            offered: vec![
                Fidelity::NewNativeSessionWithPackage,
                Fidelity::WorkspaceOnly,
            ],
            ..Script::default()
        });
        let mut req = request(2);
        req.continuation = Some(ContinuationAsk {
            from: AttemptId::from_bytes([5; 16]),
            allowed: AllowedFidelity::DEFAULT,
        });
        req.package = Some(package());
        assert_eq!(b.runner.launch(None, req).await, LaunchAnswer::Accepted);
        until("continued", || names(&b.host).contains(&"continued")).await;
        let continued = b.host.reports.lock().unwrap().iter().find_map(|o| match o {
            Observation::Continued {
                achieved, package, ..
            } => Some((*achieved, *package)),
            _ => None,
        });
        assert_eq!(
            continued,
            Some((
                Fidelity::NewNativeSessionWithPackage,
                Some(package().digest())
            ))
        );
        ended(&b, 2).await;
        assert_eq!(exited(&b.host), Some(ExitOutcome::Succeeded));
    }

    #[tokio::test]
    async fn reports_leave_in_order_and_wait_for_the_link() {
        let b = bench(Script {
            link_failures: 2,
            ..Script::default()
        });
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            LaunchAnswer::Accepted
        );
        ended(&b, 1).await;
        assert_eq!(names(&b.host), vec!["started", "receipts", "exited"]);
        let log = b.host.log.lock().unwrap().clone();
        // After a link failure a report goes again only after a heartbeat answered.
        for (i, entry) in log.iter().enumerate() {
            if *entry == "link-fail" {
                let next_send = log[i + 1..]
                    .iter()
                    .position(|e| *e != "heartbeat")
                    .map(|p| p + i + 1);
                if let Some(next) = next_send {
                    assert!(
                        log[i + 1..next].contains(&"heartbeat"),
                        "a report went again before a heartbeat answered: {log:?}"
                    );
                }
            }
        }
        assert_eq!(b.runner.counters().reports_sent, 3);
        assert!(b.runner.counters().heartbeats >= 2);
    }

    /// The pids of a running attempt without waiting on a lock another
    /// task holds.
    trait NowOrNever {
        fn now_or_never_len(self) -> usize;
    }

    impl<F: std::future::Future<Output = Vec<u32>>> NowOrNever for F {
        fn now_or_never_len(self) -> usize {
            futures_lite_now(self).map_or(0, |v| v.len())
        }
    }

    fn futures_lite_now<F: std::future::Future>(future: F) -> Option<F::Output> {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => Some(output),
            std::task::Poll::Pending => None,
        }
    }
}
