# M3 provider bridge across implementations: performance

Recorded on 2026-10-27 at commit `61285cc` on an Apple M5 Pro, 18 cores, macOS arm64, rustc 1.97.1, Node 22.23.2, against the Capsule worker of commit `9f5d65e` built from a clean clone (gates.md). `bench/machine.txt` says `dirty: 1 paths`: that is this evidence directory, not yet committed; the code was clean. The machine ran other work: load averages of 7 to 12 in `bench/machine.txt` and `budget/machine.txt`.

## Bridge latencies against the Capsule worker (new)

`CAPSULE_BRIDGE_WORKER=<worker> CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim scripts/bench-bridge.sh docs/evidence/m3-interop/bench`. The bench `capsule_worker_latencies` times from Fabric's side: a worker spawned until its handshake is answered, and one request until its answer is recorded. 15 samples per path after warm-ups, every one kept in `bench/<profile>/capsule/*_us.txt`, one per line, in microseconds. The Fabric profile changes Fabric's code only; the worker is the same.

| Path | Fabric profile | n | p50 | p90 | max |
|---|---|---:|---:|---:|---:|
| fixture worker spawned, handshake answered | test | 15 | 30.8 ms | 32.5 ms | 33.9 ms |
| fixture worker spawned, handshake answered | release | 15 | 29.0 ms | 30.4 ms | 30.8 ms |
| fixture create round trip, in-memory log | test | 15 | 241 us | 462 us | 750 us |
| fixture create round trip, in-memory log | release | 15 | 182 us | 734 us | 765 us |
| fixture exec round trip, in-memory log | test | 15 | 249 us | 316 us | 790 us |
| fixture exec round trip, in-memory log | release | 15 | 159 us | 248 us | 724 us |
| fixture create round trip, journal | test | 15 | 8.0 ms | 10.0 ms | 12.0 ms |
| fixture create round trip, journal | release | 15 | 8.0 ms | 8.0 ms | 8.0 ms |
| docker worker spawned, handshake answered | test | 15 | 33.7 ms | 36.8 ms | 43.6 ms |
| docker worker spawned, handshake answered | release | 15 | 29.3 ms | 30.5 ms | 30.6 ms |
| docker create round trip (container running) | test | 15 | 201 ms | 223 ms | 277 ms |
| docker create round trip (container running) | release | 15 | 192 ms | 209 ms | 238 ms |
| docker exec round trip (`echo`) | test | 15 | 88.6 ms | 97.8 ms | 104 ms |
| docker exec round trip (`echo`) | release | 15 | 88.7 ms | 96.6 ms | 115 ms |
| docker destroy round trip (container gone) | test | 15 | 231 ms | 242 ms | 249 ms |
| docker destroy round trip (container gone) | release | 15 | 221 ms | 268 ms | 270 ms |

The same script run measured the Rust double again (`bench/<profile>/*_us.txt`): spawn and handshake p50 1.9 ms (test) and 1.6 ms (release), create round trip with the in-memory log 147 us and 46 us, with the journal 8.1 ms and 7.9 ms, in line with docs/evidence/m3-bridge. So against the real worker:

- Spawn costs about 30 ms more than the double: Node starting and loading the worker. Fabric's own share is the double's 1.6 to 1.9 ms. It is paid once per worker, which lives for many requests.
- A fixture round trip is 0.16 to 0.25 ms p50 against the worker and 0.05 to 0.15 ms against the double. Capsule's own bench drives the same worker over its pipes from Node and measured 0.05 to 0.08 ms p50 over 500 sequential requests (Capsule's `benchmarks/after/bridge`); 15 samples after 5 warm-ups leave the worker's JIT colder than 500 do, so the numbers here are an upper bound on the worker's share.
- With the journal, a round trip is 8 ms with either worker: the two fsynced commits (ADR 0020) dominate it.
- Docker round trips are the docker CLI and the engine. Capsule's own smoke measured exec at 110 to 150 ms and create at 380 to 500 ms on the same engine earlier in the day.

None of these paths existed before M3, so there is nothing to regress against; they are the first numbers for the seam.

## Time budget, `budget::hot_paths` (ms)

This change adds tests, a bench, a CI job and docs; no code outside `crates/*/tests` changed since `1c26c1f`, whose budget runs are in docs/evidence/m3-bridge. The budget ran twice more on this commit in report mode (`budget/run-1.log`, `budget/run-2.log`): both passed, every process count equals `budget.toml`. Run 1 started at a load of 12.6 and read three ops over their quiet-machine limit; run 2 at 9.8 read every op within it.

| op | baseline | base p25 (m3-bridge runs 1, 2) | this commit p25 (runs 1, 2) | run 2 / baseline |
|---|---:|---|---|---:|
| acp suspend | 342 | 401, 462 | 379, 381 | 1.11 |
| acp move | 144 | 157, 166 | 157, 158 | 1.10 |
| acp resume | 545 | 636, 806 | 722, 593 | 1.09 |
| acp pull | 995 | 1235, 1363 | 1102, 1180 | 1.19 |
| claude suspend | 407 | 472, 495 | 450, 433 | 1.06 |
| claude move | 145 | 156, 159 | 157, 143 | 0.99 |
| claude resume | 684 | 810, 880 | 908, 772 | 1.13 |
| claude pull | 1221 | 1491, 1481 | 1258, 1262 | 1.03 |
| codex suspend | 550 | 752, 794 | 672, 657 | 1.19 |
| codex move | 144 | 161, 150 | 151, 159 | 1.10 |
| codex resume | 836 | 1077, 1219 | 1185, 966 | 1.16 |
| codex pull | 1486 | 2010, 2114 | 1784, 1729 | 1.16 |
| cold start | 57 | 58, 62 | 57, 58 | 1.02 |
| restart | 78 | 78, 78 | 79, 75 | 0.96 |

Every op is within 1.5 times both its baseline and its base's runs under load.
