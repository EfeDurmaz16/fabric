# M3 evidence: reconciling Fabric and Capsule

Recorded on 2026-10-29 on `m3/capsule-bridge`, after the deferred items (`../m3-deferred`). The host is an Apple M5 Pro, macOS arm64, 18 cores, with other work running at a load average of 8 to 13. Nothing here was timed: no benchmark, A/B run or stress loop ran, and the time budget ran in count mode. The Perf phase measures these commits on its own.

## What was checked and what changed

| Item | Result |
|---|---|
| Fixtures | `protocol/bridge/v1` here and in Capsule `m3/bridge-worker` are byte-identical (`diff -r` is empty), and both pins name manifest `af9a31d70699b1e0fc1ffec8ede50eafbb3bd8fcb35d7b1dd99a9826098d7556` (`crates/fabric-client/tests/bridge_fixtures.rs` here, `scripts/bridge-fixtures.test.mjs` in Capsule). Nothing in them changed |
| Worker process group | The open item for Capsule in `../m3-deferred` is closed in Capsule: `b333df7` gives `runDocker` an option to start the docker CLI in a process group of its own, and `16d97ed` starts the Docker profile's `create`, `start` and `rm` clients that way, so Fabric's SIGKILL to a lost worker's group no longer reaches a client whose end could abort the daemon's work. An attached `docker exec` client and the reads stay in the group and end with it, which the Docker interop test here still checks. Capsule's checks fail first: the clients shared the worker's group, and a fake `docker create` in flight when the worker's group was killed never finished |
| CI pin | `aa29953` moves both `bridge-interop` and `bridge-docker` to Capsule `a4d597a`, the pushed head of `m3/bridge-worker`. It differs from `16d97ed` only by Capsule's evidence files. `3faf05d`, the previous pin, is also reachable on Capsule's origin |
| Specification | Unchanged. `docs/bridge-protocol.md` already states the rule Capsule now follows; Capsule's pointer page states how its worker meets it |

## Interop, both ways

Fabric's client at `afac988` (`crates/fabric-bridge/tests/capsule_worker.rs`, every test with `--ignored`; this branch has changed only CI and documents since) against the Capsule worker. The test binary was built first and run directly with `env -i`, so each run's PATH is exactly what the table says. `CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim` names an image OrbStack (Docker 29.4.0) already held; nothing was pulled. `interop/run-interop.sh` is the script for the first four rows.

| Log | Worker | PATH | Docker image | Result |
|---|---|---|---|---|
| `interop/16d97ed-node-docker.log` | `dist/bin.js` built in Capsule's worktree at `16d97ed`, through Node 22.23.2 | Node 22, a `docker` link, `/usr/bin`, `/bin` | set | 8 passed, 0 failed |
| `interop/16d97ed-node-no-image.log` | the same | the same | unset, as on the `bridge-interop` runner | 8 passed, 0 failed (the Docker test reports its skip) |
| `interop/16d97ed-sea-no-node-docker.log` | Capsule's single executable (`pnpm bridge:sea` on Node 26.7.0 at `16d97ed`, 143820864 bytes), copied to a fresh directory | a `docker` link, `/usr/bin`, `/bin`: none of `node`, `nodejs`, `bun`, `pnpm`, `npm`, `npx` or `deno` reachable, which the script checks before it runs | set | 8 passed, 0 failed |
| `interop/16d97ed-sea-no-node-no-image.log` | the same | the same | unset | 8 passed, 0 failed |
| `interop/a4d597a-pin-docker.log` | `dist/bin.js` from a clean clone at `a4d597a`, the new pin, built with the CI job's steps (`pnpm install --frozen-lockfile`, offline here, and `pnpm --filter "@capsule-sdk/bridge-worker..." run build`) on Node 22.23.2 | as in the first row | set | 8 passed, 0 failed |
| `interop/a4d597a-pin-no-image.log` | the same | the same | unset | 8 passed, 0 failed |

No container with a `fabric.job` label was left after any run (`interop/summary.txt`). `interop/heads.txt` records both commits, that both worktrees were clean, and the executable's digest.

Capsule's own proofs of the same worker are in Capsule `benchmarks/after/bridge/reconcile`: its gates on Node 22 and 24, the executable's artifact smoke on Node 26 (all 18 shared scenarios, the 8 refused frames, SIGTERM and SIGINT, with an empty PATH), and its Docker smoke on a digest-pinned image already present (all 11 checks).

## Gates

`gates/run-gates.sh` (the M3 script, unchanged but for its output directory) on the head that adds this record before its gate logs (`gates/commit.txt`); `gates/summary.txt` holds each exit code, time and load.
