# A/B of the cli-protocol-ops work against the anchor

The performance rule (CONTRIBUTING.md): no hot path slower, or larger,
than the anchor, `main` at `176622e`, and the aim is to beat it. Both
runs below are `scripts/bench-ab.sh` against that anchor, op by op in
the same session, on an Apple M5 Pro (18 cores, macOS arm64, rustc
1.97.1) while six other workflows built and tested beside them: load
average 15 to 23 (`machine.txt`), two to three times the load of the
M2 runs in `docs/evidence/perf-ab`. The pairs give both builds the
same load, so the ratios hold, but the intervals are wide: an op whose
median is above 1 and whose interval holds 1 is profiled below, from
the counters, which do not depend on load.

| Directory | Profile | Pairs | Counted runs | Candidate |
|---|---|---:|---:|---|
| `run1/` | test | 20 | 3 | `7612b93`: items 2, 7, 11, 13, 14, 15, 17, 19, 20, M2-18 and M2-22. A rebase changed only its test helpers (`5f076b2` on the branch). Its `run.log` holds the first lines only: the file the run wrote to was moved during the run |
| `run2-release/` | release | 20 | 3 | `b41ec48`: all of the above, the managed subcommands, repair and export (R3), and the docs |

Also here: `m2-18-ledger-bench.txt` (M2-18's benchmark before and after),
`memory-diagnostic.sh` (the daemon memory diagnostic below), `gates.txt`
(every gate of CONTRIBUTING on the branch) and `budget-report.txt` (the
time budget's table, which fails only on the `ps` counts this branch
lowers).

`8739323`, the last commit, makes two in-memory additions smaller
(`Job` 800 to 624 bytes, a peer's acknowledged head 64 bytes instead of
a tree node); no run includes it.

## Timed pairs

Medians in ms, anchor / candidate, and the median of the pairs' ratios,
candidate over anchor, with its 95% bootstrap interval.

| op | test: anchor / candidate | ratio [95%] | release: anchor / candidate | ratio [95%] |
|---|---:|---|---:|---|
| acp new | 468.8 / 343.4 | 0.786 [0.704-0.834] faster | 505.8 / 458.6 | 0.861 [0.699-1.104] |
| acp launch | 185.0 / 137.9 | 0.720 [0.616-0.821] faster | 178.3 / 123.8 | 0.778 [0.663-0.822] faster |
| acp approve | 40.7 / 33.6 | 0.798 [0.728-0.941] faster | 38.7 / 35.2 | 0.948 [0.833-1.092] |
| claude new | 352.1 / 346.7 | 0.888 [0.715-1.141] | 645.1 / 437.6 | 0.622 [0.414-0.967] faster |
| claude launch | 164.4 / 167.4 | 1.028 [0.867-1.135] | 175.7 / 179.0 | 0.962 [0.844-1.121] |
| claude approve | 34.8 / 34.5 | 1.054 [0.839-1.269] | 35.8 / 35.5 | 1.100 [0.768-1.268] |
| codex new | 384.7 / 350.5 | 0.850 [0.677-1.056] | 424.7 / 407.4 | 0.872 [0.751-1.368] |
| codex launch | 205.1 / 214.2 | 1.028 [0.895-1.173] | 215.1 / 232.5 | 1.014 [0.879-1.260] |
| codex approve | 30.8 / 35.0 | 1.014 [0.927-1.220] | 32.7 / 29.0 | 0.860 [0.764-1.170] |
| ledger append | 33.9 / 38.1 | 0.975 [0.814-1.180] | 31.2 / 35.5 | 1.132 [0.902-1.323] |
| cli version | 4.5 / 4.6 | 1.041 [0.950-1.115] | 2.6 / 2.7 | 1.007 [0.908-1.130] |
| cli ls | 8.2 / 7.9 | 0.954 [0.857-1.037] | 3.9 / 3.3 | 0.833 [0.768-0.920] faster |
| acp suspend | 946.3 / 860.3 | 0.946 [0.864-1.010] | 946.8 / 820.5 | 0.870 [0.803-0.922] faster |
| acp move | 274.6 / 254.0 | 0.938 [0.825-1.032] | 271.3 / 260.9 | 0.965 [0.835-1.075] |
| acp resume | 1695.6 / 1626.8 | 1.016 [0.889-1.068] | 1882.7 / 1680.3 | 0.937 [0.854-1.009] |
| acp pull | 3020.5 / 2807.0 | 0.902 [0.839-1.080] | 3239.7 / 2903.8 | 0.884 [0.795-1.016] |
| claude suspend | 1023.8 / 964.9 | 0.960 [0.848-0.998] faster | 978.2 / 944.6 | 0.974 [0.826-1.043] |
| claude move | 296.5 / 258.4 | 0.891 [0.852-0.965] faster | 288.4 / 265.8 | 0.923 [0.765-1.062] |
| claude resume | 1828.5 / 1698.2 | 0.887 [0.804-0.979] faster | 2222.0 / 1789.4 | 0.899 [0.850-0.936] faster |
| claude pull | 2956.4 / 2765.7 | 0.944 [0.870-1.038] | 3140.7 / 2791.2 | 0.950 [0.883-0.979] faster |
| codex suspend | 1577.1 / 1356.4 | 0.942 [0.845-1.088] | 1199.1 / 1148.9 | 0.946 [0.803-1.057] |
| codex move | 262.7 / 308.6 | 1.108 [0.947-1.273] | 270.5 / 253.2 | 0.981 [0.862-1.047] |
| codex resume | 2544.7 / 2315.2 | 0.913 [0.855-1.011] | 2482.3 / 2080.4 | 0.936 [0.814-1.037] |
| codex pull | 3813.8 / 3996.3 | 0.982 [0.916-1.065] | 3887.1 / 3766.8 | 0.947 [0.869-1.019] |
| cold start | 228.6 / 275.6 | 1.040 [0.646-1.262] | 399.8 / 300.9 | 0.843 [0.557-1.670] |
| restart | 969.1 / 308.9 | 0.273 [0.249-0.332] faster | 1132.9 / 282.6 | 0.260 [0.226-0.346] faster |

No op is slower in either run. Faster where the interval excludes 1:
`restart` (0.26 to 0.27, item 11), `acp launch` (0.72 and 0.78),
`claude resume` (0.89 and 0.90), `claude pull`, `claude suspend`,
`claude move`, `acp suspend`, `cli ls` (0.83), `acp new` and
`claude new`, `acp approve`.

The M2 head had headless `claude launch` and `codex launch` at 1.21 and
1.18, `claude resume` at 1.04 and `codex resume` and `codex move`
slower (`docs/evidence/perf-ab/README.md`); here they are 0.96 to 1.03
with intervals that hold 1, or faster. The rest of the launch regression
is the hold of R2, which the launch-hold group owns (items 1, 3, 4).

Medians above 1 whose interval holds 1, profiled from the counters:

- `claude approve`, `ledger append` (one run each above 1, the other
  below): the same three full flushes and 98 ledger bytes as the
  anchor, 37% fewer allocations and one open fewer. The time is the
  three `F_FULLFSYNC` under load.
- `codex launch` (1.028 and 1.014): two processes fewer (`ps` 3 to 1),
  one full flush fewer, ten opens fewer, 11% fewer allocations.
- `cli version` (1.041 and 1.007): the same work; in the release build
  55 more allocations (189 to 244), the larger command tree clap builds
  with the `managed`, `repair` and `export` subcommands.
- `codex move` (1.108 in the test run, 0.981 in the release run) and
  `cold start` (intervals of 0.56 to 1.67): `cold start` counts 6 to 32
  CLI polls per sample on either build, so under this load it measures
  the load; `codex move` runs three processes fewer.

## Counters (test profile, run 1; the release run agrees)

| op | processes | full flushes | opens | allocations | CLI peak KiB |
|---|---:|---:|---:|---:|---:|
| new (each harness) | 3 -> 1 | 8 -> 8 | 11 -> 9 | -3% to -38% | -176 to -256 |
| launch | 4 -> 2 | 15/15/17 -> 11/14/16 | -10 to -13 | -3% to -13% | +208 to +752 |
| approve, ledger append | 0 -> 0 | 3 -> 3 | 3 -> 2 | -37% | -192 to -224 |
| cli ls | 0 -> 0 | 0 -> 0 | 13 -> 0 | -29% | -304 |
| suspend | equal | equal | -5 to -7 | -15% to -22% | -48 to +544 |
| move | 42 -> 36 to 39 | equal | 73 -> 51 to 54 | -11% to -14% | about equal |
| resume | -3 | 24 -> 23 | -12 | -2% | -80 to +720 |
| pull | -6 to -9 | -1 | -25 to -28 | -9% to -10% | +160 to +672 |
| restart | 0 -> 0 | 38 -> 12 | 271 -> 90 | -33% | -288 |

Where the processes went: `ps -o pgid=` at every registration (item 2)
and the `ps -o lstart=` at every harness exit (item 17); the two
`defaults` reads of every `job new` and every restore (item 14); the
other harness's `--version` and login on both nodes of a move and a
pull (item 19). One `git symbolic-ref` more per restore is M2-06, not
this branch. The restart's 26 fewer full flushes are item 11, and the
opens of every request, `job ls` and restart are item 20.

In the release run the CLI peaks are lower than the anchor's, or equal,
for every op but `acp launch` (+224 KiB), `acp resume` (+192), `acp
pull` (+144), `cli version` (+96) and `claude resume` (+16); the M2 head
had +16 to +100 on every op that asks the node.

## Daemon resident size

| daemon | test, run 1 | release, run 2 | M2 head, release |
|---|---|---|---|
| a | 1.038 [1.033-1.042] | 1.029 [1.025-1.033] | 0.997 [0.990-1.000] |
| b | 1.042 [1.028-1.060] | 1.054 [1.031-1.067] | 1.013 [1.010-1.023] |

Larger than the anchor, and than the M2 head. `memory-diagnostic.sh`
starts the anchor, the M2 head (`8d35276`) and this branch (`b41ec48`),
all release builds from the bench's cache, side by side, runs four
rounds of `job new`, `suspend`, removing the workspace and `resume`
against each, and prints each daemon's resident size, threads and vmmap
regions. Two runs, under the same memory pressure that compresses pages
of all three alike:

| run | build | resident KiB | threads | dirty (vmmap TOTAL) | __TEXT resident | MALLOC_SMALL resident | stacks |
|---|---|---:|---:|---:|---:|---:|---:|
| 1 | anchor | 11136 | 20 | 3632K | 12.5M | 1824K | 512K |
| 1 | M2 head | 11376 | 21 | 3760K | 12.7M | 1840K | 544K |
| 1 | this branch | 11552 | 21 | 3760K | 12.9M | 1808K | 560K |
| 2 | anchor | 9808 | 19 | 2752K | 11.3M | 1280K | 256K |
| 2 | M2 head | 9072 | 20 | 2960K | 11.3M | 1264K | 448K |
| 2 | this branch | 8688 | 20 | 2528K | 11.3M | 928K | 432K |

In neither run is this branch's private, dirty memory or heap above the
M2 head's, and it adds no thread; the M2 head's extra thread is its
authority's (item 6, node-runtime group). Where the first run shows this
branch larger, it is resident code (`__TEXT`): the release binary is
2.1% larger than the M2 head's (21103952 against 20663776 bytes), with
the serde code of the new ops and types and the code of repair and
export. Four rounds hold four jobs; the bench's daemons hold 60 to 80,
and over them the in-memory additions this branch keeps per job were
the kept records and the peer heads, which `8739323` makes smaller.
That commit is in no run here. A smaller binary would need the release
profile changed (LTO, one codegen unit), which the root `Cargo.toml`
owns.

## M2-18

`m2-18-ledger-bench.txt`: a push of one new record of a 1015-record job
fell from a median of 58.6 ms to 22.9 and 26.1 ms, and sends 158 bytes
instead of 159209.
