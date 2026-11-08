//! The portable work package and continuation fidelity (ADR 0024).
//!
//! Invariant: a package is a projection of accepted job state, built by
//! one pure function from the journal's state and notes, bounded in size,
//! free of anything that looks like a credential, with evidence by digest
//! only. A fidelity refusal is a refusal: `highest` never answers a
//! fidelity below the ones allowed, and `Unsupported` is never allowed.

use serde::{Deserialize, Serialize};

use super::canon::Json;
use super::ids::{TaskId, WorkRevision};
use super::lifecycle::{EvidenceRef, TaskClass, TaskReceipt, WorkspaceRevision};
use super::model::{Intent, ManagedState, Note, TaskState};
use super::placement::NodeAdvert;
use crate::credential::looks_like_credential;
use crate::harness::HarnessKind;
use crate::id::JobId;
use crate::version::HarnessVersion;

/// The package format this crate writes.
pub const PACKAGE_VERSION: u32 = 1;
/// The most bytes a package's canonical JSON has: 256 KiB.
pub const PACKAGE_MAX_BYTES: usize = 256 * 1024;
/// The most bytes one text field has: 16 KiB.
pub const TEXT_MAX_BYTES: usize = 16 * 1024;

/// The domain separator of a package digest.
const PACKAGE_DOMAIN: &[u8] = b"fabric.managed.package.v1\0";

/// A decision the job recorded, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// What was decided.
    pub what: String,
    /// Why.
    pub reason: String,
    /// The revision it was recorded at.
    pub revision: WorkRevision,
}

/// A question the job recorded and no answer named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// The question.
    pub text: String,
    /// The revision it was recorded at.
    pub revision: WorkRevision,
}

/// One task as the package carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// The task.
    pub task: TaskId,
    /// Its class.
    pub class: TaskClass,
    /// Where it is.
    pub state: TaskState,
    /// Its last receipt, if one was recorded.
    pub receipt: Option<TaskReceipt>,
}

/// An artifact by reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Its name.
    pub name: String,
    /// Its digest.
    pub digest: [u8; 32],
    /// Its size.
    pub bytes: u64,
}

/// What the owner may do next.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NextAction {
    /// `Continue`.
    Continue,
    /// `Checkpoint`.
    Checkpoint,
    /// `CancelJob`.
    Cancel,
    /// `FinishTask` of this task.
    FinishTask {
        /// The task.
        task: TaskId,
    },
    /// `RegisterTask` under this parent.
    RegisterTask {
        /// The parent.
        parent: TaskId,
    },
}

/// The digest of a package's canonical JSON.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PackageDigest(pub [u8; 32]);

impl std::fmt::Display for PackageDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for PackageDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PackageDigest({self})")
    }
}

/// The portable work package: what survives a harness change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPackage {
    /// `PACKAGE_VERSION`.
    pub version: u32,
    /// The job.
    pub job: JobId,
    /// The revision it projects.
    pub revision: WorkRevision,
    /// What the user asked for, acceptance included.
    pub intent: Intent,
    /// The decisions, in order.
    pub decisions: Vec<Decision>,
    /// The tasks, in order.
    pub tasks: Vec<TaskSummary>,
    /// The workspace revision.
    pub workspace: WorkspaceRevision,
    /// This attempt's accepted evidence, then prior attempts', by digest.
    pub evidence: Vec<EvidenceRef>,
    /// The open questions.
    pub questions: Vec<Question>,
    /// The artifacts, by reference.
    pub artifacts: Vec<ArtifactRef>,
    /// What the owner may do next.
    pub next_actions: Vec<NextAction>,
}

/// Why a package was not built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum PackageError {
    /// The canonical JSON is over `PACKAGE_MAX_BYTES`.
    #[error("the package has {bytes} bytes, more than {PACKAGE_MAX_BYTES}")]
    TooLarge {
        /// Its size.
        bytes: usize,
    },
    /// A text field is over `TEXT_MAX_BYTES`.
    #[error("{field} has {bytes} bytes, more than {TEXT_MAX_BYTES}")]
    TextTooLong {
        /// The field.
        field: &'static str,
        /// Its size.
        bytes: usize,
    },
    /// A text field looks like a credential.
    #[error("{field} looks like a credential")]
    CredentialLike {
        /// The field.
        field: &'static str,
    },
}

/// What a package is built from.
#[derive(Debug, Clone, Copy)]
pub struct PackageInput<'a> {
    /// The job's state.
    pub state: &'a ManagedState,
    /// Its notes, each with the revision it was recorded at.
    pub notes: &'a [(WorkRevision, Note)],
    /// The last receipt of each task, if any.
    pub receipts: &'a [TaskReceipt],
    /// The workspace revision.
    pub workspace: WorkspaceRevision,
    /// The accepted evidence, this attempt's first.
    pub evidence: &'a [EvidenceRef],
    /// The artifacts.
    pub artifacts: &'a [ArtifactRef],
}

fn check_text(field: &'static str, text: &str) -> Result<(), PackageError> {
    if text.len() > TEXT_MAX_BYTES {
        return Err(PackageError::TextTooLong {
            field,
            bytes: text.len(),
        });
    }
    if looks_like_credential(text) {
        return Err(PackageError::CredentialLike { field });
    }
    Ok(())
}

/// Builds the package of `input.state` at its revision.
pub fn build_package(input: &PackageInput<'_>) -> Result<WorkPackage, PackageError> {
    let state = input.state;
    check_text("intent.goal", &state.job.intent.goal)?;
    for text in &state.job.intent.acceptance {
        check_text("intent.acceptance", text)?;
    }
    let mut decisions = Vec::new();
    let mut questions = Vec::new();
    let mut answered = Vec::new();
    for (revision, note) in input.notes {
        match note {
            Note::Decision { what, reason } => {
                check_text("decision.what", what)?;
                check_text("decision.reason", reason)?;
                decisions.push(Decision {
                    what: what.clone(),
                    reason: reason.clone(),
                    revision: *revision,
                });
            }
            Note::Question { text } => {
                check_text("question.text", text)?;
                questions.push(Question {
                    text: text.clone(),
                    revision: *revision,
                });
            }
            Note::Answer { question, text } => {
                check_text("answer.text", text)?;
                answered.push(*question);
            }
        }
    }
    questions.retain(|q| !answered.contains(&q.revision));
    for artifact in input.artifacts {
        check_text("artifact.name", &artifact.name)?;
    }
    let tasks = state
        .tasks
        .iter()
        .map(|task| TaskSummary {
            task: task.id,
            class: task.class,
            state: task.state,
            receipt: input
                .receipts
                .iter()
                .rev()
                .find(|r| r.task == task.id)
                .cloned(),
        })
        .collect();
    let mut next_actions = Vec::new();
    if state.live_attempt().is_some() {
        next_actions.push(NextAction::Checkpoint);
    }
    next_actions.push(NextAction::Continue);
    next_actions.push(NextAction::Cancel);
    for task in state.tasks.iter().filter(|t| t.state == TaskState::Open) {
        let open_child = state
            .tasks
            .iter()
            .any(|c| c.parent == Some(task.id) && c.state == TaskState::Open);
        if !open_child {
            next_actions.push(NextAction::FinishTask { task: task.id });
        }
        next_actions.push(NextAction::RegisterTask { parent: task.id });
    }
    let package = WorkPackage {
        version: PACKAGE_VERSION,
        job: state.job.id,
        revision: state.job.revision,
        intent: state.job.intent.clone(),
        decisions,
        tasks,
        workspace: input.workspace,
        evidence: input.evidence.to_vec(),
        questions,
        artifacts: input.artifacts.to_vec(),
        next_actions,
    };
    let bytes = package.canonical_json().len();
    if bytes > PACKAGE_MAX_BYTES {
        return Err(PackageError::TooLarge { bytes });
    }
    Ok(package)
}

fn task_state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "open",
        TaskState::Succeeded => "succeeded",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

impl NextAction {
    fn to_json(&self) -> Json {
        match self {
            NextAction::Continue => Json::obj([("action", Json::str("continue"))]),
            NextAction::Checkpoint => Json::obj([("action", Json::str("checkpoint"))]),
            NextAction::Cancel => Json::obj([("action", Json::str("cancel"))]),
            NextAction::FinishTask { task } => Json::obj([
                ("action", Json::str("finish_task")),
                ("task", Json::str(task.to_string())),
            ]),
            NextAction::RegisterTask { parent } => Json::obj([
                ("action", Json::str("register_task")),
                ("parent", Json::str(parent.to_string())),
            ]),
        }
    }
}

impl WorkPackage {
    /// The canonical JSON of the package.
    #[must_use]
    pub fn canonical_json(&self) -> Vec<u8> {
        Json::obj([
            ("version", Json::num(self.version)),
            ("job", Json::str(self.job.to_string())),
            ("revision", Json::num(self.revision)),
            (
                "intent",
                Json::obj([
                    ("goal", Json::str(self.intent.goal.clone())),
                    ("harness", Json::str(self.intent.harness.to_string())),
                    (
                        "acceptance",
                        Json::arr(&self.intent.acceptance, |a| Json::str(a.clone())),
                    ),
                ]),
            ),
            (
                "decisions",
                Json::arr(&self.decisions, |d| {
                    Json::obj([
                        ("what", Json::str(d.what.clone())),
                        ("reason", Json::str(d.reason.clone())),
                        ("revision", Json::num(d.revision)),
                    ])
                }),
            ),
            (
                "tasks",
                Json::arr(&self.tasks, |t| {
                    Json::obj([
                        ("task", Json::str(t.task.to_string())),
                        ("class", Json::str(t.class.name())),
                        ("state", Json::str(task_state_name(t.state))),
                        (
                            "receipt",
                            Json::opt(t.receipt.as_ref(), TaskReceipt::to_json),
                        ),
                    ])
                }),
            ),
            ("workspace", self.workspace.to_json()),
            (
                "evidence",
                Json::arr(self.evidence.iter().copied(), EvidenceRef::to_json),
            ),
            (
                "questions",
                Json::arr(&self.questions, |q| {
                    Json::obj([
                        ("text", Json::str(q.text.clone())),
                        ("revision", Json::num(q.revision)),
                    ])
                }),
            ),
            (
                "artifacts",
                Json::arr(&self.artifacts, |a| {
                    Json::obj([
                        ("name", Json::str(a.name.clone())),
                        ("digest", Json::hex(&a.digest)),
                        ("bytes", Json::num(a.bytes)),
                    ])
                }),
            ),
            (
                "next_actions",
                Json::arr(&self.next_actions, NextAction::to_json),
            ),
        ])
        .to_bytes()
    }

    /// The digest of the package: blake3 over the domain separator and the canonical JSON.
    #[must_use]
    pub fn digest(&self) -> PackageDigest {
        let mut hasher = blake3::Hasher::new();
        hasher.update(PACKAGE_DOMAIN);
        hasher.update(&self.canonical_json());
        PackageDigest(*hasher.finalize().as_bytes())
    }
}

/// How faithfully a continuation carries on, from highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Fidelity {
    /// The adapter restores the captured session and the harness continues it.
    NativeSession,
    /// A new session, of the same or another harness, is handed the package.
    NewNativeSessionWithPackage,
    /// The workspace is restored; no session is started.
    WorkspaceOnly,
    /// A refusal with its reason; never allowed, never achieved.
    Unsupported,
}

impl Fidelity {
    /// The name the journal's `continuations.achieved` stores.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Fidelity::NativeSession => "native_session",
            Fidelity::NewNativeSessionWithPackage => "new_native_session_with_package",
            Fidelity::WorkspaceOnly => "workspace_only",
            Fidelity::Unsupported => "unsupported",
        }
    }

    /// The fidelity from its stored name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Fidelity> {
        Some(match name {
            "native_session" => Fidelity::NativeSession,
            "new_native_session_with_package" => Fidelity::NewNativeSessionWithPackage,
            "workspace_only" => Fidelity::WorkspaceOnly,
            "unsupported" => Fidelity::Unsupported,
            _ => return None,
        })
    }
}

/// Which fidelities a job allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AllowedFidelity {
    /// `NativeSession`.
    pub native_session: bool,
    /// `NewNativeSessionWithPackage`.
    pub new_native_session_with_package: bool,
    /// `WorkspaceOnly`; an opt-in.
    pub workspace_only: bool,
}

/// No allowed fidelity was offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("none of the fidelities offered is allowed")]
pub struct FidelityRefusal {
    /// What the job allows.
    pub allowed: AllowedFidelity,
    /// What was offered.
    pub offered: Vec<Fidelity>,
}

impl AllowedFidelity {
    /// What a job allows when it names nothing: a native session or a new one with the package.
    pub const DEFAULT: AllowedFidelity = AllowedFidelity {
        native_session: true,
        new_native_session_with_package: true,
        workspace_only: false,
    };

    /// Whether nothing is allowed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !(self.native_session || self.new_native_session_with_package || self.workspace_only)
    }

    /// Whether `fidelity` is allowed; `Unsupported` never is.
    #[must_use]
    pub fn allows(&self, fidelity: Fidelity) -> bool {
        match fidelity {
            Fidelity::NativeSession => self.native_session,
            Fidelity::NewNativeSessionWithPackage => self.new_native_session_with_package,
            Fidelity::WorkspaceOnly => self.workspace_only,
            Fidelity::Unsupported => false,
        }
    }

    /// The highest fidelity both allowed and offered.
    pub fn highest(&self, offered: &[Fidelity]) -> Result<Fidelity, FidelityRefusal> {
        [
            Fidelity::NativeSession,
            Fidelity::NewNativeSessionWithPackage,
            Fidelity::WorkspaceOnly,
        ]
        .into_iter()
        .find(|f| self.allows(*f) && offered.contains(f))
        .ok_or_else(|| FidelityRefusal {
            allowed: *self,
            offered: offered.to_vec(),
        })
    }
}

impl Default for AllowedFidelity {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What `target` can offer for a session captured under `source`:
/// `NativeSession` when it has the same harness, not older than the
/// session's newest writer, and the same layout; `NewNativeSessionWithPackage`
/// when it has any harness; `WorkspaceOnly` always. Pure; the adapter's own
/// restore refusals still apply at launch.
#[must_use]
pub fn offered_fidelity(
    source: &HarnessKind,
    known_writer: Option<&HarnessVersion>,
    target: &NodeAdvert,
    same_layout: bool,
) -> Vec<Fidelity> {
    let mut offered = Vec::new();
    let same_harness = target.harnesses.iter().find(|h| h.kind == *source);
    if let Some(have) = same_harness
        && same_layout
        && known_writer.is_none_or(|writer| have.version >= *writer)
    {
        offered.push(Fidelity::NativeSession);
    }
    if !target.harnesses.is_empty() {
        offered.push(Fidelity::NewNativeSessionWithPackage);
    }
    offered.push(Fidelity::WorkspaceOnly);
    offered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_highest_allowed_and_offered_fidelity_wins_and_a_refusal_is_a_refusal() {
        let allowed = AllowedFidelity::DEFAULT;
        assert_eq!(
            allowed.highest(&[Fidelity::WorkspaceOnly, Fidelity::NativeSession]),
            Ok(Fidelity::NativeSession)
        );
        assert_eq!(
            allowed.highest(&[
                Fidelity::WorkspaceOnly,
                Fidelity::NewNativeSessionWithPackage
            ]),
            Ok(Fidelity::NewNativeSessionWithPackage)
        );
        let refused = allowed.highest(&[Fidelity::WorkspaceOnly]).unwrap_err();
        assert_eq!(refused.offered, vec![Fidelity::WorkspaceOnly]);
        assert!(
            allowed
                .highest(&[Fidelity::Unsupported, Fidelity::WorkspaceOnly])
                .is_err()
        );
        let empty = AllowedFidelity {
            native_session: false,
            new_native_session_with_package: false,
            workspace_only: false,
        };
        assert!(empty.is_empty());
        assert!(empty.highest(&[Fidelity::NativeSession]).is_err());
        assert!(!AllowedFidelity::DEFAULT.allows(Fidelity::Unsupported));
    }

    #[test]
    fn fidelity_names_round_trip() {
        for f in [
            Fidelity::NativeSession,
            Fidelity::NewNativeSessionWithPackage,
            Fidelity::WorkspaceOnly,
            Fidelity::Unsupported,
        ] {
            assert_eq!(Fidelity::from_name(f.name()), Some(f));
        }
    }
}
