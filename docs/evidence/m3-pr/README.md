# M3 PR evidence: interop with the final Capsule worker, gates

Recorded on 2026-10-28, when the two M3 PRs were opened. The host is an Apple M5 Pro, macOS arm64, with other work running at a load average of 13 to 21 on 18 cores.

## What changed since docs/evidence/m3-review

- Capsule's `m3/bridge-worker` merged the moved `m1/correctness-perf-ci` twice (Capsule `6f5a8d0`, then `9e80135` after M1 moved to `ff0b42a`). Its worker code did not change after the review fixes; the merges brought M1's core and Docker changes under it. `1e2e1cf`, the branch head, adds only evidence files after `9e80135`.
- `0b6b3a3`: the shared scenario replay in `crates/fabric-bridge/tests/capsule_worker.rs` writes consecutive `send` steps in one write, as Capsule's own runner does. Before, `shutdown_reports_unresolved` failed about one run in 40: on the fixture's virtual clock, an `observe` that reached the worker in its own pipe read advanced before the `shutdown` drain began, so step 5 saw 4 observations against the fixture's 2.
- The `bridge-interop` CI job now builds the worker from Capsule `1e2e1cf`, the head of its branch, instead of `9f5d65e`, which predates the worker's review fixes.

## Interop with the Capsule worker

| Run | Worker | Result |
|---|---|---|
| `interop-pin.log` | Capsule `9e80135` (the worker of `1e2e1cf`) from a fresh clone, built with the CI job's steps (`pnpm install --frozen-lockfile`, `pnpm --filter "@capsule-sdk/bridge-worker..." run build`), Docker image unset as on the runner | 8 passed, 0 failed (the Docker test reports its skip) |
| `interop-full.log` | the same build, with `CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim` on OrbStack, Docker 29.4.0; the image was already present and nothing was pulled | 8 passed, 0 failed; no container left with a `fabric.job` label |
| `scenario-repeats.txt` | the worker built at Capsule `6f5a8d0` | old replay (`b6b2535`) 39 of 40 pass; new replay (`0b6b3a3`) 100 of 100 pass; the new replay against the `9e80135` build, 30 of 30 more |

## Gates

`gates/run-gates.sh` on `e8b5b1b` (`gates/commit.txt`), with the time budget added as its last step. `gates/summary.txt` holds each exit code and time.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` per feature | pass |
| `cargo test --workspace --locked -- --skip e2e_` | pass on the third run (`test-rerun2.log`): 1058 passed, 0 failed, 16 ignored. See below for the first two |
| `cargo test -p fabric --test e2e --locked` | pass on the second run (`e2e-rerun.log`): 56 passed, 3 ignored. See below for the first |
| `budget::hot_paths` | pass (`budget.log`, report mode; at this load every op reads over its quiet-machine limit, and process counts equal `budget.toml`) |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| `cargo deny check` | pass |
| hygiene, budget file and client API fixture checks against `origin/m2/durable-job-foundations`, and their script tests | pass |
| `actionlint` on both workflows | pass |
| TypeScript SDK: install, typecheck, build, `bun test` against this build | pass: 62 passed |

The failed runs, kept as logs, are load or first-launch effects on this host in code this PR does not touch, or in tests that pass on a warm binary:

- `test.log`: `fabric-harness` `crash` `a_restore_killed_after_any_placed_file_leaves_whole_files_and_converges` failed once; `cargo test` stops at the first failing binary. The `fabric-harness` crate has no change on this branch, and the test passes alone (5 of 5 in its file).
- `e2e.log`: `client::e2e_doctor_measures_in_the_clis_environment_not_the_nodes` printed no `pnpm.*` lines once. The `fabric` crate has no change on this branch, and the test passes alone and in `e2e-rerun.log`.
- `test-rerun.log`: 18 of 22 `fabric-bridge` `process` tests hit `HandshakeTimeout` (5 s) together, right after the test double binary was relinked by an alternating `-p fabric-bridge` build; macOS checks a new binary on its first launch. The same file passes alone in 2.6 s, in `test-rerun2.log`, and on both CI runners.

GitHub CI on PR #32 ran `check (ubuntu-latest)`, `check (macos-15)`, `bridge interop (ubuntu-latest)`, `hygiene`, both `deny` jobs and both `sdk typescript` jobs green on `e8b5b1b`.
