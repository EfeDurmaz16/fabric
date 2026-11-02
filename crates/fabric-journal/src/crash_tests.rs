//! Crash tests. A child re-executes this test binary, stops at a named
//! point inside journal creation, a command or a dispatch, and is killed
//! with `SIGKILL`; the parent then opens the journal as a restarted
//! authority does and checks what ADR 0020 promises: a change is all there
//! or not there, an answered key answers the same again, and an effect
//! whose dispatch was recorded is never asked for twice.

use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fabric_core::managed::{
    AttemptStatus, DomainError, OperationMark, OperationStatus, StoredResult,
};

use super::tests::{NODE, accepted, count, create, envelope, ids, start};
use super::*;

const EFFECT: &str = "effect";

/// The child side: runs the step named by the environment until the crash point stops it.
#[test]
fn crash_child() {
    let Ok(mode) = std::env::var("FABRIC_JOURNAL_CRASH_MODE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("FABRIC_JOURNAL_CRASH_DIR").unwrap());
    let path = dir.join(FILE_NAME);
    if mode == "create" {
        Journal::create(&path, NODE, 1).unwrap();
        panic!("the child did not reach its crash point");
    }
    let mut journal = Journal::open(&path).unwrap();
    match mode.as_str() {
        "submit-create" => {
            journal
                .submit(&envelope("create", None, create()), ids(1))
                .unwrap();
        }
        "submit-start" => {
            let job = ids(1).job;
            let primary = journal.job(job).unwrap().unwrap().job.primary;
            journal
                .submit(&envelope("start", Some(job), start(primary)), ids(2))
                .unwrap();
        }
        "dispatch" => {
            let job = ids(1).job;
            journal
                .mark(job, ids(2).operation, OperationMark::Dispatched, 3_000)
                .unwrap();
            // The effect: reached only when the dispatch is durable.
            std::fs::write(dir.join(EFFECT), "launched").unwrap();
        }
        other => panic!("unknown mode {other}"),
    }
    panic!("the child did not reach its crash point");
}

/// Runs the child until it reaches `point`, then kills it with `SIGKILL`.
fn run_child_and_kill(dir: &Path, mode: &str, point: &str) {
    let signal = dir.join(format!("signal-{point}"));
    let mut child = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_tests::crash_child", "--nocapture"])
        .env("FABRIC_JOURNAL_CRASH_MODE", mode)
        .env("FABRIC_JOURNAL_CRASH_DIR", dir)
        .env("FABRIC_JOURNAL_CRASH_AT", point)
        .env("FABRIC_JOURNAL_CRASH_SIGNAL", &signal)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !signal.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "{point}: the child exited with {status} before its crash point\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            started.elapsed() < Duration::from_mins(2),
            "{point}: the child did not reach its crash point in time"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    assert!(
        !child.wait().unwrap().success(),
        "{point}: the child was not killed"
    );
}

/// A journal with the job of `ids(1)`, and with the grant of `ids(2)` when `grant`.
fn journal_with_job(dir: &Path, grant: bool) -> Journal {
    let mut journal = Journal::create(&dir.join(FILE_NAME), NODE, 1).unwrap();
    journal
        .submit(&envelope("create", None, create()), ids(1))
        .unwrap();
    if grant {
        let job = ids(1).job;
        let primary = journal.job(job).unwrap().unwrap().job.primary;
        journal
            .submit(&envelope("start", Some(job), start(primary)), ids(2))
            .unwrap();
    }
    journal
}

#[test]
fn a_create_killed_before_its_rename_leaves_no_journal_and_create_can_finish() {
    let dir = tempfile::tempdir().unwrap();
    run_child_and_kill(dir.path(), "create", "create.before_rename");
    let path = dir.path().join(FILE_NAME);
    assert!(!path.exists());
    assert!(matches!(
        Journal::open(&path),
        Err(OpenError::NotAJournal(_))
    ));
    let journal = Journal::create(&path, NODE, 2).unwrap();
    journal.verify().unwrap();
    let mut tmp = path.into_os_string();
    tmp.push(".tmp");
    assert!(!Path::new(&tmp).exists());
}

#[test]
fn a_command_killed_before_its_commit_left_nothing_and_its_retry_is_new() {
    let dir = tempfile::tempdir().unwrap();
    drop(Journal::create(&dir.path().join(FILE_NAME), NODE, 1).unwrap());
    run_child_and_kill(dir.path(), "submit-create", "submit.before_commit");
    let mut journal = Journal::open(&dir.path().join(FILE_NAME)).unwrap();
    journal.verify().unwrap();
    assert!(journal.jobs().unwrap().is_empty());
    assert_eq!(count(&dir.path().join(FILE_NAME), "operations"), 0);
    let retry = journal
        .submit(&envelope("create", None, create()), ids(1))
        .unwrap();
    assert!(!retry.replayed);
    assert_eq!(journal.jobs().unwrap(), vec![ids(1).job]);
}

#[test]
fn a_command_killed_after_its_commit_answers_its_retry_with_the_same_identity() {
    let dir = tempfile::tempdir().unwrap();
    drop(Journal::create(&dir.path().join(FILE_NAME), NODE, 1).unwrap());
    run_child_and_kill(dir.path(), "submit-create", "submit.after_commit");
    let mut journal = Journal::open(&dir.path().join(FILE_NAME)).unwrap();
    journal.verify().unwrap();
    // The caller never saw the answer; it retries with the same key, and
    // the fresh ids the authority made for the retry are not used.
    let retry = journal
        .submit(&envelope("create", None, create()), ids(50))
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(accepted(&retry).job, ids(1).job);
    assert_eq!(accepted(&retry).operation, ids(1).operation);
    assert_eq!(journal.jobs().unwrap(), vec![ids(1).job]);
}

#[test]
fn a_grant_killed_between_its_commit_and_its_dispatch_is_dispatched_once() {
    let dir = tempfile::tempdir().unwrap();
    drop(journal_with_job(dir.path(), false));
    run_child_and_kill(dir.path(), "submit-start", "submit.after_commit");
    let mut journal = Journal::open(&dir.path().join(FILE_NAME)).unwrap();
    journal.verify().unwrap();
    let job = ids(1).job;
    let recovery = journal.recover(4_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(recovery.to_dispatch, vec![(job, ids(2).operation)]);
    assert!(recovery.unknown.is_empty());
    // The one dispatch: recorded, then the effect.
    journal
        .mark(job, ids(2).operation, OperationMark::Dispatched, 4_001)
        .unwrap();
    // Another restart: the dispatched grant becomes unknown and is not offered again.
    drop(journal);
    let mut journal = Journal::open(&dir.path().join(FILE_NAME)).unwrap();
    let recovery = journal.recover(5_000, |_| Liveness::CannotTell).unwrap();
    assert!(recovery.to_dispatch.is_empty());
    assert_eq!(recovery.unknown, vec![(job, ids(2).operation)]);
    journal.verify().unwrap();
}

#[test]
fn a_dispatch_killed_between_its_commit_and_its_effect_is_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    drop(journal_with_job(dir.path(), true));
    run_child_and_kill(dir.path(), "dispatch", "dispatch.after_commit");
    assert!(!dir.path().join(EFFECT).exists());
    let mut journal = Journal::open(&dir.path().join(FILE_NAME)).unwrap();
    journal.verify().unwrap();
    let job = ids(1).job;
    let recovery = journal.recover(4_000, |_| Liveness::CannotTell).unwrap();
    assert_eq!(recovery.unknown, vec![(job, ids(2).operation)]);
    assert!(recovery.to_dispatch.is_empty());
    let state = journal.job(job).unwrap().unwrap();
    assert_eq!(
        state.operation(ids(2).operation).unwrap().status,
        OperationStatus::Unknown
    );
    // The attempt may have started; nothing replaces it.
    assert_eq!(state.attempts[0].status, AttemptStatus::Granted);
    let refused = journal
        .submit(
            &envelope("start-again", Some(job), start(state.job.primary)),
            ids(3),
        )
        .unwrap();
    assert!(matches!(
        refused.result,
        StoredResult::Rejected(DomainError::AttemptStillLive { .. })
    ));
    assert!(matches!(
        journal.mark(job, ids(2).operation, OperationMark::Dispatched, 4_001),
        Err(CommitError::Domain(DomainError::OperationState { .. }))
    ));
}
