//! The M5 paths no reference build has: a submit through the facade, the
//! placement decision that follows it, a watch's delivery of a new event,
//! and the planned handoff a facade continue asks for. The anchor and the
//! M4 head have no facade, so these cannot be paired; this test drives the
//! candidate alone, on the fixture of `ab::paired` (two nodes over QUIC,
//! the fake probed programs), and reports each op's median with its 95%
//! bootstrap interval, then the counters of one counted run.
//!
//! What each op times:
//! - `facade submit`: `fabric facade submit` on b to the authority a,
//!   from start to exit.
//! - `placement decision`: from the submit's exit until an inspection from
//!   b shows the job's first placement, read every 10 ms.
//! - `watch delivery`: a watch from b waits on a job of a; the time from
//!   the start of `fabric managed note` on a until the watch exits with the
//!   new event.
//! - `facade continue`: `fabric facade continue` on b, from start to exit:
//!   the request of a planned handoff, accepted. The handoff itself is not
//!   timed: the job's attempt is granted and never launched, since no node
//!   launches a granted attempt yet (docs/managed-jobs.md, "As built (M5)",
//!   "Not built"), so the continue stays accepted, its cut never taken.

use super::*;

use serde_json::Value;

/// How long a placement or a delivery may take before the run fails.
const WITHIN: Duration = Duration::from_secs(20);

/// How often an inspection is read while a placement is awaited.
const POLL: Duration = Duration::from_millis(10);

/// How long a watch is given to start waiting before the note.
const WATCH_READY: Duration = Duration::from_millis(300);

/// The ops, in the order of a run.
const OPS: [&str; 4] = [
    "facade submit",
    "placement decision",
    "watch delivery",
    "facade continue",
];

/// The JSON a facade command printed.
fn answer(text: &str) -> Fallible<Value> {
    let start = text
        .find('{')
        .ok_or_else(|| format!("no JSON in:\n{text}"))?;
    serde_json::from_str(&text[start..]).map_err(|e| format!("{e} in:\n{text}").into())
}

/// The fixture: the authority a, the other node b, a bearer minted on a,
/// and a managed job of a that the watch reads.
struct Fixture {
    /// Held so the fixture's directory outlives its nodes.
    _env: Arc<Env>,
    a: Node,
    b: Node,
    token: String,
    watched: String,
    cursor: String,
    runs: usize,
    /// The eligible candidates of the last run's placement.
    eligible: u64,
    counts: Option<tempfile::TempDir>,
}

impl Fixture {
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
        pair(&b, &a)?;
        let minted = a.ok(&["facade", "mint", "--actor", "ab", "--ttl-ms", "86400000"])?;
        let token = minted
            .lines()
            .find_map(|line| line.strip_prefix("bearer "))
            .and_then(|rest| rest.split_whitespace().next())
            .ok_or_else(|| format!("no bearer in:\n{minted}"))?
            .to_owned();
        let created = a.ok(&[
            "managed",
            "create",
            "--empty",
            "--goal",
            "watched from b",
            "--harness",
            "codex",
            "--operation-key",
            "ab-watched",
        ])?;
        let watched = created
            .lines()
            .find(|line| line.parse::<fabric_core::id::JobId>().is_ok())
            .ok_or_else(|| format!("no job id in:\n{created}"))?
            .to_owned();
        let fixture = Self {
            _env: env,
            a,
            b,
            token,
            watched,
            cursor: String::new(),
            runs: 0,
            eligible: 0,
            counts,
        };
        let watched = fixture.watched.clone();
        fixture.placed(&watched, "initial")?;
        Ok(fixture)
    }

    /// The facade arguments every command from b takes.
    fn facade<'s>(&'s self, op: &'s str) -> Vec<&'s str> {
        vec![
            "facade",
            op,
            "--authority",
            &self.a.id,
            "--token",
            &self.token,
        ]
    }

    fn watch(&self, after: Option<&str>, wait: &str) -> Fallible<Value> {
        let mut args = self.facade("watch");
        args.extend([self.watched.as_str(), "--wait-ms", wait]);
        if let Some(after) = after {
            args.extend(["--after", after]);
        }
        answer(&self.b.ok(&args)?)
    }

    fn inspect(&self, job: &str) -> Fallible<Value> {
        let mut args = self.facade("inspect");
        args.push(job);
        let inspection = answer(&self.b.ok(&args)?)?;
        if inspection["type"] != "inspection" {
            return Err(format!("not an inspection: {inspection}").into());
        }
        Ok(inspection)
    }

    /// Waits until `job` holds a placement of `kind`; how long that took
    /// and how many of its candidates were eligible.
    fn placed(&self, job: &str, kind: &str) -> Fallible<(Duration, u64)> {
        let started = Instant::now();
        loop {
            let inspection = self.inspect(job)?;
            let found = inspection["placements"]
                .as_array()
                .and_then(|all| all.iter().find(|p| p["kind"] == kind));
            if let Some(placement) = found {
                let took = started.elapsed();
                let eligible = placement["candidates"].as_array().map_or(0, |all| {
                    all.iter()
                        .filter(|c| c["verdict"].get("eligible").is_some())
                        .count()
                });
                return Ok((took, u64::try_from(eligible)?));
            }
            if started.elapsed() > WITHIN {
                return Err(format!("no {kind} placement within {WITHIN:?}: {inspection}").into());
            }
            std::thread::sleep(POLL);
        }
    }

    /// One run: each op's time, in the order of `OPS`.
    fn run(&mut self) -> Fallible<[Duration; 4]> {
        self.runs += 1;
        let run = self.runs;
        let key = format!("ab-submit-{run}");
        let mut args = self.facade("submit");
        args.extend([
            "--empty",
            "--goal",
            "placed by the bench",
            "--harness",
            "codex",
            "--key",
            &key,
        ]);
        let started = Instant::now();
        let accepted = answer(&self.b.ok(&args)?)?;
        let submit = started.elapsed();
        if accepted["type"] != "accepted" {
            return Err(format!("the submit was not accepted: {accepted}").into());
        }
        let job = accepted["job"].as_str().ok_or("no job")?.to_owned();
        let (placement, eligible) = self.placed(&job, "initial")?;
        self.eligible = eligible;

        let delivery = self.delivery(run)?;

        let key = format!("ab-continue-{run}");
        let mut args = self.facade("continue");
        args.extend([job.as_str(), "--key", &key]);
        let started = Instant::now();
        let accepted = answer(&self.b.ok(&args)?)?;
        let cont = started.elapsed();
        if accepted["type"] != "accepted" {
            return Err(format!("the continue was not accepted: {accepted}").into());
        }
        Ok([submit, placement, delivery, cont])
    }

    /// A watch from b waits; a note on a; how long until the watch exits
    /// with it, from the note's start.
    fn delivery(&mut self, run: usize) -> Fallible<Duration> {
        // Every event so far read first, so the watch below waits for the note.
        let mut after = None;
        loop {
            let read = self.watch(after.as_deref(), "0")?;
            let next = read["next"].as_str().ok_or("no next")?.to_owned();
            let empty = read["events"].as_array().is_none_or(Vec::is_empty);
            after = Some(next);
            if empty {
                break;
            }
        }
        self.cursor = after.ok_or("no cursor")?;
        let mut args = self.facade("watch");
        args.extend([
            self.watched.as_str(),
            "--after",
            &self.cursor,
            "--wait-ms",
            "20000",
        ]);
        let waiting = self.b.spawn(&args)?;
        std::thread::sleep(WATCH_READY);
        let question = format!("still here, run {run}?");
        let key = format!("ab-note-{run}");
        let started = Instant::now();
        self.a.ok(&[
            "managed",
            "note",
            &self.watched,
            "--question",
            &question,
            "--operation-key",
            &key,
        ])?;
        let output = crate::world::finish(waiting, WITHIN)?;
        let took = started.elapsed();
        if !output.status.success() {
            return Err(format!("the watch failed: {}", crate::world::text(&output)).into());
        }
        let events = answer(&String::from_utf8(output.stdout)?)?;
        let delivered = events["events"]
            .as_array()
            .is_some_and(|all| all.iter().any(|e| e["event"]["kind"] == "note_recorded"));
        if !delivered {
            return Err(format!("the watch returned without the note: {events}").into());
        }
        events["next"]
            .as_str()
            .ok_or("no next")?
            .clone_into(&mut self.cursor);
        Ok(took)
    }

    fn daemon_sizes(&self) -> Fallible<Vec<DaemonSize>> {
        let mut sizes = Vec::new();
        for node in [&self.a, &self.b] {
            if let Some(pid) = node.daemon_pid() {
                sizes.push(DaemonSize {
                    node: node.name,
                    rss_kib: rss_kib(pid)?,
                    footprint_kib: footprint_kib(pid)?,
                });
            }
        }
        Ok(sizes)
    }
}

/// The counted run: one warm-up, then one run whose counters, read before
/// and after the whole run, are written per run, with the daemons' sizes.
fn counted(library: &Path, fabric: &Path, samples: &mut Samples) -> Fallible {
    let mut fixture = Fixture::new(fabric, Some(library))?;
    fixture.run()?;
    let dir = fixture
        .counts
        .as_ref()
        .ok_or("no counter directory")?
        .path()
        .to_owned();
    let before = read_pages(&dir)?;
    fixture.run()?;
    std::thread::sleep(SETTLE);
    let after = read_pages(&dir)?;
    if after.is_empty() {
        return Err("no fabric process kept counters".into());
    }
    for (metric, value) in delta(&before, &after) {
        if value > 0 {
            samples.push(
                "counted",
                1,
                "new paths run",
                "candidate",
                "candidate",
                &metric,
                value,
            );
        }
    }
    for size in fixture.daemon_sizes()? {
        let metric = format!("daemon_rss_kib.{}", size.node);
        samples.push(
            "counted",
            1,
            "new paths run",
            "candidate",
            "candidate",
            &metric,
            size.rss_kib,
        );
        if let Some(kib) = size.footprint_kib {
            let metric = format!("daemon_footprint_kib.{}", size.node);
            samples.push(
                "counted",
                1,
                "new paths run",
                "candidate",
                "candidate",
                &metric,
                kib,
            );
        }
    }
    Ok(())
}

/// The report: each op's median and interval, then the counters.
fn report(text: &str) -> Fallible<String> {
    let samples = parse_samples(text)?;
    let mut out = String::from(
        "### New paths, candidate alone\n\n\
         No reference build has these paths, so there is no ratio. Medians in ms \
         with the 95% bootstrap interval of the median.\n\n\
         | op | runs | median | 95% interval | min | max |\n\
         |---|---:|---:|---|---:|---:|\n",
    );
    for op in OPS {
        let mut values: Vec<f64> = samples
            .iter()
            .filter(|s| s.phase == "timed" && s.op == op && s.metric == "us")
            .map(|s| s.value / 1000.0)
            .collect();
        if values.is_empty() {
            continue;
        }
        let (low, high) = bootstrap(&values, RESAMPLES)?;
        let n = values.len();
        let mid = median(&mut values);
        let _ = writeln!(
            out,
            "| {op} | {n} | {mid:.1} | {low:.1} to {high:.1} | {:.1} | {:.1} |",
            values[0],
            values[n - 1]
        );
    }
    let eligible: Vec<String> = samples
        .iter()
        .filter(|s| s.phase == "timed" && s.op == "placement decision" && s.metric == "eligible")
        .map(|s| format!("{}", s.value))
        .collect();
    let _ = writeln!(
        out,
        "\nEligible candidates of each placement decision, of the two nodes, by run: {}",
        eligible.join(" ")
    );
    out.push_str("\nDaemon sizes in KiB, median over the runs:\n\n");
    for op in ["daemon a", "daemon b"] {
        for metric in ["rss_kib", "footprint_kib"] {
            let mut values: Vec<f64> = samples
                .iter()
                .filter(|s| s.phase == "timed" && s.op == op && s.metric == metric)
                .map(|s| s.value)
                .collect();
            if !values.is_empty() {
                let _ = writeln!(out, "- {op} {metric}: {:.0}", median(&mut values));
            }
        }
    }
    let counted: Vec<&Sample> = samples.iter().filter(|s| s.phase == "counted").collect();
    if !counted.is_empty() {
        out.push_str(
            "\n### Counted run\n\nEvery `fabric` process over one whole run of the four ops.\n\n\
             | counter | value |\n|---|---:|\n",
        );
        for sample in counted {
            let _ = writeln!(out, "| {} | {} |", sample.metric, sample.value);
        }
    }
    Ok(out)
}

/// Drives the candidate through the new paths and writes
/// `new-paths.tsv` and `new-paths.md` into `FABRIC_AB_OUT`.
#[test]
#[ignore = "benchmark of the candidate's M5 paths; scripts/bench-ab.sh runs it"]
fn run() -> Fallible {
    let fabric = path_var("FABRIC_AB_CANDIDATE")?;
    let out = path_var("FABRIC_AB_OUT")?;
    let runs = count_var("FABRIC_AB_PAIRS", DEFAULT_PAIRS)?;
    fs::create_dir_all(&out)?;
    let mut samples = Samples::new();
    if let Some(library) = std::env::var_os("FABRIC_AB_COUNTERS") {
        progress("new paths: counted run")?;
        counted(Path::new(&library), &fabric, &mut samples)?;
    }
    let mut fixture = Fixture::new(&fabric, None)?;
    for run in 0..TIMED_WARM_UPS + runs {
        progress(&format!(
            "new paths: timed run {run} of {}",
            TIMED_WARM_UPS + runs - 1
        ))?;
        let took = fixture.run()?;
        if run < TIMED_WARM_UPS {
            continue;
        }
        for (op, took) in OPS.iter().zip(took) {
            let micros = u64::try_from(took.as_micros())?;
            samples.push("timed", run, op, "candidate", "candidate", "us", micros);
        }
        samples.push(
            "timed",
            run,
            "placement decision",
            "candidate",
            "candidate",
            "eligible",
            fixture.eligible,
        );
        for size in fixture.daemon_sizes()? {
            let op = format!("daemon {}", size.node);
            samples.push(
                "timed",
                run,
                &op,
                "candidate",
                "candidate",
                "rss_kib",
                size.rss_kib,
            );
            if let Some(kib) = size.footprint_kib {
                samples.push(
                    "timed",
                    run,
                    &op,
                    "candidate",
                    "candidate",
                    "footprint_kib",
                    kib,
                );
            }
        }
    }
    fs::write(out.join("new-paths.tsv"), &samples.text)?;
    let report = report(&samples.text)?;
    fs::write(out.join("new-paths.md"), &report)?;
    println!("{report}");
    Ok(())
}
