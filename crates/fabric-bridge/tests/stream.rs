//! The client's writes against an in-process worker: a caller that gives
//! up on a request whose frame does not fit the pipe leaves no part of a
//! frame behind, so the worker reads every frame whole and in order, and
//! a request admitted before a shutdown is never written after it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use common::{create, operation};
use fabric_bridge::wire::{Action, Scope};
use fabric_bridge::{
    Bridge, BridgeError, Config, Entry, LogError, LogThread, MemoryLog, OperationLog, frames,
};
use fabric_core::managed::OperationId;
use fabric_journal::{
    CleanupRefusal, LocatorRecord, ProviderAnswer, ProviderDispatch, ProviderStatus,
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
