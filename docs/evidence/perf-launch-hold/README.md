# Launch hold: register while the trampoline starts

Items 1 and 4 of the A/B lab (`docs/evidence/perf-ab/README.md`), on the
branch `pd/wip-launch-hold`, with M2-21 and M2-23 of the M2 register in
the same branch. The rule is the one of 27 Oct 2026 (CONTRIBUTING.md,
Performance rule): no hot path slower or larger than the anchor, `main`
at `176622e`.

## What changed

- `376148b` (item 1). `process::spawn` returned only once the trampoline
  had stopped, polling `waitid` every 2 ms, so a headless launch waited
  for `/bin/sh` to start and stop (7.4 ms on this Mac) before the CLI
  could send `harness_started`, and then for the node's checks and
  durable record. `spawn` now returns as soon as the trampoline has a
  pid; the CLI registers it while the shell starts, and `Running::release`
  may come first. A headless or ACP harness is held by a line on a pipe
  the CLI holds (`read -r go && exec "$@"`), which the release writes; a
  terminal harness keeps the stop, with a watcher thread blocked in
  `waitid` and a 10 ms settle before any `SIGCONT` (below).
- `84828df` (item 4). The trampoline runs in `/bin/dash` where the system
  has it, else `/bin/sh`.
- `cba3776` (M2-23) and `f2857db` (M2-21) are fixes off the hot path:
  ACP updates bounded by bytes, and the Codex sandbox's writable roots
  and network access carried on resume.

Nothing the node checks or records changed: the harness still runs only
after `harness_recorded`, whose record is durable.

## A kernel race the early release exposed

XNU's `stop()` (`bsd/kern/kern_sig.c`) wakes a parent waiting for the
stop before it suspends the stopping task. A `SIGCONT` sent in between
resumes a task that is not suspended yet, which does nothing, and the
suspension then holds: the shell sits in state `S` inside `kill` for
good, until a second `SIGCONT`. The first build of item 1 sent the
`SIGCONT` the moment `waitid` reported the stop, and the ACP roundtrip
suite hung on its 45 s handshake deadline in its first full run, and
again in a stress of 40 runs. Measured on the stop hold with immediate
releases, many launches at once:

| build | early releases | hung |
|---|---:|---:|
| `SIGCONT` at once, 16 tasks of 60 | 960 | 16, each freed by a second `SIGCONT` |
| `SIGCONT` at once, the committed test (8 tasks of 25) | 3 runs of 200 | 2 runs failed |
| 10 ms settle (committed), the same test | 5 runs of 200 | 0 |

The M2 head is exposed to it as well, though rarely: its poll can see the
stop inside the same window, and only the registration that follows
(about 10 ms) kept its `SIGCONT` away. The micro-bench below, which sends
the `SIGCONT` right after the poll sees the stop, hung 2 times in 210
launches of the M2 head's hold. Hence the line hold for every harness
that does not keep the terminal: no signal reaches its shell at all. A
CLI that dies before the release closes the pipe, and `read` fails at
its end, so the shell exits without its exec; that no longer relies on
the kernel's `SIGHUP` of an orphaned group, and leaves no stopped shell
behind even under a reaper in the CLI's session.

## Regression tests, failing before each fix

| item | test | before the fix |
|---|---|---|
| 1 | `process::tests::hold::a_release_before_the_trampoline_is_ready_runs_the_harness_once_it_is` | `spawn waited 2.02s for the stop` |
| 1 | `..::a_hold_that_ends_before_it_is_ready_never_runs_the_harness` | `spawn` failed instead of the release |
| 1 | `..::a_late_stop_kills_the_group` | `spawn` failed instead of the release |
| 1 | `..::the_driver_gate_opens_once_the_harness_may_run` | the gate opened before the stop |
| 1 | `..::early_releases_of_stopping_trampolines_never_hang` | hung with the settle at 0 (2 of 3 runs) |
| 4 | `..::the_trampoline_runs_in_dash_where_the_system_has_it` | `the trampoline ran in "/bin/sh"` |
| M2-23 | `acp::client::tests::updates_are_bounded_by_bytes_as_well_as_by_count` | `40 updates of 1048733 bytes waited unread` |
| M2-21 | `codex_roundtrip::a_resume_carries_the_session_s_writable_roots_and_network_access` | only `sandbox_mode="workspace-write"` was passed |

## Method

- `scripts/bench-ab.sh` as the lab runs it: one fixture per build, the
  builds taking turns op by op, the first alternating per run; each op's
  ratio is the median of the pairs' candidate/anchor ratios with a 95%
  percentile bootstrap interval (10,000 resamples). Counted runs load
  `scripts/perf-ab/counters.c` into every `fabric` process.
- `holdbench/holdbench.rs`, a micro-bench of the hold alone, ABAB with a
  95% bootstrap interval of the median ratio: the time from the spawn to
  the end of a harness that is `/usr/bin/true`.

All runs on 2026-10-27/28 on an Apple M5 Pro (18 cores, macOS 26.6.2
arm64, rustc 1.97.1), while four other workflows ran their own A/B
benches and test suites beside them: load average 16 to 33
(`*/machine.txt`). The pairs give both builds the same load, so the
ratios hold; the absolute times are two to three times a quiet machine's,
and the intervals wider.

## Results

### The hold alone (`holdbench/`)

From the spawn to the end of a harness that is `/usr/bin/true`, ABAB, 95%
bootstrap interval of the median ratio:

| path | pairs | M2 head | this branch | ratio [95%] |
|---|---:|---:|---:|---|
| headless, released at once: `/bin/sh` stop and 2 ms poll, against `/bin/dash` and a line | 200 | 9.34 ms | 4.97 ms | 0.508 [0.480-0.530] |
| terminal, released after a 30 ms registration: stop, poll, registration, `SIGCONT`, against registration beside the stop and its 10 ms settle | 100 | 42.57 ms | 36.89 ms | 0.861 [0.836-0.878] |

The headless row is what item 1 and item 4 take out of a launch's
critical path; the terminal row shows the settle hides behind the
registration a terminal launch waits for anyway. Two of the M2 head's
holds in the headless row hung in `kill` (the race above) and were freed
by hand; their pairs keep the time they waited, which the median ignores.

### The launch ops alone, 100 pairs (`ab-launch-focus/`)

The bench's driver limited to `new` and `launch` (`driver.patch`, a
scratch change to `ab.rs` that is not committed), so 100 pairs fit in a
few minutes; candidate `84828df`, test profile, one counted run:

| against | op | anchor ms | candidate ms | ratio [95%] |
|---|---|---:|---:|---|
| the anchor `176622e` | acp launch | 173.3 | 141.6 | 0.800 [0.738-0.846] faster |
| the anchor `176622e` | claude launch | 172.1 | 177.5 | 1.029 [0.994-1.077] |
| the anchor `176622e` | codex launch | 223.9 | 257.0 | 1.103 [1.052-1.155] slower |
| the M2 head `8d35276` | acp launch | 130.6 | 135.8 | 0.959 [0.903-1.038] |
| the M2 head `8d35276` | claude launch | 167.5 | 158.5 | 0.968 [0.919-1.009] |
| the M2 head `8d35276` | codex launch | 221.7 | 226.6 | 1.043 [0.969-1.081] |

Against the M2 head the 4 to 5 ms the hold no longer adds are 2 to 3% of
a launch at this load, inside the interval; no launch is slower. Against
the anchor the ACP launch is faster, Claude's interval holds 1, and
Codex's is above it (next section).

### Every op of the A/B bench

`ab-test/` is `84828df` against the anchor and `ab-vs-m2-head/` the same
candidate against the M2 head, `8d35276`, both in the test profile with
20 pairs; `ab-release/` is `14c2d0f`, the branch with all four items,
against the anchor in release.
The launch rows:

| run | op | pairs | anchor ms | candidate ms | ratio [95%] |
|---|---|---:|---:|---:|---|
| against the anchor | acp launch | 20 | 170.1 | 136.1 | 0.762 [0.729-0.822] faster |
| against the anchor | claude launch | 20 | 167.4 | 202.4 | 1.118 [1.048-1.251] slower |
| against the anchor | codex launch | 20 | 218.7 | 242.3 | 1.067 [0.926-1.235] |
| against the M2 head | acp launch | 20 | 165.2 | 167.8 | 0.979 [0.797-1.232] |
| against the M2 head | claude launch | 20 | 193.2 | 219.9 | 1.053 [1.004-1.095] |
| against the M2 head | codex launch | 20 | 287.2 | 254.6 | 0.885 [0.812-0.910] faster |
| release, against the anchor | acp launch | 20 | 158.5 | 128.3 | 0.752 [0.670-0.801] faster |
| release, against the anchor | claude launch | 20 | 170.1 | 181.3 | 1.108 [0.978-1.161] |
| release, against the anchor | codex launch | 20 | 218.7 | 242.0 | 1.092 [1.029-1.169] slower |

The single pairs of these runs spread from 0.6 to 2.1 under this load
(`samples.tsv`), so a 20-pair interval that only just excludes 1, as
Claude's against the M2 head, is within what 26 ops give by chance; the
100-pair runs above, where it is 0.968 [0.919-1.009], settle the launch
rows. The other ops' rows are in each `report.md`.

### Counters

Against the M2 head every launch, resume and pull starts the same
processes and flushes as often (`ab-vs-m2-head/report.md`, counted ops):
4 processes and 12, 15 and 17 full flushes for the ACP, Claude and Codex
launch, allocations within 15 of each other, CLI peak resident size
within 50 KiB, which is the noise between two runs of one build. The
trampoline is `dash holding <harness>` instead of `sh holding <harness>`
(`spawns.tsv`). A headless launch starts no thread and no timer for its
hold any more; a terminal launch starts one watcher thread with a 128 KiB
stack, which exits at the stop. Against the anchor, the launch's 4
processes and its flushes are the M2 head's (`ab-test/report.md`): the
ACP launch's 15 full flushes fall to 12, and the CLI's peak resident
size is 0.5 to 1 MiB larger in the test profile, the larger debug binary
of M2, as the lab found.

## What is still slower than the anchor, and whose it is

The headless Codex launch (1.09 to 1.10 in release and in 100 pairs),
and Claude's where its interval lies above 1 (1.118 in one 20-pair run):
the harness can start only once the node has said `harness_recorded`, and
the anchor started it at once, before any registration (issue #17 was
that). Between the trampoline's spawn and its release the critical path
now holds only the node's own registration, which items 1 and 4 do not
touch: `ps -o pgid=` (item 2,
`getpgid` instead), `ps -o lstart=`, and the harness record's atomic write
with two `F_FULLFSYNC` (item 3, one full flush). Those are in
`crates/fabricd` and belong to other groups of this campaign; the
counters above show the same processes and flushes as the anchor's, so
what is left is their latency, not more work. A new Codex thread pays
once more: the CLI tells the node the thread with `harness_session`, and
the bench's fake Codex waits for the node's session record before its
turn goes on, so the node's durable writes for that record are in the
launch as well. These are regressions to fix, not trade-offs.

The other ops whose interval lies above 1 in one run against the anchor
(Claude move and pull, cold start and restart in the test profile; `cli
ls` and ACP resume in release) run no code this branch changed beyond the
launch inside a resume or a pull, the same launch as above: a move, a cold
start and a restart run no harness, and `cli ls` lists jobs (lab item
20). In the comparison with the M2 head, on the same machine and load,
none of them is slower, and Claude pull (0.959) and Codex resume (0.943)
are faster.

## Gates

On the branch head, each passed (`gates/summary.txt`): `cargo fmt --all
--check`; `cargo clippy --workspace --all-targets --locked -- -D
warnings` and `cargo clippy -p fabric-harness` with no feature and with
each of `acp`, `claude` and `codex`; `cargo test -p fabric-harness
--locked` with the default features and with each feature alone (repeated
three times, and the ACP and Codex roundtrip suites 60 and 20 times);
`cargo test -p fabricd --locked` (215 passed), which builds on the
harness crate; `cargo test -p fabric --test e2e --locked` (59 passed, 5
ignored) and `budget::hot_paths` in report mode, whose process counts
hold; `cargo doc` with `-D warnings`; `cargo deny check`;
`scripts/check-hygiene.sh`, `check-budget.sh` and `check-fixtures.sh`
against `origin/main` and their script tests; and the TypeScript SDK
(install, typecheck, build, test against this tree's `fabric`). The rest
of `cargo test --workspace` belongs to crates this branch does not touch
and ran with the other groups, not here.

Once, among about 30 runs of the harness suite under load 20 to 30,
`process::tests::a_version_probe_finds_an_interpreter_on_the_configured_path`,
which this branch does not change, timed out after the probe's 15 s: the
first run of a script written a moment before, which macOS scans. It
passed in every other run.
