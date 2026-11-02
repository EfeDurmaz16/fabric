//! Managed jobs (ADR 0019): the job, its tasks, its attempts and the
//! operations that changed them, kept by one authority (ADR 0020), with
//! the task lifecycle, the work package, the source bindings, the
//! placement and the facade token of M5 (ADRs 0023 to 0027).
//!
//! Invariant: a managed job's state is exactly the replay of its events.
//! The authority checks every command and observation here, writes the
//! events this module returns in one transaction with the state they fold
//! into, and never writes state any other way. Like the rest of the crate,
//! this module does no IO and makes no id: the journal adapter
//! (`fabric-journal`) passes ids, times and stored bytes in.
//!
//! The rules a caller can rely on:
//!
//! - A command carries an [`OperationKey`]. The same key with the same
//!   command gets the same answer; the same key with another command is
//!   [`DomainError::OperationConflict`] ([`dedupe`]).
//! - A grant gets the job's next [`Generation`]; a report at an older
//!   generation is recorded as refused and changes nothing.
//! - A lost worker makes its attempt [`AttemptStatus::Lost`]. The task
//!   stays open, and only an explicit `StartAttempt` or `Continue` grants again.
//! - Nothing here turns a timeout into a replay: an operation whose outcome
//!   is not observed stays `Dispatched` or `Unknown`.
//! - A create's effect is its initial placement: it stays `Accepted` until
//!   a `Placed` observation grants the first attempt or parks the job.
//!
//! ```
//! use fabric_core::harness::HarnessKind;
//! use fabric_core::id::{JobId, NodeId};
//! use fabric_core::managed::*;
//! use fabric_core::path::AbsPath;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ids = |n: u8| NewIds {
//!     job: JobId::from_bytes([1; 16]),
//!     task: TaskId::from_bytes([n; 16]),
//!     attempt: AttemptId::from_bytes([n; 16]),
//!     operation: OperationId::from_bytes([n; 16]),
//!     at_ms: 0,
//! };
//! let create = Envelope {
//!     key: OperationKey::parse("create-1")?,
//!     job: None,
//!     expected: None,
//!     body: Command::CreateJob {
//!         intent: Intent {
//!             goal: "fix the test".into(),
//!             harness: HarnessKind::Codex,
//!             acceptance: vec!["cargo test passes".into()],
//!         },
//!         source: SourceBinding::Repository { path: AbsPath::parse("/src/repo")? },
//!         policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
//!     },
//! };
//! let made = accept_command(None, &create, &ids(2))?;
//! let job = apply_batch(None, made.revision, &made.events)?;
//! assert!(job.parked(), "nothing runs until the initial placement");
//!
//! let start = Envelope {
//!     key: OperationKey::parse("start-1")?,
//!     job: Some(job.job.id),
//!     expected: Some(job.job.revision),
//!     body: Command::StartAttempt {
//!         task: job.job.primary,
//!         resource: ResourceRef::Node { node: NodeId::from_bytes([9; 32]) },
//!         initial_checkpoint: None,
//!     },
//! };
//! let granted = accept_command(Some(&job), &start, &ids(3))?;
//! let job = apply_batch(Some(job), granted.revision, &granted.events)?;
//! assert_eq!(job.attempts[0].generation, Generation::FIRST);
//!
//! // A second grant waits until the first attempt can no longer run.
//! let again = Envelope {
//!     key: OperationKey::parse("start-2")?,
//!     expected: Some(job.job.revision),
//!     ..start
//! };
//! assert!(matches!(
//!     accept_command(Some(&job), &again, &ids(4)),
//!     Err(DomainError::AttemptStillLive { .. })
//! ));
//! # Ok(()) }
//! ```

mod canon;
pub mod facade;
mod frame;
mod ids;
pub mod lifecycle;
mod model;
pub mod package;
pub mod placement;
pub mod secret_ref;
pub mod source;
mod transition;

pub use frame::{
    EventFrame, FrameError, MANAGED_FORMAT, MANAGED_FORMAT_V1, StoredPart, request_digest, v1,
};
pub use ids::{
    AttemptId, EventDigest, EventSeq, Generation, MAX_OPERATION_KEY, OperationId, OperationKey,
    OperationKeyError, ParseCounterError, RequestDigest, TaskId, WorkRevision,
};
pub use lifecycle::{
    CheckpointCut, CutDigest, DrainBudget, EvidenceKind, EvidenceRef, HandoffBlocker,
    OutcomePolicy, ReceiptStatus, TaskClass, TaskReceipt, WorkspaceRevision,
};
pub use model::{
    AcceptedOperation, Attempt, AttemptStatus, Command, CommandKind, Dedupe, DomainError,
    EffectRequest, Envelope, ExitOutcome, Intent, JobMode, JobPolicy, JobState,
    MAX_EXPORT_ALLOWLIST, MAX_SECRETS, ManagedEvent, ManagedJob, ManagedState, NewIds, Note,
    Observation, Operation, OperationMark, OperationProgress, OperationStatus, PlacementPolicy,
    ResourceRef, RestartPolicy, SourceBinding, StoredResult, Task, TaskKind, TaskOutcome,
    TaskState, Transition,
};
pub use package::{
    AllowedFidelity, Fidelity, FidelityRefusal, PACKAGE_MAX_BYTES, PACKAGE_VERSION, PackageDigest,
    PackageError, PackageInput, TEXT_MAX_BYTES, WorkPackage, build_package, offered_fidelity,
};
pub use placement::{NodeAdvert, PlacementDecision, PlacementKind};
pub use secret_ref::{SECRET_ENV_PREFIX, SecretName, SecretRef, SecretScope, SecretValue};
pub use source::{CommitId, GitUri, SourceError};
pub use transition::{
    accept_command, apply_batch, apply_observation, dedupe, mark_operation, replay,
};
