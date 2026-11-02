# Node runtime: counters, not timings

This directory holds what the node-runtime group measured for three items:

- perf A/B item 6: the managed authority's thread starts with the first
  managed command, or with the daemon only when a journal exists
  (`0392968`, `440d56e`);
- M2-14: the events that wait for a control client that stopped reading are
  bounded by bytes (`28227da`, `922522c`);
- M2-19: captures, restores and reads run on store handles of their own
  instead of one mutex for the whole store (`8b6f90a`, `15e36b8`).

## Why there are no timings here

These runs were taken on 27 and 28 Oct 2026 while other workflows ran A/B
benches, stress loops and a Linux build on the same machine, at load
averages between 11 and 40 on 18 cores (`machine.txt` records the load at
start and end). Under that load a paired ratio is not evidence, so every
timed sample, every report built from them and the store bench's timings
were deleted. The messages of `c344206` and `8b6f90a` quote store bench
times from those runs, and `052ca5a`, `440d56e` and `922522c` quote
resident sizes taken beside them: the times are void, and the sizes stand
only until the integrator measures again. What stays here is what does
not depend on load: process starts, flushes, bytes read and written,
allocations, resident sizes and thread counts. The timings are for the
integrator to take, alone, on a quiet machine.

## Files

| Path | Anchor | Candidate | What it holds |
|---|---|---|---|
| `release-15e36b8-vs-base/` | `8d35276`, the M2 base this group starts from | `15e36b8` | the counted phase of `scripts/bench-ab.sh --release` |
| `release-15e36b8/` | `176622e`, the campaign anchor | `15e36b8` | the same |
| `release-440d56e/` | `176622e` | `440d56e` | the same |
| `release-922522c/` | `176622e` | `922522c` | the same |
| `daemon-memory.sh` | | | the paired memory measure of an idle daemon, or one after a job new and two suspends (`ops`) |
| `daemon-idle-vs-*.tsv`, `daemon-ops-vs-*.tsv` | `176622e` or `8d35276` | the build of `922522c` | what `daemon-memory.sh` printed |

In each `release-*` directory, `counters.tsv` is every row of the counted
phase of `samples.tsv` (columns as `scripts/bench-ab.sh` writes them, the
timed rows taken out), `spawns.tsv` is every command line a `fabric`
process started in the counted runs, with the fixture's directory as
`<root>`, and `machine.txt` names the builds. A candidate built from a
working tree had uncommitted paths, which `machine.txt` counts but does not
name; this directory was one of them. The candidate of the `daemon-*` files is not written
in them; they were written after `922522c` and before any later commit.
`daemon-idle-vs-anchor.tsv` stopped after five pairs.

## What the counters show

Against the M2 base (`release-15e36b8-vs-base/`, medians of two counted
runs per op): no op starts a process more or flushes a file more; the
differences in bytes and allocations are within the run-to-run spread the
bundle chain makes (see `docs/evidence/perf-ab/README.md`).

An idle daemon against the M2 base (`daemon-idle-vs-base.tsv`, 20 pairs):
19 threads instead of 20, and 289 KiB instead of 298 KiB in its default
malloc zone. Against the anchor (`daemon-idle-vs-anchor.tsv`, 5 pairs): 19
threads each and 289 KiB each. After a job new and two suspends against the
M2 base (`daemon-ops-vs-base.tsv`, 16 pairs): 20 threads instead of 21 and
343 KiB instead of 352 KiB.

## What the integrator measures

Each with `scripts/bench-ab.sh --release --pairs 20`, anchor `176622e`, on a
quiet machine, and `daemon-memory.sh <anchor> <candidate> 20 idle` and
`... 20 ops`:

| Commit | Expected effect |
|---|---|
| `0392968`, `440d56e` | an idle daemon without a managed journal: one thread fewer than the M2 base and no more resident memory than the anchor; `cold start` and `restart` no slower than the anchor |
| `28227da`, `922522c` | the same op times as the anchor, since a client that reads gets every event as before; the release binary within 30 KB of the M2 base and a daemon's resident code no larger |
| `8b6f90a`, `15e36b8` | `suspend`, `resume`, `move` and `pull` no slower than the anchor with one job at a time, and a short store call no longer waits for a capture or a restore; `cargo test -p fabric-store --release --test store -- --ignored --exact shared_store_bench --nocapture` gives the makespan and the reader's longest wait of both ways on one store |
