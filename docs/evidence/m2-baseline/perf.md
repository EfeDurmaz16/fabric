# M2 baseline: performance of clean main

Recorded on 2026-10-27 with `scripts/bench-m2.sh docs/evidence/m2-baseline/perf 3 15` at commit `b6db126` (main `176622e` plus the bench harness, a test-only change). Apple M5 Pro, 18 cores, macOS arm64, rustc 1.97.1, test profile, load average 3.6 to 4.1 from other work during the runs (per run in `perf/machine.txt`; the host name is removed from its `uname` line). No other benchmark ran beside it.

The rule for M2 (issue #26, `CONTRIBUTING.md`): no hot path may get more than 1.5 times slower. After the M2 changes, run the same script, unchanged, on the same machine into a new directory and compare against the raw files here.

## Raw data

| File | What |
|---|---|
| `perf/budget-1.log` to `perf/budget-3.log` | Three full runs of `FABRIC_BUDGET=report cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture`: 2 counted warm-up runs, then 7 timed runs per op, table of p25 and median |
| `perf/budget-rows.md` | The three tables' rows, prefixed with the run |
| `perf/bench.log` | `FABRIC_BENCH_REPS=15 ... bench::m2_paths`: every sample as a `RAW` line, then the table |
| `perf/bench-raw.tsv` | The bench samples: op, run, microseconds |
| `perf/machine.txt` | Commit, machine, toolchain, load before each run |
| `perf/build.log` | The build before the runs |

## Time budget, `budget::hot_paths` (ms)

p25 of 7 timed runs, per budget run; the median of the three is the number to compare. All process counts matched `budget.toml` exactly and every op was within its limit.

| op | p25, runs 1 / 2 / 3 | median p25 | budget baseline | median of runs, 1 / 2 / 3 |
|---|---|---:|---:|---|
| acp suspend | 359 / 344 / 343 | 344 | 342 | 384 / 352 / 345 |
| acp move | 143 / 140 / 137 | 140 | 144 | 151 / 145 / 142 |
| acp resume | 538 / 554 / 529 | 538 | 545 | 709 / 653 / 652 |
| acp pull | 1010 / 979 / 992 | 992 | 995 | 1099 / 1173 / 1078 |
| claude suspend | 433 / 403 / 397 | 403 | 407 | 439 / 421 / 407 |
| claude move | 145 / 142 / 138 | 142 | 145 | 147 / 144 / 139 |
| claude resume | 703 / 704 / 663 | 703 | 684 | 851 / 868 / 802 |
| claude pull | 1250 / 1206 / 1190 | 1206 | 1221 | 1324 / 1383 / 1322 |
| codex suspend | 638 / 623 / 608 | 623 | 550 | 641 / 635 / 619 |
| codex move | 146 / 139 / 138 | 139 | 144 | 148 / 146 / 142 |
| codex resume | 909 / 894 / 883 | 894 | 836 | 1092 / 1081 / 1003 |
| codex pull | 1658 / 1640 / 1602 | 1640 | 1486 | 1825 / 1798 / 1713 |
| cold start (daemon) | 47 / 56 / 56 | 56 | 57 | 59 / 57 / 57 |
| restart (daemon) | 74 / 82 / 71 | 74 | 78 | 79 / 86 / 72 |

Resume and pull include a fake headless turn, so they include a launch. The Codex rows sit 10 to 13 percent above the recorded baselines on this machine today; they are within the limit and this is the number M2 compares against.

## M2 bench, `bench::m2_paths` (ms, 15 runs after one warm-up)

| op | p25 | median | p95 |
|---|---:|---:|---:|
| acp new | 138.4 | 139.3 | 147.8 |
| acp launch | 60.8 | 63.7 | 71.9 |
| acp approve | 14.0 | 15.0 | 16.2 |
| claude new | 129.2 | 130.3 | 142.3 |
| claude launch | 86.8 | 86.9 | 88.9 |
| claude approve | 13.7 | 13.9 | 16.0 |
| codex new | 128.4 | 130.2 | 133.2 |
| codex launch | 97.8 | 98.8 | 154.2 |
| codex approve | 14.8 | 14.9 | 17.8 |
| ledger append (one job, ledger grows by one per run) | 14.0 | 14.1 | 17.0 |
| cli version (no node contact) | 2.8 | 3.0 | 3.4 |
| cli ls (round trip to the node) | 4.3 | 5.0 | 5.5 |

`launch` is `fabric job run <job> --headless --prompt one` on a new job: plan, launch, register, the fake turn and the exit report. `approve` and `ledger append` are one durable ledger record through the CLI. p95 of 15 samples is the second-largest sample, so read it as a rough tail only.

## Limits

These are loopback fixture numbers with fake probed tools, not a two-machine SLO. The comparison after M2 must use the same machine, profile and script, and be run when the machine is about as loaded; a difference under the budget's own noise (limit 1.3 times baseline or baseline plus 50 ms) is not a regression.
