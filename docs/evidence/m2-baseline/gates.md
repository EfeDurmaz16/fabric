# M2 baseline: gates on clean main

Run on 2026-10-27 in the worktree of branch `m2/durable-job-foundations` at `176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5` (origin/main, fetched the same day: no drift from the audited commit), before any M2 change. Machine: Apple M5 Pro, 18 cores, macOS (Darwin 25.6.0, arm64), rustc 1.97.1, Bun and Node 22.23.2, load average about 4 from other work. Raw output of every command is in `gates/`, one log per step, and `gates/summary.txt` has each exit code and duration.

The commands are those of `CONTRIBUTING.md` and `.github/workflows/ci.yml`, run serially from the repository root (the SDK steps from `sdk/typescript`):

| Step | Command | Result |
|---|---|---|
| fmt | `cargo fmt --all --check` | pass |
| clippy | `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| clippy per feature | `cargo clippy -p fabric-harness --all-targets --locked --no-default-features --features "$f" -- -D warnings` for `""`, `acp`, `claude`, `codex` | pass, all four |
| test | `cargo test --workspace --locked -- --skip e2e_` | pass: 928 passed, 0 failed, 3 ignored (the hot-path budget, the store benchmark, a ledger crash child) |
| e2e | `cargo test -p fabric --test e2e --locked` | pass: 46 passed, 0 failed, 1 ignored (the budget) |
| doc | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | pass |
| deny | `cargo deny check` | pass |
| hygiene | `scripts/check-hygiene.sh origin/main` | pass |
| budget change | `scripts/check-budget.sh origin/main` | pass |
| script self-tests | `scripts/test-check-hygiene.sh`, `scripts/test-check-budget.sh`, `scripts/test-check-fixtures.sh` | pass |
| fixtures | `scripts/check-fixtures.sh origin/main` | pass |
| node build | `cargo build -p fabric --locked` | pass |
| SDK | `bun install --frozen-lockfile`, `bun run typecheck`, `bun run build`, `FABRIC_BIN=$PWD/../../target/debug/fabric bun test` | pass: 61 tests, 0 failed |

The time budget (`budget::hot_paths`) is not a gate step here; it is part of the performance baseline in `perf/` (see `perf.md`).

## Expected-failure tests

The commit after this record adds the intended-behavior tests of `defects.md`, each `#[ignore = "<ID>: expected failure until fixed"]`. They do not run in the gates above. Each was run alone with `--ignored` on this branch; the results are in `expected-failures.md`.
