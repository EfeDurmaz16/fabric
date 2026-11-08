//! Every released schema version keeps a fixture journal, and the current
//! binary opens each one, migrates it when it is older and verifies it.
//! `tests/fixtures/v1.sqlite` was made by `make_v1_fixture` with schema
//! version 1, `tests/fixtures/v2.sqlite` by `make_v2_fixture` from it, and
//! `tests/fixtures/v3.sqlite` by `make_v3_fixture` from that, with the M5
//! commands and observations on top, and `tests/fixtures/v4.sqlite` by
//! `make_v4_fixture` from that, with an M6 job on top (ADR 0028); the
//! tests below pin what each holds.
//! The v1 and v2 makers ran under codec format 1 and cannot be run again
//! byte for byte by this binary, which writes format 2; they stay as the
//! record of what the fixtures hold.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]

use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::placement::{Candidate, Exclusion, Verdict};
use fabric_core::managed::{
    AllowedFidelity, AttemptId, AttemptStatus, CheckpointAck, CheckpointCut, Command, CommitId,
    CoveredPath, CutDigest, DomainError, EffectRequest, Envelope, ExitOutcome, FenceVerdict,
    Fidelity, GitUri, GrantToken, Intent, JobPolicy, JobState, NewIds, Note, Observation,
    OperationId, OperationKey, OperationMark, OperationStatus, PackageDigest, PlacementDecision,
    PlacementKind, ReceiptStatus, RecoveryBlock, RecoveryPolicy, ResourceRef, RestartPolicy,
    SecretName, SecretRef, SecretScope, SourceBinding, TaskClass, TaskId, TaskKind, TaskOutcome,
    TaskReceipt, TaskState, WorkRevision,
};
use fabric_core::managed::{Generation, RequestDigest};
use fabric_core::path::AbsPath;
use fabric_journal::CommitError;
use fabric_journal::{
    AbandonApproval, Acknowledged, Blocker, EffectClass, FILE_NAME, Journal, LocatorRecord,
    OpenError, ProviderAnswer, ProviderDispatch, ProviderStatus, Purpose, ResourceKey,
    SCHEMA_VERSION,
};

const NODE: NodeId = NodeId::from_bytes([4; 32]);
const OTHER_NODE: NodeId = NodeId::from_bytes([6; 32]);
const RUNNER: NodeId = OTHER_NODE;
const FIRST: JobId = JobId::from_bytes([0x01; 16]);
const SECOND: JobId = JobId::from_bytes([0x02; 16]);
const THIRD: JobId = JobId::from_bytes([0x03; 16]);
const FOURTH: JobId = JobId::from_bytes([0x04; 16]);
const FIFTH: JobId = JobId::from_bytes([0x05; 16]);

/// The jobs and operations each fixture holds, as `verify` counts them.
fn pinned(version: u32) -> (u64, u64) {
    match version {
        1 | 2 => (2, 8),
        3 => (4, 14),
        _ => (5, 18),
    }
}

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
            acceptance: Vec::new(),
        },
        source: SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
    }
}

fn cut(snapshot: SnapshotId, receipts: Vec<TaskReceipt>) -> CheckpointCut {
    CheckpointCut {
        snapshot,
        receipts,
        facts: vec![("os".into(), "macos".into())],
        evidence: Vec::new(),
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
            class: TaskClass::Harness,
            program: None,
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
        .mark_authority(
            FIRST,
            OperationId::from_bytes([3; 16]),
            OperationMark::Dispatched,
            10,
        )
        .unwrap();
    journal
        .observe_authority(
            FIRST,
            &Observation::Started {
                attempt: first,
                generation: g1,
            },
            11,
        )
        .unwrap();
    journal
        .observe_authority(
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
        .mark_authority(
            FIRST,
            OperationId::from_bytes([4; 16]),
            OperationMark::Dispatched,
            13,
        )
        .unwrap();
    journal
        .observe_authority(
            FIRST,
            &Observation::Checkpointed {
                attempt: second,
                generation: g2,
                snapshot: SnapshotId::of(b"fixture manifest"),
                cut: cut(SnapshotId::of(b"fixture manifest"), Vec::new()),
            },
            14,
        )
        .unwrap();
    journal
        .observe_authority(
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
        .mark_authority(
            FIRST,
            OperationId::from_bytes([5; 16]),
            OperationMark::Dispatched,
            16,
        )
        .unwrap();
    journal
        .observe_authority(
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

/// The provider operations the v2 fixture adds to the v1 one, on the
/// first job's live third attempt: one of each status a worker's answer
/// leaves, two with the locator the worker reported, the exec and the
/// destroy with the target Fabric sent them to, a lost create an attach
/// did not find, and an exec the owner abandoned.
fn provider_dispatch(n: u8, action: &str) -> ProviderDispatch {
    ProviderDispatch {
        operation: OperationId::from_bytes([0x40 + n; 16]),
        key: OperationKey::parse(&format!("provider-{n}")).unwrap(),
        digest: RequestDigest::from_bytes([n; 32]),
        job: FIRST,
        task: TaskId::from_bytes([1; 16]),
        attempt: AttemptId::from_bytes([5; 16]),
        generation: Generation::new(3).unwrap(),
        profile: "fixture".into(),
        action: action.into(),
        effect: match action {
            "sandbox.create" => EffectClass::Create,
            "sandbox.destroy" => EffectClass::Destroy,
            _ => EffectClass::Mutate,
        },
        purpose: Purpose::Effect,
        target: matches!(action, "sandbox.exec" | "sandbox.destroy").then(locator),
    }
}

/// The fixture's one sandbox, as the bridge stores its locator; the key
/// is fabric-bridge's `resource_key` of it.
fn locator() -> LocatorRecord {
    LocatorRecord {
        text: r#"{"kind":"sandbox","native_id":"fx-provider-1","provider":"fixture","scope":{"account":"acct-a","project":"proj-1"}}"#.into(),
        key: ResourceKey::from_bytes(LOCATOR_KEY),
    }
}

const LOCATOR_KEY: [u8; 32] = [
    0x45, 0xf7, 0xbf, 0xd0, 0xc8, 0xe0, 0x20, 0xec, 0xfc, 0xfb, 0xc8, 0x87, 0xec, 0xb4, 0x2e, 0x3b,
    0x95, 0x78, 0x46, 0x5a, 0xbd, 0xc2, 0xd8, 0x8e, 0xf2, 0x28, 0x56, 0xca, 0xc8, 0x2d, 0x98, 0x4e,
];

const SEARCHED: &str =
    r#"{"kind":"sandbox","provider":"fixture","scope":{"account":"acct-a","project":"proj-1"}}"#;

fn answer(code: &str, reason: &str, effect: &str) -> ProviderAnswer {
    ProviderAnswer {
        code: Some(code.into()),
        reason: Some(reason.into()),
        effect: Some(effect.into()),
    }
}

/// Writes the v1 fixture migrated to version 2 with provider operations.
#[test]
#[ignore = "makes the v2 fixture; run by hand with FABRIC_JOURNAL_FIXTURE_OUT set"]
fn make_v2_fixture() {
    let Some(out) = std::env::var_os("FABRIC_JOURNAL_FIXTURE_OUT") else {
        return;
    };
    let path = PathBuf::from(out).join("v2.sqlite");
    std::fs::copy(fixture(1), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    let at = 1_760_000_001_000;
    let op = |n: u8| OperationId::from_bytes([0x40 + n; 16]);
    journal
        .provider_begin(&provider_dispatch(1, "sandbox.create"), None, at)
        .unwrap();
    journal
        .provider_record(
            op(1),
            ProviderStatus::Succeeded,
            &ProviderAnswer::default(),
            Some(&locator()),
            at + 1,
        )
        .unwrap();
    journal
        .provider_begin(
            &provider_dispatch(2, "sandbox.snapshot"),
            Some(&answer("unsupported", "action", "none")),
            at + 2,
        )
        .unwrap();
    journal
        .provider_begin(&provider_dispatch(3, "sandbox.exec"), None, at + 3)
        .unwrap();
    journal
        .provider_record(
            op(3),
            ProviderStatus::Unknown,
            &ProviderAnswer::default(),
            None,
            at + 4,
        )
        .unwrap();
    journal
        .provider_begin(&provider_dispatch(4, "sandbox.destroy"), None, at + 5)
        .unwrap();
    journal
        .provider_record(
            op(4),
            ProviderStatus::Dispatched,
            &answer("pending", "observation_deadline", "pending"),
            Some(&locator()),
            at + 6,
        )
        .unwrap();
    journal
        .provider_begin(&provider_dispatch(5, "sandbox.create"), None, at + 7)
        .unwrap();
    journal
        .provider_record(
            op(5),
            ProviderStatus::Unknown,
            &ProviderAnswer::default(),
            None,
            at + 8,
        )
        .unwrap();
    journal
        .provider_searched(
            op(5),
            &answer("unknown", "absent", "unknown"),
            SEARCHED,
            at + 9,
        )
        .unwrap();
    journal
        .provider_begin(&provider_dispatch(6, "sandbox.exec"), None, at + 10)
        .unwrap();
    journal
        .provider_record(
            op(6),
            ProviderStatus::Unknown,
            &ProviderAnswer::default(),
            None,
            at + 11,
        )
        .unwrap();
    journal
        .provider_abandon(
            &AbandonApproval {
                operation: op(6),
                expected: ProviderStatus::Unknown,
                acknowledged: Acknowledged::Resource(locator().key),
                reason: "the fixture's owner gave up on it".into(),
            },
            at + 12,
        )
        .unwrap();
    journal.verify().unwrap();
}

/// The v4 fixture: the v2 fixture opened (so migrated to version 4), the
/// M5 history `make_v3_fixture` wrote, and the M6 job on top. The v3
/// fixture itself is the unreleased M5 build's file, kept so its refusal
/// stays tested: codec format 2 was amended in place by ADR 0028.
#[test]
#[ignore = "makes the v4 fixture; run by hand with FABRIC_JOURNAL_FIXTURE_OUT set"]
fn make_v4_fixture() {
    let Some(out) = std::env::var_os("FABRIC_JOURNAL_FIXTURE_OUT") else {
        return;
    };
    let path = PathBuf::from(out).join("v4.sqlite");
    std::fs::copy(fixture(2), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    write_v3_history(&mut journal);
    m6_job(&mut journal, 1_760_000_100_000);
    journal.verify().unwrap();
}

/// The M6 job of the v4 fixture (ADR 0028): a fifth job on a runner
/// node with a captured and an acknowledged checkpoint, a covered write
/// the fence refused, an attempt that went unavailable and was recovered
/// from the acknowledged checkpoint, a blocked automatic recovery, and a
/// second attempt superseded with nothing granted. Every M6 event kind
/// is in it, so `verify` replays each.
#[allow(
    clippy::too_many_lines,
    reason = "the fixture's steps read best as one script"
)]
fn m6_job(journal: &mut Journal, at: u64) {
    let create = match create("fifth") {
        Command::CreateJob { intent, source, .. } => Command::CreateJob {
            intent,
            source,
            policy: JobPolicy {
                recovery: RecoveryPolicy::RestartFromAcknowledged,
                ..JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
            },
        },
        other => other,
    };
    submit(journal, "create-5", None, create, ids(FIFTH, 80));
    let primary = TaskId::from_bytes([80; 16]);
    let on_runner = Command::StartAttempt {
        task: primary,
        resource: ResourceRef::Node { node: RUNNER },
        initial_checkpoint: None,
    };
    submit(journal, "start-5-1", Some(FIFTH), on_runner, ids(FIFTH, 81));
    let g1 = Generation::FIRST;
    let token1 = GrantToken {
        job: FIFTH,
        attempt: AttemptId::from_bytes([81; 16]),
        generation: g1,
    };
    journal
        .mark_authority(
            FIFTH,
            OperationId::from_bytes([81; 16]),
            OperationMark::Dispatched,
            at,
        )
        .unwrap();
    journal
        .observe(
            &token1,
            Some(RUNNER),
            &Observation::Started {
                attempt: token1.attempt,
                generation: g1,
            },
            at + 1,
        )
        .unwrap()
        .unwrap();
    let (snapshot, cut) = (m6_snapshot(1), m6_cut(1));
    journal
        .observe(
            &token1,
            Some(RUNNER),
            &Observation::Captured {
                attempt: token1.attempt,
                generation: g1,
                snapshot,
                cut,
            },
            at + 2,
        )
        .unwrap()
        .unwrap();
    journal
        .acknowledge(FIFTH, &m6_ack(&token1, snapshot, cut), at + 3)
        .unwrap();
    // A second capture that is never acknowledged.
    journal
        .observe(
            &token1,
            Some(RUNNER),
            &Observation::Captured {
                attempt: token1.attempt,
                generation: g1,
                snapshot: m6_snapshot(2),
                cut: m6_cut(2),
            },
            at + 4,
        )
        .unwrap()
        .unwrap();
    // A ledger append from the wrong node: refused and recorded.
    let refused = journal
        .fence(&token1, CoveredPath::LedgerAppend, Some(NODE), at + 5)
        .unwrap();
    assert!(matches!(refused, Err(FenceVerdict::WrongNode { .. })));
    // The runner goes silent; an automatic recovery is blocked once, then
    // the authority recovers from the acknowledged checkpoint onto itself.
    journal
        .observe_authority(
            FIFTH,
            &Observation::ResourceUnavailable {
                attempt: token1.attempt,
                generation: g1,
                last_heartbeat_ms: Some(at + 4),
                at_ms: at + 15_000,
            },
            at + 15_000,
        )
        .unwrap();
    journal
        .recovery_blocked(
            FIFTH,
            token1.attempt,
            RecoveryBlock::NoAcknowledgedCheckpoint,
            at + 15_001,
        )
        .unwrap();
    submit(
        journal,
        "recover-5-1",
        Some(FIFTH),
        Command::Recover {
            attempt: token1.attempt,
            from: snapshot,
            reason: "the fixture's runner went silent".into(),
        },
        ids(FIFTH, 82),
    );
    // The old runner's late exit is refused by the fence and recorded.
    let late = journal
        .observe(
            &token1,
            Some(RUNNER),
            &Observation::Exited {
                attempt: token1.attempt,
                generation: g1,
                outcome: ExitOutcome::Succeeded,
            },
            at + 15_002,
        )
        .unwrap();
    assert!(matches!(late, Err(FenceVerdict::Superseded { by: None })));
    // The recover's failover placement grants on the authority's node,
    // never the runner's; the recovered attempt runs, goes silent and is
    // superseded by hand.
    journal
        .observe_authority(
            FIFTH,
            &Observation::Placed {
                operation: OperationId::from_bytes([82; 16]),
                task: primary,
                decision: PlacementDecision {
                    kind: PlacementKind::Failover,
                    decided_at_ms: at + 15_003,
                    candidates: vec![Candidate {
                        node: RUNNER,
                        verdict: Verdict::Excluded {
                            reasons: vec![Exclusion::NoAnswer { within_ms: 2_000 }],
                        },
                    }],
                    chosen: Some(NODE),
                },
            },
            at + 15_003,
        )
        .unwrap();
    let g2 = g1.next().unwrap();
    let recovered = journal.job(FIFTH).unwrap().unwrap();
    let token2 = GrantToken {
        job: FIFTH,
        attempt: recovered.attempts.last().unwrap().id,
        generation: g2,
    };
    journal
        .observe(
            &token2,
            None,
            &Observation::Started {
                attempt: token2.attempt,
                generation: g2,
            },
            at + 15_004,
        )
        .unwrap()
        .unwrap();
    journal
        .observe_authority(
            FIFTH,
            &Observation::ResourceUnavailable {
                attempt: token2.attempt,
                generation: g2,
                last_heartbeat_ms: None,
                at_ms: at + 30_000,
            },
            at + 30_000,
        )
        .unwrap();
    journal
        .observe_authority(
            FIFTH,
            &Observation::ResourceReachable {
                attempt: token2.attempt,
                generation: g2,
            },
            at + 30_001,
        )
        .unwrap();
    journal
        .observe_authority(
            FIFTH,
            &Observation::ResourceUnavailable {
                attempt: token2.attempt,
                generation: g2,
                last_heartbeat_ms: Some(at + 30_001),
                at_ms: at + 45_000,
            },
            at + 45_000,
        )
        .unwrap();
    submit(
        journal,
        "supersede-5-2",
        Some(FIFTH),
        Command::Supersede {
            attempt: token2.attempt,
            reason: "the fixture's operator took the grant away".into(),
        },
        ids(FIFTH, 83),
    );
}

fn m6_snapshot(n: u8) -> SnapshotId {
    SnapshotId::of(&[0x60 + n; 4])
}

fn m6_cut(n: u8) -> CutDigest {
    CutDigest::from_bytes([0x70 + n; 32])
}

fn m6_ack(token: &GrantToken, snapshot: SnapshotId, cut: CutDigest) -> CheckpointAck {
    CheckpointAck {
        attempt: token.attempt,
        generation: token.generation,
        snapshot,
        cut,
        manifests: 3,
        chunks: 11,
        bytes: 65_536,
        fetched_from: RUNNER,
    }
}

#[test]
fn the_v4_fixture_holds_the_m6_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(fixture(4), &path).unwrap();
    let journal = Journal::open(&path).unwrap();
    let fifth = journal.job(FIFTH).unwrap().unwrap();
    assert_eq!(
        fifth.job.policy.recovery,
        RecoveryPolicy::RestartFromAcknowledged
    );
    assert_eq!(fifth.job.generation.get(), 2);
    let statuses: Vec<_> = fifth.attempts.iter().map(|a| a.status.clone()).collect();
    assert_eq!(
        statuses,
        vec![
            AttemptStatus::Superseded {
                by: Some(Generation::new(2).unwrap())
            },
            AttemptStatus::Superseded { by: None },
        ]
    );
    assert_eq!(fifth.attempts[1].initial_checkpoint, Some(m6_snapshot(1)));
    assert_eq!(
        fifth.attempts[1].unavailable_since_ms, None,
        "superseded clears it"
    );
    let acknowledged = journal.acknowledged(FIFTH).unwrap();
    assert_eq!(acknowledged.len(), 1);
    assert_eq!(acknowledged[0].snapshot, m6_snapshot(1));
    assert_eq!(acknowledged[0].fetched_from, RUNNER);
    assert_eq!(
        fifth.captured_only(),
        vec![(fifth.attempts[0].id, Generation::FIRST, m6_snapshot(2))]
    );
    let kinds: Vec<_> = journal
        .events(FIFTH, None, 100)
        .unwrap()
        .iter()
        .map(|e| e.frame.event.name())
        .collect();
    for kind in [
        "checkpoint_captured",
        "checkpoint_acknowledged",
        "covered_write_refused",
        "attempt_unavailable",
        "attempt_reachable",
        "grant_superseded",
        "recovery_blocked",
    ] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    assert!(fifth.live_attempt().is_none());
    journal.verify().unwrap();
}

#[test]
fn the_v2_fixture_holds_what_it_was_made_with() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(fixture(2), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    let op = |n: u8| OperationId::from_bytes([0x40 + n; 16]);
    let rows: Vec<_> = (1..=6)
        .map(|n| journal.provider_operation(op(n)).unwrap().unwrap())
        .collect();
    assert_eq!(rows[0].dispatch, provider_dispatch(1, "sandbox.create"));
    assert_eq!(rows[0].status, ProviderStatus::Succeeded);
    assert_eq!(rows[0].locator, Some(locator()));
    assert_eq!(rows[1].status, ProviderStatus::Rejected);
    assert_eq!(rows[1].answer, answer("unsupported", "action", "none"));
    assert_eq!(rows[2].dispatch, provider_dispatch(3, "sandbox.exec"));
    assert_eq!(rows[2].status, ProviderStatus::Unknown);
    assert_eq!(rows[2].locator, None, "the exec reported no locator");
    assert_eq!(rows[2].dispatch.target, Some(locator()));
    assert_eq!(rows[3].dispatch, provider_dispatch(4, "sandbox.destroy"));
    assert_eq!(rows[3].status, ProviderStatus::Dispatched);
    assert_eq!(rows[3].locator, Some(locator()));
    assert_eq!(rows[4].status, ProviderStatus::Unknown);
    assert_eq!(rows[4].searched.as_deref(), Some(SEARCHED));
    assert_eq!(rows[5].status, ProviderStatus::Abandoned);
    assert_eq!(
        rows[5].abandon_reason.as_deref(),
        Some("the fixture's owner gave up on it")
    );
    // Restart recovery makes the dispatched one unknown and sends nothing.
    let recovery = journal
        .recover(1_760_000_002_000, |_| fabric_journal::Liveness::CannotTell)
        .unwrap();
    assert_eq!(recovery.provider_unknown, vec![(FIRST, op(4))]);
    let pending = journal.provider_operation(op(4)).unwrap().unwrap();
    assert_eq!(pending.status, ProviderStatus::Unknown);
    assert_eq!(pending.locator, Some(locator()));
    // The lost create, the exec that may run and the unconfirmed stop
    // block a replacement; the sandbox and the lost create are residuals.
    let blockers: Vec<_> = journal
        .replacement_blockers(FIRST)
        .unwrap()
        .iter()
        .map(Blocker::operation)
        .collect();
    assert_eq!(blockers, vec![op(3), op(4), op(5), op(6)]);
    let residuals: Vec<_> = journal
        .residuals(FIRST)
        .unwrap()
        .into_iter()
        .map(|r| (r.creating, r.searched))
        .collect();
    assert_eq!(
        residuals,
        vec![(op(1), None), (op(5), Some(SEARCHED.to_owned()))]
    );
    journal.verify().unwrap();
}

#[test]
fn every_version_fixture_opens_migrates_and_verifies() {
    for version in (1..=SCHEMA_VERSION).filter(|v| *v != 3) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::copy(fixture(version), &path).unwrap();
        let journal = Journal::open(&path).unwrap();
        assert_eq!(journal.schema_version().unwrap(), SCHEMA_VERSION);
        let report = journal.verify().unwrap();
        let (jobs, operations) = pinned(version);
        assert_eq!(report.jobs, jobs, "v{version}");
        assert_eq!(report.operations, operations, "v{version}");
    }
}

/// The M5 history the v3 fixture adds to the v2 one: a note and a
/// classed child on the first job, a third job parked by its initial
/// placement with a secret no node resolved, and a fourth job placed,
/// checkpointed in place, then continued with a planned handoff.
#[test]
#[ignore = "makes the v3 fixture; run by hand with FABRIC_JOURNAL_FIXTURE_OUT set"]
fn make_v3_fixture() {
    let Some(out) = std::env::var_os("FABRIC_JOURNAL_FIXTURE_OUT") else {
        return;
    };
    let path = PathBuf::from(out).join("v3.sqlite");
    std::fs::copy(fixture(2), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    write_v3_history(&mut journal);
    journal.verify().unwrap();
}

fn m5_create(job: JobId) -> Command {
    match job {
        THIRD => Command::CreateJob {
            intent: Intent {
                goal: "third: a git source and a secret".into(),
                harness: HarnessKind::Codex,
                acceptance: vec!["cargo test passes".into()],
            },
            source: SourceBinding::Git {
                uri: GitUri::parse("https://example.invalid/o/r.git").unwrap(),
                commit: CommitId::parse(&"a".repeat(40)).unwrap(),
            },
            policy: JobPolicy {
                secrets: vec![SecretRef {
                    name: SecretName::parse("TOKEN").unwrap(),
                    scope: SecretScope::node_any(),
                }],
                ..JobPolicy::m2_defaults(RestartPolicy::Block)
            },
        },
        _ => Command::CreateJob {
            intent: Intent {
                goal: "fourth: placed, checkpointed, continued".into(),
                harness: HarnessKind::ClaudeCode,
                acceptance: Vec::new(),
            },
            source: SourceBinding::Empty,
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
        },
    }
}

fn parked_decision() -> PlacementDecision {
    PlacementDecision {
        kind: PlacementKind::Initial,
        decided_at_ms: 1_760_000_003_000,
        candidates: vec![Candidate {
            node: NODE,
            verdict: Verdict::Excluded {
                reasons: vec![Exclusion::Secret {
                    name: SecretName::parse("TOKEN").unwrap(),
                }],
            },
        }],
        chosen: None,
    }
}

fn chosen_decision(kind: PlacementKind, node: NodeId) -> PlacementDecision {
    PlacementDecision {
        kind,
        decided_at_ms: 1_760_000_003_001,
        candidates: vec![Candidate {
            node,
            verdict: Verdict::Eligible {
                rank: vec![fabric_core::managed::placement::RankReason::Identity],
            },
        }],
        chosen: Some(node),
    }
}

/// The handoff cut's receipt: the harness, checkpointed by its suspend path.
fn handoff_harness(task: TaskId, snapshot: SnapshotId) -> TaskReceipt {
    TaskReceipt {
        task,
        class: TaskClass::Harness,
        status: ReceiptStatus::Checkpointed { snapshot },
        exit: None,
        interrupted: true,
        workspace: None,
    }
}

fn interrupted_test(task: TaskId) -> TaskReceipt {
    TaskReceipt {
        task,
        class: TaskClass::Test,
        status: ReceiptStatus::Interrupted,
        exit: None,
        interrupted: true,
        workspace: None,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixture's steps read best as one script"
)]
fn write_v3_history(journal: &mut Journal) {
    let at = 1_760_000_003_000;
    submit(
        journal,
        "note-1",
        Some(FIRST),
        Command::RecordNote {
            note: Note::Decision {
                what: "keep the first job's third attempt".into(),
                reason: "its grant is unknown, not lost".into(),
            },
        },
        ids(FIRST, 9),
    );
    submit(
        journal,
        "child-test",
        Some(FIRST),
        Command::RegisterTask {
            parent: TaskId::from_bytes([1; 16]),
            kind: TaskKind::Child {
                label: "cargo test".into(),
            },
            restart: RestartPolicy::Block,
            class: TaskClass::Test,
            program: Some(fabric_core::managed::Program {
                argv: vec!["cargo".into(), "test".into()],
                cwd: None,
                env_allowlist: Vec::new(),
            }),
        },
        ids(FIRST, 10),
    );
    // The first job's grant may still run, so the program asks for a
    // spawn there: instructed, and the node took it.
    for mark in [OperationMark::Dispatched, OperationMark::Succeeded] {
        journal
            .mark_authority(FIRST, OperationId::from_bytes([10; 16]), mark, at)
            .unwrap();
    }
    submit(journal, "create-3", None, m5_create(THIRD), ids(THIRD, 11));
    journal
        .observe_authority(
            THIRD,
            &Observation::Placed {
                operation: OperationId::from_bytes([11; 16]),
                task: TaskId::from_bytes([11; 16]),
                decision: parked_decision(),
            },
            at,
        )
        .unwrap();
    submit(
        journal,
        "create-4",
        None,
        m5_create(FOURTH),
        ids(FOURTH, 12),
    );
    let primary = TaskId::from_bytes([12; 16]);
    journal
        .observe_authority(
            FOURTH,
            &Observation::Placed {
                operation: OperationId::from_bytes([12; 16]),
                task: primary,
                decision: chosen_decision(PlacementKind::Initial, NODE),
            },
            at + 1,
        )
        .unwrap();
    let first = journal.job(FOURTH).unwrap().unwrap().attempts[0].clone();
    journal
        .observe_authority(
            FOURTH,
            &Observation::Started {
                attempt: first.id,
                generation: first.generation,
            },
            at + 2,
        )
        .unwrap();
    submit(
        journal,
        "checkpoint-4",
        Some(FOURTH),
        Command::Checkpoint { attempt: first.id },
        ids(FOURTH, 13),
    );
    journal
        .mark_authority(
            FOURTH,
            OperationId::from_bytes([13; 16]),
            OperationMark::Dispatched,
            at + 3,
        )
        .unwrap();
    let in_place = SnapshotId::of(b"fixture cut in place");
    journal
        .observe_authority(
            FOURTH,
            &Observation::Checkpointed {
                attempt: first.id,
                generation: first.generation,
                snapshot: in_place,
                cut: cut(in_place, vec![interrupted_test(primary)]),
            },
            at + 4,
        )
        .unwrap();
    submit(
        journal,
        "continue-4",
        Some(FOURTH),
        Command::Continue {
            task: primary,
            fidelity: AllowedFidelity::DEFAULT,
            pin: None,
        },
        ids(FOURTH, 14),
    );
    let cont = OperationId::from_bytes([14; 16]);
    journal
        .mark_authority(FOURTH, cont, OperationMark::Dispatched, at + 5)
        .unwrap();
    let handoff = SnapshotId::of(b"fixture handoff cut");
    journal
        .observe_authority(
            FOURTH,
            &Observation::Checkpointed {
                attempt: first.id,
                generation: first.generation,
                snapshot: handoff,
                cut: cut(handoff, vec![handoff_harness(primary, handoff)]),
            },
            at + 6,
        )
        .unwrap();
    journal
        .observe_authority(
            FOURTH,
            &Observation::Placed {
                operation: cont,
                task: primary,
                decision: chosen_decision(PlacementKind::PlannedHandoff, OTHER_NODE),
            },
            at + 7,
        )
        .unwrap();
    let second = journal.job(FOURTH).unwrap().unwrap().attempts[1].clone();
    journal
        .observe_authority(
            FOURTH,
            &Observation::Started {
                attempt: second.id,
                generation: second.generation,
            },
            at + 8,
        )
        .unwrap();
    journal
        .observe_authority(
            FOURTH,
            &Observation::Continued {
                attempt: second.id,
                generation: second.generation,
                from: first.id,
                achieved: Fidelity::NewNativeSessionWithPackage,
                package: Some(PackageDigest([0x33; 32])),
            },
            at + 9,
        )
        .unwrap();
}

#[test]
fn a_v2_journal_migrates_and_takes_the_m5_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(fixture(2), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    assert_eq!(journal.schema_version().unwrap(), SCHEMA_VERSION);
    // Every M2 task is a harness task and every M2 policy has the M5 defaults.
    let first = journal.job(FIRST).unwrap().unwrap();
    assert!(first.tasks.iter().all(|t| t.class == TaskClass::Harness));
    assert_eq!(
        first.job.policy,
        JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
    );
    assert!(first.job.intent.acceptance.is_empty());
    // The create of M2 was complete at its commit; it is not re-placed.
    let recovery = journal
        .recover(1_760_000_002_500, |_| fabric_journal::Liveness::CannotTell)
        .unwrap();
    assert!(recovery.to_effect.is_empty(), "{recovery:?}");
    write_v3_history(&mut journal);
    journal.verify().unwrap();
    check_v3(&mut journal);
}

/// `tests/fixtures/v3-draft.sqlite` is the v3 fixture as the unreleased
/// build before the launch path made it: no `tasks.program`. Version 3 was
/// amended in place, so such a file is refused, never read without it.
#[test]
fn a_draft_v3_without_the_program_column_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v3-draft.sqlite"),
        &path,
    )
    .unwrap();
    assert!(matches!(
        Journal::open(&path),
        Err(fabric_journal::OpenError::DraftSchema { version: 3 })
    ));
}

#[test]
fn the_v4_fixture_holds_the_m5_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(fixture(4), &path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    check_v3(&mut journal);
}

#[test]
fn the_m5_draft_of_version_3_is_refused_unwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::copy(fixture(3), &path).unwrap();
    let before = std::fs::read(&path).unwrap();
    let refused = Journal::open(&path).unwrap_err();
    assert!(
        matches!(refused, OpenError::DraftSchema { version: 3 }),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the draft was written to"
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the M5 history's checks read best as one list"
)]
fn check_v3(journal: &mut Journal) {
    assert!(
        journal
            .jobs()
            .unwrap()
            .starts_with(&[FIRST, SECOND, THIRD, FOURTH])
    );
    let notes = journal.notes(FIRST).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(matches!(notes[0].1, Note::Decision { .. }));
    let first = journal.job(FIRST).unwrap().unwrap();
    assert_eq!(first.tasks.len(), 3);
    assert_eq!(first.tasks[2].class, TaskClass::Test);
    assert!(
        first
            .requests
            .iter()
            .any(|(_, r)| matches!(r, EffectRequest::Spawn { task } if *task == first.tasks[2].id)),
        "{:?}",
        first.requests
    );
    assert_eq!(
        first.tasks[2].program,
        Some(fabric_core::managed::Program {
            argv: vec!["cargo".into(), "test".into()],
            cwd: None,
            env_allowlist: Vec::new(),
        })
    );

    let third = journal.job(THIRD).unwrap().unwrap();
    assert!(third.parked());
    assert!(matches!(third.job.source, SourceBinding::Git { .. }));
    assert_eq!(third.job.policy.secrets.len(), 1);
    let placements = journal.placements(THIRD).unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].1, parked_decision());
    assert_eq!(
        third.operations[0].status,
        OperationStatus::Succeeded,
        "a parked create is complete"
    );

    let fourth = journal.job(FOURTH).unwrap().unwrap();
    assert_eq!(fourth.attempts.len(), 2);
    assert!(matches!(
        fourth.attempts[0].status,
        AttemptStatus::Checkpointed { .. }
    ));
    assert_eq!(fourth.attempts[1].status, AttemptStatus::Running);
    assert_eq!(
        fourth.attempts[1].initial_checkpoint,
        Some(SnapshotId::of(b"fixture handoff cut"))
    );
    assert_eq!(
        fourth.attempts[1].resource,
        ResourceRef::Node { node: OTHER_NODE }
    );
    let kinds: Vec<_> = journal
        .placements(FOURTH)
        .unwrap()
        .into_iter()
        .map(|(_, d)| d.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![PlacementKind::Initial, PlacementKind::PlannedHandoff]
    );
    let continuations = journal.continuations(FOURTH).unwrap();
    assert_eq!(
        continuations,
        vec![(
            fourth.attempts[1].id,
            fourth.attempts[0].id,
            Fidelity::NewNativeSessionWithPackage,
            Some(PackageDigest([0x33; 32]))
        )]
    );
    // The in-place checkpoint's receipts said the test was interrupted;
    // the handoff cut's say the harness was checkpointed, and the last cut wins.
    assert_eq!(
        journal.receipts(FOURTH, fourth.attempts[0].id).unwrap(),
        vec![handoff_harness(
            fourth.job.primary,
            SnapshotId::of(b"fixture handoff cut")
        )]
    );
    assert!(
        journal
            .job(FOURTH)
            .unwrap()
            .unwrap()
            .operations
            .iter()
            .all(|o| o.status == OperationStatus::Succeeded),
        "{:?}",
        fourth.operations
    );
    assert_eq!(fourth.requests.len(), 2);
    check_v3_reads(journal, &fourth);
    let recovery = journal
        .recover(1_760_000_004_000, |_| fabric_journal::Liveness::CannotTell)
        .unwrap();
    assert!(recovery.to_effect.is_empty());
    assert!(recovery.unknown.is_empty());
    journal.verify().unwrap();
}

/// The reads a work package is built from: the newest cut of an attempt
/// and the job's head.
fn check_v3_reads(journal: &Journal, fourth: &fabric_core::managed::ManagedState) {
    // The newest cut of the first attempt is the handoff's, not the in-place one.
    let (cut_snapshot, _) = journal
        .last_cut(FOURTH, fourth.attempts[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(cut_snapshot, SnapshotId::of(b"fixture handoff cut"));
    assert_eq!(
        journal.last_cut(FOURTH, fourth.attempts[1].id).unwrap(),
        None
    );
    let newest = journal.events(FOURTH, None, 1_000).unwrap();
    assert!(journal.head(FOURTH).unwrap().is_some());
    assert_eq!(
        journal.head(FOURTH).unwrap(),
        newest
            .last()
            .map(|e| fabric_core::managed::EventFrame::digest(&e.frame.encode().unwrap()))
    );
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

    // An old key was stored under a format 1 digest; the same command
    // digests differently under format 2, so the repeat is a conflict and
    // writes nothing (docs/managed-jobs.md, Codec and schema version 3).
    let envelope = Envelope {
        key: OperationKey::parse("create-2").unwrap(),
        job: None,
        expected: None,
        body: create("second"),
    };
    assert!(matches!(
        journal.submit(&envelope, ids(SECOND, 99)),
        Err(CommitError::Domain(DomainError::OperationConflict { .. }))
    ));
    assert_eq!(journal.jobs().unwrap(), vec![FIRST, SECOND]);
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
    // The third attempt may still run (its grant is unknown), so the
    // cancel is accepted and waits for the attempt to be stopped (M5).
    assert_eq!(first.job.state, JobState::Active);
    assert_eq!(
        first
            .operation(OperationId::from_bytes([100; 16]))
            .unwrap()
            .status,
        OperationStatus::Accepted
    );
    journal.verify().unwrap();
}
