//! The authority in the daemon: the journal is made by the first managed
//! command, a restart keeps identity and progress, a managed command never
//! touches a standalone job, and a state directory from main before
//! managed jobs is byte-identical after a managed job is made beside it.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::{
    Command, DomainError, Envelope, Intent, JobPolicy, OperationKey, OperationMark,
    OperationStatus, ResourceRef, RestartPolicy, SourceBinding, StoredResult,
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
            },
            source: SourceBinding::Repository {
                path: AbsPath::parse("/src/repo").unwrap(),
            },
            policy: JobPolicy {
                restart: RestartPolicy::ExplicitOnly,
            },
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
        .mark(job, grant.operation, OperationMark::Dispatched)
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
        .observe(
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
