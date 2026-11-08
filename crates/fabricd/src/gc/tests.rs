//! The gc tests: the live set of the managed journal, the launch path's
//! residuals, and a provider destroy that leaves the authority serving
//! while the worker acts.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::sync::Arc;
use std::time::Duration;

use fabric_bridge::frames;
use fabric_bridge::wire::{Action, CreateParams, Digest, OperationRef, Scope};
use fabric_bridge::{Bridge, Config, Effect};
use fabric_client::api::{GcApproval, GcReport};
use fabric_core::harness::HarnessKind;
use fabric_core::id::JobId;
use fabric_core::managed::{
    AttemptId, AttemptStatus, Command, Envelope, ExitOutcome, Generation, Intent, JobPolicy,
    Observation, OperationId, OperationKey, ResourceRef, RestartPolicy, SourceBinding, TaskId,
};
use fabric_core::path::AbsPath;
use fabric_journal::ProviderStatus;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{Keep, Kind, Managed, Running, StagingLabel, Verdict, Worker, label};
use crate::node::Node;

const PROFILE: &str = include_str!("../../../../protocol/bridge/v1/profiles/fixture.json");

/// A managed job with one task whose first attempt is granted here.
struct Granted {
    job: JobId,
    task: TaskId,
    attempt: AttemptId,
    generation: Generation,
}

async fn granted(node: &Arc<Node>) -> Granted {
    let create = Envelope {
        key: OperationKey::parse("gc-create").unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: "gc".into(),
                harness: HarnessKind::Codex,
                acceptance: Vec::new(),
            },
            source: SourceBinding::Repository {
                path: AbsPath::parse("/src/repo").unwrap(),
            },
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
        },
    };
    node.authority.submit(create).await.unwrap();
    let state = node.authority.list().await.unwrap().remove(0);
    let (job, task) = (state.job.id, state.tasks[0].id);
    let start = Envelope {
        key: OperationKey::parse("gc-start").unwrap(),
        job: Some(job),
        expected: None,
        body: Command::StartAttempt {
            task,
            resource: ResourceRef::Node { node: node.id() },
            initial_checkpoint: None,
        },
    };
    node.authority.submit(start).await.unwrap();
    let state = node.authority.job(job).await.unwrap().unwrap();
    let attempt = &state.attempts[0];
    assert_eq!(attempt.status, AttemptStatus::Granted);
    Granted {
        job,
        task,
        attempt: attempt.id,
        generation: attempt.generation,
    }
}

/// Ends the attempt: started, then exited with a failure.
async fn end(node: &Node, g: &Granted) {
    for observation in [
        Observation::Started {
            attempt: g.attempt,
            generation: g.generation,
        },
        Observation::Exited {
            attempt: g.attempt,
            generation: g.generation,
            outcome: ExitOutcome::Failed { code: Some(1) },
        },
    ] {
        node.authority.observe(g.job, observation).await.unwrap();
    }
}

fn ack(hello: &Value) -> Value {
    json!({
        "type": "hello_ack",
        "protocol": 1,
        "worker": "gc-test/0",
        "profile": serde_json::from_str::<Value>(PROFILE).unwrap(),
        "limits": hello["limits"].clone(),
    })
}

fn resource() -> Value {
    json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": "proj-1"},
        "kind": "sandbox",
        "native_id": "fx-gc-1",
    })
}

fn completed(request: &Value, output: &Value) -> Value {
    json!({
        "type": "result",
        "request_id": request["request_id"],
        "operation_id": request["operation"]["id"],
        "state": "completed",
        "resource": resource(),
        "output": output,
        "replayed": false,
        "receipts": [],
    })
}

/// An in-process worker: it answers the create at once, and the destroy
/// only once `release` fires, after it says on `reached` that it holds it.
async fn worker(reached: oneshot::Sender<Value>, release: oneshot::Receiver<()>) -> Bridge {
    let (fabric, worker) = tokio::io::duplex(64 << 10);
    let (from_worker, to_worker) = tokio::io::split(fabric);
    let (mut reader, mut writer) = tokio::io::split(worker);
    tokio::spawn(async move {
        let hello = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        let frame = frames::encode(&ack(&hello), 1 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        let create = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        assert_eq!(create["action"], "sandbox.create", "{create}");
        let frame = frames::encode(&completed(&create, &json!({"status": "ready"})), 1 << 20);
        frames::write(&mut writer, &frame.unwrap()).await.unwrap();
        let destroy = frames::read(&mut reader, 1 << 20).await.unwrap().unwrap();
        assert_eq!(destroy["action"], "sandbox.destroy", "{destroy}");
        reached.send(destroy.clone()).unwrap();
        release.await.unwrap();
        let frame = frames::encode(&completed(&destroy, &json!({})), 1 << 20).unwrap();
        frames::write(&mut writer, &frame).await.unwrap();
        // Kept open until the bridge closes.
        let _ = frames::read(&mut reader, 1 << 20).await;
    });
    let mut config = Config::new("fixture");
    config.grace = Duration::from_millis(200);
    config.kill_grace = Duration::from_millis(300);
    Bridge::connect(from_worker, Box::new(to_worker), config)
        .await
        .unwrap()
}

/// The create the attempt makes through `bridge`, recorded through the
/// authority as gc's cleanup records.
async fn create(node: &Node, bridge: &Bridge, g: &Granted) -> OperationId {
    let id = OperationId::from_bytes([0x61; 16]);
    let effect = Effect {
        operation: OperationRef {
            id,
            key: OperationKey::parse("gc-sandbox").unwrap(),
            digest: Digest([0x61; 32]),
        },
        scope: Scope {
            job: g.job,
            task: g.task,
            attempt: g.attempt,
            generation: g.generation,
        },
        action: Action::Create(CreateParams {
            image: "fixture/ok".into(),
            ..CreateParams::default()
        }),
        approval: None,
        policy: None,
        deadline_ms: 30_000,
    };
    let mut log = node.authority.recorder();
    let made = bridge
        .request(&mut log, effect)
        .await
        .unwrap()
        .finish(&mut log)
        .await
        .unwrap();
    assert_eq!(made.status, ProviderStatus::Succeeded);
    id
}

fn providers(report: &GcReport) -> Vec<(&str, Option<OperationId>)> {
    report
        .residuals
        .iter()
        .filter(|r| r.kind == "provider_resource")
        .map(|r| (r.state.as_str(), r.operation))
        .collect()
}

async fn recovered_node() -> (tempfile::TempDir, Arc<Node>) {
    let (dir, node) = crate::ops::tests::node();
    tokio::spawn(Arc::clone(&node).recover());
    node.recovered().await;
    (dir, node)
}

/// While a worker holds an approved destroy, the authority still answers
/// every other call: the destroy waits on the runtime and records one
/// step at a time through the authority's queue. Before, the cleanup ran
/// on the authority's thread, which then answered nothing until the
/// worker did, for up to the cleanup's 60 s deadline. The create of a
/// live attempt is never a residual, and once the attempt ended it is
/// one, labelled with its attempt, removed with its approval alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_destroy_leaves_the_authority_serving_while_the_worker_acts() {
    let (_dir, node) = recovered_node().await;
    let g = granted(&node).await;
    let (reached, held) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let bridge = worker(reached, released).await;
    let made = create(&node, &bridge, &g).await;

    let live = node.gc_with(false, Vec::new(), None).await.unwrap();
    assert_eq!(providers(&live), [], "the attempt is live: {live:?}");
    assert!(
        live.live
            .iter()
            .any(|l| l.holds == "attempt" && l.label.attempt == Some(g.attempt)),
        "{live:?}"
    );

    end(&node, &g).await;
    let ended = node.gc_with(false, Vec::new(), None).await.unwrap();
    assert_eq!(providers(&ended), [("found", Some(made))], "{ended:?}");
    let found = &ended.residuals[0];
    assert_eq!(found.label.attempt, Some(g.attempt));
    assert_eq!(found.label.generation, Some(g.generation));

    let gc = {
        let node = Arc::clone(&node);
        tokio::spawn(async move {
            node.gc_with(
                true,
                vec![GcApproval {
                    operation: made,
                    approval: "apr-gc-1".into(),
                }],
                Some(Worker::Ready(bridge)),
            )
            .await
        })
    };
    let destroy = held.await.unwrap();
    assert_eq!(destroy["approval"]["ref"], "apr-gc-1", "{destroy}");
    // The worker holds the destroy: the authority answers anyway. The
    // timeout only turns a hang into a failure.
    let listed = tokio::time::timeout(Duration::from_secs(10), node.authority.list())
        .await
        .expect("the authority answered nothing while the worker held the destroy")
        .unwrap();
    assert_eq!(listed.len(), 1);
    release.send(()).unwrap();
    let removed = gc.await.unwrap().unwrap();
    assert_eq!(
        providers(&removed),
        [("removed", Some(made))],
        "{removed:?}"
    );

    let after = node.gc_with(false, Vec::new(), None).await.unwrap();
    assert_eq!(providers(&after), [], "{after:?}");
}

/// An attempt the journal holds `granted`, `running` or `unknown` may
/// still run, at whatever generation, so nothing of it is a residual; an
/// attempt it holds ended is; one it does not hold is kept.
#[test]
fn an_attempt_that_may_still_run_is_live() {
    let attempt = AttemptId::from_bytes([3; 16]);
    let named = label(JobId::from_bytes([4; 16]));
    let ended = [
        AttemptStatus::Succeeded,
        AttemptStatus::Failed,
        AttemptStatus::Cancelled,
        AttemptStatus::Lost,
    ];
    for status in [
        AttemptStatus::Granted,
        AttemptStatus::Running,
        AttemptStatus::Unknown,
    ]
    .into_iter()
    .chain(ended)
    {
        let mut managed = Managed::default();
        managed
            .attempts
            .insert(attempt, (named.clone(), status.may_run()));
        let verdict = managed.verdict(attempt, &Running::default());
        assert_eq!(
            matches!(verdict, Verdict::Live),
            matches!(
                status,
                AttemptStatus::Granted | AttemptStatus::Running | AttemptStatus::Unknown
            ),
            "{status:?}"
        );
        // An attempt this node's executor holds open is live whatever
        // the journal says.
        let running = Running {
            tokens: Vec::new(),
            open: [attempt].into(),
        };
        assert!(matches!(managed.verdict(attempt, &running), Verdict::Live));
    }
    assert!(matches!(
        Managed::default().verdict(attempt, &Running::default()),
        Verdict::Unheld
    ));
}

/// The launch path's files of an ended attempt are residuals, labelled
/// with the attempt, and `remove` takes exactly those; those of an
/// attempt the journal here does not hold, and every taken record, are
/// kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launch_path_leaves_residuals_only_for_ended_attempts() {
    let (_dir, node) = recovered_node().await;
    let g = granted(&node).await;
    end(&node, &g).await;
    let runs = node.cfg.state.root().join("runs");
    let restored = runs.join("restored").join(g.attempt.to_string());
    std::fs::create_dir_all(restored.join("sessions")).unwrap();
    let stranger = runs
        .join("restored")
        .join(AttemptId::from_bytes([9; 16]).to_string());
    std::fs::create_dir_all(&stranger).unwrap();
    let package = crate::paths::home_jobs(&node.cfg.home)
        .join(g.job.to_string())
        .join(format!("package-{}.json", g.attempt));
    std::fs::create_dir_all(package.parent().unwrap()).unwrap();
    std::fs::write(&package, b"{}").unwrap();
    let taken = runs.join("taken").join(format!("{}.json", g.attempt));
    std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
    crate::local::write(
        &taken,
        &crate::runner::Taken {
            token: fabric_core::managed::launch::GrantToken {
                job: g.job,
                attempt: g.attempt,
                generation: g.generation,
            },
        },
    )
    .unwrap();

    let found = node.gc_with(false, Vec::new(), None).await.unwrap();
    let listed: Vec<(&str, Option<&std::path::Path>)> = found
        .residuals
        .iter()
        .map(|r| (r.kind.as_str(), r.path.as_deref()))
        .collect();
    assert_eq!(
        listed,
        [
            ("restored_state", Some(restored.as_path())),
            ("work_package", Some(package.as_path())),
        ],
        "{found:?}"
    );
    assert!(
        found
            .residuals
            .iter()
            .all(|r| r.label.attempt == Some(g.attempt) && r.label.job == g.job)
    );
    let kept: Vec<(&str, &str)> = found
        .kept
        .iter()
        .map(|k| (k.kind.as_str(), k.reason.as_str()))
        .collect();
    assert!(
        kept.contains(&("restored_state", "unknown_attempt")),
        "{kept:?}"
    );
    assert!(kept.contains(&("taken_record", "preserved")), "{kept:?}");

    let removed = node.gc_with(true, Vec::new(), None).await.unwrap();
    assert!(
        removed.residuals.iter().all(|r| r.state == "removed"),
        "{removed:?}"
    );
    assert!(!restored.exists() && !package.exists());
    assert!(
        stranger.exists() && taken.exists(),
        "only the listed set goes"
    );
    let again = node.gc_with(false, Vec::new(), None).await.unwrap();
    assert!(again.is_clean(), "{again:?}");
}

/// A staging label reads back as written, and anything this version does
/// not write reads as none.
#[test]
fn a_staging_label_reads_back_and_nothing_else_does() {
    let written = StagingLabel {
        job: JobId::from_bytes([1; 16]),
        node: fabric_core::id::NodeId::from_bytes([2; 32]),
        epoch: fabric_core::id::Epoch::new(3).unwrap(),
    };
    let text = written.encode();
    assert_eq!(StagingLabel::parse(&text), Some(written));
    assert_eq!(StagingLabel::parse(&text.replace("v1", "v2")), None);
    assert_eq!(StagingLabel::parse(&format!("{text}extra\n")), None);
    assert_eq!(StagingLabel::parse(text.trim_end()), None);
}

/// Every kind and every reason has a name of its own: the report and the
/// inventory key on them.
#[test]
fn every_kind_and_reason_has_its_own_name() {
    let kinds: std::collections::BTreeSet<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
    assert_eq!(kinds.len(), Kind::ALL.len());
    let reasons: std::collections::BTreeSet<&str> = Keep::ALL.iter().map(|k| k.name()).collect();
    assert_eq!(reasons.len(), Keep::ALL.len());
}
