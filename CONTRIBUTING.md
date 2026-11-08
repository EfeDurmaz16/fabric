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
scripts/check-fixtures.sh origin/main
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

The bridge tests against the real Capsule worker are ignored by default and
run when `CAPSULE_BRIDGE_WORKER` names a worker built from a Capsule checkout
(see docs/bridge-protocol.md, Against the Capsule worker); CI builds one from
the Capsule commit pinned in `.github/workflows/ci.yml`.

`scripts/check-hygiene.sh` fails on an em dash or an AI attribution line in any
tracked file or in any commit message on the branch, from any directory in the
repository. `scripts/test-check-hygiene.sh` proves it fires; CI runs it first.

`scripts/check-fixtures.sh` fails when a fixture under `protocol/fixtures` that
the base already had was edited, renamed or removed, or `protocol/required.txt`
changed for one, without a bump of `API_VERSION` (ADR 0018: a client API
version only grows); `scripts/test-check-fixtures.sh` proves it fires.

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

The rule gates the release build, the one a node runs (Efe, 2026-10-29):
the A/B run that must show no op slower and no daemon or CLI larger than
the anchor is `scripts/bench-ab.sh --release`, which CI runs. The test
profile is tracked, not gated: its runs stay in the record, but an
unoptimized binary keeps more code pages resident than the anchor's, so
its resident size is not held to the anchor's. CI's time budget below is a separate, coarse
guard and still runs in the test profile.

CI measures it, on every pull request and every push to `main`: the
`perf` job of `.github/workflows/perf.yml` runs on `macos-15`, builds the
anchor (cached by commit) and the candidate in release on the same runner,
and runs `scripts/bench-ab.sh --release --pairs 15 --counted 5`, then
`scripts/bench-ab.sh --gate`. The job fails when a check breaks the rule:

- an op whose time ratio, candidate over anchor, has a family interval
  above 1: the 95% bootstrap interval across all timed ops at once
  (Bonferroni). For independent, alike samples that is at most one false
  failure in 20 runs however many ops there are; a shared runner's
  samples are neither, which is why a failing run is confirmed (below);
- daemon a's resident size or physical footprint after the timed runs,
  or the CLI's peak resident size or physical footprint over the counted
  ops, whose ratio has a family interval above 1 across the memory rows.
  Daemon a restarts in every timed run, so its pairs are fresh processes;
  daemon b lives through the whole timed phase, so its readings are one
  process per side read again and again, and its sizes are reported, not
  gated;
- an op that starts more processes, or makes more full or barrier
  flushes, than the anchor, medians over the counted runs, each counter
  of each op a check of its own. These do not
  depend on the runner's load, so they hold where a shared runner's times
  are too noisy to: `new`, `launch`, `cold start` and the approvals, whose
  family intervals span about 0.6 to 1.9 on the runner.

A run that fails is confirmed: the job runs both builds again, with the
candidate's fixture started first this time, and
`scripts/bench-ab.sh --gate --confirm` fails a check only when it fails in
both runs. 6 of 15 single runs of the anchor against itself on the runner
failed a check; none of the four confirmed A/A jobs did. The runner does not count instructions (the
`proc_pid_rusage` count reads 0 in its virtual machine), so there is no
instruction gate; the report still shows them where the machine counts
them, as a local Mac does.

The verdict, the machine and the full report go to the job summary, every
sample to the run's artifact (`perf-ab-<sha>-<attempt>`), and the verdict
to one comment on the pull request, edited on each push (a pull request
from a fork gets a read-only token, so it has the summary only).
`docs/evidence/perf-ab/README.md` holds the runs that set this design. A
pull request does not need a local run any more, and keeps none in
`docs/evidence/`; the CI run is the record.

A local run is optional, to see a change before pushing it.
`scripts/bench-ab.sh` builds the anchor once into a cache, builds this
working tree (or `--candidate <commit>`), and runs the ignored test
`ab::paired` in `crates/fabric/tests/e2e/ab.rs`, which drives both builds
through the same fixture and ops, taking turns op by op:

```sh
scripts/bench-ab.sh --release <dir>             # what CI runs, 15 pairs
scripts/bench-ab.sh --release --pairs 0 <dir>   # counted phase only
scripts/bench-ab.sh --release --gate <dir>      # CI's verdict on that run
scripts/bench-ab.sh --analyze <dir>             # the report again
```

Its `report.md` gives each op's median ratio, candidate over anchor, with
a 95% bootstrap interval, and each daemon's resident size and physical
footprint. The resident size, from `ps`, also counts the clean pages of
the binary's code, which a larger build keeps resident; the physical
footprint, from macOS's `footprint(1)`, counts only the pages the daemon
dirtied. A counted phase loads `scripts/perf-ab/counters.c` into every
`fabric` process and reports what does not depend on load: processes
started by command, file flushes by kind and file, bytes read and written,
allocations, the instructions every process retired where the machine
counts them, each CLI's peak resident size, physical footprint and CPU
time, and each daemon's resident size and physical footprint after every
op. The counters are macOS only (`docs/evidence/perf-ab/README.md` says
why); elsewhere the counted phase is skipped. A median above 1 whose
interval holds 1 is profiled before merge.

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
would compound. A process count is the same rule measured without the
load, so it never goes up either. `scripts/check-budget.sh` fails a change
that raises or removes a baseline, raises the ratio or the grace, lowers
the percentile or the runs per batch, raises a process count of a program
the base counted, makes an op start a counted program it did not start,
removes an op's counts, or takes a program off the `counted` list, the
programs the warm-up runs count, which `budget.rs` checks against its own.
A program counted for the first time, and a new op, bring their counts. To
change the budget otherwise, as for a faster op or fewer processes:

1. Say in the PR why, with the table before and after.
2. Lower the op's baseline in `budget.toml`, or its counts.
3. Add a `[[change]]` entry at the end of `budget.toml`, with
   `date = "YYYY-MM-DD"` and a `why` that names the PR. Entries are never
   edited or removed. `scripts/check-budget.sh` fails a change to the
   budget without a new one.

The one time a baseline may rise is when the anchor itself was timed
again on CI, as when CI moves to another runner: run `176622e`'s code on
it at least three times, set the baselines from those runs as below, and
say so in the new entry with
`anchor = "176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5"` and the runs,
`runs = "<id> <id> <id>"`, beside its date and why. The check reads each
run with `gh`: it must be a finished run of `.github/workflows/ci.yml` on
a commit whose crates, Cargo files and toolchain are the anchor's, and
every raised baseline must be the one `scripts/budget-baselines.sh` gives
from the runs' logs. Without `gh`, or with runs it cannot read, the check
fails.

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
