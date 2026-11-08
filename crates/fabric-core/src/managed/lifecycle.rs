//! The task lifecycle of a managed job (ADR 0023): what class a task has,
//! what an interruption means for it, what a quiescence may wait, what a
//! task leaves behind as its receipt, and the checkpoint cut that binds a
//! snapshot to the receipts, facts and evidence it was taken with.
//!
//! Invariant: an interrupted build or test is `Interrupted`, never passed
//! and never failed, and nothing here folds it into either. A cut's digest
//! is the digest of its canonical JSON under one domain separator, so two
//! nodes that hold the same cut compute the same digest.

use serde::{Deserialize, Serialize};

use super::canon::Json;
use super::ids::{AttemptId, EventDigest, OperationId, TaskId};
use super::model::DomainError;
use crate::id::SnapshotId;
use crate::path::RelPath;

/// What a task is, behaviorally: what an interruption means for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskClass {
    /// A build; cut by a cancel, restarted from scratch.
    Build,
    /// A test; cut by a cancel, restarted from scratch, never recorded passed when cut.
    Test,
    /// An agent turn through `fabric-harness`; checkpointed by the suspend path.
    Harness,
    /// Nothing is known about it; a handoff blocks on it.
    Opaque,
}

/// What the executor does with a task at the end of its drain budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutcomePolicy {
    /// Stop it and record it interrupted; the next attempt starts it again.
    CancelAndRestart,
    /// Take the supported checkpoint; block the handoff if that fails.
    Checkpoint,
    /// Block the handoff naming the task.
    Block,
}

impl TaskClass {
    /// The policy of the class.
    #[must_use]
    pub fn outcome_policy(self) -> OutcomePolicy {
        match self {
            TaskClass::Build | TaskClass::Test => OutcomePolicy::CancelAndRestart,
            TaskClass::Harness => OutcomePolicy::Checkpoint,
            TaskClass::Opaque => OutcomePolicy::Block,
        }
    }

    /// The name the journal's `tasks.class` stores.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            TaskClass::Build => "build",
            TaskClass::Test => "test",
            TaskClass::Harness => "harness",
            TaskClass::Opaque => "opaque",
        }
    }

    /// The class from its stored name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<TaskClass> {
        Some(match name {
            "build" => TaskClass::Build,
            "test" => TaskClass::Test,
            "harness" => TaskClass::Harness,
            "opaque" => TaskClass::Opaque,
            _ => return None,
        })
    }
}

/// How long a quiescence waits for a task to settle, in milliseconds.
/// It bounds the wait, never the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DrainBudget(u32);

impl DrainBudget {
    /// The shortest budget: one second.
    pub const MIN_MS: u32 = 1_000;
    /// The budget a job gets when its policy names none.
    pub const DEFAULT_MS: u32 = 30_000;
    /// The longest budget: ten minutes.
    pub const MAX_MS: u32 = 600_000;

    /// Checks `ms` against the bounds.
    pub fn new(ms: u32) -> Result<Self, DomainError> {
        if (Self::MIN_MS..=Self::MAX_MS).contains(&ms) {
            Ok(DrainBudget(ms))
        } else {
            Err(DomainError::BadPolicy {
                field: "drain_budget_ms".into(),
            })
        }
    }

    /// The budget in milliseconds.
    #[must_use]
    pub fn ms(self) -> u32 {
        self.0
    }

    /// The budget as a duration.
    #[must_use]
    pub fn duration(self) -> std::time::Duration {
        std::time::Duration::from_millis(u64::from(self.0))
    }
}

impl Default for DrainBudget {
    fn default() -> Self {
        DrainBudget(Self::DEFAULT_MS)
    }
}

/// Where a task was when its receipt was written.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReceiptStatus {
    /// Still running when the receipt was written.
    Running,
    /// Its process ended on its own.
    Exited,
    /// Cut by a cancel or a quiescence; never passed, never failed.
    Interrupted,
    /// Checkpointed by the suspend path.
    Checkpointed {
        /// The snapshot that holds it.
        snapshot: SnapshotId,
    },
    /// The handoff blocked on it.
    Blocked {
        /// Why.
        reason: String,
    },
}

/// What a task left behind at a cut or a stop.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskReceipt {
    /// The task.
    pub task: TaskId,
    /// Its class.
    pub class: TaskClass,
    /// Where it was.
    pub status: ReceiptStatus,
    /// Its exit code; `Some` only for `Exited`.
    pub exit: Option<i32>,
    /// True exactly for `Interrupted` and for a harness checkpointed mid-turn.
    pub interrupted: bool,
    /// The workspace revision at the receipt, when one was taken.
    pub workspace: Option<WorkspaceRevision>,
}

impl TaskReceipt {
    /// Whether the receipt may be read as the task having passed: only an
    /// exit with code zero that was not interrupted.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.status == ReceiptStatus::Exited && self.exit == Some(0) && !self.interrupted
    }

    pub(crate) fn to_json(&self) -> Json {
        let (status, snapshot, reason) = match &self.status {
            ReceiptStatus::Running => ("running", None, None),
            ReceiptStatus::Exited => ("exited", None, None),
            ReceiptStatus::Interrupted => ("interrupted", None, None),
            ReceiptStatus::Checkpointed { snapshot } => ("checkpointed", Some(*snapshot), None),
            ReceiptStatus::Blocked { reason } => ("blocked", None, Some(reason.as_str())),
        };
        Json::obj([
            ("task", Json::str(self.task.to_string())),
            ("class", Json::str(self.class.name())),
            ("status", Json::str(status)),
            (
                "snapshot",
                Json::opt(snapshot, |s| Json::str(s.to_string())),
            ),
            ("reason", Json::opt(reason, Json::str)),
            ("exit", Json::opt(self.exit, Json::num)),
            ("interrupted", Json::Bool(self.interrupted)),
            (
                "workspace",
                Json::opt(self.workspace, WorkspaceRevision::to_json),
            ),
        ])
    }
}

/// The revision of a workspace: the snapshot whose manifest names the
/// bundle chain head, and the digest of the job's latest journal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceRevision {
    /// The snapshot at the head of the bundle chain.
    pub bundle_head: SnapshotId,
    /// The digest of the job's latest event when the revision was taken.
    pub ledger_head: EventDigest,
}

impl WorkspaceRevision {
    pub(crate) fn to_json(self) -> Json {
        Json::obj([
            ("bundle_head", Json::str(self.bundle_head.to_string())),
            ("ledger_head", Json::str(self.ledger_head.to_string())),
        ])
    }
}

/// What kind of evidence a reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvidenceKind {
    /// A checkpoint: a snapshot, or a cut.
    Checkpoint,
    /// An exit.
    Exit,
    /// A receipt.
    Receipt,
}

impl EvidenceKind {
    fn name(self) -> &'static str {
        match self {
            EvidenceKind::Checkpoint => "checkpoint",
            EvidenceKind::Exit => "exit",
            EvidenceKind::Receipt => "receipt",
        }
    }
}

/// A reference to accepted evidence, by digest; never the evidence itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// The digest of the evidence.
    pub digest: [u8; 32],
    /// The attempt it belongs to.
    pub attempt: AttemptId,
    /// What it is.
    pub kind: EvidenceKind,
}

impl EvidenceRef {
    pub(crate) fn to_json(self) -> Json {
        Json::obj([
            ("digest", Json::hex(&self.digest)),
            ("attempt", Json::str(self.attempt.to_string())),
            ("kind", Json::str(self.kind.name())),
        ])
    }
}

/// The domain separator of a cut digest.
const CUT_DOMAIN: &[u8] = b"fabric.managed.cut.v1\0";

/// The digest of a checkpoint cut's canonical JSON.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CutDigest(pub [u8; 32]);

impl std::fmt::Display for CutDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for CutDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CutDigest({self})")
    }
}

/// A checkpoint cut: the snapshot with what a continuation needs to read
/// it correctly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointCut {
    /// The snapshot the capture published.
    pub snapshot: SnapshotId,
    /// One receipt per task of the attempt.
    pub receipts: Vec<TaskReceipt>,
    /// The node's facts for the job, sorted by name, as `NodeFacts::iter` gives them.
    pub facts: Vec<(String, String)>,
    /// The accepted evidence of the attempt, by digest.
    pub evidence: Vec<EvidenceRef>,
}

impl CheckpointCut {
    /// The canonical JSON of the cut.
    #[must_use]
    pub fn canonical_json(&self) -> Vec<u8> {
        Json::obj([
            ("version", Json::num(1_u32)),
            ("snapshot", Json::str(self.snapshot.to_string())),
            ("receipts", Json::arr(&self.receipts, TaskReceipt::to_json)),
            (
                "facts",
                Json::arr(&self.facts, |(k, v)| {
                    Json::Arr(vec![Json::str(k.clone()), Json::str(v.clone())])
                }),
            ),
            (
                "evidence",
                Json::arr(self.evidence.iter().copied(), EvidenceRef::to_json),
            ),
        ])
        .to_bytes()
    }

    /// The digest of the cut: blake3 over the domain separator and the canonical JSON.
    #[must_use]
    pub fn digest(&self) -> CutDigest {
        let mut hasher = blake3::Hasher::new();
        hasher.update(CUT_DOMAIN);
        hasher.update(&self.canonical_json());
        CutDigest(*hasher.finalize().as_bytes())
    }
}

/// Why a strong handoff is blocked. None of these fails the job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HandoffBlocker {
    /// A process in a task's group that no registered task owns.
    EscapedChild {
        /// Its pid.
        pid: u32,
        /// The group it was found in.
        group: u32,
    },
    /// A process outside every group that holds a file under the root open for writing.
    UncontrolledWriter {
        /// Its pid.
        pid: u32,
        /// The file, relative to the root.
        path: RelPath,
    },
    /// A harness task's checkpoint failed.
    CheckpointFailed {
        /// The task.
        task: TaskId,
        /// The capture's reason.
        reason: String,
    },
    /// An opaque task is running.
    Opaque {
        /// The task.
        task: TaskId,
    },
    /// A provider operation is dispatched or unknown (ADR 0022).
    UnknownEffect {
        /// The operation.
        operation: OperationId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drain_budget_keeps_its_bounds() {
        assert!(DrainBudget::new(999).is_err());
        assert_eq!(DrainBudget::new(1_000).unwrap().ms(), 1_000);
        assert_eq!(DrainBudget::default().ms(), 30_000);
        assert!(DrainBudget::new(600_000).is_ok());
        assert!(DrainBudget::new(600_001).is_err());
    }

    #[test]
    fn classes_map_to_their_policies_and_names() {
        assert_eq!(
            TaskClass::Build.outcome_policy(),
            OutcomePolicy::CancelAndRestart
        );
        assert_eq!(
            TaskClass::Test.outcome_policy(),
            OutcomePolicy::CancelAndRestart
        );
        assert_eq!(
            TaskClass::Harness.outcome_policy(),
            OutcomePolicy::Checkpoint
        );
        assert_eq!(TaskClass::Opaque.outcome_policy(), OutcomePolicy::Block);
        for class in [
            TaskClass::Build,
            TaskClass::Test,
            TaskClass::Harness,
            TaskClass::Opaque,
        ] {
            assert_eq!(TaskClass::from_name(class.name()), Some(class));
        }
        assert_eq!(TaskClass::from_name("primary"), None);
    }

    #[test]
    fn an_interrupted_receipt_never_passes() {
        let receipt = TaskReceipt {
            task: TaskId::from_bytes([1; 16]),
            class: TaskClass::Test,
            status: ReceiptStatus::Interrupted,
            exit: None,
            interrupted: true,
            workspace: None,
        };
        assert!(!receipt.passed());
        let exited = TaskReceipt {
            status: ReceiptStatus::Exited,
            exit: Some(0),
            interrupted: false,
            ..receipt.clone()
        };
        assert!(exited.passed());
        let cut_mid_exit = TaskReceipt {
            interrupted: true,
            ..exited
        };
        assert!(!cut_mid_exit.passed());
    }

    #[test]
    fn a_cut_digest_is_of_its_canonical_json_and_changes_with_every_field() {
        let cut = CheckpointCut {
            snapshot: SnapshotId::of(b"s"),
            receipts: vec![],
            facts: vec![("os".into(), "macos".into())],
            evidence: vec![],
        };
        let json = String::from_utf8(cut.canonical_json()).unwrap();
        assert!(
            json.starts_with(
                r#"{"evidence":[],"facts":[["os","macos"]],"receipts":[],"snapshot":""#
            )
        );
        assert!(json.ends_with(r#""version":"1"}"#));
        let mut other = cut.clone();
        other.facts.push(("arch".into(), "arm64".into()));
        assert_ne!(cut.digest(), other.digest());
        assert_eq!(cut.digest(), cut.clone().digest());
    }
}
