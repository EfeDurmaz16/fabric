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
//!   reports progress, then exits; with `double/silent` it never answers;
//!   with `double/oversize` it declares a frame above the cap; with
//!   `double/running` it answers a running result, which a terminal-only
//!   action never may; with
//!   `double/other-operation` it answers under another operation id.
//! - `sandbox.exec` echoes its argv; `sandbox.destroy` needs an approval.
//! - A repeated key with the same digest replays the first answer; with
//!   another digest it is an idempotency conflict.
//!
//! `FABRIC_BRIDGE_DOUBLE_HELLO` scripts the handshake: `ok` (the default),
//! `raise_limit`, `wrong_profile`, `oversize`, `silent` or `exit`.

use std::collections::HashMap;
use std::process::ExitCode;

use fabric_bridge::frames;
use fabric_bridge::wire::{
    Action, CancelState, EffectState, ErrorBody, ErrorCode, ErrorMessage, Limits, Locator, Phase,
    Profile, Refusal, Request, ResultMessage, ResultState, ToFabric, ToWorker, Unresolved,
    Versions,
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
}

fn profiles() -> Option<Vec<Profile>> {
    [FIXTURE, DOCKER]
        .iter()
        .map(|text| {
            serde_json::from_str::<Value>(text)
                .ok()
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
            "double/crash" => Step::Exit(3),
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
        Action::Exec { target, params } => {
            let text = format!("{}\n", params.argv.join(" "));
            Step::Answer(completed(
                request,
                target.clone(),
                output(&[
                    ("exit_code", 0.into()),
                    ("stdout", stream(&text)),
                    ("stderr", stream("")),
                ]),
            ))
        }
        Action::Destroy { target } => {
            if request.approval.is_none() {
                Step::Answer(error(
                    &request.request_id,
                    request.operation.id,
                    ErrorCode::Denied,
                    "approval_required",
                ))
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
        match script(request) {
            Step::Answer(message) => {
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
                    let status = if observe.action == "sandbox.destroy" {
                        "destroyed"
                    } else {
                        "ready"
                    };
                    let message = result(
                        observe.request_id,
                        observe.operation_id,
                        ResultState::Completed,
                        observe.resource,
                        output(&[("status", status.into())]),
                    );
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

async fn run() -> u8 {
    let Some(profiles) = profiles() else {
        return 70;
    };
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
    let mut worker = Worker {
        stdin,
        stdout,
        profile,
        limits,
        answered: HashMap::new(),
        silent: Vec::new(),
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
