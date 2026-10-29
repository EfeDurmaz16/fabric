# M2 core: performance after the managed job foundations

Recorded on 2026-10-27 with `scripts/bench-m2.sh docs/evidence/m2-core/perf 3 15`, the script and arguments of the baseline, unchanged, at commit `638e122` (the managed authority, journal and client ops). Same machine as the baseline (Apple M5 Pro, 18 cores, macOS arm64, rustc 1.97.1, test profile), load average 3.8 to 5.8 from other work during the runs (per run in `perf/machine.txt`). No other benchmark ran beside it. The `dirty: 1 paths` line is the output directory the script made itself; no source was changed during the run.

Baseline: `docs/evidence/m2-baseline/perf`. The rule is no hot path more than 1.5 times slower (issue #26). The tables are computed from the raw files of both runs by the same method (median of the three budget p25s; bench p25 interpolated and median of 15 samples).

## Time budget, `budget::hot_paths` (ms)

| op | p25 runs before | median p25 before | p25 runs after | median p25 after | ratio | processes equal |
|---|---|---:|---|---:|---:|---|
| acp suspend | 359 / 344 / 343 | 344 | 345 / 340 / 346 | 345 | 1.00 | yes |
| acp move | 143 / 140 / 137 | 140 | 145 / 142 / 140 | 142 | 1.01 | yes |
| acp resume | 538 / 554 / 529 | 538 | 539 / 538 / 545 | 539 | 1.00 | yes |
| acp pull | 1010 / 979 / 992 | 992 | 1024 / 975 / 981 | 981 | 0.99 | yes |
| claude suspend | 433 / 403 / 397 | 403 | 410 / 398 / 406 | 406 | 1.01 | yes |
| claude move | 145 / 142 / 138 | 142 | 143 / 144 / 142 | 143 | 1.01 | yes |
| claude resume | 703 / 704 / 663 | 703 | 703 / 713 / 712 | 712 | 1.01 | yes |
| claude pull | 1250 / 1206 / 1190 | 1206 | 1253 / 1236 / 1237 | 1237 | 1.03 | yes |
| codex suspend | 638 / 623 / 608 | 623 | 614 / 613 / 617 | 614 | 0.99 | yes |
| codex move | 146 / 139 / 138 | 139 | 145 / 144 / 144 | 144 | 1.04 | yes |
| codex resume | 909 / 894 / 883 | 894 | 892 / 906 / 892 | 892 | 1.00 | yes |
| codex pull | 1658 / 1640 / 1602 | 1640 | 1616 / 1601 / 1625 | 1616 | 0.99 | yes |
| cold start | 47 / 56 / 56 | 56 | 45 / 58 / 46 | 46 | 0.82 | yes |
| restart | 74 / 82 / 71 | 74 | 65 / 64 / 72 | 65 | 0.88 | yes |

Every op is within its `budget.toml` limit in every run, and every process count is identical to the baseline's (`perf/budget-rows.md`). The largest ratio is 1.04 (codex move); cold start and restart are faster, within run-to-run noise. The authority adds one thread and no process to the daemon's start, and makes no journal, so nothing it does is on these paths.

## M2 bench, `bench::m2_paths` (ms, 15 runs after one warm-up)

| op | p25 before | p25 after | median before | median after | ratio of medians |
|---|---:|---:|---:|---:|---:|
| acp approve | 14.4 | 14.6 | 15.0 | 14.9 | 0.99 |
| acp launch | 60.8 | 60.9 | 63.7 | 62.6 | 0.98 |
| acp new | 138.6 | 140.2 | 139.2 | 143.1 | 1.03 |
| claude approve | 13.7 | 14.7 | 13.9 | 14.9 | 1.07 |
| claude launch | 86.8 | 86.4 | 86.9 | 90.9 | 1.05 |
| claude new | 129.2 | 133.3 | 130.2 | 134.4 | 1.03 |
| cli ls | 4.4 | 4.4 | 5.0 | 4.9 | 0.97 |
| cli version | 2.8 | 2.9 | 3.0 | 3.0 | 0.99 |
| codex approve | 14.8 | 14.4 | 14.9 | 14.9 | 1.00 |
| codex launch | 97.9 | 98.3 | 98.8 | 101.8 | 1.03 |
| codex new | 128.8 | 133.6 | 130.2 | 134.2 | 1.03 |
| ledger append | 14.0 | 14.4 | 14.1 | 15.1 | 1.07 |

The largest ratio is 1.07 (claude approve and ledger append, both about 1 ms on 14 ms), under the budget's own noise allowance and far under 1.5. Standalone job creation, launch, approval and CLI paths do not call the authority: only a standalone lookup of a job this node does not hold asks it, on the refusal path.

## Not measured here

Managed commands themselves (one SQLite transaction with fsync per command) are not on a standalone hot path and have no budget yet. Their cost is a journal write per command, off every standalone path.
