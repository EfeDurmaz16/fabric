//! The span rules: the ids the Capsule worker links to, the endpoint
//! forms, and that no secret, raw key, argv, path or message text leaves
//! the node in a span, against a receiver that records every post.

use std::path::Path;
use std::sync::{Arc, Mutex};

use fabric_client::api::Request;
use fabric_core::harness::HarnessKind;
use fabric_core::id::NodeId;
use fabric_core::managed::launch::Program;
use fabric_core::managed::{
    Command, Envelope, Intent, JobPolicy, OperationId, OperationKey, RestartPolicy, SourceBinding,
    StoredResult, TaskClass, TaskKind,
};
use fabric_core::path::AbsPath;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::span::{body, digest, operation_context};
use super::*;
use crate::authority::Authority;
use crate::error::NodeError;
use crate::paths::StateDir;

const NODE: NodeId = NodeId::from_bytes([7; 32]);

/// A value that must reach no span.
const SECRET: &str = "s3cr3t_value_9f1c2e";
/// An idempotency key shaped like an approval ref: only its digest may leave.
const KEY: &str = "approval_ref_raw_77";
/// A repository path on the node.
const REPO: &str = "/private/var/fabric-otel-test/private-repo";

/// An OTLP receiver on a free port: every body posted to it, parsed.
async fn receiver() -> (OtlpEndpoint, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buf = [0_u8; 4096];
                let body = loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0, "the post ended early");
                    bytes.extend_from_slice(&buf[..n]);
                    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = std::str::from_utf8(&bytes[..end]).unwrap().to_owned();
                    assert!(head.starts_with("POST /v1/traces HTTP/1.1"), "{head}");
                    assert!(head.contains("Content-Type: application/json"), "{head}");
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + len {
                        break bytes[end + 4..end + 4 + len].to_vec();
                    }
                };
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .await
                    .unwrap();
                seen.lock()
                    .unwrap()
                    .push(serde_json::from_slice(&body).unwrap());
            });
        }
    });
    (format!("http://{addr}").parse().unwrap(), bodies)
}

/// Every span the receiver got.
fn spans(bodies: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
    bodies
        .lock()
        .unwrap()
        .iter()
        .flat_map(|b| {
            b["resourceSpans"][0]["scopeSpans"][0]["spans"]
                .as_array()
                .unwrap()
                .clone()
        })
        .collect()
}

fn attr<'a>(span: &'a Value, key: &str) -> Option<&'a Value> {
    span["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["key"] == key)
        .map(|a| &a["value"])
}

#[test]
fn an_operation_s_ids_are_the_ones_the_capsule_worker_links_to() {
    // From packages/bridge-worker/src/trace.ts on Capsule's main, run with
    // node: fabricOperationLink("01JABCDEFGHJKMNPQRSTVWXYZ0").
    let operation: OperationId = "01JABCDEFGHJKMNPQRSTVWXYZ0".parse().unwrap();
    let context = operation_context(operation);
    assert_eq!(
        context.trace.to_string(),
        "6ef37855f4b6f479dea9ebe4a32063db"
    );
    assert_eq!(context.span.to_string(), "f6ef0f9115858ef9");
    assert_eq!(
        traceparent(context),
        "00-6ef37855f4b6f479dea9ebe4a32063db-f6ef0f9115858ef9-01"
    );
}

#[test]
fn an_endpoint_is_plain_http_and_gets_the_traces_path() {
    for (text, shown) in [
        ("http://localhost:4318", "http://localhost:4318/v1/traces"),
        ("http://127.0.0.1:4318/", "http://127.0.0.1:4318/v1/traces"),
        (
            "http://collector/otlp",
            "http://collector:80/otlp/v1/traces",
        ),
        (
            "http://h:1/a-b_c.d~e/%2F/x:y@z!$&'()*+,;=",
            "http://h:1/a-b_c.d~e/%2F/x:y@z!$&'()*+,;=/v1/traces",
        ),
        ("http://[::1]:4318", "http://[::1]:4318/v1/traces"),
    ] {
        assert_eq!(text.parse::<OtlpEndpoint>().unwrap().to_string(), shown);
    }
    assert_eq!(
        "https://collector:4318".parse::<OtlpEndpoint>(),
        Err(OtlpEndpointError::Https)
    );
    for text in [
        "localhost:4318",
        "ftp://x",
        "http://",
        "http://a b",
        "http://u@h:1",
        "http://127.0.0.1:4318/otel collector",
        "http://127.0.0.1:4318/otlp\n",
        "http://127.0.0.1:4318/a\r\nX-Injected: 1",
        "http://h/%zz",
        "http://h/caf\u{e9}",
    ] {
        assert_eq!(
            text.parse::<OtlpEndpoint>(),
            Err(OtlpEndpointError::NotHttp),
            "{text}"
        );
    }
    assert_eq!(
        "http://h:0".parse::<OtlpEndpoint>(),
        Err(OtlpEndpointError::Port)
    );
}

#[test]
fn an_error_s_type_name_is_its_variant_and_never_its_data() {
    let error = NodeError::io(Path::new(REPO), std::io::Error::other(SECRET));
    assert_eq!(type_name(&error).as_str(), "Io");
    assert!(Code::new("has space").is_none());
    assert!(Code::new(REPO).is_none());
}

fn key(text: &str) -> OperationKey {
    OperationKey::parse(text).unwrap()
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every command whose input holds a secret, then every check, reads as one script"
)]
async fn no_secret_raw_key_argv_path_or_message_reaches_a_span() {
    let (endpoint, bodies) = receiver().await;
    let tracer = Tracer::start(endpoint, NODE);
    let dir = tempfile::tempdir().unwrap();
    let state = StateDir::resolve(Some(dir.path().as_os_str()), Path::new("/")).unwrap();
    let authority = Authority::start_traced(&state, NODE, tracer.clone()).unwrap();

    let created = authority
        .submit(Envelope {
            key: key(KEY),
            job: None,
            expected: None,
            body: Command::CreateJob {
                intent: Intent {
                    goal: format!("deploy with {SECRET}"),
                    harness: HarnessKind::Codex,
                    acceptance: Vec::new(),
                },
                source: SourceBinding::Repository {
                    path: AbsPath::parse(REPO).unwrap(),
                },
                policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
            },
        })
        .await
        .unwrap();
    let StoredResult::Accepted(accepted) = created.result else {
        panic!("{created:?}");
    };
    let state_now = authority.job(accepted.job).await.unwrap().unwrap();
    // A build whose argv carries the secret, then a cancel whose reason does.
    authority
        .submit(Envelope {
            key: key("register-1"),
            job: Some(accepted.job),
            expected: None,
            body: Command::RegisterTask {
                parent: state_now.job.primary,
                kind: TaskKind::Child {
                    label: format!("label {SECRET}"),
                },
                restart: RestartPolicy::ExplicitOnly,
                class: TaskClass::Build,
                program: Some(Program {
                    argv: vec!["deploy".into(), format!("--token={SECRET}")],
                    cwd: None,
                    env_allowlist: Vec::new(),
                }),
            },
        })
        .await
        .unwrap();
    authority
        .submit(Envelope {
            key: key("cancel-1"),
            job: Some(accepted.job),
            expected: None,
            body: Command::CancelJob {
                reason: format!("because {SECRET}"),
            },
        })
        .await
        .unwrap();
    // The same create again replays; a command for a job the journal
    // lacks is refused.
    authority
        .submit(Envelope {
            key: key(KEY),
            job: None,
            expected: None,
            body: Command::CreateJob {
                intent: Intent {
                    goal: format!("deploy with {SECRET}"),
                    harness: HarnessKind::Codex,
                    acceptance: Vec::new(),
                },
                source: SourceBinding::Repository {
                    path: AbsPath::parse(REPO).unwrap(),
                },
                policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
            },
        })
        .await
        .unwrap();
    let unknown = fabric_core::id::JobId::from_bytes([9; 16]);
    let _ = authority
        .submit(Envelope {
            key: key("cancel-2"),
            job: Some(unknown),
            expected: None,
            body: Command::CancelJob {
                reason: SECRET.into(),
            },
        })
        .await;
    // A client request whose answer is an error naming a path.
    let request = Request::JobNew {
        repo: fabric_client::api::SystemPath::parse(REPO).unwrap(),
        goal: SECRET.into(),
        harness: HarnessKind::Codex,
    };
    let mut span = hooks::request_begin(&request).unwrap();
    hooks::request_end(
        &mut span,
        &Err(NodeError::io(
            Path::new(REPO),
            std::io::Error::other(SECRET),
        )),
    );
    tracer.emit(span);
    tracer.flush(FLUSH_DEADLINE).await;

    let posted = serde_json::to_string(&*bodies.lock().unwrap()).unwrap();
    for needle in [SECRET, KEY, REPO, "deploy", "--token", "because"] {
        assert!(
            !posted.contains(needle),
            "{needle} reached a span: {posted}"
        );
    }
    let spans = spans(&bodies);
    let names: Vec<&str> = spans.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "fabric managed.create_job",
            "fabric managed.register_task",
            "fabric managed.cancel_job",
            "fabric managed.create_job",
            "fabric managed.cancel_job",
            "fabric job.new",
        ]
    );
    let digest = digest(KEY).to_string();
    assert_eq!(
        attr(&spans[0], "fabric.operation.key_digest").unwrap()["stringValue"],
        digest.as_str()
    );
    assert_eq!(
        spans[0]["traceId"],
        operation_context(accepted.operation).trace.to_string()
    );
    assert_eq!(
        attr(&spans[3], "fabric.result").unwrap()["stringValue"],
        "replayed"
    );
    // The replay is a child in the operation's trace, never a second span
    // with the original's ids.
    assert_eq!(spans[3]["traceId"], spans[0]["traceId"]);
    assert_ne!(spans[3]["spanId"], spans[0]["spanId"]);
    assert_eq!(spans[3]["parentSpanId"], spans[0]["spanId"]);
    assert_eq!(
        attr(&spans[4], "fabric.result").unwrap()["stringValue"],
        "refused"
    );
    assert_eq!(
        attr(&spans[4], "fabric.refusal.reason").unwrap()["stringValue"],
        "unknown_job"
    );
    assert_eq!(
        attr(&spans[5], "fabric.error.type").unwrap()["stringValue"],
        "Io"
    );
    assert_eq!(spans[5]["status"]["message"], "Io");
    assert_eq!(tracer.dropped(), 0);
    let resource = &bodies.lock().unwrap()[0]["resourceSpans"][0]["resource"];
    assert_eq!(resource["attributes"][0]["value"]["stringValue"], "fabricd");
    assert_eq!(
        resource["attributes"][2]["value"]["stringValue"],
        NODE.to_string().as_str()
    );
}

#[tokio::test]
async fn a_tracer_that_is_off_builds_and_sends_nothing() {
    let tracer = Tracer::off();
    assert!(!tracer.on());
    tracer.emit(OpSpan::begin(
        SpanOp::JOB_NEW,
        SpanKind::Internal,
        standalone_context(),
    ));
    tracer.flush(FLUSH_DEADLINE).await;
    assert_eq!(tracer.dropped(), 0);
}

#[tokio::test]
async fn spans_an_endpoint_does_not_take_are_dropped_and_counted() {
    // A port nothing listens on: the post fails, the batch is dropped.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    let tracer = Tracer::start(format!("http://{addr}").parse().unwrap(), NODE);
    for _ in 0..3 {
        tracer.emit(OpSpan::begin(
            SpanOp::JOB_RUN,
            SpanKind::Internal,
            standalone_context(),
        ));
    }
    tracer.flush(FLUSH_DEADLINE).await;
    assert_eq!(tracer.dropped(), 3);
}

#[test]
fn a_body_names_the_service_its_version_and_the_node() {
    let body = body(NODE, &[]);
    let attributes = &body["resourceSpans"][0]["resource"]["attributes"];
    let keys: Vec<&str> = attributes
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        ["service.name", "service.version", "service.instance.id"]
    );
}

#[test]
fn only_the_accepted_launch_is_the_attempt_root() {
    use fabric_core::managed::launch::{GrantToken, LaunchRefusal};
    use fabric_core::managed::{AttemptId, Generation};
    let token = GrantToken {
        job: fabric_core::id::JobId::from_bytes([3; 16]),
        attempt: AttemptId::from_bytes([4; 16]),
        generation: Generation::FIRST,
    };
    let root = super::span::attempt_context(token.attempt);
    let mut accepted = hooks::launch_begin(SpanOp::PEER_LAUNCH, &token, None);
    hooks::answer_end::<LaunchRefusal>(&mut accepted, None);
    assert_eq!(accepted.context, root);
    assert_eq!(accepted.parent, None);
    // The same plan sent again after a restart is refused: a child of the
    // root with its own id, never a second root.
    let mut again = hooks::launch_begin(SpanOp::PEER_LAUNCH, &token, None);
    hooks::answer_end(&mut again, Some(&LaunchRefusal::AlreadyLaunched));
    assert_eq!(again.context.trace, root.trace);
    assert_ne!(again.context.span, root.span);
    assert_eq!(again.parent, Some(root.span));
}

#[test]
fn a_removing_gc_gets_a_span_and_a_dry_run_does_not() {
    let gc = |remove| Request::Gc {
        remove,
        approvals: Vec::new(),
        worker: None,
    };
    assert!(hooks::request_begin(&gc(false)).is_none());
    let span = hooks::request_begin(&gc(true)).unwrap();
    assert_eq!(span.op, SpanOp::GC);
}

/// The node a remote attempt runs on in the recovery scenario.
const RUNNER: NodeId = NodeId::from_bytes([8; 32]);

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one loss, its block, its return and its recovery read as one script"
)]
async fn loss_recovery_gets_spans_under_the_same_rules() {
    use fabric_core::id::SnapshotId;
    use fabric_core::managed::launch::Heartbeat;
    use fabric_core::managed::{
        CheckpointAck, CutDigest, FenceVerdict, GrantToken, Observation, OperationMark,
        RecoveryPolicy, ResourceRef, UNAVAILABLE_AFTER_MS,
    };

    let (endpoint, bodies) = receiver().await;
    let tracer = Tracer::start(endpoint, NODE);
    let dir = tempfile::tempdir().unwrap();
    let state = StateDir::resolve(Some(dir.path().as_os_str()), Path::new("/")).unwrap();
    let authority = Authority::start_traced(&state, NODE, tracer.clone()).unwrap();
    let mut policy = JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly);
    policy.recovery = RecoveryPolicy::RestartFromAcknowledged;
    let made = authority
        .submit(Envelope {
            key: key(KEY),
            job: None,
            expected: None,
            body: Command::CreateJob {
                intent: Intent {
                    goal: format!("recover with {SECRET}"),
                    harness: HarnessKind::Codex,
                    acceptance: Vec::new(),
                },
                source: SourceBinding::Repository {
                    path: AbsPath::parse(REPO).unwrap(),
                },
                policy,
            },
        })
        .await
        .unwrap();
    let StoredResult::Accepted(created) = made.result else {
        panic!("{made:?}");
    };
    let job = created.job;
    let primary = authority.job(job).await.unwrap().unwrap().job.primary;
    let granted = authority
        .submit(Envelope {
            key: key("start-1"),
            job: Some(job),
            expected: None,
            body: Command::StartAttempt {
                task: primary,
                resource: ResourceRef::Node { node: RUNNER },
                initial_checkpoint: None,
            },
        })
        .await
        .unwrap();
    let StoredResult::Accepted(grant) = granted.result else {
        panic!("{granted:?}");
    };
    authority
        .mark_authority(job, grant.operation, OperationMark::Dispatched)
        .await
        .unwrap();
    let attempt = authority.job(job).await.unwrap().unwrap().attempts[0].clone();
    let token = GrantToken {
        job,
        attempt: attempt.id,
        generation: attempt.generation,
    };
    authority
        .report(
            token,
            Some(RUNNER),
            Observation::Started {
                attempt: token.attempt,
                generation: token.generation,
            },
        )
        .await
        .unwrap()
        .unwrap();
    // Silent: unknown, then blocked with nothing acknowledged.
    let now = crate::clock::mono_ms();
    authority.tick(now + UNAVAILABLE_AFTER_MS).await.unwrap();
    authority
        .tick(now + 2 * UNAVAILABLE_AFTER_MS)
        .await
        .unwrap();
    // Back on its heartbeat; a checkpoint of its is acknowledged.
    authority
        .heartbeat(
            RUNNER,
            Heartbeat {
                attempts: vec![token],
                running: true,
            },
        )
        .await
        .unwrap();
    let snapshot = SnapshotId::of(b"the runner's checkpoint");
    let cut = CutDigest::from_bytes([0x1c; 32]);
    authority
        .report(
            token,
            Some(RUNNER),
            Observation::Captured {
                attempt: token.attempt,
                generation: token.generation,
                snapshot,
                cut,
            },
        )
        .await
        .unwrap()
        .unwrap();
    authority
        .acknowledge(
            job,
            CheckpointAck {
                attempt: token.attempt,
                generation: token.generation,
                snapshot,
                cut,
                manifests: 1,
                chunks: 1,
                bytes: 1,
                fetched_from: RUNNER,
            },
        )
        .await
        .unwrap();
    // Silent again: unknown, then recovered from the acknowledged checkpoint.
    let later = crate::clock::mono_ms() + UNAVAILABLE_AFTER_MS;
    authority.tick(later).await.unwrap();
    authority.tick(later + UNAVAILABLE_AFTER_MS).await.unwrap();
    // The old runner's report is refused by the fence: its grant was taken.
    let refused = authority
        .report(
            token,
            Some(RUNNER),
            Observation::Exited {
                attempt: token.attempt,
                generation: token.generation,
                outcome: fabric_core::managed::ExitOutcome::Succeeded,
            },
        )
        .await
        .unwrap();
    assert!(refused.is_err(), "{refused:?}");
    // A restarted runner presents the grant it had taken.
    tracer.emit(hooks::present(
        &token,
        Some(NODE),
        &FenceVerdict::Superseded { by: None },
    ));
    tracer.flush(FLUSH_DEADLINE).await;

    let posted = serde_json::to_string(&*bodies.lock().unwrap()).unwrap();
    let recover_key = format!("recover.{job}.{}.1", token.attempt);
    for needle in [SECRET, KEY, REPO, recover_key.as_str(), "the domain"] {
        assert!(
            !posted.contains(needle),
            "{needle} reached a span: {posted}"
        );
    }
    let spans = spans(&bodies);
    let names: Vec<&str> = spans.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "fabric managed.create_job",
            "fabric managed.start_attempt",
            "fabric observe.started",
            "fabric observe.resource_unavailable",
            "fabric recovery.blocked",
            "fabric observe.resource_reachable",
            "fabric observe.captured",
            "fabric recovery.acknowledge",
            "fabric observe.resource_unavailable",
            "fabric managed.recover",
            "fabric observe.exited",
            "fabric runner.present",
        ]
    );
    let by = |name: &str| {
        spans
            .iter()
            .find(|s| s["name"] == format!("fabric {name}"))
            .unwrap()
    };
    let attempt_trace = attempt_context(token.attempt).trace.to_string();
    for name in [
        "observe.resource_unavailable",
        "recovery.blocked",
        "observe.resource_reachable",
        "observe.captured",
        "recovery.acknowledge",
        "runner.present",
    ] {
        assert_eq!(by(name)["traceId"], attempt_trace.as_str(), "{name}");
    }
    assert_eq!(
        attr(by("recovery.blocked"), "fabric.refusal.reason").unwrap()["stringValue"],
        "no_acknowledged_checkpoint"
    );
    assert_eq!(
        attr(by("observe.captured"), "fabric.snapshot").unwrap()["stringValue"],
        snapshot.to_string().as_str()
    );
    assert_eq!(
        attr(by("observe.captured"), "fabric.checkpoint").unwrap()["stringValue"],
        super::span::Digest(cut.0).to_string().as_str()
    );
    // The authority's own recover: its key as a digest, in its operation's
    // trace, linked to the attempt it grants, which supersedes generation 1.
    let recover = by("managed.recover");
    assert_eq!(
        attr(recover, "fabric.operation.key_digest").unwrap()["stringValue"],
        digest(&recover_key).to_string().as_str()
    );
    assert_eq!(
        attr(recover, "fabric.result").unwrap()["stringValue"],
        "done"
    );
    assert_eq!(
        attr(recover, "fabric.generation.supersedes").unwrap()["intValue"],
        "1"
    );
    assert!(recover["links"].as_array().is_some_and(|l| l.len() == 1));
    let exited = by("observe.exited");
    assert_eq!(
        attr(exited, "fabric.result").unwrap()["stringValue"],
        "refused"
    );
    assert_eq!(
        attr(exited, "fabric.refusal.reason").unwrap()["stringValue"],
        "superseded"
    );
    assert_eq!(
        attr(exited, "fabric.peer").unwrap()["stringValue"],
        RUNNER.to_string().as_str()
    );
    let present = by("runner.present");
    assert_eq!(present["kind"], 3);
    assert_eq!(
        attr(present, "fabric.refusal.reason").unwrap()["stringValue"],
        "superseded"
    );
}

#[test]
fn the_m6_commands_and_observations_have_their_own_ops() {
    use fabric_core::managed::CommandKind;
    assert_eq!(
        SpanOp::managed(CommandKind::Supersede).name(),
        "managed.supersede"
    );
    assert_eq!(
        SpanOp::managed(CommandKind::Recover).name(),
        "managed.recover"
    );
}
