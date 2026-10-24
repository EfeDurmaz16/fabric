# Contributing

## Pull request flow

1. Branch from `main`, or from the branch your work depends on (stacked PR), in
   your own worktree. Nobody commits to `main` directly.
2. Commit one logical change at a time.
3. Run the gates below, push, and open a draft PR. Fill in every section of the template.
4. Mark the PR ready when CI is green and review findings are resolved.
5. Efe merges.

## Commits

Use Conventional Commits in the imperative mood: `feat(store): add chunk index`.
Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`.
No AI attribution lines and no generated-by footers.

## Review gates

Run these before every push. CI runs the same steps on macOS 15 (arm64) and Ubuntu
with `--locked`, so commit every `Cargo.lock` change.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
scripts/check-hygiene.sh origin/main
scripts/check-budget.sh origin/main
```

The TypeScript SDK in `sdk/typescript` needs Bun, and Node 22 with npm on
`PATH` for its package test. Its end-to-end test runs against a node built
from your tree:

```sh
cargo build -p fabric --locked
cd sdk/typescript
bun install --frozen-lockfile
bun run typecheck
bun run build
FABRIC_BIN="$PWD/../../target/debug/fabric" bun test
```

`scripts/check-hygiene.sh` fails on an em dash or an AI attribution line in any
tracked file or in any commit message on the branch, from any directory in the
repository. `scripts/test-check-hygiene.sh` proves it fires; CI runs it first.

`scripts/check-budget.sh` fails when the time budget changed without a new
`[[change]]` entry (see Time budget); `scripts/test-check-budget.sh` proves it
fires.

`cargo deny check` needs `cargo install cargo-deny --locked`. CI also runs it
once a week, so a new advisory against an unchanged `Cargo.lock` still shows
up. `deny.toml` holds the license, advisory, and source policy. A new
dependency must pass it, and the PR lists the crate, version, last release
date, and license. Dependabot opens a weekly PR for a stale action pin or
crate; it goes through the same gates.

## Time budget

A hot path that gets about 1.5 times slower is a bug (issue #26). The
ignored test `budget::hot_paths` in `crates/fabric/tests/e2e/budget.rs`
times the hot paths on the end-to-end fixture of two nodes over QUIC:
suspend, move, resume and pull of a job under each harness (ACP, Claude,
Codex), and a daemon's cold start and restart. Every program the node's
facts probe, but git, is a fake that answers after 50 ms; git is the
machine's own. It also counts the git and ps processes each op starts.
`crates/fabric/tests/e2e/budget.toml` holds the limits: the ratio, the
grace, the runs per batch, each platform's baselines, the exact process
counts, and the `[[change]]` entries that record why the budget changed.
CI runs it after the end-to-end step on both runners and writes its table
to the step summary.

Run it on a quiet machine, since it measures wall time. It takes a few
minutes:

```sh
cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
FABRIC_BUDGET=enforce cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
```

The first command only reports times; the second fails on them. An op is
over its limit when its 25th percentile of 7 runs is above both 1.3 times
its baseline and its baseline plus 50 ms, and it fails when it is over
again in a second batch of 7. A process count fails in both modes when it
differs from the budget, higher or lower: fewer starts can mean a check was
dropped. A platform with no baselines, as `linux-x86_64` until CI produces
numbers, only reports its times.

To accept a slowdown, or a new process count:

1. Say in the PR why the op must cost more, with the table before and after.
2. Set the op's baseline in `budget.toml` to the new 25th percentile from the
   table, or set its counts.
3. Add a `[[change]]` entry at the end of `budget.toml`, with
   `date = "YYYY-MM-DD"` and a `why` that names the PR. Entries are never
   edited or removed. `scripts/check-budget.sh` fails a change to the
   budget without a new one.

To give a platform its baselines, copy the 25th percentile column of a CI
run on main from the step summary, with a `[[change]]` entry that names the
run.

## Workspace rules

Every crate sets `[lints] workspace = true`. The root `Cargo.toml` holds the
lint policy, and `clippy.toml` holds its test exceptions and disallowed methods.

- `unwrap`, `expect`, `panic!`, `println!` and `eprintln!` fail clippy outside
  tests. Inside a `#[cfg(test)]` module every function may use them. In
  `tests/` files only `#[test]` functions may; a helper there returns `Result`
  and writes nothing.
- `todo!`, `unimplemented!`, `unreachable!` and `dbg!` fail clippy everywhere.
- `std::process::exit` fails clippy everywhere, `main` included. Return
  `std::process::ExitCode` from `main` so destructors run.
- Every `#[allow]` and `#[expect]` carries a `reason = "..."`.
- `missing_docs` also covers integration test files. Start each one with a `//!`
  line that says what it proves.
- Crates set `publish.workspace = true`, which inherits `publish = false`. This
  keeps the code off crates.io and lets path dependencies omit a version.
