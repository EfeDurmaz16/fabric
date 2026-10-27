//! The client's writes against an in-process worker over a small pipe:
//! a caller that gives up on a request whose frame does not fit the pipe
//! leaves no part of a frame behind, so the worker reads every frame
//! whole and in order.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::time::Duration;

use common::create;
use fabric_bridge::wire::Action;
use fabric_bridge::{Bridge, Config, MemoryLog, frames};
use fabric_journal::ProviderStatus;
use serde_json::{Value, json};

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
