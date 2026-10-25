//! The time budget of the hot paths (issue #26). On the fixture of two
//! nodes over QUIC, one job under each harness Fabric drives is
//! suspended, moved, resumed on the target and pulled back, run after
//! run, and a daemon is started on an empty state directory and restarted
//! on a full one. Each op's time is held to the baseline `budget.toml`
//! records for this platform, and the git and ps processes it starts to
//! the exact counts recorded there.
//!
//! Every program the node's facts probe, but git, is a fake that answers
//! after a fixed 50 ms, so a probe costs about the same on every machine
//! and a probe run once more than before shows. git is the machine's own,
//! linked into the test's `bin`.
//!
//! Two warm-up runs come first, and they count processes: git and ps are
//! then wrappers that note each start, and plain links again for the
//! timed runs, so the counting costs them nothing. The first run is each
//! job's first move, which differs from the later ones; a count that
//! grows from the first run to the second grows with the job's history.
//! Then `reps` runs are timed, and an op whose time at `percentile` is
//! over its limit is timed in `reps` more runs, and fails only when it is
//! over again. `FABRIC_BUDGET=report`, the default, prints the times and
//! fails on none of them; `FABRIC_BUDGET=enforce` fails on them. A process
//! count that differs from the budget fails in both.
//!
//! It is ignored by default, since it takes minutes. CONTRIBUTING.md says
//! how to run it and how to accept a slowdown.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::world::{Env, Fallible, Node, Wire, edit, pair, write_executable};

/// The budget, read when the test is built.
const BUDGET: &str = include_str!("budget.toml");

/// How long every fake probed program waits before it answers, in seconds.
const PROBE_DELAY: &str = "0.05";

/// The programs counted: each is a wrapper while the warm-up runs.
const COUNTED: [&str; 2] = ["git", "ps"];

/// How many warm-up runs count processes.
const COUNTED_RUNS: usize = 2;

/// Where macOS and Linux keep `ps`.
const PS: [&str; 2] = ["/bin/ps", "/usr/bin/ps"];

/// Each program the node's facts probe (`NodeFacts::measure` in
/// `crates/fabricd/src/facts.rs`), but git and the harnesses, with the
/// shell its fake runs after the delay. A probe added there needs its fake
/// here, or it runs whatever the machine has. `defaults` finds no setting,
/// as on a Mac that never turned iCloud Drive on.
const PROBED: [(&str, &str); 13] = [
    ("id", "echo fabric"),
    ("sw_vers", "echo 26.0"),
    ("xcrun", "echo 26.0"),
    (
        "node",
        "if [ \"$1\" = -p ]; then echo 127; else echo v22.23.2; fi",
    ),
    ("npm", "echo 10.9.3"),
    (
        "pnpm",
        "if [ \"$1\" = store ]; then echo \"$HOME/.pnpm-store/v10\"; else echo 10.18.0; fi",
    ),
    ("yarn", "echo 1.22.22"),
    ("bun", "echo 1.3.14"),
    ("rustc", "echo 'rustc 1.97.0'"),
    ("cargo", "echo 'cargo 1.97.0'"),
    (
        "python3",
        "if [ \"$1\" = -c ]; then echo /usr; else echo 'Python 3.13.5'; fi",
    ),
    ("uv", "echo 'uv 0.9.0'"),
    ("defaults", "exit 1"),
];

/// A job shape: the harness its job runs under.
struct Shape {
    /// Its name in the budget.
    name: &'static str,
    /// The harness, as `fabric job new --harness` takes it.
    harness: &'static str,
}

/// One shape per kind of harness.
const SHAPES: [Shape; 3] = [
    Shape {
        name: "acp",
        harness: crate::AGENT,
    },
    Shape {
        name: "claude",
        harness: "claude",
    },
    Shape {
        name: "codex",
        harness: "codex",
    },
];

/// The ops each shape's job goes through in one run, in order.
const JOB_OPS: [&str; 4] = ["suspend", "move", "resume", "pull"];

/// The ops a daemon goes through in one run.
const DAEMON_OPS: [&str; 2] = ["cold start", "restart"];

/// Every op the budget holds, in the order the report lists them.
fn ops() -> Vec<String> {
    let mut ops: Vec<String> = SHAPES
        .iter()
        .flat_map(|shape| JOB_OPS.map(|op| format!("{} {op}", shape.name)))
        .collect();
    ops.extend(DAEMON_OPS.map(str::to_owned));
    ops
}

/// What a time over its limit does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// It is reported.
    Report,
    /// It fails the test.
    Enforce,
}

impl Mode {
    /// The mode `FABRIC_BUDGET` names: `report`, the default, or `enforce`.
    fn from_env() -> Fallible<Self> {
        match std::env::var("FABRIC_BUDGET") {
            Err(std::env::VarError::NotPresent) => Ok(Mode::Report),
            Ok(mode) if mode == "report" => Ok(Mode::Report),
            Ok(mode) if mode == "enforce" => Ok(Mode::Enforce),
            other => Err(format!("FABRIC_BUDGET must be report or enforce, not {other:?}").into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mode::Report => "report",
            Mode::Enforce => "enforce",
        }
    }
}

/// This platform as `budget.toml` names it, such as `macos-aarch64`.
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What `budget.toml` holds for this platform.
#[derive(Debug)]
struct Budget {
    ratio: f64,
    grace: Duration,
    reps: usize,
    percentile: usize,
    /// Each op's baseline on this platform; empty where it has none yet.
    baseline: BTreeMap<String, Duration>,
    /// Each op's exact count of starts by program, one per counted run.
    processes: BTreeMap<String, BTreeMap<String, Vec<u64>>>,
}

/// `value` as a count or a time in ms: a whole number of at least 0.
fn whole(value: &toml::Value, what: &str) -> Fallible<u64> {
    let value = value
        .as_integer()
        .ok_or_else(|| format!("budget.toml: {what} is not a whole number"))?;
    u64::try_from(value).map_err(|_| format!("budget.toml: {what} is below 0").into())
}

impl Budget {
    fn parse(text: &str, platform: &str) -> Fallible<Self> {
        let table: toml::Table = text.parse()?;
        let key = |key: &str| {
            table
                .get(key)
                .ok_or_else(|| format!("budget.toml: no {key}"))
        };
        let ratio = key("ratio")?
            .as_float()
            .filter(|ratio| ratio.is_finite() && *ratio >= 1.0)
            .ok_or("budget.toml: ratio must be a number of at least 1.0")?;
        let reps = usize::try_from(whole(key("reps")?, "reps")?)?;
        if !(1..=32).contains(&reps) {
            return Err("budget.toml: reps must be 1 to 32".into());
        }
        let percentile = usize::try_from(whole(key("percentile")?, "percentile")?)?;
        if percentile > 100 {
            return Err("budget.toml: percentile must be 0 to 100".into());
        }
        let known = ops();
        let mut baseline = BTreeMap::new();
        let baselines = key("baseline")?
            .as_table()
            .ok_or("budget.toml: baseline is not a table")?;
        if let Some(times) = baselines.get(platform) {
            let times = times
                .as_table()
                .ok_or_else(|| format!("budget.toml: baseline.{platform} is not a table"))?;
            for (op, ms) in times {
                if !known.contains(op) {
                    return Err(
                        format!("budget.toml: baseline.{platform} names no op {op:?}").into(),
                    );
                }
                let ms = whole(ms, &format!("the baseline of {op}"))?;
                baseline.insert(op.clone(), Duration::from_millis(ms));
            }
        }
        let mut processes = BTreeMap::new();
        for (op, counts) in key("processes")?
            .as_table()
            .ok_or("budget.toml: processes is not a table")?
        {
            if !known.contains(op) {
                return Err(format!("budget.toml: processes names no op {op:?}").into());
            }
            let counts = counts
                .as_table()
                .ok_or_else(|| format!("budget.toml: processes of {op:?} is not a table"))?;
            let mut runs = BTreeMap::new();
            for (program, per_run) in counts {
                if !COUNTED.contains(&program.as_str()) {
                    return Err(format!("budget.toml: {program} is not counted").into());
                }
                let what = format!("{op} {program}");
                let per_run = per_run
                    .as_array()
                    .filter(|per_run| per_run.len() == COUNTED_RUNS)
                    .ok_or_else(|| {
                        format!("budget.toml: {what} needs {COUNTED_RUNS} counts, one per run")
                    })?
                    .iter()
                    .map(|count| whole(count, &what))
                    .collect::<Fallible<Vec<u64>>>()?;
                runs.insert(program.clone(), per_run);
            }
            processes.insert(op.clone(), runs);
        }
        Ok(Self {
            ratio,
            grace: Duration::from_millis(whole(key("grace_ms")?, "grace_ms")?),
            reps,
            percentile,
            baseline,
            processes,
        })
    }

    /// The time an op whose baseline is `base` may take.
    fn limit(&self, base: Duration) -> Duration {
        base.mul_f64(self.ratio).max(base + self.grace)
    }

    /// The op's time at the budget's percentile in `batch`, if it was timed.
    fn at(&self, batch: &Batch, op: &str) -> Option<Duration> {
        batch
            .times
            .get(op)
            .and_then(|times| percentile(times, self.percentile))
    }

    /// True when some op of `batch` is over its limit.
    fn any_over(&self, batch: &Batch) -> bool {
        self.baseline
            .iter()
            .any(|(op, base)| self.at(batch, op).is_some_and(|at| at > self.limit(*base)))
    }
}

/// The time at `percentile` of `samples`, rounded down to a sample: of
/// seven, the 25th percentile is the second fastest.
fn percentile(samples: &[Duration], percentile: usize) -> Option<Duration> {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let last = sorted.len().checked_sub(1)?;
    sorted.get(last * percentile / 100).copied()
}

/// The counted programs in the test's `bin`: a link to the real program
/// while timing, and while counting a wrapper that appends a byte to
/// `counts/<program>` and runs it.
struct Counter {
    bin: PathBuf,
    dir: PathBuf,
    real: [(&'static str, PathBuf); 2],
}

impl Counter {
    fn new(env: &Env) -> Fallible<Self> {
        let git = fs::read_link(env.bin().join("git"))?;
        let ps = PS
            .iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .ok_or("no ps")?;
        let dir = env.root.join("counts");
        fs::create_dir_all(&dir)?;
        let counter = Self {
            bin: env.bin(),
            dir,
            real: [("git", git), ("ps", ps)],
        };
        counter.off()?;
        Ok(counter)
    }

    /// Puts the wrappers in place, each by one rename.
    fn on(&self) -> Fallible {
        for (name, real) in &self.real {
            let script = format!(
                "#!/bin/sh\necho >> '{}'\nexec '{}' \"$@\"\n",
                self.dir.join(name).display(),
                real.display()
            );
            let new = self.bin.join(format!(".{name}.new"));
            write_executable(&new, script.as_bytes())?;
            fs::rename(&new, self.bin.join(name))?;
        }
        Ok(())
    }

    /// Puts the links in place, each by one rename.
    fn off(&self) -> Fallible {
        for (name, real) in &self.real {
            let new = self.bin.join(format!(".{name}.new"));
            match fs::remove_file(&new) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            std::os::unix::fs::symlink(real, &new)?;
            fs::rename(&new, self.bin.join(name))?;
        }
        Ok(())
    }

    /// How many times each program started under a wrapper so far.
    fn read(&self) -> Fallible<BTreeMap<&'static str, u64>> {
        let mut counts = BTreeMap::new();
        for (name, _) in &self.real {
            let count = match fs::metadata(self.dir.join(name)) {
                Ok(meta) => meta.len(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                Err(e) => return Err(e.into()),
            };
            counts.insert(*name, count);
        }
        Ok(counts)
    }
}

/// What some runs measured: each op's times, and in counted runs the
/// processes each op started, by program, one count per run.
#[derive(Default)]
struct Batch {
    times: BTreeMap<String, Vec<Duration>>,
    counts: BTreeMap<String, BTreeMap<&'static str, Vec<u64>>>,
}

impl Batch {
    /// Times `step` as `op`, and counts what it starts when `counter` is given.
    fn time<T>(
        &mut self,
        op: &str,
        counter: Option<&Counter>,
        step: impl FnOnce() -> Fallible<T>,
    ) -> Fallible<T> {
        let before = counter.map(Counter::read).transpose()?;
        let started = Instant::now();
        let out = step()?;
        let took = started.elapsed();
        self.times.entry(op.to_owned()).or_default().push(took);
        if let (Some(counter), Some(before)) = (counter, before) {
            let counts = self.counts.entry(op.to_owned()).or_default();
            for (name, after) in counter.read()? {
                let started = after.saturating_sub(before.get(name).copied().unwrap_or(0));
                counts.entry(name).or_default().push(started);
            }
        }
        Ok(out)
    }
}

/// The two paired nodes and the jobs they pass between them.
struct Fixture {
    env: Arc<Env>,
    a: Node,
    b: Node,
    jobs: Vec<(&'static Shape, String)>,
    counter: Counter,
    /// How many runs have started, which names each run's cold node.
    runs: usize,
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
            let script = fs::read_to_string(&path)?;
            let (shebang, rest) = script.split_once('\n').ok_or("an empty fake")?;
            write_executable(
                &path,
                format!("{shebang}\nsleep {PROBE_DELAY}\n{rest}").as_bytes(),
            )?;
        }
        // An empty session registry: no claude writes the session.
        fs::create_dir_all(env.home.join(".claude/sessions"))?;
        let counter = Counter::new(&env)?;
        let mut a = Node::new(&env, "a", Wire::Quic)?;
        let mut b = Node::new(&env, "b", Wire::Quic)?;
        a.start(None)?;
        b.start(None)?;
        pair(&a, &b)?;
        let mut jobs = Vec::new();
        for shape in &SHAPES {
            let job = crate::new_job(&env, &a, shape.harness)?;
            a.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
            edit(&env, &job)?;
            jobs.push((shape, job));
        }
        Ok(Self {
            env,
            a,
            b,
            jobs,
            counter,
            runs: 0,
        })
    }

    /// One run of every op into `batch`, counting processes when `count`.
    fn run(&mut self, batch: &mut Batch, count: bool) -> Fallible {
        self.runs += 1;
        let counter = count.then_some(&self.counter);
        for (shape, job) in &self.jobs {
            let op = |name: &str| format!("{} {name}", shape.name);
            // New work since the last capture, as a turn leaves.
            fs::write(
                self.env.work(job).join("notes.txt"),
                format!("untracked, run {}\n", self.runs),
            )?;
            batch.time(&op("suspend"), counter, || {
                self.a.ok(&["job", "suspend", job])
            })?;
            batch.time(&op("move"), counter, || {
                self.a.ok(&["job", "move", job, "--to", &self.b.id])
            })?;
            batch.time(&op("resume"), counter, || {
                self.b
                    .ok(&["job", "resume", job, "--headless", "--prompt", "next"])
            })?;
            // New work on b too, before the pull suspends it there. A capture
            // dates its commits to the second, so one of the same tree in
            // the same second as the last makes the same commits and an
            // empty bundle, which every later restore skips: the git counts
            // would then depend on how fast the ops before it ran.
            fs::write(
                self.env.work(job).join("notes.txt"),
                format!("untracked, run {}, after the turn on b\n", self.runs),
            )?;
            batch.time(&op("pull"), counter, || {
                self.a
                    .ok(&["job", "resume", job, "--headless", "--prompt", "back"])
            })?;
        }
        // A node lives as long as the test, and so may its name.
        let name: &'static str = Box::leak(format!("cold{}", self.runs).into_boxed_str());
        let mut cold = Node::new(&self.env, name, Wire::Quic)?;
        batch.time("cold start", counter, || cold.start(None))?;
        cold.kill()?;
        self.a.kill()?;
        batch.time("restart", counter, || self.a.start(None))?;
        Ok(())
    }

    /// The warm-up runs, which count processes.
    fn warm_up(&mut self) -> Fallible<Batch> {
        let mut batch = Batch::default();
        self.counter.on()?;
        for _ in 0..COUNTED_RUNS {
            self.run(&mut batch, true)?;
        }
        self.counter.off()?;
        Ok(batch)
    }

    /// `reps` timed runs.
    fn batch(&mut self, reps: usize) -> Fallible<Batch> {
        let mut batch = Batch::default();
        for _ in 0..reps {
            self.run(&mut batch, false)?;
        }
        Ok(batch)
    }
}

/// One op's line in the report.
struct Row {
    op: String,
    at: Option<Duration>,
    median: Option<Duration>,
    baseline: Option<Duration>,
    limit: Option<Duration>,
    again: Option<Duration>,
    processes: String,
    verdict: &'static str,
}

/// Counts, one per run, as `53/59`.
fn runs(counts: Option<&Vec<u64>>) -> String {
    counts.map_or_else(
        || "none".to_owned(),
        |counts| {
            counts
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("/")
        },
    )
}

/// The report's rows and the failures, from the warm-up's counts and the
/// timed batches.
fn judge(
    budget: &Budget,
    mode: Mode,
    warm: &Batch,
    first: &Batch,
    second: Option<&Batch>,
) -> (Vec<Row>, Vec<String>) {
    let mut failures = Vec::new();
    let mut rows = Vec::new();
    for op in ops() {
        let times = first.times.get(&op).map(Vec::as_slice).unwrap_or_default();
        let at = budget.at(first, &op);
        let baseline = budget.baseline.get(&op).copied();
        let limit = baseline.map(|base| budget.limit(base));
        let again = second.and_then(|batch| budget.at(batch, &op));
        let verdict = match (at, limit) {
            (None, _) => {
                failures.push(format!("{op}: not timed"));
                "not timed"
            }
            (Some(_), None) => "no baseline",
            (Some(at), Some(limit)) if at <= limit => "within",
            (Some(_), Some(_)) if mode == Mode::Report => "over",
            (Some(at), Some(limit)) => match again {
                Some(again) if again > limit => {
                    failures.push(format!(
                        "{op}: {} ms, then {} ms in a second batch, over its limit of {} ms \
                         (baseline {} ms). If the slowdown is intended, raise the baseline in \
                         budget.toml and say why in a new [[change]] entry",
                        at.as_millis(),
                        again.as_millis(),
                        limit.as_millis(),
                        baseline.unwrap_or_default().as_millis(),
                    ));
                    "over twice"
                }
                _ => "over once",
            },
        };
        let counted = warm.counts.get(&op);
        let allowed = budget.processes.get(&op);
        let mut processes = Vec::new();
        for program in COUNTED {
            let count = counted.and_then(|counts| counts.get(program));
            let limit = allowed.and_then(|limits| limits.get(program));
            if count == limit {
                processes.push(format!("{program} {}", runs(count)));
            } else {
                processes.push(format!(
                    "{program} {} (budget {})",
                    runs(count),
                    runs(limit)
                ));
                failures.push(format!(
                    "{op}: started {program} {} times in the counted runs, budget.toml says {}. \
                     Set the counts in budget.toml and say why in a new [[change]] entry",
                    runs(count),
                    runs(limit)
                ));
            }
        }
        rows.push(Row {
            op,
            at,
            median: percentile(times, 50),
            baseline,
            limit,
            again,
            processes: processes.join(", "),
            verdict,
        });
    }
    (rows, failures)
}

/// The rows as a Markdown table, under a heading that says how they were measured.
fn report(budget: &Budget, mode: Mode, rows: &[Row]) -> String {
    let ms = |d: Option<Duration>| d.map_or_else(|| "-".to_owned(), |d| d.as_millis().to_string());
    let platform = platform();
    let mut out = format!(
        "### Time budget, {platform}, {}\n\n\
         Times in ms: the {}th percentile of {} timed runs, the median, and the {}th \
         percentile of a second batch when the first was over the limit in enforce mode. \
         The limit is the larger of {} x baseline and baseline + {} ms. Processes are the \
         starts of git and ps in each of the {COUNTED_RUNS} warm-up runs.\n\n",
        mode.name(),
        budget.percentile,
        budget.reps,
        budget.percentile,
        budget.ratio,
        budget.grace.as_millis(),
    );
    if budget.baseline.is_empty() {
        let _ = writeln!(
            out,
            "budget.toml has no baselines for {platform} yet, so its times are only reported.\n"
        );
    }
    let p = budget.percentile;
    let _ = writeln!(
        out,
        "| op | p{p} | median | baseline | limit | second p{p} | processes | verdict |\n\
         |---|---:|---:|---:|---:|---:|---|---|"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            row.op,
            ms(row.at),
            ms(row.median),
            ms(row.baseline),
            ms(row.limit),
            ms(row.again),
            row.processes,
            row.verdict
        );
    }
    out
}

/// Appends `text` to the step summary GitHub Actions shows, when there is one.
fn summarize(text: &str) -> Fallible {
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?
            .write_all(format!("{text}\n").as_bytes())?;
    }
    Ok(())
}

/// The hot paths stay within the time budget and start exactly the
/// processes it records.
#[test]
#[ignore = "takes minutes; CI runs it in a step of its own"]
fn hot_paths() -> Fallible {
    let budget = Budget::parse(BUDGET, &platform())?;
    let mode = Mode::from_env()?;
    let mut fixture = Fixture::new()?;
    let warm = fixture.warm_up()?;
    let first = fixture.batch(budget.reps)?;
    let second = if mode == Mode::Enforce && budget.any_over(&first) {
        Some(fixture.batch(budget.reps)?)
    } else {
        None
    };
    let (rows, failures) = judge(&budget, mode, &warm, &first, second.as_ref());
    let report = report(&budget, mode, &rows);
    println!("{report}");
    summarize(&report)?;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

/// The budget the repository holds parses, names every op, and gives each
/// op a count for every counted program.
#[test]
fn the_budget_file_holds_every_op() -> Fallible {
    let budget = Budget::parse(BUDGET, &platform())?;
    for op in ops() {
        let counts = budget
            .processes
            .get(&op)
            .ok_or_else(|| format!("no process counts for {op}"))?;
        for program in COUNTED {
            assert!(counts.contains_key(program), "{op} has no {program} count");
        }
    }
    Ok(())
}

/// The rules on a made-up budget: the limit is the larger of the ratio and
/// the grace; a time over it fails in enforce mode only when a second
/// batch is over too, and never in report mode; a process count that
/// differs from the budget, higher or lower, fails in both modes.
#[test]
fn the_verdicts_follow_the_rules() -> Fallible {
    let ms = Duration::from_millis;
    let mut text = String::from(
        "ratio = 1.3\ngrace_ms = 50\nreps = 7\npercentile = 25\n\
         [baseline.test]\n\"acp suspend\" = 1000\n\"cold start\" = 100\n[processes]\n",
    );
    for op in ops() {
        writeln!(text, "\"{op}\" = {{ git = [1, 2], ps = [0, 0] }}")?;
    }
    let budget = Budget::parse(&text, "test")?;
    assert_eq!(budget.limit(ms(1000)), ms(1300), "the ratio is larger");
    assert_eq!(budget.limit(ms(100)), ms(150), "the grace is larger");
    let samples = [7, 1, 6, 2, 5, 3, 4].map(ms);
    assert_eq!(percentile(&samples, 25), Some(ms(2)), "the second fastest");
    assert_eq!(percentile(&samples, 50), Some(ms(4)), "the median");

    // Seven runs of every op at 10 ms, and of `slow` at its time.
    let timed = |slow: &[(&str, u64)]| {
        let mut batch = Batch::default();
        for op in ops() {
            let at = slow
                .iter()
                .find(|(name, _)| *name == op)
                .map_or(10, |(_, at)| *at);
            batch.times.insert(op, vec![ms(at); 7]);
        }
        batch
    };
    let mut warm = Batch::default();
    for op in ops() {
        let counts = warm.counts.entry(op).or_default();
        counts.insert("git", vec![1, 2]);
        counts.insert("ps", vec![0, 0]);
    }
    let verdict = |rows: &[Row], op: &str| {
        rows.iter()
            .find(|row| row.op == op)
            .map(|row| row.verdict)
            .unwrap()
    };

    let over = timed(&[("acp suspend", 1400), ("cold start", 140)]);
    let (rows, failures) = judge(&budget, Mode::Report, &warm, &over, None);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(verdict(&rows, "acp suspend"), "over");
    assert_eq!(verdict(&rows, "cold start"), "within", "within the grace");
    assert_eq!(verdict(&rows, "acp move"), "no baseline");

    let back = timed(&[("acp suspend", 1200)]);
    let (rows, failures) = judge(&budget, Mode::Enforce, &warm, &over, Some(&back));
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(verdict(&rows, "acp suspend"), "over once");

    let (rows, failures) = judge(&budget, Mode::Enforce, &warm, &over, Some(&over));
    assert_eq!(verdict(&rows, "acp suspend"), "over twice");
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("acp suspend: 1400 ms"),
        "{failures:?}"
    );

    for counts in [vec![1, 3], vec![1, 1]] {
        let mut changed = Batch::default();
        changed.counts.clone_from(&warm.counts);
        changed
            .counts
            .get_mut("codex resume")
            .unwrap()
            .insert("git", counts.clone());
        let (_, failures) = judge(&budget, Mode::Report, &changed, &timed(&[]), None);
        assert_eq!(failures.len(), 1, "{counts:?}: {failures:?}");
        assert!(
            failures[0].starts_with("codex resume: started git"),
            "{failures:?}"
        );
    }
    Ok(())
}
