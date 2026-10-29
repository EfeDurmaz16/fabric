# M2 integrated: performance after every M2 fix

Recorded on 2026-10-27 with `scripts/bench-m2.sh docs/evidence/m2-integration/perf 3 15`, the script and arguments of the baseline, unchanged, at commit `069ecbf` (the integrated branch with the launch fix below). Same machine as the baseline (Apple M5 Pro, 18 cores, macOS arm64, rustc 1.97.1, test profile). No other benchmark or build of this work ran beside it; the load average was 5.3 to 7.0 from the desktop (a browser and system daemons), above the baseline's 3.6 to 4.1, per run in `perf/machine.txt`. The `dirty: 1 paths` line is the output directory the script made itself.

Baseline: `docs/evidence/m2-baseline/perf`. The rule is no hot path more than 1.5 times slower (issue #26). The tables are computed from the raw files of both runs by the method of `docs/evidence/m2-core/perf.md` (median of the three budget p25s; bench p25 interpolated and median of 15 samples), which reproduces that file's tables exactly.

## Runs on the integrated branch

| Directory | Commit | Result |
|---|---|---|
| `perf-run1-failed/` | `4a6d07d` | Stopped in its first budget run: every resume and pull started one more `git` than `budget.toml` said (17/21, 51/55). It is the `git symbolic-ref` that restores `refs/remotes/origin/HEAD` (M2-06); the job's workspace is a clone. `budget.toml` now records it with a `[[change]]` entry (`84167d5`). |
| `perf-run2-launch-regression/` | `939b279` | Budget within limits. The bench found Claude launch at 1.58 and Codex launch at 1.51 times the baseline: R2 held every harness until the node answered its registration, so the node's durable writes after the harness record (about 30 ms here) moved in front of every headless start. Fixed in `2b05583`: the node sends `harness_recorded` once the record is durable and the CLI releases a harness not wired to the terminal then. |
| `perf/` | `069ecbf` | The run below. Every op within 1.5 times the baseline. |

## Time budget, `budget::hot_paths` (ms)

| op | p25 runs before | median p25 before | p25 runs after | median p25 after | ratio | processes after |
|---|---|---:|---|---:|---:|---|
| acp suspend | 359 / 344 / 343 | 344 | 379 / 387 / 388 | 387 | 1.12 | equal |
| acp move | 143 / 140 / 137 | 140 | 143 / 146 / 142 | 143 | 1.02 | equal |
| acp resume | 538 / 554 / 529 | 538 | 624 / 594 / 603 | 603 | 1.12 | git 17/21, kill 0/0, lsof 0/0, pgrep 0/0, ps 3/3 |
| acp pull | 1010 / 979 / 992 | 992 | 1057 / 1122 / 1035 | 1057 | 1.07 | git 51/55, kill 0/0, lsof 0/0, pgrep 0/0, ps 3/3 |
| claude suspend | 433 / 403 / 397 | 403 | 429 / 441 / 440 | 440 | 1.09 | equal |
| claude move | 145 / 142 / 138 | 142 | 141 / 143 / 143 | 143 | 1.01 | equal |
| claude resume | 703 / 704 / 663 | 703 | 813 / 756 / 743 | 756 | 1.08 | git 17/21, kill 0/0, lsof 0/0, pgrep 0/0, ps 3/3 |
| claude pull | 1250 / 1206 / 1190 | 1206 | 1299 / 1275 / 1278 | 1278 | 1.06 | git 51/55, kill 0/0, lsof 0/0, pgrep 0/0, ps 3/3 |
| codex suspend | 638 / 623 / 608 | 623 | 625 / 655 / 676 | 655 | 1.05 | equal |
| codex move | 146 / 139 / 138 | 139 | 143 / 145 / 147 | 145 | 1.04 | equal |
| codex resume | 909 / 894 / 883 | 894 | 990 / 976 / 1010 | 990 | 1.11 | git 17/21, kill 0/0, lsof 1/1, pgrep 0/0, ps 3/3 |
| codex pull | 1658 / 1640 / 1602 | 1640 | 1782 / 1724 / 1780 | 1780 | 1.09 | git 51/55, kill 0/0, lsof 2/2, pgrep 0/0, ps 3/3 |
| cold start | 47 / 56 / 56 | 56 | 56 / 57 / 57 | 57 | 1.02 | equal |
| restart | 74 / 82 / 71 | 74 | 72 / 74 / 73 | 73 | 0.99 | equal |

Every op was within its `budget.toml` limit in every run. "equal" means the process counts equal the baseline's; resume and pull run one more `git` each, as `budget.toml` now says. Suspend and move did not change in code, so their 1.0 to 1.12 ratios are the higher load of this run.

## M2 bench, `bench::m2_paths` (ms, 15 runs after one warm-up)

| op | p25 before | p25 after | median before | median after | ratio of medians |
|---|---:|---:|---:|---:|---:|
| acp approve | 14.4 | 14.1 | 15.0 | 14.9 | 0.99 |
| acp launch | 60.8 | 61.5 | 63.7 | 62.6 | 0.98 |
| acp new | 138.6 | 140.2 | 139.2 | 142.1 | 1.02 |
| claude approve | 13.7 | 14.9 | 13.9 | 15.9 | 1.14 |
| claude launch | 86.8 | 104.9 | 86.9 | 107.0 | 1.23 |
| claude new | 129.2 | 129.1 | 130.2 | 130.9 | 1.01 |
| cli ls | 4.4 | 4.9 | 5.0 | 5.1 | 1.02 |
| cli version | 2.8 | 2.9 | 3.0 | 3.1 | 1.03 |
| codex approve | 14.8 | 14.1 | 14.9 | 15.8 | 1.06 |
| codex launch | 97.9 | 118.0 | 98.8 | 119.1 | 1.20 |
| codex new | 128.8 | 129.6 | 130.2 | 131.2 | 1.01 |
| ledger append | 14.0 | 15.1 | 14.1 | 15.9 | 1.13 |

The largest ratio is 1.23, Claude launch: a headless launch now waits for the node's durable harness record (R2) instead of overlapping it, plus one `/bin/sh` start. The 1.13 and 1.14 of an approval and a ledger append are one durable write each under the higher load; nothing in M2 changed that path.

## Raw data

As in the baseline: `perf/budget-1.log` to `budget-3.log`, `perf/budget-rows.md`, `perf/bench.log`, `perf/bench-raw.tsv`, `perf/machine.txt`, `perf/build.log`, and the same files of the two earlier runs in their directories.
