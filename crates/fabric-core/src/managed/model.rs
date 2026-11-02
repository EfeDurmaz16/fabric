//! What a managed job is made of, the commands that change it, the
//! observations an executor reports and the events the authority records
//! (ADR 0019, docs/managed-jobs.md).

use serde::{Deserialize, Serialize};

use super::fence::{CoveredPath, FenceVerdict};
use super::ids::{
    AttemptId, Generation, OperationId, OperationKey, RequestDigest, TaskId, WorkRevision,
};
use crate::harness::HarnessKind;
use crate::id::{JobId, NodeId, SnapshotId};
use crate::path::AbsPath;

/// How often an executor node heartbeats for its live attempts (ADR 0028).
pub const HEARTBEAT_INTERVAL_MS: u64 = 2_000;

/// How long without a heartbeat makes an attempt's resource unavailable (ADR 0028).
pub const UNAVAILABLE_AFTER_MS: u64 = 10_000;

/// How a job is kept, fixed when it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobMode {
    /// A ledger file owned by one node (ADRs 0001 to 0003).
    Standalone,
    /// Journal rows owned by one authority (ADRs 0019 and 0020).
    Managed,
}

/// The work the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    /// The goal in the user's words.
    pub goal: String,
    /// The harness the primary task runs under.
    pub harness: HarnessKind,
}

/// Where the job's workspace comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceBinding {
    /// A repository on the authority's host.
    Repository {
        /// Its absolute path.
        path: AbsPath,
    },
    /// A published snapshot, such as a standalone job's, imported as a new job's start.
    Snapshot {
        /// The snapshot.
        snapshot: SnapshotId,
    },
}

/// What may restart a task after its attempt ends without finishing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RestartPolicy {
    /// Nothing restarts it; the task waits for a decision.
    Block,
    /// Only an explicit `StartAttempt` command restarts it.
    ExplicitOnly,
}

/// What the authority may do by itself when an attempt is unavailable (ADR 0028).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryPolicy {
    /// Record it and wait for an operator's `Recover` or `Supersede`. The default.
    #[default]
    Block,
    /// Issue `Recover` itself once the attempt is unavailable, when every
    /// admission condition of `recovery_decision` holds.
    RestartFromAcknowledged,
}

impl RecoveryPolicy {
    /// The name the evidence and the client API use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            RecoveryPolicy::Block => "block",
            RecoveryPolicy::RestartFromAcknowledged => "restart_from_acknowledged",
        }
    }
}

/// The job's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobPolicy {
    /// The restart policy of the primary task.
    pub restart: RestartPolicy,
    /// What the authority may do by itself when an attempt is unavailable.
    pub recovery: RecoveryPolicy,
}

impl JobPolicy {
    /// The code the canonical encoding stores: the restart policy's
    /// variant plus twice the recovery policy's. A policy written before
    /// M6 is the restart variant alone (0 or 1), so it reads back with
    /// `recovery: Block` and a `Block` policy encodes to the bytes it
    /// always had, which keeps every stored digest and request digest of
    /// an older job the same (ADR 0028).
    fn code(self) -> u32 {
        let restart = match self.restart {
            RestartPolicy::Block => 0,
            RestartPolicy::ExplicitOnly => 1,
        };
        let recovery = match self.recovery {
            RecoveryPolicy::Block => 0,
            RecoveryPolicy::RestartFromAcknowledged => 1,
        };
        restart + 2 * recovery
    }

    fn from_code(code: u32) -> Option<JobPolicy> {
        let restart = match code % 2 {
            0 => RestartPolicy::Block,
            _ => RestartPolicy::ExplicitOnly,
        };
        let recovery = match code / 2 {
            0 => RecoveryPolicy::Block,
            1 => RecoveryPolicy::RestartFromAcknowledged,
            _ => return None,
        };
        Some(JobPolicy { restart, recovery })
    }
}

impl Serialize for JobPolicy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.code())
    }
}

impl<'de> Deserialize<'de> for JobPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = u32::deserialize(deserializer)?;
        JobPolicy::from_code(code)
            .ok_or_else(|| serde::de::Error::custom(format!("job policy code {code}")))
    }
}

/// What an M5 task's outcome policy says happens to it at the end of a
/// drain budget (ADR 0023), as M6 reads it for the recovery admission.
/// The M5 code lands the type under this name at the restack; until then
/// the primary task of a managed job is a harness task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutcomePolicy {
    /// A build or test: interrupted and restarted from the job's newest checkpoint.
    CancelAndRestart,
    /// A harness turn: continued from its own acknowledged checkpoint.
    Checkpoint,
    /// An opaque task: never restarted by the authority.
    Block,
}

impl OutcomePolicy {
    /// The name ADR 0023 uses.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            OutcomePolicy::CancelAndRestart => "cancel_and_restart",
            OutcomePolicy::Checkpoint => "checkpoint",
            OutcomePolicy::Block => "block",
        }
    }

    /// The outcome policy of a job's primary task: a harness task, so
    /// `Checkpoint`, until the M5 task kinds land.
    #[must_use]
    pub fn of_primary(_intent: &Intent) -> OutcomePolicy {
        OutcomePolicy::Checkpoint
    }
}

/// Why an M5 placement excluded a node (ADR 0026), as the recovery
/// decision records it. The M5 code lands the type at the restack.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Exclusion {
    /// The node.
    pub node: NodeId,
    /// Why it is not eligible.
    pub reason: String,
}

/// Why an automatic restart did not happen; the job stays under `Block`
/// with this recorded, never under a weaker restart.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryBlock {
    /// The task's outcome policy (ADR 0023) is `block`: an `opaque` task.
    OutcomeBlocks {
        /// The outcome policy's name.
        kind: String,
    },
    /// Nothing acknowledged to restart from: for a `checkpoint` task, no
    /// acknowledged checkpoint of this attempt; for a `cancel_and_restart`
    /// task, none of the job.
    NoAcknowledgedCheckpoint,
    /// The M4 replacement barrier holds: these provider operations may still act.
    ReplacementBlocked {
        /// The blocking provider operations, oldest first.
        blockers: Vec<OperationId>,
    },
    /// M5 placement found no eligible node with the unavailable node excluded.
    NoEligibleNode {
        /// Every candidate and why it was excluded.
        exclusions: Vec<Exclusion>,
    },
}

/// The digest of a checkpoint cut (ADR 0024): the receipts, facts and
/// evidence refs the cut names, canonically encoded and hashed with blake3.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CutDigest([u8; 32]);

impl CutDigest {
    /// A digest from its bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> CutDigest {
        CutDigest(bytes)
    }

    /// The digest's bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// The digest of a cut's canonical bytes.
    #[must_use]
    pub fn of(cut: &[u8]) -> CutDigest {
        CutDigest(*blake3::hash(cut).as_bytes())
    }
}

impl std::fmt::Display for CutDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for CutDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CutDigest({self})")
    }
}

/// What the authority verified before it accepted a checkpoint as the
/// job's. Written to the journal as `CheckpointAcknowledged`, never to the
/// store: the store's row says the bytes are complete on this node, the
/// event says the authority accepted them under the generation that made them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointAck {
    /// The attempt that captured it.
    pub attempt: AttemptId,
    /// The generation it was captured under.
    pub generation: Generation,
    /// The snapshot.
    pub snapshot: SnapshotId,
    /// Equal to the `cut` the runner reported in `Captured`.
    pub cut: CutDigest,
    /// How many manifests `fetch_snapshot` published, each after its bytes
    /// hashed to the id asked for (ADR 0014).
    pub manifests: u32,
    /// How many chunks it kept, each after its bytes hashed to the id.
    pub chunks: u64,
    /// The bytes it fetched.
    pub bytes: u64,
    /// The node the bytes were fetched from: the attempt's resource.
    pub fetched_from: NodeId,
}

/// One checkpoint of a job as the state keeps it: captured by its attempt,
/// and acknowledged by the authority or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The attempt that captured it.
    pub attempt: AttemptId,
    /// The generation it was captured under.
    pub generation: Generation,
    /// The snapshot.
    pub snapshot: SnapshotId,
    /// The cut the runner reported.
    pub cut: CutDigest,
    /// What the authority verified, once it did.
    pub ack: Option<CheckpointAck>,
}

/// Where a managed job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobState {
    /// It takes commands.
    Active,
    /// Its primary task finished.
    Finished,
    /// A `CancelJob` command ended it.
    Cancelled,
}

/// A managed job: the projection of its events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedJob {
    /// Its id, never reused.
    pub id: JobId,
    /// Its revision: one per committed change.
    pub revision: WorkRevision,
    /// The generation of its latest grant, or one before any grant.
    pub generation: Generation,
    /// What the user asked for.
    pub intent: Intent,
    /// Where the workspace comes from.
    pub source: SourceBinding,
    /// Its policy.
    pub policy: JobPolicy,
    /// Where it is in its life.
    pub state: JobState,
    /// The primary task, made with the job.
    pub primary: TaskId,
}

/// What kind of task a task is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskKind {
    /// The job's primary harness task, made with the job.
    Primary,
    /// A child task registered by a command.
    Child {
        /// The caller's label for it.
        label: String,
    },
}

/// Where a task is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskState {
    /// Not finished; an attempt may run it.
    Open,
    /// Finished as succeeded.
    Succeeded,
    /// Finished as failed.
    Failed,
    /// Finished as cancelled.
    Cancelled,
}

/// One logical piece of a job's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Its id.
    pub id: TaskId,
    /// Its job.
    pub job: JobId,
    /// The task that registered it; `None` for the primary task.
    pub parent: Option<TaskId>,
    /// Its kind.
    pub kind: TaskKind,
    /// What may restart it.
    pub restart: RestartPolicy,
    /// Where it is in its life.
    pub state: TaskState,
}

/// Where an attempt runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceRef {
    /// A local node; the one form in M2.
    Node {
        /// The node.
        node: NodeId,
    },
}

/// Where an attempt is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttemptStatus {
    /// Recorded and not observed started.
    Granted,
    /// Observed started.
    Running,
    /// Observed ended with success.
    Succeeded,
    /// Observed ended with failure.
    Failed,
    /// Observed ended by a cancel.
    Cancelled,
    /// Observed ended at a checkpoint.
    Checkpointed {
        /// The snapshot it left.
        snapshot: SnapshotId,
    },
    /// Its worker is gone; a verified stop. The task stays open.
    Lost,
    /// The authority cannot tell.
    Unknown,
    /// Its grant was taken away while it may have been alive (ADR 0028).
    /// Does not `may_run`; the task stays open.
    Superseded {
        /// The generation granted in its place, `None` while nothing is.
        by: Option<Generation>,
    },
}

impl AttemptStatus {
    /// Whether the attempt may still run: no new grant while it is.
    #[must_use]
    pub fn may_run(&self) -> bool {
        matches!(
            self,
            AttemptStatus::Granted | AttemptStatus::Running | AttemptStatus::Unknown
        )
    }
}

/// One execution of one task on one resource under one generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    /// Its id, never reused.
    pub id: AttemptId,
    /// The task it runs.
    pub task: TaskId,
    /// The task's job.
    pub job: JobId,
    /// Where it runs.
    pub resource: ResourceRef,
    /// The generation of its grant; unique in its job.
    pub generation: Generation,
    /// The snapshot it starts from, if any.
    pub initial_checkpoint: Option<SnapshotId>,
    /// The operation that granted it.
    pub granted_by: OperationId,
    /// Where it is in its life.
    pub status: AttemptStatus,
    /// When the authority last marked its resource unavailable, while it
    /// is `unknown` for that reason (ADR 0028).
    pub unavailable_since_ms: Option<u64>,
}

/// The kind of a command, as the journal's `operations.kind` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandKind {
    /// `Command::CreateJob`.
    CreateJob,
    /// `Command::StartAttempt`.
    StartAttempt,
    /// `Command::RegisterTask`.
    RegisterTask,
    /// `Command::FinishTask`.
    FinishTask,
    /// `Command::CancelJob`.
    CancelJob,
    /// `Command::Supersede`.
    Supersede,
    /// `Command::Recover`.
    Recover,
}

impl CommandKind {
    /// Whether the command asks an executor for an effect after it commits.
    /// Only a grant does (`StartAttempt`, and `Recover`, which grants);
    /// every other command is complete once committed.
    #[must_use]
    pub fn has_effect(self) -> bool {
        self.grants()
    }

    /// Whether the command grants an attempt.
    #[must_use]
    pub fn grants(self) -> bool {
        matches!(self, CommandKind::StartAttempt | CommandKind::Recover)
    }

    /// The name the journal stores.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            CommandKind::CreateJob => "create_job",
            CommandKind::StartAttempt => "start_attempt",
            CommandKind::RegisterTask => "register_task",
            CommandKind::FinishTask => "finish_task",
            CommandKind::CancelJob => "cancel_job",
            CommandKind::Supersede => "supersede",
            CommandKind::Recover => "recover",
        }
    }
}

/// Where an operation is in its life (docs/managed-jobs.md, Durable operation states).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    /// Durable; nothing dispatched.
    Accepted,
    /// Recorded before the executor was asked; an effect may have happened.
    Dispatched,
    /// An observation said it succeeded, or it had no effect to wait for.
    Succeeded,
    /// An observation said it failed.
    Failed,
    /// An observation said it was cancelled.
    Cancelled,
    /// Refused before any effect. Terminal.
    Rejected {
        /// Why.
        refusal: DomainError,
    },
    /// The authority cannot tell; only an observation moves it on.
    Unknown,
}

impl OperationStatus {
    /// The name the journal stores.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            OperationStatus::Accepted => "accepted",
            OperationStatus::Dispatched => "dispatched",
            OperationStatus::Succeeded => "succeeded",
            OperationStatus::Failed => "failed",
            OperationStatus::Cancelled => "cancelled",
            OperationStatus::Rejected { .. } => "rejected",
            OperationStatus::Unknown => "unknown",
        }
    }
}

/// A later status an operation can be marked with, by the authority or an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationMark {
    /// About to ask the executor.
    Dispatched,
    /// Observed succeeded.
    Succeeded,
    /// Observed failed.
    Failed,
    /// Observed cancelled.
    Cancelled,
    /// Cannot tell.
    Unknown,
}

impl OperationMark {
    /// The status the mark sets.
    #[must_use]
    pub fn status(self) -> OperationStatus {
        match self {
            OperationMark::Dispatched => OperationStatus::Dispatched,
            OperationMark::Succeeded => OperationStatus::Succeeded,
            OperationMark::Failed => OperationStatus::Failed,
            OperationMark::Cancelled => OperationStatus::Cancelled,
            OperationMark::Unknown => OperationStatus::Unknown,
        }
    }
}

/// One accepted command of a job, as the job's events tell it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationProgress {
    /// Its id.
    pub id: OperationId,
    /// The caller's key.
    pub key: OperationKey,
    /// The digest of the command.
    pub digest: RequestDigest,
    /// The kind of command.
    pub kind: CommandKind,
    /// The revision it was accepted at.
    pub revision: WorkRevision,
    /// Where it is.
    pub status: OperationStatus,
}

/// A durable operation as the journal keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    /// Its id.
    pub id: OperationId,
    /// The caller's key.
    pub key: OperationKey,
    /// The job it acted on; `None` for a refused command that named no job the journal has.
    pub job: Option<JobId>,
    /// The revision the caller expected.
    pub expected_revision: Option<WorkRevision>,
    /// The digest of the command.
    pub digest: RequestDigest,
    /// Where it is.
    pub status: OperationStatus,
}

/// How a task finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskOutcome {
    /// It did what it was for.
    Succeeded,
    /// It did not.
    Failed,
    /// The caller gave it up.
    Cancelled,
}

impl TaskOutcome {
    pub(crate) fn state(self) -> TaskState {
        match self {
            TaskOutcome::Succeeded => TaskState::Succeeded,
            TaskOutcome::Failed => TaskState::Failed,
            TaskOutcome::Cancelled => TaskState::Cancelled,
        }
    }
}

/// A command a caller sends the authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Makes a managed job and its primary task.
    CreateJob {
        /// What the user asked for.
        intent: Intent,
        /// Where the workspace comes from.
        source: SourceBinding,
        /// The job's policy.
        policy: JobPolicy,
    },
    /// Grants a new attempt of an open task at the job's next generation.
    StartAttempt {
        /// The task.
        task: TaskId,
        /// Where it runs.
        resource: ResourceRef,
        /// The snapshot it starts from.
        initial_checkpoint: Option<SnapshotId>,
    },
    /// Registers a child task.
    RegisterTask {
        /// The task that registers it.
        parent: TaskId,
        /// Its kind; never `Primary`.
        kind: TaskKind,
        /// What may restart it.
        restart: RestartPolicy,
    },
    /// Finishes a task none of whose attempts may still run.
    FinishTask {
        /// The task.
        task: TaskId,
        /// How it finished.
        outcome: TaskOutcome,
    },
    /// Ends the job; refused while an attempt may still run.
    CancelJob {
        /// Why, for the history.
        reason: String,
    },
    /// Takes the grant away from an `unknown` attempt and grants nothing (ADR 0028).
    Supersede {
        /// The attempt.
        attempt: AttemptId,
        /// Why, for the history.
        reason: String,
    },
    /// `Supersede` and `StartAttempt` in one transaction: the next attempt
    /// at the next generation, placed on `resource`, from the acknowledged
    /// checkpoint `from` (ADR 0028).
    Recover {
        /// The `unknown` attempt whose grant is taken.
        attempt: AttemptId,
        /// Where the next attempt runs.
        resource: ResourceRef,
        /// The acknowledged checkpoint it starts from.
        from: SnapshotId,
        /// Why, for the history.
        reason: String,
    },
}

impl Command {
    /// The kind of the command.
    #[must_use]
    pub fn kind(&self) -> CommandKind {
        match self {
            Command::CreateJob { .. } => CommandKind::CreateJob,
            Command::StartAttempt { .. } => CommandKind::StartAttempt,
            Command::RegisterTask { .. } => CommandKind::RegisterTask,
            Command::FinishTask { .. } => CommandKind::FinishTask,
            Command::CancelJob { .. } => CommandKind::CancelJob,
            Command::Supersede { .. } => CommandKind::Supersede,
            Command::Recover { .. } => CommandKind::Recover,
        }
    }
}

/// How an attempt's process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExitOutcome {
    /// It exited with success.
    Succeeded,
    /// It exited with failure.
    Failed {
        /// Its exit code, when it had one.
        code: Option<i32>,
    },
    /// It was cancelled.
    Cancelled,
}

/// What an executor, or the authority's own check, reports about an attempt or an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Observation {
    /// The attempt started.
    Started {
        /// The attempt.
        attempt: AttemptId,
        /// The generation the reporter acts under.
        generation: Generation,
    },
    /// The attempt ended at a checkpoint.
    Checkpointed {
        /// The attempt.
        attempt: AttemptId,
        /// The generation the reporter acts under.
        generation: Generation,
        /// The snapshot it left.
        snapshot: SnapshotId,
    },
    /// The attempt's process ended.
    Exited {
        /// The attempt.
        attempt: AttemptId,
        /// The generation the reporter acts under.
        generation: Generation,
        /// How.
        outcome: ExitOutcome,
    },
    /// The attempt's worker is gone, verified.
    Lost {
        /// The attempt.
        attempt: AttemptId,
        /// The generation the reporter acts under.
        generation: Generation,
    },
    /// The outcome of a dispatched operation cannot be told.
    Unknown {
        /// The operation.
        operation: OperationId,
    },
    /// The attempt captured a checkpoint and keeps running (ADR 0028).
    /// `Checkpointed` stays the report of an attempt that ended at one.
    Captured {
        /// The attempt.
        attempt: AttemptId,
        /// The generation the reporter acts under.
        generation: Generation,
        /// The snapshot it published into its own store.
        snapshot: SnapshotId,
        /// The digest of the cut.
        cut: CutDigest,
    },
    /// Made by the authority itself, never by a reporter: no heartbeat for
    /// the attempt in `UNAVAILABLE_AFTER_MS`. Moves `granted` or `running`
    /// to `unknown`; says nothing about whether the attempt stopped.
    ResourceUnavailable {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
        /// The last heartbeat the authority holds; `None` for one never heard from.
        last_heartbeat_ms: Option<u64>,
        /// The authority's clock when it decided.
        at_ms: u64,
    },
    /// Made by the authority itself: a heartbeat returned for an attempt
    /// `unknown` by `ResourceUnavailable` and nothing else happened to it.
    /// Moves it back to `running`.
    ResourceReachable {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
    },
}

/// A command or an observation with the caller's key and expectations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// The caller's name for the command; the dedupe key.
    pub key: OperationKey,
    /// The job; `None` only for `CreateJob`.
    pub job: Option<JobId>,
    /// The revision the caller expects the job to be at; checked when given.
    pub expected: Option<WorkRevision>,
    /// The command.
    pub body: T,
}

/// One change the authority recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManagedEvent {
    /// A command was accepted; the events after it, up to the next
    /// `CommandAccepted`, are its effects on the history.
    CommandAccepted {
        /// The new operation.
        operation: OperationId,
        /// The caller's key.
        key: OperationKey,
        /// The digest of the command.
        digest: RequestDigest,
        /// The kind of command.
        kind: CommandKind,
    },
    /// The job was made.
    JobCreated {
        /// The job.
        job: JobId,
        /// What the user asked for.
        intent: Intent,
        /// Where the workspace comes from.
        source: SourceBinding,
        /// The job's policy.
        policy: JobPolicy,
        /// Its primary task.
        primary: TaskId,
    },
    /// A task was registered.
    TaskRegistered {
        /// The task.
        task: TaskId,
        /// The task that registered it.
        parent: Option<TaskId>,
        /// Its kind.
        kind: TaskKind,
        /// What may restart it.
        restart: RestartPolicy,
    },
    /// An attempt was granted at a new generation.
    AttemptGranted {
        /// The attempt.
        attempt: AttemptId,
        /// The task.
        task: TaskId,
        /// Where it runs.
        resource: ResourceRef,
        /// Its generation.
        generation: Generation,
        /// The snapshot it starts from.
        initial_checkpoint: Option<SnapshotId>,
        /// The operation that granted it.
        operation: OperationId,
    },
    /// An observation changed an attempt's status.
    AttemptObserved {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
        /// Its new status.
        status: AttemptStatus,
    },
    /// A task finished.
    TaskFinished {
        /// The task.
        task: TaskId,
        /// How.
        outcome: TaskOutcome,
    },
    /// The job was cancelled.
    JobCancelled {
        /// Why.
        reason: String,
    },
    /// An observation named an older generation and changed nothing else.
    StaleObservationRefused {
        /// The attempt it named.
        attempt: AttemptId,
        /// The generation it named.
        generation: Generation,
        /// The job's generation then.
        current: Generation,
    },
    /// An operation's status changed.
    OperationMarked {
        /// The operation.
        operation: OperationId,
        /// The new status.
        status: OperationMark,
    },
    /// An attempt captured a checkpoint into its own store and keeps
    /// running (ADR 0028); the bytes may exist only there.
    CheckpointCaptured {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
        /// The snapshot.
        snapshot: SnapshotId,
        /// The digest of the cut.
        cut: CutDigest,
    },
    /// The authority fetched and verified a captured checkpoint (ADR 0028).
    CheckpointAcknowledged {
        /// What it verified.
        ack: CheckpointAck,
    },
    /// A covered write other than a report was refused by the fence; a
    /// refused report stays `StaleObservationRefused` (ADR 0028).
    CoveredWriteRefused {
        /// The path.
        path: CoveredPath,
        /// The attempt the token named.
        attempt: AttemptId,
        /// The generation the token named.
        generation: Generation,
        /// Why.
        verdict: FenceVerdict,
    },
    /// No heartbeat for the attempt in `UNAVAILABLE_AFTER_MS` (ADR 0028).
    AttemptUnavailable {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
        /// The last heartbeat the authority holds.
        last_heartbeat_ms: Option<u64>,
        /// The authority's clock when it decided.
        since_ms: u64,
    },
    /// A heartbeat returned for an attempt `AttemptUnavailable` made `unknown` (ADR 0028).
    AttemptReachable {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
    },
    /// An attempt's grant was taken away while it may have been alive (ADR 0028).
    GrantSuperseded {
        /// The attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
        /// The generation granted in its place in the same batch, if one was.
        by: Option<Generation>,
        /// Why.
        reason: String,
    },
    /// An automatic restart did not happen; the job waits under `Block` (ADR 0028).
    RecoveryBlocked {
        /// The unavailable attempt.
        attempt: AttemptId,
        /// Its generation.
        generation: Generation,
        /// Why.
        reason: RecoveryBlock,
    },
}

impl ManagedEvent {
    /// The name the journal's `events.kind` stores.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            ManagedEvent::CommandAccepted { .. } => "command_accepted",
            ManagedEvent::JobCreated { .. } => "job_created",
            ManagedEvent::TaskRegistered { .. } => "task_registered",
            ManagedEvent::AttemptGranted { .. } => "attempt_granted",
            ManagedEvent::AttemptObserved { .. } => "attempt_observed",
            ManagedEvent::TaskFinished { .. } => "task_finished",
            ManagedEvent::JobCancelled { .. } => "job_cancelled",
            ManagedEvent::StaleObservationRefused { .. } => "stale_observation_refused",
            ManagedEvent::OperationMarked { .. } => "operation_marked",
            ManagedEvent::CheckpointCaptured { .. } => "checkpoint_captured",
            ManagedEvent::CheckpointAcknowledged { .. } => "checkpoint_acknowledged",
            ManagedEvent::CoveredWriteRefused { .. } => "covered_write_refused",
            ManagedEvent::AttemptUnavailable { .. } => "attempt_unavailable",
            ManagedEvent::AttemptReachable { .. } => "attempt_reachable",
            ManagedEvent::GrantSuperseded { .. } => "grant_superseded",
            ManagedEvent::RecoveryBlocked { .. } => "recovery_blocked",
        }
    }
}

/// Why the domain refused a command, an observation or a history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum DomainError {
    /// The key was used for another command.
    #[error("operation key {key} was used for another command")]
    OperationConflict {
        /// The key.
        key: OperationKey,
    },
    /// The job is not at the revision the caller expected.
    #[error("the job is at revision {current}, the command expected {expected}")]
    RevisionMismatch {
        /// What the caller expected.
        expected: WorkRevision,
        /// Where the job is.
        current: WorkRevision,
    },
    /// A report named a generation that is not its attempt's or is newer than the job's.
    #[error("generation {given} does not match generation {current}")]
    StaleGeneration {
        /// The generation named.
        given: Generation,
        /// The generation it must be.
        current: Generation,
    },
    /// The job has no such task.
    #[error("the job has no task {task}")]
    UnknownTask {
        /// The task.
        task: TaskId,
    },
    /// The job has no such attempt.
    #[error("the job has no attempt {attempt}")]
    UnknownAttempt {
        /// The attempt.
        attempt: AttemptId,
    },
    /// The journal has no such managed job.
    #[error("no managed job {job}")]
    UnknownJob {
        /// The job.
        job: JobId,
    },
    /// The job has no such operation.
    #[error("the job has no operation {operation}")]
    UnknownOperation {
        /// The operation.
        operation: OperationId,
    },
    /// An attempt may still run, so a new grant, a finish or a cancel waits.
    #[error("attempt {attempt} may still run")]
    AttemptStillLive {
        /// The attempt.
        attempt: AttemptId,
    },
    /// The attempt already ended another way.
    #[error("attempt {attempt} already ended")]
    AttemptEnded {
        /// The attempt.
        attempt: AttemptId,
    },
    /// The task is finished.
    #[error("task {task} is finished")]
    TaskClosed {
        /// The task.
        task: TaskId,
    },
    /// The job is finished or cancelled.
    #[error("job {job} is finished or cancelled")]
    JobClosed {
        /// The job.
        job: JobId,
    },
    /// A command for one mode named a job of the other.
    #[error("job {job} is a {mode:?} job")]
    WrongMode {
        /// The job.
        job: JobId,
        /// Its mode.
        mode: JobMode,
    },
    /// The parent is not a task of the job, or the command tried to register a second primary task.
    #[error("task {task} cannot be the parent here")]
    BadParent {
        /// The task.
        task: TaskId,
    },
    /// A `CreateJob` named a job, or another command named none.
    #[error("the envelope's job does not fit the command")]
    BadEnvelope,
    /// The operation cannot move to that status from where it is.
    #[error("operation {operation} cannot become {to:?}")]
    OperationState {
        /// The operation.
        operation: OperationId,
        /// The status asked for.
        to: OperationMark,
    },
    /// A counter would pass its limit.
    #[error("a counter of the job is exhausted")]
    Exhausted,
    /// A stored history does not replay.
    #[error("the history does not replay: {reason}")]
    Broken {
        /// What is wrong.
        reason: String,
    },
    /// A provider effect of the job may still act on its resources (a
    /// create unresolved, an exec that may run, a stop not confirmed), so
    /// no replacement attempt is granted (docs/managed-jobs.md, the
    /// replacement barrier). Appended after every earlier variant, so
    /// stored refusals keep their encoding.
    #[error("provider operation {operation} blocks a replacement attempt")]
    ReplacementBlocked {
        /// The oldest provider operation that blocks it.
        operation: OperationId,
    },
    /// `Captured` repeats a snapshot the attempt already reported, or an
    /// acknowledgement names one it never reported (ADR 0028).
    #[error("attempt {attempt} has no reported checkpoint {snapshot}")]
    UnknownCheckpoint {
        /// The attempt.
        attempt: AttemptId,
        /// The snapshot.
        snapshot: SnapshotId,
    },
    /// The acknowledgement's cut digest is not the one the runner reported.
    #[error("checkpoint {snapshot} was reported with cut {reported}, verified as {verified}")]
    CutMismatch {
        /// The snapshot.
        snapshot: SnapshotId,
        /// What the runner reported.
        reported: CutDigest,
        /// What the authority verified.
        verified: CutDigest,
    },
    /// A `Recover` names a snapshot no `CheckpointAcknowledged` covers.
    #[error("checkpoint {snapshot} is not acknowledged by the authority")]
    NotAcknowledged {
        /// The snapshot.
        snapshot: SnapshotId,
    },
    /// `Supersede` or `Recover` on an attempt that is not `unknown`: a
    /// reachable attempt is handed over by the M5 planned handoff, never recovered.
    #[error(
        "attempt {attempt} is {status:?}, not unknown; a reachable attempt is handed over, not recovered"
    )]
    AttemptReachable {
        /// The attempt.
        attempt: AttemptId,
        /// Its status.
        status: AttemptStatus,
    },
    /// `CreateJob` with `RestartFromAcknowledged` for a primary task whose
    /// outcome policy is `block` (an `opaque` task). Refused at admission.
    #[error("restart_from_acknowledged is not supported for a {kind} task")]
    RecoveryUnsupported {
        /// The outcome policy's name.
        kind: String,
    },
}

/// The ids and the time a command may need, made by the authority before it applies one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewIds {
    /// The id of a job the command creates.
    pub job: JobId,
    /// The id of a task the command creates.
    pub task: TaskId,
    /// The id of an attempt the command grants.
    pub attempt: AttemptId,
    /// The id of the operation.
    pub operation: OperationId,
    /// Unix milliseconds.
    pub at_ms: u64,
}

/// A managed job with its tasks, attempts and accepted operations: the replay of its events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedState {
    /// The job.
    pub job: ManagedJob,
    /// Its tasks, in the order they were registered.
    pub tasks: Vec<Task>,
    /// Its attempts, in the order they were granted.
    pub attempts: Vec<Attempt>,
    /// Its accepted operations, in the order they were accepted.
    pub operations: Vec<OperationProgress>,
    /// Its checkpoints, in the order they were captured (ADR 0028).
    pub checkpoints: Vec<Checkpoint>,
}

impl ManagedState {
    /// The checkpoint record of `snapshot`, if an attempt reported it.
    #[must_use]
    pub fn checkpoint(&self, snapshot: SnapshotId) -> Option<&Checkpoint> {
        self.checkpoints.iter().find(|c| c.snapshot == snapshot)
    }

    /// Every acknowledged checkpoint of the job, newest first, each with
    /// the attempt and generation that made it.
    #[must_use]
    pub fn acknowledged(&self) -> Vec<&CheckpointAck> {
        self.checkpoints
            .iter()
            .rev()
            .filter_map(|c| c.ack.as_ref())
            .collect()
    }

    /// Checkpoints reported `Captured` and never acknowledged; listed, and
    /// a restart point for nothing.
    #[must_use]
    pub fn captured_only(&self) -> Vec<(AttemptId, Generation, SnapshotId)> {
        self.checkpoints
            .iter()
            .filter(|c| c.ack.is_none())
            .map(|c| (c.attempt, c.generation, c.snapshot))
            .collect()
    }

    /// The task with this id.
    #[must_use]
    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// The attempt with this id.
    #[must_use]
    pub fn attempt(&self, id: AttemptId) -> Option<&Attempt> {
        self.attempts.iter().find(|a| a.id == id)
    }

    /// The operation with this id.
    #[must_use]
    pub fn operation(&self, id: OperationId) -> Option<&OperationProgress> {
        self.operations.iter().find(|o| o.id == id)
    }

    /// The attempt that may still run, if any: at most one, the current grant.
    #[must_use]
    pub fn live_attempt(&self) -> Option<&Attempt> {
        self.attempts.iter().find(|a| a.status.may_run())
    }

    /// Whether `operation` is an accepted grant that may still be
    /// dispatched: its attempt has not been seen running or ending, was
    /// not lost, and is the job's current generation. A lost or running
    /// attempt is never launched again, and an older grant never runs
    /// next to a newer one.
    #[must_use]
    pub fn grant_may_dispatch(&self, operation: OperationId) -> bool {
        self.operation(operation)
            .is_some_and(|op| op.status == OperationStatus::Accepted && op.kind.grants())
            && self.attempts.iter().any(|a| {
                a.granted_by == operation
                    && a.status == AttemptStatus::Granted
                    && a.generation == self.job.generation
            })
    }
}

/// The changes one command or observation makes: the job's next revision
/// and its events. No events means nothing changed and nothing is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// The revision the events are recorded at.
    pub revision: WorkRevision,
    /// The events, in order.
    pub events: Vec<ManagedEvent>,
}

/// The answer to an accepted command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedOperation {
    /// The new operation.
    pub operation: OperationId,
    /// The job it acted on.
    pub job: JobId,
    /// The job's revision after it.
    pub revision: WorkRevision,
    /// The events it recorded.
    pub events: Vec<ManagedEvent>,
}

/// The answer a caller got for an operation key, kept so a retry gets the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredResult {
    /// The command was accepted.
    Accepted(AcceptedOperation),
    /// The command was refused before any effect.
    Rejected(DomainError),
}

/// What to do with a command whose key may have been seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dedupe {
    /// The key is new: apply the command.
    New,
    /// The same command under the same key: answer with the stored result.
    Replay,
    /// Another command under the same key: refuse it.
    Conflict,
}
