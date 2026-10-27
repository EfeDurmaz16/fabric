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

## Runs

| Directory | Anchor | Candidate | Pairs | Counted runs |
|---|---|---|---:|---:|
| `anchor-vs-c20483f/` | `176622e` (main) | `c20483f` | 21 | 3 |
| `base-vs-head/` | `8d35276` (M2 head, this branch's base) | `37cbe1b` | 21 | 3 |
| `anchor-vs-head/` | `176622e` (main) | `37cbe1b` | 21 | 3 |

All three ran with `scripts/bench-ab.sh` in the test profile on 2026-10-27
on an Apple M5 Pro (18 cores, macOS arm64, rustc 1.97.1), while other
workflows built and tested beside them: load average 11 to 43
(`machine.txt`). The pairs give both builds the same load, but the absolute
times are two to three times those of the lab's runs, and a single op's
interval is wide. Each op's three harness shapes run the same facts, carry
and network code, so the shapes are also pooled below (63 pairs).

The first run found the probe change costing elsewhere: a move counted 117
opens against main's 73, because every probe now opened `/dev/null` for its
stdin and stderr, and 167 KB more allocated bytes, the reader's 4 KiB buffer
in each probe's task. `37cbe1b` fixed both; the two later runs measure it.

## What this branch changes against its base

`base-vs-head/report.md`, counters of every `fabric` process of the side
during the op, median of the counted runs, base `8d35276` to this branch:

| op | opens | pipe and socket reads | allocations | spawns, flushes |
|---|---|---|---|---|
| acp, claude, codex move | 73 to 33 | 124 to 76 | 8412 to 8206, 8267 to 8051, 8270 to 8055 | equal |
| acp, claude, codex pull | 416 to 371, 461 to 415, 512 to 466 | 492 to 436, 512 to 450, 515 to 450 | 28734 to 28437, 33868 to 33588, 29412 to 29103 | equal |
| acp, claude, codex suspend | 142 to 139, 145 to 142, 154 to 149 | 187 to 183, 185 to 181, 190 to 186 | 12866 to 12843, 16394 to 16372, 12936 to 12563 | equal |
| claude, codex resume | 249 to 247, 295 to 291 | 173 to 166, 182 to 177 | 11448 to 11431, 11585 to 11564 | equal |
| acp, claude, codex new | 11 to 10 | 6 to 4 | 38522 to 38515, 1688 to 1680, 1703 to 1680 | equal |

No op that runs this code does more counted work than the base, and the
daemons' resident size is the base's: 0.999 [0.992-1.002] and 1.009
[0.997-1.023]. Times, pooled over the three shapes, median of the pairs'
ratios with a 95% bootstrap interval:

| op | base-vs-head | anchor-vs-head |
|---|---|---|
| new | 1.032 [0.897-1.151] | 1.022 [0.922-1.083] |
| move | 1.004 [0.946-1.040] | 1.023 [0.965-1.057] |
| suspend | 0.999 [0.960-1.047] | 0.991 [0.942-1.066] |
| resume | 1.064 [0.960-1.089] | 1.046 [0.986-1.104] |
| pull | 1.017 [0.982-1.056] | 1.064 [0.995-1.116] |

Every pooled interval holds 1. Of the ops this branch's code runs in, two
single ones have an interval above 1: acp new against the base, 1.163
[1.032-1.464], and codex new against main, 1.187 [1.045-1.374]. Neither is
repeated by the other run, nor by the other shapes, which run the same code
(claude new against main is faster, 0.849 [0.772-0.989]). The code they run
here is the two iCloud probes of `new`, and their counters show one open and
two pipe reads fewer than both the base and main, and 7 to 23 allocations
fewer than the base; main makes 8 to 11 fewer still, which the base's other
M2 changes added. With 26 ops per run, about one interval in twenty
excludes 1 by chance. The launch rows main's run calls slower (claude
1.164, codex 1.082) are R2's, which the A/B lab's README names
(`docs/evidence/perf-ab/README.md`, item 1); this branch does not touch
launch. Resume and pull, whose medians are above 1 against main, and codex
resume, slower in the first run, end in that launch and run one more `git
symbolic-ref` (M2-06), as the counters of both runs against main show.

## The code each fix changed, measured alone

Each micro A/B builds two test binaries, the base and the candidate, and
runs them in turns, the first alternating per pair, with the same input.
Sources and raw samples are in `micro/`; `micro/analyze.py <samples>
<metrics>` gives the medians and intervals.

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

## Memory

- A probe holds at most one fact of its output, 4096 bytes, however much
  the tool prints, where it held all of it and all of its stderr until the
  tool exited or ten seconds passed (M2-13).
- A peer's requests take at most `MAX_REQUEST_BYTES`, 16 control frames,
  about 16 MiB, over all its sessions, where they had no bound (M2-15). A
  request's frame is dropped once decoded, so held work holds the decoded
  request alone instead of both, and its buffer is allocated once at its
  exact size.
- A frame read as a response grows only to its declared length, half the
  room a 1 MiB or 8 MiB frame took.
