//! A worker that speaks bridge protocol v1 from a script, for the bridge
//! tests and the bench. It never calls a provider: its "provider" is the
//! fixture profile's locators, made up in memory.
//!
//! It parses every frame with the crate's own worker-side parser, so it
//! refuses as the protocol says. What it does for a request is scripted by
//! the request itself, so one binary serves every test:
//!
//! - `sandbox.create` with image `fixture/ok` makes `fx-<key>`; with
//!   `fixture/forbidden` it is denied before any effect; with
//!   `double/crash` it exits mid-request; with `double/progress-crash` it
//!   reports progress, then exits; with `double/progress-16` it reports
//!   progress 1 to 16, then answers; with `double/progress-repeat` it
//!   reports progress 1 twice, then answers; with `double/late` it answers
//!   after 400 ms, reading nothing meanwhile; with `double/silent` it
//!   never answers; with `double/oversize` it declares a frame above the
//!   cap; with `double/running` it answers a running result, which a
//!   terminal-only action never may; with `double/other-operation` it
//!   answers under another operation id.
//! - `sandbox.exec` echoes its argv, except `double/running`, which it
//!   answers `running` and keeps running; `sandbox.destroy` needs an
//!   approval, and of `fx-stopping` it stays stopping: `pending` with its
//!   target.
//! - `observe` of an exec answers that exec's `completed` result once it
//!   ran here (a running one ends when observed), and `unknown` for an exec
//!   this process did not run, as a restarted worker would. Any other
//!   observe answers `completed` for whatever resource it names, as a
//!   destroy observe of a resource that is not there would.
//! - A repeated key with the same digest replays the first answer; with
//!   another digest it is an idempotency conflict.
//! - `sandbox.create` of `double/made-crash` makes `fx-<key>`, then exits
//!   before it answers, as a worker lost after its provider acted.
//! - `attach` finds `fx-<key>` when a create of that key made it:
//!   `completed` when that create was the attaching operation, else
//!   `denied`/`owner_mismatch`; it answers `absent` with the scope it
//!   searched when no create made it.
//!
//! `FABRIC_BRIDGE_DOUBLE_PROVIDER` names a directory the double's
//! "provider" keeps what it made in, one file per sandbox holding the
//! operation that made it, so a restarted double attaches what an earlier
//! one made, as a provider outlives its worker.
//!
//! `FABRIC_BRIDGE_DOUBLE_HELLO` scripts the handshake: `ok` (the default),
//! `raise_limit`, `wrong_profile`, `oversize`, `silent` or `exit`; with
//! `stall` it answers hello and never reads its input again, alive.
//!
//! `FABRIC_BRIDGE_DOUBLE_CHILDREN` names a file: before the handshake the
//! double starts two `sleep` processes and writes its own pid and theirs
//! there. The first stays in the double's process group, as an attached
//! `docker exec` client stays in a worker's; the second leads a group of
//! its own, as a provider effect that must outlive the worker does.

use std::collections::HashMap;
use std::process::ExitCode;

use fabric_bridge::frames;
use fabric_bridge::wire::{
    Action, Attach, CancelState, EffectState, ErrorBody, ErrorCode, ErrorMessage, Limits, Locator,
    Phase, Profile, Refusal, Request, ResultMessage, ResultState, Searched, ToFabric, ToWorker,
    Unresolved, Versions,
};
use fabric_core::managed::OperationId;
use serde_json::{Map, Value};
use tokio::io::{AsyncWriteExt, Stdin, Stdout};

const FIXTURE: &str = include_str!("../../../../protocol/bridge/v1/profiles/fixture.json");
const DOCKER: &str = include_str!("../../../../protocol/bridge/v1/profiles/docker.json");

struct Worker {
    stdin: Stdin,
    stdout: Stdout,
    profile: Profile,
    limits: Limits,
    answered: HashMap<String, (String, ToFabric)>,
    silent: Vec<(String, OperationId, Locator)>,
    /// The execs this process ran, by operation: their argv.
    execs: HashMap<OperationId, Vec<String>>,
    /// The sandboxes the provider made, by key: the operation that made each.
    made: Provider,
}

/// What the double's provider made, in memory, and in
/// `FABRIC_BRIDGE_DOUBLE_PROVIDER` when it names a directory.
struct Provider {
    made: HashMap<String, OperationId>,
    dir: Option<std::path::PathBuf>,
}

impl Provider {
    fn open() -> Provider {
        let dir = std::env::var_os("FABRIC_BRIDGE_DOUBLE_PROVIDER").map(std::path::PathBuf::from);
        let mut made = HashMap::new();
        if let Some(entries) = dir.as_ref().and_then(|d| std::fs::read_dir(d).ok()) {
            for entry in entries.flatten() {
                let key = entry.file_name().to_string_lossy().into_owned();
                let operation = std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|text| text.trim().parse().ok());
                if let Some(operation) = operation {
                    made.insert(key, operation);
                }
            }
        }
        Provider { made, dir }
    }

    /// Makes `fx-<key>` for `operation`, once: a repeated key keeps the first.
    fn make(&mut self, key: &str, operation: OperationId) {
        if self.made.contains_key(key) {
            return;
        }
        self.made.insert(key.to_owned(), operation);
        if let Some(dir) = &self.dir {
            let _ = std::fs::write(dir.join(key), operation.to_string());
        }
    }
}

// The v1 correction of 2026-11-02 gives every profile a `max_timeout_ms`,
// the bound on an exec's `timeout_ms` advertised in `hello_ack`. The
// on-disk profile carries it once the Ship step copies the Capsule-written
// fixtures; until then this adds the bound v1 names for the fixture
// profile, 60000 ms, when it is absent. Remove with that copy.
fn with_timeout_bound(mut value: Value) -> Value {
    if let Some(limits) = value["limits"].as_object_mut() {
        limits
            .entry("max_timeout_ms")
            .or_insert_with(|| 60_000.into());
    }
    value
}

fn profiles() -> Option<Vec<Profile>> {
    [FIXTURE, DOCKER]
        .iter()
        .map(|text| {
            serde_json::from_str::<Value>(text)
                .ok()
                .map(with_timeout_bound)
                .and_then(|v| Profile::from_value(&v).ok())
        })
        .collect()
}

async fn send(stdout: &mut Stdout, message: &ToFabric) -> bool {
    match frames::encode(&message.to_value(), 16 << 20) {
        Ok(frame) => frames::write(stdout, &frame).await.is_ok(),
        Err(_) => false,
    }
}

fn locator(key: &str) -> Locator {
    Locator {
        provider: "fixture".into(),
        scope: [
            ("account".to_owned(), "acct-a".to_owned()),
            ("project".to_owned(), "proj-1".to_owned()),
        ]
        .into_iter()
        .collect(),
        kind: "sandbox".into(),
        native_id: format!("fx-{key}"),
        native_operation_id: None,
    }
}

fn error(request_id: &str, operation: OperationId, code: ErrorCode, reason: &str) -> ToFabric {
    ToFabric::Error(ErrorMessage {
        request_id: request_id.to_owned(),
        operation_id: operation,
        error: ErrorBody {
            code,
            reason: reason.to_owned(),
            message: format!("{}: {reason}", code.name()),
            effect: EffectState::None,
            resource: None,
            state: None,
            observations: None,
            deadline_ms: None,
            cleanup: None,
            searched: None,
        },
    })
}

/// A terminal-only action not terminal by its deadline: `pending` with
/// the resource it acts on.
fn pending(request: &Request, resource: &Locator) -> ToFabric {
    ToFabric::Error(ErrorMessage {
        request_id: request.request_id.clone(),
        operation_id: request.operation.id,
        error: ErrorBody {
            code: ErrorCode::Pending,
            reason: "observation_deadline".to_owned(),
            message: "pending: observation_deadline".to_owned(),
            effect: EffectState::Pending,
            resource: Some(resource.clone()),
            state: Some("stopping".to_owned()),
            observations: None,
            deadline_ms: None,
            cleanup: None,
            searched: None,
        },
    })
}

fn output(fields: &[(&str, Value)]) -> Map<String, Value> {
    fields
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn result(
    request_id: String,
    operation_id: OperationId,
    state: ResultState,
    resource: Locator,
    out: Map<String, Value>,
) -> ToFabric {
    ToFabric::Result(ResultMessage {
        request_id,
        operation_id,
        state,
        resource,
        output: out,
        replayed: false,
        receipts: Vec::new(),
        evidence_error: None,
    })
}

fn completed(request: &Request, resource: Locator, out: Map<String, Value>) -> ToFabric {
    result(
        request.request_id.clone(),
        request.operation.id,
        ResultState::Completed,
        resource,
        out,
    )
}

/// What an exec of `argv` printed: its argv.
fn exec_output(argv: &[String]) -> Map<String, Value> {
    let text = format!("{}\n", argv.join(" "));
    output(&[
        ("exit_code", 0.into()),
        ("stdout", stream(&text)),
        ("stderr", stream("")),
    ])
}

fn stream(text: &str) -> Value {
    serde_json::json!({
        "text": text,
        "captured_bytes": text.len(),
        "truncated": false,
        "lossy": false,
    })
}

/// What one request does: an answer, no answer, or the worker's end.
enum Step {
    Answer(ToFabric),
    /// `progress` frames with these `seq` values, then the answer.
    Progressing(Vec<u64>, ToFabric),
    /// The answer, this many milliseconds late.
    Late(u64, ToFabric),
    Silent(Locator),
    Exit(u8),
    Oversize,
    Progress(ToFabric, u8),
}

fn script(request: &Request) -> Step {
    let key = request.operation.key.as_str();
    let ready = || output(&[("status", "ready".into())]);
    match &request.action {
        Action::Create(params) => match params.image.as_str() {
            "fixture/forbidden" => Step::Answer(error(
                &request.request_id,
                request.operation.id,
                ErrorCode::Denied,
                "policy_denied",
            )),
            "double/crash" | "double/made-crash" => Step::Exit(3),
            "double/silent" => Step::Silent(locator(key)),
            "double/oversize" => Step::Oversize,
            "double/progress-crash" => Step::Progress(
                ToFabric::Progress {
                    request_id: request.request_id.clone(),
                    operation_id: request.operation.id,
                    seq: 1,
                    state: "starting".into(),
                    resource: locator(key),
                },
                3,
            ),
            "double/progress-16" => Step::Progressing(
                (1..=16).collect(),
                completed(request, locator(key), ready()),
            ),
            "double/progress-repeat" => {
                Step::Progressing(vec![1, 1], completed(request, locator(key), ready()))
            }
            "double/late" => Step::Late(400, completed(request, locator(key), ready())),
            "double/running" => Step::Answer(result(
                request.request_id.clone(),
                request.operation.id,
                ResultState::Running,
                locator(key),
                ready(),
            )),
            "double/other-operation" => Step::Answer(result(
                request.request_id.clone(),
                OperationId::from_bytes([0xee; 16]),
                ResultState::Completed,
                locator(key),
                ready(),
            )),
            _ => Step::Answer(completed(request, locator(key), ready())),
        },
        Action::Exec { target, params }
            if params.argv.first().is_some_and(|a| a == "double/running") =>
        {
            Step::Answer(result(
                request.request_id.clone(),
                request.operation.id,
                ResultState::Running,
                target.clone(),
                output(&[
                    ("exit_code", Value::Null),
                    ("stdout", stream("")),
                    ("stderr", stream("")),
                ]),
            ))
        }
        Action::Exec { target, params } => Step::Answer(completed(
            request,
            target.clone(),
            exec_output(&params.argv),
        )),
        Action::Destroy { target } => {
            if request.approval.is_none() {
                Step::Answer(error(
                    &request.request_id,
                    request.operation.id,
                    ErrorCode::Denied,
                    "approval_required",
                ))
            } else if target.native_id == "fx-stopping" {
                Step::Answer(pending(request, target))
            } else {
                Step::Answer(completed(
                    request,
                    target.clone(),
                    output(&[("status", "destroyed".into())]),
                ))
            }
        }
        Action::Other { .. } => Step::Answer(error(
            &request.request_id,
            request.operation.id,
            ErrorCode::Unsupported,
            "action",
        )),
    }
}

fn rename(message: &ToFabric, request_id: &str) -> ToFabric {
    match message.clone() {
        ToFabric::Result(mut r) => {
            request_id.clone_into(&mut r.request_id);
            r.replayed = true;
            ToFabric::Result(r)
        }
        ToFabric::Error(mut e) => {
            request_id.clone_into(&mut e.request_id);
            ToFabric::Error(e)
        }
        other => other,
    }
}

/// Declares a frame one byte above `cap`, then hangs, as a broken worker would.
async fn oversize(stdout: &mut Stdout, cap: u32) {
    let _ = stdout.write_all(&cap.saturating_add(1).to_le_bytes()).await;
    let _ = stdout.flush().await;
    std::future::pending::<()>().await;
}

impl Worker {
    async fn refuse(&mut self, value: &Value, refusal: &Refusal) -> Option<u8> {
        if refusal.code == "protocol_error" || refusal.code == "hello_refused" {
            let message = ToFabric::ProtocolError {
                code: "unexpected".to_owned(),
                message: refusal.detail.clone(),
            };
            send(&mut self.stdout, &message).await;
            return Some(2);
        }
        let request_id = value["request_id"].as_str().unwrap_or("r-0").to_owned();
        let operation = value["operation"]["id"]
            .as_str()
            .or_else(|| value["operation_id"].as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(OperationId::from_bytes([0; 16]));
        let code = if refusal.code == "unsupported" {
            ErrorCode::Unsupported
        } else {
            ErrorCode::Invalid
        };
        let message = error(&request_id, operation, code, refusal.reason);
        (!send(&mut self.stdout, &message).await).then_some(1)
    }

    async fn request(&mut self, request: &Request) -> Option<u8> {
        let key = request.operation.key.as_str().to_owned();
        let digest = request.operation.digest.to_string();
        if let Some((seen, answer)) = self.answered.get(&key) {
            let message = if *seen == digest {
                rename(answer, &request.request_id)
            } else {
                error(
                    &request.request_id,
                    request.operation.id,
                    ErrorCode::Invalid,
                    "idempotency_conflict",
                )
            };
            return (!send(&mut self.stdout, &message).await).then_some(1);
        }
        match &request.action {
            Action::Exec { params, .. } => {
                self.execs.insert(request.operation.id, params.argv.clone());
            }
            Action::Create(params)
                if matches!(params.image.as_str(), "fixture/ok" | "double/made-crash") =>
            {
                self.made.make(&key, request.operation.id);
            }
            _ => {}
        }
        match script(request) {
            Step::Answer(message) => {
                self.answered.insert(key, (digest, message.clone()));
                (!send(&mut self.stdout, &message).await).then_some(1)
            }
            Step::Progressing(seqs, message) => {
                for seq in seqs {
                    let progress = ToFabric::Progress {
                        request_id: request.request_id.clone(),
                        operation_id: request.operation.id,
                        seq,
                        state: "starting".into(),
                        resource: locator(&key),
                    };
                    if !send(&mut self.stdout, &progress).await {
                        return Some(1);
                    }
                }
                self.answered.insert(key, (digest, message.clone()));
                (!send(&mut self.stdout, &message).await).then_some(1)
            }
            Step::Late(ms, message) => {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                self.answered.insert(key, (digest, message.clone()));
                (!send(&mut self.stdout, &message).await).then_some(1)
            }
            Step::Silent(resource) => {
                self.silent
                    .push((request.request_id.clone(), request.operation.id, resource));
                None
            }
            Step::Exit(code) => Some(code),
            Step::Oversize => {
                oversize(&mut self.stdout, 16 << 20).await;
                None
            }
            Step::Progress(message, code) => {
                send(&mut self.stdout, &message).await;
                Some(code)
            }
        }
    }

    fn observe(&self, observe: fabric_bridge::wire::Observe) -> ToFabric {
        let out = match observe.action.as_str() {
            "sandbox.exec" => match self.execs.get(&observe.operation_id) {
                Some(argv) => exec_output(argv),
                None => {
                    return ToFabric::Error(ErrorMessage {
                        request_id: observe.request_id,
                        operation_id: observe.operation_id,
                        error: ErrorBody {
                            code: ErrorCode::Unknown,
                            reason: "observation_failed".to_owned(),
                            message: "unknown: observation_failed".to_owned(),
                            effect: EffectState::Unknown,
                            resource: Some(observe.resource),
                            state: None,
                            observations: None,
                            deadline_ms: None,
                            cleanup: None,
                            searched: None,
                        },
                    });
                }
            },
            "sandbox.destroy" => output(&[("status", "destroyed".into())]),
            _ => output(&[("status", "ready".into())]),
        };
        result(
            observe.request_id,
            observe.operation_id,
            ResultState::Completed,
            observe.resource,
            out,
        )
    }

    /// Finds `fx-<key>` by the attaching operation's identity, as a
    /// `provider_key` profile does.
    fn attach(&self, attach: Attach) -> ToFabric {
        let key = attach.key.as_str();
        match self.made.made.get(key) {
            Some(made) if *made == attach.operation_id => result(
                attach.request_id,
                attach.operation_id,
                ResultState::Completed,
                locator(key),
                output(&[("status", "ready".into())]),
            ),
            Some(_) => error(
                &attach.request_id,
                attach.operation_id,
                ErrorCode::Denied,
                "owner_mismatch",
            ),
            None => {
                let here = locator(key);
                ToFabric::Error(ErrorMessage {
                    request_id: attach.request_id,
                    operation_id: attach.operation_id,
                    error: ErrorBody {
                        code: ErrorCode::Unknown,
                        reason: "absent".to_owned(),
                        message: "unknown: absent".to_owned(),
                        effect: EffectState::Unknown,
                        resource: None,
                        state: None,
                        observations: Some(1),
                        deadline_ms: Some(attach.wait_ms),
                        cleanup: None,
                        searched: Some(Searched {
                            provider: here.provider,
                            scope: here.scope,
                            kind: here.kind,
                        }),
                    },
                })
            }
        }
    }

    async fn cancel(&mut self, request_id: String, cancels: String) -> Option<u8> {
        let found = self.silent.iter().position(|(id, _, _)| *id == cancels);
        let state = if found.is_some() {
            CancelState::Requested
        } else {
            CancelState::NotFound
        };
        let ack = ToFabric::CancelAck {
            request_id,
            cancels,
            state,
        };
        if !send(&mut self.stdout, &ack).await {
            return Some(1);
        }
        if let Some(at) = found {
            let (request_id, operation_id, resource) = self.silent.remove(at);
            let message = result(
                request_id,
                operation_id,
                ResultState::Cancelled,
                resource,
                output(&[("status", "cancelled".into())]),
            );
            if !send(&mut self.stdout, &message).await {
                return Some(1);
            }
        }
        None
    }

    async fn session(&mut self) -> u8 {
        loop {
            let cap = usize::try_from(self.limits.max_request_bytes).unwrap_or(usize::MAX);
            let value = match frames::read(&mut self.stdin, cap).await {
                Ok(Some(value)) => value,
                Ok(None) => return 0,
                Err(fault) => {
                    let message = ToFabric::ProtocolError {
                        code: fault.code().to_owned(),
                        message: fault.to_string(),
                    };
                    send(&mut self.stdout, &message).await;
                    return 2;
                }
            };
            let parsed = ToWorker::parse_as_worker(
                &value,
                Phase::Session,
                Versions::V1,
                std::slice::from_ref(&self.profile),
                &self.profile,
            );
            let exit = match parsed {
                Err(refusal) => self.refuse(&value, &refusal).await,
                Ok(ToWorker::Request(request)) => self.request(&request).await,
                Ok(ToWorker::Observe(observe)) => {
                    let message = self.observe(observe);
                    (!send(&mut self.stdout, &message).await).then_some(1)
                }
                Ok(ToWorker::Attach(attach)) => {
                    let message = self.attach(attach);
                    (!send(&mut self.stdout, &message).await).then_some(1)
                }
                Ok(ToWorker::Cancel(cancel)) => {
                    self.cancel(cancel.request_id, cancel.cancels).await
                }
                Ok(ToWorker::Shutdown { request_id, .. }) => {
                    let unresolved = self
                        .silent
                        .drain(..)
                        .map(|(request_id, operation_id, resource)| Unresolved {
                            request_id,
                            operation_id,
                            state: EffectState::Unknown,
                            resource,
                        })
                        .collect();
                    let ack = ToFabric::ShutdownAck {
                        request_id,
                        unresolved,
                    };
                    send(&mut self.stdout, &ack).await;
                    return 0;
                }
                Ok(ToWorker::Hello(_)) => Some(2),
            };
            if let Some(code) = exit {
                return code;
            }
        }
    }
}

/// Starts the two `sleep` processes `FABRIC_BRIDGE_DOUBLE_CHILDREN` asks
/// for and writes `<double> <in group> <own group>` pids to its file.
fn children() -> Result<(), std::io::Error> {
    use std::os::unix::process::CommandExt;
    let Some(path) = std::env::var_os("FABRIC_BRIDGE_DOUBLE_CHILDREN") else {
        return Ok(());
    };
    let sleep = || {
        let mut command = std::process::Command::new("sleep");
        command
            .arg("600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command
    };
    let client = sleep().spawn()?;
    let effect = sleep().process_group(0).spawn()?;
    let pids = format!("{} {} {}\n", std::process::id(), client.id(), effect.id());
    let partial = std::path::PathBuf::from(&path).with_extension("partial");
    std::fs::write(&partial, pids)?;
    std::fs::rename(partial, path)
}

async fn run() -> u8 {
    let Some(profiles) = profiles() else {
        return 70;
    };
    if children().is_err() {
        return 71;
    }
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let script = std::env::var("FABRIC_BRIDGE_DOUBLE_HELLO").unwrap_or_default();
    let value = match frames::read(&mut stdin, 1 << 20).await {
        Ok(Some(value)) => value,
        Ok(None) => return 0,
        Err(_) => return 2,
    };
    let hello = match ToWorker::parse_as_worker(
        &value,
        Phase::Handshake,
        Versions::V1,
        &profiles,
        &profiles[0],
    ) {
        Ok(ToWorker::Hello(hello)) => hello,
        Err(refusal) if refusal.code == "hello_refused" => {
            let message = ToFabric::HelloRefused {
                versions: Versions::V1,
                code: refusal.reason.to_owned(),
                message: "the double speaks bridge protocol 1 to 1".into(),
            };
            send(&mut stdout, &message).await;
            return 2;
        }
        _ => return 2,
    };
    let Some(profile) = profiles
        .iter()
        .find(|p| p.profile == hello.profile)
        .cloned()
    else {
        return 2;
    };
    let mut limits = hello.limits;
    limits.max_in_flight = limits.max_in_flight.min(4);
    let mut ack_profile = profile.clone();
    match script.as_str() {
        "raise_limit" => limits.max_in_flight = hello.limits.max_in_flight + 1,
        "wrong_profile" => {
            if let Some(other) = profiles.iter().find(|p| p.profile != hello.profile) {
                ack_profile = other.clone();
            }
        }
        "oversize" => oversize(&mut stdout, 1 << 20).await,
        "silent" => std::future::pending::<()>().await,
        "exit" => return 4,
        _ => {}
    }
    let ack = ToFabric::HelloAck {
        protocol: 1,
        worker: "fabric-bridge-double/0.1.0".into(),
        profile: ack_profile,
        limits,
    };
    if !send(&mut stdout, &ack).await {
        return 1;
    }
    if script == "stall" {
        std::future::pending::<()>().await;
    }
    let mut worker = Worker {
        stdin,
        stdout,
        profile,
        limits,
        answered: HashMap::new(),
        silent: Vec::new(),
        execs: HashMap::new(),
        made: Provider::open(),
    };
    worker.session().await
}

fn main() -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::from(70);
    };
    ExitCode::from(runtime.block_on(run()))
}
