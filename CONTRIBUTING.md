# Contributing

## Pull request flow

1. Branch from `main` in your own worktree. Nobody commits to `main` directly.
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
```

`cargo deny check` runs on every PR and once a week. `deny.toml` holds the
license, advisory, and source policy. A new dependency must pass it, and the PR
lists the crate, version, last release date, and license.

## Workspace rules

Every crate sets `[lints] workspace = true`. The root `Cargo.toml` holds the
lint policy, and `clippy.toml` holds its test exceptions.

- `unwrap`, `expect`, and `panic!` fail clippy outside tests. Test helpers that
  are not `#[test]` functions return `Result` instead.
- `todo!`, `unimplemented!`, and `dbg!` fail clippy everywhere.
- `missing_docs` also covers integration test files. Start each one with a `//!`
  line that says what it proves.
- Crates set `publish.workspace = true`, which inherits `publish = false`. This
  keeps the code off crates.io and lets path dependencies omit a version.
