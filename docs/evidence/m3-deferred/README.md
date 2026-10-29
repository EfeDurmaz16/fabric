# M3 evidence: the deferred items

Recorded on 2026-10-29 on `m3/capsule-bridge`. The host is an Apple M5 Pro, macOS arm64, 18 cores, with other work running at a load average of 6 to 10. Nothing here was timed: no benchmark, A/B run or stress loop ran, and the time budget ran in count mode. The Perf phase measures these commits on its own.

Four items were left open when the M3 pull requests were opened (PR #32's "Limits and deferred items"). Each is fixed here with a test that fails without the fix.

| Commit | Item | Change |
|---|---|---|
| `07c14c6` | process group | The worker leads a process group of its own. When it is lost or closed, it gets the kill grace to exit, what is left of its group gets the rest of the grace, then the group gets SIGKILL, once. A dropped bridge kills the group. A provider effect is never in the group, so ending it is not a cancel |
| `fa350bd` | process group | The Docker interop test checks that a killed worker's attached `docker exec` client is gone within the kill grace while the command the engine runs for it runs on |
| `8c10558` | journal off the runtime | The client reaches its log through a `Recorder`. `LogThread` runs the journal on a thread of its own behind a bounded queue; `MemoryLog` records in place. Recording a request `dispatched` and queueing its frame are one call on the log thread, so the record still precedes the frame |
| `7126f63` | Docker smoke in CI | Job `bridge-docker` runs the Docker test against the runner's engine with busybox 1.37.0 pulled by digest, and fails if a container with a `fabric.job` label is left |
| `1569bc9` | v1 additions | `observe` carries the original operation's scope (from the journal, never from the caller) and both profiles observe `sandbox.exec`; the fixtures Capsule wrote are copied byte for byte and the manifest pin moves to `af9a31d70699b1e0fc1ffec8ede50eafbb3bd8fcb35d7b1dd99a9826098d7556` |
| `7c43cb6` | v1 additions | The interop tests label every fixture seed with the operation that made it, expect an exec observe by a restarted worker to be `unknown`, and on Docker settle an exec answered `running` by observing it |
| `2aba0e5` | v1 additions | The specification states the two additions, the seed labels, and records them under Versioning |
| `ce389a8` | v1 additions | Both interop jobs build the worker from Capsule `3faf05d`, which carries the additions |

## Each fix fails first

| Fix | Run without it | Log |
|---|---|---|
| process group | `process` tests `the_worker_leads_its_own_group_and_close_ends_what_it_left_there`, `a_crashed_worker_takes_its_group_and_not_what_runs_outside_it`, `a_worker_that_breaks_the_protocol_is_terminated_with_its_group`, `a_dropped_bridge_takes_the_workers_group`, with `process_group(0)` removed and the group never signalled: 4 of 4 fail (the worker shares the test's group; the client outlives a close, a crash, a broken protocol and a dropped bridge). The same four failed on the client before `07c14c6` | `fails-first/group-mutation.log` |
| exec client of a killed worker | the Docker interop test with the same mutation: "the exec client outlived the worker" | `fails-first/docker-exec-client-mutation.log` |
| journal off the runtime | the check of `off_runtime.rs`, written against the client before `8c10558` (`fails-first/old_runtime.rs.txt`): "begin ran on" the test's runtime thread | `fails-first/off-runtime-fails-first.log` |
| record and queue as one call | `off_runtime` `a_caller_that_stops_waiting_on_the_log_leaves_the_operation_recorded_and_sent`, with the frame queued by the caller after the log answers: "the frame was never sent" | `fails-first/dropped-caller-mutation.log` |
| v1 additions | the copied fixtures against the Fabric side before `1569bc9`: the golden tests refuse `scope` as `unknown_field` (`observe_exec_valid.json`, `exec_observe_after_worker_loss.json`) and the scenario run stops on the same refusal | `fails-first/additions-fail-first.log` |

## Counters: the journal calls before and after

`off_runtime` `every_log_call_runs_on_the_log_thread_and_each_step_makes_its_pinned_calls` wraps the journal so each call notes its thread, and pins the counts per step. A write is one `IMMEDIATE` transaction that commits before it returns; the journal runs `journal_mode = WAL`, `synchronous = FULL` and, on macOS, `fullfsync = ON`, where SQLite syncs the write-ahead log once per commit, so the writes column is the WAL fsync count. No checkpoint runs in these steps. These are derived counts: fsyncs were not traced, since that needs root on macOS.

| Step | Status | Writes (fsyncs), before | after | Reads, before | after | Log thread calls after | Thread hops after (before: 0) |
|---|---|---:|---:|---:|---:|---:|---:|
| create | succeeded | 2 | 2 | 1 | 1 | 2 | 4 |
| exec | succeeded | 2 | 2 | 1 | 1 | 2 | 4 |
| create with 16 `progress` | succeeded | 18 | 18 | 1 | 1 | 18 | 36 |
| refused before sending | rejected | 1 | 1 | 0 | 0 | 1 | 2 |
| timed out | unknown | 2 | 2 | 1 | 1 | 2 | 4 |
| observe refused (nothing to observe) | succeeded | 0 | 0 | 1 | 1 | 1 | 2 |

Before: `counters/old-client.log`, the same steps against the client before `8c10558` (`counters/old_counts.rs.txt`); every call ran on the test's runtime thread. After: `counters/new-client.log`; every call ran on `fabric-bridge-log`. The durable work did not change; each call adds two thread hops and takes the SQLite commit and its fsync off the async runtime.

## Interop with the Capsule worker

Each run builds the worker from a clean clone with the CI job's steps (`pnpm install --frozen-lockfile`, offline here, and `pnpm --filter "@capsule-sdk/bridge-worker..." run build`) and sets `CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim`, an image OrbStack (Docker 29.4.0) already held; nothing was pulled.

| Log | Worker | Fabric | Result |
|---|---|---|---|
| `interop/95cbbcf-after-group.log` | Capsule `95cbbcf`, the previous CI pin | `07c14c6` | 8 passed, 0 failed |
| `interop/95cbbcf-after-log-thread.log` | Capsule `95cbbcf` | `8c10558` | 8 passed, 0 failed |
| `interop/95cbbcf-after-exec-client-check.log` | Capsule `95cbbcf` | `fa350bd` | 8 passed, 0 failed |
| `interop/77a9fb6-after-additions.log` | Capsule `77a9fb6`, an earlier head that carried the additions | `7c43cb6` | 8 passed, 0 failed |
| `interop/b2d9ff8-docker.log` | Capsule `b2d9ff8`, a later head of `m3/bridge-worker` | `2aba0e5` | 8 passed, 0 failed |
| `interop/3faf05d-pin-docker.log` | Capsule `3faf05d`, the new CI pin | `2aba0e5` | 8 passed, 0 failed |
| `interop/3faf05d-pin-no-image.log` | Capsule `3faf05d`, Docker image unset as on the `bridge-interop` runner | `2aba0e5` | 8 passed, 0 failed (the Docker test reports its skip) |

No container with a `fabric.job` label was left after any run. Capsule `95cbbcf` refuses the new observe `scope` field, so the interop needs a worker with the additions, and `ce389a8` moves both CI pins to `3faf05d`. Capsule rewrote the commits after it on its branch while this work ran (its SEA and CI work); `git diff 3faf05d` of `packages/bridge-worker/src`, `packages/core/src`, `packages/docker/src` and `protocol` against its head at the time of the last run was empty. The pin must stay reachable once Capsule's branch is pushed; if `3faf05d` is rewritten too, the pin moves to the commit that holds the same worker.

The CI job's image, busybox 1.37.0 (`busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec`, the multi-platform index; the amd64 image is 2,211,514 bytes compressed), was not run here: pulling is not allowed on this host. Its digest and platforms were read from Docker Hub's registry API. The Docker test needs `sh -lc`, `sleep`, `cat`, `printf`, `head -c`, `tr` and `echo`, which busybox has; the first CI run of `bridge-docker` is its proof.

## Gates

`gates/run-gates.sh` (the M3 script, unchanged) on `2aba0e5` (`gates/commit.txt`), the last code commit; `ce389a8` changes only `ci.yml`, which `actionlint` passes. Every gate passed on its first run; `gates/summary.txt` holds each exit code, time and load.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` with no features and each of `acp`, `claude`, `codex` | pass |
| `cargo test --workspace --locked -- --skip e2e_` | pass: 1187 passed, 0 failed, 21 ignored (7 new: 4 process group, 2 off-runtime, 1 exec observe) |
| `cargo test -p fabric --test e2e --locked` | pass: 69 passed, 5 ignored |
| `budget::hot_paths` with `FABRIC_BUDGET=count` | pass: every process count equals `budget.toml`; no op was timed |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| `cargo deny check` | pass; no dependency added, the lockfile is unchanged |
| hygiene, budget file and client API fixture checks against `origin/m2/durable-job-foundations`, and their script tests | pass |
| `actionlint` on both workflows | pass |
| TypeScript SDK: install, typecheck, build, `bun test` against this build | pass: 62 passed |
| `fabric-client` `bridge_fixtures`: every file against `manifest.sha256`, the manifest pinned | pass at `af9a31d70699b1e0fc1ffec8ede50eafbb3bd8fcb35d7b1dd99a9826098d7556`, byte-identical to Capsule's copy |

## For the Perf phase

Paths these commits touch, none of them timed here:

- Bridge round trip through the journal: each log call now takes two thread hops to `fabric-bridge-log` and back; the SQLite commits and fsyncs are the same and no longer block the async runtime. The in-memory path records in place as before.
- Spawn: `process_group(0)` adds a `setpgid` in the child before exec.
- `close` and the end of a worker's output: after the worker is reaped, one `kill(-pgid, 0)` finds its group empty; a group with leftovers is polled every 10 ms up to the kill grace.
- `bench.rs` and `scripts/bench-bridge.sh` run unchanged; the journal paths there now go through `LogThread`.

## Open for Capsule

The specification now says a worker keeps out of its process group every process whose end would end or abort a provider effect. Capsule's Docker profile leaves its `docker create`, `start` and `rm` clients to finish because killing one could abort the daemon's work, but starts them in the worker's group: when Fabric ends a lost worker's group, such a client gets the kill grace (2 s by default) and then SIGKILL. The operation in flight is `unknown` in Fabric's journal either way and is found again by observe or its labels, so Fabric's records stay true; starting those clients in a group of their own (`detached: true` in Node's `spawn`) makes them finish as the Capsule worker intends.
