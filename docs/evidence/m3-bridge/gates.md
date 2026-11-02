# M3 provider bridge, Fabric side: gates

Run on 2026-10-27, serially, on commit `1c26c1f` (`gates/commit.txt`), the branch `m3/capsule-bridge` stacked on `m2/durable-job-foundations`. Logs are in `gates/`.

| Gate | Command | Result |
|---|---|---|
| format | `cargo fmt --all --check` | pass |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` with no features and with each of `acp`, `claude`, `codex` alone | pass |
| tests | `cargo test --workspace --locked -- --skip e2e_` | pass: 1051 passed, 0 failed, 7 ignored |
| end to end | `cargo test -p fabric --test e2e --locked` | pass: 56 passed, 0 failed, 3 ignored |
| docs | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| dependencies | `cargo deny check` | pass; no crate entered the lockfile but `fabric-bridge` itself |
| hygiene | `scripts/check-hygiene.sh origin/m2/durable-job-foundations` and its script test | pass |
| time budget file | `scripts/check-budget.sh origin/m2/durable-job-foundations` and its script test; `budget.toml` is unchanged | pass |
| client API fixtures | `scripts/check-fixtures.sh origin/m2/durable-job-foundations` and its script test; `protocol/fixtures` is unchanged | pass |
| TypeScript SDK | `bun install --frozen-lockfile`, `bun run typecheck`, `bun run build`, `bun test` with `FABRIC_BIN` set to this build | pass: 62 passed, 0 failed |

The new ignored tests are `make_v2_fixture` (the journal fixture generator) and `bridge_latencies` (the bridge bench, `perf.md`). The time budget ran in report mode in `budget/` (`perf.md`).
