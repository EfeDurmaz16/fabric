//! The journal's contract: idempotent commands, restart recovery,
//! tamper evidence and the file's version checks.

use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{
    AttemptStatus, Command, DomainError, Envelope, Generation, Intent, JobPolicy, ManagedEvent,
    NewIds, Observation, OperationKey, OperationMark, OperationStatus, ResourceRef, RestartPolicy,
    SourceBinding, StoredResult, TaskState, WorkRevision,
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
        },
        source: SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        policy: JobPolicy {
            restart: RestartPolicy::ExplicitOnly,
        },
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
        .mark(job, grant, OperationMark::Dispatched, 2_000)
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
    // A second recovery changes nothing more.
    let again = f.journal.recover(4_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(again, Recovery::default());
    // The executor's late report settles it.
    let attempt = after.attempts[0].clone();
    f.journal
        .observe(
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
        .mark(job, grant, OperationMark::Dispatched, 2_001)
        .unwrap();
    assert!(matches!(
        f.journal.mark(job, grant, OperationMark::Dispatched, 2_002),
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
        .observe(
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
fn events_and_evidence_are_append_only_and_tampering_is_found() {
    let mut f = Fixture::new();
    let job = accepted(&f.submit("create", None, create())).job;
    let primary = f.journal.job(job).unwrap().unwrap().job.primary;
    f.submit("start", Some(job), start(primary));
    let attempt = f.journal.job(job).unwrap().unwrap().attempts[0].clone();
    let snapshot = SnapshotId::of(b"manifest");
    f.journal
        .observe(
            job,
            &Observation::Checkpointed {
                attempt: attempt.id,
                generation: attempt.generation,
                snapshot,
            },
            2_000,
        )
        .unwrap();
    assert_eq!(count(&f.path(), "evidence"), 1);

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
