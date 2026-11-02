# M2 integrated: gates

Run on 2026-10-27 by `gates/run-gates.sh`, serially, on commit `5bfff12` (`gates/commit.txt`), the integrated branch with every owner branch, the integrator fixes and the launch fix. Logs are in `gates/`; `gates/summary.txt` has each command, its exit code and its time.

| Gate | Command | Result |
|---|---|---|
| format | `cargo fmt --all --check` | pass |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` with no features and with each of `acp`, `claude`, `codex` alone | pass |
| tests | `cargo test --workspace --locked`, e2e included | pass: 1062 passed, 0 failed, 6 ignored |
| ignored tests | `cargo test --workspace --locked -- --list --ignored` | the 6 are the two fixture generators, two benchmarks, the time budget (its own CI step) and a ledger crash test's child; no expected-failure marker is left |
| docs | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| dependencies | `cargo deny check` | pass |
| hygiene | `scripts/check-hygiene.sh origin/main` and its script test | pass |
| time budget file | `scripts/check-budget.sh origin/main` and its script test (one new `[[change]]` entry) | pass |
| client API fixtures | `scripts/check-fixtures.sh origin/main` and its script test (two new fixtures, `required.txt` only grew) | pass |
| TypeScript SDK | `bun install --frozen-lockfile`, `bun run typecheck`, `bun run build`, `bun test` with `FABRIC_BIN` set to this build | pass: 62 passed, 0 skipped |

The time budget itself ran three times in report mode in `perf/` (`perf.md`): every op within its limit and every process count equal to `budget.toml`.
