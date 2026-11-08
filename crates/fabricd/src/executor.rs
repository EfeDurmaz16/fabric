//! The executor of managed attempts (ADR 0023): launch, child registration,
//! quiescence, receipts and the checkpoint cut.
//!
//! Invariant: every task runs in a process group of its own and is
//! registered in a durable record before it is launched and completed
//! after, so a restart finds every group this node started and stops what
//! it cannot account for. An interrupted build or test is `Interrupted`,
//! never passed and never failed; a harness task is never killed by a
//! quiescence but checkpointed by the suspend path, or the handoff is
//! blocked with the capture's reason; an opaque task blocks. A secret
//! reaches a task's environment at spawn and is held nowhere else.
//!
//! What the executor counts: spawns, signals and captures. It flushes
//! nothing beyond the record it writes per launch and the capture's own
//! writes.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::{JobId, SnapshotId};
use fabric_core::managed::secret_ref::AttemptScope;
use fabric_core::managed::{
    AttemptId, CheckpointCut, DrainBudget, EvidenceKind, EvidenceRef, Generation, HandoffBlocker,
    OutcomePolicy, ReceiptStatus, SECRET_ENV_PREFIX, SecretRef, SourceError, TaskClass, TaskId,
    TaskReceipt,
};
use fabric_core::path::{AbsPath, RelPath};
use rustix::process::{Pid, Signal};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::local;
use crate::secrets::{LocalResolver, SecretError};

/// How long a stop waits after `SIGKILL` for the group to go.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// How often a wait looks again.
const POLL: Duration = Duration::from_millis(25);
/// The `ps` that lists a group's processes.
const PS: &str = "ps";
/// The `lsof` that lists writers under the root.
const LSOF: &str = "lsof";

/// Why the executor refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// The process did not start.
    #[error("the task did not start: {0}")]
    Spawn(#[source] std::io::Error),
    /// No such attempt runs here.
    #[error("attempt {attempt} is not running on this node")]
    NotRunning {
        /// The attempt.
        attempt: AttemptId,
    },
    /// The attempt is quiescing or stopped; nothing more is spawned for it.
    #[error("attempt {attempt} is frozen; no task is spawned for it")]
    Frozen {
        /// The attempt.
        attempt: AttemptId,
    },
    /// A secret the task needs did not resolve.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// The source did not prepare.
    #[error(transparent)]
    Source(#[from] SourceError),
    /// The durable record of a launch could not be written or read.
    #[error(transparent)]
    Record(#[from] local::LocalError),
}

/// What a spawn made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spawned {
    /// The process.
    pub pid: u32,
    /// Its process group, which it leads.
    pub group: u32,
    /// The task.
    pub task: TaskId,
}

/// What a quiescence found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quiescence {
    /// One receipt per task of the attempt.
    pub receipts: Vec<TaskReceipt>,
    /// What blocks a strong handoff; empty when nothing does.
    pub blockers: Vec<HandoffBlocker>,
}

/// What the executor counted since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Processes spawned.
    pub spawns: u64,
    /// Signals sent to groups.
    pub signals: u64,
    /// Captures asked of the capturer.
    pub captures: u64,
}

/// The capture side of the executor: the suspend path for a harness task
/// and the snapshot capture for a cut. The node wires the real one; the
/// tests a fake.
pub trait Capturer: Send + Sync {
    /// Checkpoints a harness task through its adapter's suspend path and
    /// returns the snapshot, or the capture's reason.
    fn checkpoint_harness(
        &self,
        attempt: AttemptId,
        task: TaskId,
    ) -> BoxFuture<'_, Result<SnapshotId, String>>;
    /// Captures the attempt's workspace into a snapshot.
    fn capture(&self, attempt: AttemptId) -> BoxFuture<'_, Result<SnapshotId, String>>;
    /// The node's facts for the job, sorted by name.
    fn facts(&self) -> Vec<(String, String)>;
}

/// The durable record of one launched task: what a restarted executor
/// stops. The start time tells the pid from one given out again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRecord {
    /// The attempt.
    pub attempt: AttemptId,
    /// The task.
    pub task: TaskId,
    /// Its class.
    pub class: TaskClass,
    /// The pid; `None` while the launch is intended and not yet made.
    pub pid: Option<u32>,
    /// Its start time as `ps` prints it, once known.
    pub started: Option<String>,
}

/// One running task.
struct Running {
    task: TaskId,
    class: TaskClass,
    pid: NonZeroU32,
    started: Option<String>,
    child: tokio::process::Child,
    /// The exit status, once reaped.
    exit: Option<std::process::ExitStatus>,
}

/// One attempt this node runs.
struct AttemptRun {
    job: JobId,
    generation: Generation,
    root: AbsPath,
    secrets: Vec<SecretRef>,
    frozen: bool,
    tasks: Vec<Running>,
}

/// The attempts this node runs, their groups and registered children.
///
/// Each attempt has a lock of its own: a quiescence or a stop that waits
/// out one attempt's drain budget holds that attempt alone, and the map
/// is locked only to find or change an entry, never across a wait.
pub struct Executor {
    records: PathBuf,
    resolver: LocalResolver,
    capturer: Arc<dyn Capturer>,
    attempts: Mutex<BTreeMap<AttemptId, Arc<Mutex<AttemptRun>>>>,
    /// The job's current generation as this node knows it: the newest
    /// attempt begun here, or what the authority reported since.
    current: Mutex<BTreeMap<JobId, Generation>>,
    spawns: AtomicU64,
    signals: AtomicU64,
    captures: AtomicU64,
    /// Fails the launch record's completion once, for the test of a
    /// spawn that fails after its process exists.
    #[cfg(test)]
    fail_completion: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Executor({})", self.records.display())
    }
}

fn pid_of(pid: NonZeroU32) -> Option<Pid> {
    Pid::from_raw(i32::try_from(pid.get()).ok()?)
}

impl Executor {
    /// An executor whose launch records live under `records`, whose
    /// secrets come from `resolver` and whose captures go through `capturer`.
    pub fn new(records: PathBuf, resolver: LocalResolver, capturer: Arc<dyn Capturer>) -> Self {
        Executor {
            records,
            resolver,
            capturer,
            attempts: Mutex::new(BTreeMap::new()),
            current: Mutex::new(BTreeMap::new()),
            spawns: AtomicU64::new(0),
            signals: AtomicU64::new(0),
            captures: AtomicU64::new(0),
            #[cfg(test)]
            fail_completion: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The attempt's run, or `NotRunning`; the map's lock is released
    /// before the caller locks the run.
    async fn run(&self, attempt: AttemptId) -> Result<Arc<Mutex<AttemptRun>>, ExecutorError> {
        self.attempts
            .lock()
            .await
            .get(&attempt)
            .cloned()
            .ok_or(ExecutorError::NotRunning { attempt })
    }

    /// Records that `job`'s current grant is at `generation` (the
    /// authority's word, or a newer attempt begun here), so an older
    /// attempt of the job no longer receives a secret scoped to the
    /// current attempt. A lower generation than known changes nothing.
    pub async fn supersede(&self, job: JobId, generation: Generation) {
        let mut current = self.current.lock().await;
        let known = current.entry(job).or_insert(generation);
        if generation > *known {
            *known = generation;
        }
    }

    /// What the executor counted.
    pub fn counters(&self) -> Counters {
        Counters {
            spawns: self.spawns.load(Ordering::Relaxed),
            signals: self.signals.load(Ordering::Relaxed),
            captures: self.captures.load(Ordering::Relaxed),
        }
    }

    /// Opens an attempt on this node: its job and generation, the root
    /// its tasks run in and the secrets its tasks may need. Nothing is
    /// spawned. A newer generation of the job supersedes the older
    /// attempts this node runs (see [`Executor::supersede`]).
    pub async fn begin(
        &self,
        attempt: AttemptId,
        job: JobId,
        generation: Generation,
        root: AbsPath,
        secrets: Vec<SecretRef>,
    ) {
        self.supersede(job, generation).await;
        self.attempts.lock().await.insert(
            attempt,
            Arc::new(Mutex::new(AttemptRun {
                job,
                generation,
                root,
                secrets,
                frozen: false,
                tasks: Vec::new(),
            })),
        );
    }

    /// Lets a quiesced attempt spawn again: a checkpoint command's cut
    /// leaves the attempt running (ADR 0023), and its next build or test
    /// starts anew, since the quiescence ended the ones it interrupted.
    pub async fn resume(&self, attempt: AttemptId) -> Result<(), ExecutorError> {
        self.run(attempt).await?.lock().await.frozen = false;
        Ok(())
    }

    /// Forgets an attempt whose tasks all ended; its records are removed.
    pub async fn end(&self, attempt: AttemptId) -> Result<(), ExecutorError> {
        self.attempts.lock().await.remove(&attempt);
        let dir = self.records.join(attempt.to_string());
        if dir.exists() {
            for entry in std::fs::read_dir(&dir).map_err(|source| local::LocalError::Io {
                path: dir.clone(),
                source,
            })? {
                let path = entry
                    .map_err(|source| local::LocalError::Io {
                        path: dir.clone(),
                        source,
                    })?
                    .path();
                local::remove(&path)?;
            }
            let _ = std::fs::remove_dir(&dir);
        }
        Ok(())
    }

    fn record_path(&self, attempt: AttemptId, task: TaskId) -> PathBuf {
        self.records
            .join(attempt.to_string())
            .join(format!("{task}.json"))
    }

    /// Spawns `command` for `task` in a group of its own, with the job's
    /// secrets in its environment under their names, and registers it.
    /// Refused while the attempt is frozen. The launch is recorded before
    /// the process exists and completed after, so a restart stops what a
    /// crash between the two left.
    pub async fn spawn(
        &self,
        attempt: AttemptId,
        task: TaskId,
        class: TaskClass,
        mut command: std::process::Command,
    ) -> Result<Spawned, ExecutorError> {
        let run = self.run(attempt).await?;
        let mut run = run.lock().await;
        if run.frozen {
            return Err(ExecutorError::Frozen { attempt });
        }
        // An attempt is the job's current grant until a newer generation
        // of the job is known here; a secret scoped to the current
        // attempt reaches only that one (ADR 0025).
        let is_current = self
            .current
            .lock()
            .await
            .get(&run.job)
            .is_none_or(|current| *current <= run.generation);
        // Secrets are resolved now, never earlier and never kept: a
        // missing one refuses the launch before any process exists.
        let mut env = Vec::new();
        for secret in &run.secrets {
            if secret.scope.attempt == AttemptScope::Current && !is_current {
                continue;
            }
            let value = self.resolver.resolve(run.job, attempt, secret)?;
            env.push((
                secret.name.as_str().to_owned(),
                String::from_utf8_lossy(value.expose()).into_owned(),
            ));
        }
        let path = self.record_path(attempt, task);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| local::LocalError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        let mut record = LaunchRecord {
            attempt,
            task,
            class,
            pid: None,
            started: None,
        };
        // Launch boundary one (`managed_launch.before_record`): the intent
        // is durable and no process exists; a crash here leaves a record
        // with no pid, which recovery clears.
        local::write(&path, &record)?;
        command.process_group(0).current_dir(run.root.as_str());
        scrub_secrets(&mut command);
        for (key, value) in &env {
            command.env(key, value);
        }
        drop(env);
        let child = tokio::process::Command::from(command)
            .kill_on_drop(false)
            .spawn()
            .map_err(ExecutorError::Spawn)?;
        self.spawns.fetch_add(1, Ordering::Relaxed);
        let pid = child
            .id()
            .and_then(NonZeroU32::new)
            .ok_or_else(|| ExecutorError::Spawn(std::io::Error::other("the child has no pid")))?;
        let started = local::process_started(pid).await;
        record.pid = Some(pid.get());
        record.started.clone_from(&started);
        if let Err(e) = self.complete_record(&path, &record) {
            // The process exists and no record names it: it is stopped
            // now, so nothing runs that no receipt, blocker or recovery
            // accounts for. The intent record (no pid) is what recovery
            // then clears.
            let mut child = child;
            if let Some(group) = pid_of(pid) {
                self.signals.fetch_add(1, Ordering::Relaxed);
                signal_group(group, Signal::KILL);
                wait_group_gone(group, KILL_WAIT).await;
            }
            let _ = child.wait().await;
            return Err(e.into());
        }
        // Launch boundary two (`managed_launch.after_record`): the process
        // runs and its record names it; a crash here leaves a group that
        // recovery stops by its recorded pid and start time.
        run.tasks.push(Running {
            task,
            class,
            pid,
            started,
            child,
            exit: None,
        });
        Ok(Spawned {
            pid: pid.get(),
            group: pid.get(),
            task,
        })
    }

    /// Completes a launch record with its pid.
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "the tests fail the completion through the executor"
        )
    )]
    fn complete_record(&self, path: &Path, record: &LaunchRecord) -> Result<(), local::LocalError> {
        #[cfg(test)]
        if self.fail_completion.swap(false, Ordering::Relaxed) {
            return Err(local::LocalError::Io {
                path: path.to_owned(),
                source: std::io::Error::other("the test failed the record's completion"),
            });
        }
        local::write(path, record)
    }

    /// Stops every group a crashed executor left recorded under `records`
    /// and removes the records: a launch intended and not made has
    /// nothing to stop; one made is signalled only when the pid still
    /// starts when the record says. Returns the records found.
    pub async fn recover(
        records: &Path,
        budget: DrainBudget,
    ) -> Result<Vec<LaunchRecord>, ExecutorError> {
        let mut found = Vec::new();
        let Ok(attempts) = std::fs::read_dir(records) else {
            return Ok(found);
        };
        for attempt in attempts.flatten() {
            let Ok(tasks) = std::fs::read_dir(attempt.path()) else {
                continue;
            };
            for entry in tasks.flatten() {
                let path = entry.path();
                let Some(record) = local::read::<LaunchRecord>(&path)? else {
                    continue;
                };
                if let (Some(pid), Some(started)) = (record.pid, &record.started)
                    && let Some(pid) = NonZeroU32::new(pid)
                    && local::process_started(pid).await.as_ref() == Some(started)
                    && let Some(group) = pid_of(pid)
                {
                    signal_group(group, Signal::TERM);
                    if !wait_group_gone(group, budget.duration()).await {
                        signal_group(group, Signal::KILL);
                        wait_group_gone(group, KILL_WAIT).await;
                    }
                }
                local::remove(&path)?;
                found.push(record);
            }
            let _ = std::fs::remove_dir(attempt.path());
        }
        Ok(found)
    }

    /// Freezes spawning for the attempt, settles leaves before parents
    /// under each task's policy within `budget`, and reports receipts and
    /// blockers. Never kills a harness task: it goes through the suspend
    /// path and comes back `Checkpointed` or `CheckpointFailed`.
    pub async fn quiesce(
        &self,
        attempt: AttemptId,
        budget: DrainBudget,
    ) -> Result<Quiescence, ExecutorError> {
        let run = self.run(attempt).await?;
        let mut run = run.lock().await;
        run.frozen = true;
        let root = run.root.clone();
        let mut receipts = Vec::new();
        let mut blockers = Vec::new();
        // Leaves before parents: a child is spawned after its parent, so
        // the newest task settles first.
        for running in run.tasks.iter_mut().rev() {
            let receipt = match running.class.outcome_policy() {
                OutcomePolicy::CancelAndRestart => {
                    settle_or_interrupt(running, budget, &self.signals).await
                }
                OutcomePolicy::Checkpoint => {
                    if reap(running).is_some() {
                        exited(running)
                    } else {
                        self.captures.fetch_add(1, Ordering::Relaxed);
                        match self
                            .capturer
                            .checkpoint_harness(attempt, running.task)
                            .await
                        {
                            Ok(snapshot) => TaskReceipt {
                                task: running.task,
                                class: running.class,
                                status: ReceiptStatus::Checkpointed { snapshot },
                                exit: None,
                                // The turn was cut: the harness was mid-turn.
                                interrupted: true,
                                workspace: None,
                            },
                            Err(reason) => {
                                blockers.push(HandoffBlocker::CheckpointFailed {
                                    task: running.task,
                                    reason: reason.clone(),
                                });
                                TaskReceipt {
                                    task: running.task,
                                    class: running.class,
                                    status: ReceiptStatus::Blocked { reason },
                                    exit: None,
                                    interrupted: false,
                                    workspace: None,
                                }
                            }
                        }
                    }
                }
                OutcomePolicy::Block => {
                    if reap(running).is_some() {
                        exited(running)
                    } else {
                        blockers.push(HandoffBlocker::Opaque { task: running.task });
                        TaskReceipt {
                            task: running.task,
                            class: running.class,
                            status: ReceiptStatus::Blocked {
                                reason: "opaque task is running".into(),
                            },
                            exit: None,
                            interrupted: false,
                            workspace: None,
                        }
                    }
                }
            };
            receipts.push(receipt);
        }
        receipts.reverse();
        let registered: Vec<u32> = run.tasks.iter().map(|t| t.pid.get()).collect();
        let groups: Vec<u32> = registered.clone();
        drop(run);
        blockers.extend(group_blockers(&groups, &registered, &root).await);
        Ok(Quiescence { receipts, blockers })
    }

    /// Stops every group of the attempt: `SIGTERM`, `budget`, `SIGKILL`;
    /// every receipt `Interrupted`, except a task that had already exited,
    /// whose group is still ended for whatever it left in it.
    pub async fn stop(
        &self,
        attempt: AttemptId,
        budget: DrainBudget,
    ) -> Result<Vec<TaskReceipt>, ExecutorError> {
        let run = self.run(attempt).await?;
        let mut run = run.lock().await;
        run.frozen = true;
        let mut receipts = Vec::new();
        for running in &mut run.tasks {
            let exited_before = reap(running).is_some();
            interrupt(running, budget, &self.signals).await;
            receipts.push(if exited_before {
                exited(running)
            } else {
                TaskReceipt {
                    task: running.task,
                    class: running.class,
                    status: ReceiptStatus::Interrupted,
                    exit: None,
                    interrupted: true,
                    workspace: None,
                }
            });
        }
        Ok(receipts)
    }

    /// Processes in the attempt's groups that no registered task owns,
    /// and writers under `root` outside every group (`lsof`, where the
    /// facts found it). Empty for an attempt this node does not run.
    pub async fn blockers(&self, attempt: AttemptId, root: &AbsPath) -> Vec<HandoffBlocker> {
        let (groups, registered) = {
            let Ok(run) = self.run(attempt).await else {
                return Vec::new();
            };
            let run = run.lock().await;
            let pids: Vec<u32> = run.tasks.iter().map(|t| t.pid.get()).collect();
            (pids.clone(), pids)
        };
        group_blockers(&groups, &registered, root).await
    }

    /// The cut: the capture through the existing path, then the document
    /// with the quiescence's receipts, the node's facts and the evidence
    /// refs of the receipts by digest.
    pub async fn cut(
        &self,
        attempt: AttemptId,
        quiescence: &Quiescence,
    ) -> Result<CheckpointCut, ExecutorError> {
        if !self.attempts.lock().await.contains_key(&attempt) {
            return Err(ExecutorError::NotRunning { attempt });
        }
        self.captures.fetch_add(1, Ordering::Relaxed);
        let snapshot = self
            .capturer
            .capture(attempt)
            .await
            .map_err(|reason| ExecutorError::Source(SourceError::Unavailable { reason }))?;
        let evidence = quiescence
            .receipts
            .iter()
            .map(|receipt| EvidenceRef {
                digest: receipt_digest(receipt),
                attempt,
                kind: EvidenceKind::Receipt,
            })
            .collect();
        Ok(CheckpointCut {
            snapshot,
            receipts: quiescence.receipts.clone(),
            facts: self.capturer.facts(),
            evidence,
        })
    }

    /// The pids of the attempt's registered tasks, for the tests and the inspect surface.
    pub async fn pids(&self, attempt: AttemptId) -> Vec<u32> {
        match self.run(attempt).await {
            Ok(run) => run.lock().await.tasks.iter().map(|t| t.pid.get()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Removes every `FABRIC_SECRET_*` variable from what `command` would
/// give its process: the ones it inherits from the daemon, which holds
/// the node's secrets there (ADR 0025), and the ones set on it. Only the
/// job's declared secrets reach a task, under their bare names.
fn scrub_secrets(command: &mut std::process::Command) {
    let explicit: Vec<std::ffi::OsString> =
        command.get_envs().map(|(key, _)| key.to_owned()).collect();
    for key in std::env::vars_os().map(|(key, _)| key).chain(explicit) {
        if key
            .as_encoded_bytes()
            .starts_with(SECRET_ENV_PREFIX.as_bytes())
        {
            command.env_remove(&key);
        }
    }
}

/// The digest of a receipt: blake3 of its JSON under its own domain.
fn receipt_digest(receipt: &TaskReceipt) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fabric.managed.receipt.v1\0");
    hasher.update(&serde_json::to_vec(receipt).unwrap_or_default());
    *hasher.finalize().as_bytes()
}

/// Reaps the task if it exited; its status, once known, is kept.
fn reap(running: &mut Running) -> Option<std::process::ExitStatus> {
    if running.exit.is_none()
        && let Ok(Some(status)) = running.child.try_wait()
    {
        running.exit = Some(status);
    }
    running.exit
}

fn exited(running: &Running) -> TaskReceipt {
    TaskReceipt {
        task: running.task,
        class: running.class,
        status: ReceiptStatus::Exited,
        exit: running.exit.and_then(|s| s.code()),
        interrupted: false,
        workspace: None,
    }
}

/// Waits up to `budget` for the task to exit on its own; interrupts it
/// otherwise. A task that exited within the budget is `Exited`.
async fn settle_or_interrupt(
    running: &mut Running,
    budget: DrainBudget,
    signals: &AtomicU64,
) -> TaskReceipt {
    let deadline = tokio::time::Instant::now() + budget.duration();
    loop {
        if reap(running).is_some() {
            return exited(running);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    interrupt(
        running,
        DrainBudget::new(DrainBudget::MIN_MS).unwrap_or_default(),
        signals,
    )
    .await;
    TaskReceipt {
        task: running.task,
        class: running.class,
        status: ReceiptStatus::Interrupted,
        exit: None,
        interrupted: true,
        workspace: None,
    }
}

/// `SIGTERM` to the task's group, `grace`, then `SIGKILL`; reaps the
/// leader. A group with nothing live in it is left alone, and so is a
/// leader pid that now starts at another time, which another process has.
async fn interrupt(running: &mut Running, grace: DrainBudget, signals: &AtomicU64) {
    let Some(group) = pid_of(running.pid) else {
        return;
    };
    if group_members(running.pid.get()).await.is_empty() {
        let _ = running
            .child
            .try_wait()
            .map(|s| running.exit = s.or(running.exit));
        return;
    }
    if running.exit.is_none()
        && let Some(started) = &running.started
        && local::process_started(running.pid).await.as_ref() != Some(started)
        && running.child.try_wait().ok().flatten().is_none()
    {
        return;
    }
    signals.fetch_add(1, Ordering::Relaxed);
    signal_group(group, Signal::TERM);
    if !wait_group_gone(group, grace.duration()).await {
        signals.fetch_add(1, Ordering::Relaxed);
        signal_group(group, Signal::KILL);
        wait_group_gone(group, KILL_WAIT).await;
    }
    let _ = running.child.wait().await.map(|s| running.exit = Some(s));
}

fn signal_group(group: Pid, signal: Signal) {
    let _ = rustix::process::kill_process_group(group, signal);
}

/// Whether nothing of the group runs within `limit`; a zombie leader whose
/// parent has not reaped it yet counts as gone once nothing else does.
async fn wait_group_gone(group: Pid, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if group_members(group.as_raw_nonzero().get().unsigned_abs())
            .await
            .is_empty()
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The live pids in process group `group`, as `ps` lists them; zombies
/// left out (state `Z`).
async fn group_members(group: u32) -> Vec<u32> {
    let Ok(output) = tokio::process::Command::new(PS)
        .args(["-axo", "pid=,pgid=,stat="])
        .output()
        .await
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid: u32 = fields.next()?.parse().ok()?;
            let group_of: u32 = fields.next()?.parse().ok()?;
            let stat = fields.next().unwrap_or_default();
            (group_of == group && !stat.starts_with('Z')).then_some(pid)
        })
        .collect()
}

/// Escaped children in `groups` that are not in `registered`, and
/// writers under `root` outside every group.
async fn group_blockers(groups: &[u32], registered: &[u32], root: &AbsPath) -> Vec<HandoffBlocker> {
    let mut blockers = Vec::new();
    for &group in groups {
        for pid in group_members(group).await {
            if !registered.contains(&pid) {
                blockers.push(HandoffBlocker::EscapedChild { pid, group });
            }
        }
    }
    for (pid, path) in writers_under(root).await {
        let in_group = groups.contains(&pid)
            || blockers
                .iter()
                .any(|b| matches!(b, HandoffBlocker::EscapedChild { pid: p, .. } if *p == pid));
        if !in_group {
            blockers.push(HandoffBlocker::UncontrolledWriter { pid, path });
        }
    }
    blockers
}

/// Processes that hold a file under `root` open for writing, as `lsof`
/// reports them; empty when `lsof` is not there or finds none.
async fn writers_under(root: &AbsPath) -> Vec<(u32, RelPath)> {
    let Ok(output) = tokio::process::Command::new(LSOF)
        .args(["-w", "-F", "pan", "+D", root.as_str()])
        .output()
        .await
    else {
        return Vec::new();
    };
    // `lsof` prints resolved paths; the root may be reached through a link.
    let mut roots = vec![root.as_str().to_owned()];
    if let Ok(real) = std::fs::canonicalize(root.as_str())
        && let Some(real) = real.to_str()
        && real != root.as_str()
    {
        roots.push(real.to_owned());
    }
    let mut writers = Vec::new();
    let (mut pid, mut access) = (None::<u32>, None::<char>);
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match line.split_at_checked(1) {
            Some(("p", rest)) => pid = rest.parse().ok(),
            Some(("a", rest)) => access = rest.chars().next(),
            Some(("n", rest)) => {
                if let (Some(pid), Some('w' | 'u')) = (pid, access)
                    && let Some(rel) = roots
                        .iter()
                        .find_map(|r| rest.strip_prefix(r.as_str()))
                        .and_then(|r| r.strip_prefix('/'))
                    && let Ok(path) = RelPath::parse(rel)
                {
                    writers.push((pid, path));
                }
            }
            _ => {}
        }
    }
    writers
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use fabric_core::managed::SecretName;

    use super::*;

    /// A capturer that answers from a script and remembers what it was asked.
    struct FakeCapturer {
        harness: Result<SnapshotId, String>,
        asked: StdMutex<Vec<TaskId>>,
    }

    impl Capturer for FakeCapturer {
        fn checkpoint_harness(
            &self,
            _attempt: AttemptId,
            task: TaskId,
        ) -> BoxFuture<'_, Result<SnapshotId, String>> {
            self.asked.lock().unwrap().push(task);
            let answer = self.harness.clone();
            Box::pin(async move { answer })
        }

        fn capture(&self, attempt: AttemptId) -> BoxFuture<'_, Result<SnapshotId, String>> {
            Box::pin(async move { Ok(SnapshotId::of(&attempt.to_bytes())) })
        }

        fn facts(&self) -> Vec<(String, String)> {
            vec![("os".into(), "test".into())]
        }
    }

    struct Bench {
        dir: tempfile::TempDir,
        root: AbsPath,
        executor: Executor,
        attempt: AttemptId,
        capturer: Arc<FakeCapturer>,
    }

    async fn bench(harness: Result<SnapshotId, String>) -> Bench {
        bench_with(harness, Vec::new(), Vec::new()).await
    }

    async fn bench_with(
        harness: Result<SnapshotId, String>,
        secrets: Vec<SecretRef>,
        env: Vec<(String, String)>,
    ) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(&root).unwrap();
        let root = AbsPath::parse(root.to_str().unwrap()).unwrap();
        let capturer = Arc::new(FakeCapturer {
            harness,
            asked: StdMutex::new(Vec::new()),
        });
        let resolver = LocalResolver::with_environment(dir.path(), env);
        let executor = Executor::new(dir.path().join("records"), resolver, capturer.clone());
        let attempt = AttemptId::from_bytes([7; 16]);
        executor
            .begin(
                attempt,
                JobId::from_bytes([1; 16]),
                Generation::FIRST,
                root.clone(),
                secrets,
            )
            .await;
        Bench {
            dir,
            root,
            executor,
            attempt,
            capturer,
        }
    }

    /// A task that runs `script` under `sh -c`. A shell may fork its last
    /// command instead of replacing itself (dash on Ubuntu does), and the
    /// forked child is then an escaped child of the task's group (ADR
    /// 0023), so a task meant to be one process says `exec`.
    fn sh(script: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd
    }

    fn task(n: u8) -> TaskId {
        TaskId::from_bytes([n; 16])
    }

    fn budget(ms: u32) -> DrainBudget {
        DrainBudget::new(ms).unwrap()
    }

    fn alive(pid: u32) -> bool {
        NonZeroU32::new(pid)
            .and_then(pid_of)
            .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
            && std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .is_ok_and(|o| {
                    let s = String::from_utf8_lossy(&o.stdout);
                    !s.trim().is_empty() && !s.trim_start().starts_with('Z')
                })
    }

    #[tokio::test]
    async fn an_interrupted_test_is_recorded_interrupted_never_passed() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Test, sh("exec sleep 30"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        assert_eq!(q.receipts.len(), 1);
        let r = &q.receipts[0];
        assert_eq!(r.status, ReceiptStatus::Interrupted);
        assert!(r.interrupted);
        assert_eq!(r.exit, None);
        assert!(!r.passed());
        assert!(q.blockers.is_empty(), "{:?}", q.blockers);
        // A test that finished within the budget is exited, and passes only on zero.
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Test, sh("exit 0"))
            .await
            .unwrap();
        b.executor
            .spawn(b.attempt, task(2), TaskClass::Test, sh("exit 3"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(2_000)).await.unwrap();
        assert!(q.receipts[0].passed());
        assert_eq!(q.receipts[1].exit, Some(3));
        assert!(!q.receipts[1].passed());
        assert!(!q.receipts[1].interrupted);
    }

    #[tokio::test]
    async fn an_interrupted_build_is_restarted_not_resumed() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        let spawned = b
            .executor
            .spawn(b.attempt, task(1), TaskClass::Build, sh("exec sleep 30"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        assert_eq!(q.receipts[0].status, ReceiptStatus::Interrupted);
        assert!(!alive(spawned.pid), "the build's group was stopped");
        // Frozen: nothing more is spawned for the attempt; the next
        // attempt starts the build again from scratch.
        assert!(matches!(
            b.executor
                .spawn(b.attempt, task(2), TaskClass::Build, sh("exit 0"))
                .await,
            Err(ExecutorError::Frozen { .. })
        ));
        assert_eq!(b.executor.counters().spawns, 1);
    }

    #[tokio::test]
    async fn a_harness_task_is_checkpointed_by_the_suspend_path() {
        let snapshot = SnapshotId::of(b"harness");
        let b = bench(Ok(snapshot)).await;
        let spawned = b
            .executor
            .spawn(b.attempt, task(1), TaskClass::Harness, sh("exec sleep 30"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        assert_eq!(
            q.receipts[0].status,
            ReceiptStatus::Checkpointed { snapshot }
        );
        assert!(q.receipts[0].interrupted, "mid-turn");
        assert_eq!(*b.capturer.asked.lock().unwrap(), vec![task(1)]);
        assert!(
            alive(spawned.pid),
            "a harness task is never killed by a quiescence"
        );
        assert_eq!(b.executor.counters().signals, 0);
        let cut = b.executor.cut(b.attempt, &q).await.unwrap();
        assert_eq!(cut.receipts, q.receipts);
        assert_eq!(cut.facts, vec![("os".to_owned(), "test".to_owned())]);
        assert_eq!(cut.evidence.len(), 1);
        assert_eq!(cut.evidence[0].kind, EvidenceKind::Receipt);
        assert_eq!(b.executor.counters().captures, 2);
        b.executor.stop(b.attempt, budget(1_000)).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_harness_checkpoint_blocks_the_handoff() {
        let b = bench(Err("the transcript is locked".into())).await;
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Harness, sh("exec sleep 30"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        assert_eq!(
            q.blockers,
            vec![HandoffBlocker::CheckpointFailed {
                task: task(1),
                reason: "the transcript is locked".into()
            }]
        );
        assert!(matches!(
            q.receipts[0].status,
            ReceiptStatus::Blocked { .. }
        ));
        let pids = b.executor.pids(b.attempt).await;
        assert!(alive(pids[0]), "the attempt keeps running");
        b.executor.stop(b.attempt, budget(1_000)).await.unwrap();
    }

    #[tokio::test]
    async fn an_opaque_task_blocks() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Opaque, sh("exec sleep 30"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        assert_eq!(q.blockers, vec![HandoffBlocker::Opaque { task: task(1) }]);
        let pids = b.executor.pids(b.attempt).await;
        assert!(alive(pids[0]), "nothing is guessed about an opaque task");
        let receipts = b.executor.stop(b.attempt, budget(1_000)).await.unwrap();
        assert_eq!(receipts[0].status, ReceiptStatus::Interrupted);
        assert!(!alive(pids[0]));
    }

    #[tokio::test]
    async fn an_escaped_child_blocks_a_strong_handoff_and_fails_nothing() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        // The task starts a child in its group and exits; the child outlives it.
        b.executor
            .spawn(
                b.attempt,
                task(1),
                TaskClass::Test,
                sh("sleep 30 >/dev/null 2>&1 </dev/null & exit 0"),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        assert!(
            q.receipts[0].passed(),
            "the test itself exited zero: {:?}",
            q.receipts
        );
        let escaped: Vec<_> = q
            .blockers
            .iter()
            .filter(|b| matches!(b, HandoffBlocker::EscapedChild { .. }))
            .collect();
        assert_eq!(escaped.len(), 1, "{:?}", q.blockers);
        let HandoffBlocker::EscapedChild { pid, group } = escaped[0] else {
            panic!("not an escaped child: {:?}", escaped[0])
        };
        assert_eq!(b.executor.pids(b.attempt).await, vec![*group]);
        assert!(
            alive(*pid),
            "an escaped child is reported, never killed by a quiescence"
        );
        // A stop ends the whole group, the escaped child with it.
        b.executor.stop(b.attempt, budget(1_000)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!alive(*pid));
    }

    #[tokio::test]
    async fn an_uncontrolled_writer_blocks() {
        if std::process::Command::new(LSOF).arg("-v").output().is_err() {
            return;
        }
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        // A writer this executor never spawned, holding a file under the root open.
        let file = Path::new(b.root.as_str()).join("out.log");
        let mut writer = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exec 3>>{} ; sleep 30", file.display()))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let blockers = b.executor.blockers(b.attempt, &b.root).await;
        let _ = writer.kill();
        let _ = writer.wait();
        assert!(
            blockers.iter().any(|blocker| matches!(
                blocker,
                HandoffBlocker::UncontrolledWriter { pid, path }
                    if *pid == writer.id() && path.as_str() == "out.log"
            )),
            "{blockers:?}"
        );
    }

    #[tokio::test]
    async fn a_parent_waits_for_its_leaves() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        // The parent is spawned first, the leaf after; the leaf settles first.
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Build, sh("exec sleep 30"))
            .await
            .unwrap();
        b.executor
            .spawn(b.attempt, task(2), TaskClass::Test, sh("sleep 0.2; exit 0"))
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(2_000)).await.unwrap();
        assert_eq!(q.receipts[0].task, task(1));
        assert_eq!(q.receipts[1].task, task(2));
        assert!(q.receipts[1].passed(), "the leaf had its budget first");
        assert_eq!(q.receipts[0].status, ReceiptStatus::Interrupted);
    }

    #[tokio::test]
    async fn the_drain_budget_bounds_quiescence_not_the_task() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        let spawned = b
            .executor
            .spawn(b.attempt, task(1), TaskClass::Opaque, sh("exec sleep 30"))
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        let q = b.executor.quiesce(b.attempt, budget(1_000)).await.unwrap();
        // An opaque task is not waited for at all, and never cut: the
        // budget bounds the wait, the task runs on.
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(q.blockers.len(), 1);
        assert!(alive(spawned.pid));
        // A test task gets the budget, then is cut; it does not run on.
        let b2 = bench(Ok(SnapshotId::of(b"x"))).await;
        let spawned2 = b2
            .executor
            .spawn(b2.attempt, task(1), TaskClass::Test, sh("exec sleep 30"))
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        b2.executor
            .quiesce(b2.attempt, budget(1_000))
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(900));
        assert!(!alive(spawned2.pid));
        b.executor.stop(b.attempt, budget(1_000)).await.unwrap();
    }

    #[tokio::test]
    async fn a_secret_reaches_the_task_environment_and_nothing_else() {
        let name = SecretName::parse("TOKEN").unwrap();
        let secret = SecretRef {
            name: name.clone(),
            scope: fabric_core::managed::SecretScope::node_any(),
        };
        let b = bench_with(
            Ok(SnapshotId::of(b"x")),
            vec![secret.clone()],
            vec![("FABRIC_SECRET_TOKEN".into(), "hunter2".into())],
        )
        .await;
        let out = Path::new(b.root.as_str()).join("seen");
        b.executor
            .spawn(
                b.attempt,
                task(1),
                TaskClass::Test,
                sh(&format!("printf '%s' \"$TOKEN\" > {}", out.display())),
            )
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(2_000)).await.unwrap();
        assert!(q.receipts[0].passed());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hunter2");
        // Nothing the executor keeps or writes holds the value.
        let cut = b.executor.cut(b.attempt, &q).await.unwrap();
        assert!(!String::from_utf8_lossy(&cut.canonical_json()).contains("hunter2"));
        assert!(!format!("{:?}", b.executor).contains("hunter2"));
        for entry in walkdir(&b.dir.path().join("records")) {
            assert!(!std::fs::read_to_string(entry).unwrap().contains("hunter2"));
        }
        // A missing secret refuses the launch before any process exists.
        let missing = SecretRef {
            name: SecretName::parse("ABSENT").unwrap(),
            ..secret
        };
        let b = bench_with(Ok(SnapshotId::of(b"x")), vec![missing], Vec::new()).await;
        assert!(matches!(
            b.executor
                .spawn(b.attempt, task(1), TaskClass::Test, sh("exit 0"))
                .await,
            Err(ExecutorError::Secret(SecretError::Missing { .. }))
        ));
        assert_eq!(b.executor.counters().spawns, 0);
        assert!(b.executor.pids(b.attempt).await.is_empty());
    }

    /// No `FABRIC_SECRET_*` variable reaches a task but the job's own
    /// secrets under their bare names: not one the daemon's environment
    /// holds, not one set on the command, declared or not.
    #[tokio::test]
    async fn no_node_secret_reaches_a_task_but_its_declared_ones() {
        let secret = SecretRef {
            name: SecretName::parse("GITHUB_TOKEN").unwrap(),
            scope: fabric_core::managed::SecretScope::node_any(),
        };
        let b = bench_with(
            Ok(SnapshotId::of(b"x")),
            vec![secret],
            vec![
                ("FABRIC_SECRET_GITHUB_TOKEN".into(), "declared".into()),
                ("FABRIC_SECRET_NPM_TOKEN".into(), "undeclared".into()),
            ],
        )
        .await;
        let out = Path::new(b.root.as_str()).join("env");
        let mut command = sh(&format!("env > {}", out.display()));
        command.env("FABRIC_SECRET_NPM_TOKEN", "undeclared");
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Test, command)
            .await
            .unwrap();
        let q = b.executor.quiesce(b.attempt, budget(2_000)).await.unwrap();
        assert!(q.receipts[0].passed());
        let env = std::fs::read_to_string(&out).unwrap();
        assert!(env.lines().any(|l| l == "GITHUB_TOKEN=declared"), "{env}");
        assert!(!env.contains("FABRIC_SECRET_"), "{env}");
        assert!(!env.contains("undeclared"), "{env}");
    }

    /// A quiescence that drains one attempt for its whole budget does not
    /// hold up another attempt on the node: its spawn and stop go on.
    #[tokio::test]
    async fn one_attempts_drain_does_not_stall_another() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        let other = AttemptId::from_bytes([8; 16]);
        b.executor
            .begin(
                other,
                JobId::from_bytes([2; 16]),
                Generation::FIRST,
                b.root.clone(),
                Vec::new(),
            )
            .await;
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Test, sh("exec sleep 30"))
            .await
            .unwrap();
        let executor = Arc::new(b.executor);
        let draining = {
            let executor = Arc::clone(&executor);
            let attempt = b.attempt;
            tokio::spawn(async move { executor.quiesce(attempt, budget(3_000)).await })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        let started = tokio::time::Instant::now();
        executor
            .spawn(other, task(2), TaskClass::Test, sh("exit 0"))
            .await
            .unwrap();
        executor.stop(other, budget(1_000)).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "the other attempt waited {:?}",
            started.elapsed()
        );
        draining.await.unwrap().unwrap();
    }

    /// A secret scoped to the current attempt reaches a task of the job's
    /// current grant only: once a newer generation of the job is known,
    /// the older attempt's tasks get its `any` secrets and not that one.
    #[tokio::test]
    async fn a_current_scoped_secret_reaches_only_the_current_attempt() {
        let any = SecretRef {
            name: SecretName::parse("CACHE_TOKEN").unwrap(),
            scope: fabric_core::managed::SecretScope::node_any(),
        };
        let current = SecretRef {
            name: SecretName::parse("DEPLOY_KEY").unwrap(),
            scope: fabric_core::managed::SecretScope {
                attempt: AttemptScope::Current,
                ..fabric_core::managed::SecretScope::node_any()
            },
        };
        let b = bench_with(
            Ok(SnapshotId::of(b"x")),
            vec![any, current],
            vec![
                ("FABRIC_SECRET_CACHE_TOKEN".into(), "cache".into()),
                ("FABRIC_SECRET_DEPLOY_KEY".into(), "deploy".into()),
            ],
        )
        .await;
        let job = JobId::from_bytes([1; 16]);
        let seen = |name: &str| Path::new(b.root.as_str()).join(name);
        let print = |name: &str| {
            sh(&format!(
                "printf '%s|%s' \"$CACHE_TOKEN\" \"$DEPLOY_KEY\" > {}",
                seen(name).display()
            ))
        };
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Test, print("before"))
            .await
            .unwrap();
        b.executor
            .supersede(job, Generation::FIRST.next().unwrap())
            .await;
        b.executor
            .spawn(b.attempt, task(2), TaskClass::Test, print("after"))
            .await
            .unwrap();
        b.executor.quiesce(b.attempt, budget(2_000)).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(seen("before")).unwrap(),
            "cache|deploy"
        );
        assert_eq!(std::fs::read_to_string(seen("after")).unwrap(), "cache|");
    }

    /// A checkpoint command's quiescence leaves the attempt running: once
    /// resumed it spawns again (ADR 0023).
    #[tokio::test]
    async fn a_resumed_attempt_spawns_again_after_its_cut() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        b.executor
            .spawn(b.attempt, task(1), TaskClass::Test, sh("exit 0"))
            .await
            .unwrap();
        b.executor.quiesce(b.attempt, budget(2_000)).await.unwrap();
        assert!(matches!(
            b.executor
                .spawn(b.attempt, task(2), TaskClass::Test, sh("exit 0"))
                .await,
            Err(ExecutorError::Frozen { .. })
        ));
        b.executor.resume(b.attempt).await.unwrap();
        b.executor
            .spawn(b.attempt, task(2), TaskClass::Test, sh("exit 0"))
            .await
            .unwrap();
        b.executor.stop(b.attempt, budget(1_000)).await.unwrap();
    }

    /// A spawn whose launch record cannot be completed after the process
    /// exists stops that process: nothing runs that no record names.
    #[tokio::test]
    async fn a_spawn_that_fails_after_the_process_exists_leaves_nothing_running() {
        let b = bench(Ok(SnapshotId::of(b"x"))).await;
        let marker = Path::new(b.root.as_str()).join("pid");
        b.executor
            .fail_completion
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let spawned = b
            .executor
            .spawn(
                b.attempt,
                task(1),
                TaskClass::Test,
                sh(&format!("echo $$ > {}; exec sleep 30", marker.display())),
            )
            .await;
        assert!(
            matches!(spawned, Err(ExecutorError::Record(_))),
            "{spawned:?}"
        );
        assert!(b.executor.pids(b.attempt).await.is_empty());
        tokio::time::sleep(Duration::from_millis(300)).await;
        // The kill can land after the shell opened the marker and before
        // it wrote its pid: an empty marker is a shell stopped early.
        let written = std::fs::read_to_string(&marker).ok();
        if let Some(pid) = written.and_then(|s| s.trim().parse().ok()) {
            assert!(!alive(pid), "the process was stopped");
        }
    }

    fn walkdir(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(walkdir(&path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    /// The launch boundaries: a record intended and not made has nothing
    /// to stop; one made and left by a crash is stopped by recovery, so no
    /// task of a dead executor runs on. The crash is simulated by dropping
    /// the executor without stopping, which is what a killed daemon does.
    #[tokio::test]
    async fn a_client_killed_at_each_launch_boundary_leaves_no_task_running() {
        let dir = tempfile::tempdir().unwrap();
        let records = dir.path().join("records");
        let attempt = AttemptId::from_bytes([9; 16]);
        // Boundary one: before the record is complete, nothing was spawned;
        // the intent record alone is what a crash leaves.
        std::fs::create_dir_all(records.join(attempt.to_string())).unwrap();
        local::write(
            &records
                .join(attempt.to_string())
                .join(format!("{}.json", task(1))),
            &LaunchRecord {
                attempt,
                task: task(1),
                class: TaskClass::Test,
                pid: None,
                started: None,
            },
        )
        .unwrap();
        let found = Executor::recover(&records, budget(1_000)).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid, None);
        assert!(walkdir(&records).is_empty(), "the intent is cleared");

        // Boundary two: after the record, the task runs; the executor dies.
        let pid = {
            let root = dir.path().join("work");
            std::fs::create_dir_all(&root).unwrap();
            let root = AbsPath::parse(root.to_str().unwrap()).unwrap();
            let capturer = Arc::new(FakeCapturer {
                harness: Ok(SnapshotId::of(b"x")),
                asked: StdMutex::new(Vec::new()),
            });
            let executor =
                Executor::new(records.clone(), LocalResolver::open(dir.path()), capturer);
            executor
                .begin(
                    attempt,
                    JobId::from_bytes([1; 16]),
                    Generation::FIRST,
                    root,
                    Vec::new(),
                )
                .await;
            let spawned = executor
                .spawn(attempt, task(2), TaskClass::Test, sh("exec sleep 30"))
                .await
                .unwrap();
            assert!(alive(spawned.pid));
            spawned.pid
        };
        assert!(
            alive(pid),
            "the task outlives the dropped executor, as it would a killed daemon"
        );
        let record: LaunchRecord = local::read(
            &records
                .join(attempt.to_string())
                .join(format!("{}.json", task(2))),
        )
        .unwrap()
        .unwrap();
        assert_eq!(record.pid, Some(pid));
        let found = Executor::recover(&records, budget(1_000)).await.unwrap();
        assert_eq!(found.len(), 1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!alive(pid), "recovery stopped the recorded group");
        assert!(walkdir(&records).is_empty());
        // A pid the record names that now belongs to another process is
        // never signalled: the start time does not match.
        std::fs::create_dir_all(records.join(attempt.to_string())).unwrap();
        local::write(
            &records
                .join(attempt.to_string())
                .join(format!("{}.json", task(3))),
            &LaunchRecord {
                attempt,
                task: task(3),
                class: TaskClass::Test,
                pid: Some(std::process::id()),
                started: Some("not when this process started".into()),
            },
        )
        .unwrap();
        Executor::recover(&records, budget(1_000)).await.unwrap();
        assert!(walkdir(&records).is_empty());
    }
}
