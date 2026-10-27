//! The bridge client against a real worker process: the Rust test double
//! (`src/bin/double.rs`) over stdio, recording into a real journal. Each
//! test names the failure it pins: a crash mid-request, a deadline, a
//! frame over the cap, a broken handshake, a refusal before effect.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

use common::{create, journal, operation, scope};
use fabric_bridge::wire::{
    Action, CancelState, CreateParams, ErrorCode, Locator, ResultState, Versions,
};
use fabric_bridge::{Answer, Bridge, BridgeError, Config, Control, Effect, Event, LogError};
use fabric_core::managed::OperationId;
use fabric_journal::{Journal, ProviderStatus};
use tokio::process::Command;

fn double() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fabric-bridge-double"))
}

fn config() -> Config {
    let mut config = Config::new("fixture");
    config.grace = Duration::from_millis(200);
    config.kill_grace = Duration::from_millis(500);
    config.handshake_timeout = Duration::from_secs(5);
    config
}

async fn spawn() -> Bridge {
    Bridge::spawn(double(), config()).await.unwrap()
}

fn status(journal: &Journal, n: u8) -> Option<ProviderStatus> {
    journal
        .provider_operation(OperationId::from_bytes([n; 16]))
        .unwrap()
        .map(|op| op.status)
}

fn fx(key: &str) -> Locator {
    Locator::from_value(&serde_json::json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": "proj-1"},
        "kind": "sandbox",
        "native_id": format!("fx-{key}"),
    }))
    .unwrap()
}

#[tokio::test]
async fn a_create_is_recorded_dispatched_then_succeeded_with_its_locator() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    assert_eq!(bridge.protocol(), 1);
    assert_eq!(bridge.profile().profile, "fixture");
    assert_eq!(bridge.limits().max_in_flight, 4, "the worker lowered it");
    let flight = bridge
        .request(&mut journal, create(1, "fixture/ok"))
        .await
        .unwrap();
    // Recorded before the frame was written, so before any answer.
    assert_eq!(status(&journal, 1), Some(ProviderStatus::Dispatched));
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Succeeded);
    let op = journal
        .provider_operation(OperationId::from_bytes([1; 16]))
        .unwrap()
        .unwrap();
    assert_eq!(
        op.locator.as_deref(),
        Some(fx("key-1").to_value().to_string().as_str())
    );
    let exit = bridge.close().await.unwrap();
    assert_eq!(exit.code(), Some(0), "closing stdin is a clean shutdown");
}

/// Denied before effect: the operation is rejected, never failed or unknown.
#[tokio::test]
async fn a_denied_request_records_rejected_and_no_effect() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let flight = bridge
        .request(&mut journal, create(2, "fixture/forbidden"))
        .await
        .unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Rejected);
    let Answer::Error(e) = outcome.answer else {
        panic!("{:?}", outcome.answer)
    };
    assert_eq!(e.error.code, ErrorCode::Denied);
    let op = journal
        .provider_operation(OperationId::from_bytes([2; 16]))
        .unwrap()
        .unwrap();
    assert_eq!(op.answer.effect.as_deref(), Some("none"));
    assert_eq!(op.locator, None);
}

/// Worker loss is not cancellation: the operation in flight is unknown,
/// the bridge refuses what comes next without touching the journal, and
/// the job's identity is unchanged.
#[tokio::test]
async fn a_worker_crash_mid_request_leaves_the_operation_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let flight = bridge
        .request(&mut journal, create(3, "double/crash"))
        .await
        .unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert!(
        matches!(outcome.answer, Answer::Lost(_)),
        "{:?}",
        outcome.answer
    );
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    assert_eq!(status(&journal, 3), Some(ProviderStatus::Unknown));
    assert!(bridge.lost().is_some());
    let next = bridge.request(&mut journal, create(4, "fixture/ok")).await;
    assert!(matches!(next, Err(BridgeError::Lost(_))), "{next:?}");
    assert_eq!(
        status(&journal, 4),
        None,
        "nothing recorded for a lost worker"
    );
    let state = journal.job(scope().job).unwrap().unwrap();
    assert_eq!(state.job.generation, scope().generation);
    let exit = bridge.close().await.unwrap();
    assert_eq!(exit.code(), Some(3));
}

/// A locator reported before the crash is kept with the unknown outcome:
/// it is what a later observe or a cleanup needs.
#[tokio::test]
async fn a_locator_reported_before_a_crash_is_kept_with_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let mut flight = bridge
        .request(&mut journal, create(5, "double/progress-crash"))
        .await
        .unwrap();
    let event = flight.next(&mut journal).await.unwrap();
    assert_eq!(event, Event::Progress(fx("key-5")));
    let Event::Done(outcome) = flight.next(&mut journal).await.unwrap() else {
        panic!("a second progress")
    };
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    let op = journal
        .provider_operation(OperationId::from_bytes([5; 16]))
        .unwrap()
        .unwrap();
    assert_eq!(
        op.locator.as_deref(),
        Some(fx("key-5").to_value().to_string().as_str())
    );
}

/// A timeout is not permission to replay: the operation is unknown, and
/// the same operation cannot be sent again.
#[tokio::test]
async fn a_timeout_leaves_unknown_and_the_operation_is_never_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let mut effect = create(6, "double/silent");
    effect.deadline_ms = 100;
    let started = Instant::now();
    let flight = bridge.request(&mut journal, effect.clone()).await.unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert_eq!(outcome.answer, Answer::TimedOut);
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "deadline plus grace"
    );
    let again = bridge.request(&mut journal, effect).await;
    assert!(
        matches!(again, Err(BridgeError::Log(LogError::Used(_)))),
        "{again:?}"
    );
    assert_eq!(status(&journal, 6), Some(ProviderStatus::Unknown));
    // The worker is still there for other operations.
    let ok = bridge
        .request(&mut journal, create(7, "fixture/ok"))
        .await
        .unwrap();
    assert_eq!(
        ok.finish(&mut journal).await.unwrap().status,
        ProviderStatus::Succeeded
    );
    // A key the journal holds is refused before the frame: had it been
    // written, the worker would have replayed its answer to no one.
    let again = bridge.request(&mut journal, create(7, "fixture/ok")).await;
    assert!(
        matches!(again, Err(BridgeError::Log(LogError::Used(_)))),
        "{again:?}"
    );
    let next = bridge
        .request(&mut journal, create(15, "fixture/ok"))
        .await
        .unwrap();
    assert_eq!(
        next.finish(&mut journal).await.unwrap().status,
        ProviderStatus::Succeeded
    );
    assert_eq!(
        bridge.late_answers(),
        0,
        "a refused request reached the worker"
    );
}

/// A frame over the cap is refused from its length; the worker is
/// terminated and what it had in flight is unknown.
#[tokio::test]
async fn a_worker_frame_over_the_cap_is_refused_unread_and_the_worker_terminated() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let flight = bridge
        .request(&mut journal, create(8, "double/oversize"))
        .await
        .unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    let Answer::Lost(why) = &outcome.answer else {
        panic!("{:?}", outcome.answer)
    };
    assert!(why.contains("oversize"), "{why}");
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    let exit = bridge.close().await.unwrap();
    assert_eq!(exit.signal(), Some(15), "SIGTERM: {exit:?}");
}

/// Terminal-only stays terminal-only: a running result for a create
/// breaks the protocol, so the worker is ended and the create is unknown.
#[tokio::test]
async fn a_terminal_only_action_answered_before_terminal_is_unknown_and_ends_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let flight = bridge
        .request(&mut journal, create(16, "double/running"))
        .await
        .unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    assert!(bridge.lost().unwrap().contains("does not answer it"));
    assert!(bridge.close().await.is_some());
}

#[tokio::test]
async fn an_answer_under_another_operation_breaks_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let flight = bridge
        .request(&mut journal, create(9, "double/other-operation"))
        .await
        .unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    assert!(bridge.lost().unwrap().contains("another operation"));
}

/// Refusals before effect that Fabric can make itself: an action the
/// profile does not offer, a deadline over its limit, a frame over the
/// request cap. Each is recorded rejected and nothing is written.
#[tokio::test]
async fn fabric_refuses_before_sending_what_the_profile_or_the_cap_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let mut other = create(10, "fixture/ok");
    other.action = Action::Other {
        name: "sandbox.snapshot".into(),
        target: Some(fx("key-1")),
        params: serde_json::Map::new(),
    };
    let mut late = create(11, "fixture/ok");
    late.deadline_ms = 600_001;
    let mut huge = create(12, "fixture/ok");
    huge.action = Action::Create(CreateParams {
        image: "fixture/ok".into(),
        labels: (0..300)
            .map(|i| (format!("l{i}"), "x".repeat(4000)))
            .collect(),
        ..CreateParams::default()
    });
    for (n, effect, reason) in [
        (10, other, ("unsupported", "action")),
        (11, late, ("invalid", "limit")),
        (12, huge, ("invalid", "limit")),
    ] {
        match bridge.request(&mut journal, effect).await {
            Err(BridgeError::RefusedLocally(r)) => assert_eq!((r.code, r.reason), reason),
            other => panic!("{n}: {other:?}"),
        }
        assert_eq!(status(&journal, n), Some(ProviderStatus::Rejected), "{n}");
    }
    // Nothing reached the worker: it would have answered each refused
    // request before this one, to no one.
    let ok = bridge
        .request(&mut journal, create(13, "fixture/ok"))
        .await
        .unwrap();
    assert_eq!(
        ok.finish(&mut journal).await.unwrap().status,
        ProviderStatus::Succeeded
    );
    assert_eq!(
        bridge.late_answers(),
        0,
        "a refused request reached the worker"
    );
}

/// Fabric owns fencing: an effect for an attempt that is not the job's
/// current live grant is refused before anything is recorded or sent.
#[tokio::test]
async fn a_stale_generation_is_fenced_before_anything_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let mut stale = create(14, "fixture/ok");
    stale.scope.generation = fabric_core::managed::Generation::new(2).unwrap();
    let result = bridge.request(&mut journal, stale).await;
    assert!(
        matches!(result, Err(BridgeError::Log(LogError::Fenced(_)))),
        "{result:?}"
    );
    assert_eq!(status(&journal, 14), None);
}

#[tokio::test]
async fn the_in_flight_limit_refuses_before_the_journal_and_shutdown_lists_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let mut flights = Vec::new();
    for n in 20..24 {
        flights.push(
            bridge
                .request(&mut journal, create(n, "double/silent"))
                .await
                .unwrap(),
        );
    }
    let busy = bridge.request(&mut journal, create(24, "fixture/ok")).await;
    assert!(matches!(busy, Err(BridgeError::Busy(4))), "{busy:?}");
    assert_eq!(status(&journal, 24), None, "a busy refusal records nothing");
    // A create does not cancel in the fixture profile; a request that is
    // not outstanding is not found. Neither writes a frame.
    match bridge.cancel(flights[0].request_id()).await.unwrap() {
        Control::Local(state) => assert_eq!(state, CancelState::Unsupported),
        Control::Sent(_) => panic!("sent a cancel the profile does not take"),
    }
    match bridge.cancel("r-404").await.unwrap() {
        Control::Local(state) => assert_eq!(state, CancelState::NotFound),
        Control::Sent(_) => panic!("sent a cancel of nothing"),
    }
    let unresolved = bridge.shutdown(1000).await.unwrap().wait().await.unwrap();
    assert_eq!(unresolved.len(), 4);
    let refused = bridge.request(&mut journal, create(25, "fixture/ok")).await;
    assert!(
        matches!(refused, Err(BridgeError::ShuttingDown)),
        "{refused:?}"
    );
    for flight in flights {
        let outcome = flight.finish(&mut journal).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Unknown);
    }
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}

#[tokio::test]
async fn an_observe_of_an_unknown_operation_settles_it_and_a_refused_one_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let flight = bridge
        .request(&mut journal, create(30, "double/progress-crash"))
        .await
        .unwrap();
    let _ = flight.finish(&mut journal).await.unwrap();
    assert_eq!(status(&journal, 30), Some(ProviderStatus::Unknown));
    // Certainty returns only by observe, on a new worker, with the original id.
    let bridge = spawn().await;
    let refused = bridge
        .observe(
            &mut journal,
            OperationId::from_bytes([30; 16]),
            "sandbox.exec",
            fx("key-30"),
            1000,
        )
        .await;
    assert!(
        matches!(refused, Err(BridgeError::RefusedLocally(_))),
        "{refused:?}"
    );
    assert_eq!(status(&journal, 30), Some(ProviderStatus::Unknown));
    let flight = bridge
        .observe(
            &mut journal,
            OperationId::from_bytes([30; 16]),
            "sandbox.create",
            fx("key-30"),
            1000,
        )
        .await
        .unwrap();
    let outcome = flight.finish(&mut journal).await.unwrap();
    let Answer::Result(r) = &outcome.answer else {
        panic!("{:?}", outcome.answer)
    };
    assert_eq!(r.state, ResultState::Completed);
    assert_eq!(outcome.status, ProviderStatus::Succeeded);
    let none = bridge
        .observe(
            &mut journal,
            OperationId::from_bytes([31; 16]),
            "sandbox.create",
            fx("key-31"),
            1000,
        )
        .await;
    assert!(
        matches!(none, Err(BridgeError::NotObservable(_))),
        "{none:?}"
    );
}

#[tokio::test]
async fn a_handshake_that_raises_a_limit_or_names_another_profile_is_refused() {
    for (script, reason) in [
        ("raise_limit", "limit_raised"),
        ("wrong_profile", "profile_mismatch"),
    ] {
        let mut command = double();
        command.env("FABRIC_BRIDGE_DOUBLE_HELLO", script);
        match Bridge::spawn(command, config()).await {
            Err(BridgeError::Violation(r)) => assert_eq!(r.reason, reason, "{script}"),
            other => panic!("{script}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_handshake_answer_over_1_mib_is_refused_from_its_length() {
    let mut command = double();
    command.env("FABRIC_BRIDGE_DOUBLE_HELLO", "oversize");
    match Bridge::spawn(command, config()).await {
        Err(BridgeError::Frame(fault)) => assert_eq!(fault.code(), "oversize"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_worker_that_does_not_answer_hello_or_exits_is_not_a_bridge() {
    let mut command = double();
    command.env("FABRIC_BRIDGE_DOUBLE_HELLO", "silent");
    let mut quick = config();
    quick.handshake_timeout = Duration::from_millis(300);
    let result = Bridge::spawn(command, quick).await;
    assert!(
        matches!(result, Err(BridgeError::HandshakeTimeout)),
        "{result:?}"
    );
    let mut command = double();
    command.env("FABRIC_BRIDGE_DOUBLE_HELLO", "exit");
    let result = Bridge::spawn(command, config()).await;
    assert!(matches!(result, Err(BridgeError::Lost(_))), "{result:?}");
}

#[tokio::test]
async fn a_worker_without_a_common_version_refuses_and_says_which_it_speaks() {
    let mut future = config();
    future.versions = Versions { min: 2, max: 3 };
    match Bridge::spawn(double(), future).await {
        Err(BridgeError::Refused { versions, code, .. }) => {
            assert_eq!(versions, Versions::V1);
            assert_eq!(code, "unsupported_version");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn exec_and_destroy_round_trip_with_their_targets() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let bridge = spawn().await;
    let created = bridge
        .request(&mut journal, create(40, "fixture/ok"))
        .await
        .unwrap();
    created.finish(&mut journal).await.unwrap();
    let exec = Effect {
        operation: operation(41),
        scope: scope(),
        action: Action::Exec {
            target: fx("key-40"),
            params: fabric_bridge::wire::ExecParams {
                argv: vec!["echo".into(), "hi".into()],
                stdin: None,
                cwd: None,
                timeout_ms: 1000,
                max_output_bytes: 1024,
                secret_names: Vec::new(),
            },
        },
        approval: None,
        policy: None,
        deadline_ms: 5000,
    };
    let outcome = bridge
        .request(&mut journal, exec)
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    let Answer::Result(r) = &outcome.answer else {
        panic!("{:?}", outcome.answer)
    };
    assert_eq!(r.output["stdout"]["text"], "echo hi\n");
    let destroy = |n: u8, approval: Option<&str>| Effect {
        operation: operation(n),
        scope: scope(),
        action: Action::Destroy {
            target: fx("key-40"),
        },
        approval: approval.map(str::to_owned),
        policy: None,
        deadline_ms: 5000,
    };
    let refused = bridge
        .request(&mut journal, destroy(42, None))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(refused.status, ProviderStatus::Rejected);
    let done = bridge
        .request(&mut journal, destroy(43, Some("apr-destroy-1")))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(done.status, ProviderStatus::Succeeded);
}
