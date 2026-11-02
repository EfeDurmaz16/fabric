# Performance of the facts, net and carry fixes

The performance rule of 27 Oct 2026 (CONTRIBUTING.md, Performance rule): no
hot path may be slower, or use more memory, than the anchor, `main` at
`176622e`, and the aim is to beat it. This directory holds the A/B runs of
the branch that fixes M2-13, M2-26, M2-15 and M2-20 on top of the M2 head
`8d35276`, and the micro A/B runs of the code each fix changed.

| Commit | What |
|---|---|
| `32fe4bd` | M2-13: a facts probe reads its output as it arrives and stops at `MAX_FACT_VALUE`; stderr, and an exit-code probe's stdout, go to `/dev/null` instead of a pipe |
| `af473fe` | M2-26: an answer with no room for every fact keeps the measured ones and names the rest in `facts.left_out`; the preflight names that cause |
| `86fedc8` | M2-20: a venv whose installer the target lacks travels only to its interpreter |
| `ceba4ce` | perf: a frame's payload is read after its version byte, into a buffer that never grows past the payload |
| `c20483f` | M2-15: a node holds at most `MAX_REQUEST_BYTES` of one peer's requests over all its sessions |
| `37cbe1b` | perf: one `/dev/null` per facts measure, lent by `dup`, and a 512 byte read buffer; made after the first run below |
| `e61f197` | perf: a probe starts its command in a plain function, so the command is never part of its task, and reads 256 bytes at a time; made after the footprint runs below |

## Runs

| Directory | Anchor | Candidate | Pairs | Counted runs | Daemon size |
|---|---|---|---:|---:|---|
| `anchor-vs-c20483f/` | `176622e` (main) | `c20483f` | 21 | 3 | `ps` resident |
| `base-vs-head/` | `8d35276` (M2 head, this branch's base) | `37cbe1b` | 21 | 3 | `ps` resident |
| `anchor-vs-head/` | `176622e` (main) | `37cbe1b` | 21 | 3 | `ps` resident |
| `base-vs-b0f25c0/` | `8d35276` | `b0f25c0` (same code as `37cbe1b`) | 21 | 3 | `ps` resident |
| `footprint/base-vs-b0f25c0/` | `8d35276` | `b0f25c0` | 21 | 3 | physical footprint |
| `footprint/base-vs-base/` | `8d35276` | `8d35276` (an A/A control) | 15 | 2 | physical footprint |
| `footprint/facts-only/` | `8d35276` | `8d35276` with the facts and carry commits, `e61f197` included | 15 | 2 | physical footprint |
| `footprint/net-only/` | `8d35276` | `8d35276` with `ceba4ce` and `c20483f` | 15 | 2 | physical footprint |
| `footprint/base-vs-e61f197/` | `8d35276` | `e61f197` | 21 | 3 | physical footprint |
| `anchor-vs-e61f197/` | `176622e` (main) | `e61f197` | 21 | 3 | `ps` resident |

All ran with `scripts/bench-ab.sh` in the test profile on an Apple M5 Pro
(18 cores, macOS arm64, rustc 1.97.1), while other workflows built and
tested beside them (`machine.txt`). The first three ran on 2026-10-27 at a
load average of 11 to 43; the others on 2026-10-28 at 4 to 18. Each op's
three harness shapes run the same facts, carry and network code, so the
shapes are also pooled (63 pairs; `footprint/pool.py <samples.tsv>`).

The runs under `footprint/` used the bench with one change, in
`footprint/ab-footprint.patch`: each daemon's size is its physical
footprint and its peak, as `footprint(1)` reports them, instead of its
resident size from `ps`. The footprint counts the pages a process dirtied,
compressed ones included, and leaves out clean pages of its binary, so the
machine's memory pressure and the layout of the code do not move it.

## What this branch changes against its base

Counters of every `fabric` process of the side during the op, median of
the counted runs, base `8d35276` to `e61f197`
(`footprint/base-vs-e61f197/report.md`; the earlier runs agree):

| op | opens | pipe and socket reads | allocations | bytes allocated | spawns, flushes |
|---|---|---|---|---|---|
| acp, claude, codex move | 73 to 33 | 124 to 76 | 8427 to 8195, 8279 to 8060, 8284 to 8051 | 2025532 to 1998879, 2014178 to 1987309, 2012867 to 1986304 | equal |
| acp, claude, codex pull | 416 to 371, 459 to 415, 510 to 464 | 501 to 445, 509 to 451, 517 to 459 | 28684 to 29361, 33867 to 33595, 29367 to 29083 | 8830116 to 9032952, 9151506 to 9118326, 9296777 to 9265189 | equal |
| acp, claude, codex suspend | 142 to 139, 145 to 142, 152 to 149 | 187 to 183, 185 to 181, 190 to 186 | 12872 to 12857, 16406 to 16402, 12593 to 12599 | 3531008 to 3528534, 3596639 to 3595884, 3662039 to 3661394 | equal |
| acp, claude, codex resume | equal, 249 to 247, equal | 175 to 170, 180 to 176, 183 to 180 | 11555 to 11546, 11480 to 11439, 11593 to 11579 | 4399320 to 4397666, 4412337 to 4406992, 4515915 to 4514034 | equal |
| acp, claude, codex new | 11 to 10 | 6 to 4 | 38519 to 38516, 1689 to 1683, 1687 to 1681 | 13431673 to 13430704, 393326 to 392465, 393125 to 391829 | equal |

The one count that rose, acp pull's allocations, moves as much between two
copies of one build: the A/A control (`footprint/base-vs-base/`) counts
acp pull 28432 to 27782 allocations and 8630286 to 8463060 bytes, and
acp launch, which no change here touches, 4738 to 4543. The other two
shapes of pull, which run the same code, allocate less.

### Time

Median of the pairs' ratios, candidate over base, with a 95% bootstrap
interval, pooled over the three shapes:

| op | base-vs-head (load 11 to 43) | base-vs-b0f25c0 | footprint/base-vs-e61f197 | A/A control |
|---|---|---|---|---|
| new | 1.032 [0.897-1.151] | 1.025 [0.993-1.055] | 0.996 [0.990-1.007] | 0.997 [0.992-1.010] |
| launch | 1.080 [0.973-1.152] | 1.003 [0.986-1.037] | 0.997 [0.971-1.022] | 1.001 [0.974-1.034] |
| approve | 1.046 [0.934-1.133] | 1.012 [1.003-1.036] | 0.994 [0.982-1.013] | 1.021 [1.010-1.032] |
| move | 1.004 [0.946-1.040] | 0.995 [0.964-1.036] | 1.004 [0.991-1.022] | 0.990 [0.983-1.019] |
| suspend | 0.999 [0.960-1.047] | 0.994 [0.947-1.010] | 1.007 [0.989-1.015] | 1.002 [0.994-1.011] |
| resume | 1.064 [0.960-1.089] | 0.992 [0.984-1.012] | 1.004 [0.997-1.006] | 1.008 [0.997-1.014] |
| pull | 1.017 [0.982-1.056] | 1.010 [0.980-1.035] | 1.002 [0.994-1.011] | 1.001 [0.988-1.005] |

In the final run no op, single or pooled, has an interval above 1. The
first run under heavy load called acp new slower, 1.163 [1.032-1.464]; the
reruns at a lower load do not repeat it (acp new 1.031 [0.998-1.058], then
0.995 [0.990-1.019]). Approve, which runs no code of this branch, came out
1.012 [1.003-1.036] once; the A/A control, two copies of one build, gives
it 1.021 [1.010-1.032], so the bench's own bias on that op is larger than
that, and the final run gives 0.994 [0.982-1.013]. `base-vs-b0f25c0/`
calls acp resume faster, 0.965 [0.919-0.986].

### Memory, and the fix it led to

The resident size `ps` reports was 1.022 [1.002-1.023] for daemon b in
`base-vs-b0f25c0/`, with a strong order effect: the build that ran second
in a pair, its pages touched last, held more. Read as physical footprint
(`footprint/base-vs-b0f25c0/`), daemon b, the node that answers its peer's
facts, held 1.026 [1.017-1.031] of the base, about 400 KiB, and daemon a,
which measures no facts, 1.006 [1.003-1.022]. A bisect with 6 pairs a run
(not kept) put it in the facts commits: before `37cbe1b`, daemon b held
10 to 12% more. A probe's task had grown, in the test profile, from 2840
bytes at the base to 10664 at `32fe4bd`, its 4 KiB read buffer counted
twice, and to 3512 at `37cbe1b`; each of the 18 worker threads that polls
one carries it on its stack. `e61f197` brings it to 2424 bytes, smaller
than the base's: the command, a few hundred bytes, is built and started in
a plain function, so it is gone before the probe first waits, and the
reader takes 256 bytes at a time, which still holds every probe's answer
in one read (`rustc -vV`, 192 bytes, is the longest).

A daemon in the bench lives through a whole phase, so a run's 21 samples of
its size are 21 looks at one process, and their interval says nothing of
how much two processes of one build differ. Two copies of the base give
daemon b 0.992 [0.989-0.994] (`footprint/base-vs-base/`). The size of
each daemon after the timed phase, one number a session, median of its
pairs, candidate over base:

| sessions | daemon a | daemon b |
|---|---|---|
| A/A, the base against itself: 15 pairs, then three of 4 pairs | 1.000, 0.990, 0.995, 1.023 | 0.992, 1.001, 0.967, 1.022 |
| `e61f197`: 21 pairs, then three of 4 pairs | 1.006, 1.012, 1.016, 0.998 | 1.012, 0.966, 0.960, 0.967 |
| facts commits alone, `net-only` alone (15 pairs) | 0.997, 0.997 | 0.995, 0.982 |

Daemon b of `e61f197` falls inside the A/A spread or below it in every
session, and below the median of the A/A sessions in three of four.
Daemon a's sessions overlap the A/A ones; a Mann-Whitney test of the four
against the four gives a two-sided p of 0.49. The short sessions' raw samples are
not kept; `footprint/fpsum.py` gives these numbers from a `samples.tsv`.

The micro A/B below measures each changed path alone, in fresh processes,
so its samples are independent: the facts measure and the request path
hold the base's footprint, 1.003 [0.986-1.028] and 1.001 [0.999-1.007].

## Against main

`anchor-vs-e61f197/`, main `176622e` against `e61f197`, pooled: new 0.995
[0.988-1.013], move 1.007 [0.994-1.023], suspend 1.003 [0.998-1.009],
approve 0.998 [0.989-1.010], resume 1.015 [1.009-1.028], pull 1.010
[1.004-1.019], launch 1.139 [1.039-1.193]; claude and codex launch 1.197
and 1.210, claude and codex resume 1.037 and 1.016, claude and codex pull
1.019 and 1.010 are slower, and the daemons' resident size is 1.032 and
1.034. These are the base's, which the A/B lab measured against main
(`docs/evidence/perf-ab/m2-head/`, pooled the same way): launch 1.168
[1.149-1.184], resume 1.029 [1.019-1.040], pull 1.017 [1.004-1.025], daemon
a 1.024 and b 1.038, for the reasons its README gives (R2's launch, which
resume and pull end in, and M2-06's `git symbolic-ref`). Against the base,
this branch moves none of them: launch 0.997 [0.971-1.022], resume 1.004
[0.997-1.006], pull 1.002 [0.994-1.011] (`footprint/base-vs-e61f197/`).

## The code each fix changed, measured alone

Each micro A/B builds two test binaries, the base and the candidate, and
runs them in turns, the first alternating per pair, with the same input.
Sources and raw samples are in `micro/`; `micro/analyze.py <samples>
<metrics>` and `micro/mem-analyze.py <samples>` give the medians and
intervals. The `.rs` files are test functions pasted into the crate's test
module for the run, not part of the build.

- Frame reading (`ceba4ce`, `micro/frame-bench.rs`, release, an in-memory
  reader, the old and the new `read_frame` in one binary): new over old 0.27
  for a 1 MiB frame, 0.56 for 8 MiB, 0.87 for 1000 bytes, 0.58 for 12
  bytes, 0.96 for 100 bytes; the buffer of a 1 MiB frame is 1 MiB instead
  of 2 MiB, of an 8 MiB frame 8 MiB instead of 16 (`micro/frame-results.txt`).
- Requests over the in-memory multiplexer (`8d35276` against `c20483f`,
  release, `micro/rpc-bench.rs`, 62 pairs): 3000 pings 0.996 [0.968-1.032],
  3000 appends of a 200 byte record 0.996 [0.969-1.013], 60 appends of a 1
  MiB record 0.978 [0.958-0.986], 200 rounds of 16 appends at once 0.960
  [0.936-0.996].
- A facts measure of fake tools (`micro/facts-bench.rs`, 20 measures each):
  release, `8d35276` against `c20483f`, 31 pairs: every tool 1.002
  [0.912-1.092], node and python3 0.910 [0.824-1.040]; test profile,
  `8d35276` against `37cbe1b`, 61 pairs: every tool 1.010 [0.931-1.064],
  node and python3 0.979 [0.897-1.108], the iCloud probes 0.969
  [0.849-1.121]. A measure's time is that of its slowest process start, so
  these intervals are wide; the e2e counters above are the load-insensitive
  measure of it.
- Memory, `8d35276` against `e61f197`, test profile, a multi-thread
  runtime as the daemon runs, 21 pairs of fresh processes
  (`micro/mem-run.sh`, `micro/mem-samples-test-e61f197.txt`): 20 facts
  measures of every fake tool (`micro/mem-facts-bench.rs`) take 0.993
  [0.988-1.007] of the time and end at 1.003 [0.986-1.028] of the
  footprint, medians 4624 and 4640 KiB; 1000 pings, 1000 small appends,
  40 appends of 1 MiB and 100 rounds of 16 appends at once
  (`micro/mem-rpc-bench.rs`) take 1.003 [0.999-1.006] and end at 1.001
  [0.999-1.007], medians 11408 KiB both.

## Memory, by design

- A probe holds at most one fact of its output, 4096 bytes, however much
  the tool prints, where it held all of it and all of its stderr until the
  tool exited or ten seconds passed (M2-13). Its task is 2424 bytes in the
  test profile, against the base's 2840.
- A peer's requests take at most `MAX_REQUEST_BYTES`, 16 control frames,
  about 16 MiB, over all its sessions, where they had no bound (M2-15). A
  request's frame is dropped once decoded, so held work holds the decoded
  request alone instead of both, and its buffer is allocated once at its
  exact size. A served request's task stays 1064 bytes.
- A frame read as a response grows only to its declared length, half the
  room a 1 MiB or 8 MiB frame took.
