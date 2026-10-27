//! Every released schema version keeps a fixture journal, and the current
//! binary opens each one, migrates it when it is older and verifies it.
//! `tests/fixtures/v1.sqlite` was made by `make_v1_fixture` with schema
//! version 1; the test below pins what it holds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]

use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{
    AttemptId, AttemptStatus, Command, Envelope, ExitOutcome, Intent, JobPolicy, JobState, NewIds,
    Observation, OperationId, OperationKey, OperationMark, OperationStatus, ResourceRef,
    RestartPolicy, SourceBinding, StoredResult, TaskId, TaskKind, TaskOutcome, TaskState,
    WorkRevision,
};
use fabric_core::path::AbsPath;
use fabric_journal::{FILE_NAME, Journal, SCHEMA_VERSION};

const NODE: NodeId = NodeId::from_bytes([4; 32]);
const FIRST: JobId = JobId::from_bytes([0x01; 16]);
const SECOND: JobId = JobId::from_bytes([0x02; 16]);

fn fixture(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("v{version}.sqlite"))
}

fn ids(job: JobId, n: u8) -> NewIds {
    NewIds {
        job,
        task: TaskId::from_bytes([n; 16]),
        attempt: AttemptId::from_bytes([n; 16]),
        operation: OperationId::from_bytes([n; 16]),
        at_ms: 1_760_000_000_000 + u64::from(n),
    }
}

fn submit(journal: &mut Journal, key: &str, job: Option<JobId>, body: Command, ids: NewIds) {
    let envelope = Envelope {
        key: OperationKey::parse(key).unwrap(),
        job,
        expected: None,
        body,
    };
    journal.submit(&envelope, ids).unwrap();
}

fn create(goal: &str) -> Command {
    Command::CreateJob {
        intent: Intent {
            goal: goal.into(),
            harness: HarnessKind::ClaudeCode,
        },
        source: SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        policy: JobPolicy {
            restart: RestartPolicy::ExplicitOnly,
        },
    }
}

fn start(task: TaskId) -> Command {
    Command::StartAttempt {
        task,
        resource: ResourceRef::Node { node: NODE },
        initial_checkpoint: None,
    }
}

/// Writes a journal with one of everything version 1 stores.
#[test]
#[ignore = "makes the v1 fixture; run by hand with FABRIC_JOURNAL_FIXTURE_OUT set"]
#[allow(
    clippy::too_many_lines,
    reason = "the fixture's steps read best as one script"
)]
fn make_v1_fixture() {
    let Some(out) = std::env::var_os("FABRIC_JOURNAL_FIXTURE_OUT") else {
        return;
    };
    let path = PathBuf::from(out).join("v1.sqlite");
    let mut journal = Journal::create(&path, NODE, 1_760_000_000_000).unwrap();
    submit(
        &mut journal,
        "create-1",
        None,
        create("first"),
        ids(FIRST, 1),
    );
    let primary = TaskId::from_bytes([1; 16]);
    submit(
        &mut journal,
        "child",
        Some(FIRST),
        Command::RegisterTask {
            parent: primary,
            kind: TaskKind::Child {
                label: "tests".into(),
            },
            restart: RestartPolicy::Block,
        },
        ids(FIRST, 2),
    );
    submit(
        &mut journal,
        "start-1",
        Some(FIRST),
        start(primary),
        ids(FIRST, 3),
    );
    let first = AttemptId::from_bytes([3; 16]);
    let g1 = fabric_core::managed::Generation::FIRST;
    journal
        .mark(
            FIRST,
            OperationId::from_bytes([3; 16]),
            OperationMark::Dispatched,
            10,
        )
        .unwrap();
    journal
        .observe(
            FIRST,
            &Observation::Started {
                attempt: first,
                generation: g1,
            },
            11,
        )
        .unwrap();
    journal
        .observe(
            FIRST,
            &Observation::Lost {
                attempt: first,
                generation: g1,
            },
            12,
        )
        .unwrap();
    submit(
        &mut journal,
        "start-2",
        Some(FIRST),
        start(primary),
        ids(FIRST, 4),
    );
    let second = AttemptId::from_bytes([4; 16]);
    let g2 = g1.next().unwrap();
    journal
        .mark(
            FIRST,
            OperationId::from_bytes([4; 16]),
            OperationMark::Dispatched,
            13,
        )
        .unwrap();
    journal
        .observe(
            FIRST,
            &Observation::Checkpointed {
                attempt: second,
                generation: g2,
                snapshot: SnapshotId::of(b"fixture manifest"),
            },
            14,
        )
        .unwrap();
    journal
        .observe(
            FIRST,
            &Observation::Exited {
                attempt: first,
                generation: g1,
                outcome: ExitOutcome::Succeeded,
            },
            15,
        )
        .unwrap();
    submit(
        &mut journal,
        "start-3",
        Some(FIRST),
        start(primary),
        ids(FIRST, 5),
    );
    journal
        .mark(
            FIRST,
            OperationId::from_bytes([5; 16]),
            OperationMark::Dispatched,
            16,
        )
        .unwrap();
    journal
        .observe(
            FIRST,
            &Observation::Unknown {
                operation: OperationId::from_bytes([5; 16]),
            },
            17,
        )
        .unwrap();
    let stale = Envelope {
        key: OperationKey::parse("stale").unwrap(),
        job: Some(FIRST),
        expected: Some(WorkRevision::FIRST),
        body: Command::CancelJob {
            reason: "late".into(),
        },
    };
    journal.submit(&stale, ids(FIRST, 6)).unwrap();
    submit(
        &mut journal,
        "create-2",
        None,
        create("second"),
        ids(SECOND, 7),
    );
    submit(
        &mut journal,
        "finish-2",
        Some(SECOND),
        Command::FinishTask {
            task: TaskId::from_bytes([7; 16]),
            outcome: TaskOutcome::Succeeded,
        },
        ids(SECOND, 8),
    );
    journal.verify().unwrap();
}

#[test]
fn every_version_fixture_opens_migrates_and_verifies() {
    for version in 1..=SCHEMA_VERSION {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::copy(fixture(version), &path).unwrap();
        let journal = Journal::open(&path).unwrap();
        assert_eq!(journal.schema_version().unwrap(), SCHEMA_VERSION);
        let report = journal.verify().unwrap();
        assert_eq!(report.jobs, 2, "v{version}");
        assert_eq!(report.operations, 8, "v{version}");
    }
}

#[test]
fn the_v1_fixture_holds_what_it_was_made_with() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(fixture(1), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    assert_eq!(journal.authority().unwrap(), NODE);
    assert_eq!(journal.jobs().unwrap(), vec![FIRST, SECOND]);

    let first = journal.job(FIRST).unwrap().unwrap();
    assert_eq!(first.job.state, JobState::Active);
    assert_eq!(first.job.generation.get(), 3);
    assert_eq!(first.tasks.len(), 2);
    let statuses: Vec<_> = first.attempts.iter().map(|a| a.status.clone()).collect();
    assert_eq!(
        statuses,
        vec![
            AttemptStatus::Lost,
            AttemptStatus::Checkpointed {
                snapshot: SnapshotId::of(b"fixture manifest")
            },
            AttemptStatus::Granted,
        ]
    );
    let grant3 = first.operation(OperationId::from_bytes([5; 16])).unwrap();
    assert_eq!(grant3.status, OperationStatus::Unknown);
    let stale = journal
        .operation(&OperationKey::parse("stale").unwrap())
        .unwrap()
        .unwrap();
    assert!(matches!(stale.status, OperationStatus::Rejected { .. }));

    let second = journal.job(SECOND).unwrap().unwrap();
    assert_eq!(second.job.state, JobState::Finished);
    assert_eq!(second.tasks[0].state, TaskState::Succeeded);

    // An old key still answers with the bytes stored when it was made.
    let envelope = Envelope {
        key: OperationKey::parse("create-2").unwrap(),
        job: None,
        expected: None,
        body: create("second"),
    };
    let answer = journal.submit(&envelope, ids(SECOND, 99)).unwrap();
    assert!(answer.replayed);
    assert!(matches!(answer.result, StoredResult::Accepted(a) if a.job == SECOND));
    // And the fixture keeps taking commands.
    submit(
        &mut journal,
        "cancel-1",
        Some(FIRST),
        Command::CancelJob {
            reason: "after the fixture".into(),
        },
        ids(FIRST, 100),
    );
    let first = journal.job(FIRST).unwrap().unwrap();
    // The third attempt may still run (its grant is unknown), so the cancel waits.
    assert_eq!(first.job.state, JobState::Active);
    journal.verify().unwrap();
}
