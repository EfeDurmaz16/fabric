//! The A/B bench: two builds of `fabric`, the anchor (main before a change)
//! and a candidate, each driving a fixture of its own, the one `budget` and
//! `bench` use (two nodes over QUIC, fake probed programs that answer after
//! 50 ms), through the same ops in the same order. The builds take turns op
//! by op, and the build that goes first alternates from run to run, so a
//! change in the machine's load reaches both alike and neither always goes
//! first. `scripts/bench-ab.sh` builds both and runs it.
//!
//! One run is, per harness (ACP, Claude, Codex), a new job (`new`), its
//! first headless turn (`launch`) and an approval (`approve`); an approval
//! on a job whose ledger grows a record a run (`ledger append`); the CLI's
//! cold start (`cli version`) and a round trip to the node (`cli ls`);
//! then, per harness, a job suspended (`suspend`), moved to the other node
//! (`move`), resumed there with a headless turn (`resume`) and pulled back
//! with another (`pull`); last, a daemon started on an empty state
//! directory (`cold start`) and the first node killed and started again
//! (`restart`).
//!
//! It runs in two phases, each on fixtures of its own.
//!
//! - Counted. With `FABRIC_AB_COUNTERS` naming the counter library that
//!   `scripts/perf-ab/counters.c` builds, every `fabric` process keeps a
//!   page of counters, and each op records what all of them did while it
//!   ran: the processes they started, by program; file flushes, by kind and
//!   by class of file; opens, renames and unlinks; allocations; the bytes
//!   and calls of every read and write, by class of file. Also the peak
//!   resident size and CPU time of the CLI processes the op started, and
//!   each daemon's resident size after it. None of this depends on how
//!   loaded the machine is. One warm-up run, then `FABRIC_AB_COUNTED` runs,
//!   3 when unset.
//! - Timed. Two warm-up runs, since a job's first move differs from the
//!   later ones, then `FABRIC_AB_PAIRS` runs, 15 when unset: each op's time
//!   under both builds in one run is a pair. Each daemon's resident size is
//!   read after every run.
//!
//! The counted phase also keeps every process a `fabric` process started,
//! with its arguments, in `spawns.tsv`, and counts each op's commands as
//! `cmd:<command>`, such as `cmd:git symbolic-ref`.
//!
//! Every sample goes to `samples.tsv` in `FABRIC_AB_OUT`, and the report to
//! `report.md` there: per op, both medians and the median of the pairs'
//! ratios, candidate over anchor, with a 95% bootstrap interval, then the
//! counts and each daemon's resident size, after the counted ops and after
//! the timed runs. With `FABRIC_AB_PAIRS=0` only the counted phase runs.
//! `ab::analyze` writes the report again from `samples.tsv`.
//! `FABRIC_AB_ANCHOR` and `FABRIC_AB_CANDIDATE` name the two builds.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::world::{Env, Fallible, Node, Wire, edit, pair};

/// The harness of each job shape, by its name in the report.
const SHAPES: [(&str, &str); 3] = [
    ("acp", crate::AGENT),
    ("claude", "claude"),
    ("codex", "codex"),
];

/// The two builds, in the order of the report's columns.
const SIDES: [&str; 2] = ["anchor", "candidate"];

/// Timed runs when `FABRIC_AB_PAIRS` is unset, and the fewest the
/// performance rule accepts as evidence.
const DEFAULT_PAIRS: usize = 15;

/// Counted runs when `FABRIC_AB_COUNTED` is unset.
const DEFAULT_COUNTED: usize = 3;

/// Untimed runs before the timed ones: a job's first move differs from
/// the later ones.
const TIMED_WARM_UPS: usize = 2;

/// How long a counted op waits after it returns before the counters are
/// read, for work the daemons finish after answering.
const SETTLE: Duration = Duration::from_millis(150);

/// Resamples of the bootstrap interval.
const RESAMPLES: usize = 10_000;

/// The scalar words of a counter page, by name, in the order of `enum
/// word` in `scripts/perf-ab/counters.c`, which writes them.
const WORDS: [&str; 17] = [
    "magic",
    "pid",
    "exited",
    "maxrss",
    "cpu_us",
    "spawns",
    "execs",
    "fsync",
    "fullfsync",
    "barrierfsync",
    "opens",
    "renames",
    "unlinks",
    "mallocs",
    "malloc_bytes",
    "frees",
    "lock",
];

/// The classes of file a read, write or flush touches, in the order of
/// `enum class` in `scripts/perf-ab/counters.c`.
const CLASSES: [&str; 7] = [
    "journal", "ledger", "jobs", "store", "state", "outside", "ipc",
];

/// The counters kept per class, after the scalar words, in their order.
const PER_CLASS: [&str; 5] = [
    "write_bytes",
    "write_calls",
    "read_bytes",
    "read_calls",
    "syncs",
];

/// Where a page keeps the arguments of its process, and how many bytes.
const ROLE: (usize, usize) = (1024, 128);

/// Where a page keeps the programs its process started: slots of a count
/// and a name.
const PROGRAMS: (usize, usize, usize) = (2048, 32, 64);

/// One op of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    New(usize),
    Launch(usize),
    Approve(usize),
    LedgerAppend,
    CliVersion,
    CliLs,
    Suspend(usize),
    Move(usize),
    Resume(usize),
    Pull(usize),
    ColdStart,
    Restart,
}

impl Op {
    /// Every op of a run, in order.
    fn all() -> Vec<Op> {
        let mut ops = Vec::new();
        for shape in 0..SHAPES.len() {
            ops.extend([Op::New(shape), Op::Launch(shape), Op::Approve(shape)]);
        }
        ops.extend([Op::LedgerAppend, Op::CliVersion, Op::CliLs]);
        for shape in 0..SHAPES.len() {
            ops.extend([
                Op::Suspend(shape),
                Op::Move(shape),
                Op::Resume(shape),
                Op::Pull(shape),
            ]);
        }
        ops.extend([Op::ColdStart, Op::Restart]);
        ops
    }

    /// Its name in the samples and the report.
    fn name(self) -> String {
        let shaped = |shape: usize, op: &str| format!("{} {op}", SHAPES[shape].0);
        match self {
            Op::New(shape) => shaped(shape, "new"),
            Op::Launch(shape) => shaped(shape, "launch"),
            Op::Approve(shape) => shaped(shape, "approve"),
            Op::LedgerAppend => "ledger append".to_owned(),
            Op::CliVersion => "cli version".to_owned(),
            Op::CliLs => "cli ls".to_owned(),
            Op::Suspend(shape) => shaped(shape, "suspend"),
            Op::Move(shape) => shaped(shape, "move"),
            Op::Resume(shape) => shaped(shape, "resume"),
            Op::Pull(shape) => shaped(shape, "pull"),
            Op::ColdStart => "cold start".to_owned(),
            Op::Restart => "restart".to_owned(),
        }
    }
}

/// How long `step` took.
fn time<T>(step: impl FnOnce() -> Fallible<T>) -> Fallible<Duration> {
    let started = Instant::now();
    step()?;
    Ok(started.elapsed())
}

/// One build's fixture: two paired nodes, a job per harness that the
/// suspend, move, resume and pull cycle drives, and the job whose ledger
/// grows.
struct Side {
    env: Arc<Env>,
    a: Node,
    b: Node,
    cycled: Vec<String>,
    long: String,
    /// Per harness, the job `new` made in this run.
    made: Vec<String>,
    runs: usize,
    /// The directory of the counter pages, in a counted phase.
    counts: Option<tempfile::TempDir>,
}

impl Side {
    /// Starts the fixture of the build `fabric`, with its processes keeping
    /// counters when `counters` names the counter library.
    fn new(fabric: &Path, counters: Option<&Path>) -> Fallible<Self> {
        let mut vars: Vec<(OsString, OsString)> = Vec::new();
        let counts = match counters {
            Some(library) => {
                let dir = tempfile::Builder::new()
                    .prefix("fabab")
                    .tempdir_in("/tmp")?;
                vars.push(("DYLD_INSERT_LIBRARIES".into(), library.into()));
                vars.push(("FABRIC_AB_COUNTS".into(), dir.path().into()));
                Some(dir)
            }
            None => None,
        };
        let env = Env::with_fabric(fabric.to_owned(), vars)?;
        crate::budget::install_fakes(&env)?;
        let mut a = Node::new(&env, "a", Wire::Quic)?;
        let mut b = Node::new(&env, "b", Wire::Quic)?;
        a.start(None)?;
        b.start(None)?;
        pair(&a, &b)?;
        let mut cycled = Vec::new();
        for (_, harness) in SHAPES {
            let job = crate::new_job(&env, &a, harness)?;
            a.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
            edit(&env, &job)?;
            cycled.push(job);
        }
        let long = crate::new_job(&env, &a, crate::AGENT)?;
        Ok(Self {
            env,
            a,
            b,
            cycled,
            long,
            made: vec![String::new(); SHAPES.len()],
            runs: 0,
            counts,
        })
    }

    /// Runs `op`, with its untimed preparation, and returns how long the
    /// timed part took.
    fn step(&mut self, op: Op) -> Fallible<Duration> {
        match op {
            Op::New(shape) => {
                let started = Instant::now();
                let job = crate::new_job(&self.env, &self.a, SHAPES[shape].1)?;
                let took = started.elapsed();
                self.made[shape] = job;
                Ok(took)
            }
            Op::Launch(shape) => time(|| {
                self.a.ok(&[
                    "job",
                    "run",
                    &self.made[shape],
                    "--headless",
                    "--prompt",
                    "one",
                ])
            }),
            Op::Approve(shape) => time(|| {
                self.a
                    .ok(&["job", "approve", &self.made[shape], "--note", "bench"])
            }),
            Op::LedgerAppend => time(|| {
                self.a
                    .ok(&["job", "approve", &self.long, "--note", "bench"])
            }),
            Op::CliVersion => time(|| self.a.ok(&["--version"])),
            Op::CliLs => time(|| self.a.ok(&["job", "ls"])),
            Op::Suspend(shape) => {
                let job = &self.cycled[shape];
                // New work since the last capture, as a turn leaves.
                fs::write(
                    self.env.work(job).join("notes.txt"),
                    format!("untracked, run {}\n", self.runs),
                )?;
                time(|| self.a.ok(&["job", "suspend", job]))
            }
            Op::Move(shape) => time(|| {
                self.a
                    .ok(&["job", "move", &self.cycled[shape], "--to", &self.b.id])
            }),
            Op::Resume(shape) => time(|| {
                self.b.ok(&[
                    "job",
                    "resume",
                    &self.cycled[shape],
                    "--headless",
                    "--prompt",
                    "next",
                ])
            }),
            Op::Pull(shape) => {
                let job = &self.cycled[shape];
                // New work on b, so the pull's capture is not empty; see
                // `budget::Fixture::run`.
                fs::write(
                    self.env.work(job).join("notes.txt"),
                    format!("untracked, run {}, after the turn on b\n", self.runs),
                )?;
                time(|| {
                    self.a
                        .ok(&["job", "resume", job, "--headless", "--prompt", "back"])
                })
            }
            Op::ColdStart => {
                // A node lives as long as the test, and so may its name.
                let name: &'static str = Box::leak(format!("cold{}", self.runs).into_boxed_str());
                let mut cold = Node::new(&self.env, name, Wire::Quic)?;
                let took = time(|| cold.start(None))?;
                cold.kill()?;
                Ok(took)
            }
            Op::Restart => {
                self.a.kill()?;
                time(|| self.a.start(None))
            }
        }
    }

    /// The resident size of each daemon, in KiB, by node.
    fn daemon_rss(&self) -> Fallible<Vec<(&'static str, u64)>> {
        let mut sizes = Vec::new();
        for node in [&self.a, &self.b] {
            if let Some(pid) = node.daemon_pid() {
                sizes.push((node.name, rss_kib(pid)?));
            }
        }
        Ok(sizes)
    }
}

/// The resident size of process `pid` in KiB, as `ps` reports it.
fn rss_kib(pid: u32) -> Fallible<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

/// One process's counter page.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Page {
    /// The arguments it ran with, after the program.
    role: String,
    /// Every counter, by name: the scalar words, then `<counter>.<class>`.
    counts: BTreeMap<String, u64>,
    /// How many times it started each program, by file name.
    programs: BTreeMap<String, u64>,
}

impl Page {
    /// Reads a page `counters.c` wrote; `None` while it is not filled in yet.
    fn parse(bytes: &[u8]) -> Fallible<Option<Self>> {
        let word = |at: usize| -> Fallible<u64> {
            let bytes = bytes
                .get(at * 8..at * 8 + 8)
                .ok_or("a short counter page")?;
            Ok(u64::from_le_bytes(bytes.try_into()?))
        };
        if word(0)?.to_le_bytes() != *b"FABAB\x01\0\0" {
            return Ok(None);
        }
        let mut counts = BTreeMap::new();
        for (at, name) in WORDS.iter().enumerate() {
            counts.insert((*name).to_owned(), word(at)?);
        }
        for (group, counter) in PER_CLASS.iter().enumerate() {
            for (class, class_name) in CLASSES.iter().enumerate() {
                let at = WORDS.len() + group * CLASSES.len() + class;
                counts.insert(format!("{counter}.{class_name}"), word(at)?);
            }
        }
        let text = |range: std::ops::Range<usize>| -> Fallible<String> {
            let raw = bytes.get(range).ok_or("a short counter page")?;
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
        };
        let role = text(ROLE.0..ROLE.0 + ROLE.1)?;
        let mut programs = BTreeMap::new();
        let (start, slots, size) = PROGRAMS;
        for slot in 0..slots {
            let at = start + slot * size;
            let name = text(at + 8..at + size)?;
            if name.is_empty() {
                break;
            }
            *programs.entry(name).or_default() += word(at / 8)?;
        }
        Ok(Some(Self {
            role,
            counts,
            programs,
        }))
    }

    /// True for a daemon's page, false for a CLI command's.
    fn is_daemon(&self) -> bool {
        self.role.starts_with("serve")
    }
}

/// True when the file `name` ends in `.<extension>`.
fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|found| found == extension)
}

/// Every counter page in `dir`, by file name.
fn read_pages(dir: &Path) -> Fallible<BTreeMap<String, Page>> {
    let mut pages = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if has_extension(&name, "page")
            && let Some(page) = Page::parse(&fs::read(entry.path())?)?
        {
            pages.insert(name, page);
        }
    }
    Ok(pages)
}

/// How long each spawn log in `dir` is, by file name.
fn log_lengths(dir: &Path) -> Fallible<BTreeMap<String, u64>> {
    let mut lengths = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if has_extension(&name, "spawns") {
            lengths.insert(name, entry.metadata()?.len());
        }
    }
    Ok(lengths)
}

/// The lines the spawn logs in `dir` gained since they had `before`'s lengths.
fn new_spawns(dir: &Path, before: &BTreeMap<String, u64>) -> Fallible<Vec<String>> {
    let mut lines = Vec::new();
    for name in log_lengths(dir)?.keys() {
        let text = fs::read(dir.join(name))?;
        let from = usize::try_from(before.get(name).copied().unwrap_or(0))?;
        let added = text.get(from..).unwrap_or_default();
        lines.extend(String::from_utf8_lossy(added).lines().map(str::to_owned));
    }
    Ok(lines)
}

/// A spawn log line as the command it ran: git and its subcommand, the
/// fields `ps` asked for, the harness a held shell runs, or the program.
fn command(line: &str) -> String {
    let mut words = line.split(' ');
    let program = words.next().unwrap_or_default();
    let rest: Vec<&str> = words.collect();
    match program {
        "git" => {
            let mut at = 0;
            while let Some(word) = rest.get(at) {
                match *word {
                    "-c" | "-C" | "--git-dir" | "--work-tree" => at += 2,
                    _ if word.starts_with('-') => at += 1,
                    _ => return format!("git {word}"),
                }
            }
            "git".to_owned()
        }
        "ps" => match rest.iter().position(|word| *word == "-o") {
            Some(at) => format!("ps -o {}", rest.get(at + 1).unwrap_or(&"")),
            None => "ps".to_owned(),
        },
        "sh" | "bash" | "dash" => match rest.iter().position(|word| *word == "fabric-hold") {
            Some(at) => {
                let held = rest.get(at + 1).unwrap_or(&"");
                format!(
                    "{program} holding {}",
                    held.rsplit('/').next().unwrap_or(held)
                )
            }
            None => program.to_owned(),
        },
        _ => program.to_owned(),
    }
}

/// The counters that add up across processes and over time.
fn additive(name: &str) -> bool {
    !matches!(name, "magic" | "pid" | "exited" | "maxrss" | "lock")
}

/// What every process did between `before` and `after`: the change of each
/// additive counter summed over the pages, each program's starts as
/// `spawn:<program>`, and of the processes that started meanwhile but the
/// daemons, how many there were, their CPU time and their largest peak
/// resident size.
fn delta(before: &BTreeMap<String, Page>, after: &BTreeMap<String, Page>) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    let empty = BTreeMap::new();
    for (file, page) in after {
        let old = before.get(file);
        for (name, value) in &page.counts {
            if additive(name) {
                let was = old
                    .and_then(|old| old.counts.get(name))
                    .copied()
                    .unwrap_or(0);
                *out.entry(name.clone()).or_default() += value.saturating_sub(was);
            }
        }
        let old_programs = old.map_or(&empty, |old| &old.programs);
        for (program, count) in &page.programs {
            let was = old_programs.get(program).copied().unwrap_or(0);
            if *count > was {
                *out.entry(format!("spawn:{program}")).or_default() += count - was;
            }
        }
        if old.is_none() && !page.is_daemon() {
            *out.entry("cli_processes".to_owned()).or_default() += 1;
            *out.entry("cli_cpu_us".to_owned()).or_default() += page.counts["cpu_us"];
            let peak = out.entry("cli_maxrss_kib".to_owned()).or_default();
            *peak = (*peak).max(page.counts["maxrss"] / 1024);
        }
    }
    out
}

/// The samples file: a header, then one line per sample.
struct Samples {
    text: String,
}

impl Samples {
    fn new() -> Self {
        Self {
            text: "phase\trun\top\tside\tfirst\tmetric\tvalue\n".to_owned(),
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one column each")]
    fn push(
        &mut self,
        phase: &str,
        run: usize,
        op: &str,
        side: &str,
        first: &str,
        metric: &str,
        value: u64,
    ) {
        let _ = writeln!(
            self.text,
            "{phase}\t{run}\t{op}\t{side}\t{first}\t{metric}\t{value}"
        );
    }
}

/// The builds and settings the environment names.
struct Setup {
    builds: [PathBuf; 2],
    out: PathBuf,
    pairs: usize,
    counted: usize,
    counters: Option<PathBuf>,
}

/// `name` as a count, or `default` when it is unset.
fn count_var(name: &str, default: usize) -> Fallible<usize> {
    match std::env::var(name) {
        Ok(text) => Ok(text.parse()?),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(e.into()),
    }
}

/// The path `name` holds; it must be set.
fn path_var(name: &str) -> Fallible<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not set; scripts/bench-ab.sh sets it").into())
}

impl Setup {
    fn from_env() -> Fallible<Self> {
        Ok(Self {
            builds: [
                path_var("FABRIC_AB_ANCHOR")?,
                path_var("FABRIC_AB_CANDIDATE")?,
            ],
            out: path_var("FABRIC_AB_OUT")?,
            pairs: count_var("FABRIC_AB_PAIRS", DEFAULT_PAIRS)?,
            counted: count_var("FABRIC_AB_COUNTED", DEFAULT_COUNTED)?,
            counters: std::env::var_os("FABRIC_AB_COUNTERS").map(PathBuf::from),
        })
    }
}

/// The two sides in the order they go in run `run`: the anchor first in
/// even runs, the candidate first in odd ones.
fn order(run: usize) -> [usize; 2] {
    if run.is_multiple_of(2) {
        [0, 1]
    } else {
        [1, 0]
    }
}

/// Says how far the bench got, on standard error.
fn progress(line: &str) -> Fallible {
    use std::io::Write as _;
    writeln!(std::io::stderr(), "ab: {line}")?;
    Ok(())
}

/// Runs `op` on `side` with its processes counting, and returns what they
/// did: the counters' changes, with each command started as
/// `cmd:<command>`, and the spawn log lines, with the fixture's directory
/// written as `<root>`.
fn counted_step(side: &mut Side, op: Op) -> Fallible<(BTreeMap<String, u64>, Vec<String>)> {
    let dir = side
        .counts
        .as_ref()
        .ok_or("no counter directory")?
        .path()
        .to_owned();
    let before = read_pages(&dir)?;
    let logged = log_lengths(&dir)?;
    side.step(op)?;
    std::thread::sleep(SETTLE);
    let after = read_pages(&dir)?;
    if after.is_empty() {
        return Err("no fabric process kept counters; does the build load the library?".into());
    }
    let mut counts = delta(&before, &after);
    let root = side.env.root.display().to_string();
    let mut lines = Vec::new();
    for line in new_spawns(&dir, &logged)? {
        *counts.entry(format!("cmd:{}", command(&line))).or_default() += 1;
        lines.push(line.replace(&root, "<root>"));
    }
    Ok((counts, lines))
}

/// The counted phase: one warm-up run, then `runs` counted runs. Every
/// spawn log line goes to `spawns`.
fn counted_phase(
    setup: &Setup,
    samples: &mut Samples,
    spawns: &mut String,
    runs: usize,
) -> Fallible {
    let Some(library) = setup.counters.as_deref() else {
        progress("no FABRIC_AB_COUNTERS, so no counted phase")?;
        return Ok(());
    };
    let mut sides = [
        Side::new(&setup.builds[0], Some(library))?,
        Side::new(&setup.builds[1], Some(library))?,
    ];
    for run in 0..=runs {
        let first = SIDES[order(run)[0]];
        progress(&format!("counted run {run} of {runs}, {first} first"))?;
        for side in &mut sides {
            side.runs += 1;
        }
        for op in Op::all() {
            for index in order(run) {
                let side = &mut sides[index];
                let (counts, lines) = counted_step(side, op)?;
                if run == 0 {
                    continue;
                }
                let name = op.name();
                let label = SIDES[index];
                for (metric, value) in counts {
                    if value > 0 {
                        samples.push("counted", run, &name, label, first, &metric, value);
                    }
                }
                for (node, kib) in side.daemon_rss()? {
                    let metric = format!("daemon_rss_kib.{node}");
                    samples.push("counted", run, &name, label, first, &metric, kib);
                }
                for line in lines {
                    let _ = writeln!(spawns, "{run}\t{name}\t{label}\t{line}");
                }
            }
        }
    }
    Ok(())
}

/// The timed phase: the warm-up runs, then `pairs` timed runs; none when
/// `pairs` is 0, for a run that only counts.
fn timed_phase(setup: &Setup, samples: &mut Samples) -> Fallible {
    if setup.pairs == 0 {
        return Ok(());
    }
    let mut sides = [
        Side::new(&setup.builds[0], None)?,
        Side::new(&setup.builds[1], None)?,
    ];
    for run in 0..TIMED_WARM_UPS + setup.pairs {
        let first = SIDES[order(run)[0]];
        progress(&format!(
            "timed run {run} of {}, {first} first",
            TIMED_WARM_UPS + setup.pairs - 1
        ))?;
        for side in &mut sides {
            side.runs += 1;
        }
        for op in Op::all() {
            for index in order(run) {
                let took = sides[index].step(op)?;
                if run >= TIMED_WARM_UPS {
                    let micros = u64::try_from(took.as_micros())?;
                    samples.push("timed", run, &op.name(), SIDES[index], first, "us", micros);
                }
            }
        }
        if run >= TIMED_WARM_UPS {
            for (index, side) in sides.iter().enumerate() {
                for (node, kib) in side.daemon_rss()? {
                    let op = format!("daemon {node}");
                    samples.push("timed", run, &op, SIDES[index], first, "rss_kib", kib);
                }
            }
        }
    }
    Ok(())
}

/// Runs both builds through the counted and the timed phases and writes
/// the samples and the report.
#[test]
#[ignore = "benchmark of two builds; scripts/bench-ab.sh runs it"]
fn paired() -> Fallible {
    let setup = Setup::from_env()?;
    fs::create_dir_all(&setup.out)?;
    let mut samples = Samples::new();
    let mut spawns = "run\top\tside\tcommand line\n".to_owned();
    counted_phase(&setup, &mut samples, &mut spawns, setup.counted)?;
    fs::write(setup.out.join("samples.tsv"), &samples.text)?;
    fs::write(setup.out.join("spawns.tsv"), &spawns)?;
    timed_phase(&setup, &mut samples)?;
    fs::write(setup.out.join("samples.tsv"), &samples.text)?;
    let report = report(&samples.text)?;
    fs::write(setup.out.join("report.md"), &report)?;
    println!("{report}");
    Ok(())
}

/// Writes `report.md` again from the `samples.tsv` in `FABRIC_AB_OUT`.
#[test]
#[ignore = "reads the samples of an A/B run; scripts/bench-ab.sh runs it"]
fn analyze() -> Fallible {
    let out = path_var("FABRIC_AB_OUT")?;
    let report = report(&fs::read_to_string(out.join("samples.tsv"))?)?;
    fs::write(out.join("report.md"), &report)?;
    println!("{report}");
    Ok(())
}

/// One line of `samples.tsv`.
struct Sample {
    phase: String,
    run: usize,
    op: String,
    side: usize,
    metric: String,
    value: f64,
}

fn parse_samples(text: &str) -> Fallible<Vec<Sample>> {
    let mut samples = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        let [phase, run, op, side, _first, metric, value] = fields[..] else {
            return Err(format!("a sample line without 7 fields: {line:?}").into());
        };
        samples.push(Sample {
            phase: phase.to_owned(),
            run: run.parse()?,
            op: op.to_owned(),
            side: SIDES
                .iter()
                .position(|name| *name == side)
                .ok_or_else(|| format!("no side {side:?}"))?,
            metric: metric.to_owned(),
            value: value.parse()?,
        });
    }
    Ok(samples)
}

/// The median of `values`, which it sorts; 0 for none.
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => values[n / 2],
        _ => f64::midpoint(values[n / 2 - 1], values[n / 2]),
    }
}

/// A small, seeded generator, so a report made twice from the same samples
/// is the same report.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which is not 0.
    fn below(&mut self, n: usize) -> Fallible<usize> {
        Ok(usize::try_from(self.next() % u64::try_from(n)?)?)
    }
}

/// The 95% percentile bootstrap interval of the median of `values`.
fn bootstrap(values: &[f64], resamples: usize) -> Fallible<(f64, f64)> {
    if values.is_empty() {
        return Ok((0.0, 0.0));
    }
    let mut rng = SplitMix(0x00ab_00ab);
    let mut medians = Vec::with_capacity(resamples);
    let mut drawn = vec![0.0; values.len()];
    for _ in 0..resamples {
        for slot in &mut drawn {
            *slot = values[rng.below(values.len())?];
        }
        medians.push(median(&mut drawn));
    }
    medians.sort_by(f64::total_cmp);
    let low = resamples * 25 / 1000;
    let high = (resamples * 975 / 1000).saturating_sub(1);
    Ok((medians[low], medians[high]))
}

/// One op's pairs, judged; the medians are in the samples' unit.
struct Judged {
    pairs: usize,
    anchor: f64,
    candidate: f64,
    ratio: f64,
    low: f64,
    high: f64,
}

impl Judged {
    fn verdict(&self) -> &'static str {
        if self.low > 1.0 {
            "slower"
        } else if self.high < 1.0 {
            "faster"
        } else if self.ratio > 1.0 {
            "not shown, median above 1"
        } else {
            "not shown"
        }
    }
}

/// The pairs of `metric` in `phase` for `op`: the anchor's and the
/// candidate's value in each run that has both.
fn pairs(samples: &[Sample], phase: &str, op: &str, metric: &str) -> Vec<(f64, f64)> {
    let mut by_run: BTreeMap<usize, [Option<f64>; 2]> = BTreeMap::new();
    for sample in samples {
        if sample.phase == phase && sample.op == op && sample.metric == metric {
            by_run.entry(sample.run).or_default()[sample.side] = Some(sample.value);
        }
    }
    by_run
        .values()
        .filter_map(|pair| Some((pair[0]?, pair[1]?)))
        .collect()
}

fn judge(pairs: &[(f64, f64)]) -> Fallible<Judged> {
    let mut anchor: Vec<f64> = pairs.iter().map(|pair| pair.0).collect();
    let mut candidate: Vec<f64> = pairs.iter().map(|pair| pair.1).collect();
    let mut ratios: Vec<f64> = pairs
        .iter()
        .filter(|pair| pair.0 > 0.0)
        .map(|pair| pair.1 / pair.0)
        .collect();
    let (low, high) = bootstrap(&ratios, RESAMPLES)?;
    Ok(Judged {
        pairs: pairs.len(),
        anchor: median(&mut anchor),
        candidate: median(&mut candidate),
        ratio: median(&mut ratios),
        low,
        high,
    })
}

/// The ops of the samples in the order of a run, then any other.
fn ops_in(samples: &[Sample], phase: &str) -> Vec<String> {
    let seen: BTreeSet<&str> = samples
        .iter()
        .filter(|sample| sample.phase == phase)
        .map(|sample| sample.op.as_str())
        .collect();
    let mut ops: Vec<String> = Op::all()
        .into_iter()
        .map(Op::name)
        .filter(|op| seen.contains(op.as_str()))
        .collect();
    for op in seen {
        if !ops.iter().any(|known| known == op) {
            ops.push(op.to_owned());
        }
    }
    ops
}

/// The median of `metric` per side over the counted runs of `op`.
fn counted_medians(samples: &[Sample], op: &str, metric: &str) -> [f64; 2] {
    let mut values: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut runs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for sample in samples {
        if sample.phase == "counted" && sample.op == op {
            runs.insert((sample.side, sample.run));
            if sample.metric == metric {
                values[sample.side].push(sample.value);
            }
        }
    }
    // A run that recorded nothing of the metric counted 0 of it.
    for (side, values) in values.iter_mut().enumerate() {
        let recorded = runs.iter().filter(|(s, _)| *s == side).count();
        values.resize(recorded.max(values.len()), 0.0);
    }
    let [anchor, candidate] = &mut values;
    [median(anchor), median(candidate)]
}

/// The report: the timed table, the counted table, every counter whose
/// median differs between the builds, and the daemons' resident sizes.
fn report(text: &str) -> Fallible<String> {
    let samples = parse_samples(text)?;
    let mut out = String::from(
        "### Timed pairs\n\n\
         Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, \
         with its 95% bootstrap interval. Slower or faster only when the interval \
         excludes 1.\n\n\
         | op | pairs | anchor | candidate | ratio | 95% interval | verdict |\n\
         |---|---:|---:|---:|---:|---|---|\n",
    );
    for op in ops_in(&samples, "timed") {
        let found = pairs(&samples, "timed", &op, "us");
        if found.is_empty() {
            continue;
        }
        let judged = judge(&found)?;
        let _ = writeln!(
            out,
            "| {op} | {} | {:.1} | {:.1} | {:.3} | {:.3} to {:.3} | {} |",
            judged.pairs,
            judged.anchor / 1000.0,
            judged.candidate / 1000.0,
            judged.ratio,
            judged.low,
            judged.high,
            judged.verdict()
        );
    }
    counted_report(&samples, &mut out);
    rss_report(&samples, &mut out)?;
    Ok(out)
}

/// The counted table and the list of counters that differ.
fn counted_report(samples: &[Sample], out: &mut String) {
    let ops = ops_in(samples, "counted");
    if ops.is_empty() {
        return;
    }
    let columns: [(&str, &[&str]); 8] = [
        ("spawns", &["spawns"]),
        ("full flushes", &["fullfsync"]),
        ("fsyncs", &["fsync"]),
        (
            "journal+ledger bytes written",
            &["write_bytes.journal", "write_bytes.ledger"],
        ),
        (
            "state bytes read",
            &[
                "read_bytes.journal",
                "read_bytes.ledger",
                "read_bytes.jobs",
                "read_bytes.store",
                "read_bytes.state",
            ],
        ),
        ("allocations", &["mallocs"]),
        ("CLI peak RSS KiB", &["cli_maxrss_kib"]),
        ("CLI CPU ms", &["cli_cpu_us"]),
    ];
    out.push_str(
        "\n### Counted ops\n\n\
         Medians over the counted runs, anchor -> candidate, of all `fabric` processes \
         of the side while the op ran.\n\n| op |",
    );
    for (title, _) in &columns {
        let _ = write!(out, " {title} |");
    }
    out.push_str("\n|---|");
    out.push_str(&"---:|".repeat(columns.len()));
    out.push('\n');
    for op in &ops {
        let _ = write!(out, "| {op} |");
        for (title, metrics) in &columns {
            let mut sum = [0.0, 0.0];
            for metric in *metrics {
                let [anchor, candidate] = counted_medians(samples, op, metric);
                sum[0] += anchor;
                sum[1] += candidate;
            }
            if *title == "CLI CPU ms" {
                let _ = write!(out, " {:.1} -> {:.1} |", sum[0] / 1000.0, sum[1] / 1000.0);
            } else {
                let _ = write!(out, " {:.0} -> {:.0} |", sum[0], sum[1]);
            }
        }
        out.push('\n');
    }
    out.push_str(
        "\n### Counters that differ\n\n\
         Every counter whose median differs between the builds, per op, anchor -> candidate. \
         `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits \
         reads, writes and flushes (`syncs`) by the file they touch.\n\n",
    );
    for op in &ops {
        let metrics: BTreeSet<&str> = samples
            .iter()
            .filter(|sample| sample.phase == "counted" && sample.op == *op)
            .map(|sample| sample.metric.as_str())
            .filter(|metric| !metric.starts_with("daemon_rss") && *metric != "cpu_us")
            .collect();
        let mut differs = Vec::new();
        for metric in metrics {
            let [anchor, candidate] = counted_medians(samples, op, metric);
            if (anchor - candidate).abs() > f64::EPSILON {
                differs.push(format!("{metric} {anchor:.0} -> {candidate:.0}"));
            }
        }
        if !differs.is_empty() {
            let _ = writeln!(out, "- {op}: {}", differs.join(", "));
        }
    }
}

/// Each daemon's resident size after the counted ops and after the timed
/// runs.
fn rss_report(samples: &[Sample], out: &mut String) -> Fallible {
    counted_rss_report(samples, out)?;
    let daemons: BTreeSet<&str> = samples
        .iter()
        .filter(|sample| sample.phase == "timed" && sample.metric == "rss_kib")
        .map(|sample| sample.op.as_str())
        .collect();
    if daemons.is_empty() {
        return Ok(());
    }
    out.push_str(
        "\n### Daemon resident size\n\n\
         KiB after each timed run: median and largest per build, and the median of the \
         pairs' ratios with its 95% bootstrap interval.\n\n\
         | daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |\n\
         |---|---:|---:|---:|---:|---:|---|\n",
    );
    for daemon in daemons {
        let found = pairs(samples, "timed", daemon, "rss_kib");
        let judged = judge(&found)?;
        let max = |side: usize| {
            found
                .iter()
                .map(|pair| if side == 0 { pair.0 } else { pair.1 })
                .fold(0.0, f64::max)
        };
        let _ = writeln!(
            out,
            "| {daemon} | {:.0} | {:.0} | {:.0} | {:.0} | {:.3} | {:.3} to {:.3} |",
            judged.anchor,
            max(0),
            judged.candidate,
            max(1),
            judged.ratio,
            judged.low,
            judged.high
        );
    }
    Ok(())
}

/// Each daemon's resident size after every counted op, paired by run and
/// op. Both builds load the counter library then, so the sizes hold its
/// page and calls alike; they do not depend on load, as the times do.
fn counted_rss_report(samples: &[Sample], out: &mut String) -> Fallible {
    /// A daemon's size after one op of one run, per build.
    type ByOp<'a> = BTreeMap<(usize, &'a str), [Option<f64>; 2]>;
    let mut daemons: BTreeMap<&str, ByOp<'_>> = BTreeMap::new();
    for sample in samples {
        if sample.phase == "counted"
            && let Some(daemon) = sample.metric.strip_prefix("daemon_rss_kib.")
        {
            daemons
                .entry(daemon)
                .or_default()
                .entry((sample.run, sample.op.as_str()))
                .or_default()[sample.side] = Some(sample.value);
        }
    }
    if daemons.is_empty() {
        return Ok(());
    }
    out.push_str(
        "\n### Daemon resident size, counted runs\n\n\
         KiB after each counted op, both builds with the counter library loaded: the \
         number of ops, median and largest per build, and the median of the ops' ratios \
         with its 95% bootstrap interval.\n\n\
         | daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |\n\
         |---|---:|---:|---:|---:|---:|---:|---|\n",
    );
    for (daemon, by_op) in daemons {
        let found: Vec<(f64, f64)> = by_op
            .values()
            .filter_map(|pair| Some((pair[0]?, pair[1]?)))
            .collect();
        let judged = judge(&found)?;
        let max = |side: usize| {
            found
                .iter()
                .map(|pair| if side == 0 { pair.0 } else { pair.1 })
                .fold(0.0, f64::max)
        };
        let _ = writeln!(
            out,
            "| {daemon} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.3} | {:.3} to {:.3} |",
            judged.pairs,
            judged.anchor,
            max(0),
            judged.candidate,
            max(1),
            judged.ratio,
            judged.low,
            judged.high
        );
    }
    Ok(())
}

/// The median and its interval follow the samples, and the same samples
/// give the same interval every time.
#[test]
fn the_bootstrap_interval_holds_the_median_and_repeats() -> Fallible {
    let ratios: Vec<f64> = (0..20).map(|i| 1.0 + f64::from(i) / 100.0).collect();
    let mut sorted = ratios.clone();
    assert!((median(&mut sorted) - 1.095).abs() < 1e-9);
    let (low, high) = bootstrap(&ratios, 2000)?;
    assert!(low < 1.095 && 1.095 < high, "{low} {high}");
    assert!(low >= 1.0 && high <= 1.19, "{low} {high}");
    let (again_low, again_high) = bootstrap(&ratios, 2000)?;
    assert_eq!(
        (again_low.to_bits(), again_high.to_bits()),
        (low.to_bits(), high.to_bits())
    );
    let judged = judge(&[(1000.0, 1200.0); 15])?;
    assert_eq!(judged.verdict(), "slower");
    let judged = judge(&[(1000.0, 800.0); 15])?;
    assert_eq!(judged.verdict(), "faster");
    Ok(())
}

/// A page laid out as `counters.c` lays it out reads back, and the change
/// between two reads counts a new CLI process and its programs.
#[test]
fn a_counter_page_reads_back() -> Fallible {
    let mut bytes = vec![0_u8; 4096];
    bytes[..8].copy_from_slice(b"FABAB\x01\0\0");
    let set = |bytes: &mut Vec<u8>, word: usize, value: u64| {
        bytes[word * 8..word * 8 + 8].copy_from_slice(&value.to_le_bytes());
    };
    set(&mut bytes, 3, 8 << 20);
    set(&mut bytes, 8, 2);
    // write_bytes.ledger: the second class of the first group.
    set(&mut bytes, WORDS.len() + 1, 300);
    bytes[ROLE.0..ROLE.0 + 11].copy_from_slice(b"job approve");
    set(&mut bytes, PROGRAMS.0 / 8, 3);
    bytes[PROGRAMS.0 + 8..PROGRAMS.0 + 11].copy_from_slice(b"git");
    let page = Page::parse(&bytes)?.ok_or("no page")?;
    assert_eq!(page.role, "job approve");
    assert!(!page.is_daemon());
    assert_eq!(page.counts["fullfsync"], 2);
    assert_eq!(page.counts["write_bytes.ledger"], 300);
    assert_eq!(page.programs["git"], 3);
    let after = BTreeMap::from([("1.1".to_owned(), page)]);
    let changed = delta(&BTreeMap::new(), &after);
    assert_eq!(changed["fullfsync"], 2);
    assert_eq!(changed["spawn:git"], 3);
    assert_eq!(changed["cli_processes"], 1);
    assert_eq!(changed["cli_maxrss_kib"], 8192);
    assert_eq!(Page::parse(&[0_u8; 4096])?, None);
    Ok(())
}

/// A run that only counts still reports each daemon's resident size,
/// from the counted runs: memory is held to the anchor as time is, and a
/// counted run is the one an owner may take on a busy machine.
#[test]
fn a_counted_run_reports_the_daemons_resident_size() -> Fallible {
    let mut text = String::from("phase\trun\top\tside\tfirst\tmetric\tvalue\n");
    for run in 1..=3 {
        for op in ["acp suspend", "cli ls"] {
            for (side, kib) in [("anchor", 20_000), ("candidate", 19_000)] {
                let _ = writeln!(
                    text,
                    "counted\t{run}\t{op}\t{side}\tanchor\tdaemon_rss_kib.a\t{}",
                    kib + run * 10
                );
            }
        }
    }
    let report = report(&text)?;
    assert!(
        report.contains("### Daemon resident size, counted runs"),
        "{report}"
    );
    assert!(
        report.contains("| a | 6 | 20020 | 20030 | 19020 | 19030 | 0.950 |"),
        "{report}"
    );
    Ok(())
}

/// A spawn log line reads as the command the report counts.
#[test]
fn a_spawn_line_reads_as_its_command() {
    assert_eq!(
        command("git -c core.fsync=all -C /w symbolic-ref refs/remotes/origin/HEAD refs/x"),
        "git symbolic-ref"
    );
    assert_eq!(command("git --version"), "git");
    assert_eq!(command("ps -o lstart= -p 42"), "ps -o lstart=");
    assert_eq!(
        command("sh -c unset -f kill && exec \"$@\" fabric-hold /t/bin/claude -p"),
        "sh holding claude"
    );
    assert_eq!(command("kill -KILL -- -42"), "kill");
}
