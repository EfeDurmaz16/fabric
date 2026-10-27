//! Fabric's side of every bridge scenario in `protocol/bridge/v1/scenarios`:
//! the client runs against a scripted worker that replays the scenario's
//! `receive` steps, over an in-process stream, and records into a real
//! managed journal whose job, task and attempt are the scenario's.
//!
//! Each `send` is made through the client's API and the frame it writes is
//! compared with the fixture exactly, after mapping the fixture's request
//! ids to the client's own. A `send` the client refuses before writing it
//! (an action outside the profile, an observe the profile does not serve,
//! a cancel of nothing outstanding) must refuse with the same code and
//! reason, or cancel state, as the `receive` the worker would have sent.
//! A `send` that repeats an operation id or key the journal holds is
//! never written: Fabric does not send an operation twice, so the worker's
//! answer to it is only parsed. A `send` whose scope is not the journal's
//! current live attempt (the scope check of `scope_retained` names a job
//! the journal does not hold) is fenced before any effect, where the
//! worker refuses it before any effect too. Each `journal` step is checked against the
//! journal. Worker-side steps (`provider_calls`, `sandbox`, `advance`,
//! `worker_exit`) belong to the Capsule run of the same fixtures.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_bridge::frames;
use fabric_bridge::wire::{
    Action, CancelState, Expect, Hello, Limits, Locator, Profile, ToFabric, ToWorker,
};
use fabric_bridge::{
    Answer, Bridge, BridgeError, Config, Control, Effect, Event, InFlight, LogError, Pending,
};
use fabric_core::id::JobId;
use fabric_core::managed::{AttemptId, AttemptStatus, OperationId};
use fabric_journal::Journal;
use serde_json::Value;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;

mod common;

use common::journal;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/bridge/v1")
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn case(name: &str) -> Value {
    load(&root().join(format!("cases/{name}.json")))["message"].clone()
}

fn fixture_profile() -> Profile {
    Profile::from_value(&load(&root().join("profiles/fixture.json"))).unwrap()
}

/// What Fabric did with a `send` it did not write.
#[derive(Debug)]
enum Local {
    Refused(&'static str, &'static str),
    Used,
    Fenced,
    Cancel(CancelState),
    NotObservable,
}

type Worker = (ReadHalf<DuplexStream>, WriteHalf<DuplexStream>);

struct Run {
    name: String,
    profile: Profile,
    journal: Journal,
    bridge: Option<Bridge>,
    worker: Option<Worker>,
    connecting: Option<JoinHandle<Result<Bridge, BridgeError>>>,
    ids: HashMap<String, String>,
    local: HashMap<String, Local>,
    flights: HashMap<String, InFlight>,
    cancels: HashMap<String, Pending<CancelState>>,
    shutdown: Option<Pending<Vec<fabric_bridge::wire::Unresolved>>>,
}

fn config(hello: &Hello) -> Config {
    let mut config = Config::new(&hello.profile);
    config.fabric.clone_from(&hello.fabric);
    config.versions = hello.versions;
    config.limits = hello.limits;
    config
}

/// `value` with each fixture request id in `fields` replaced by the client's.
fn mapped(value: &Value, ids: &HashMap<String, String>, fields: &[&str]) -> Value {
    let mut value = value.clone();
    for field in fields {
        if let Some(id) = value[*field].as_str().and_then(|id| ids.get(id)) {
            value[*field] = Value::String(id.clone());
        }
    }
    if let Some(items) = value.get_mut("unresolved").and_then(Value::as_array_mut) {
        for item in items {
            if let Some(id) = item["request_id"].as_str().and_then(|id| ids.get(id)) {
                item["request_id"] = Value::String(id.clone());
            }
        }
    }
    value
}

impl Run {
    fn bridge(&self) -> &Bridge {
        self.bridge.as_ref().unwrap()
    }

    /// Reads the frame the client wrote and checks it is `expected`,
    /// recording the client's id for the fixture's.
    async fn written(&mut self, expected: &Value, fixture_id: Option<&str>, actual: Option<&str>) {
        if let (Some(f), Some(a)) = (fixture_id, actual) {
            self.ids.insert(f.to_owned(), a.to_owned());
        }
        let (reader, _) = self.worker.as_mut().unwrap();
        let frame = frames::read(reader, 1 << 20).await.unwrap().unwrap();
        let want = mapped(expected, &self.ids, &["request_id", "cancels"]);
        assert_eq!(frame, want, "{}: the frame Fabric wrote", self.name);
    }

    fn connect(&mut self, hello: &Hello) {
        let (fabric, worker) = tokio::io::duplex(64 << 20);
        let (reader, writer) = tokio::io::split(fabric);
        self.worker = Some(tokio::io::split(worker));
        self.connecting = Some(tokio::spawn(Bridge::connect(
            reader,
            Box::new(writer),
            config(hello),
        )));
    }

    async fn send(&mut self, message: &Value) {
        let fixture_id = message["request_id"].as_str().map(str::to_owned);
        match ToWorker::from_value(message).unwrap() {
            ToWorker::Hello(hello) => {
                self.connect(&hello);
                self.written(message, None, None).await;
            }
            ToWorker::Request(request) => {
                let effect = Effect {
                    operation: request.operation.clone(),
                    scope: request.scope,
                    action: request.action.clone(),
                    approval: request.approval.clone(),
                    policy: request.policy.clone(),
                    deadline_ms: request.deadline_ms,
                };
                let bridge = self.bridge.as_ref().unwrap();
                match bridge.request(&mut self.journal, effect).await {
                    Ok(flight) => {
                        let actual = flight.request_id().to_owned();
                        self.flights.insert(fixture_id.clone().unwrap(), flight);
                        self.written(message, fixture_id.as_deref(), Some(&actual))
                            .await;
                    }
                    Err(BridgeError::RefusedLocally(r)) => {
                        self.local
                            .insert(fixture_id.unwrap(), Local::Refused(r.code, r.reason));
                    }
                    Err(BridgeError::Log(LogError::Used(_))) => {
                        self.local.insert(fixture_id.unwrap(), Local::Used);
                    }
                    Err(BridgeError::Log(LogError::Fenced(_))) => {
                        self.local.insert(fixture_id.unwrap(), Local::Fenced);
                    }
                    Err(e) => panic!("{}: request refused: {e}", self.name),
                }
            }
            ToWorker::Observe(observe) => {
                let bridge = self.bridge.as_ref().unwrap();
                let result = bridge
                    .observe(
                        &mut self.journal,
                        observe.operation_id,
                        &observe.action,
                        observe.resource.clone(),
                        observe.wait_ms,
                    )
                    .await;
                match result {
                    Ok(flight) => {
                        let actual = flight.request_id().to_owned();
                        self.flights.insert(fixture_id.clone().unwrap(), flight);
                        self.written(message, fixture_id.as_deref(), Some(&actual))
                            .await;
                    }
                    Err(BridgeError::RefusedLocally(r)) => {
                        self.local
                            .insert(fixture_id.unwrap(), Local::Refused(r.code, r.reason));
                    }
                    Err(BridgeError::NotObservable(_)) => {
                        self.local.insert(fixture_id.unwrap(), Local::NotObservable);
                    }
                    Err(e) => panic!("{}: observe refused: {e}", self.name),
                }
            }
            ToWorker::Cancel(cancel) => {
                let target = self
                    .ids
                    .get(&cancel.cancels)
                    .cloned()
                    .unwrap_or_else(|| format!("unsent-{}", cancel.cancels));
                match self.bridge().cancel(&target).await.unwrap() {
                    Control::Local(state) => {
                        self.local.insert(fixture_id.unwrap(), Local::Cancel(state));
                    }
                    Control::Sent(pending) => {
                        let actual = pending.request_id().to_owned();
                        self.cancels.insert(fixture_id.clone().unwrap(), pending);
                        self.written(message, fixture_id.as_deref(), Some(&actual))
                            .await;
                    }
                }
            }
            ToWorker::Shutdown { drain_ms, .. } => {
                let pending = self.bridge().shutdown(drain_ms).await.unwrap();
                let actual = pending.request_id().to_owned();
                self.shutdown = Some(pending);
                self.written(message, fixture_id.as_deref(), Some(&actual))
                    .await;
            }
        }
    }

    /// A `receive` for a `send` Fabric did not write: Fabric's own answer
    /// must be the worker's.
    fn local_answer(&self, local: &Local, message: &Value) {
        let parsed = ToFabric::parse(message, Expect::Session(&self.profile)).unwrap();
        match (local, parsed) {
            (Local::Refused(code, reason), ToFabric::Error(e)) => assert_eq!(
                (*code, *reason),
                (e.error.code.name(), e.error.reason.as_str()),
                "{}: Fabric's refusal is not the worker's",
                self.name
            ),
            (Local::Cancel(state), ToFabric::CancelAck { state: want, .. }) => {
                assert_eq!(*state, want, "{}", self.name);
            }
            // A scope the journal does not hold as the job's current live
            // attempt is fenced by Fabric before any effect, where the
            // worker would refuse it before any effect too.
            (Local::Fenced, ToFabric::Error(e)) => assert!(
                e.error.code.is_refusal(),
                "{}: Fabric fenced what the worker answers {:?}",
                self.name,
                e.error.code
            ),
            // Fabric never sends an operation twice and never observes
            // one its journal does not hold; the worker's answer is only parsed.
            (Local::Used | Local::NotObservable, _) => {}
            (local, other) => panic!("{}: {local:?} against {other:?}", self.name),
        }
    }

    async fn receive(&mut self, message: &Value) {
        let fixture_id = message["request_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if let Some(local) = self.local.get(&fixture_id) {
            self.local_answer(local, message);
            return;
        }
        let wire = mapped(message, &self.ids, &["request_id", "cancels"]);
        {
            let (_, writer) = self.worker.as_mut().unwrap();
            let frame = frames::encode(&wire, 16 << 20).unwrap();
            frames::write(writer, &frame).await.unwrap();
        }
        match message["type"].as_str().unwrap() {
            "hello_ack" | "hello_refused" => {
                let connected = self.connecting.take().unwrap().await.unwrap();
                match (connected, message["type"].as_str()) {
                    (Ok(bridge), Some("hello_ack")) => {
                        assert_eq!(bridge.profile(), &self.profile, "{}", self.name);
                        self.bridge = Some(bridge);
                    }
                    (Err(BridgeError::Refused { versions, code, .. }), Some("hello_refused")) => {
                        assert_eq!(versions.min, message["versions"]["min"].as_u64().unwrap());
                        assert_eq!(code, message["reason"]["code"].as_str().unwrap());
                    }
                    (other, _) => panic!("{}: handshake {other:?}", self.name),
                }
            }
            "progress" => {
                let flight = self.flights.get_mut(&fixture_id).unwrap();
                let event = flight.next(&mut self.journal).await.unwrap();
                let want = Locator::from_value(&message["resource"]).unwrap();
                assert_eq!(event, Event::Progress(want), "{}", self.name);
            }
            "result" | "error" => {
                let flight = self.flights.remove(&fixture_id).unwrap();
                let outcome = flight.finish(&mut self.journal).await.unwrap();
                let want = ToFabric::parse(&wire, Expect::Session(&self.profile)).unwrap();
                let got = match outcome.answer {
                    Answer::Result(r) => ToFabric::Result(r),
                    Answer::Error(e) => ToFabric::Error(e),
                    other => panic!("{}: {other:?}", self.name),
                };
                assert_eq!(got, want, "{}", self.name);
            }
            "cancel_ack" => {
                let state = self
                    .cancels
                    .remove(&fixture_id)
                    .unwrap()
                    .wait()
                    .await
                    .unwrap();
                assert_eq!(
                    state.name(),
                    message["state"].as_str().unwrap(),
                    "{}",
                    self.name
                );
            }
            "shutdown_ack" => {
                let unresolved = self.shutdown.take().unwrap().wait().await.unwrap();
                let want = match ToFabric::parse(&wire, Expect::Session(&self.profile)).unwrap() {
                    ToFabric::ShutdownAck { unresolved, .. } => unresolved,
                    other => panic!("{other:?}"),
                };
                assert_eq!(unresolved, want, "{}", self.name);
            }
            other => panic!("{}: a {other} step", self.name),
        }
    }

    fn journal_step(&self, step: &Value) {
        if let Some(op) = step["operation_id"].as_str() {
            let op: OperationId = op.parse().unwrap();
            let entry = self
                .journal
                .provider_operation(op)
                .unwrap()
                .unwrap_or_else(|| panic!("{}: {op} is not in the journal", self.name));
            assert_eq!(
                entry.status.name(),
                step["status"].as_str().unwrap(),
                "{}: {op}",
                self.name
            );
            return;
        }
        // Identity unchanged: the job's generation and its attempt stand.
        let job: JobId = step["job"].as_str().unwrap().parse().unwrap();
        let attempt: AttemptId = step["attempt"].as_str().unwrap().parse().unwrap();
        let state = self.journal.job(job).unwrap().unwrap();
        assert_eq!(
            state.job.generation.to_string(),
            step["generation"].as_str().unwrap(),
            "{}",
            self.name
        );
        let attempt = state.attempt(attempt).unwrap();
        assert_eq!(attempt.status, AttemptStatus::Granted, "{}", self.name);
        assert_eq!(attempt.generation, state.job.generation, "{}", self.name);
    }

    /// The worker dies: every outstanding operation becomes unknown.
    async fn kill(&mut self) {
        self.worker = None;
        for (_, flight) in self.flights.drain() {
            let outcome = flight.finish(&mut self.journal).await.unwrap();
            assert!(matches!(outcome.answer, Answer::Lost(_)), "{}", self.name);
            assert_eq!(outcome.status.name(), "unknown", "{}", self.name);
        }
        let lost = self.bridge.take().unwrap();
        assert!(lost.lost().is_some(), "{}", self.name);
    }
}

async fn run(name: String, scenario: Value) {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run {
        name,
        profile: fixture_profile(),
        journal: journal(dir.path()),
        bridge: None,
        worker: None,
        connecting: None,
        ids: HashMap::new(),
        local: HashMap::new(),
        flights: HashMap::new(),
        cancels: HashMap::new(),
        shutdown: None,
    };
    let steps = scenario["steps"].as_array().unwrap();
    let mut steps = steps.iter().peekable();
    if steps
        .peek()
        .is_some_and(|s| s["handshake"] == Value::Bool(false))
    {
        steps.next();
    } else {
        run.send(&case("hello_valid")).await;
        run.receive(&case("hello_ack_valid")).await;
    }
    for step in steps {
        let (kind, body) = step.as_object().unwrap().iter().next().unwrap();
        match kind.as_str() {
            "send" => run.send(body).await,
            "receive" => run.receive(body).await,
            "journal" => run.journal_step(body),
            "kill_worker" => run.kill().await,
            // A respawn is the next send's hello; the rest is the worker's side.
            "respawn" | "provider_calls" | "sandbox" | "advance" | "worker_exit" => {}
            other => panic!("{}: unknown step {other}", run.name),
        }
    }
    assert!(run.flights.is_empty(), "{}: unanswered requests", run.name);
    run.journal.verify().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fabric_runs_every_scenario_against_a_scripted_worker() {
    let mut names = Vec::new();
    for entry in fs::read_dir(root().join("scenarios")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let scenario = load(&path);
        tokio::time::timeout(Duration::from_secs(20), run(name.clone(), scenario))
            .await
            .unwrap_or_else(|_| panic!("{name} timed out"));
        names.push(name);
    }
    names.sort();
    assert_eq!(names.len(), 15, "{names:?}");
}

/// The scenarios name their action through the profile; a sanity check
/// that the fixture profile is the one the scenarios negotiate.
#[test]
fn the_scenarios_negotiate_the_fixture_profile() {
    let ack = case("hello_ack_valid");
    let profile = Profile::from_value(&ack["profile"]).unwrap();
    assert_eq!(profile, fixture_profile());
    assert_eq!(
        Limits::DEFAULT.max_request_bytes,
        ack["limits"]["max_request_bytes"].as_u64().unwrap()
    );
    assert!(matches!(
        ToWorker::from_value(&case("request_create_valid")).unwrap(),
        ToWorker::Request(r) if matches!(r.action, Action::Create(_))
    ));
}
