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
scripts/check-docs.sh
```

`scripts/check-docs.sh` needs mdBook at the version CI pins; [Docs](#docs)
says how to install it.

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

Where it is measured changed on 2026-11-08: Efe decided that performance is
measured in GitHub Actions, as a paired A/B of the anchor and the candidate
on one runner, with an interval, not on a developer's Mac. That CI job is
roadmap PR 15 and is not on `main` yet. Until it lands, a pull request runs
no local timed benchmark; it says in its body that its timing waits for the
CI job. The rule below, and the tool it names, stay as they are.

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
the anchor is `scripts/bench-ab.sh --release`. The test profile is
tracked, not gated: its runs stay in the record, but an unoptimized binary
keeps more code pages resident than the anchor's, so its resident size is
not held to the anchor's. CI's time budget below is a separate, coarse
guard and still runs in the test profile.

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

## Writing a harness adapter

A harness adapter is the code that knows how to detect, capture, restore
and launch one coding agent (ADR 0006, ADR 0015). There are two ways to add
one, and the first is the one a day is enough for.

### An ACP agent: no Rust code

An agent that speaks the Agent Client Protocol and can continue a session
(`session/resume`, or `session/load` with `loadSession: true`) runs under
the generic `Acp` adapter. Tell the node where it keeps its sessions, in
`$FABRIC_HOME/harness.toml`, read once when `fabric serve` starts:

```toml
[acp.opencode]
state_dir = ".local/share/opencode/sessions"   # relative to the home
```

Then make a job with `--harness acp:<program>`. Capture takes only the
files under `state_dir` with a path component that is the session id, or
the id plus an extension. The directory may not be the home itself, lie
under `~/.fabric`, or be reached through a link. The adapter cannot see
whether a generic agent is writing its session, so the node's own handle
on the process it launched is the guard. A push or a pull of an ACP job
checks that the target has the agent on its `PATH` and a `state_dir` for
it before the job changes hands.

What to bring in the pull request: a run of suspend, move and resume of a
real session of that agent, with the commands and what they printed, under
`docs/evidence/`, and a row for it in the sessions table (the fidelity
declaration, below). If the agent needs a fake of its own for CI, the fake
ACP agent is the pattern (below).

### A native adapter: an implementation of `Harness`

An agent that does not speak ACP needs an adapter in `crates/fabric-harness`,
behind a cargo feature of its own, as `claude` and `codex` are. The trait is
`fabric_harness::Harness` in `crates/fabric-harness/src/harness.rs`:

- `kind`, and `detect`: find the binary and its version, `None` when it is
  not installed.
- `capture`: copy everything the session needs to continue into a state
  tree, redacted, with a manifest; refuse while the harness may be writing
  the session.
- `confirm_departure`: record that the capture has left this node.
- `restore`: place a state tree where the harness expects it; refuse a
  workspace or home other than the captured ones, any file outside the
  places the adapter itself captures, and a restore while the harness
  writes the same session; place it all or nothing.
- `launch`: start the harness in the workspace, without the markers of a
  Claude Code or Codex session this process runs under.

Around the trait, a native adapter touches more than its own module:

- a `HarnessKind` variant in `crates/fabric-core/src/harness.rs`, with its
  text form for `--harness`, and the harness kind of the client API, which
  grows only as ADR 0018, Compatibility, allows, with fixtures under
  `protocol/fixtures` for the messages that carry it;
- the dispatch in `crates/fabricd/src/harnesses.rs` (`adapter`,
  `ask_installed_ahead`, `adapter_with_installed`), and the facts `doctor`
  measures for it (its version and whether it is logged in);
- the denylist in `crates/fabric-harness/src/denylist.rs`, for its
  credentials and host state, which must never enter a state tree;
- `crates/fabric-harness/src/markers.rs`, for the environment variables
  that mark a process as running inside it;
- a `read_headless_line` for its headless output, which
  `crates/fabric/src/lib.rs` picks by harness, as it does for Claude and
  Codex.

This is several days of work, not one, and it needs an ADR (below) when
the agent's session store raises a question the existing adapters did not
answer. Read the Claude and Codex modules first: the lock and version
rules in ADR 0015, Running harnesses, are where adapters went wrong before.

### The fake harness pattern

No test runs a real agent. Each adapter is tested against a fake binary
that behaves as the real one does in the ways the adapter depends on:

- `crates/fabric-harness/src/fake_tool.rs` writes a fake executable and runs
  it once, so the operating system's first-run check is paid when
  the fake is made, not inside a timed probe.
- `crates/fabric-harness/tests/common/mod.rs` is the fake `claude`: it
  reports a version, stays alive on `hold` so the running-process check
  sees it, and writes its arguments, home, pid and environment to a log.
- `crates/fabric-harness/tests/fake_acp_agent/mod.rs` is a fake ACP agent
  whose capabilities the tests shape through variables (`FAKE_LOAD=0`,
  `FAKE_RESUME=1`, `FAKE_ASK_PERMISSION=1`, `FAKE_PROMPT_ERROR=1`,
  `FAKE_LINGER=1`).
- `crates/fabric/fixtures/fake_agent.rs` is built as the
  `fabric-fake-agent` binary, the harness of most end-to-end tests
  (`--harness acp:fabric-fake-agent`).

### The fixtures and the tests to add

- A round trip in `crates/fabric-harness/tests/<name>_roundtrip.rs`, as
  `claude_roundtrip.rs`, `codex_roundtrip.rs` and `acp_roundtrip.rs` do:
  capture a session made by the fake, restore it onto a fresh home with
  the same workspace path, launch the fake there, and check that it
  continues the same session. Add the refusals: a session that holds a
  secret, a file outside the adapter's places, a harness that still writes.
- A crash test in `crates/fabric-harness/tests/crash.rs`: `SIGKILL` at
  each named crash point of capture and restore (feature `crashpoints`),
  then the invariant: nothing half placed.
- End-to-end rows in `crates/fabric/tests/e2e/`: an `install_fake_<name>`
  in `world.rs` beside `install_fake_claude` and `install_fake_codex`, and
  a job under the new harness that is suspended on one node, moved, and
  resumed on the other, over both wires. The end-to-end test runs two
  daemons on one machine.
- A new op in the time budget brings its process counts (Time budget).

### The fidelity declaration

Every adapter says what of a session survives a move. Until roadmap PR 5
adds the fidelity table to the README, the declaration is a row in the
sessions table of `docs/start/what-moves.md`: what is captured, how the
session continues on the target, and what proves it on `main` (fakes end
to end, or a real run between two machines with its evidence). Say what is
lost, too, such as a tool call in flight or approvals. A row claims only
what a test or a recorded run shows.

## Docs

The docs site is built by mdBook from `docs/`, with `docs/SUMMARY.md` as
its table of contents. mdBook is the one tool the docs add: it is a Rust
tool, it renders the Markdown that GitHub already shows, and the site is
the same files. It is not a dependency of any crate. CI pins its version
in `.github/workflows/docs.yml` and publishes the site from `main` to
GitHub Pages.

`scripts/check-docs.sh` needs mdBook and `python3`. Install the mdBook
version CI pins (`MDBOOK_VERSION` in `.github/workflows/docs.yml`), or set
`MDBOOK` to a binary of it; the build script names the version when it
finds none:

```sh
cargo install mdbook --version 0.5.4 --locked
```

```sh
cargo build -p fabric --locked
scripts/docs/cli-reference.sh   # docs/reference/cli.md, from the binary's --help
python3 scripts/docs/gen.py     # ADR index, evidence index, the matrix copies
scripts/check-docs.sh           # what CI runs: the above with --check, links, the build
```

Some pages are generated and say so in their first line: the CLI
reference, the ADR index, the ADR list in `SUMMARY.md`, the evidence
index, and the "what moves" matrix in `README.md` and `docs/README.md`,
which is written once in `docs/start/what-moves.md`. Edit the source and
run the script; CI fails when a generated page is out of date. A relative
link must name a file the repository has; on the site, a link to anything
that is not a page goes to the file on GitHub. The product's names, which
are not final, live in `scripts/docs/names.env`.

Every claim in the docs must be true on `main`. Where something is proved
only on a branch, say which branch and that it is not on `main`.

## Decisions

Any choice that a future reader would ask "why" about gets an ADR in
`docs/adr/` (ADR 0009):

1. Take the next number. Numbers are never reused or renumbered.
2. Name the file `NNNN-short-title.md`. The first line is
   `# ADR NNNN: Title`, and a line under it is
   `Status: proposed, YYYY-MM-DD`; the ADR index reads both.
3. Write Context, Decision, Alternatives considered (with evidence paths),
   Consequences and Open questions.
4. Open it in the pull request that needs it. Efe decides its status. A
   merged ADR can stay `proposed` while parts of it are open, as 0019 to
   0022 do; when its code lands, the status line says so: `as built on
   main since YYYY-MM-DD (PR #N)`.
5. Until the first release, amend an ADR in place with a dated note under
   the status line that says what changed, why, and which PR or review
   found it. After the first release, supersede it with a new ADR.

Run `python3 scripts/docs/gen.py` to add it to the index and the site.

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
