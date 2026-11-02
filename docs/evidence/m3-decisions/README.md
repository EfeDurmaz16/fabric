# M3 evidence: the decisions of 2026-10-29

Recorded on 2026-10-29. The host is an Apple M5 Pro, macOS arm64, 18 cores, with other work running at a load average of 6 to 13. No timing was measured: the time budget ran in count mode, and nothing here is a benchmark.

## What changed

Efe decided on 2026-10-29 that an exec `timeout_ms` or `max_output_bytes` of 0 is invalid and fails closed, that the `provider_operations` table (journal schema version 2) is approved, and that protocol v1, which has only been on draft pull requests, takes additive or corrective changes itself rather than through a v2.

| Commit | Change |
|---|---|
| `e01fee6` | merges `m2/durable-job-foundations` (`31577c4`) into this branch. No file conflicted; the merged tree builds |
| `776403a` | the worker-side parser refuses an exec `timeout_ms` or `max_output_bytes` of 0 as `invalid`/`limit`, so the client refuses such a request before writing it and records it `rejected` |
| `53c24a5` | the specification rules 0 invalid; the reject cases `request_exec_timeout_zero` and `request_exec_output_bytes_zero`; the regenerated `manifest.sha256`, pinned at `5390b6b5c520e7d2a7a4fb46527f32e440c7f346eb5bc31e1ab85803e96cd63e` here and in Capsule; the Versioning section says the growth-only rule binds from v1's first release |
| `ba950db` | ADR 0020, ADR 0021 and docs/managed-jobs.md record the approval of `provider_operations` |
| `6d96c1c` | the `bridge-interop` CI job builds the worker from Capsule `95cbbcf`, the head of its `m3/bridge-worker` |

Capsule's side: `546e9f7` copies the two cases and the manifest byte for byte and moves its pin, `105c2fe` updates its pointer page, `95cbbcf` records its gates. Capsule's worker already refused both values; its source did not change. Its `m3/bridge-worker` already held `m1/correctness-perf-ci` at `ff0b42a`, so there was nothing to merge there.

## Each new check fails first

- `golden` `every_case_is_accepted_or_refused_as_it_says_and_writes_back_unchanged`, with the two cases added and the parser unchanged: `request_exec_output_bytes_zero.json: accepted`.
- `process` `fabric_refuses_before_sending_what_the_profile_or_the_cap_refuses`, with the parser's two `bound` calls put back to `int`: operation 18 (a `timeout_ms` of 0) is sent instead of refused. With the parser as committed it passes.
- Capsule `protocol.test.ts`, with the lower bound of both fields lowered from 1 to 0 in `validate.ts`: both new case tests fail (Capsule `benchmarks/after/bridge/zero-bounds/gates.txt`).

## Interop with the Capsule worker

| Run | Worker | Result |
|---|---|---|
| `gates/interop-pin.log` | Capsule `95cbbcf` from a fresh clone, built with the CI job's steps (`pnpm install --frozen-lockfile`, `pnpm --filter "@capsule-sdk/bridge-worker..." run build`), Docker image unset as on the runner | 8 passed, 0 failed (the Docker test reports its skip) |
| `gates/interop-full.log` | the same build, with `CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim` on OrbStack, Docker 29.4.0; the image was already present and nothing was pulled | 8 passed, 0 failed; no container left with a `fabric.job` label |

## Gates

`gates/run-gates.sh` on `6d96c1c` (`gates/commit.txt`). `gates/summary.txt` holds each exit code, time and load.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` per feature | pass |
| `cargo test --workspace --locked -- --skip e2e_` | pass on the rerun with `--no-fail-fast` (`test-rerun.log`): 1180 passed, 0 failed, 21 ignored. See below for the first run |
| `cargo test -p fabric --test e2e --locked` | pass: 69 passed, 5 ignored |
| `budget::hot_paths` with `FABRIC_BUDGET=count` | pass: every process count equals `budget.toml`; no op was timed |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| `cargo deny check` | pass |
| hygiene, budget file and client API fixture checks against `origin/m2/durable-job-foundations`, and their script tests | pass |
| `actionlint` on both workflows | pass |
| TypeScript SDK: install, typecheck, build, `bun test` against this build | pass: 62 passed |

The first workspace run (`test.log`) failed one test in code this branch does not change: `fabricd` `local::tests::a_stop_reaches_the_group_after_its_leader_exited` asserted `the background process outlived the stop`. The file comes from M2 unchanged. Run alone, it failed 1 of 38 times on this host, and the rerun passed it. The orphaned `sleep` it signals is reparented to launchd, so the check right after the stop can find it before launchd reaps it; that reading is not confirmed, and the test belongs to the M2 branch.
