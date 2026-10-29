//! The time budget of the hot paths (issue #26). On the fixture of two
//! nodes over QUIC, one job under each harness Fabric drives is
//! suspended, moved, resumed on the target and pulled back, run after
//! run, and a daemon is started on an empty state directory and restarted
//! on a full one. Each op's time is held to the baseline `budget.toml`
//! records for this platform, and the processes it starts to the exact
//! counts recorded there, by program: git and the kill, lsof, pgrep and ps
//! the harness runs from their system paths, which the `counted-tools`
//! build takes from the test's `bin` (`FABRIC_SYSTEM_TOOLS`), and every
//! fake in that `bin`: each program the node's facts probe and each
//! harness.
//!
//! Every program the node's facts probe, but git, is a fake that answers
//! after a fixed 50 ms, so a probe costs about the same on every machine.
//! Probes run in parallel, so one run more than before can hide in the
//! time; its count shows it. git is the machine's own, linked into the
//! test's `bin`.
//!
//! Two warm-up runs come first, and they count processes: the counted
//! programs are then wrappers that note each start, and for the timed runs
//! the programs themselves again, so the counting costs them nothing. The
//! first run is each job's first move, which differs from the later ones;
//! a count that grows from the first run to the second grows with the
//! job's history.
//! Then `reps` runs are timed, and an op whose time at `percentile` is
//! over its limit is timed in `reps` more runs, and fails only when it is
//! over again. `FABRIC_BUDGET=report`, the default, prints the times and
//! fails on none of them; `FABRIC_BUDGET=enforce` fails on them;
//! `FABRIC_BUDGET=count` runs only the warm-up runs and times nothing, so
//! the counts can be checked on a machine too busy to time. A process
//! count that differs from the budget fails in all three.
//!
//! It is ignored by default, since it takes minutes. CONTRIBUTING.md says
//! how to run it and when the budget may change.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::world::{Env, Fallible, Node, Wire, edit, pair, write_executable};

/// The budget, read when the test is built.
const BUDGET: &str = include_str!("budget.toml");

/// How long every fake probed program waits before it answers, in seconds.
const PROBE_DELAY: &str = "0.05";

/// The harnesses Fabric drives, each a fake in the test's `bin`: Claude,
/// Codex and the ACP agent.
const HARNESSES: [&str; 3] = ["claude", "codex", "fabric-fake-agent"];

/// The programs counted, in the order the report lists them: git, the
/// harness's system tools, the probed fakes and the harnesses. Each is a
/// wrapper while the warm-up runs.
fn counted() -> Vec<&'static str> {
    let mut programs = vec!["git"];
    programs.extend(SYSTEM.map(|(name, _)| name));
    programs.extend(PROBED.map(|(name, _)| name));
    programs.extend(HARNESSES);
    programs
}

/// How many warm-up runs count processes.
const COUNTED_RUNS: usize = 2;

/// Where macOS and Linux keep each counted program but git, as the
/// harness looks for it.
const SYSTEM: [(&str, &[&str]); 4] = [
    ("kill", &["/bin/kill", "/usr/bin/kill"]),
    (
        "lsof",
        &["/usr/sbin/lsof", "/usr/bin/lsof", "/sbin/lsof", "/bin/lsof"],
    ),
    ("pgrep", &["/usr/bin/pgrep", "/bin/pgrep"]),
    ("ps", &["/bin/ps", "/usr/bin/ps"]),
];

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

/// Puts in the test's `bin` the fake Claude and Codex and a fake of every
/// program the node's facts probe but git, each answering after
/// `PROBE_DELAY`, as the fakes of the harnesses do too, and leaves Claude's
/// session registry empty, so no claude writes the session. The budget,
/// the bench and the A/B bench run on this.
pub(crate) fn install_fakes(env: &Env) -> Fallible {
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
    fs::create_dir_all(env.home.join(".claude/sessions"))?;
    Ok(())
}

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

/// What a time over its limit does, or whether ops are timed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// It is reported.
    Report,
    /// It fails the test.
    Enforce,
    /// Nothing is timed: only the warm-up runs run, and their process
    /// counts are checked.
    Count,
}

impl Mode {
    /// The mode `FABRIC_BUDGET` names: `report`, the default, `enforce`
    /// or `count`.
    fn from_env() -> Fallible<Self> {
        match std::env::var("FABRIC_BUDGET") {
            Err(std::env::VarError::NotPresent) => Ok(Mode::Report),
            Ok(mode) => Self::named(&mode),
            Err(e) => Err(format!("FABRIC_BUDGET: {e}").into()),
        }
    }

    /// The mode called `name`.
    fn named(name: &str) -> Fallible<Self> {
        match name {
            "report" => Ok(Mode::Report),
            "enforce" => Ok(Mode::Enforce),
            "count" => Ok(Mode::Count),
            other => {
                Err(format!("FABRIC_BUDGET must be report, enforce or count, not {other:?}").into())
            }
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mode::Report => "report",
            Mode::Enforce => "enforce",
            Mode::Count => "count",
        }
    }
}

/// This platform as `budget.toml` names it: `FABRIC_BUDGET_PLATFORM` when
/// it is set and not empty, else the OS and architecture, such as
/// `macos-aarch64`. CI names its macOS runner `macos-aarch64-github`,
/// which has no baselines, since it is not the Mac that timed those of
/// `macos-aarch64`.
fn platform() -> String {
    platform_named(std::env::var("FABRIC_BUDGET_PLATFORM").ok())
}

/// The platform `named`, or this machine's OS and architecture when it is
/// unnamed or empty.
fn platform_named(named: Option<String>) -> String {
    named
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH))
}

/// The CPU and its core count, for the report: a CI runner of another
/// kind explains a time no change of the code made.
fn machine() -> String {
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let linux = fs::read_to_string("/proc/cpuinfo").ok().and_then(|info| {
        info.lines().find_map(|line| {
            let (key, name) = line.split_once(':')?;
            (key.trim() == "model name").then(|| name.trim().to_owned())
        })
    });
    let cpu = linux
        .or_else(|| {
            let out = std::process::Command::new("sysctl")
                .args(["-n", "machdep.cpu.brand_string"])
                .output()
                .ok()
                .filter(|out| out.status.success())?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        })
        .filter(|cpu| !cpu.is_empty())
        .unwrap_or_else(|| "an unknown CPU".to_owned());
    format!("{cpu}, {cores} cores")
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
    /// A program the budget does not name for an op starts in no run.
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
        let counted = counted();
        // scripts/check-budget.sh reads which programs are counted from
        // the budget, so the list must be the one counted here.
        let listed: Vec<&str> = key("counted")?
            .as_array()
            .ok_or("budget.toml: counted is not a list")?
            .iter()
            .map(|name| {
                name.as_str()
                    .ok_or("budget.toml: counted names a program by a string")
            })
            .collect::<Result<_, _>>()?;
        if listed != counted {
            return Err(format!(
                "budget.toml: counted must name the programs the warm-up runs count, in their order: {counted:?}"
            )
            .into());
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
                if !counted.contains(&program.as_str()) {
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

    /// How many times `op` may start `program` in each counted run.
    fn allowed(&self, op: &str, program: &str) -> Vec<u64> {
        self.processes
            .get(op)
            .and_then(|counts| counts.get(program))
            .cloned()
            .unwrap_or_else(|| vec![0; COUNTED_RUNS])
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

/// How the test's `bin` holds a counted program while nothing counts.
enum Real {
    /// A symbolic link to it: git, the system tools and the ACP agent,
    /// which live outside `bin`.
    Link(PathBuf),
    /// A hard link to it, kept beside `bin`: a fake, which is then the same
    /// file it was before the counter, at the same path.
    Fake(PathBuf),
}

impl Real {
    /// Where the program itself is.
    fn path(&self) -> &Path {
        match self {
            Real::Link(path) | Real::Fake(path) => path,
        }
    }
}

/// The counted programs in the test's `bin`: the program itself while
/// timing, and while counting a wrapper that appends a byte to
/// `counts/<program>` and runs it.
struct Counter {
    bin: PathBuf,
    dir: PathBuf,
    real: Vec<(&'static str, Real)>,
}

impl Counter {
    /// Counts the programs of `env`'s `bin`, after `install_fakes` put the
    /// fakes there: each fake moves to `fakes/` beside it.
    fn new(env: &Env) -> Fallible<Self> {
        let bin = env.bin();
        let mut real = vec![("git", Real::Link(fs::read_link(bin.join("git"))?))];
        for (name, candidates) in SYSTEM {
            let found = candidates
                .iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
                .ok_or_else(|| format!("no {name}"))?;
            real.push((name, Real::Link(found)));
        }
        let fakes = env.root.join("fakes");
        fs::create_dir_all(&fakes)?;
        for name in PROBED.map(|(name, _)| name).into_iter().chain(HARNESSES) {
            let path = bin.join(name);
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                real.push((name, Real::Link(fs::read_link(&path)?)));
            } else {
                let kept = fakes.join(name);
                fs::rename(&path, &kept)?;
                real.push((name, Real::Fake(kept)));
            }
        }
        let dir = env.root.join("counts");
        fs::create_dir_all(&dir)?;
        let counter = Self { bin, dir, real };
        counter.off()?;
        Ok(counter)
    }

    /// Puts the wrappers in place, each by one rename.
    fn on(&self) -> Fallible {
        for (name, real) in &self.real {
            let script = format!(
                "#!/bin/sh\necho >> '{}'\nexec '{}' \"$@\"\n",
                self.dir.join(name).display(),
                real.path().display()
            );
            let new = self.bin.join(format!(".{name}.new"));
            write_executable(&new, script.as_bytes())?;
            fs::rename(&new, self.bin.join(name))?;
        }
        Ok(())
    }

    /// Puts the programs themselves in place, each by one rename.
    fn off(&self) -> Fallible {
        for (name, real) in &self.real {
            let new = self.bin.join(format!(".{name}.new"));
            match fs::remove_file(&new) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            match real {
                Real::Link(path) => std::os::unix::fs::symlink(path, &new)?,
                Real::Fake(path) => fs::hard_link(path, &new)?,
            }
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
        install_fakes(&env)?;
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
fn runs(counts: &[u64]) -> String {
    counts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("/")
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
            (None, _) if mode == Mode::Count => "counted only",
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
                         (baseline {} ms, the anchor's time). A slower op is a bug to fix, and \
                         its baseline never rises (CONTRIBUTING.md, Performance rule)",
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
        let mut processes = Vec::new();
        match warm.counts.get(&op) {
            None => {
                failures.push(format!("{op}: not counted"));
                processes.push("not counted".to_owned());
            }
            Some(started) => {
                for program in counted() {
                    let count = started
                        .get(program)
                        .cloned()
                        .unwrap_or_else(|| vec![0; COUNTED_RUNS]);
                    let limit = budget.allowed(&op, program);
                    if count != limit {
                        processes.push(format!(
                            "{program} {} (budget {})",
                            runs(&count),
                            runs(&limit)
                        ));
                        failures.push(format!(
                            "{op}: started {program} {} times in the counted runs, budget.toml \
                             says {}. Set the counts in budget.toml and say why in a new \
                             [[change]] entry",
                            runs(&count),
                            runs(&limit)
                        ));
                    } else if count.iter().any(|n| *n > 0) {
                        processes.push(format!("{program} {}", runs(&count)));
                    }
                }
                if processes.is_empty() {
                    processes.push("none".to_owned());
                }
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
         On {}. Times in ms: the {}th percentile of {} timed runs, the median, and the {}th \
         percentile of a second batch when the first was over the limit in enforce mode. \
         The limit is the larger of {} x baseline and baseline + {} ms. Processes are the \
         starts of each counted program in each of the {COUNTED_RUNS} warm-up runs; a \
         program not named started in none.\n\n",
        mode.name(),
        machine(),
        budget.percentile,
        budget.reps,
        budget.percentile,
        budget.ratio,
        budget.grace.as_millis(),
    );
    if mode == Mode::Count {
        let _ = writeln!(
            out,
            "Count mode: only the warm-up runs ran, and no op was timed.\n"
        );
    } else if budget.baseline.is_empty() {
        let _ = writeln!(
            out,
            "budget.toml has no baselines for {platform}, so its times are only reported.\n"
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
    let first = if mode == Mode::Count {
        Batch::default()
    } else {
        fixture.batch(budget.reps)?
    };
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

/// The budget the repository holds parses and names every op under
/// `[processes]`, each with only the programs it starts: a program left
/// out starts in no run, so a count of none written out would be a second
/// spelling of the same budget.
#[test]
fn the_budget_file_holds_every_op() -> Fallible {
    let budget = Budget::parse(BUDGET, &platform())?;
    for op in ops() {
        let counts = budget
            .processes
            .get(&op)
            .ok_or_else(|| format!("no process counts for {op}"))?;
        for (program, runs) in counts {
            assert!(
                runs.iter().any(|n| *n > 0),
                "{op} names {program}, which it never starts"
            );
        }
    }
    Ok(())
}

/// The budget names the programs the warm-up runs count, which
/// scripts/check-budget.sh reads to hold each one's counts at the base:
/// a list that leaves one out, or names one the runs do not count, fails
/// to parse.
#[test]
fn the_budget_names_the_programs_it_counts() -> Fallible {
    Budget::parse(BUDGET, &platform())?;
    let mut fewer = counted();
    fewer.pop();
    let mut more = counted();
    more.push("sh");
    for list in [fewer, more] {
        let text = BUDGET.replacen(
            &format!("counted = {:?}", counted()),
            &format!("counted = {list:?}"),
            1,
        );
        assert_ne!(text, BUDGET, "the budget holds no counted list to replace");
        assert!(Budget::parse(&text, &platform()).is_err(), "{list:?}");
    }
    Ok(())
}

/// Every op has a baseline on both platforms the budget holds times on:
/// CI's Linux runner, where CI enforces them, and the Mac of the macOS
/// baselines. An op with none there is only reported, so a slower one
/// would pass (M2-25).
#[test]
fn every_op_has_a_baseline_on_linux_and_macos() -> Fallible {
    for platform in ["linux-x86_64", "macos-aarch64"] {
        let budget = Budget::parse(BUDGET, platform)?;
        let missing: Vec<String> = ops()
            .into_iter()
            .filter(|op| !budget.baseline.contains_key(op))
            .collect();
        assert!(
            missing.is_empty(),
            "{platform} has no baseline for {missing:?}"
        );
    }
    Ok(())
}

/// CI holds every runner to the budget: the step that runs `hot_paths`
/// enforces it, where it once only reported (M2-25). A runner with no
/// baselines, as CI's macOS one, still only reports its times, and its
/// process counts fail in both modes.
#[test]
fn ci_enforces_the_budget() -> Fallible {
    let ci = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/ci.yml"
    ))?;
    let modes: Vec<&str> = ci
        .lines()
        .filter_map(|line| line.trim().strip_prefix("FABRIC_BUDGET:"))
        .map(str::trim)
        .collect();
    assert_eq!(modes, ["enforce"], "the FABRIC_BUDGET of ci.yml");
    Ok(())
}

/// `FABRIC_BUDGET_PLATFORM` names the platform when it is set and not empty.
#[test]
fn a_named_platform_replaces_this_machines() {
    let here = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    assert_eq!(platform_named(None), here);
    assert_eq!(platform_named(Some(String::new())), here);
    assert_eq!(
        platform_named(Some("macos-aarch64-github".to_owned())),
        "macos-aarch64-github"
    );
}

/// While the warm-up runs count, a fake in the test's `bin`, such as the
/// probed `node`, is a wrapper that counts each start, so a probe run more
/// often than the budget says fails even when the probes run in parallel
/// and the time hides it; afterwards the fake itself is back.
#[test]
fn the_counter_counts_a_fake_and_puts_it_back() -> Fallible {
    let env = Env::new()?;
    install_fakes(&env)?;
    let node = env.bin().join("node");
    let fake = fs::read(&node)?;
    let counter = Counter::new(&env)?;
    counter.on()?;
    for _ in 0..2 {
        let out = std::process::Command::new(&node)
            .arg("--version")
            .output()?;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(String::from_utf8(out.stdout)?, "v22.23.2\n");
    }
    assert_eq!(counter.read()?.get("node"), Some(&2), "node started twice");
    counter.off()?;
    assert_eq!(fs::read(&node)?, fake, "the fake is back");
    Ok(())
}

/// The rules on a made-up budget: the limit is the larger of the ratio and
/// the grace; a time over it fails in enforce mode only when a second
/// batch is over too, and never in report mode; a process count that
/// differs from the budget, higher or lower, fails in both modes.
#[test]
fn the_verdicts_follow_the_rules() -> Fallible {
    let ms = Duration::from_millis;
    let mut text = format!(
        "ratio = 1.3\ngrace_ms = 50\nreps = 7\npercentile = 25\ncounted = {:?}\n\
         [baseline.test]\n\"acp suspend\" = 1000\n\"cold start\" = 100\n[processes]\n",
        counted()
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
    // A baseline is the anchor's time and never rises, so the failure
    // asks for the op to be made fast again, not for a higher baseline.
    assert!(
        failures[0].contains("Performance rule") && !failures[0].contains("raise the baseline"),
        "{failures:?}"
    );

    // A program the budget does not name for an op starts in no run, so
    // one start of it fails.
    let mut probed = Batch::default();
    probed.counts.clone_from(&warm.counts);
    probed
        .counts
        .get_mut("codex move")
        .unwrap()
        .insert("node", vec![0, 1]);
    let (_, failures) = judge(&budget, Mode::Report, &probed, &timed(&[]), None);
    assert_eq!(
        failures,
        [
            "codex move: started node 0/1 times in the counted runs, budget.toml says 0/0. \
          Set the counts in budget.toml and say why in a new [[change]] entry"
        ]
    );

    // A count run times nothing and fails on no time, only on counts.
    assert_eq!(Mode::named("count")?, Mode::Count);
    let (rows, failures) = judge(&budget, Mode::Count, &warm, &Batch::default(), None);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(verdict(&rows, "acp suspend"), "counted only");

    for counts in [vec![1, 3], vec![1, 1]] {
        let mut changed = Batch::default();
        changed.counts.clone_from(&warm.counts);
        changed
            .counts
            .get_mut("codex resume")
            .unwrap()
            .insert("git", counts.clone());
        for (mode, times) in [(Mode::Report, timed(&[])), (Mode::Count, Batch::default())] {
            let (_, failures) = judge(&budget, mode, &changed, &times, None);
            assert_eq!(failures.len(), 1, "{mode:?} {counts:?}: {failures:?}");
            assert!(
                failures[0].starts_with("codex resume: started git"),
                "{failures:?}"
            );
        }
    }
    Ok(())
}
