# M3 provider bridge, Fabric side: performance

Recorded on 2026-10-27 at commit `1c26c1f` on an Apple M5 Pro, 18 cores, macOS arm64, rustc 1.97.1. The machine was busy with other work: a load average of 8 to 11 in every run, in `bench/machine.txt` and `budget/machine.txt`. No build or other benchmark of this branch ran beside these runs.

## Bridge latencies (new)

`scripts/bench-bridge.sh docs/evidence/m3-bridge/bench`: the ignored test `bridge_latencies` under the test profile (the budget's) and the release profile. The worker is the Rust test double, so these are Fabric's side of the bridge: process start, pipes, framing, parsing and the log, not a provider. Every sample is in `bench/<profile>/*_us.txt`, one per line, in microseconds.

| Path | Profile | n | p50 | p90 | p99 | max |
|---|---|---:|---:|---:|---:|---:|
| worker spawn and handshake answered | test | 50 | 1684 | 1970 | 2141 | 2141 |
| worker spawn and handshake answered | release | 50 | 1420 | 1588 | 1839 | 1839 |
| create round trip, in-memory log | test | 500 | 144 | 155 | 244 | 445 |
| create round trip, in-memory log | release | 500 | 42 | 48 | 57 | 98 |
| create round trip, journal | test | 200 | 7987 | 11922 | 14085 | 16822 |
| create round trip, journal | release | 200 | 7964 | 13174 | 15980 | 18065 |

The journal round trip is two committed SQLite transactions, `dispatched` before the frame and the answer after it, each with `synchronous = FULL` and `fullfsync` (ADR 0020): about 4 ms of each is the fsync, which is why the profiles agree. That cost is the price of recording intent before effect and is not on any existing hot path. There is no earlier bridge to compare with; these are the first numbers.

## Time budget, `budget::hot_paths` (ms)

The bridge adds no work to a suspend, move, resume, pull, cold start or restart; the journal's version 2 migration runs once when a journal opens. Because the machine was loaded, the budget ran on this branch and on its base, `origin/m2/durable-job-foundations` (`f381d42`, a `git archive` copy built in its own directory), interleaved: base, branch, base, branch, after a first branch run. `budget/run-*.log` are the branch, `budget/base-run-*.log` the base, in report mode.

| op | base p25 (runs 1, 2) | branch p25 (runs 1, 2, 3) | branch median / base median |
|---|---|---|---:|
| acp suspend | 401, 462 | 447, 427, 415 | 0.99 |
| acp move | 157, 166 | 151, 165, 148 | 0.93 |
| acp resume | 636, 806 | 643, 650, 650 | 0.90 |
| acp pull | 1235, 1363 | 1161, 1145, 1138 | 0.88 |
| claude suspend | 472, 495 | 431, 488, 471 | 0.97 |
| claude move | 156, 159 | 142, 156, 160 | 0.99 |
| claude resume | 810, 880 | 808, 815, 823 | 0.96 |
| claude pull | 1491, 1481 | 1432, 1522, 1382 | 0.96 |
| codex suspend | 752, 794 | 702, 709, 727 | 0.92 |
| codex move | 161, 150 | 145, 149, 145 | 0.93 |
| codex resume | 1077, 1219 | 1101, 1173, 1009 | 0.96 |
| codex pull | 2010, 2114 | 1958, 2140, 1910 | 0.95 |
| cold start | 58, 62 | 54, 56, 57 | 0.93 |
| restart | 78, 78 | 87, 74, 86 | 1.10 |

Every op is within 1.5 times its base under the same load, and every process count equals `budget.toml` in every run. Against the quiet-machine baselines of `budget.toml` some ops read over their limit in report mode (branch run 1: acp suspend, codex resume, codex pull); the base read over too under the same load, so that is the load, not this branch.
