//! The attempts this node runs for an authority (docs/managed-jobs.md,
//! "Launch path", the node side): a `Launch` takes a grant, an `Instruct`
//! asks the running attempt to cut, stop or spawn, and every outcome goes
//! back as a `Report`, in order, one at a time per attempt.
//!
//! Invariant: a node acts on a grant only while it is the job's live
//! grant. A launch of a generation older than one this node knows is
//! refused `Superseded`; a heartbeat verdict or a report refusal that
//! ends the grant (the fence's `Stale`, `Superseded`, `WrongNode`,
//! `UnknownAttempt`, `UnknownJob`, or `NotWritable` for an ended attempt:
//! `FenceVerdict::ends_grant`, `ReportRefusal::ends_grant`) stops every
//! group of the attempt and reports nothing more. A launch is idempotent by attempt: its record is
//! written before the attempt starts, so a launch sent again, even after
//! this node restarted, is answered `AlreadyLaunched`. An answer says the
//! node took the message, never what came of it: the outcome is a report.
//! Nothing read from a message is trusted past its check: the request's
//! own bounds and every program are checked again here.
//!
//! What the runner counts: launches taken, reports sent and refused,
//! heartbeats and grant-ending verdicts obeyed. Time is the Perf step's alone.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::harness::HarnessKind;
use fabric_core::id::NodeId;
use fabric_core::managed::launch::{
    AfterCut, HEARTBEAT_MS, Heartbeat, HeartbeatAnswer, InstructAnswer, InstructRefusal,
    Instruction, LaunchAnswer, LaunchRefusal, LaunchRequest, Report, ReportAnswer, ReportRefusal,
    TaskLaunch,
};
use fabric_core::managed::placement::PathLayout;
use fabric_core::managed::{
    AllowedFidelity, AttemptId, DrainBudget, ExitOutcome, Fidelity, GrantToken, NodeAdvert,
    Observation, Program, ReceiptStatus, TaskClass, TaskReceipt,
};
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use fabric_core::version::HarnessVersion;
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
    /// Verdicts or refusals that ended a grant, obeyed by a stop.
    pub stale_obeyed: u64,
    /// Grants taken before a restart and presented after it.
    pub presented: u64,
}

/// The durable record of a launch taken, which answers a launch sent
/// again: the local marker of a grant this node ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Taken {
    pub(crate) token: GrantToken,
    /// The authority that granted it; `None` is this node's own.
    #[serde(default)]
    pub(crate) from: Option<NodeId>,
    /// How this node's part in the grant ended; `None` while it may run.
    #[serde(default)]
    pub(crate) ended: Option<TakenEnd>,
}

/// How a grant this node took ended here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TakenEnd {
    /// Its loop ran to its end on this node.
    Finished,
    /// The authority's verdict ended it; nothing more was reported.
    GrantEnded {
        /// The verdict's or the refusal's name.
        verdict: String,
    },
    /// This node restarted while it ran: it is never resumed here. The
    /// authority's answer when the grant was presented after the restart
    /// is kept; a grant still live there is left to fall silent, so the
    /// authority's own loss handling decides it (ADR 0028).
    Abandoned {
        /// The name of the verdict the presentation got.
        verdict: String,
    },
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
    presented: AtomicU64,
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
    /// The authority said the grant ended (a verdict or a refusal for
    /// which `ends_grant` holds), by that verdict's name; everything is
    /// stopped.
    Stale(&'static str),
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
            presented: AtomicU64::new(0),
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
            presented: self.presented.load(Ordering::Relaxed),
        }
    }

    /// The grants of the attempts this node runs now, and the attempts its
    /// executor holds open: what `gc` holds live on the launch path (ADR
    /// 0030). Read under the lock a launch takes before it writes
    /// anything of the attempt, so an attempt launched after this read
    /// left nothing a read before it saw.
    pub(crate) async fn running(&self) -> (Vec<GrantToken>, Vec<AttemptId>) {
        let tokens = self
            .attempts
            .lock()
            .await
            .values()
            .map(|ctl| ctl.token)
            .collect();
        (tokens, self.executor.open().await)
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
            .and_then(|()| {
                local::write(
                    &self.taken_path(token.attempt),
                    &Taken {
                        token,
                        from,
                        ended: None,
                    },
                )
            })
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
        let (stale_tx, stale) = watch::channel::<Option<&'static str>>(None);
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
                        if changed.is_err() {
                            return Err(Ended::Stale("unknown_job"));
                        }
                        if let Some(verdict) = *stale.borrow() {
                            return Err(Ended::Stale(verdict));
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
        let end = match &outcome {
            Err(Ended::Stale(verdict)) => {
                self.stale_obeyed.fetch_add(1, Ordering::Relaxed);
                let _ = self.executor.stop(attempt, budget).await;
                TakenEnd::GrantEnded {
                    verdict: (*verdict).to_owned(),
                }
            }
            Ok(()) => TakenEnd::Finished,
        };
        self.mark_ended(token, from, end);
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
        let quiescence = match self.executor.quiesce(attempt, budget, then).await {
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
            AfterCut::End => Some(
                self.executor
                    .stop(attempt, budget)
                    .await
                    .unwrap_or_default(),
            ),
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
        if matches!(answer, ReportAnswer::Recorded { .. }) {
            // Only a recorded cut is a departure: a refused one is never
            // continued from, so its state must not count at a restore here.
            if let Err(e) = self.executor.confirm_departure(attempt).await {
                tracing::warn!(%attempt, error = %e, "the departure of the cut's harness state was not confirmed");
            }
        }
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

/// The name a report refusal is recorded under in a taken marker.
fn refusal_name(reason: &ReportRefusal) -> &'static str {
    match reason {
        ReportRefusal::StaleGeneration { .. } => "stale",
        ReportRefusal::UnknownJob => "unknown_job",
        ReportRefusal::UnknownAttempt => "unknown_attempt",
        ReportRefusal::WrongNode => "wrong_node",
        ReportRefusal::Superseded { .. } => "superseded",
        ReportRefusal::NotWritable { .. } => "not_writable",
        ReportRefusal::TokenMismatch => "token_mismatch",
        ReportRefusal::NotReportable => "not_reportable",
        ReportRefusal::Domain { .. } => "domain",
        ReportRefusal::Unavailable => "unavailable",
    }
}

impl Runner {
    /// Writes how this node's part in `token`'s grant ended into its
    /// marker; a marker that cannot be written is logged, never retried
    /// into a second run.
    fn mark_ended(&self, token: GrantToken, from: Option<NodeId>, end: TakenEnd) {
        let marker = Taken {
            token,
            from,
            ended: Some(end),
        };
        if let Err(e) = local::write(&self.taken_path(token.attempt), &marker) {
            tracing::warn!(attempt = %token.attempt, error = %e, "the taken marker was not updated");
        }
    }

    /// After a restart: every grant this node had taken and not seen end
    /// is presented to its authority in a heartbeat until the authority
    /// answers it, and its marker records the answer. None is resumed: the
    /// node's handles on its processes died with the previous run, and
    /// what that run left running was stopped before this one started
    /// (`Executor::recover`). A grant the authority still holds live is
    /// left to fall silent, so its loss handling and recovery policy
    /// decide it (ADR 0028). Returns the markers it wrote, in attempt order.
    pub(crate) async fn present_taken(&self) -> Vec<Taken> {
        let Some(host) = self.host.get().cloned() else {
            return Vec::new();
        };
        let mut open: Vec<Taken> = match std::fs::read_dir(&self.taken) {
            Ok(entries) => {
                let mut found: Vec<Taken> = entries
                    .flatten()
                    .filter_map(|e| local::read::<Taken>(&e.path()).ok().flatten())
                    .filter(|t| t.ended.is_none())
                    .collect();
                found.sort_by_key(|t| t.token.attempt);
                found
            }
            Err(_) => Vec::new(),
        };
        let running = self.attempts.lock().await;
        open.retain(|t| !running.contains_key(&t.token.attempt));
        drop(running);
        let mut written = Vec::new();
        let mut interval = tokio::time::interval(self.heartbeat);
        while !open.is_empty() {
            interval.tick().await;
            let mut still = Vec::new();
            for taken in open {
                let answer = host
                    .heartbeat(
                        taken.from,
                        Heartbeat {
                            attempts: vec![taken.token],
                            running: false,
                        },
                    )
                    .await;
                let verdict = answer.ok().and_then(|a| {
                    a.verdicts
                        .into_iter()
                        .find(|(attempt, _)| *attempt == taken.token.attempt)
                        .map(|(_, v)| v)
                });
                let Some(verdict) = verdict else {
                    still.push(taken);
                    continue;
                };
                self.presented.fetch_add(1, Ordering::Relaxed);
                let end = if verdict.ends_grant() {
                    TakenEnd::GrantEnded {
                        verdict: verdict.name().to_owned(),
                    }
                } else {
                    TakenEnd::Abandoned {
                        verdict: verdict.name().to_owned(),
                    }
                };
                tracing::info!(attempt = %taken.token.attempt, verdict = verdict.name(), "a grant taken before the restart was presented; it is not resumed");
                self.mark_ended(taken.token, taken.from, end.clone());
                written.push(Taken {
                    ended: Some(end),
                    ..taken
                });
            }
            open = still;
        }
        written
    }
}

/// Where a continuation's package is written: beside the root.
fn package_path(root: &AbsPath, attempt: AttemptId) -> PathBuf {
    let root = std::path::Path::new(root.as_str());
    root.parent()
        .unwrap_or(root)
        .join(format!("package-{attempt}.json"))
}

/// Step 7: a heartbeat every interval; a verdict that ends the grant
/// (`FenceVerdict::ends_grant`) ends the attempt, any other answer lets
/// a waiting report go again.
async fn heartbeat_loop(
    runner: Arc<Runner>,
    host: Arc<dyn Host>,
    to: Option<NodeId>,
    token: GrantToken,
    stale: watch::Sender<Option<&'static str>>,
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
                    running: true,
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
            .map(|(_, verdict)| verdict);
        match verdict {
            Some(verdict) if verdict.ends_grant() => {
                tracing::info!(attempt = %token.attempt, verdict = verdict.name(), "the grant ended; stopping the attempt");
                let _ = stale.send(Some(verdict.name()));
                return;
            }
            // Live, `unknown` (the authority not knowing), or an answer
            // that names nothing of this grant, which says nothing of it:
            // a waiting report goes again.
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
    stale: watch::Receiver<Option<&'static str>>,
}

impl Reporter<'_> {
    async fn send(&self, observation: Observation) -> Result<ReportAnswer, Ended> {
        let mut stale = self.stale.clone();
        loop {
            if let Some(verdict) = *stale.borrow() {
                return Err(Ended::Stale(verdict));
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
                        if reason.ends_grant() {
                            return Err(Ended::Stale(refusal_name(reason)));
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

/// The harness state a cut holds, as its manifest names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedSession {
    /// The harness that wrote it.
    pub harness: HarnessKind,
    /// The session it continues.
    pub session: SessionId,
    /// The root and home it was captured under (ADR 0011).
    pub layout: PathLayout,
    /// The version that last wrote it, when the manifest names one.
    pub writer: Option<HarnessVersion>,
}

/// What this node offers a continuation of `harness` from a cut that holds
/// `captured`, with `advert` its own and `here` its physical root and home,
/// and the session it restores for it: one only when the continuation
/// allows a native session and this node can offer one. The layout is the
/// same only when the harness, the root and the home all are (ADR 0011).
pub(crate) fn continuation_offer(
    harness: &HarnessKind,
    allowed: AllowedFidelity,
    captured: Option<&CapturedSession>,
    here: &PathLayout,
    advert: &NodeAdvert,
) -> (Vec<Fidelity>, Option<SessionId>) {
    let same = captured.filter(|c| c.harness == *harness && c.layout == *here);
    let writer = same.and_then(|c| c.writer.as_ref());
    let offered = fabric_core::managed::offered_fidelity(harness, writer, advert, same.is_some());
    let restore = same
        .filter(|_| allowed.native_session && offered.contains(&Fidelity::NativeSession))
        .map(|c| c.session.clone());
    (offered, restore)
}

/// Reads the harness state a restore wrote to `dir`; `None` when the cut
/// held none, which leaves `dir` absent or empty. A state that is there
/// and cannot be read is an error.
fn captured_in(dir: &std::path::Path) -> Result<Option<CapturedSession>, String> {
    match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
        Ok(mut entries) => {
            if entries.next().is_none() {
                return Ok(None);
            }
        }
    }
    let manifest = fabric_harness::manifest::StateManifest::read(dir)
        .map_err(|e| format!("the cut's harness state cannot be read: {e}"))?;
    Ok(Some(CapturedSession {
        harness: manifest.kind().clone(),
        session: manifest.session().clone(),
        layout: PathLayout {
            home: manifest.home().clone(),
            root: manifest.root().clone(),
        },
        writer: manifest
            .harness_version()
            .and_then(|v| HarnessVersion::parse(v).ok()),
    }))
}

/// Removes `dir`, this node's own copy of a cut's harness state.
fn remove_state(dir: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {e}", dir.display()))
        }
        _ => Ok(()),
    }
}

/// What this node offers the continuation `ask` of `request` from the
/// root it prepared, and the session it restored for a native session:
/// the cut's harness state in `state_dir` goes back through the adapter's
/// restore, as `job resume` does, when the layout is the same. A refusal
/// there fails the launch, never lowers the fidelity.
async fn native_offer(
    node: &crate::node::Node,
    request: &LaunchRequest,
    ask: &fabric_core::managed::launch::ContinuationAsk,
    root: &AbsPath,
    state_dir: Option<&std::path::Path>,
) -> Result<(Vec<Fidelity>, Option<SessionId>), String> {
    let wire = fabric_net::rpc::Advertise {
        job: request.token.job,
        harness: request.intent.harness.clone(),
        root: root.clone(),
        snapshot: request.initial_checkpoint,
        secrets: Vec::new(),
    };
    let advert = crate::placement::advert_from_wire(&node.own_advert(&wire).await);
    let harness_env = node
        .harness_env(&node.node_env())
        .map_err(|e| e.to_string())?;
    let physical = std::fs::canonicalize(root.as_str())
        .ok()
        .and_then(|p| AbsPath::parse(p.to_str()?).ok())
        .ok_or_else(|| format!("{root} has no physical path"))?;
    let here = PathLayout {
        home: harness_env.home.clone(),
        root: physical,
    };
    offer_restoring(
        &request.intent.harness,
        ask.allowed,
        state_dir,
        std::path::Path::new(root.as_str()),
        &here,
        &advert,
        || {
            crate::harnesses::adapter(&request.intent.harness, &harness_env, None)
                .map_err(|e| e.to_string())
        },
    )
    .await
}

/// The offer of a node whose layout is `here` for a continuation from a
/// cut whose harness state is in `state_dir`, and the session it restored
/// for the workspace at `workspace` through the adapter `adapter` builds. A state that cannot be
/// read, or a restore that refuses, is an error: never a lower fidelity.
async fn offer_restoring(
    harness: &HarnessKind,
    allowed: AllowedFidelity,
    state_dir: Option<&std::path::Path>,
    workspace: &std::path::Path,
    here: &PathLayout,
    advert: &NodeAdvert,
    adapter: impl FnOnce() -> Result<Box<dyn fabric_harness::Harness>, String>,
) -> Result<(Vec<Fidelity>, Option<SessionId>), String> {
    let captured = match state_dir {
        Some(dir) => captured_in(dir)?,
        None => None,
    };
    let (offered, session) = continuation_offer(harness, allowed, captured.as_ref(), here, advert);
    if let (Some(_), Some(dir)) = (&session, state_dir) {
        adapter()?
            .restore(workspace, dir)
            .await
            .map_err(|e| format!("the harness state was not restored: {e}"))?;
    }
    Ok((offered, session))
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
            // A continuation that allows a native session takes the cut's
            // harness state too, into this node's own copy of it.
            let native = request.initial_checkpoint.is_some()
                && request
                    .continuation
                    .as_ref()
                    .is_some_and(|ask| ask.allowed.native_session);
            let state_dir = native.then(|| {
                node.cfg
                    .state
                    .root()
                    .join("runs")
                    .join("restored")
                    .join(request.token.attempt.to_string())
            });
            if let Some(dir) = &state_dir {
                remove_state(dir)?;
                if let Some(parent) = dir.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("{}: {e}", parent.display()))?;
                }
            }
            let prepared = crate::sources::prepare(&node, job, &source, state_dir.clone())
                .await
                .map_err(|e| e.to_string())?;
            let root = match prepared {
                crate::sources::Prepared::Cloned { root, .. }
                | crate::sources::Prepared::Restored { root, .. }
                | crate::sources::Prepared::Empty { root } => root,
            };
            let Some(ask) = &request.continuation else {
                return Ok(Prepared {
                    root,
                    offered: Vec::new(),
                    session: None,
                });
            };
            let offer = native_offer(&node, request, ask, &root, state_dir.as_deref()).await;
            if let Some(dir) = &state_dir {
                remove_state(dir)?;
            }
            let (offered, session) = offer?;
            Ok(Prepared {
                root,
                offered,
                session,
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
                    HarnessKind::OpenCode => {
                        Some(fabric_harness::opencode::OpenCode::read_headless_line)
                    }
                    HarnessKind::Pi => Some(fabric_harness::pi::Pi::read_headless_line),
                    HarnessKind::GeminiCli => {
                        Some(fabric_harness::gemini::GeminiCli::read_headless_line)
                    }
                    HarnessKind::CursorAgent => {
                        Some(fabric_harness::cursor::CursorAgent::read_headless_line)
                    }
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
                    let span = node.trace.on().then(|| {
                        crate::otel::hooks::launch_begin(
                            crate::otel::SpanOp::REPORT,
                            &report.token,
                            Some(peer),
                        )
                    });
                    let session = node.session(peer).await.map_err(|e| e.to_string());
                    let answer = match session {
                        Ok(session) => session
                            .report(report, LINK_DEADLINE)
                            .await
                            .map_err(|e| e.to_string()),
                        Err(e) => Err(e),
                    };
                    if let Some(mut span) = span {
                        match &answer {
                            Ok(ReportAnswer::Refused { reason }) => {
                                crate::otel::hooks::answer_end(&mut span, Some(reason));
                            }
                            Ok(ReportAnswer::Recorded { .. }) => {
                                crate::otel::hooks::answer_end::<()>(&mut span, None);
                            }
                            Err(_) => span.failed(
                                crate::otel::SpanResult::Unknown,
                                crate::otel::Code::fixed("Unreachable"),
                            ),
                        }
                        node.trace.emit(span);
                    }
                    answer
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

/// Builds the adapter of a harness on this node.
type AdapterFor =
    dyn Fn(&HarnessKind) -> Result<Box<dyn fabric_harness::Harness>, String> + Send + Sync;

/// The adapters' suspend path, for the capturer: the capture `job
/// suspend` makes, and the confirmation of its departure record.
pub(crate) struct AdapterSuspend {
    adapter: Box<AdapterFor>,
}

impl AdapterSuspend {
    pub(crate) fn new(node: std::sync::Weak<crate::node::Node>) -> Self {
        Self::with(move |harness| {
            let node = node
                .upgrade()
                .ok_or_else(|| "the node is stopping".to_owned())?;
            let env = node
                .harness_env(&node.node_env())
                .map_err(|e| e.to_string())?;
            crate::harnesses::adapter(harness, &env, None).map_err(|e| e.to_string())
        })
    }

    /// The suspend path over the adapters `adapter` builds.
    pub(crate) fn with(
        adapter: impl Fn(&HarnessKind) -> Result<Box<dyn fabric_harness::Harness>, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        AdapterSuspend {
            adapter: Box::new(adapter),
        }
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
            let adapter = (self.adapter)(&attempt.harness)?;
            adapter
                .capture(std::path::Path::new(attempt.root.as_str()), session, out)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }

    fn confirm<'a>(
        &'a self,
        attempt: &'a crate::capturer::Attempt,
        dir: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let manifest = fabric_harness::manifest::StateManifest::read(dir)
                .map_err(|e| format!("the cut's harness state cannot be read: {e}"))?;
            let adapter = (self.adapter)(&attempt.harness)?;
            match adapter.confirm_departure(&manifest).await {
                Ok(()) | Err(fabric_harness::HarnessError::DepartureNotPending { .. }) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
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
        AllowedFidelity, EventDigest, FenceVerdict, Generation, Intent, JobPolicy, RestartPolicy,
        SourceBinding, TaskId, WorkPackage, WorkRevision, WorkspaceRevision,
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
            _run: crate::executor::HarnessRun,
        ) -> BoxFuture<'_, Result<SnapshotId, String>> {
            Box::pin(async move { Ok(SnapshotId::of(&attempt.to_bytes())) })
        }
        fn capture(&self, attempt: AttemptId) -> BoxFuture<'_, Result<SnapshotId, String>> {
            Box::pin(async move { Ok(SnapshotId::of(&[attempt.to_bytes(), [1; 16]].concat())) })
        }
        fn confirm_departure(&self, _attempt: AttemptId) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
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
        verdict: Option<FenceVerdict>,
        /// A verdict for a named attempt, before `verdict`.
        verdicts_by_attempt: Vec<(AttemptId, FenceVerdict)>,
        /// What every report is refused with.
        refuse: Option<ReportRefusal>,
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
        /// What happened, in order: `report`, `link-fail`, `heartbeat`,
        /// `presentation` (a heartbeat that does not run its grants).
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
                if let Some(reason) = script.refuse.clone() {
                    return Ok(ReportAnswer::Refused { reason });
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
                self.log.lock().unwrap().push(if beat.running {
                    "heartbeat"
                } else {
                    "presentation"
                });
                let script = self.script.lock().unwrap();
                let verdict = script.verdict.clone().unwrap_or(FenceVerdict::Live);
                Ok(HeartbeatAnswer {
                    verdicts: beat
                        .attempts
                        .iter()
                        .map(|t| {
                            let named = script
                                .verdicts_by_attempt
                                .iter()
                                .find(|(a, _)| *a == t.attempt)
                                .map(|(_, v)| v.clone());
                            (t.attempt, named.unwrap_or_else(|| verdict.clone()))
                        })
                        .collect(),
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

    /// Every fence verdict that ends the grant stops the attempt: the
    /// runner obeys the one verdict type the authority answers with.
    #[tokio::test]
    async fn every_heartbeat_verdict_that_ends_the_grant_stops_the_attempt() {
        for verdict in [
            FenceVerdict::Stale {
                current: Generation::FIRST.next().unwrap(),
            },
            FenceVerdict::Superseded { by: None },
            FenceVerdict::Superseded {
                by: Generation::FIRST.next(),
            },
            FenceVerdict::NotWritable {
                status: fabric_core::managed::AttemptStatus::Lost,
            },
            FenceVerdict::WrongNode {
                granted: NodeId::from_bytes([1; 32]),
                from: NodeId::from_bytes([2; 32]),
            },
            FenceVerdict::UnknownAttempt,
            FenceVerdict::UnknownJob,
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

    /// After a restart, every grant taken and not seen end is presented to
    /// its authority and never resumed: a superseded one is marked ended by
    /// the verdict, a live one abandoned, and neither starts a harness or
    /// sends a report; a marker that ended is not presented again, and the
    /// grant is not taken a second time.
    #[tokio::test]
    async fn a_restarted_runner_presents_its_taken_grants_and_resumes_none() {
        let b = bench(Script::default());
        let superseded = token(1);
        let live = token(2);
        let finished = token(3);
        std::fs::create_dir_all(b.dir.path().join("taken")).unwrap();
        for (t, ended) in [
            (superseded, None),
            (live, None),
            (finished, Some(TakenEnd::Finished)),
        ] {
            local::write(
                &b.dir
                    .path()
                    .join("taken")
                    .join(format!("{}.json", t.attempt)),
                &Taken {
                    token: t,
                    from: None,
                    ended,
                },
            )
            .unwrap();
        }
        // The authority answers the first superseded and the second live.
        b.host.script.lock().unwrap().verdicts_by_attempt = vec![
            (superseded.attempt, FenceVerdict::Superseded { by: None }),
            (live.attempt, FenceVerdict::Live),
        ];
        let written = b.runner.present_taken().await;
        let ends: Vec<_> = written.iter().map(|t| (t.token, t.ended.clone())).collect();
        assert_eq!(
            ends,
            vec![
                (
                    superseded,
                    Some(TakenEnd::GrantEnded {
                        verdict: "superseded".into()
                    })
                ),
                (
                    live,
                    Some(TakenEnd::Abandoned {
                        verdict: "live".into()
                    })
                ),
            ]
        );
        assert_eq!(b.runner.counters().presented, 2);
        assert!(
            b.host.harness_pids.lock().unwrap().is_empty(),
            "nothing resumed"
        );
        // Presented as not running, so the authority never takes it as
        // presence (review finding on PR #39).
        let log = b.host.log.lock().unwrap().clone();
        assert!(log.contains(&"presentation"), "{log:?}");
        assert!(!log.contains(&"heartbeat"), "{log:?}");
        assert!(!names(&b.host).contains(&"report"), "nothing reported");
        // The markers say so on disk, and a second start presents nothing.
        let marker: Taken = local::read(
            &b.dir
                .path()
                .join("taken")
                .join(format!("{}.json", superseded.attempt)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            marker.ended,
            Some(TakenEnd::GrantEnded {
                verdict: "superseded".into()
            })
        );
        assert!(b.runner.present_taken().await.is_empty());
        // The grant is not taken again.
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            refused(LaunchRefusal::AlreadyLaunched)
        );
    }

    /// A `NotWritable { Unknown }` heartbeat verdict is the authority not
    /// knowing: the attempt keeps running and its reports wait.
    #[tokio::test]
    async fn an_unknown_heartbeat_verdict_keeps_the_attempt() {
        let b = bench(Script {
            harness_secs: 30,
            ..Script::default()
        });
        assert_eq!(
            b.runner.launch(None, request(1)).await,
            LaunchAnswer::Accepted
        );
        until("started", || names(&b.host).contains(&"started")).await;
        b.host.script.lock().unwrap().verdict = Some(FenceVerdict::NotWritable {
            status: fabric_core::managed::AttemptStatus::Unknown,
        });
        let seen = b.runner.counters().heartbeats;
        until("two more heartbeats", || {
            b.runner.counters().heartbeats >= seen + 2
        })
        .await;
        let harness = b.host.harness_pids.lock().unwrap()[0];
        assert!(alive(harness), "the attempt was stopped on unknown");
        assert_eq!(b.runner.counters().stale_obeyed, 0);
        b.runner
            .instruct(
                None,
                token(1),
                Instruction::Stop {
                    reason: "end of test".into(),
                },
            )
            .await;
        ended(&b, 1).await;
    }

    /// Every report refusal that ends the grant stops the attempt, and
    /// one that says nothing of the grant does not.
    #[tokio::test]
    async fn every_report_refusal_that_ends_the_grant_stops_the_attempt() {
        for reason in [
            ReportRefusal::StaleGeneration {
                current: Generation::FIRST.next().unwrap(),
            },
            ReportRefusal::Superseded { by: None },
            ReportRefusal::NotWritable {
                status: fabric_core::managed::AttemptStatus::Lost,
            },
            ReportRefusal::WrongNode,
            ReportRefusal::UnknownAttempt,
            ReportRefusal::UnknownJob,
        ] {
            let b = bench(Script {
                harness_secs: 30,
                refuse: Some(reason.clone()),
                ..Script::default()
            });
            assert_eq!(
                b.runner.launch(None, request(1)).await,
                LaunchAnswer::Accepted
            );
            ended(&b, 1).await;
            let harness = b.host.harness_pids.lock().unwrap()[0];
            assert!(!alive(harness), "{reason:?} left the harness running");
            assert_eq!(names(&b.host), vec!["started"], "{reason:?}");
            assert_eq!(b.runner.counters().reports_refused, 1);
            assert_eq!(b.runner.counters().stale_obeyed, 1, "{reason:?}");
        }
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

    /// A native session is offered, and its session restored, only for the
    /// same harness, root and home, a version not older than its writer,
    /// and a continuation that allows it; anything else offers the package.
    #[test]
    fn a_native_session_is_restored_only_for_the_same_layout() {
        let abs = |p: &str| AbsPath::parse(p).unwrap();
        let here = PathLayout {
            home: abs("/home/u"),
            root: abs("/home/u/fabric/jobs/j/work"),
        };
        let advert = codex_advert(&here);
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let captured = CapturedSession {
            harness: HarnessKind::Codex,
            session: session.clone(),
            layout: here.clone(),
            writer: Some(HarnessVersion::parse("0.155.1").unwrap()),
        };
        let allowed = AllowedFidelity::DEFAULT;
        let offer = |captured: Option<&CapturedSession>, allowed: AllowedFidelity| {
            continuation_offer(&HarnessKind::Codex, allowed, captured, &here, &advert)
        };
        let package = vec![
            Fidelity::NewNativeSessionWithPackage,
            Fidelity::WorkspaceOnly,
        ];

        let (offered, restored) = offer(Some(&captured), allowed);
        assert_eq!(offered[0], Fidelity::NativeSession);
        assert_eq!(restored, Some(session.clone()));
        assert_eq!(allowed.highest(&offered).unwrap(), Fidelity::NativeSession);

        for other in [
            CapturedSession {
                layout: PathLayout {
                    home: abs("/home/v"),
                    ..here.clone()
                },
                ..captured.clone()
            },
            CapturedSession {
                layout: PathLayout {
                    root: abs("/home/u/elsewhere"),
                    ..here.clone()
                },
                ..captured.clone()
            },
            CapturedSession {
                harness: HarnessKind::ClaudeCode,
                ..captured.clone()
            },
            CapturedSession {
                writer: Some(HarnessVersion::parse("0.200.0").unwrap()),
                ..captured.clone()
            },
        ] {
            assert_eq!(
                offer(Some(&other), allowed),
                (package.clone(), None),
                "{other:?}"
            );
        }
        assert_eq!(
            offer(None, allowed),
            (package.clone(), None),
            "no state in the cut"
        );
        let no_native = AllowedFidelity {
            native_session: false,
            ..AllowedFidelity::DEFAULT
        };
        let (offered, restored) = offer(Some(&captured), no_native);
        assert_eq!(
            restored, None,
            "a native session not allowed restores nothing"
        );
        assert_eq!(
            no_native.highest(&offered).unwrap(),
            Fidelity::NewNativeSessionWithPackage
        );
    }

    /// The advert of a node with Codex 0.155.1 and the layout `here`.
    fn codex_advert(here: &PathLayout) -> NodeAdvert {
        use fabric_core::managed::placement::{Capacity, HarnessInventory, Warmth};
        use fabric_core::snapshot::Platform;

        NodeAdvert {
            node: NodeId::from_bytes([2; 32]),
            platform: Platform {
                os: "macos".to_owned(),
                arch: "aarch64".to_owned(),
            },
            harnesses: vec![HarnessInventory {
                kind: HarnessKind::Codex,
                version: HarnessVersion::parse("0.155.1").unwrap(),
            }],
            layout: here.clone(),
            capacity: Capacity {
                load_1m_milli: 0,
                free_disk_bytes: 0,
            },
            warmth: Warmth {
                bundle_chain: false,
                workspace: false,
            },
            measured_at_ms: 0,
        }
    }

    /// A Codex session captured on a node with home `home` and root
    /// `work`, its state in `out`, with the adapter that captured it.
    async fn codex_state(
        home: &std::path::Path,
        work: &std::path::Path,
        out: &std::path::Path,
    ) -> (PathBuf, fabric_harness::codex::Codex) {
        use fabric_harness::Harness;

        const THREAD: &str = "019f233e-bb33-7342-85fa-f2d33b14368a";
        let day = home.join(".codex/sessions/2026/10/19");
        std::fs::create_dir_all(&day).unwrap();
        let rollout = day.join(format!("rollout-2026-10-19T11-00-00-{THREAD}.jsonl"));
        std::fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{THREAD}\",\"cwd\":\"{}\",\"cli_version\":\"0.155.1\"}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"hi\"}}]}}}}\n",
                work.display()
            ),
        )
        .unwrap();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let codex = bin.join("codex");
        std::fs::write(
            &codex,
            "#!/bin/sh\n[ \"$1\" = --version ] && echo \"codex-cli 0.155.1\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&codex, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let adapter = fabric_harness::codex::Codex::new(
            AbsPath::parse(home.to_str().unwrap()).unwrap(),
            bin.into_os_string(),
        );
        adapter
            .capture(work, &SessionId::parse(THREAD).unwrap(), out)
            .await
            .unwrap();
        (rollout, adapter)
    }

    /// A native session the layout allows is restored through the
    /// adapter; when the adapter refuses, or the cut's state cannot be
    /// read, the launch fails: the continuation never takes a lower
    /// fidelity than the one it was offered.
    #[tokio::test]
    async fn a_restore_failure_fails_the_launch_never_a_lower_fidelity() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        let work = home.join("w");
        std::fs::create_dir_all(&work).unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let (rollout, adapter) = codex_state(&home, &work, &state).await;
        let here = PathLayout {
            home: AbsPath::parse(home.to_str().unwrap()).unwrap(),
            root: AbsPath::parse(work.to_str().unwrap()).unwrap(),
        };
        let advert = codex_advert(&here);
        let offer = |state: &std::path::Path| {
            let adapter = adapter.clone();
            let advert = advert.clone();
            let here = here.clone();
            let state = state.to_owned();
            let work = work.clone();
            async move {
                offer_restoring(
                    &HarnessKind::Codex,
                    AllowedFidelity::DEFAULT,
                    Some(&state),
                    &work,
                    &here,
                    &advert,
                    move || Ok(Box::new(adapter) as Box<dyn fabric_harness::Harness>),
                )
                .await
            }
        };

        let (offered, session) = offer(&state).await.unwrap();
        assert_eq!(offered[0], Fidelity::NativeSession);
        assert!(session.is_some(), "the same layout restores the session");

        // This node's copy of the thread grew after the cut was taken:
        // the adapter refuses the restore as divergence.
        let mut grown = std::fs::read(&rollout).unwrap();
        grown.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"here\"}]}}\n");
        std::fs::write(&rollout, &grown).unwrap();
        let err = offer(&state).await.unwrap_err();
        assert!(err.contains("the harness state was not restored"), "{err}");
        assert_eq!(
            std::fs::read(&rollout).unwrap(),
            grown,
            "nothing was placed"
        );

        // A state that is there and cannot be read.
        let broken = dir.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("manifest.json"), b"{").unwrap();
        let err = offer(&broken).await.unwrap_err();
        assert!(err.contains("cannot be read"), "{err}");
    }
}
