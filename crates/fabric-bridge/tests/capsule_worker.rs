//! Fabric's bridge against the real Capsule worker (Capsule's
//! `packages/bridge-worker`), not the Rust double: the seam across the two
//! implementations of bridge protocol v1 (ADR 0021, docs/bridge-protocol.md).
//!
//! Two kinds of proof run here. Every shared scenario in
//! `protocol/bridge/v1/scenarios` is replayed over the worker's real stdio,
//! with Fabric's framing and Fabric's parser reading every answer. Then
//! Fabric's client and journal run the fixture profile end to end: create,
//! exec, observe and destroy keeping the scoped locator, refusals before
//! any effect, a repeated key, and a worker killed mid-request. The Docker
//! test does the same against the local engine.
//!
//! Every test is ignored by default, because the worker is built from
//! another repository with Node and pnpm:
//!
//! ```sh
//! CAPSULE_BRIDGE_WORKER=<capsule>/packages/bridge-worker/dist/bin.js \
//!   cargo test -p fabric-bridge --test capsule_worker -- --ignored
//! ```
//!
//! Run with `--ignored` and without the variable, each test fails and says
//! so. The Docker test also needs `CAPSULE_BRIDGE_DOCKER_IMAGE`, an image
//! the engine already holds; it never pulls one. Without it the Docker
//! test prints why it did nothing and returns.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{
    ATTEMPT, Cleanup, DOCKER_IMAGE_VAR, JOB, Logged, capsule_worker, create, docker, docker_image,
    fresh, journal, journal_for, operation, scope,
};
use fabric_bridge::frames;
use fabric_bridge::wire::{
    Action, CreateParams, Digest, EffectState, ErrorCode, ExecParams, Expect, Locator,
    OperationRef, Profile, RequestPolicy, ResultMessage, ResultState, Scope, SecretsPolicy,
    ToFabric, ToWorker,
};
use fabric_bridge::{Answer, Bridge, BridgeError, Config, Effect, Event, LogError, Outcome};
use fabric_core::id::JobId;
use fabric_core::managed::{
    AttemptId, AttemptStatus, Generation, OperationId, OperationKey, TaskId,
};
use fabric_journal::{Journal, ProviderOperation, ProviderStatus};
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/bridge/v1")
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn case(name: &str) -> Value {
    load(&root().join(format!("cases/{name}.json")))["message"].clone()
}

fn profile(name: &str) -> Profile {
    Profile::from_value(&load(&root().join(format!("profiles/{name}.json")))).unwrap()
}

/// Writes a worker config into `dir` and returns its path.
fn write_config(dir: &Path, name: &str, config: &Value) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, serde_json::to_vec_pretty(config).unwrap()).unwrap();
    path
}

/// A node worker starts slower than the double, and slower still on a busy
/// machine: the handshake gets the protocol's full 10 seconds.
fn config(profile: &str) -> Config {
    let mut config = Config::new(profile);
    config.grace = Duration::from_secs(2);
    config.kill_grace = Duration::from_secs(2);
    config
}

async fn spawn(profile: &str, config_path: &Path) -> Bridge {
    Bridge::spawn(capsule_worker(profile, config_path), config(profile))
        .await
        .unwrap()
}

fn entry(journal: &Journal, id: OperationId) -> ProviderOperation {
    journal.provider_operation(id).unwrap().unwrap()
}

fn id(n: u8) -> OperationId {
    OperationId::from_bytes([n; 16])
}

/// What the journal stores for `locator`: its JSON text.
fn stored(locator: &Locator) -> String {
    locator.to_value().to_string()
}

fn result(outcome: &Outcome) -> &ResultMessage {
    match &outcome.answer {
        Answer::Result(r) => r,
        other => panic!("expected a result, got {other:?}"),
    }
}

/// The code, reason and effect of an `error` answer.
fn refusal(outcome: &Outcome) -> (ErrorCode, &str, EffectState) {
    match &outcome.answer {
        Answer::Error(e) => (e.error.code, e.error.reason.as_str(), e.error.effect),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn exec(n: u8, target: &Locator, argv: &[&str], max_output_bytes: u64) -> Effect {
    Effect {
        operation: operation(n),
        scope: scope(),
        action: Action::Exec {
            target: target.clone(),
            params: ExecParams {
                argv: argv.iter().map(|a| (*a).to_owned()).collect(),
                stdin: None,
                cwd: None,
                timeout_ms: 10_000,
                max_output_bytes,
                secret_names: Vec::new(),
            },
        },
        approval: None,
        policy: None,
        deadline_ms: 30_000,
    }
}

fn destroy(n: u8, target: &Locator, approval: Option<&str>) -> Effect {
    Effect {
        operation: operation(n),
        scope: scope(),
        action: Action::Destroy {
            target: target.clone(),
        },
        approval: approval.map(str::to_owned),
        policy: None,
        deadline_ms: 30_000,
    }
}

fn stream<'a>(r: &'a ResultMessage, name: &str) -> &'a Value {
    &r.output[name]
}

// ---------------------------------------------------------------------------
// The shared scenarios over the worker's real stdio.
// ---------------------------------------------------------------------------

/// One worker process with raw pipes: Fabric's framing writes and reads,
/// Fabric's parser checks every answer.
struct Raw {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: tokio::process::ChildStdout,
}

impl Raw {
    fn spawn(config: &Path) -> Raw {
        let mut command = capsule_worker("fixture", config);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        Raw {
            child,
            stdin,
            stdout,
        }
    }

    async fn send(&mut self, message: &Value) {
        self.send_all(std::slice::from_ref(message)).await;
    }

    /// Writes the frames of `messages` in one write, the way back-to-back
    /// frames reach the worker on a pipe. Capsule's own scenario runner
    /// sends consecutive `send` steps as one chunk too; on the fixture's
    /// virtual clock, a split write can let an `observe` advance before a
    /// following `shutdown` arrives.
    async fn send_all(&mut self, messages: &[Value]) {
        let mut chunk = Vec::new();
        for message in messages {
            chunk.extend(frames::encode(message, 1 << 20).unwrap());
        }
        frames::write(self.stdin.as_mut().unwrap(), &chunk)
            .await
            .unwrap();
    }

    async fn receive(&mut self, cap: usize, what: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(20), frames::read(&mut self.stdout, cap))
            .await
            .unwrap_or_else(|_| panic!("{what}: no frame within 20 s"))
            .unwrap_or_else(|fault| panic!("{what}: frame refused: {fault}"))
            .unwrap_or_else(|| panic!("{what}: the worker closed its output"))
    }

    /// Reads the next answer, checks Fabric's parser accepts it under the
    /// last `hello` or the session profile, and compares it with `expected`
    /// bar `message` strings.
    async fn expect(&mut self, expected: &Value, hello: &Value, session: &Profile, what: &str) {
        let handshake = matches!(
            expected["type"].as_str(),
            Some("hello_ack" | "hello_refused")
        );
        let cap = if handshake { 1 << 20 } else { 16 << 20 };
        let actual = self.receive(cap, what).await;
        let ToWorker::Hello(sent) = ToWorker::from_value(hello).unwrap() else {
            panic!("{what}: the last hello is not a hello")
        };
        let expect = if handshake {
            Expect::Handshake(&sent)
        } else {
            Expect::Session(session)
        };
        ToFabric::parse(&actual, expect)
            .unwrap_or_else(|r| panic!("{what}: Fabric refuses the worker's answer: {r}"));
        assert_eq!(comparable(&actual), comparable(expected), "{what}");
    }

    /// Waits for the worker to exit and checks it wrote nothing more.
    async fn exit(&mut self, what: &str) -> std::process::ExitStatus {
        let more = tokio::time::timeout(
            Duration::from_secs(20),
            frames::read(&mut self.stdout, 16 << 20),
        )
        .await
        .unwrap_or_else(|_| panic!("{what}: stdout still open after 20 s"));
        assert!(
            matches!(more, Ok(None)),
            "{what}: the worker wrote more than the scenario expects: {more:?}"
        );
        tokio::time::timeout(Duration::from_secs(20), self.child.wait())
            .await
            .unwrap_or_else(|_| panic!("{what}: the worker did not exit"))
            .unwrap()
    }
}

/// `message` without the strings the fixtures compare for presence only.
fn comparable(message: &Value) -> Value {
    let mut message = message.clone();
    let kind = message["type"].as_str().unwrap_or_default().to_owned();
    let object = message.as_object_mut().unwrap();
    match kind.as_str() {
        "error" => {
            let error = object
                .get_mut("error")
                .and_then(Value::as_object_mut)
                .unwrap();
            assert!(
                error.remove("message").is_some(),
                "an error without message"
            );
        }
        "protocol_error" => {
            object.remove("message");
        }
        _ => {}
    }
    message
}

/// What a replay did with a scenario's steps.
#[derive(Debug, Default)]
struct Tally {
    /// `send` and `receive` steps over the pipes, and kills, respawns and exits.
    wire: usize,
    /// Steps only the in-process provider or Fabric's journal can check.
    elsewhere: usize,
    /// Steps not run because an `advance` needs the in-process provider.
    after_advance: usize,
}

async fn replay(name: &str, dir: &Path) -> Tally {
    let scenario = load(&root().join(format!("scenarios/{name}.json")));
    assert_eq!(scenario["profile"], "fixture", "{name}");
    let config = write_config(dir, &format!("{name}.json"), &scenario["provider_state"]);
    let session = profile("fixture");
    let mut steps: Vec<Value> = scenario["steps"].as_array().unwrap().clone();
    let mut tally = Tally::default();
    let mut raw = Raw::spawn(&config);
    let mut hello = case("hello_valid");
    if steps.first().is_some_and(|s| s["handshake"] == false) {
        steps.remove(0);
    } else {
        raw.send(&hello).await;
        let ack = raw.receive(1 << 20, &format!("{name}: hello_ack")).await;
        assert_eq!(ack, case("hello_ack_valid"), "{name}: hello_ack");
    }
    let mut alive = true;
    // Consecutive `send` steps go out as one write, as Capsule's runner
    // sends them; any other step writes what is queued first.
    let mut outbox: Vec<Value> = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let what = format!("{name} step {index}");
        let form = step.as_object().unwrap().keys().next().unwrap().as_str();
        if form == "send" {
            if step["send"]["type"] == "hello" {
                hello = step["send"].clone();
            }
            outbox.push(step["send"].clone());
            tally.wire += 1;
            continue;
        }
        if !outbox.is_empty() {
            raw.send_all(&outbox).await;
            outbox.clear();
        }
        match form {
            "receive" => {
                raw.expect(&step["receive"], &hello, &session, &what).await;
                tally.wire += 1;
            }
            "kill_worker" => {
                assert_eq!(step["kill_worker"], "SIGKILL", "{what}");
                raw.child.start_kill().unwrap();
                let status = raw.child.wait().await.unwrap();
                assert_eq!(status.signal(), Some(9), "{what}");
                alive = false;
                tally.wire += 1;
            }
            "respawn" => {
                raw = Raw::spawn(&config);
                alive = true;
                tally.wire += 1;
            }
            "worker_exit" => {
                let status = raw.exit(&what).await;
                assert_eq!(
                    status.code().map(i64::from),
                    step["worker_exit"].as_i64(),
                    "{what}"
                );
                alive = false;
                tally.wire += 1;
            }
            "advance" => {
                // Settling scripted states is the in-process provider's;
                // what follows depends on it, so the replay stops here.
                tally.after_advance = steps.len() - index;
                break;
            }
            "provider_calls" | "sandbox" | "journal" => tally.elsewhere += 1,
            other => panic!("{what}: unknown step form {other}"),
        }
    }
    if !outbox.is_empty() {
        raw.send_all(&outbox).await;
    }
    if alive {
        if tally.after_advance > 0 {
            raw.child.start_kill().unwrap();
            raw.child.wait().await.unwrap();
        } else {
            // End of input is a shutdown with no drain: nothing is open,
            // so the worker writes nothing and exits 0.
            raw.stdin.take();
            let status = raw.exit(&format!("{name}: end of input")).await;
            assert_eq!(status.code(), Some(0), "{name}: end of input");
        }
    }
    tally
}

/// Every shared scenario, as the real worker answers it over its pipes,
/// matches the fixture exactly (bar `message` strings) and parses with
/// Fabric's own parser. Steps that read the in-process provider
/// (`provider_calls`, `sandbox`, `advance`) or Fabric's journal are the
/// other runs' to check; `pending_destroy_with_locator` stops at its
/// `advance`.
#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
async fn every_shared_scenario_over_the_workers_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let mut names: Vec<String> = fs::read_dir(root().join("scenarios"))
        .unwrap()
        .map(|e| {
            e.unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names.len(), 15, "the v1 scenario set");
    let mut stopped = Vec::new();
    for name in &names {
        let tally = replay(name, dir.path()).await;
        eprintln!(
            "{name}: {} wire steps, {} checked elsewhere, {} after advance",
            tally.wire, tally.elsewhere, tally.after_advance
        );
        assert!(tally.wire > 0, "{name}");
        if tally.after_advance > 0 {
            stopped.push(name.clone());
        }
    }
    assert_eq!(stopped, ["pending_destroy_with_locator"]);
}

// ---------------------------------------------------------------------------
// Fabric's client and journal against the fixture profile.
// ---------------------------------------------------------------------------

const OTHER_JOB: &str = "01K8W3A00000000000000000J2";

/// The fixture locator of `native_id` in the worker's scope.
fn fx(native_id: &str) -> Locator {
    fx_in("proj-1", native_id)
}

fn fx_in(project: &str, native_id: &str) -> Locator {
    Locator::from_value(&json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": project},
        "kind": "sandbox",
        "native_id": native_id,
    }))
    .unwrap()
}

/// The fixture provider every client test starts from: a policy, grants
/// the worker resolves by ref, sandboxes of this job, of another job and
/// of another project, and scripted faults.
fn fixture_config(extra_sandboxes: &[&str]) -> Value {
    let mut sandboxes = vec![
        json!({"native_id": "fx-seed", "project": "proj-1", "owner_job": JOB, "status": "ready"}),
        json!({"native_id": "fx-long", "project": "proj-1", "owner_job": JOB, "status": "ready"}),
        json!({"native_id": "fx-other-job", "project": "proj-1", "owner_job": OTHER_JOB, "status": "ready"}),
    ];
    for native_id in extra_sandboxes {
        sandboxes.push(
            json!({"native_id": native_id, "project": "proj-1", "owner_job": JOB, "status": "ready"}),
        );
    }
    json!({
        "binding": {"account": "acct-a", "project": "proj-1"},
        "policy": {"images": ["fixture/ok"], "secrets": {"allowed": ["API_TOKEN", "DB_URL"]}},
        "approvals": [
            {"ref": "apr-destroy-10", "action": "sandbox.destroy", "native_id": "fx-key-10", "expired": false},
            {"ref": "apr-expired", "action": "sandbox.destroy", "native_id": "fx-seed", "expired": true},
            {"ref": "apr-create-only", "action": "sandbox.create", "native_id": null, "expired": false}
        ],
        "sandboxes": sandboxes,
        "script": [
            {"action": "sandbox.create", "native_id": "fx-key-11", "behavior": "apply_then_lose_reply"},
            {"action": "sandbox.create", "native_id": "fx-key-51", "behavior": "apply_then_lose_reply"},
            {"action": "sandbox.exec", "native_id": "fx-long", "behavior": "run_until_cancelled"}
        ]
    })
}

/// Every locator the journal holds names the fixture provider, a sandbox
/// and the worker's whole scope: a native id never travels alone.
fn assert_scope_retained(journal: &Journal, ids: &[OperationId]) {
    for &op in ids {
        let entry = entry(journal, op);
        let locator: Value = serde_json::from_str(entry.locator.as_deref().unwrap()).unwrap();
        assert_eq!(locator["provider"], "fixture", "{op:?}");
        assert_eq!(locator["kind"], "sandbox", "{op:?}");
        assert_eq!(
            locator["scope"],
            json!({"account": "acct-a", "project": "proj-1"}),
            "{op:?}"
        );
    }
}

#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
async fn the_handshake_negotiates_the_fixture_profile_and_refuses_another_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "worker.json", &fixture_config(&[]));
    let bridge = spawn("fixture", &path).await;
    assert_eq!(bridge.protocol(), 1);
    assert_eq!(
        *bridge.profile(),
        profile("fixture"),
        "the worker sends the v1 profile whole"
    );
    assert_eq!(bridge.limits().max_in_flight, 4, "the worker lowered it");
    assert!(
        bridge.worker().starts_with("capsule-worker/"),
        "{}",
        bridge.worker()
    );
    let exit = bridge.close().await.unwrap();
    assert_eq!(exit.code(), Some(0), "end of input is a clean shutdown");

    let mut later = config("fixture");
    later.versions.min = 2;
    later.versions.max = 3;
    let refused = Bridge::spawn(capsule_worker("fixture", &path), later).await;
    let Err(BridgeError::Refused { versions, code, .. }) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!((versions.min, versions.max), (1, 1));
    assert_eq!(code, "unsupported_version");
}

/// Create, exec, observe and destroy through Fabric's client, each
/// recorded in the journal with the full scoped locator.
#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
#[expect(clippy::too_many_lines, reason = "one session, step by step")]
async fn create_exec_observe_destroy_keep_the_scoped_locator() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let path = write_config(dir.path(), "worker.json", &fixture_config(&[]));
    let bridge = spawn("fixture", &path).await;
    let sandbox = fx("fx-key-10");

    // Recorded dispatched before the frame is written, then succeeded.
    let flight = bridge
        .request(&mut journal, create(10, "fixture/ok"))
        .await
        .unwrap();
    assert_eq!(entry(&journal, id(10)).status, ProviderStatus::Dispatched);
    let created = flight.finish(&mut journal).await.unwrap();
    assert_eq!(created.status, ProviderStatus::Succeeded);
    assert_eq!(result(&created).resource, sandbox);
    assert_eq!(result(&created).output["status"], "ready");
    assert!(!result(&created).replayed);
    assert_eq!(entry(&journal, id(10)).locator, Some(stored(&sandbox)));

    let echoed = bridge
        .request(
            &mut journal,
            exec(12, &sandbox, &["echo", "hello", "bridge"], 4096),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(echoed.status, ProviderStatus::Succeeded);
    let r = result(&echoed);
    assert_eq!(r.state, ResultState::Completed);
    assert_eq!(r.output["exit_code"], 0);
    assert_eq!(stream(r, "stdout")["text"], "hello bridge\n");
    assert_eq!(stream(r, "stdout")["truncated"], false);

    // A nonzero exit is a completed result; the sandbox stays.
    let failed = bridge
        .request(
            &mut journal,
            exec(13, &sandbox, &["fail", "3", "boom"], 4096),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(failed.status, ProviderStatus::Succeeded);
    assert_eq!(result(&failed).output["exit_code"], 3);
    assert_eq!(stream(result(&failed), "stderr")["text"], "boom\n");

    // Output is capped while it is captured, and says so.
    let mut big = exec(14, &sandbox, &["cat"], 1024);
    if let Action::Exec { params, .. } = &mut big.action {
        params.stdin = Some("a".repeat(10_000));
    }
    let capped = bridge
        .request(&mut journal, big)
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    let out = stream(result(&capped), "stdout");
    assert_eq!(out["truncated"], true);
    assert_eq!(out["captured_bytes"], 1024);
    assert_eq!(out["text"].as_str().unwrap().len(), 1024);

    // The reply is lost after the create applied: unknown, with the
    // locator. Only observe by the original operation moves it on.
    let lost = bridge
        .request(&mut journal, create(11, "fixture/ok"))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(lost.status, ProviderStatus::Unknown);
    let (code, reason, effect) = refusal(&lost);
    assert_eq!(
        (code, reason, effect),
        (ErrorCode::Unknown, "reply_lost", EffectState::Unknown)
    );
    assert_eq!(
        entry(&journal, id(11)).locator,
        Some(stored(&fx("fx-key-11")))
    );
    let observed = bridge
        .observe(
            &mut journal,
            id(11),
            "sandbox.create",
            fx("fx-key-11"),
            5_000,
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(observed.status, ProviderStatus::Succeeded);
    assert_eq!(result(&observed).resource, fx("fx-key-11"));

    // Destroy needs its own fresh grant, and is terminal when observed gone.
    let destroyed = bridge
        .request(&mut journal, destroy(15, &sandbox, Some("apr-destroy-10")))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(destroyed.status, ProviderStatus::Succeeded);
    assert_eq!(result(&destroyed).output["status"], "destroyed");

    assert_scope_retained(&journal, &[id(10), id(11), id(12), id(13), id(14), id(15)]);
    assert!(bridge.lost().is_none());
    assert_eq!(bridge.late_answers(), 0);
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}

/// Each refusal, by the worker or by Fabric, is recorded `rejected` with
/// no effect and no locator, and leaves the worker serving.
#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
#[expect(
    clippy::too_many_lines,
    reason = "one refusal per step, each with its proof"
)]
async fn denied_and_unsupported_requests_fail_before_any_effect() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let path = write_config(dir.path(), "worker.json", &fixture_config(&[]));
    let bridge = spawn("fixture", &path).await;

    let mut refused = Vec::new();
    let mut check = |journal: &Journal, outcome: &Outcome, n: u8, want: (ErrorCode, &str)| {
        assert_eq!(outcome.status, ProviderStatus::Rejected, "operation {n}");
        let (code, reason, effect) = refusal(outcome);
        assert_eq!((code, reason), want, "operation {n}");
        assert_eq!(effect, EffectState::None, "operation {n}");
        let entry = entry(journal, id(n));
        assert_eq!(entry.status, ProviderStatus::Rejected, "operation {n}");
        assert_eq!(
            entry.answer.effect.as_deref(),
            Some("none"),
            "operation {n}"
        );
        assert_eq!(entry.locator, None, "operation {n}");
        refused.push(n);
    };

    // An image the worker's policy does not allow.
    let denied = bridge
        .request(&mut journal, create(20, "fixture/forbidden"))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    check(&journal, &denied, 20, (ErrorCode::Denied, "policy_denied"));
    // No sandbox came of it: the provider does not know fx-key-20.
    let probe = bridge
        .request(&mut journal, exec(21, &fx("fx-key-20"), &["true"], 1024))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(probe.status, ProviderStatus::Failed);
    assert_eq!(
        refusal(&probe),
        (ErrorCode::ProviderFailed, "not_found", EffectState::None)
    );

    // Destroy without a grant, with a create-only grant, with an expired one.
    for (n, approval, reason) in [
        (22, None, "approval_required"),
        (23, Some("apr-create-only"), "approval_scope"),
        (24, Some("apr-expired"), "approval_expired"),
    ] {
        let outcome = bridge
            .request(&mut journal, destroy(n, &fx("fx-seed"), approval))
            .await
            .unwrap()
            .finish(&mut journal)
            .await
            .unwrap();
        check(&journal, &outcome, n, (ErrorCode::Denied, reason));
    }

    // A locator in another project, and a sandbox of another job.
    let other_scope = bridge
        .request(
            &mut journal,
            exec(26, &fx_in("proj-2", "fx-seed"), &["true"], 1024),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    check(
        &journal,
        &other_scope,
        26,
        (ErrorCode::Denied, "scope_mismatch"),
    );
    let other_job = bridge
        .request(&mut journal, exec(27, &fx("fx-other-job"), &["true"], 1024))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    check(
        &journal,
        &other_job,
        27,
        (ErrorCode::Denied, "owner_mismatch"),
    );

    // Secrets: a name the worker does not allow; a request block with
    // `allowed` left out denies all; two allowlists intersect.
    let secret = |n: u8, names: &[&str], policy: Option<RequestPolicy>| {
        let mut effect = create(n, "fixture/ok");
        if let Action::Create(params) = &mut effect.action {
            params.secret_names = names.iter().map(|s| (*s).to_owned()).collect();
        }
        effect.policy = policy;
        effect
    };
    let not_allowed = bridge
        .request(&mut journal, secret(28, &["OTHER"], None))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    check(
        &journal,
        &not_allowed,
        28,
        (ErrorCode::Denied, "secret_not_allowed"),
    );
    let deny_all = RequestPolicy {
        secrets: Some(SecretsPolicy { allowed: None }),
    };
    let denied_all = bridge
        .request(&mut journal, secret(29, &["API_TOKEN"], Some(deny_all)))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    check(
        &journal,
        &denied_all,
        29,
        (ErrorCode::Denied, "secret_not_allowed"),
    );
    let narrowed = RequestPolicy {
        secrets: Some(SecretsPolicy {
            allowed: Some(vec!["API_TOKEN".into(), "OTHER".into()]),
        }),
    };
    let intersected = bridge
        .request(&mut journal, secret(30, &["API_TOKEN"], Some(narrowed)))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(intersected.status, ProviderStatus::Succeeded);
    let answer = result(&intersected);
    let text = answer
        .output
        .iter()
        .fold(String::new(), |s, (k, v)| s + k + &v.to_string());
    assert!(
        !text.contains("fixture-secret"),
        "a secret value crossed the bridge"
    );

    // What Fabric can tell the worker would refuse is refused before it is
    // written: an action outside the profile, an observe of an action the
    // profile does not observe.
    let mut snapshot = create(31, "fixture/ok");
    snapshot.action = Action::Other {
        name: "sandbox.snapshot".into(),
        target: Some(fx("fx-seed")),
        params: serde_json::Map::new(),
    };
    let local = bridge.request(&mut journal, snapshot).await;
    let Err(BridgeError::RefusedLocally(r)) = local else {
        panic!("{local:?}")
    };
    assert_eq!((r.code, r.reason), ("unsupported", "action"));
    let entry31 = entry(&journal, id(31));
    assert_eq!(entry31.status, ProviderStatus::Rejected);
    assert_eq!(entry31.answer.effect.as_deref(), Some("none"));
    let observe = bridge
        .observe(&mut journal, id(27), "sandbox.exec", fx("fx-seed"), 1_000)
        .await;
    let Err(BridgeError::RefusedLocally(r)) = observe else {
        panic!("{observe:?}")
    };
    assert_eq!((r.code, r.reason), ("unsupported", "observe"));

    // The worker is still serving after every refusal.
    let ok = bridge
        .request(
            &mut journal,
            exec(32, &fx("fx-seed"), &["echo", "ok"], 1024),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(ok.status, ProviderStatus::Succeeded);
    assert_eq!(refused, [20, 22, 23, 24, 26, 27, 28, 29]);
    assert!(bridge.lost().is_none());
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}

/// A key the journal holds is never sent again, with the same operation
/// or another: the worker's own answer to a repeat (the
/// `duplicate_idempotency_key` scenario) is replayed over stdio above.
#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
async fn a_repeated_idempotency_key_is_never_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let path = write_config(dir.path(), "worker.json", &fixture_config(&[]));
    let bridge = spawn("fixture", &path).await;
    let first = bridge
        .request(&mut journal, create(40, "fixture/ok"))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(first.status, ProviderStatus::Succeeded);
    let before = entry(&journal, id(40));

    let again = bridge.request(&mut journal, create(40, "fixture/ok")).await;
    assert!(
        matches!(again, Err(BridgeError::Log(LogError::Used(_)))),
        "{again:?}"
    );
    let mut same_key = create(41, "fixture/ok");
    same_key.operation.key = OperationKey::parse("key-40").unwrap();
    let reused = bridge.request(&mut journal, same_key).await;
    assert!(
        matches!(reused, Err(BridgeError::Log(LogError::Used(_)))),
        "{reused:?}"
    );
    assert_eq!(entry(&journal, id(40)), before);
    assert!(journal.provider_operation(id(41)).unwrap().is_none());
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}

/// Worker loss is not cancellation and a restart is not permission to
/// replay: an exec in flight when the worker is killed stays `unknown`
/// with its locator, the job keeps its identity, the restarted worker is
/// never sent the operation again, and a create whose reply was lost is
/// reconciled by observing its original identity instead of creating it
/// twice.
#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
#[expect(clippy::too_many_lines, reason = "three workers in one story")]
async fn a_killed_worker_leaves_unknown_and_a_restart_sends_nothing_twice() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let path = write_config(dir.path(), "worker.json", &fixture_config(&[]));
    let bridge = spawn("fixture", &path).await;
    let long = fx("fx-long");

    let mut sleeping = exec(50, &long, &["sleep", "60"], 4096);
    sleeping.deadline_ms = 60_000;
    let mut flight = bridge
        .request(&mut journal, sleeping.clone())
        .await
        .unwrap();
    // The worker reports the locator while the exec runs; it is recorded.
    let event = flight.next(&mut journal).await.unwrap();
    assert_eq!(event, Event::Progress(long.clone()));
    assert_eq!(entry(&journal, id(50)).status, ProviderStatus::Dispatched);
    bridge.kill().await;
    let Event::Done(outcome) = flight.next(&mut journal).await.unwrap() else {
        panic!("a second progress")
    };
    assert!(
        matches!(outcome.answer, Answer::Lost(_)),
        "{:?}",
        outcome.answer
    );
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    let lost = entry(&journal, id(50));
    assert_eq!(lost.status, ProviderStatus::Unknown);
    assert_eq!(
        lost.locator,
        Some(stored(&long)),
        "the locator survives the loss"
    );
    let state = journal.job(scope().job).unwrap().unwrap();
    assert_eq!(state.job.generation, Generation::FIRST);
    let attempt = state.attempt(ATTEMPT.parse().unwrap()).unwrap();
    assert_eq!(
        attempt.status,
        AttemptStatus::Granted,
        "the attempt is untouched"
    );
    drop(bridge);

    // A restarted worker is a new connection. The operation is never sent
    // again, and the profile does not observe an exec: it stays unknown.
    let bridge = spawn("fixture", &path).await;
    let again = bridge.request(&mut journal, sleeping).await;
    assert!(
        matches!(again, Err(BridgeError::Log(LogError::Used(_)))),
        "{again:?}"
    );
    let observe = bridge
        .observe(&mut journal, id(50), "sandbox.exec", long.clone(), 1_000)
        .await;
    assert!(
        matches!(&observe, Err(BridgeError::RefusedLocally(r)) if r.reason == "observe"),
        "{observe:?}"
    );
    assert_eq!(entry(&journal, id(50)), lost, "unknown stays explicit");
    // The resource identity did not change with the worker.
    let still = bridge
        .request(
            &mut journal,
            exec(52, &fx("fx-seed"), &["echo", "after"], 1024),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(still.status, ProviderStatus::Succeeded);

    // A create applies and its reply is lost; then the worker is lost too.
    let unknown = bridge
        .request(&mut journal, create(51, "fixture/ok"))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(unknown.status, ProviderStatus::Unknown);
    bridge.kill().await;
    drop(bridge);

    // The fixture provider lives inside the worker process, so the
    // restarted worker is seeded with the sandbox the first one created:
    // the provider outlived the worker. (The Docker test below has a real
    // provider that does.) The create is not sent again; observing its
    // original identity finds the one sandbox.
    let seeded = write_config(
        dir.path(),
        "restarted.json",
        &fixture_config(&["fx-key-51"]),
    );
    let bridge = spawn("fixture", &seeded).await;
    let again = bridge.request(&mut journal, create(51, "fixture/ok")).await;
    assert!(
        matches!(again, Err(BridgeError::Log(LogError::Used(_)))),
        "{again:?}"
    );
    let observed = bridge
        .observe(
            &mut journal,
            id(51),
            "sandbox.create",
            fx("fx-key-51"),
            5_000,
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(observed.status, ProviderStatus::Succeeded);
    assert_eq!(
        entry(&journal, id(51)).locator,
        Some(stored(&fx("fx-key-51")))
    );
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}

/// Observe settles an operation only from its own resource, against a
/// worker that answers `completed` for a destroy of a resource that is
/// not there. A pending destroy observed by another locator or as another
/// action, and a lost create observed on another job's sandbox, are
/// refused before any frame; the destroy stays dispatched while its
/// target is still stopping. A destroy whose answer Fabric never read
/// keeps its target through a restart, and is observed by it.
#[tokio::test]
#[ignore = "needs the Capsule worker: set CAPSULE_BRIDGE_WORKER, run with --ignored"]
#[expect(
    clippy::too_many_lines,
    reason = "one journal through the refusals, a restart and the settlements"
)]
async fn observe_reads_only_the_operations_own_resource() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    let mut worker = fixture_config(&["fx-a", "fx-b"]);
    for native_id in ["fx-a", "fx-b"] {
        worker["approvals"].as_array_mut().unwrap().push(json!({
            "ref": format!("apr-destroy-{native_id}"),
            "action": "sandbox.destroy",
            "native_id": native_id,
            "expired": false,
        }));
    }
    worker["script"].as_array_mut().unwrap().push(json!({
        "action": "sandbox.destroy",
        "native_id": "fx-a",
        "behavior": "stay_stopping_until_settle",
    }));
    let path = write_config(dir.path(), "observe.json", &worker);
    let bridge = spawn("fixture", &path).await;

    let pending = bridge
        .request(
            &mut journal,
            destroy(60, &fx("fx-a"), Some("apr-destroy-fx-a")),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(pending.status, ProviderStatus::Dispatched);
    assert_eq!(refusal(&pending).0, ErrorCode::Pending);
    for (action, resource) in [
        ("sandbox.destroy", fx("fx-ghost")),
        ("sandbox.create", fx("fx-seed")),
    ] {
        let refused = bridge
            .observe(&mut journal, id(60), action, resource, 1_000)
            .await;
        assert!(
            matches!(refused, Err(BridgeError::ObserveMismatch { .. })),
            "{action}: {refused:?}"
        );
    }
    let own = bridge
        .observe(&mut journal, id(60), "sandbox.destroy", fx("fx-a"), 1_000)
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    // Read by its own target, the sandbox is still stopping: not gone.
    assert_eq!(own.status, ProviderStatus::Dispatched);
    assert_eq!(refusal(&own).0, ErrorCode::Pending);

    let lost = bridge
        .request(&mut journal, create(11, "fixture/ok"))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(lost.status, ProviderStatus::Unknown);
    let refused = bridge
        .observe(
            &mut journal,
            id(11),
            "sandbox.create",
            fx("fx-other-job"),
            1_000,
        )
        .await;
    assert!(
        matches!(refused, Err(BridgeError::ObserveMismatch { .. })),
        "{refused:?}"
    );
    assert_eq!(entry(&journal, id(11)).status, ProviderStatus::Unknown);
    let settled = bridge
        .observe(
            &mut journal,
            id(11),
            "sandbox.create",
            fx("fx-key-11"),
            1_000,
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(settled.status, ProviderStatus::Succeeded);

    // A destroy is sent and Fabric stops before reading its answer.
    let unread = bridge
        .request(
            &mut journal,
            destroy(62, &fx("fx-b"), Some("apr-destroy-fx-b")),
        )
        .await
        .unwrap();
    drop(unread);
    bridge.kill().await;
    drop(bridge);
    drop(journal.into_inner().await);
    let mut journal = Journal::open(&dir.path().join(fabric_journal::FILE_NAME)).unwrap();
    let recovery = journal
        .recover(1_770_000_000_000, |_| fabric_journal::Liveness::CannotTell)
        .unwrap();
    assert!(recovery.provider_unknown.contains(&(scope().job, id(62))));
    let row = entry(&journal, id(62));
    assert_eq!(row.status, ProviderStatus::Unknown);
    assert_eq!(row.dispatch.target, Some(stored(&fx("fx-b"))));
    let mut journal = Logged::new(journal);

    let bridge = spawn("fixture", &path).await;
    let refused = bridge
        .observe(
            &mut journal,
            id(62),
            "sandbox.destroy",
            fx("fx-ghost"),
            1_000,
        )
        .await;
    assert!(
        matches!(refused, Err(BridgeError::ObserveMismatch { .. })),
        "{refused:?}"
    );
    // The restarted fixture provider holds fx-b again, so it is not gone:
    // still unknown, never settled from another resource.
    let observed = bridge
        .observe(&mut journal, id(62), "sandbox.destroy", fx("fx-b"), 1_000)
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(observed.status, ProviderStatus::Unknown);
    assert_eq!(refusal(&observed).0, ErrorCode::Pending);
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}

// ---------------------------------------------------------------------------
// The Docker profile against the local engine.
// ---------------------------------------------------------------------------

fn docker_effect(scope: Scope, n: u8, action: Action, approval: Option<&str>) -> Effect {
    let mut digest = [0; 32];
    digest[0] = n;
    Effect {
        operation: OperationRef {
            id: OperationId::from_bytes(fresh(&format!("op-{n}"))),
            key: OperationKey::parse(&format!("docker-key-{n}")).unwrap(),
            digest: Digest(digest),
        },
        scope,
        action,
        approval: approval.map(str::to_owned),
        policy: None,
        deadline_ms: 60_000,
    }
}

fn docker_exec(
    scope: Scope,
    n: u8,
    target: &Locator,
    argv: &[&str],
    max_output_bytes: u64,
) -> Effect {
    docker_effect(
        scope,
        n,
        Action::Exec {
            target: target.clone(),
            params: ExecParams {
                argv: argv.iter().map(|a| (*a).to_owned()).collect(),
                stdin: None,
                cwd: None,
                timeout_ms: 60_000,
                max_output_bytes,
                secret_names: Vec::new(),
            },
        },
        None,
    )
}

/// Lines of `docker top` whose command is exactly `command`.
fn running(container: &str, command: &str) -> usize {
    docker(&["top", container])
        .unwrap_or_default()
        .lines()
        .filter(|line| line.trim_end().ends_with(&format!(" {command}")))
        .count()
}

/// The Docker profile through Fabric's client, on the local engine with an
/// image it already holds: refusals that leave nothing behind, a create
/// with its labels and scoped locator, exec and artifact reads within
/// bounds, a worker killed mid-exec that leaves the container running and
/// is not sent the exec again after a restart, and a destroy that needs
/// its own grant and is observed gone.
#[tokio::test]
#[ignore = "needs the Capsule worker and a local Docker engine: set CAPSULE_BRIDGE_WORKER and CAPSULE_BRIDGE_DOCKER_IMAGE, run with --ignored"]
#[expect(clippy::too_many_lines, reason = "one physical run, step by step")]
async fn the_docker_profile_on_the_local_engine() {
    let image = match docker_image() {
        Ok(image) => image,
        Err(why) if std::env::var_os(DOCKER_IMAGE_VAR).is_none() => {
            eprintln!("skipped: {why}");
            return;
        }
        Err(why) => panic!("{DOCKER_IMAGE_VAR} is set, but {why}"),
    };
    let server = docker(&["version", "--format", "{{.Server.Version}}"]).unwrap();
    let engine = docker(&["context", "show"]).unwrap().trim().to_owned();
    eprintln!(
        "docker server {}, context {engine}, image {image}",
        server.trim()
    );

    let scope = Scope {
        job: JobId::from_bytes(fresh("job")),
        task: TaskId::from_bytes(fresh("task")),
        attempt: AttemptId::from_bytes(fresh("attempt")),
        generation: Generation::FIRST,
    };
    let label = format!("label=fabric.job={}", scope.job);
    let _cleanup = Cleanup(label.clone());
    let containers = || {
        docker(&["ps", "-a", "-q", "--filter", &label])
            .unwrap()
            .split_whitespace()
            .count()
    };
    let missing = format!(
        "capsule-fabric-missing:{}",
        scope.task.to_string().to_lowercase()
    );
    assert!(docker(&["image", "inspect", &missing]).is_none());

    let dir = tempfile::tempdir().unwrap();
    let mut journal = Logged::new(journal_for(dir.path(), scope));
    let path = write_config(
        dir.path(),
        "docker.json",
        &json!({
            "policy": {"images": [image, missing]},
            "approvals": [{"ref": "apr-destroy", "action": "sandbox.destroy", "native_id": null}]
        }),
    );
    let started = Instant::now();
    let bridge = spawn("docker", &path).await;
    eprintln!("spawn and handshake: {:?}", started.elapsed());
    assert_eq!(*bridge.profile(), profile("docker"));
    let make = |n: u8, image: &str| {
        docker_effect(
            scope,
            n,
            Action::Create(CreateParams {
                image: image.to_owned(),
                env: BTreeMap::new(),
                secret_names: Vec::new(),
                labels: BTreeMap::from([("purpose".to_owned(), "fabric-interop".to_owned())]),
            }),
            None,
        )
    };

    // An image outside the worker's policy: refused, nothing created.
    let denied = bridge
        .request(&mut journal, make(1, "fabric-interop-denied:latest"))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(denied.status, ProviderStatus::Rejected);
    assert_eq!(
        refusal(&denied),
        (ErrorCode::Denied, "policy_denied", EffectState::None)
    );
    assert_eq!(containers(), 0);

    // An allowed image the engine lacks: a definite failure, never a pull.
    let absent = bridge
        .request(&mut journal, make(2, &missing))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(absent.status, ProviderStatus::Failed);
    let (code, _, effect) = refusal(&absent);
    assert_eq!(
        (code, effect),
        (ErrorCode::ProviderFailed, EffectState::None)
    );
    assert!(
        docker(&["image", "inspect", &missing]).is_none(),
        "the image was pulled"
    );
    assert_eq!(containers(), 0);

    // Create: a running container with this operation's labels.
    let creation = make(3, &image);
    let creation_id = creation.operation.id;
    let created = bridge
        .request(&mut journal, creation)
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(created.status, ProviderStatus::Succeeded);
    let container = result(&created).resource.clone();
    assert_eq!(container.provider, "docker");
    assert_eq!(container.kind, "container");
    assert_eq!(
        container.scope,
        BTreeMap::from([("engine".to_owned(), engine.clone())])
    );
    assert_eq!(
        entry(&journal, creation_id).locator,
        Some(stored(&container))
    );
    let cid = container.native_id.clone();
    let labels: BTreeMap<String, String> = serde_json::from_str(
        &docker(&["inspect", "--format", "{{json .Config.Labels}}", &cid]).unwrap(),
    )
    .unwrap();
    assert_eq!(labels["fabric.operation"], creation_id.to_string());
    assert_eq!(labels["fabric.job"], scope.job.to_string());
    assert_eq!(labels["fabric.attempt"], scope.attempt.to_string());
    assert_eq!(labels["fabric.generation"], "1");
    assert_eq!(labels["purpose"], "fabric-interop");
    assert_eq!(containers(), 1);

    // Write an artifact, read it back within bounds, and a larger one
    // capped while it is read.
    let run = |n: u8, argv: &[&str], cap: u64| docker_exec(scope, n, &container, argv, cap);
    let wrote = bridge
        .request(
            &mut journal,
            run(
                4,
                &["sh", "-c", "printf artifact-body > /tmp/artifact.txt"],
                4096,
            ),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(result(&wrote).output["exit_code"], 0);
    let read = bridge
        .request(&mut journal, run(5, &["cat", "/tmp/artifact.txt"], 4096))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    let out = stream(result(&read), "stdout");
    assert_eq!(out["text"], "artifact-body");
    assert_eq!(out["truncated"], false);
    assert_eq!(out["captured_bytes"], 13);
    let big = bridge
        .request(
            &mut journal,
            run(
                6,
                &["sh", "-c", "head -c 200000 /dev/zero | tr '\\000' a"],
                1024,
            ),
        )
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    let out = stream(result(&big), "stdout");
    assert_eq!(out["truncated"], true);
    assert_eq!(out["captured_bytes"], 1024);
    assert_eq!(out["text"].as_str().unwrap(), "a".repeat(1024));
    let nonzero = bridge
        .request(&mut journal, run(7, &["sh", "-c", "exit 7"], 1024))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(nonzero.status, ProviderStatus::Succeeded);
    assert_eq!(result(&nonzero).output["exit_code"], 7);

    // Kill the worker while an exec runs in the container.
    let sleeping = run(8, &["sleep", "20"], 1024);
    let sleeping_id = sleeping.operation.id;
    let flight = bridge
        .request(&mut journal, sleeping.clone())
        .await
        .unwrap();
    let waited = Instant::now();
    while running(&cid, "sleep 20") == 0 {
        assert!(
            waited.elapsed() < Duration::from_secs(15),
            "the exec never started"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bridge.kill().await;
    let outcome = flight.finish(&mut journal).await.unwrap();
    assert!(
        matches!(outcome.answer, Answer::Lost(_)),
        "{:?}",
        outcome.answer
    );
    assert_eq!(outcome.status, ProviderStatus::Unknown);
    assert_eq!(entry(&journal, sleeping_id).status, ProviderStatus::Unknown);
    assert_eq!(
        docker(&["inspect", "--format", "{{.State.Running}}", &cid])
            .unwrap()
            .trim(),
        "true",
        "worker loss is not cancellation: the container runs on"
    );
    let state = journal.job(scope.job).unwrap().unwrap();
    assert_eq!(state.job.generation, Generation::FIRST);
    drop(bridge);

    // A restarted worker is never sent the exec again: one sleep runs.
    let bridge = spawn("docker", &path).await;
    let again = bridge.request(&mut journal, sleeping).await;
    assert!(
        matches!(again, Err(BridgeError::Log(LogError::Used(_)))),
        "{again:?}"
    );
    assert_eq!(running(&cid, "sleep 20"), 1, "the exec ran once");
    assert_eq!(containers(), 1, "and there is one container");
    let after = bridge
        .request(&mut journal, run(9, &["echo", "still-here"], 1024))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(stream(result(&after), "stdout")["text"], "still-here\n");

    // Destroy without a grant changes nothing; with its own grant the
    // container is observed gone.
    let target = container.clone();
    let remove = |n: u8, approval: Option<&str>| {
        docker_effect(
            scope,
            n,
            Action::Destroy {
                target: target.clone(),
            },
            approval,
        )
    };
    let ungranted = bridge
        .request(&mut journal, remove(10, None))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(ungranted.status, ProviderStatus::Rejected);
    assert_eq!(
        refusal(&ungranted),
        (ErrorCode::Denied, "approval_required", EffectState::None)
    );
    assert!(
        docker(&["inspect", &cid]).is_some(),
        "a refused destroy removed it"
    );
    let gone = bridge
        .request(&mut journal, remove(11, Some("apr-destroy")))
        .await
        .unwrap()
        .finish(&mut journal)
        .await
        .unwrap();
    assert_eq!(gone.status, ProviderStatus::Succeeded);
    assert_eq!(result(&gone).output["status"], "destroyed");
    assert!(
        docker(&["inspect", &cid]).is_none(),
        "the container is still there"
    );
    assert_eq!(containers(), 0);
    assert_eq!(bridge.close().await.unwrap().code(), Some(0));
}
