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

`cargo deny check` needs `cargo install cargo-deny --locked`. CI also runs it
once a week, so a new advisory against an unchanged `Cargo.lock` still shows
up. `deny.toml` holds the license, advisory, and source policy. A new
dependency must pass it, and the PR lists the crate, version, last release
date, and license. Dependabot opens a weekly PR for a stale action pin or
crate; it goes through the same gates.

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
