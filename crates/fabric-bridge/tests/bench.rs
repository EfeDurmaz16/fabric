//! The bridge's own latencies, kept as raw samples (ADR 0021): a worker
//! spawned and its handshake answered, and one request round trip to it,
//! with the in-memory log and with the journal's two committed writes.
//! Run by `scripts/bench-bridge.sh`, which keeps every sample with the
//! machine it ran on. `bridge_latencies` runs the Rust test double, so its
//! numbers are Fabric's side of the bridge. `capsule_worker_latencies`
//! runs the same paths against the real Capsule worker when
//! `CAPSULE_BRIDGE_WORKER` names it, on the fixture profile and, when
//! `CAPSULE_BRIDGE_DOCKER_IMAGE` names an image the engine holds, on the
//! local Docker engine.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{
    Cleanup, WORKER_VAR, capsule_worker, docker, docker_image, double, fresh, journal, scope,
};
use fabric_bridge::wire::{Action, CreateParams, Digest, ExecParams, Locator, OperationRef, Scope};
use fabric_bridge::{Bridge, Config, Effect, MemoryLog, Recorder};
use fabric_core::id::JobId;
use fabric_core::managed::{AttemptId, Generation, OperationId, OperationKey, TaskId};
use fabric_journal::ProviderStatus;

fn effect(n: u32) -> Effect {
    let mut id = [0x5a; 16];
    id[12..].copy_from_slice(&n.to_be_bytes());
    let mut digest = [0; 32];
    digest[..4].copy_from_slice(&n.to_be_bytes());
    Effect {
        operation: OperationRef {
            id: OperationId::from_bytes(id),
            key: OperationKey::parse(&format!("bench-{n}")).unwrap(),
            digest: Digest(digest),
        },
        scope: scope(),
        action: Action::Create(CreateParams {
            image: "fixture/ok".into(),
            ..CreateParams::default()
        }),
        approval: None,
        policy: None,
        deadline_ms: 30_000,
    }
}

fn reps(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// The sample at percentile `p` of sorted `samples`, nearest rank.
fn percentile(sorted: &[u64], p: usize) -> u64 {
    let rank = (sorted.len() * p).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn keep(out: &Path, name: &str, samples: &[u64], summary: &mut String) {
    let raw: String = samples.iter().fold(String::new(), |mut s, v| {
        let _ = writeln!(s, "{v}");
        s
    });
    std::fs::write(out.join(format!("{name}_us.txt")), raw).unwrap();
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let _ = writeln!(
        summary,
        "{name}: n={} p50={}us p90={}us p99={}us max={}us",
        sorted.len(),
        percentile(&sorted, 50),
        percentile(&sorted, 90),
        percentile(&sorted, 99),
        sorted[sorted.len() - 1],
    );
}

async fn round_trips<L: Recorder>(bridge: &Bridge, log: &mut L, from: u32, n: usize) -> Vec<u64> {
    let mut samples = Vec::with_capacity(n);
    for i in 0..u32::try_from(n).unwrap() {
        let started = Instant::now();
        let outcome = bridge
            .request(log, effect(from + i))
            .await
            .unwrap()
            .finish(log)
            .await
            .unwrap();
        samples.push(micros(started.elapsed()));
        assert_eq!(outcome.status, ProviderStatus::Succeeded);
    }
    samples
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a bench; run by scripts/bench-bridge.sh with FABRIC_BRIDGE_BENCH_OUT set"]
async fn bridge_latencies() {
    let Some(out) = std::env::var_os("FABRIC_BRIDGE_BENCH_OUT").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let mut summary = String::new();

    let spawns = reps("FABRIC_BRIDGE_BENCH_SPAWNS", 50);
    let mut spawn = Vec::with_capacity(spawns);
    for _ in 0..3 {
        Bridge::spawn(double(), Config::new("fixture"))
            .await
            .unwrap()
            .close()
            .await;
    }
    for _ in 0..spawns {
        let started = Instant::now();
        let bridge = Bridge::spawn(double(), Config::new("fixture"))
            .await
            .unwrap();
        spawn.push(micros(started.elapsed()));
        bridge.close().await;
    }
    keep(&out, "spawn_handshake", &spawn, &mut summary);

    let trips = reps("FABRIC_BRIDGE_BENCH_TRIPS", 500);
    let bridge = Bridge::spawn(double(), Config::new("fixture"))
        .await
        .unwrap();
    let mut memory = MemoryLog::new();
    round_trips(&bridge, &mut memory, 0, 50).await;
    let samples = round_trips(&bridge, &mut memory, 1_000, trips).await;
    keep(&out, "round_trip_memory_log", &samples, &mut summary);

    let dir = tempfile::tempdir().unwrap();
    let mut journal = journal(dir.path());
    round_trips(&bridge, &mut journal, 100_000, 20).await;
    let journal_trips = reps("FABRIC_BRIDGE_BENCH_JOURNAL_TRIPS", 200);
    let samples = round_trips(&bridge, &mut journal, 200_000, journal_trips).await;
    keep(&out, "round_trip_journal", &samples, &mut summary);
    bridge.close().await;

    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    eprint!("{summary}");
}

/// One effect of the Capsule bench, under its own operation id and key.
fn capsule_effect(
    scope: Scope,
    tag: &str,
    n: usize,
    action: Action,
    approval: Option<&str>,
) -> Effect {
    let mut digest = [0; 32];
    digest[..8].copy_from_slice(&(n as u64).to_be_bytes());
    Effect {
        operation: OperationRef {
            id: OperationId::from_bytes(fresh(&format!("{tag}-{n}"))),
            key: OperationKey::parse(&format!("{tag}-{n}")).unwrap(),
            digest: Digest(digest),
        },
        scope,
        action,
        approval: approval.map(str::to_owned),
        policy: None,
        deadline_ms: 60_000,
    }
}

fn echo(target: &Locator) -> Action {
    Action::Exec {
        target: target.clone(),
        params: ExecParams {
            argv: vec!["echo".into(), "bench".into()],
            stdin: None,
            cwd: None,
            timeout_ms: 30_000,
            max_output_bytes: 4096,
            secret_names: Vec::new(),
        },
    }
}

/// Times `n` requests made by `make`, each to its answer, which must be
/// `succeeded`.
async fn timed<L: Recorder>(
    bridge: &Bridge,
    log: &mut L,
    n: usize,
    mut make: impl FnMut(usize) -> Effect,
) -> Vec<u64> {
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let effect = make(i);
        let started = Instant::now();
        let outcome = bridge
            .request(log, effect)
            .await
            .unwrap()
            .finish(log)
            .await
            .unwrap();
        samples.push(micros(started.elapsed()));
        assert_eq!(
            outcome.status,
            ProviderStatus::Succeeded,
            "{:?}",
            outcome.answer
        );
    }
    samples
}

/// Spawns the worker `n` times, each to its answered handshake.
async fn spawns(profile: &str, config: &Path, n: usize) -> Vec<u64> {
    for _ in 0..2 {
        Bridge::spawn(capsule_worker(profile, config), Config::new(profile))
            .await
            .unwrap()
            .close()
            .await;
    }
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let started = Instant::now();
        let bridge = Bridge::spawn(capsule_worker(profile, config), Config::new(profile))
            .await
            .unwrap();
        samples.push(micros(started.elapsed()));
        bridge.close().await;
    }
    samples
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a bench; run by scripts/bench-bridge.sh with FABRIC_BRIDGE_BENCH_OUT and CAPSULE_BRIDGE_WORKER set"]
#[expect(clippy::too_many_lines, reason = "one bench, profile by profile")]
async fn capsule_worker_latencies() {
    let Some(out) = std::env::var_os("FABRIC_BRIDGE_BENCH_OUT").map(PathBuf::from) else {
        return;
    };
    let out = out.join("capsule");
    std::fs::create_dir_all(&out).unwrap();
    if std::env::var_os(WORKER_VAR).is_none() {
        std::fs::write(
            out.join("skipped.txt"),
            format!("{WORKER_VAR} is not set\n"),
        )
        .unwrap();
        return;
    }
    let n = reps("FABRIC_BRIDGE_BENCH_CAPSULE_REPS", 15);
    let mut summary = String::new();
    let dir = tempfile::tempdir().unwrap();

    // The fixture profile: the worker and the Capsule facade, no provider.
    let fixture = dir.path().join("fixture.json");
    let seed = serde_json::json!({
        "binding": {"account": "acct-a", "project": "proj-1"},
        "policy": {"images": ["fixture/ok"]},
        "approvals": [],
        "sandboxes": [{"native_id": "fx-seed", "project": "proj-1", "owner_job": common::JOB, "status": "ready"}],
        "script": []
    });
    std::fs::write(&fixture, seed.to_string()).unwrap();
    keep(
        &out,
        "fixture_spawn_handshake",
        &spawns("fixture", &fixture, n).await,
        &mut summary,
    );
    let bridge = Bridge::spawn(capsule_worker("fixture", &fixture), Config::new("fixture"))
        .await
        .unwrap();
    let create = |tag: &'static str| {
        move |i| {
            capsule_effect(
                scope(),
                tag,
                i,
                Action::Create(CreateParams {
                    image: "fixture/ok".into(),
                    ..CreateParams::default()
                }),
                None,
            )
        }
    };
    let mut memory = MemoryLog::new();
    timed(&bridge, &mut memory, 5, create("warm")).await;
    let samples = timed(&bridge, &mut memory, n, create("mem")).await;
    keep(&out, "fixture_create_memory_log", &samples, &mut summary);
    let seed_locator = Locator::from_value(&serde_json::json!({
        "provider": "fixture",
        "scope": {"account": "acct-a", "project": "proj-1"},
        "kind": "sandbox",
        "native_id": "fx-seed",
    }))
    .unwrap();
    let samples = timed(&bridge, &mut memory, n, |i| {
        capsule_effect(scope(), "exec", i, echo(&seed_locator), None)
    })
    .await;
    keep(&out, "fixture_exec_memory_log", &samples, &mut summary);
    let jdir = tempfile::tempdir().unwrap();
    let mut journal = journal(jdir.path());
    timed(&bridge, &mut journal, 3, create("jwarm")).await;
    let samples = timed(&bridge, &mut journal, n, create("journal")).await;
    keep(&out, "fixture_create_journal", &samples, &mut summary);
    bridge.close().await;

    // The Docker profile on the local engine, when it can run.
    match docker_image() {
        Err(why) => {
            let _ = writeln!(summary, "docker: skipped: {why}");
        }
        Ok(image) => {
            let scope = Scope {
                job: JobId::from_bytes(fresh("bench-job")),
                task: TaskId::from_bytes(fresh("bench-task")),
                attempt: AttemptId::from_bytes(fresh("bench-attempt")),
                generation: Generation::FIRST,
            };
            let _cleanup = Cleanup(format!("label=fabric.job={}", scope.job));
            let config = dir.path().join("docker.json");
            let worker = serde_json::json!({
                "policy": {"images": [image]},
                "approvals": [{"ref": "apr-destroy", "action": "sandbox.destroy", "native_id": null}]
            });
            std::fs::write(&config, worker.to_string()).unwrap();
            let server =
                docker(&["version", "--format", "{{.Server.Version}}"]).unwrap_or_default();
            let engine = docker(&["context", "show"]).unwrap_or_default();
            std::fs::write(
                out.join("docker.txt"),
                format!(
                    "server: {}\ncontext: {}\nimage: {image}\n",
                    server.trim(),
                    engine.trim()
                ),
            )
            .unwrap();
            keep(
                &out,
                "docker_spawn_handshake",
                &spawns("docker", &config, n).await,
                &mut summary,
            );
            let bridge = Bridge::spawn(capsule_worker("docker", &config), Config::new("docker"))
                .await
                .unwrap();
            let mut log = MemoryLog::new();
            let make = |i| {
                capsule_effect(
                    scope,
                    "dcreate",
                    i,
                    Action::Create(CreateParams {
                        image: image.clone(),
                        ..CreateParams::default()
                    }),
                    None,
                )
            };
            let mut created = Vec::with_capacity(n);
            let mut create_samples = Vec::with_capacity(n);
            for i in 0..n {
                let effect = make(i);
                let started = Instant::now();
                let outcome = bridge
                    .request(&mut log, effect)
                    .await
                    .unwrap()
                    .finish(&mut log)
                    .await
                    .unwrap();
                create_samples.push(micros(started.elapsed()));
                let fabric_bridge::Answer::Result(r) = outcome.answer else {
                    panic!("{:?}", outcome.answer)
                };
                created.push(r.resource);
            }
            keep(&out, "docker_create", &create_samples, &mut summary);
            let target = created[0].clone();
            timed(&bridge, &mut log, 3, |i| {
                capsule_effect(scope, "dwarm", i, echo(&target), None)
            })
            .await;
            let samples = timed(&bridge, &mut log, n, |i| {
                capsule_effect(scope, "dexec", i, echo(&target), None)
            })
            .await;
            keep(&out, "docker_exec", &samples, &mut summary);
            let samples = timed(&bridge, &mut log, n, |i| {
                capsule_effect(
                    scope,
                    "ddestroy",
                    i,
                    Action::Destroy {
                        target: created[i].clone(),
                    },
                    Some("apr-destroy"),
                )
            })
            .await;
            keep(&out, "docker_destroy", &samples, &mut summary);
            bridge.close().await;
        }
    }

    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    eprint!("{summary}");
}
