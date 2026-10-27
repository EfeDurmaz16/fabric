//! The bridge's own latencies, kept as raw samples (ADR 0021): a worker
//! spawned and its handshake answered, and one request round trip to it,
//! with the in-memory log and with the journal's two committed writes.
//! Run by `scripts/bench-bridge.sh`, which keeps every sample with the
//! machine it ran on; the worker is the Rust test double, so the numbers
//! are Fabric's side of the bridge, not a provider's.

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

use common::{journal, scope};
use fabric_bridge::wire::{Action, CreateParams, Digest, OperationRef};
use fabric_bridge::{Bridge, Config, Effect, MemoryLog, OperationLog};
use fabric_core::managed::{OperationId, OperationKey};
use fabric_journal::ProviderStatus;
use tokio::process::Command;

fn double() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fabric-bridge-double"))
}

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

async fn round_trips<L: OperationLog>(
    bridge: &Bridge,
    log: &mut L,
    from: u32,
    n: usize,
) -> Vec<u64> {
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
