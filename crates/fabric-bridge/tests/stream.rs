//! The client's writes against an in-process worker: a caller that gives
//! up on a request whose frame does not fit the pipe leaves no part of a
//! frame behind, so the worker reads every frame whole and in order, a
//! request admitted before a shutdown is never written after it, and one
//! made after it is refused as shutting down before anything else.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use common::{Logged, create, journal, operation};
use fabric_bridge::wire::{Action, CreateParams, ExecParams, Locator, Scope};
use fabric_bridge::{
    Bridge, BridgeError, Config, Effect, Entry, LogError, LogThread, MemoryLog, OperationLog,
    frames,
};
use fabric_core::managed::{Generation, OperationId};
use fabric_journal::{
    CleanupRefusal, Journal, LocatorRecord, ProviderAnswer, ProviderDispatch, ProviderStatus,
};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

const PROFILE: &str = include_str!("../../../protocol/bridge/v1/profiles/fixture.json");

fn ack(hello: &Value) -> Value {
    json!({
        "type": "hello_ack",
        "protocol": 1,
        "worker": "stream-test/0",
        "profile": serde_json::from_str::<Value>(PROFILE).unwrap(),
        "limits": hello["limits"].clone(),
    })
}

fn config() -> Config {
    let mut config = Config::new("fixture");
    config.grace = Duration::from_millis(200);
    config.kill_grace = Duration::from_millis(300);
    config
}

/// The caller wraps a request in a timeout and gives up while the worker
/// is not reading, as a node does around a slow call. Every frame still
/// reaches the worker whole: the first request's, then the next one's,
/// which the worker answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_that_gives_up_leaves_no_partial_frame() {
    // A 4 KiB pipe: the first request's frame is about 22 KiB.
    let (fabric, worker) = tokio::io::duplex(4096);
    let (from_worker, to_worker) = tokio::io::split(fabric);
    let (mut reader, mut writer) = tokio::io::split(worker);
    let (go, wait) = tokio::sync::oneshot::channel::<()>();
    let script = tokio::spawn(async move {
        let hello = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        let frame = frames::encode(&ack(&hello), 1 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        // Busy for a moment: not reading.
        wait.await.unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let read =
                tokio::time::timeout(Duration::from_secs(5), frames::read(&mut reader, 1 << 20))
                    .await
                    .expect("a frame never arrived whole");
            let request = read.unwrap().unwrap();
            assert_eq!(request["type"], "request", "{request}");
            ids.push(request["request_id"].as_str().unwrap().to_owned());
            if ids.len() == 2 {
                let answer = json!({
                    "type": "result",
                    "request_id": request["request_id"],
                    "operation_id": request["operation"]["id"],
                    "state": "completed",
                    "resource": {
                        "provider": "fixture",
                        "scope": {"account": "acct-a", "project": "proj-1"},
                        "kind": "sandbox",
                        "native_id": "fx-key-2",
                    },
                    "output": {"status": "ready"},
                    "replayed": false,
                    "receipts": [],
                });
                let frame = frames::encode(&answer, 16 << 20).unwrap();
                frames::write(&mut writer, &frame).await.unwrap();
            }
        }
        (ids, reader, writer)
    });
    let bridge = Bridge::connect(from_worker, Box::new(to_worker), config())
        .await
        .unwrap();
    let mut log = MemoryLog::new();
    let mut big = create(1, "fixture/ok");
    let Action::Create(params) = &mut big.action else {
        panic!("a create")
    };
    for i in 0..200 {
        params.labels.insert(format!("l{i}"), "x".repeat(100));
    }
    let first =
        tokio::time::timeout(Duration::from_millis(200), bridge.request(&mut log, big)).await;
    // Whatever the caller got, it stops waiting on it.
    drop(first);
    go.send(()).unwrap();
    let second = bridge
        .request(&mut log, create(2, "fixture/ok"))
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), second.finish(&mut log))
        .await
        .expect("the next request was never answered")
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Succeeded);
    let (ids, _reader, _writer) = script.await.unwrap();
    assert_eq!(ids, ["r-1", "r-2"]);
    assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
}

/// A log whose first `begin` says it started, then waits for the test to
/// let it through: a request held on the log's thread between its
/// admission and its frame, as behind other journal calls and their
/// fsyncs.
struct Gated {
    log: MemoryLog,
    started: Option<oneshot::Sender<()>>,
    open: std_mpsc::Receiver<()>,
}

impl OperationLog for Gated {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        if let Some(started) = self.started.take() {
            started.send(()).unwrap();
            self.open.recv().unwrap();
        }
        self.log.begin(dispatch, refusal)
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&LocatorRecord>,
    ) -> Result<(), LogError> {
        self.log.record(operation, status, answer, locator)
    }

    fn searched(
        &mut self,
        operation: OperationId,
        answer: &ProviderAnswer,
        searched: &str,
    ) -> Result<(), LogError> {
        self.log.searched(operation, answer, searched)
    }

    fn locate(&mut self, operation: OperationId, locator: &LocatorRecord) -> Result<(), LogError> {
        self.log.locate(operation, locator)
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        self.log.entry(operation)
    }

    fn cleanup_target(
        &self,
        creating: OperationId,
    ) -> Result<Result<(Scope, LocatorRecord), CleanupRefusal>, LogError> {
        self.log.cleanup_target(creating)
    }
}

/// A request is admitted, then held on the log's thread while a shutdown
/// is sent and acknowledged by a worker that, with nothing in flight,
/// closes its output. Written now, behind the shutdown, the request would
/// never be read by the worker yet settle `unknown`, and a create with no
/// locator cannot be observed, only attached. It is recorded `rejected`
/// as a draining worker answers a request read after a shutdown, and the
/// worker reads only the shutdown. The order is the same on every run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_admitted_before_a_shutdown_is_never_written_after_it() {
    let (fabric, worker) = tokio::io::duplex(64 << 10);
    let (from_worker, to_worker) = tokio::io::split(fabric);
    let (mut reader, mut writer) = tokio::io::split(worker);
    let script = tokio::spawn(async move {
        let hello = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        let frame = frames::encode(&ack(&hello), 1 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        // Every frame until Fabric closes the worker's input.
        let mut read = Vec::new();
        while let Some(frame) = frames::read(&mut reader, 1 << 20).await.unwrap() {
            let (kind, id) = (&frame["type"], &frame["request_id"]);
            read.push(format!(
                "{}:{}",
                kind.as_str().unwrap(),
                id.as_str().unwrap()
            ));
            if kind == "shutdown" {
                let ack = json!({"type": "shutdown_ack", "request_id": id, "unresolved": []});
                let frame = frames::encode(&ack, 1 << 20).unwrap();
                frames::write(&mut writer, &frame).await.unwrap();
                writer.shutdown().await.unwrap();
            }
        }
        read
    });
    let bridge = Bridge::connect(from_worker, Box::new(to_worker), config())
        .await
        .unwrap();
    let (started, held) = oneshot::channel();
    let (open, gate) = std_mpsc::channel();
    let mut log = LogThread::spawn(Gated {
        log: MemoryLog::new(),
        started: Some(started),
        open: gate,
    })
    .unwrap();
    let request = bridge.request(&mut log, create(1, "fixture/ok"));
    let shutdown = async {
        held.await.unwrap();
        let unresolved = bridge.shutdown(1000).await.unwrap().wait().await.unwrap();
        open.send(()).unwrap();
        unresolved
    };
    let (result, unresolved) = tokio::join!(request, shutdown);
    assert!(
        matches!(
            &result,
            Err(BridgeError::RefusedLocally(r)) if (r.code, r.reason) == ("unavailable", "shutting_down")
        ),
        "{result:?}"
    );
    assert_eq!(unresolved, []);
    let id = operation(1).id;
    let entry = log.call(move |log| log.entry(id)).await.unwrap().unwrap();
    assert_eq!(entry.map(|e| e.status), Some(ProviderStatus::Rejected));
    assert_eq!(bridge.close().await, None);
    assert_eq!(script.await.unwrap(), ["shutdown:r-2"]);
}

fn fx(key: &str) -> fabric_bridge::wire::Locator {
    fabric_bridge::wire::Locator::from_value(&json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": "proj-1"},
        "kind": "sandbox",
        "native_id": format!("fx-{key}"),
    }))
    .unwrap()
}

/// An error frame answering `request_id` for `operation_id`.
fn error(
    request_id: &Value,
    operation_id: &Value,
    code: &str,
    reason: &str,
    effect: &str,
) -> Value {
    json!({
        "type": "error",
        "request_id": request_id,
        "operation_id": operation_id,
        "error": {
            "code": code,
            "reason": reason,
            "message": format!("{code}: {reason}"),
            "effect": effect,
        },
    })
}

/// A worker reads an exec, and before it has sent the exec anywhere
/// Fabric observes it: the worker knows nothing of it yet and answers
/// `unknown`/`observation_failed`. A read that learned nothing changes
/// nothing, so the exec stays `dispatched` and the request's own answer,
/// a refusal before any effect, settles it `rejected`. Were the observe
/// to move it `unknown`, the refusal could not be recorded and the exec
/// would block the job's replacement although nothing ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_observe_that_learned_nothing_leaves_a_dispatched_request_to_its_answer() {
    let (fabric, worker) = tokio::io::duplex(64 << 10);
    let (from_worker, to_worker) = tokio::io::split(fabric);
    let (mut reader, mut writer) = tokio::io::split(worker);
    let script = tokio::spawn(async move {
        let hello = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        let frame = frames::encode(&ack(&hello), 1 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        let request = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        assert_eq!(request["type"], "request", "{request}");
        let observe = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        assert_eq!(observe["type"], "observe", "{observe}");
        let mut answer = error(
            &observe["request_id"],
            &observe["operation_id"],
            "unknown",
            "observation_failed",
            "unknown",
        );
        answer["error"]["resource"] = observe["resource"].clone();
        let frame = frames::encode(&answer, 16 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        let refusal = error(
            &request["request_id"],
            &request["operation"]["id"],
            "denied",
            "policy_denied",
            "none",
        );
        let frame = frames::encode(&refusal, 16 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        (reader, writer)
    });
    let bridge = Bridge::connect(from_worker, Box::new(to_worker), config())
        .await
        .unwrap();
    let mut log = MemoryLog::new();
    let target = fx("key-9");
    let exec = fabric_bridge::Effect {
        operation: operation(1),
        scope: common::scope(),
        action: Action::Exec {
            target: target.clone(),
            params: fabric_bridge::wire::ExecParams {
                argv: vec!["true".into()],
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
    let running = bridge.request(&mut log, exec).await.unwrap();
    let observed = bridge
        .observe(
            &mut log,
            OperationId::from_bytes([1; 16]),
            "sandbox.exec",
            target,
            1000,
        )
        .await
        .unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(5), observed.finish(&mut log))
        .await
        .expect("the observe was never answered")
        .unwrap();
    assert_eq!(observed.status, ProviderStatus::Dispatched);
    let outcome = tokio::time::timeout(Duration::from_secs(5), running.finish(&mut log))
        .await
        .expect("the request was never answered");
    let outcome = outcome.unwrap_or_else(|e| panic!("the refusal was not recorded: {e}"));
    assert_eq!(outcome.status, ProviderStatus::Rejected);
    let entry = log
        .entry(OperationId::from_bytes([1; 16]))
        .unwrap()
        .unwrap();
    assert_eq!(entry.status, ProviderStatus::Rejected);
    let _streams = script.await.unwrap();
    assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
}

/// A caller drops a create before its answer, and the worker is then
/// lost with it in flight. Nothing waits on it, so no answer or loss can
/// settle it; the next call handed the log records it `unknown`, as the
/// protocol's loss rule says of every dispatched operation of a lost
/// worker, instead of leaving it `dispatched` until a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_whose_caller_stopped_waiting_is_unknown_once_the_worker_is_lost() {
    let (fabric, worker) = tokio::io::duplex(64 << 10);
    let (from_worker, to_worker) = tokio::io::split(fabric);
    let (mut reader, mut writer) = tokio::io::split(worker);
    let (read, was_read) = oneshot::channel::<()>();
    let script = tokio::spawn(async move {
        let hello = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        let frame = frames::encode(&ack(&hello), 1 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        let request = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        assert_eq!(request["type"], "request", "{request}");
        read.send(()).unwrap();
        // The worker ends with the request in flight: its output closes.
        writer.shutdown().await.unwrap();
        drop(writer);
        reader
    });
    let bridge = Bridge::connect(from_worker, Box::new(to_worker), config())
        .await
        .unwrap();
    let mut log = MemoryLog::new();
    let dropped = bridge
        .request(&mut log, create(1, "fixture/ok"))
        .await
        .unwrap();
    drop(dropped);
    was_read.await.unwrap();
    let started = std::time::Instant::now();
    while bridge.lost().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the worker was never lost"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bridge.record_late(&mut log).await.unwrap();
    let entry = log
        .entry(OperationId::from_bytes([1; 16]))
        .unwrap()
        .unwrap();
    assert_eq!(entry.status, ProviderStatus::Unknown);
    assert_eq!(entry.locator, None);
    let _reader = script.await.unwrap();
}

/// A worker that answers the handshake, then reads every frame until its
/// input closes and answers none of them but a `shutdown`. That one it
/// answers once the test lets it exit: an empty `shutdown_ack`, then the
/// end of its output, as a worker that exits.
struct Draining {
    exit: Option<oneshot::Sender<()>>,
    script: tokio::task::JoinHandle<Vec<String>>,
}

impl Draining {
    /// The worker, and a bridge connected to it under `config`.
    async fn start(config: Config) -> (Draining, Bridge) {
        let (fabric, worker) = tokio::io::duplex(64 << 10);
        let (from_worker, to_worker) = tokio::io::split(fabric);
        let (mut reader, mut writer) = tokio::io::split(worker);
        let (exit, exited) = oneshot::channel::<()>();
        let script = tokio::spawn(async move {
            let hello = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
            let frame = frames::encode(&ack(&hello), 1 << 20).unwrap();
            frames::write(&mut writer, &frame).await.unwrap();
            let mut exited = Some(exited);
            let mut read = Vec::new();
            while let Some(frame) = frames::read(&mut reader, 1 << 20).await.unwrap() {
                read.push(frame["type"].as_str().unwrap().to_owned());
                if frame["type"] == "shutdown" {
                    // A test that failed before it let the worker exit.
                    if let Some(exited) = exited.take()
                        && exited.await.is_err()
                    {
                        return read;
                    }
                    let ack = json!({
                        "type": "shutdown_ack",
                        "request_id": frame["request_id"],
                        "unresolved": [],
                    });
                    let frame = frames::encode(&ack, 1 << 20).unwrap();
                    frames::write(&mut writer, &frame).await.unwrap();
                    writer.shutdown().await.unwrap();
                }
            }
            read
        });
        let bridge = Bridge::connect(from_worker, Box::new(to_worker), config)
            .await
            .unwrap();
        let draining = Draining {
            exit: Some(exit),
            script,
        };
        (draining, bridge)
    }

    /// Lets the worker answer the shutdown and exit, and waits until the
    /// bridge has read the end of its output.
    async fn exit(&mut self, bridge: &Bridge) {
        if let Some(exit) = self.exit.take() {
            exit.send(()).unwrap();
        }
        let started = std::time::Instant::now();
        while bridge.lost().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the worker never exited"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The type of every frame the worker read after the handshake, once
    /// Fabric closed its input.
    async fn read(self, bridge: Bridge) -> Vec<String> {
        assert_eq!(bridge.close().await, None);
        self.script.await.unwrap()
    }
}

fn status(journal: &Journal, n: u8) -> Option<ProviderStatus> {
    journal
        .provider_operation(OperationId::from_bytes([n; 16]))
        .unwrap()
        .map(|op| op.status)
}

/// A `sandbox.snapshot`, which the fixture profile does not offer, as
/// operation `n`.
fn snapshot(n: u8) -> Effect {
    let mut effect = create(n, "fixture/ok");
    effect.action = Action::Other {
        name: "sandbox.snapshot".into(),
        target: None,
        params: serde_json::Map::new(),
    };
    effect
}

/// Asks for each of `effects` and checks it is refused as shutting down
/// with its journal row as it was: whatever else would refuse it, nothing
/// is recorded and nothing is sent once the bridge is shutting down.
async fn refused_as_shutting_down(bridge: &Bridge, journal: &mut Logged, effects: Vec<Effect>) {
    for effect in effects {
        let n = effect.operation.id.to_bytes()[0];
        let before = status(journal, n);
        let result = bridge.request(journal, effect).await;
        assert!(
            matches!(result, Err(BridgeError::ShuttingDown)),
            "{n}: {result:?}"
        );
        assert_eq!(status(journal, n), before, "{n}");
    }
}

/// After `shutdown`, a request for an action outside the profile is
/// refused as shutting down, while the worker drains and once it has
/// exited, as every other request is: nothing is recorded, though Fabric
/// records such a request `rejected` while the bridge is up, and nothing
/// is written after the shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_outside_the_profile_after_a_shutdown_is_refused_as_shutting_down() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let (mut worker, bridge) = Draining::start(config()).await;
    let shutdown = bridge.shutdown(5000).await.unwrap();
    refused_as_shutting_down(&bridge, &mut journal, vec![snapshot(1)]).await;
    assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
    worker.exit(&bridge).await;
    assert_eq!(shutdown.wait().await.unwrap(), []);
    refused_as_shutting_down(&bridge, &mut journal, vec![snapshot(2)]).await;
    assert_eq!(worker.read(bridge).await, ["shutdown"]);
}

/// After `shutdown`, a request over one of the profile's limits, one the
/// encoding rules refuse, and one whose frame is over the request cap are
/// refused as shutting down, while the worker drains and once it has
/// exited, with nothing recorded and nothing written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_fails_validation_after_a_shutdown_is_refused_as_shutting_down() {
    let invalid = |base: u8| {
        let mut late = create(base, "fixture/ok");
        late.deadline_ms = 600_001;
        let mut nul = create(base + 1, "fixture/ok");
        nul.action = Action::Exec {
            target: Locator::from_value(&json!({
                "provider": "fixture",
                "scope": {"account": "acct-a", "project": "proj-1"},
                "kind": "sandbox",
                "native_id": "fx-key-1",
            }))
            .unwrap(),
            params: ExecParams {
                argv: vec!["echo".into(), "a\0b".into()],
                stdin: None,
                cwd: None,
                timeout_ms: 1000,
                max_output_bytes: 1024,
                secret_names: Vec::new(),
            },
        };
        let mut huge = create(base + 2, "fixture/ok");
        huge.action = Action::Create(CreateParams {
            image: "fixture/ok".into(),
            labels: (0..300)
                .map(|i| (format!("l{i}"), "x".repeat(4000)))
                .collect(),
            ..CreateParams::default()
        });
        vec![late, nul, huge]
    };
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let (mut worker, bridge) = Draining::start(config()).await;
    let shutdown = bridge.shutdown(5000).await.unwrap();
    refused_as_shutting_down(&bridge, &mut journal, invalid(1)).await;
    assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
    worker.exit(&bridge).await;
    assert_eq!(shutdown.wait().await.unwrap(), []);
    refused_as_shutting_down(&bridge, &mut journal, invalid(4)).await;
    assert_eq!(worker.read(bridge).await, ["shutdown"]);
}

/// After `shutdown`, a request the journal would fence, or refuse for a
/// key it holds, is refused as shutting down before the journal is asked,
/// whether or not it is in the profile: the journal is unchanged and
/// nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_the_journal_would_refuse_after_a_shutdown_is_refused_as_shutting_down() {
    let stale = |effect: Effect| Effect {
        scope: Scope {
            generation: Generation::new(2).unwrap(),
            ..effect.scope
        },
        ..effect
    };
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let (mut worker, bridge) = Draining::start(config()).await;
    // Recorded `rejected` while the bridge is up: its key is held.
    let held = bridge.request(&mut journal, snapshot(9)).await;
    assert!(
        matches!(held, Err(BridgeError::RefusedLocally(_))),
        "{held:?}"
    );
    assert_eq!(status(&journal, 9), Some(ProviderStatus::Rejected));
    let shutdown = bridge.shutdown(5000).await.unwrap();
    let refused = |a: u8, b: u8| {
        vec![
            stale(create(a, "fixture/ok")),
            stale(snapshot(b)),
            snapshot(9),
        ]
    };
    refused_as_shutting_down(&bridge, &mut journal, refused(1, 2)).await;
    assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
    worker.exit(&bridge).await;
    assert_eq!(shutdown.wait().await.unwrap(), []);
    refused_as_shutting_down(&bridge, &mut journal, refused(3, 4)).await;
    assert_eq!(worker.read(bridge).await, ["shutdown"]);
}

/// At the worker's in-flight limit, a request outside the profile is
/// refused as busy with nothing recorded, so it may be asked again later;
/// after `shutdown` it is refused as shutting down, while the worker drains
/// and once it has exited. Only the admitted request and the shutdown are
/// written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_outside_the_profile_at_the_in_flight_limit_is_refused_as_busy() {
    let mut config = config();
    config.limits.max_in_flight = 1;
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let (mut worker, bridge) = Draining::start(config).await;
    let flight = bridge
        .request(&mut journal, create(1, "fixture/ok"))
        .await
        .unwrap();
    let busy = bridge.request(&mut journal, snapshot(2)).await;
    assert!(matches!(busy, Err(BridgeError::Busy(1))), "{busy:?}");
    assert_eq!(status(&journal, 2), None, "a busy refusal records nothing");
    let shutdown = bridge.shutdown(5000).await.unwrap();
    refused_as_shutting_down(&bridge, &mut journal, vec![snapshot(2)]).await;
    assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
    worker.exit(&bridge).await;
    assert_eq!(shutdown.wait().await.unwrap(), []);
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    refused_as_shutting_down(&bridge, &mut journal, vec![snapshot(2), snapshot(3)]).await;
    assert_eq!(worker.read(bridge).await, ["request", "shutdown"]);
}

/// After `shutdown`, an observe is refused as shutting down, while the
/// worker drains and once it has exited, before the profile or the journal
/// is read: of an action the profile does not observe, and of an operation
/// the journal does not hold. Nothing is written after the shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_observe_after_a_shutdown_is_refused_as_shutting_down() {
    let resource = Locator::from_value(&json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": "proj-1"},
        "kind": "sandbox",
        "native_id": "fx-key-1",
    }))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let (mut worker, bridge) = Draining::start(config()).await;
    let shutdown = bridge.shutdown(5000).await.unwrap();
    for exited in [false, true] {
        if exited {
            worker.exit(&bridge).await;
        } else {
            assert!(bridge.lost().is_none(), "{:?}", bridge.lost());
        }
        for action in ["sandbox.snapshot", "sandbox.create"] {
            let result = bridge
                .observe(
                    &mut journal,
                    OperationId::from_bytes([1; 16]),
                    action,
                    resource.clone(),
                    1000,
                )
                .await;
            assert!(
                matches!(result, Err(BridgeError::ShuttingDown)),
                "{action}, exited {exited}: {result:?}"
            );
        }
    }
    assert_eq!(shutdown.wait().await.unwrap(), []);
    assert_eq!(worker.read(bridge).await, ["shutdown"]);
}
