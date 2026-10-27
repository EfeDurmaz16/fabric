//! The M2 baseline bench: times the paths the durable job work touches
//! that `budget::hot_paths` does not, on the same fixture of two nodes
//! over QUIC with the same fake probed programs that answer after 50 ms.
//!
//! Per harness (ACP, Claude, Codex), each run makes a new job (`new`,
//! which clones the repository and writes the job's first ledger
//! records), runs its first headless turn (`launch`, which plans,
//! launches, registers and reaps the harness) and records one approval
//! (`approve`, one ledger append made durable). A long-lived job takes
//! one more approval per run (`ledger append`), so the series shows how an
//! append costs as its ledger grows. Then the CLI starts cold without
//! asking the node (`cli version`) and asks the node for its jobs
//! (`cli ls`). Daemon cold start and restart, suspend, move, resume and
//! pull stay in `budget::hot_paths`.
//!
//! It prints every sample as a `RAW` line and a table of each op's 25th
//! percentile, median and 95th percentile. It checks nothing and is
//! ignored by default; `scripts/bench-m2.sh` runs it beside the budget
//! and keeps the raw output. `FABRIC_BENCH_REPS` sets the runs, 15 when
//! unset.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::budget::{PROBE_DELAY, PROBED};
use crate::world::{Env, Fallible, Node, Wire, pair, write_executable};

/// The harness of each job shape, by its name in the report.
const SHAPES: [(&str, &str); 3] = [
    ("acp", crate::AGENT),
    ("claude", "claude"),
    ("codex", "codex"),
];

/// Runs when `FABRIC_BENCH_REPS` is unset.
const DEFAULT_REPS: usize = 15;

/// The paired nodes the bench drives.
struct Fixture {
    env: Arc<Env>,
    a: Node,
    _b: Node,
    /// The job whose ledger grows by one approval a run.
    long: String,
}

impl Fixture {
    fn new() -> Fallible<Self> {
        let env = Env::new()?;
        env.install_fake_claude()?;
        env.install_fake_codex()?;
        for (program, answer) in PROBED {
            env.install(
                program,
                &format!("#!/bin/sh\nsleep {PROBE_DELAY}\n{answer}\n"),
            )?;
        }
        for harness in ["claude", "codex"] {
            let path = env.bin().join(harness);
            let script = std::fs::read_to_string(&path)?;
            let (shebang, rest) = script.split_once('\n').ok_or("an empty fake")?;
            write_executable(
                &path,
                format!("{shebang}\nsleep {PROBE_DELAY}\n{rest}").as_bytes(),
            )?;
        }
        std::fs::create_dir_all(env.home.join(".claude/sessions"))?;
        let mut a = Node::new(&env, "a", Wire::Quic)?;
        let mut b = Node::new(&env, "b", Wire::Quic)?;
        a.start(None)?;
        b.start(None)?;
        pair(&a, &b)?;
        let long = crate::new_job(&env, &a, crate::AGENT)?;
        Ok(Self {
            env,
            a,
            _b: b,
            long,
        })
    }

    /// One run of every op into `times`.
    fn run(&self, times: &mut BTreeMap<String, Vec<Duration>>) -> Fallible {
        let mut time = |op: String, step: &dyn Fn() -> Fallible<String>| -> Fallible<String> {
            let started = Instant::now();
            let out = step()?;
            times.entry(op).or_default().push(started.elapsed());
            Ok(out)
        };
        for (name, harness) in SHAPES {
            let made = time(format!("{name} new"), &|| {
                crate::new_job(&self.env, &self.a, harness)
            })?;
            time(format!("{name} launch"), &|| {
                self.a
                    .ok(&["job", "run", &made, "--headless", "--prompt", "one"])
            })?;
            time(format!("{name} approve"), &|| {
                self.a.ok(&["job", "approve", &made, "--note", "bench"])
            })?;
        }
        time("ledger append".to_owned(), &|| {
            self.a
                .ok(&["job", "approve", &self.long, "--note", "bench"])
        })?;
        time("cli version".to_owned(), &|| self.a.ok(&["--version"]))?;
        time("cli ls".to_owned(), &|| self.a.ok(&["job", "ls"]))?;
        Ok(())
    }
}

/// The sample at `percentile` of `samples`, nearest rank.
fn at(samples: &[Duration], percentile: usize) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort();
    let rank = (percentile * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or_default()
}

/// Every sample as a tab-separated `RAW` line, then the table.
fn report(times: &BTreeMap<String, Vec<Duration>>, reps: usize) -> String {
    let mut out = String::new();
    for (op, samples) in times {
        for (run, took) in samples.iter().enumerate() {
            let _ = writeln!(out, "RAW\t{op}\t{run}\t{}", took.as_micros());
        }
    }
    let _ = writeln!(
        out,
        "\n### M2 bench, {reps} runs, times in ms\n\n| op | p25 | median | p95 |\n|---|---:|---:|---:|"
    );
    for (op, samples) in times {
        let ms = |p| at(samples, p).as_secs_f64() * 1000.0;
        let _ = writeln!(
            out,
            "| {op} | {:.1} | {:.1} | {:.1} |",
            ms(25),
            ms(50),
            ms(95)
        );
    }
    out
}

/// The runs `FABRIC_BENCH_REPS` asks for.
fn reps() -> Fallible<usize> {
    match std::env::var("FABRIC_BENCH_REPS") {
        Ok(text) => Ok(text.parse()?),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_REPS),
        Err(e) => Err(e.into()),
    }
}

/// Times the M2 paths and prints every sample; checks nothing.
#[test]
#[ignore = "benchmark; scripts/bench-m2.sh runs it"]
fn m2_paths() -> Fallible {
    let reps = reps()?;
    let fixture = Fixture::new()?;
    // One warm-up run, not reported.
    fixture.run(&mut BTreeMap::new())?;
    let mut times = BTreeMap::new();
    for _ in 0..reps {
        fixture.run(&mut times)?;
    }
    println!("{}", report(&times, reps));
    Ok(())
}
