//! The authority in the daemon: the journal is made by the first managed
//! command, a restart keeps identity and progress, a managed command never
//! touches a standalone job, and a state directory from main before
//! managed jobs is byte-identical after a managed job is made beside it.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::launch::Heartbeat;
use fabric_core::managed::{
    AttemptStatus, CheckpointAck, Command, CutDigest, DomainError, Envelope, FenceVerdict,
    Generation, GrantToken, Intent, JobPolicy, ManagedEvent, Observation, OperationKey,
    OperationMark, OperationStatus, RecoveryBlock, RecoveryPolicy, ResourceRef, RestartPolicy,
    SourceBinding, StoredResult, UNAVAILABLE_AFTER_MS,
};
use fabric_core::path::AbsPath;
use fabric_journal::Answer;

use super::*;

const NODE: NodeId = NodeId::from_bytes([5; 32]);

/// The legacy fixture of commit 176622e (crates/fabricd/tests/fixtures).
fn legacy_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-176622e")
}

fn state(dir: &Path) -> StateDir {
    StateDir::resolve(Some(dir.as_os_str()), Path::new("/")).unwrap()
}

fn create(key: &str) -> Envelope<Command> {
    Envelope {
        key: OperationKey::parse(key).unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: "make the greeting louder".into(),
                harness: HarnessKind::Codex,
                acceptance: Vec::new(),
            },
            source: SourceBinding::Repository {
                path: AbsPath::parse("/src/repo").unwrap(),
            },
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
        },
    }
}

fn job_of(answer: &Answer) -> JobId {
    match &answer.result {
        StoredResult::Accepted(a) => a.job,
        StoredResult::Rejected(e) => panic!("rejected: {e}"),
    }
}

/// Every file under `root` with its bytes.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

fn copy_tree(from: &Path, to: &Path) {
    let status = std::process::Command::new("cp")
        .arg("-pR")
        .arg(format!("{}/.", from.display()))
        .arg(to)
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn a_start_makes_no_journal_and_the_first_create_does() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    assert!(authority.list().await.unwrap().is_empty());
    assert!(
        authority
            .job(JobId::from_bytes([1; 16]))
            .await
            .unwrap()
            .is_none()
    );
    assert!(!managed_dir(&state).exists());
    let cancel = Envelope {
        key: OperationKey::parse("cancel").unwrap(),
        job: Some(JobId::from_bytes([1; 16])),
        expected: None,
        body: Command::CancelJob { reason: "x".into() },
    };
    let refused = authority.submit(cancel).await.unwrap_err();
    assert!(matches!(
        refused.domain(),
        Some(DomainError::UnknownJob { .. })
    ));
    assert!(
        !managed_dir(&state).exists(),
        "a refused command made a journal"
    );

    let made = authority.submit(create("create-1")).await.unwrap();
    assert!(journal_path(&state).exists());
    let dir_mode = std::fs::metadata(managed_dir(&state))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
    let job = job_of(&made);
    assert_eq!(authority.list().await.unwrap()[0].job.id, job);
}

#[tokio::test]
async fn a_restart_keeps_identity_progress_and_the_answer_to_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let made = authority.submit(create("create-1")).await.unwrap();
    let job = job_of(&made);
    let primary = authority.job(job).await.unwrap().unwrap().job.primary;
    let start = Envelope {
        key: OperationKey::parse("start-1").unwrap(),
        job: Some(job),
        expected: None,
        body: Command::StartAttempt {
            task: primary,
            resource: ResourceRef::Node { node: NODE },
            initial_checkpoint: None,
        },
    };
    let granted = authority.submit(start.clone()).await.unwrap();
    let StoredResult::Accepted(grant) = &granted.result else {
        panic!("the grant was refused");
    };
    authority
        .mark_authority(job, grant.operation, OperationMark::Dispatched)
        .await
        .unwrap();
    let before = authority.job(job).await.unwrap().unwrap();
    drop(authority);

    // The restarted authority recovers before it answers.
    let authority = Authority::start(&state, NODE).unwrap();
    let after = authority.job(job).await.unwrap().unwrap();
    assert_eq!(after.job.id, before.job.id);
    assert_eq!(after.job.primary, before.job.primary);
    assert_eq!(after.attempts[0].id, before.attempts[0].id);
    assert_eq!(after.job.revision.get(), before.job.revision.get() + 1);
    assert_eq!(
        after.operation(grant.operation).unwrap().status,
        OperationStatus::Unknown
    );
    let again = authority.submit(create("create-1")).await.unwrap();
    assert!(again.replayed);
    assert_eq!(again.bytes, made.bytes);
    let regrant = authority.submit(start).await.unwrap();
    assert!(regrant.replayed);
    assert_eq!(regrant.bytes, granted.bytes);
    assert_eq!(authority.list().await.unwrap().len(), 1);

    // The executor's late report settles the unknown grant.
    let attempt = after.attempts[0].clone();
    authority
        .observe_authority(
            job,
            fabric_core::managed::Observation::Started {
                attempt: attempt.id,
                generation: attempt.generation,
            },
        )
        .await
        .unwrap();
    let settled = authority.job(job).await.unwrap().unwrap();
    assert_eq!(
        settled.operation(grant.operation).unwrap().status,
        OperationStatus::Succeeded
    );
}

#[tokio::test]
async fn the_managed_ids_are_published_outside_the_queue_and_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let other = JobId::from_bytes([9; 16]);
    let authority = Authority::start(&state, NODE).unwrap();
    assert!(!authority.holds(other).await.unwrap());
    let job = job_of(&authority.submit(create("create-1")).await.unwrap());
    assert!(authority.holds(job).await.unwrap());
    assert!(!authority.holds(other).await.unwrap());
    drop(authority);

    let authority = Authority::start(&state, NODE).unwrap();
    assert!(authority.holds(job).await.unwrap());
    assert!(!authority.holds(other).await.unwrap());
}

#[tokio::test]
async fn a_journal_that_does_not_open_leaves_every_id_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    std::fs::create_dir_all(managed_dir(&state)).unwrap();
    std::fs::write(journal_path(&state), b"not a journal").unwrap();
    let authority = Authority::start(&state, NODE).unwrap();
    assert!(matches!(
        authority.holds(JobId::from_bytes([9; 16])).await,
        Err(AuthorityError::Unavailable(_))
    ));
}

#[tokio::test]
async fn a_journal_that_does_not_verify_stops_the_authority_not_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    std::fs::create_dir_all(managed_dir(&state)).unwrap();
    std::fs::write(journal_path(&state), b"not a journal").unwrap();
    let authority = Authority::start(&state, NODE).unwrap();
    assert!(matches!(
        authority.list().await,
        Err(AuthorityError::Unavailable(_))
    ));
    assert!(matches!(
        authority.submit(create("c")).await,
        Err(AuthorityError::Unavailable(_))
    ));
    assert_eq!(
        std::fs::read(journal_path(&state)).unwrap(),
        b"not a journal"
    );
}

#[tokio::test]
async fn legacy_state_from_main_is_untouched_by_a_managed_job_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&legacy_fixture().join("state"), dir.path());
    let state = state(dir.path());
    let legacy: JobId = std::fs::read_to_string(legacy_fixture().join("job"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let before = tree(dir.path());

    // The frozen ledger opens with this binary's reader.
    let file = crate::ledger_file::LedgerFile::open(&state.job(legacy).ledger()).unwrap();
    let ledger = fabric_core::ledger::Ledger::open(legacy, &file).unwrap();
    assert_eq!(ledger.lease().epoch().get(), 1);
    drop(file);

    let authority = Authority::start(&state, NODE).unwrap();
    let made = authority.submit(create("create-1")).await.unwrap();
    let managed = job_of(&made);
    assert_ne!(managed, legacy);

    // A managed command never acts on the standalone job.
    let cancel = Envelope {
        key: OperationKey::parse("cancel-legacy").unwrap(),
        job: Some(legacy),
        expected: None,
        body: Command::CancelJob { reason: "x".into() },
    };
    assert!(matches!(
        authority.submit(cancel).await,
        Err(AuthorityError::Standalone(job)) if job == legacy
    ));
    drop(authority);

    let after = tree(dir.path());
    for (path, bytes) in &before {
        assert_eq!(after.get(path), Some(bytes), "{} changed", path.display());
    }
    let added: Vec<_> = after.keys().filter(|p| !before.contains_key(*p)).collect();
    assert!(
        added.iter().all(|p| p.starts_with("managed")),
        "files outside managed/: {added:?}"
    );
}

/// Perf A/B item 6 (docs/evidence/perf-ab/README.md): every daemon
/// started the authority's thread, journal or not, which cost each one a
/// thread and about 80 KiB against main. The thread starts with the
/// daemon only for an existing journal, which it recovers before any
/// call, and otherwise with the first managed command; a read before
/// that answers from the start's look, which found no journal.
#[tokio::test]
async fn the_thread_starts_with_a_journal_or_with_the_first_managed_command() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let other = JobId::from_bytes([1; 16]);
    let authority = Authority::start(&state, NODE).unwrap();
    assert_eq!(
        authority.threads(),
        0,
        "a start without a journal started the thread"
    );
    assert!(authority.list().await.unwrap().is_empty());
    assert!(authority.job(other).await.unwrap().is_none());
    assert!(!authority.holds(other).await.unwrap());
    assert_eq!(authority.threads(), 0, "a read started the thread");

    let job = job_of(&authority.submit(create("create-1")).await.unwrap());
    assert_eq!(authority.threads(), 1);
    assert!(authority.holds(job).await.unwrap());
    assert_eq!(authority.list().await.unwrap()[0].job.id, job);
    let second = authority.clone();
    second.submit(create("create-2")).await.unwrap();
    assert_eq!(authority.list().await.unwrap().len(), 2);
    assert_eq!(
        authority.threads(),
        1,
        "a second command started another thread"
    );
    drop((authority, second));

    // With a journal, the thread starts with the daemon and recovers it.
    let authority = Authority::start(&state, NODE).unwrap();
    assert_eq!(authority.threads(), 1);
    assert!(authority.holds(job).await.unwrap());
    assert_eq!(authority.list().await.unwrap().len(), 2);
}

/// A journal whose presence cannot be read, here under a managed
/// directory this user cannot search, is not taken for no journal: the
/// start leaves it to the thread, which refuses every id as it does for a
/// journal that does not open, so a standalone job is never made under a
/// managed one's id.
#[tokio::test]
async fn a_journal_that_cannot_be_looked_at_refuses_every_id() {
    // Root searches any directory, so the look cannot fail.
    if rustix::process::geteuid().as_raw() == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let managed = managed_dir(&state);
    std::fs::create_dir_all(&managed).unwrap();
    std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let authority = Authority::start(&state, NODE).unwrap();
    let held = authority.holds(JobId::from_bytes([9; 16])).await;
    std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(held, Err(AuthorityError::Unavailable(_))),
        "{held:?}"
    );
    assert_eq!(authority.threads(), 1);
}

// M6 (ADR 0028): heartbeats, presence and the recovery policy in the daemon.

const RUNNER: NodeId = NodeId::from_bytes([7; 32]);
const OTHER: NodeId = NodeId::from_bytes([8; 32]);

fn create_with(key: &str, recovery: RecoveryPolicy) -> Envelope<Command> {
    let mut envelope = create(key);
    if let Command::CreateJob { policy, .. } = &mut envelope.body {
        policy.recovery = recovery;
    }
    envelope
}

/// A job whose first attempt runs on `RUNNER`, as its executor reported.
async fn running_on_runner(authority: &Authority, recovery: RecoveryPolicy) -> (JobId, GrantToken) {
    let made = authority
        .submit(create_with("create-1", recovery))
        .await
        .unwrap();
    let job = job_of(&made);
    let primary = authority.job(job).await.unwrap().unwrap().job.primary;
    let start = Envelope {
        key: OperationKey::parse("start-1").unwrap(),
        job: Some(job),
        expected: None,
        body: Command::StartAttempt {
            task: primary,
            resource: ResourceRef::Node { node: RUNNER },
            initial_checkpoint: None,
        },
    };
    let granted = authority.submit(start).await.unwrap();
    let StoredResult::Accepted(grant) = &granted.result else {
        panic!("the grant was refused");
    };
    authority
        .mark_authority(job, grant.operation, OperationMark::Dispatched)
        .await
        .unwrap();
    let attempt = authority.job(job).await.unwrap().unwrap().attempts[0].clone();
    let token = GrantToken {
        job,
        attempt: attempt.id,
        generation: attempt.generation,
    };
    authority
        .report(
            token,
            Some(RUNNER),
            Observation::Started {
                attempt: token.attempt,
                generation: token.generation,
            },
        )
        .await
        .unwrap()
        .unwrap();
    (job, token)
}

fn beat(token: GrantToken) -> Heartbeat {
    Heartbeat {
        attempts: vec![token],
        running: true,
    }
}

async fn status(authority: &Authority, token: GrantToken) -> AttemptStatus {
    authority
        .job(token.job)
        .await
        .unwrap()
        .unwrap()
        .attempt(token.attempt)
        .unwrap()
        .status
        .clone()
}

async fn revision(authority: &Authority, job: JobId) -> u64 {
    authority
        .job(job)
        .await
        .unwrap()
        .unwrap()
        .job
        .revision
        .get()
}

#[tokio::test]
async fn presence_marks_overdue_on_the_authority_clock_and_writes_no_heartbeat() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    let before = revision(&authority, job).await;
    // A heartbeat from the runner: live, and nothing written.
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(answer.verdicts, vec![(token.attempt, FenceVerdict::Live)]);
    assert_eq!(revision(&authority, job).await, before);
    // One from another node: wrong node, nothing written, nothing reset.
    let answer = authority.heartbeat(OTHER, beat(token)).await.unwrap();
    assert_eq!(
        answer.verdicts,
        vec![(
            token.attempt,
            FenceVerdict::WrongNode {
                granted: RUNNER,
                from: OTHER
            }
        )]
    );
    assert_eq!(revision(&authority, job).await, before);
    // Silence short of the threshold on the authority's monotonic clock: nothing.
    let now = mono_ms();
    assert!(authority.tick(now).await.unwrap().is_empty());
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
    // Past it: unknown, with the state visible; the job waits under Block.
    let overdue = authority.tick(now + UNAVAILABLE_AFTER_MS).await.unwrap();
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].attempt, token.attempt);
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Unknown);
    assert!(s.attempts[0].unavailable_since_ms.is_some());
    assert_eq!(s.job.generation, Generation::FIRST);
    assert!(s.live_attempt().is_some(), "unknown still may run");
    // Another tick records nothing more under Block.
    let after = revision(&authority, job).await;
    assert!(
        authority
            .tick(now + 2 * UNAVAILABLE_AFTER_MS)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(revision(&authority, job).await, after);
    // The heartbeat returns: one observation, running again, live.
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(answer.verdicts, vec![(token.attempt, FenceVerdict::Live)]);
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
    assert_eq!(revision(&authority, job).await, after + 1);
    // A restarted authority tracks the live remote attempt again.
    drop(authority);
    let authority = Authority::start(&state, NODE).unwrap();
    assert!(authority.tick(mono_ms()).await.unwrap().is_empty());
    let overdue = authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(overdue.len(), 1);
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
}

#[tokio::test]
async fn a_heartbeat_or_a_tick_that_writes_nothing_wakes_no_watch() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (_job, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    let mut commits = authority.commits().unwrap();
    commits.borrow_and_update();
    let now = mono_ms();
    assert!(authority.tick(now).await.unwrap().is_empty());
    authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    authority.heartbeat(OTHER, beat(token)).await.unwrap();
    assert!(
        !commits.has_changed().unwrap(),
        "nothing was written, so no watch wakes"
    );
    // A tick that makes the attempt unknown writes, and moves the counter.
    assert_eq!(
        authority
            .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(commits.has_changed().unwrap());
    commits.borrow_and_update();
    // The heartbeat that brings it back writes too.
    authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert!(commits.has_changed().unwrap());
}

/// The runner captures a checkpoint and the authority acknowledges it
/// after its own fetch (the fetch is M5's executor path; the
/// acknowledgement is what recovery reads).
async fn acknowledged(authority: &Authority, job: JobId, token: GrantToken) -> SnapshotId {
    let snapshot = SnapshotId::of(b"the runner's checkpoint");
    let cut = CutDigest::from_bytes([0x1c; 32]);
    authority
        .report(
            token,
            Some(RUNNER),
            Observation::Captured {
                attempt: token.attempt,
                generation: token.generation,
                snapshot,
                cut,
            },
        )
        .await
        .unwrap()
        .unwrap();
    authority
        .acknowledge(
            job,
            CheckpointAck {
                attempt: token.attempt,
                generation: token.generation,
                snapshot,
                cut,
                manifests: 1,
                chunks: 1,
                bytes: 1,
                fetched_from: RUNNER,
            },
        )
        .await
        .unwrap();
    snapshot
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the whole loss, restart and wake reads best as one scenario"
)]
async fn restart_from_acknowledged_recovers_onto_this_node_and_fences_the_old_runner() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::RestartFromAcknowledged).await;
    let now = mono_ms();
    // Silent: unknown after the threshold; the policy waits as long again.
    authority.tick(now + UNAVAILABLE_AFTER_MS).await.unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Unknown);
    assert_eq!(s.job.generation, Generation::FIRST);
    let unknown_at = s.job.revision.get();
    // Unknown for the threshold with nothing acknowledged: blocked,
    // recorded once, and not again on the next tick.
    authority
        .tick(now + 2 * UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(
        s.job.generation,
        Generation::FIRST,
        "nothing to restart from"
    );
    assert_eq!(
        s.job.revision.get(),
        unknown_at + 1,
        "the block is recorded"
    );
    authority
        .tick(now + 2 * UNAVAILABLE_AFTER_MS + 2_000)
        .await
        .unwrap();
    assert_eq!(
        revision(&authority, job).await,
        unknown_at + 1,
        "the same reason is not recorded twice"
    );
    // The runner comes back and a checkpoint of its is acknowledged.
    authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
    let snapshot = acknowledged(&authority, job, token).await;
    // Silent again: unknown at the threshold, still the same grant while
    // it dwells, and recovered onto this node from the acknowledged
    // checkpoint once it has been unknown for the threshold.
    let later = mono_ms() + UNAVAILABLE_AFTER_MS;
    authority.tick(later).await.unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Unknown);
    assert_eq!(
        s.job.generation,
        Generation::FIRST,
        "no restart before the dwell"
    );
    let dwelling = s.job.revision.get();
    authority.tick(later + 2_000).await.unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.job.generation, Generation::FIRST);
    assert_eq!(
        s.job.revision.get(),
        dwelling,
        "a tick inside the dwell writes nothing"
    );
    authority.tick(later + UNAVAILABLE_AFTER_MS).await.unwrap();
    // The recover takes the grant at once; its failover is placed by the
    // dispatch, which never chooses the runner's node.
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.job.generation, Generation::FIRST);
    assert_eq!(s.attempts[0].status, AttemptStatus::Superseded { by: None });
    let recover = recover_of(&s);
    assert_eq!(
        s.request(recover),
        Some(&fabric_core::managed::EffectRequest::Recover {
            task: s.attempts[0].task,
            from: snapshot,
            lost: RUNNER,
        })
    );
    assert!(
        authority
            .pending()
            .await
            .unwrap()
            .iter()
            .any(|e| e.operation == recover),
        "the failover waits for the dispatch"
    );
    failover(&authority, job, recover, NODE).await;
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.job.generation.get(), 2);
    assert_eq!(
        s.attempts[0].status,
        AttemptStatus::Superseded {
            by: Some(Generation::new(2).unwrap())
        }
    );
    let next = &s.attempts[1];
    assert_eq!(next.resource, ResourceRef::Node { node: NODE });
    assert_eq!(next.initial_checkpoint, Some(snapshot));
    assert_eq!(next.status, AttemptStatus::Granted);
    let recover = s.operation(next.granted_by).unwrap();
    assert_eq!(recover.kind, fabric_core::managed::CommandKind::Recover);
    assert!(recover.key.as_str().starts_with("recover."));
    // The old runner wakes: its heartbeat is answered superseded, its
    // writes are refused and recorded, and the job does not move.
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(
        answer.verdicts,
        vec![(
            token.attempt,
            FenceVerdict::Superseded {
                by: Some(Generation::new(2).unwrap())
            }
        )]
    );
    let refused = authority
        .report(
            token,
            Some(RUNNER),
            Observation::Exited {
                attempt: token.attempt,
                generation: token.generation,
                outcome: fabric_core::managed::ExitOutcome::Succeeded,
            },
        )
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(refused, FenceVerdict::Superseded { .. }));
    let refused = authority
        .fenced_write(
            token,
            fabric_core::managed::CoveredPath::LedgerAppend,
            Some(RUNNER),
            || panic!("a refused write ran"),
        )
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(refused, FenceVerdict::Superseded { .. }));
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.job.generation.get(), 2);
    assert_eq!(s.attempts.len(), 2);
    assert_eq!(s.attempts[1].status, AttemptStatus::Granted);
    // The new attempt is local: no heartbeat, never unavailable this way.
    assert!(
        authority
            .tick(later + 4 * UNAVAILABLE_AFTER_MS)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        status(&authority, token).await,
        AttemptStatus::Superseded {
            by: Some(Generation::new(2).unwrap())
        }
    );
    // A restart replays the authority's recover under its derived key
    // rather than granting again.
    drop(authority);
    let authority = Authority::start(&state, NODE).unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.attempts.len(), 2);
    assert_eq!(s.job.generation.get(), 2);
}

/// A failover placement that chooses the runner's own node is refused by
/// the domain: the attempt it replaces may still run there. Nothing is
/// granted and the old grant stays taken.
#[tokio::test]
async fn a_failover_onto_the_unavailable_node_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::RestartFromAcknowledged).await;
    acknowledged(&authority, job, token).await;
    let now = mono_ms();
    authority.tick(now + UNAVAILABLE_AFTER_MS).await.unwrap();
    authority
        .tick(now + 2 * UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    let recover = recover_of(&s);
    let task = s.attempts[0].task;
    let refused = authority
        .observe_authority(
            job,
            Observation::Placed {
                operation: recover,
                task,
                decision: fabric_core::managed::PlacementDecision {
                    kind: fabric_core::managed::PlacementKind::Failover,
                    decided_at_ms: 1,
                    candidates: Vec::new(),
                    chosen: Some(RUNNER),
                },
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.domain(),
        Some(&fabric_core::managed::DomainError::FailoverToLostNode { node: RUNNER })
    );
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.attempts.len(), 1, "nothing was granted");
    assert_eq!(s.attempts[0].status, AttemptStatus::Superseded { by: None });
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(
        answer.verdicts,
        vec![(token.attempt, FenceVerdict::Superseded { by: None })]
    );
}

/// The job's open recover.
fn recover_of(s: &ManagedState) -> fabric_core::managed::OperationId {
    s.operations
        .iter()
        .rev()
        .find(|o| o.kind == fabric_core::managed::CommandKind::Recover)
        .expect("a recover")
        .id
}

/// The dispatch's failover placement of `recover`, choosing `node`.
async fn failover(
    authority: &Authority,
    job: fabric_core::id::JobId,
    recover: fabric_core::managed::OperationId,
    node: NodeId,
) {
    let s = authority.job(job).await.unwrap().unwrap();
    let Some(fabric_core::managed::EffectRequest::Recover { task, .. }) = s.request(recover) else {
        panic!("not a recover");
    };
    authority
        .observe_authority(
            job,
            Observation::Placed {
                operation: recover,
                task: *task,
                decision: fabric_core::managed::PlacementDecision {
                    kind: fabric_core::managed::PlacementKind::Failover,
                    decided_at_ms: 1,
                    candidates: Vec::new(),
                    chosen: Some(node),
                },
            },
        )
        .await
        .unwrap();
}

/// An attempt that was `unknown` when the authority restarted is not
/// wedged: its returning heartbeat makes it `running` and live, its
/// covered writes are taken, and under `RestartFromAcknowledged` the
/// recovery policy still runs, with the dwell counted from the restart.
#[tokio::test]
async fn an_attempt_unknown_at_a_restart_comes_back_on_its_heartbeat_and_still_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    drop(authority);
    let authority = Authority::start(&state, NODE).unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    let before = revision(&authority, job).await;
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(answer.verdicts, vec![(token.attempt, FenceVerdict::Live)]);
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
    assert_eq!(revision(&authority, job).await, before + 1, "one reachable");
    let written = authority
        .fenced_write(
            token,
            fabric_core::managed::CoveredPath::LedgerAppend,
            Some(RUNNER),
            || 7,
        )
        .await
        .unwrap();
    assert_eq!(written, Ok(7));
    // Silence again after the restart is counted from the restart.
    assert!(authority.tick(mono_ms() + 1_000).await.unwrap().is_empty());

    // Under RestartFromAcknowledged the restart does not lose the policy:
    // the attempt dwells from the restart and is then recovered.
    let dir = tempfile::tempdir().unwrap();
    let state = self::state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::RestartFromAcknowledged).await;
    acknowledged(&authority, job, token).await;
    authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    drop(authority);
    let authority = Authority::start(&state, NODE).unwrap();
    // The start scan runs on the thread before its first answer; the
    // dwell counts from then.
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    let at = mono_ms();
    authority.tick(at).await.unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.job.generation, Generation::FIRST, "the dwell runs again");
    authority.tick(at + UNAVAILABLE_AFTER_MS).await.unwrap();
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(
        s.attempts[0].status,
        AttemptStatus::Superseded { by: None },
        "recovered after the restart"
    );
    failover(&authority, job, recover_of(&s), NODE).await;
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.job.generation.get(), 2);
    assert_eq!(s.attempts[1].resource, ResourceRef::Node { node: NODE });
}

/// The recovery reasons recorded for `job`, in order.
async fn blocks(authority: &Authority, job: JobId) -> Vec<RecoveryBlock> {
    authority
        .with_journal(false, move |journal| {
            Ok(journal
                .events(job, None, 1_000)?
                .into_iter()
                .filter_map(|e| match e.frame.event {
                    ManagedEvent::RecoveryBlocked { reason, .. } => Some(reason),
                    _ => None,
                })
                .collect())
        })
        .await
        .unwrap()
}

/// Review finding on PR #39: under `restart_from_acknowledged` an open
/// cancel made the authority's `Recover` refused `EffectInFlight`; the
/// refusal was logged, stored under the recover's key, and the attempt
/// forgotten, so the job showed `unknown` with no reason and was never
/// judged again. The open cancel is now a recorded block, judged again
/// on the next tick, and the restart happens once the cancel has ended.
#[tokio::test]
async fn an_open_cancel_blocks_the_automatic_restart_with_its_reason_until_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::RestartFromAcknowledged).await;
    acknowledged(&authority, job, token).await;
    let cancel = authority
        .submit(Envelope {
            key: OperationKey::parse("cancel-1").unwrap(),
            job: Some(job),
            expected: None,
            body: Command::CancelJob {
                reason: "operator".into(),
            },
        })
        .await
        .unwrap();
    let StoredResult::Accepted(cancel) = &cancel.result else {
        panic!("the cancel was refused");
    };
    let cancel = cancel.operation;
    authority
        .mark_authority(job, cancel, OperationMark::Dispatched)
        .await
        .unwrap();
    let now = mono_ms();
    authority.tick(now + UNAVAILABLE_AFTER_MS).await.unwrap();
    authority
        .tick(now + 2 * UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    assert_eq!(
        blocks(&authority, job).await,
        vec![RecoveryBlock::EffectInFlight { operation: cancel }],
        "the reason is recorded"
    );
    // The stop never reached the runner and failed: judged again, the
    // attempt is recovered.
    authority
        .mark_authority(job, cancel, OperationMark::Failed)
        .await
        .unwrap();
    authority
        .tick(now + 2 * UNAVAILABLE_AFTER_MS + 2_000)
        .await
        .unwrap();
    assert_eq!(
        status(&authority, token).await,
        AttemptStatus::Superseded { by: None },
        "recovered once the cancel ended"
    );
}

/// Review finding on PR #39: a restarted runner presents the grants it
/// had taken after it stopped what was left of them, and the authority
/// took that presentation as presence, so an `unknown` attempt whose
/// harness the runner had just killed came back `running` and its
/// recovery waited a full silence more. A presentation is answered and
/// changes nothing; only a heartbeat from a runner that runs the grant
/// brings it back.
#[tokio::test]
async fn a_restarted_runner_s_presentation_is_answered_and_never_presence() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    let before = revision(&authority, job).await;
    let presented = Heartbeat {
        attempts: vec![token],
        running: false,
    };
    let answer = authority.heartbeat(RUNNER, presented).await.unwrap();
    assert_eq!(answer.verdicts.len(), 1);
    assert_ne!(answer.verdicts[0].1, FenceVerdict::Live, "{answer:?}");
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    assert_eq!(revision(&authority, job).await, before, "nothing recorded");
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(answer.verdicts, vec![(token.attempt, FenceVerdict::Live)]);
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
}

/// A covered write from the node an `unknown` attempt was granted to is
/// presence: the attempt is observed reachable first and the write is
/// taken, so a report that arrives before the healing heartbeat is not
/// lost. From any other node an `unknown` attempt stays not writable.
#[tokio::test]
async fn a_covered_write_from_the_granted_node_brings_an_unknown_attempt_back() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    let exited = Observation::Exited {
        attempt: token.attempt,
        generation: token.generation,
        outcome: fabric_core::managed::ExitOutcome::Succeeded,
    };
    let refused = authority
        .report(token, Some(OTHER), exited.clone())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refused,
        FenceVerdict::NotWritable {
            status: AttemptStatus::Unknown
        }
    );
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    let before = revision(&authority, job).await;
    authority
        .report(token, Some(RUNNER), exited)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Succeeded);
    assert_eq!(
        revision(&authority, job).await,
        before + 2,
        "reachable, then the exit"
    );

    // The same for a covered write other than a report.
    let dir = tempfile::tempdir().unwrap();
    let state = self::state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (_, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(status(&authority, token).await, AttemptStatus::Unknown);
    let written = authority
        .fenced_write(
            token,
            fabric_core::managed::CoveredPath::LedgerAppend,
            Some(RUNNER),
            || "kept",
        )
        .await
        .unwrap();
    assert_eq!(written, Ok("kept"));
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
}

/// A covered write made on the authority's own host (`from: None`) is
/// checked against the node the attempt was granted to, like one from
/// any peer: it is this node's write, and the attempt lives elsewhere.
#[tokio::test]
async fn a_write_made_on_this_node_is_checked_against_the_granted_node() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (_, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    let refused = authority
        .report(
            token,
            None,
            Observation::Exited {
                attempt: token.attempt,
                generation: token.generation,
                outcome: fabric_core::managed::ExitOutcome::Succeeded,
            },
        )
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refused,
        FenceVerdict::WrongNode {
            granted: RUNNER,
            from: NODE
        }
    );
    let refused = authority
        .fenced_write(
            token,
            fabric_core::managed::CoveredPath::LedgerAppend,
            None,
            || panic!("a refused write ran"),
        )
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(refused, FenceVerdict::WrongNode { .. }));
    assert_eq!(status(&authority, token).await, AttemptStatus::Running);
}

/// With no managed journal every covered path tells the runner the same
/// thing a heartbeat does, `UnknownJob`, rather than an internal fault,
/// and starts no thread for it.
#[tokio::test]
async fn with_no_journal_a_covered_write_is_unknown_job_and_starts_no_thread() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let token = GrantToken {
        job: JobId::from_bytes([3; 16]),
        attempt: fabric_core::managed::AttemptId::from_bytes([4; 16]),
        generation: Generation::FIRST,
    };
    let fenced = authority
        .fenced_write(
            token,
            fabric_core::managed::CoveredPath::LedgerAppend,
            Some(RUNNER),
            || panic!("a refused write ran"),
        )
        .await
        .unwrap();
    assert_eq!(fenced, Err(FenceVerdict::UnknownJob));
    let reported = authority
        .report(
            token,
            Some(RUNNER),
            Observation::Started {
                attempt: token.attempt,
                generation: token.generation,
            },
        )
        .await
        .unwrap();
    assert_eq!(reported.unwrap_err(), FenceVerdict::UnknownJob);
    let answer = authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    assert_eq!(
        answer.verdicts,
        vec![(token.attempt, FenceVerdict::UnknownJob)]
    );
    assert_eq!(authority.threads(), 0, "a refused write started the thread");
}

/// The write a fence guards runs on the authority's thread right after
/// the fence, before the next call is taken: a tick, and so a `Recover`,
/// cannot commit between the check and the write.
#[tokio::test]
async fn a_fenced_write_runs_inside_the_authority_s_turn() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (_, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let writer = tokio::spawn({
        let authority = authority.clone();
        let order = Arc::clone(&order);
        async move {
            authority
                .fenced_write(
                    token,
                    fabric_core::managed::CoveredPath::LedgerAppend,
                    Some(RUNNER),
                    move || {
                        gate.recv().unwrap();
                        order.lock().unwrap().push("write");
                        5
                    },
                )
                .await
        }
    });
    // The write holds the authority's turn; a tick queued behind it waits.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let ticker = tokio::spawn({
        let authority = authority.clone();
        let order = Arc::clone(&order);
        async move {
            let ticked = authority.tick(mono_ms()).await;
            order.lock().unwrap().push("tick");
            ticked
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!ticker.is_finished(), "a tick ran during a fenced write");
    release.send(()).unwrap();
    assert_eq!(writer.await.unwrap().unwrap(), Ok(5));
    ticker.await.unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), ["write", "tick"]);
}

/// Times the M6 authority paths that have no reference build (neither the
/// anchor nor the M5 head has a heartbeat, a fence or an acknowledgement):
/// 20 rounds of `PER_ROUND` calls each, after a warm-up, and the journal
/// bytes each path writes. `FABRIC_M6_BENCH_OUT` names the file the rows
/// go to.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a bench; run on demand with FABRIC_M6_BENCH_OUT set, --release --ignored"]
#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    reason = "one bench reads best as one function; nanoseconds fit an f64"
)]
async fn bench_m6_paths() {
    use std::time::Instant;

    const ROUNDS: usize = 20;
    const PER_ROUND: u32 = 200;

    fn summary(op: &str, mut rounds: Vec<f64>, bytes: u64, calls: u64) -> String {
        rounds.sort_by(f64::total_cmp);
        let median = f64::midpoint(rounds[rounds.len() / 2 - 1], rounds[rounds.len() / 2]);
        format!(
            "{op}\t{}\t{median:.0}\t{:.0}\t{:.0}\t{}\n",
            rounds.len(),
            rounds[0],
            rounds[rounds.len() - 1],
            bytes / calls.max(1),
        )
    }
    let journal_len = |state: &StateDir| std::fs::metadata(journal_path(state)).unwrap().len();

    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let (job, token) = running_on_runner(&authority, RecoveryPolicy::Block).await;
    let mut out = String::from("op\trounds\tmedian_ns\tmin_ns\tmax_ns\tjournal_bytes_per_call\n");

    // A heartbeat from the granted runner of a tracked, reachable attempt.
    for _ in 0..PER_ROUND {
        authority.heartbeat(RUNNER, beat(token)).await.unwrap();
    }
    let before = journal_len(&state);
    let mut rounds = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for _ in 0..PER_ROUND {
            authority.heartbeat(RUNNER, beat(token)).await.unwrap();
        }
        rounds.push(start.elapsed().as_nanos() as f64 / f64::from(PER_ROUND));
    }
    let calls = (ROUNDS as u64) * u64::from(PER_ROUND);
    out.push_str(&summary(
        "heartbeat live",
        rounds,
        journal_len(&state) - before,
        calls,
    ));

    // A covered write from the granted runner, run inside the authority's turn.
    let path = fabric_core::managed::CoveredPath::LedgerAppend;
    let before = journal_len(&state);
    let mut rounds = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for _ in 0..PER_ROUND {
            authority
                .fenced_write(token, path, Some(RUNNER), || ())
                .await
                .unwrap()
                .unwrap();
        }
        rounds.push(start.elapsed().as_nanos() as f64 / f64::from(PER_ROUND));
    }
    out.push_str(&summary(
        "fenced write",
        rounds,
        journal_len(&state) - before,
        calls,
    ));

    // A tick with nothing overdue.
    let before = journal_len(&state);
    let mut rounds = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for _ in 0..PER_ROUND {
            authority.tick(mono_ms()).await.unwrap();
        }
        rounds.push(start.elapsed().as_nanos() as f64 / f64::from(PER_ROUND));
    }
    out.push_str(&summary(
        "tick idle",
        rounds,
        journal_len(&state) - before,
        calls,
    ));

    // A checkpoint acknowledgement after the runner reported its capture;
    // only the acknowledgement is timed.
    let mut n = 0u64;
    let mut ack_rounds = Vec::new();
    let mut ack_bytes = 0;
    let acks_per_round = 20u32;
    for _ in 0..ROUNDS {
        let mut elapsed = std::time::Duration::ZERO;
        for _ in 0..acks_per_round {
            n += 1;
            let snapshot = SnapshotId::of(&n.to_le_bytes());
            let mut cut = [0u8; 32];
            cut[..8].copy_from_slice(&n.to_le_bytes());
            let cut = CutDigest::from_bytes(cut);
            authority
                .report(
                    token,
                    Some(RUNNER),
                    Observation::Captured {
                        attempt: token.attempt,
                        generation: token.generation,
                        snapshot,
                        cut,
                    },
                )
                .await
                .unwrap()
                .unwrap();
            let before = journal_len(&state);
            let start = Instant::now();
            authority
                .acknowledge(
                    job,
                    CheckpointAck {
                        attempt: token.attempt,
                        generation: token.generation,
                        snapshot,
                        cut,
                        manifests: 1,
                        chunks: 1,
                        bytes: 1,
                        fetched_from: RUNNER,
                    },
                )
                .await
                .unwrap();
            elapsed += start.elapsed();
            ack_bytes += journal_len(&state) - before;
        }
        ack_rounds.push(elapsed.as_nanos() as f64 / f64::from(acks_per_round));
    }
    let acks = (ROUNDS as u64) * u64::from(acks_per_round);
    out.push_str(&summary(
        "checkpoint acknowledge",
        ack_rounds,
        ack_bytes,
        acks,
    ));
    drop(authority);

    // The first heartbeat after a restart, when the attempt is untracked:
    // one journal read, and the heal when it is not reachable.
    let mut rounds = Vec::new();
    let mut heal_bytes = 0;
    for _ in 0..ROUNDS {
        let authority = Authority::start(&state, NODE).unwrap();
        authority.job(job).await.unwrap().unwrap();
        let before = journal_len(&state);
        let start = Instant::now();
        authority.heartbeat(RUNNER, beat(token)).await.unwrap();
        rounds.push(start.elapsed().as_nanos() as f64);
        heal_bytes += journal_len(&state) - before;
        drop(authority);
    }
    out.push_str(&summary(
        "heartbeat after restart",
        rounds,
        heal_bytes,
        ROUNDS as u64,
    ));

    eprint!("{out}");
    if let Ok(file) = std::env::var("FABRIC_M6_BENCH_OUT") {
        std::fs::write(file, out).unwrap();
    }
}

/// The last handle to drop joins the thread, so the journal is closed when
/// the drop returns: `SQLite` unlinks the WAL and shm it keeps beside an
/// open journal on the last close, and whatever looks at the state
/// directory after the drop (a restart, a backup, the legacy test above)
/// must not race that unlink. Red without the join: the side files are
/// still there when the drop returns.
#[tokio::test]
async fn the_last_handle_closes_the_journal_before_its_drop_returns() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    authority.submit(create("create-1")).await.unwrap();
    let mut wal = journal_path(&state).into_os_string();
    wal.push("-wal");
    assert!(Path::new(&wal).exists(), "the journal is not in WAL mode");
    drop(authority);
    let mut names: Vec<String> = std::fs::read_dir(managed_dir(&state))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [fabric_journal::FILE_NAME],
        "the journal was still open when the drop returned"
    );
}

/// A grant a derived command makes (a continue's new attempt) is tracked
/// like one `submit` makes: its silence is seen without a restart.
#[tokio::test]
async fn a_grant_made_by_a_derived_command_is_tracked_for_presence() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let authority = Authority::start(&state, NODE).unwrap();
    let made = authority
        .submit(create_with("create-1", RecoveryPolicy::Block))
        .await
        .unwrap();
    let job = job_of(&made);
    let key = OperationKey::parse("start-derived").unwrap();
    let answer = authority
        .submit_derived(
            key.clone(),
            super::request_digest(&[b"start"]),
            job,
            move |s: &ManagedState| {
                Ok::<_, ()>(Envelope {
                    key,
                    job: Some(job),
                    expected: None,
                    body: Command::StartAttempt {
                        task: s.job.primary,
                        resource: ResourceRef::Node { node: RUNNER },
                        initial_checkpoint: None,
                    },
                })
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(answer.result, StoredResult::Accepted(_)));
    let overdue = authority
        .tick(mono_ms() + UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    assert_eq!(overdue.len(), 1, "the derived grant is tracked");
    let s = authority.job(job).await.unwrap().unwrap();
    assert_eq!(s.attempts[0].status, AttemptStatus::Unknown);
}
