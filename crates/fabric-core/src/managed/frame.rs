//! The byte forms of managed values: request digests, stored event frames
//! and stored answers.
//!
//! Invariant: every form is a format byte followed by the canonical
//! postcard bytes of ADR 0002 (`crate::codec`): decoding refuses another
//! format byte, trailing bytes and a non-canonical encoding, so a digest
//! of the bytes is a digest of the value. A frame names the digest of the
//! job's previous frame, so a job's history is a hash chain.
//!
//! Two formats are read. Format `1` is what M2 to M4 wrote; its shapes
//! are frozen in [`v1`] and widen to the M5 shapes with the M2 defaults
//! (`JobPolicy::m2_defaults`, `TaskClass::Harness`, no acceptance, an
//! effect only for a grant). Format `2` is what M5 writes. A format 1
//! frame keeps its bytes and its digest: nothing re-encodes it, so no
//! chain changes when a journal is opened by this crate.

use serde::{Deserialize, Serialize};

use super::ids::{EventDigest, OperationId, RequestDigest, WorkRevision};
use super::lifecycle::TaskClass;
use super::model::{
    AcceptedOperation, AttemptStatus, Command, Envelope, Intent, JobPolicy, ManagedEvent,
    ResourceRef, SourceBinding, StoredResult, TaskKind,
};
use crate::codec::{self, CodecError};
use crate::id::JobId;

/// The format byte of every managed frame and stored answer this crate writes.
pub const MANAGED_FORMAT: u8 = 2;

/// The format byte M2 to M4 wrote; read, never written.
pub const MANAGED_FORMAT_V1: u8 = 1;

/// The domain separator of a request digest.
const COMMAND_DOMAIN: &[u8] = b"fabric.managed.command.v1\0";

/// Why managed bytes did not decode.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// No bytes at all.
    #[error("an empty frame")]
    Empty,
    /// A format byte this crate does not read.
    #[error("managed format {0} is not supported")]
    UnknownFormat(u8),
    /// The bytes after the format byte are not a canonical value.
    #[error(transparent)]
    Codec(#[from] CodecError),
}

/// The digest of a command: blake3 over the domain separator and the
/// format 2 bytes of the job, the expected revision and the command. The
/// key is not part of it: it is what the digest is compared under.
pub fn request_digest(envelope: &Envelope<Command>) -> Result<RequestDigest, CodecError> {
    let body = encode_framed(&(envelope.job, envelope.expected, &envelope.body))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(COMMAND_DOMAIN);
    hasher.update(&body);
    Ok(RequestDigest::from_bytes(*hasher.finalize().as_bytes()))
}

/// One stored event: the event with where it sits in its job's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventFrame {
    /// The job.
    pub job: JobId,
    /// The revision of the batch it belongs to.
    pub revision: WorkRevision,
    /// The operation whose command made it; `None` for an observation's events.
    pub operation: Option<OperationId>,
    /// The digest of the job's previous frame; `None` for its first.
    pub prev: Option<EventDigest>,
    /// Unix milliseconds when it was committed; informational.
    pub at_ms: u64,
    /// The event.
    pub event: ManagedEvent,
}

fn encode_framed<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut bytes = vec![MANAGED_FORMAT];
    bytes.extend(codec::encode(value)?);
    Ok(bytes)
}

/// Splits the format byte off, refusing an empty frame.
fn split_format(bytes: &[u8]) -> Result<(u8, &[u8]), FrameError> {
    bytes
        .split_first()
        .map(|(&f, rest)| (f, rest))
        .ok_or(FrameError::Empty)
}

/// Decodes a value whose shape is the same in both formats.
fn decode_same<T: Serialize + serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, FrameError> {
    match split_format(bytes)? {
        (MANAGED_FORMAT | MANAGED_FORMAT_V1, rest) => Ok(codec::decode(rest)?),
        (other, _) => Err(FrameError::UnknownFormat(other)),
    }
}

/// Decodes a value whose format 1 shape is `V1`, widening it.
fn decode_widening<T, V1>(bytes: &[u8]) -> Result<T, FrameError>
where
    T: Serialize + serde::de::DeserializeOwned,
    V1: Serialize + serde::de::DeserializeOwned + Into<T>,
{
    match split_format(bytes)? {
        (MANAGED_FORMAT, rest) => Ok(codec::decode(rest)?),
        (MANAGED_FORMAT_V1, rest) => Ok(codec::decode::<V1>(rest)?.into()),
        (other, _) => Err(FrameError::UnknownFormat(other)),
    }
}

impl EventFrame {
    /// The frame's stored bytes.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        encode_framed(self)
    }

    /// Reads stored bytes back, refusing any but the canonical form of
    /// format 2 or format 1.
    pub fn decode(bytes: &[u8]) -> Result<EventFrame, FrameError> {
        decode_widening::<EventFrame, v1::EventFrame>(bytes)
    }

    /// The digest of a frame's stored bytes.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> EventDigest {
        EventDigest::from_bytes(*blake3::hash(bytes).as_bytes())
    }
}

impl StoredResult {
    /// The answer's stored bytes: what a retry under its key gets back.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        encode_framed(self)
    }

    /// Reads stored bytes back, refusing any but the canonical form.
    pub fn decode(bytes: &[u8]) -> Result<StoredResult, FrameError> {
        decode_widening::<StoredResult, v1::StoredResult>(bytes)
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A part of a managed job the journal keeps in a column of its own, in
/// the same framed canonical form as an event.
pub trait StoredPart: Serialize + serde::de::DeserializeOwned + sealed::Sealed {
    /// The part's stored bytes.
    fn to_stored(&self) -> Result<Vec<u8>, CodecError> {
        encode_framed(self)
    }

    /// Reads stored bytes back, refusing any but the canonical form; a
    /// format 1 part is read with its frozen shape and widened.
    fn from_stored(bytes: &[u8]) -> Result<Self, FrameError> {
        decode_same(bytes)
    }
}

macro_rules! stored_part {
    ($($t:ty),*) => {$(
        impl sealed::Sealed for $t {}
        impl StoredPart for $t {}
    )*};
}

stored_part!(SourceBinding, TaskKind, ResourceRef, AttemptStatus);

impl sealed::Sealed for Intent {}
impl StoredPart for Intent {
    fn from_stored(bytes: &[u8]) -> Result<Self, FrameError> {
        decode_widening::<Intent, v1::Intent>(bytes)
    }
}

impl sealed::Sealed for JobPolicy {}
impl StoredPart for JobPolicy {
    fn from_stored(bytes: &[u8]) -> Result<Self, FrameError> {
        decode_widening::<JobPolicy, v1::JobPolicy>(bytes)
    }
}

/// The M2 shapes, frozen: what a format 1 frame or part decodes with.
/// Each widens with `Into` to its M5 shape and the M2 defaults. The
/// variant order of every enum here is the order M2 wrote; the types the
/// M2 shapes share with M5 (`SourceBinding`, `TaskKind`, `ResourceRef`,
/// `AttemptStatus`, `CommandKind`, `DomainError`) only gained variants at
/// the end, so their M5 types read the M2 bytes.
pub mod v1 {
    use serde::{Deserialize, Serialize};

    use super::super::ids::{
        AttemptId, EventDigest, Generation, OperationId, OperationKey, RequestDigest, TaskId,
        WorkRevision,
    };
    use super::super::model;
    use super::super::model::{
        AttemptStatus, CommandKind, DomainError, OperationMark, ResourceRef, RestartPolicy,
        SourceBinding, TaskKind, TaskOutcome,
    };
    use super::TaskClass;
    use crate::harness::HarnessKind;
    use crate::id::{JobId, SnapshotId};

    /// The M2 intent.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Intent {
        /// The goal.
        pub goal: String,
        /// The harness.
        pub harness: HarnessKind,
    }

    impl From<Intent> for model::Intent {
        fn from(v: Intent) -> Self {
            model::Intent {
                goal: v.goal,
                harness: v.harness,
                acceptance: Vec::new(),
            }
        }
    }

    /// The M2 policy.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub struct JobPolicy {
        /// The restart policy.
        pub restart: RestartPolicy,
    }

    impl From<JobPolicy> for model::JobPolicy {
        fn from(v: JobPolicy) -> Self {
            model::JobPolicy::m2_defaults(v.restart)
        }
    }

    /// The M2 event.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[allow(
        missing_docs,
        reason = "the fields are those of the M5 event of the same name"
    )]
    pub enum ManagedEvent {
        CommandAccepted {
            operation: OperationId,
            key: OperationKey,
            digest: RequestDigest,
            kind: CommandKind,
        },
        JobCreated {
            job: JobId,
            intent: Intent,
            source: SourceBinding,
            policy: JobPolicy,
            primary: TaskId,
        },
        TaskRegistered {
            task: TaskId,
            parent: Option<TaskId>,
            kind: TaskKind,
            restart: RestartPolicy,
        },
        AttemptGranted {
            attempt: AttemptId,
            task: TaskId,
            resource: ResourceRef,
            generation: Generation,
            initial_checkpoint: Option<SnapshotId>,
            operation: OperationId,
        },
        AttemptObserved {
            attempt: AttemptId,
            generation: Generation,
            status: AttemptStatus,
        },
        TaskFinished {
            task: TaskId,
            outcome: TaskOutcome,
        },
        JobCancelled {
            reason: String,
        },
        StaleObservationRefused {
            attempt: AttemptId,
            generation: Generation,
            current: Generation,
        },
        OperationMarked {
            operation: OperationId,
            status: OperationMark,
        },
    }

    impl From<ManagedEvent> for model::ManagedEvent {
        fn from(v: ManagedEvent) -> Self {
            match v {
                ManagedEvent::CommandAccepted {
                    operation,
                    key,
                    digest,
                    kind,
                } => model::ManagedEvent::CommandAccepted {
                    operation,
                    key,
                    digest,
                    kind,
                    // In M2 only a grant asked an executor for an effect.
                    effect: kind == CommandKind::StartAttempt,
                },
                ManagedEvent::JobCreated {
                    job,
                    intent,
                    source,
                    policy,
                    primary,
                } => model::ManagedEvent::JobCreated {
                    job,
                    intent: intent.into(),
                    source,
                    policy: policy.into(),
                    primary,
                },
                ManagedEvent::TaskRegistered {
                    task,
                    parent,
                    kind,
                    restart,
                } => model::ManagedEvent::TaskRegistered {
                    task,
                    parent,
                    kind,
                    restart,
                    class: TaskClass::Harness,
                },
                ManagedEvent::AttemptGranted {
                    attempt,
                    task,
                    resource,
                    generation,
                    initial_checkpoint,
                    operation,
                } => model::ManagedEvent::AttemptGranted {
                    attempt,
                    task,
                    resource,
                    generation,
                    initial_checkpoint,
                    operation,
                },
                ManagedEvent::AttemptObserved {
                    attempt,
                    generation,
                    status,
                } => model::ManagedEvent::AttemptObserved {
                    attempt,
                    generation,
                    status,
                },
                ManagedEvent::TaskFinished { task, outcome } => {
                    model::ManagedEvent::TaskFinished { task, outcome }
                }
                ManagedEvent::JobCancelled { reason } => {
                    model::ManagedEvent::JobCancelled { reason }
                }
                ManagedEvent::StaleObservationRefused {
                    attempt,
                    generation,
                    current,
                } => model::ManagedEvent::StaleObservationRefused {
                    attempt,
                    generation,
                    current,
                },
                ManagedEvent::OperationMarked { operation, status } => {
                    model::ManagedEvent::OperationMarked { operation, status }
                }
            }
        }
    }

    /// The M2 frame.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[allow(missing_docs, reason = "the fields are those of the M5 frame")]
    pub struct EventFrame {
        pub job: JobId,
        pub revision: WorkRevision,
        pub operation: Option<OperationId>,
        pub prev: Option<EventDigest>,
        pub at_ms: u64,
        pub event: ManagedEvent,
    }

    impl From<EventFrame> for super::EventFrame {
        fn from(v: EventFrame) -> Self {
            super::EventFrame {
                job: v.job,
                revision: v.revision,
                operation: v.operation,
                prev: v.prev,
                at_ms: v.at_ms,
                event: v.event.into(),
            }
        }
    }

    /// The M2 accepted answer.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[allow(missing_docs, reason = "the fields are those of the M5 answer")]
    pub struct AcceptedOperation {
        pub operation: OperationId,
        pub job: JobId,
        pub revision: WorkRevision,
        pub events: Vec<ManagedEvent>,
    }

    /// The M2 stored answer.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[allow(missing_docs, reason = "the variants are those of the M5 answer")]
    pub enum StoredResult {
        Accepted(AcceptedOperation),
        Rejected(DomainError),
    }

    impl From<StoredResult> for model::StoredResult {
        fn from(v: StoredResult) -> Self {
            match v {
                StoredResult::Accepted(a) => {
                    model::StoredResult::Accepted(super::AcceptedOperation {
                        operation: a.operation,
                        job: a.job,
                        revision: a.revision,
                        events: a.events.into_iter().map(Into::into).collect(),
                    })
                }
                StoredResult::Rejected(e) => model::StoredResult::Rejected(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_2_frame_round_trips_and_other_formats_are_refused() {
        let frame = EventFrame {
            job: JobId::from_bytes([1; 16]),
            revision: WorkRevision::FIRST,
            operation: None,
            prev: None,
            at_ms: 5,
            event: ManagedEvent::JobCancelled { reason: "x".into() },
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(bytes[0], MANAGED_FORMAT);
        assert_eq!(EventFrame::decode(&bytes).unwrap(), frame);
        let mut other = bytes.clone();
        other[0] = 3;
        assert!(matches!(
            EventFrame::decode(&other),
            Err(FrameError::UnknownFormat(3))
        ));
        assert!(matches!(EventFrame::decode(&[]), Err(FrameError::Empty)));
    }
}
