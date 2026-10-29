# M3 provider bridge across implementations: gates and interop runs

Run on 2026-10-27 on commit `61285cc` (`gates/commit.txt`), the branch `m3/capsule-bridge` stacked on `m2/durable-job-foundations`, on an Apple M5 Pro (macOS arm64). The Capsule worker is commit `9f5d65e` of Capsule's `m3/bridge-worker`, built from a clean clone with the steps of the CI job `bridge-interop`: pnpm 10.32.1 (the version its `packageManager` field pins), `pnpm install --frozen-lockfile`, `pnpm --filter "@capsule-sdk/bridge-worker..." run build`. The built `dist/bin.js` is byte-identical to the one in the Capsule worktree.

## The seam: Fabric's side against the real worker

`crates/fabric-bridge/tests/capsule_worker.rs`, ignored by default, run with `CAPSULE_BRIDGE_WORKER` set and `CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim` (already on the engine, never pulled). Logs are in `interop/`.

| Run | Result |
| --- | --- |
| Node 22.23.2 (`interop/run.log`) | pass: 7 passed, 0 failed |
| Node 24.19.0 (`interop/run-node24.log`) | pass: 7 passed, 0 failed |
| Mutation: the worker's owner check removed from the built bundle (`interop/mutations.log`) | fails: the refusals test (`Succeeded` where `Rejected` is required) and the scenario replay (`scope_retained` answered with a result) |
| Mutation: the worker's image policy check removed | fails: the refusals test, the scenario replay (`denied_before_effect`) and the Docker test |
| Containers left with a `fabric.job` label after all runs | 0 |

What each test covers is in docs/bridge-protocol.md, "Against the Capsule worker". In short:

- All 15 shared scenarios over the worker's real stdio, every `receive` equal to the fixture except `message` strings and parsed by Fabric's own parser; `pending_destroy_with_locator` stops at its `advance` (4 wire steps run, 5 left to the in-process run).
- Hello: the worker sends the v1 fixture profile whole, lowers `max_in_flight` to 4, and refuses versions 2 to 3 with `unsupported_version`.
- Fixture profile through Fabric's client and a real journal: create, exec, observe (of a create whose reply was lost) and destroy with its own grant, every journal row holding the full `{provider, scope: {account, project}, kind, native_id}` locator.
- Denied and unsupported before effect: eight worker refusals recorded `rejected` with effect `none` and no locator, an exec probe proving the refused create made nothing, and two refusals Fabric makes before writing anything.
- A repeated key is refused by the journal before any frame, with the same operation id or another.
- A worker killed mid-exec leaves the operation `unknown` with the locator its progress reported; job, attempt and generation are unchanged; the restarted worker is never sent it again; a create whose reply was lost is reconciled by `observe`, not created twice.
- Docker on OrbStack (Docker 29.4.0, context `orbstack`): a policy refusal and a missing image leave no container and pull nothing, the container carries the four `fabric.*` labels and `{engine: orbstack}` scope, an artifact is written and read back within `max_output_bytes` (13 bytes whole, 200000 bytes capped at 1024), exit 7 is a completed result, a worker killed during `sleep 20` leaves the container running, the restarted worker is not sent the exec again and one `sleep 20` runs, a destroy without a grant changes nothing and a destroy with its own grant is observed gone.

## Repository gates

Run serially by one script after the interop runs; logs are in `gates/`.

| Gate | Command | Result |
|---|---|---|
| format | `cargo fmt --all --check` | pass |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` with no features and with each of `acp`, `claude`, `codex` alone | pass |
| tests | `cargo test --workspace --locked -- --skip e2e_` | pass: 1051 passed, 0 failed, 15 ignored (the 7 interop tests and the new bench join the earlier 7) |
| end to end | `cargo test -p fabric --test e2e --locked` | pass: 56 passed, 0 failed, 3 ignored |
| docs | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| dependencies | `cargo deny check` | pass; the lockfile is unchanged |
| hygiene | `scripts/check-hygiene.sh origin/m2/durable-job-foundations` and its script test | pass |
| time budget file | `scripts/check-budget.sh origin/m2/durable-job-foundations` and its script test; `budget.toml` is unchanged | pass |
| client API fixtures | `scripts/check-fixtures.sh origin/m2/durable-job-foundations` and its script test; `protocol/` is unchanged | pass |
| workflows | `actionlint` on `ci.yml` and `deny.yml` | pass |
| TypeScript SDK | `bun install --frozen-lockfile`, `bun run typecheck`, `bun run build`, `bun test` with `FABRIC_BIN` set to this build | pass: 62 passed, 0 failed |
| time budget | `budget::hot_paths`, report mode, twice | pass; see perf.md |

The CI job `bridge-interop` has not run on GitHub yet: CI runs on pull requests, and none is open for this branch. Its steps were run locally from a clean clone of the pinned Capsule commit, as above.
