# Contributing

## Pull request flow

1. Branch from `main`, or from the branch your work depends on (stacked PR), in
   your own worktree. Nobody commits to `main` directly.
2. Commit one logical change at a time.
3. Run the gates below, push, and open a draft PR. Fill in every section of the template.
4. Mark the PR ready when CI is green and review findings are resolved.
5. Efe merges.

## Commits

Use Conventional Commits in the imperative mood: `feat(store): add chunk index`.
Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`.
No AI attribution lines and no generated-by footers.

## Review gates

Run these before every push. CI runs the same steps on macOS 15 (arm64) and Ubuntu
with `--locked`, so commit every `Cargo.lock` change.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
scripts/check-hygiene.sh origin/main
scripts/check-budget.sh origin/main
```

The TypeScript SDK in `sdk/typescript` needs Bun, and Node 22 with npm on
`PATH` for its package test. Its end-to-end test runs against a node built
from your tree:

```sh
cargo build -p fabric --locked
cd sdk/typescript
bun install --frozen-lockfile
bun run typecheck
bun run build
FABRIC_BIN="$PWD/../../target/debug/fabric" bun test
```

`scripts/check-hygiene.sh` fails on an em dash or an AI attribution line in any
tracked file or in any commit message on the branch, from any directory in the
repository. `scripts/test-check-hygiene.sh` proves it fires; CI runs it first.

`scripts/check-budget.sh` fails when the time budget changed without a new
`[[change]]` entry, or when it loosens a limit (see Time budget);
`scripts/test-check-budget.sh` proves it fires.

`cargo deny check` needs `cargo install cargo-deny --locked`. CI also runs it
once a week, so a new advisory against an unchanged `Cargo.lock` still shows
up. `deny.toml` holds the license, advisory, and source policy. A new
dependency must pass it, and the PR lists the crate, version, last release
date, and license. Dependabot opens a weekly PR for a stale action pin or
crate; it goes through the same gates.

## Performance rule

No hot path may be slower, or use more memory, than the anchor: `main`
before the durable job campaign, commit `176622e`. A tolerance per pull
request compounds (1.5 times per PR is 5 times after four), so every
change is compared with the same fixed anchor, and the aim is to be faster
and smaller than it, not only within a budget. A slowdown the A/B bench
measures on a hot path is a bug to fix, never a trade-off to document, and
the machine's load is not an explanation: the bench gives both builds of a
pair the same load. A correctness check is never removed or weakened to
win time; find a cheaper way to do the same check.

`scripts/bench-ab.sh` is the evidence. It builds the anchor once into a
cache, builds this working tree (or `--candidate <commit>`), and runs the
ignored test `ab::paired` in `crates/fabric/tests/e2e/ab.rs`, which drives
both builds through the same fixture and ops, taking turns op by op:

```sh
scripts/bench-ab.sh docs/evidence/perf-ab/<name>            # test profile, 15 pairs
scripts/bench-ab.sh --release --pairs 20 docs/evidence/perf-ab/<name>
scripts/bench-ab.sh --pairs 0 docs/evidence/perf-ab/<name>  # counted phase only
scripts/bench-ab.sh --analyze docs/evidence/perf-ab/<name>  # the report again
```

Its `report.md` gives each op's median ratio, candidate over anchor, with
a 95% bootstrap interval from at least 15 pairs, and each daemon's
resident size and physical footprint. The resident size, from `ps`, also
counts the clean pages of the binary's code, which a larger build keeps
resident; the physical footprint, from macOS's `footprint(1)`, counts only
the pages the daemon dirtied. On macOS a counted phase also loads
`scripts/perf-ab/counters.c` into every `fabric` process and reports what
does not depend on load: processes started by command, file flushes by
kind and file, bytes read and written, allocations, and each CLI's peak
resident size and CPU time, and each daemon's resident size and
physical footprint after every op. With `--pairs 0` only that counted
phase runs: its numbers do not depend on load, so it is the run to make
on a busy machine, and a timed
run waits for a quiet one. A pull request that touches a hot path keeps
its run under `docs/evidence/perf-ab/`. An interval above 1 is a
regression; a median above 1 whose interval holds 1 is profiled before
merge.

## Time budget

The time budget is CI's coarse guard; the performance rule above is the bar.
The ignored test `budget::hot_paths` in `crates/fabric/tests/e2e/budget.rs`
times the hot paths on the end-to-end fixture of two nodes over QUIC:
suspend, move, resume and pull of a job under each harness (ACP, Claude,
Codex), and a daemon's cold start and restart. Every program the node's
facts probe, but git, is a fake that answers after 50 ms; git is the
machine's own. It also counts the processes each op starts, by program:
git, every probed fake and every harness, and the kill, lsof, pgrep and ps
the harness runs, which the end-to-end build's `counted-tools` feature
takes from the test's `bin` (`FABRIC_SYSTEM_TOOLS`), where the budget
counts them. Probes run in parallel, so a probe started once more hides
in the time; its count shows it.
`crates/fabric/tests/e2e/budget.toml` holds the limits: the ratio, the
grace, the runs per batch, each platform's baselines, the exact process
counts, and the `[[change]]` entries that record why the budget changed. CI
runs it after the end-to-end step on both runners with
`FABRIC_BUDGET=enforce` and writes its table to the step summary. The Linux
runner is held to the `linux-x86_64` baselines. The macOS runner is not the
Mac of the `macos-aarch64` baselines, and runs of one commit there differ by
up to 1.34 times, more than the ratio, so it sets
`FABRIC_BUDGET_PLATFORM=macos-aarch64-github`, a platform with no
baselines: its times only report, and its process counts are held as
everywhere.

Run it on a quiet machine, since it measures wall time. It takes a few
minutes:

```sh
cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
FABRIC_BUDGET=enforce cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
FABRIC_BUDGET=count cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
```

The first command only reports times; the second fails on them. A third,
with `FABRIC_BUDGET=count`, runs only the two warm-up runs and times
nothing: the process counts do not depend on load, so it is the one to run
on a busy machine, or to set the counts after a change. An op is
over its limit when its 25th percentile of 7 runs is above both 1.3 times
its baseline and its baseline plus 50 ms, and it fails when it is over
again in a second batch of 7. A process count fails in every mode when it
differs from the budget, higher or lower: fewer starts can mean a check was
dropped. A platform with no baselines only reports its times.
`macos-aarch64` holds the times of an 18-core Apple silicon Mac and
`linux-x86_64` those of CI's `ubuntu-latest` runner.

A baseline is the anchor's time, so it never goes up: a slower op is a bug
to fix (Performance rule), and a baseline raised once per pull request
would compound. `scripts/check-budget.sh` fails a change that raises or
removes a baseline, raises the ratio or the grace, or lowers the
percentile or the runs per batch. To change the budget otherwise, as for a
faster op or a new process count:

1. Say in the PR why, with the table before and after; for a new process
   count, with the A/B run that shows the op is not slower than the anchor.
2. Lower the op's baseline in `budget.toml`, or set its counts.
3. Add a `[[change]]` entry at the end of `budget.toml`, with
   `date = "YYYY-MM-DD"` and a `why` that names the PR. Entries are never
   edited or removed. `scripts/check-budget.sh` fails a change to the
   budget without a new one.

The one time a baseline may rise is when the anchor itself was timed
again, as when CI moves to another runner: build `176622e`, time it, set
the baselines from those runs, and say so in the new entry with
`anchor = "176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5"` beside its date and
why.

To give a platform its baselines, run the anchor's code on it at least
three times and take each op's median 25th percentile. For a CI runner,
`scripts/budget-baselines.sh` does that from the runs' logs:

```sh
gh run view <run-id> --log > run-<run-id>.log   # once per run of the anchor's code
scripts/budget-baselines.sh linux-x86_64 run-*.log
```

It prints the `[baseline.<platform>]` table; the `[[change]]` entry names
the runs.

## Workspace rules

Every crate sets `[lints] workspace = true`. The root `Cargo.toml` holds the
lint policy, and `clippy.toml` holds its test exceptions and disallowed methods.

- `unwrap`, `expect`, `panic!`, `println!` and `eprintln!` fail clippy outside
  tests. Inside a `#[cfg(test)]` module every function may use them. In
  `tests/` files only `#[test]` functions may; a helper there returns `Result`
  and writes nothing.
- `todo!`, `unimplemented!`, `unreachable!` and `dbg!` fail clippy everywhere.
- `std::process::exit` fails clippy everywhere, `main` included. Return
  `std::process::ExitCode` from `main` so destructors run.
- Every `#[allow]` and `#[expect]` carries a `reason = "..."`.
- `missing_docs` also covers integration test files. Start each one with a `//!`
  line that says what it proves.
- Crates set `publish.workspace = true`, which inherits `publish = false`. This
  keeps the code off crates.io and lets path dependencies omit a version.
