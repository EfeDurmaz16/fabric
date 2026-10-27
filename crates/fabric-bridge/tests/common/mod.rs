//! What the bridge tests share: a journal holding the fixtures' job, and
//! effects on it.

#![allow(dead_code, reason = "each test file uses part of this module")]

use std::path::Path;

use fabric_bridge::Effect;
use fabric_bridge::wire::{Action, CreateParams, Digest, OperationRef, Scope};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::{
    AttemptId, Command, Envelope, Generation, Intent, JobPolicy, NewIds, OperationId, OperationKey,
    ResourceRef, RestartPolicy, SourceBinding, TaskId,
};
use fabric_core::path::AbsPath;
use fabric_journal::{FILE_NAME, Journal};

/// The fixtures' job, task and attempt.
pub const JOB: &str = "01K8W3A00000000000000000J1";
/// The fixtures' task.
pub const TASK: &str = "01K8W3A00000000000000000T1";
/// The fixtures' attempt.
pub const ATTEMPT: &str = "01K8W3A00000000000000000A1";

/// A journal holding the fixtures' job with its attempt granted at generation 1.
pub fn journal(dir: &Path) -> Journal {
    let node = NodeId::from_bytes([4; 32]);
    let mut journal = Journal::create(&dir.join(FILE_NAME), node, 1).unwrap();
    let job: JobId = JOB.parse().unwrap();
    let task: TaskId = TASK.parse().unwrap();
    let attempt: AttemptId = ATTEMPT.parse().unwrap();
    let ids = |n: u8| NewIds {
        job,
        task,
        attempt,
        operation: OperationId::from_bytes([0xf0 + n; 16]),
        at_ms: 1_760_000_000_000 + u64::from(n),
    };
    let create = Envelope {
        key: OperationKey::parse("scenario-create").unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: "bridge scenario".into(),
                harness: HarnessKind::Codex,
            },
            source: SourceBinding::Repository {
                path: AbsPath::parse("/src/repo").unwrap(),
            },
            policy: JobPolicy {
                restart: RestartPolicy::ExplicitOnly,
            },
        },
    };
    journal.submit(&create, ids(1)).unwrap();
    let start = Envelope {
        key: OperationKey::parse("scenario-start").unwrap(),
        job: Some(job),
        expected: None,
        body: Command::StartAttempt {
            task,
            resource: ResourceRef::Node { node },
            initial_checkpoint: None,
        },
    };
    journal.submit(&start, ids(2)).unwrap();
    journal
}

/// The fixtures' scope.
pub fn scope() -> Scope {
    Scope {
        job: JOB.parse().unwrap(),
        task: TASK.parse().unwrap(),
        attempt: ATTEMPT.parse().unwrap(),
        generation: Generation::FIRST,
    }
}

/// Operation `n` of a test, with key `key-<n>`.
pub fn operation(n: u8) -> OperationRef {
    OperationRef {
        id: OperationId::from_bytes([n; 16]),
        key: OperationKey::parse(&format!("key-{n}")).unwrap(),
        digest: Digest([n; 32]),
    }
}

/// A `sandbox.create` of `image` as operation `n`.
pub fn create(n: u8, image: &str) -> Effect {
    Effect {
        operation: operation(n),
        scope: scope(),
        action: Action::Create(CreateParams {
            image: image.into(),
            ..CreateParams::default()
        }),
        approval: None,
        policy: None,
        deadline_ms: 30_000,
    }
}
