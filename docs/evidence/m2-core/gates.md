# M2 core: gates

Run on 2026-10-27 at the tip of the core change (after the performance run), on the tree that is committed with this file. Logs are in `gates/`; `gates/summary.txt` has the exit codes.

| Gate | Command | Result |
|---|---|---|
| format | `cargo fmt --all --check` | pass |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| tests | `cargo test --workspace --locked` (e2e included; ignored tests are the register's expected failures and the two fixture generators) | pass |
| dependencies | `cargo deny check` | pass |
| hygiene | `scripts/check-hygiene.sh 176622e` | pass |
| client API fixtures | `scripts/check-fixtures.sh 176622e` (no fixture of api 1 changed, `required.txt` only grew) | pass |
| time budget file | `scripts/check-budget.sh 176622e` (`budget.toml` unchanged) | pass |
| TypeScript SDK | `bun run typecheck` and `bun test` with `FABRIC_BIN` set, so its e2e suite ran against this node | pass |

Every commit on the branch also passes `cargo fmt --check` (the branch was rebased with a format step on each commit). The commit `feat(fabricd): add the managed authority that owns the journal` carries two `expect(dead_code)` attributes that the next commit removes when the client ops call the authority.
