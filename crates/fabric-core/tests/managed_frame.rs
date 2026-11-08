//! Codec format 2 (ADR 0023 to 0025): a format 1 frame, answer or part
//! written by M2 to M4 decodes with its frozen shape and widens to the M5
//! shape with the M2 defaults, keeping its bytes and so its digest.

#![allow(
    clippy::unwrap_used,
    reason = "the helpers here are test code; a failed unwrap is the test failing"
)]

use fabric_core::harness::HarnessKind;
use fabric_core::id::JobId;
use fabric_core::managed::*;
use fabric_core::path::AbsPath;

fn framed_v1<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = vec![MANAGED_FORMAT_V1];
    bytes.extend(postcard::to_stdvec(value).unwrap());
    bytes
}

fn v1_create_frame() -> v1::EventFrame {
    v1::EventFrame {
        job: JobId::from_bytes([1; 16]),
        revision: WorkRevision::FIRST,
        operation: Some(OperationId::from_bytes([2; 16])),
        prev: None,
        at_ms: 7,
        event: v1::ManagedEvent::JobCreated {
            job: JobId::from_bytes([1; 16]),
            intent: v1::Intent {
                goal: "make the greeting louder".into(),
                harness: HarnessKind::Codex,
            },
            source: SourceBinding::Repository {
                path: AbsPath::parse("/src/repo").unwrap(),
            },
            policy: v1::JobPolicy {
                restart: RestartPolicy::Block,
            },
            primary: TaskId::from_bytes([3; 16]),
        },
    }
}

#[test]
fn a_format_1_frame_decodes_and_widens_to_the_m2_defaults() {
    let bytes = framed_v1(&v1_create_frame());
    let frame = EventFrame::decode(&bytes).unwrap();
    let ManagedEvent::JobCreated {
        intent,
        policy,
        source,
        ..
    } = &frame.event
    else {
        panic!("not a create: {frame:?}");
    };
    assert_eq!(intent.acceptance, Vec::<String>::new());
    assert_eq!(intent.goal, "make the greeting louder");
    assert_eq!(*policy, JobPolicy::m2_defaults(RestartPolicy::Block));
    assert!(matches!(source, SourceBinding::Repository { .. }));
    // The digest is of the stored bytes, so it is unchanged by the read.
    assert_eq!(
        EventFrame::digest(&bytes).to_bytes(),
        *blake3::hash(&bytes).as_bytes()
    );
    // Re-encoding gives format 2 bytes, which nothing does to a stored frame.
    assert_eq!(frame.encode().unwrap()[0], MANAGED_FORMAT);
    assert_ne!(frame.encode().unwrap(), bytes);

    // A format 1 command acceptance has an effect only for a grant, and a
    // registered task is a harness task.
    let accepted = framed_v1(&v1::EventFrame {
        event: v1::ManagedEvent::CommandAccepted {
            operation: OperationId::from_bytes([2; 16]),
            key: OperationKey::parse("create").unwrap(),
            digest: RequestDigest::from_bytes([9; 32]),
            kind: CommandKind::CreateJob,
        },
        ..v1_create_frame()
    });
    assert!(matches!(
        EventFrame::decode(&accepted).unwrap().event,
        ManagedEvent::CommandAccepted { effect: false, .. }
    ));
    let grant = framed_v1(&v1::EventFrame {
        event: v1::ManagedEvent::CommandAccepted {
            operation: OperationId::from_bytes([2; 16]),
            key: OperationKey::parse("start").unwrap(),
            digest: RequestDigest::from_bytes([9; 32]),
            kind: CommandKind::StartAttempt,
        },
        ..v1_create_frame()
    });
    assert!(matches!(
        EventFrame::decode(&grant).unwrap().event,
        ManagedEvent::CommandAccepted { effect: true, .. }
    ));
    let registered = framed_v1(&v1::EventFrame {
        event: v1::ManagedEvent::TaskRegistered {
            task: TaskId::from_bytes([3; 16]),
            parent: None,
            kind: TaskKind::Primary,
            restart: RestartPolicy::Block,
        },
        ..v1_create_frame()
    });
    assert!(matches!(
        EventFrame::decode(&registered).unwrap().event,
        ManagedEvent::TaskRegistered {
            class: TaskClass::Harness,
            ..
        }
    ));
}

#[test]
fn format_1_parts_and_answers_widen_and_format_1_is_never_written() {
    let intent = framed_v1(&v1::Intent {
        goal: "g".into(),
        harness: HarnessKind::ClaudeCode,
    });
    let widened = <Intent as StoredPart>::from_stored(&intent).unwrap();
    assert_eq!(widened.acceptance, Vec::<String>::new());
    assert_eq!(widened.to_stored().unwrap()[0], MANAGED_FORMAT);

    let policy = framed_v1(&v1::JobPolicy {
        restart: RestartPolicy::ExplicitOnly,
    });
    assert_eq!(
        <JobPolicy as StoredPart>::from_stored(&policy).unwrap(),
        JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
    );

    // Parts whose shape did not change read format 1 bytes as they are.
    let status = framed_v1(&AttemptStatus::Lost);
    assert_eq!(
        <AttemptStatus as StoredPart>::from_stored(&status).unwrap(),
        AttemptStatus::Lost
    );

    let answer = framed_v1(&v1::StoredResult::Accepted(v1::AcceptedOperation {
        operation: OperationId::from_bytes([2; 16]),
        job: JobId::from_bytes([1; 16]),
        revision: WorkRevision::FIRST,
        events: vec![v1::ManagedEvent::JobCancelled { reason: "r".into() }],
    }));
    let StoredResult::Accepted(accepted) = StoredResult::decode(&answer).unwrap() else {
        panic!("not accepted");
    };
    assert_eq!(
        accepted.events,
        vec![ManagedEvent::JobCancelled { reason: "r".into() }]
    );
    let refusal = framed_v1(&v1::StoredResult::Rejected(DomainError::Exhausted));
    assert_eq!(
        StoredResult::decode(&refusal).unwrap(),
        StoredResult::Rejected(DomainError::Exhausted)
    );

    // A format 1 frame with trailing bytes or a non-canonical body is refused as before.
    let mut trailing = framed_v1(&v1_create_frame());
    trailing.push(0);
    assert!(EventFrame::decode(&trailing).is_err());
}
