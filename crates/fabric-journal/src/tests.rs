//! The journal's contract: idempotent commands, restart recovery,
//! tamper evidence and the file's version checks.

use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{
    AttemptStatus, CheckpointAck, Command, CoveredPath, CutDigest, DomainError, Envelope,
    FenceVerdict, Generation, GrantToken, Intent, JobPolicy, ManagedEvent, NewIds, Observation,
    OperationKey, OperationMark, OperationStatus, ResourceRef, RestartPolicy, SourceBinding,
    StoredResult, TaskState, WorkRevision,
};
use fabric_core::path::AbsPath;
use rusqlite::Connection;

use super::*;

pub(crate) const NODE: NodeId = NodeId::from_bytes([4; 32]);

/// A fresh journal in a temp directory.
pub(crate) struct Fixture {
    pub dir: tempfile::TempDir,
    pub journal: Journal,
    pub next: u8,
}

impl Fixture {
    pub(crate) fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::create(&dir.path().join(FILE_NAME), NODE, 1).unwrap();
        Fixture {
            dir,
            journal,
            next: 0,
        }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.path().join(FILE_NAME)
    }

    pub(crate) fn reopen(&mut self) {
        self.journal = Journal::open(&self.path()).unwrap();
    }

    pub(crate) fn ids(&mut self) -> NewIds {
        self.next += 1;
        ids(self.next)
    }

    pub(crate) fn submit(&mut self, key: &str, job: Option<JobId>, body: Command) -> Answer {
        let ids = self.ids();
        self.journal.submit(&envelope(key, job, body), ids).unwrap()
    }
}

/// Deterministic ids for step `n`; the job id is the same every time.
pub(crate) fn ids(n: u8) -> NewIds {
    NewIds {
        job: JobId::from_bytes([0x01; 16]),
        task: fabric_core::managed::TaskId::from_bytes([n; 16]),
        attempt: fabric_core::managed::AttemptId::from_bytes([n; 16]),
        operation: fabric_core::managed::OperationId::from_bytes([n; 16]),
        at_ms: 1_000 + u64::from(n),
    }
}

pub(crate) fn envelope(key: &str, job: Option<JobId>, body: Command) -> Envelope<Command> {
    Envelope {
        key: OperationKey::parse(key).unwrap(),
        job,
        expected: None,
        body,
    }
}

pub(crate) fn create() -> Command {
    Command::CreateJob {
        intent: Intent {
            goal: "make the greeting louder".into(),
            harness: HarnessKind::Codex,
            acceptance: Vec::new(),
        },
        source: SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
    }
}

pub(crate) fn start(task: fabric_core::managed::TaskId) -> Command {
    Command::StartAttempt {
        task,
        resource: ResourceRef::Node { node: NODE },
        initial_checkpoint: None,
    }
}

pub(crate) fn accepted(answer: &Answer) -> &fabric_core::managed::AcceptedOperation {
    match &answer.result {
        StoredResult::Accepted(a) => a,
        StoredResult::Rejected(e) => panic!("rejected: {e}"),
    }
}

pub(crate) fn count(path: &Path, table: &str) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_new_journal_has_its_version_its_authority_and_nothing_else() {
    let f = Fixture::new();
    assert_eq!(f.journal.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(f.journal.authority().unwrap(), NODE);
    assert_eq!(
        f.journal.verify().unwrap(),
        VerifyReport {
            jobs: 0,
            events: 0,
            operations: 0
        }
    );
    assert!(matches!(
        Journal::create(&f.path(), NODE, 1),
        Err(OpenError::Exists(_))
    ));
    let mut tmp = f.path().into_os_string();
    tmp.push(".tmp");
    assert!(!Path::new(&tmp).exists());
}

#[test]
fn a_duplicate_command_gets_the_same_identity_and_bytes_before_and_after_a_restart() {
    let mut f = Fixture::new();
    let first = f.submit("create-1", None, create());
    assert!(!first.replayed);
    let job = accepted(&first).job;
    let events = count(&f.path(), "events");

    let again = f.submit("create-1", None, create());
    assert!(again.replayed);
    assert_eq!(again.bytes, first.bytes);
    assert_eq!(accepted(&again).job, job);

    f.reopen();
    let after_restart = f.submit("create-1", None, create());
    assert!(after_restart.replayed);
    assert_eq!(after_restart.bytes, first.bytes);
    assert_eq!(f.journal.jobs().unwrap(), vec![job]);
    assert_eq!(count(&f.path(), "events"), events);
    assert_eq!(count(&f.path(), "operations"), 1);

    // The same key with another command is a conflict and writes nothing.
    let other = Command::CancelJob {
        reason: "no".into(),
    };
    let ids = f.ids();
    let conflict = f
        .journal
        .submit(&envelope("create-1", Some(job), other), ids);
    assert!(matches!(
        conflict,
        Err(CommitError::Domain(DomainError::OperationConflict { .. }))
    ));
    assert_eq!(count(&f.path(), "operations"), 1);
    f.journal.verify().unwrap();
}

#[test]
fn a_refusal_is_stored_under_its_key_and_changes_no_job() {
    let mut f = Fixture::new();
    let made = f.submit("create", None, create());
    let job = accepted(&made).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    let mut wrong = envelope("start", Some(job), start(primary));
    wrong.expected = Some(WorkRevision::new(9).unwrap());
    let ids = f.ids();
    let refused = f.journal.submit(&wrong, ids).unwrap();
    assert!(matches!(
        refused.result,
        StoredResult::Rejected(DomainError::RevisionMismatch { .. })
    ));
    let ids = f.ids();
    let retry = f.journal.submit(&wrong, ids).unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.bytes, refused.bytes);
    let op = f
        .journal
        .operation(&OperationKey::parse("start").unwrap())
        .unwrap()
        .unwrap();
    assert!(matches!(op.status, OperationStatus::Rejected { .. }));
    assert_eq!(op.expected_revision, WorkRevision::new(9));
    let state = f.journal.job(job).unwrap().unwrap();
    assert_eq!(state.job.revision, WorkRevision::FIRST);
    assert!(state.attempts.is_empty());
    // A command for a job the journal does not have is refused and stored too.
    let ids = f.ids();
    let unknown = f
        .journal
        .submit(
            &envelope(
                "elsewhere",
                Some(JobId::from_bytes([9; 16])),
                Command::CancelJob { reason: "x".into() },
            ),
            ids,
        )
        .unwrap();
    assert!(matches!(
        unknown.result,
        StoredResult::Rejected(DomainError::UnknownJob { .. })
    ));
    f.journal.verify().unwrap();
}

#[test]
fn a_restart_keeps_identity_and_progress_and_makes_a_dispatch_unknown() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    let grant = accepted(&f.submit("start-1", Some(job), start(primary))).operation;
    f.journal
        .mark_authority(job, grant, OperationMark::Dispatched, 2_000)
        .unwrap();
    let before = f.journal.job(job).unwrap().unwrap();

    f.reopen();
    f.journal.verify().unwrap();
    assert_eq!(f.journal.job(job).unwrap().unwrap(), before);
    let recovery = f.journal.recover(3_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(recovery.unknown, vec![(job, grant)]);
    assert!(
        recovery.to_dispatch.is_empty(),
        "a dispatched grant was offered again"
    );
    let after = f.journal.job(job).unwrap().unwrap();
    assert_eq!(after.job.id, before.job.id);
    assert_eq!(after.job.primary, before.job.primary);
    assert_eq!(after.attempts[0].id, before.attempts[0].id);
    assert_eq!(after.attempts[0].status, AttemptStatus::Granted);
    assert_eq!(
        after.operation(grant).unwrap().status,
        OperationStatus::Unknown
    );
    assert_eq!(after.job.revision.get(), before.job.revision.get() + 1);

    // The unknown grant may still run: no replacement.
    let refused = f.submit("start-2", Some(job), start(primary));
    assert!(matches!(
        refused.result,
        StoredResult::Rejected(DomainError::AttemptStillLive { .. })
    ));
    // A second recovery changes nothing more; the explicit grant stood in
    // for the create's initial placement, so no effect waits.
    let again = f.journal.recover(4_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(again, Recovery::default());
    // The executor's late report settles it.
    let attempt = after.attempts[0].clone();
    f.journal
        .observe_authority(
            job,
            &Observation::Started {
                attempt: attempt.id,
                generation: attempt.generation,
            },
            5_000,
        )
        .unwrap();
    let settled = f.journal.job(job).unwrap().unwrap();
    assert_eq!(
        settled.operation(grant).unwrap().status,
        OperationStatus::Succeeded
    );
    f.journal.verify().unwrap();
}

#[test]
fn recovery_offers_an_accepted_grant_once_and_acts_only_on_a_verified_probe() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    let grant = accepted(&f.submit("start-1", Some(job), start(primary))).operation;
    f.reopen();
    let recovery = f.journal.recover(2_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(recovery.to_dispatch, vec![(job, grant)]);
    f.journal
        .mark_authority(job, grant, OperationMark::Dispatched, 2_001)
        .unwrap();
    assert!(matches!(
        f.journal.mark_authority(job, grant, OperationMark::Dispatched, 2_002),
        Ok(t) if t.events.is_empty()
    ));

    // The group is verified gone: lost, the task stays open, no new grant.
    f.reopen();
    let recovery = f.journal.recover(3_000, |_| Liveness::Gone).unwrap();
    let state = f.journal.job(job).unwrap().unwrap();
    assert_eq!(recovery.lost, vec![(job, state.attempts[0].id)]);
    assert_eq!(state.attempts[0].status, AttemptStatus::Lost);
    assert_eq!(state.task(primary).unwrap().state, TaskState::Open);
    assert_eq!(state.attempts.len(), 1);

    // Only an explicit command grants again, at the next generation.
    let second = f.submit("start-2", Some(job), start(primary));
    accepted(&second);
    let state = f.journal.job(job).unwrap().unwrap();
    assert_eq!(state.attempts[1].generation, Generation::new(2).unwrap());
    // A report from the lost attempt is recorded as stale and changes nothing.
    let old = state.attempts[0].clone();
    let t = f
        .journal
        .observe_authority(
            job,
            &Observation::Exited {
                attempt: old.id,
                generation: old.generation,
                outcome: fabric_core::managed::ExitOutcome::Succeeded,
            },
            4_000,
        )
        .unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::StaleObservationRefused { .. }]
    ));
    let after = f.journal.job(job).unwrap().unwrap();
    assert_eq!(after.attempts, state.attempts);
    f.journal.verify().unwrap();
}

#[test]
fn a_running_probe_marks_a_granted_attempt_running() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    f.submit("start-1", Some(job), start(primary));
    let recovery = f.journal.recover(2_000, |_| Liveness::Running).unwrap();
    assert_eq!(recovery.running.len(), 1);
    let state = f.journal.job(job).unwrap().unwrap();
    assert_eq!(state.attempts[0].status, AttemptStatus::Running);
    f.journal.verify().unwrap();
}

#[test]
fn recovery_never_offers_a_lost_grant_and_it_cannot_run_next_to_its_replacement() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    let first = accepted(&f.submit("start-1", Some(job), start(primary))).operation;

    // Crash before dispatch; the probe verifies nothing runs.
    f.reopen();
    let recovery = f.journal.recover(2_000, |_| Liveness::Gone).unwrap();
    assert_eq!(recovery.lost.len(), 1);
    assert!(
        recovery.to_dispatch.is_empty(),
        "the grant of a lost attempt was offered"
    );

    let second = accepted(&f.submit("start-2", Some(job), start(primary))).operation;
    f.reopen();
    let recovery = f.journal.recover(3_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(recovery.to_dispatch, vec![(job, second)]);
    assert!(matches!(
        f.journal
            .mark_authority(job, first, OperationMark::Dispatched, 3_001),
        Err(CommitError::Domain(DomainError::OperationState { .. }))
    ));
    f.journal
        .mark_authority(job, second, OperationMark::Dispatched, 3_002)
        .unwrap();
    f.journal.verify().unwrap();
}

#[test]
fn recovery_never_offers_the_grant_of_an_attempt_it_saw_running() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    let grant = accepted(&f.submit("start-1", Some(job), start(primary))).operation;
    let recovery = f.journal.recover(2_000, |_| Liveness::Running).unwrap();
    assert_eq!(recovery.running.len(), 1);
    assert!(
        recovery.to_dispatch.is_empty(),
        "a running attempt was offered for a second launch"
    );
    assert!(matches!(
        f.journal
            .mark_authority(job, grant, OperationMark::Dispatched, 2_001),
        Err(CommitError::Domain(DomainError::OperationState { .. }))
    ));
    f.journal.verify().unwrap();
}

#[test]
fn events_and_evidence_are_append_only_and_tampering_is_found() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    f.submit("start", Some(job), start(primary));
    let attempt = f.journal.job(job).unwrap().unwrap().attempts[0].clone();
    let snapshot = SnapshotId::of(b"manifest");
    f.journal
        .observe_authority(
            job,
            &Observation::Checkpointed {
                attempt: attempt.id,
                generation: attempt.generation,
                snapshot,
                cut: fabric_core::managed::CheckpointCut {
                    snapshot,
                    receipts: vec![fabric_core::managed::TaskReceipt {
                        task: attempt.task,
                        class: fabric_core::managed::TaskClass::Harness,
                        status: fabric_core::managed::ReceiptStatus::Checkpointed { snapshot },
                        exit: None,
                        interrupted: true,
                        workspace: None,
                    }],
                    facts: Vec::new(),
                    evidence: Vec::new(),
                },
            },
            2_000,
        )
        .unwrap();
    // The snapshot's evidence row and the cut's.
    assert_eq!(count(&f.path(), "evidence"), 2);

    let conn = Connection::open(f.path()).unwrap();
    assert!(conn.execute("UPDATE events SET at_ms = 0", []).is_err());
    assert!(conn.execute("DELETE FROM events", []).is_err());
    assert!(conn.execute("DELETE FROM evidence", []).is_err());
    // Two grants at one generation are impossible in the file itself.
    let dup = conn.execute(
        "INSERT INTO attempts (attempt_id, task_id, job_id, generation, resource, granted_by,
                               status, status_body, granted_ms)
         SELECT randomblob(16), task_id, job_id, generation, resource, granted_by, status,
                status_body, granted_ms FROM attempts",
        [],
    );
    assert!(dup.is_err());

    // A projection that disagrees with the events stops verify.
    conn.execute("UPDATE jobs SET revision = revision + 1", [])
        .unwrap();
    assert!(matches!(
        f.journal.verify(),
        Err(CommitError::ProjectionMismatch { .. })
    ));
    conn.execute("UPDATE jobs SET revision = revision - 1", [])
        .unwrap();
    f.journal.verify().unwrap();

    // An edited event body breaks its digest.
    conn.execute_batch("DROP TRIGGER events_append_only_update")
        .unwrap();
    conn.execute(
        "UPDATE events SET body = zeroblob(length(body)) WHERE seq = (SELECT max(seq) FROM events)",
        [],
    )
    .unwrap();
    assert!(matches!(
        f.journal.verify(),
        Err(CommitError::Corrupt { .. })
    ));
}

#[test]
fn files_that_are_not_journals_or_are_newer_are_refused_unwritten() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("text");
    std::fs::write(&text, "hello").unwrap();
    assert!(matches!(
        Journal::open(&text),
        Err(OpenError::NotAJournal(_))
    ));
    let other = dir.path().join("other.sqlite");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE t (x INTEGER); PRAGMA user_version = 1;")
        .unwrap();
    assert!(matches!(
        Journal::open(&other),
        Err(OpenError::NotAJournal(_))
    ));
    assert!(matches!(
        Journal::open(&dir.path().join("missing")),
        Err(OpenError::NotAJournal(_))
    ));

    let f = Fixture::new();
    let newer = dir.path().join(FILE_NAME);
    f.journal.backup_to(&newer).unwrap();
    Connection::open(&newer)
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    let bytes = std::fs::read(&newer).unwrap();
    assert!(matches!(
        Journal::open(&newer),
        Err(OpenError::NewerSchema { found, known })
            if found == SCHEMA_VERSION + 1 && known == SCHEMA_VERSION
    ));
    assert_eq!(std::fs::read(&newer).unwrap(), bytes);
}

/// A journal an unreleased build made with the draft version 2 table,
/// before ADR 0022 completed it, is refused at open with what to do,
/// rather than failing later on a missing column.
#[test]
fn a_draft_version_2_journal_is_refused_at_open() {
    let f = Fixture::new();
    let path = f.path();
    drop(f.journal);
    let conn = Connection::open(&path).unwrap();
    // The draft table as an unreleased build wrote it.
    conn.execute_batch(
        "DROP TABLE provider_operations;
         CREATE TABLE provider_operations (
           operation_id BLOB PRIMARY KEY, op_key TEXT NOT NULL UNIQUE,
           request_digest BLOB NOT NULL, job_id BLOB NOT NULL, task_id BLOB NOT NULL,
           attempt_id BLOB NOT NULL, generation INTEGER NOT NULL, action TEXT NOT NULL,
           target TEXT, status TEXT NOT NULL, code TEXT, reason TEXT, effect TEXT,
           locator TEXT, dispatched_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
         ) STRICT;",
    )
    .unwrap();
    drop(conn);
    assert!(matches!(
        Journal::open(&path),
        Err(OpenError::DraftSchema { version: 2 })
    ));
}

#[test]
fn a_backup_opens_and_holds_the_same_jobs() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let target = f.dir.path().join("backup.sqlite");
    f.journal.backup_to(&target).unwrap();
    let copy = Journal::open(&target).unwrap();
    copy.verify().unwrap();
    assert_eq!(copy.job(job).unwrap(), f.journal.job(job).unwrap());
}

#[test]
fn events_page_in_order() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    f.submit(
        "cancel",
        Some(job),
        Command::CancelJob {
            reason: "done".into(),
        },
    );
    let all = f.journal.events(job, None, 100).unwrap();
    assert_eq!(all.len(), 5);
    let page = f.journal.events(job, Some(all[1].seq), 2).unwrap();
    assert_eq!(page, all[2..4].to_vec());
    assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(all[0].frame.prev, None);
}

// M6 (ADR 0028): the fence on every journal operation, acknowledgements,
// loss as availability and the derived key of an automatic recover.

/// A job whose first attempt is dispatched and running on `NODE`.
fn running(f: &mut Fixture) -> (JobId, GrantToken) {
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    let grant = accepted(&f.submit("start-1", Some(job), start(primary))).operation;
    f.journal
        .mark_authority(job, grant, OperationMark::Dispatched, 2_000)
        .unwrap();
    let attempt = f.journal.job(job).unwrap().unwrap().attempts[0].clone();
    let token = GrantToken {
        job,
        attempt: attempt.id,
        generation: attempt.generation,
    };
    f.journal
        .observe(
            &token,
            Some(NODE),
            &Observation::Started {
                attempt: attempt.id,
                generation: attempt.generation,
            },
            2_001,
        )
        .unwrap()
        .unwrap();
    (job, token)
}

fn captured(f: &mut Fixture, token: &GrantToken, n: u8) -> (SnapshotId, CutDigest) {
    let snapshot = SnapshotId::of(&[n; 3]);
    let cut = CutDigest::from_bytes([n; 32]);
    f.journal
        .observe(
            token,
            Some(NODE),
            &Observation::Captured {
                attempt: token.attempt,
                generation: token.generation,
                snapshot,
                cut,
            },
            2_100 + u64::from(n),
        )
        .unwrap()
        .unwrap();
    (snapshot, cut)
}

fn ack(token: &GrantToken, snapshot: SnapshotId, cut: CutDigest) -> CheckpointAck {
    CheckpointAck {
        attempt: token.attempt,
        generation: token.generation,
        snapshot,
        cut,
        manifests: 2,
        chunks: 7,
        bytes: 4_096,
        fetched_from: NODE,
    }
}

fn silent(f: &mut Fixture, token: &GrantToken, at_ms: u64) {
    f.journal
        .observe_authority(
            token.job,
            &Observation::ResourceUnavailable {
                attempt: token.attempt,
                generation: token.generation,
                last_heartbeat_ms: Some(at_ms - 10_000),
                at_ms,
            },
            at_ms,
        )
        .unwrap();
}

fn rows(path: &Path, sql: &str) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn acknowledge_writes_the_event_the_evidence_and_the_column() {
    let mut f = Fixture::new();
    let (job, token) = running(&mut f);
    let (snapshot, cut) = captured(&mut f, &token, 1);
    let path = f.path();
    assert_eq!(
        rows(&path, "SELECT kind FROM evidence ORDER BY rowid"),
        vec!["checkpoint"]
    );
    assert_eq!(
        rows(&path, "SELECT hex(last_acknowledged) FROM attempts"),
        vec![""],
        "captured is not acknowledged"
    );
    assert!(f.journal.acknowledged(job).unwrap().is_empty());
    let a = ack(&token, snapshot, cut);
    let t = f.journal.acknowledge(job, &a, 3_000).unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::CheckpointAcknowledged { .. }]
    ));
    assert_eq!(f.journal.acknowledged(job).unwrap(), vec![a.clone()]);
    assert_eq!(
        rows(&path, "SELECT kind FROM evidence ORDER BY rowid"),
        vec!["checkpoint", "checkpoint_ack"]
    );
    assert_eq!(
        rows(
            &path,
            "SELECT hex(digest) FROM evidence WHERE kind = 'checkpoint_ack'"
        ),
        vec![format!("{cut}").to_uppercase()]
    );
    assert_eq!(
        rows(&path, "SELECT hex(last_acknowledged) FROM attempts"),
        vec![format!("{snapshot}").to_uppercase()]
    );
    assert_eq!(
        rows(&path, "SELECT kind FROM events ORDER BY seq")
            .last()
            .map(String::as_str),
        Some("checkpoint_acknowledged")
    );
    // Again: nothing more written.
    let events = count(&path, "events");
    assert!(
        f.journal
            .acknowledge(job, &a, 3_001)
            .unwrap()
            .events
            .is_empty()
    );
    assert_eq!(count(&path, "events"), events);
    // The projection is the replay, before and after a reopen.
    f.journal.verify().unwrap();
    f.reopen();
    f.journal.verify().unwrap();
    assert_eq!(f.journal.acknowledged(job).unwrap(), vec![a]);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "every covered path and verdict is walked in one place"
)]
fn a_refused_covered_write_changes_nothing_else() {
    let mut f = Fixture::new();
    let (job, token) = running(&mut f);
    let path = f.path();
    let before = f.journal.job(job).unwrap().unwrap();
    let other = NodeId::from_bytes([0x77; 32]);
    // Every covered path from the wrong node: refused, recorded, nothing else.
    for path_kind in CoveredPath::ALL {
        let verdict = f
            .journal
            .fence(&token, path_kind, Some(other), 2_500)
            .unwrap()
            .unwrap_err();
        assert_eq!(
            verdict,
            FenceVerdict::WrongNode {
                granted: NODE,
                from: other
            }
        );
    }
    let after = f.journal.job(job).unwrap().unwrap();
    assert_eq!(after.attempts, before.attempts);
    assert_eq!(after.tasks, before.tasks);
    assert_eq!(after.operations, before.operations);
    assert_eq!(after.job.generation, before.job.generation);
    assert_eq!(after.job.revision.get(), before.job.revision.get() + 4);
    let kinds = rows(&path, "SELECT kind FROM events ORDER BY seq");
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "covered_write_refused")
            .count(),
        4
    );
    // A live token from its node writes nothing for a bare fence.
    let events = count(&path, "events");
    f.journal
        .fence(&token, CoveredPath::LedgerAppend, Some(NODE), 2_501)
        .unwrap()
        .unwrap();
    assert_eq!(count(&path, "events"), events);
    // Superseded: the report is refused on the journal path and the
    // attempt's exit never reaches the projection.
    silent(&mut f, &token, 12_600);
    let superseded = f.submit(
        "supersede",
        Some(job),
        Command::Supersede {
            attempt: token.attempt,
            reason: "test".into(),
        },
    );
    accepted(&superseded);
    let refused = f
        .journal
        .observe(
            &token,
            Some(NODE),
            &Observation::Exited {
                attempt: token.attempt,
                generation: token.generation,
                outcome: fabric_core::managed::ExitOutcome::Succeeded,
            },
            12_700,
        )
        .unwrap()
        .unwrap_err();
    assert_eq!(refused, FenceVerdict::Superseded { by: None });
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(
        s.attempt(token.attempt).unwrap().status,
        AttemptStatus::Superseded { by: None }
    );
    assert!(
        rows(&path, "SELECT kind FROM evidence")
            .iter()
            .all(|k| k != "exit"),
        "the superseded exit left no evidence"
    );
    // A mark through the fence is refused the same way.
    let refused = f
        .journal
        .mark(
            &token,
            Some(NODE),
            s.attempts[0].granted_by,
            OperationMark::Failed,
            12_701,
        )
        .unwrap()
        .unwrap_err();
    assert_eq!(refused, FenceVerdict::Superseded { by: None });
    // A later generation makes the old token stale, and a stale report is
    // recorded as the M2 refusal.
    let primary = s.job.primary;
    accepted(&f.submit("start-2", Some(job), start(primary)));
    let refused = f
        .journal
        .observe(
            &token,
            Some(NODE),
            &Observation::Started {
                attempt: token.attempt,
                generation: token.generation,
            },
            12_800,
        )
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refused,
        FenceVerdict::Superseded { by: None },
        "superseded wins over stale: it names the reason"
    );
    let ghost = GrantToken {
        attempt: fabric_core::managed::AttemptId::from_bytes([0xee; 16]),
        ..token
    };
    assert_eq!(
        f.journal
            .fence(&ghost, CoveredPath::FacadeOutcome, None, 12_801)
            .unwrap()
            .unwrap_err(),
        FenceVerdict::UnknownAttempt
    );
    let no_job = GrantToken {
        job: JobId::from_bytes([0xdd; 16]),
        ..token
    };
    let events = count(&path, "events");
    assert_eq!(
        f.journal
            .fence(&no_job, CoveredPath::FacadeOutcome, None, 12_802)
            .unwrap()
            .unwrap_err(),
        FenceVerdict::UnknownJob
    );
    assert_eq!(
        count(&path, "events"),
        events,
        "no job, nothing to record against"
    );
    f.journal.verify().unwrap();
}

/// A runner that keeps writing under a grant it lost, or any peer that
/// makes tokens up, cannot grow a job's history without bound: the same
/// refusal is recorded once per attempt, path and verdict while the
/// journal is open, a token that names no attempt of the job is refused
/// without a record, and a different verdict for the same attempt is
/// recorded again.
#[test]
fn two_wrong_nodes_alternating_are_recorded_once() {
    let mut f = Fixture::new();
    let (job, token) = running(&mut f);
    let x = NodeId::from_bytes([0x77; 32]);
    let y = NodeId::from_bytes([0x78; 32]);
    let revision = |f: &Fixture| f.journal.job(job).unwrap().unwrap().job.revision.get();
    let before = revision(&f);
    for i in 0..6 {
        let from = if i % 2 == 0 { x } else { y };
        let verdict = f
            .journal
            .fence(&token, CoveredPath::LedgerAppend, Some(from), 3_000 + i)
            .unwrap()
            .unwrap_err();
        assert!(matches!(verdict, FenceVerdict::WrongNode { .. }));
    }
    assert_eq!(
        revision(&f),
        before + 1,
        "a wrong node is one refusal whichever peer sent it"
    );
}

#[test]
fn a_repeated_refusal_is_recorded_once_and_a_made_up_attempt_never() {
    let mut f = Fixture::new();
    let (job, token) = running(&mut f);
    let path = f.path();
    let other = NodeId::from_bytes([0x77; 32]);
    let revision = |f: &Fixture| f.journal.job(job).unwrap().unwrap().job.revision.get();
    let before = revision(&f);
    for i in 0..5 {
        let verdict = f
            .journal
            .fence(&token, CoveredPath::LedgerAppend, Some(other), 3_000 + i)
            .unwrap()
            .unwrap_err();
        assert!(matches!(verdict, FenceVerdict::WrongNode { .. }));
    }
    assert_eq!(
        revision(&f),
        before + 1,
        "five identical refusals, one record"
    );
    // Another path for the same token: its own record, once.
    for i in 0..3 {
        f.journal
            .fence(&token, CoveredPath::BundlePublish, Some(other), 3_100 + i)
            .unwrap()
            .unwrap_err();
    }
    assert_eq!(revision(&f), before + 2);
    // A token for an attempt the job does not have: refused, never recorded.
    let ghost = GrantToken {
        attempt: fabric_core::managed::AttemptId::from_bytes([0xee; 16]),
        ..token
    };
    for i in 0..5 {
        let verdict = f
            .journal
            .fence(&ghost, CoveredPath::LedgerAppend, Some(NODE), 3_200 + i)
            .unwrap()
            .unwrap_err();
        assert_eq!(verdict, FenceVerdict::UnknownAttempt);
        let ghost_report = f
            .journal
            .observe(
                &ghost,
                Some(NODE),
                &Observation::Started {
                    attempt: ghost.attempt,
                    generation: ghost.generation,
                },
                3_300 + i,
            )
            .unwrap()
            .unwrap_err();
        assert_eq!(ghost_report, FenceVerdict::UnknownAttempt);
    }
    assert_eq!(revision(&f), before + 2);
    // A new verdict for the same attempt and path is recorded again.
    silent(&mut f, &token, 13_000);
    let supersede = f.submit(
        "supersede",
        Some(job),
        Command::Supersede {
            attempt: token.attempt,
            reason: "test".into(),
        },
    );
    accepted(&supersede);
    let after_supersede = revision(&f);
    for i in 0..3 {
        let verdict = f
            .journal
            .fence(&token, CoveredPath::LedgerAppend, Some(NODE), 13_100 + i)
            .unwrap()
            .unwrap_err();
        assert_eq!(verdict, FenceVerdict::Superseded { by: None });
    }
    assert_eq!(revision(&f), after_supersede + 1);
    let kinds = rows(&path, "SELECT kind FROM events ORDER BY seq");
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "covered_write_refused")
            .count(),
        3
    );
    // After a reopen the memory is gone: the first repeat is recorded
    // once more, and the next ones are not.
    f.reopen();
    for i in 0..3 {
        f.journal
            .fence(&token, CoveredPath::LedgerAppend, Some(NODE), 13_200 + i)
            .unwrap()
            .unwrap_err();
    }
    assert_eq!(revision(&f), after_supersede + 2);
}

#[test]
fn recover_never_marks_lost_from_a_missed_heartbeat() {
    let mut f = Fixture::new();
    let (job, token) = running(&mut f);
    silent(&mut f, &token, 20_000);
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Unknown);
    assert_eq!(s.attempts[0].unavailable_since_ms, Some(20_000));
    // Restart recovery probes only what it can verify: an unknown attempt
    // is not granted, running or lost; nothing is offered for dispatch.
    f.reopen();
    let recovery = f.journal.recover(21_000, |_| Liveness::Gone).unwrap();
    assert_eq!(recovery, Recovery::default());
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Unknown);
    // The heartbeat returns: running again, the column cleared.
    f.journal
        .observe_authority(
            job,
            &Observation::ResourceReachable {
                attempt: token.attempt,
                generation: token.generation,
            },
            22_000,
        )
        .unwrap();
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Running);
    assert_eq!(s.attempts[0].unavailable_since_ms, None);
    assert_eq!(
        rows(
            &f.path(),
            "SELECT CASE WHEN unavailable_since_ms IS NULL THEN 'null' ELSE 'set' END FROM attempts"
        ),
        vec!["null"]
    );
    f.journal.verify().unwrap();
}

#[test]
fn an_automatic_recover_replays_under_its_derived_key() {
    let mut f = Fixture::new();
    let (job, token) = running(&mut f);
    let (snapshot, cut) = captured(&mut f, &token, 1);
    f.journal
        .acknowledge(job, &ack(&token, snapshot, cut), 3_000)
        .unwrap();
    silent(&mut f, &token, 20_000);
    let decided = fabric_core::managed::recovery_decision(
        &f.journal.job(job).unwrap().unwrap(),
        token.attempt,
        fabric_core::managed::OutcomePolicy::Checkpoint,
        &[],
    )
    .unwrap();
    // The key is derived from (job, attempt, generation): the same decision
    // after a restart replays the same operation and takes nothing twice.
    let key = format!("recover.{job}.{}.{}", token.attempt, token.generation.get());
    let first = f.submit(&key, Some(job), decided.clone());
    assert!(!first.replayed);
    let again = f.submit(&key, Some(job), decided);
    assert!(again.replayed);
    assert_eq!(again.bytes, first.bytes);
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(s.attempts.len(), 1, "the grant follows the placement");
    assert_eq!(s.attempts[0].status, AttemptStatus::Superseded { by: None });
    // The failover placement grants once, from the acknowledged checkpoint.
    let StoredResult::Accepted(accepted) = &first.result else {
        panic!("not accepted: {:?}", first.result);
    };
    let here = NodeId::from_bytes([0x11; 32]);
    f.journal
        .observe_authority(
            job,
            &Observation::Placed {
                operation: accepted.operation,
                task: s.attempts[0].task,
                decision: fabric_core::managed::PlacementDecision {
                    kind: fabric_core::managed::PlacementKind::Failover,
                    decided_at_ms: 21_000,
                    candidates: Vec::new(),
                    chosen: Some(here),
                },
            },
            21_000,
        )
        .unwrap();
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(s.attempts.len(), 2);
    assert_eq!(s.job.generation.get(), 2);
    assert_eq!(s.attempts[1].resource, ResourceRef::Node { node: here });
    assert_eq!(s.attempts[1].initial_checkpoint, Some(snapshot));
    assert_eq!(
        rows(
            &f.path(),
            "SELECT kind FROM operations WHERE op_key LIKE 'recover.%'"
        ),
        vec!["recover"]
    );
    // A blocked decision is recorded and the job stays as it was.
    let token2 = GrantToken {
        job,
        attempt: s.attempts[1].id,
        generation: s.attempts[1].generation,
    };
    f.journal
        .mark_authority(
            job,
            s.attempts[1].granted_by,
            OperationMark::Dispatched,
            20_001,
        )
        .unwrap();
    silent(&mut f, &token2, 40_000);
    let t = f
        .journal
        .recovery_blocked(
            job,
            token2.attempt,
            fabric_core::managed::RecoveryBlock::NoAcknowledgedCheckpoint,
            40_001,
        )
        .unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::RecoveryBlocked { .. }]
    ));
    let s = f.journal.job(job).unwrap().unwrap();
    assert_eq!(s.attempts[1].status, AttemptStatus::Unknown);
    f.journal.verify().unwrap();
    f.reopen();
    f.journal.verify().unwrap();
}

/// The receipts read of one job never scans the events table: it searches
/// `events_by_job` for that job alone, whatever other jobs hold.
#[test]
fn the_receipts_read_searches_one_jobs_events() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::create(&dir.path().join(FILE_NAME), NODE, 1).unwrap();
    let plan: Vec<String> = journal
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {RECEIPTS_SQL}"))
        .unwrap()
        .query_map([JobId::from_bytes([1; 16]).to_bytes()], |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        plan.iter().any(|step| step.contains("events_by_job")),
        "{plan:?}"
    );
    assert!(
        !plan.iter().any(|step| step.starts_with("SCAN events")),
        "{plan:?}"
    );
}
