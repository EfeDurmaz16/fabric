# ADR 0009: Engineering conventions

Status: accepted, 2026-10-19

## Context

Fabric is small, Rust-only, and will be written fast by several people and agents in parallel. The failure mode of that setup is slop: placeholder modules, narrating comments, stubs that pretend, tests that test the compiler. These conventions exist so a reviewer can reject work on a stated rule rather than taste.

## Decision

### Language and toolchain

- Rust, edition 2024, toolchain pinned in `rust-toolchain.toml` (1.97.1 today). `unsafe_code = "forbid"` at the workspace level.
- Minor versions pinned in `Cargo.toml` (`tokio = "1.54"` style). Every new dependency is justified in the PR body with crate, version, last release date and license. Nothing without a release in the last twelve months.

### Code

- Libraries: `thiserror` error enums per crate, one variant per distinct failure a caller can act on. No `unwrap` or `expect` on fallible paths. No panics. No `println`; `tracing` for logs.
- Binaries: `anyhow` at the edge, `clap` for the CLI, human messages that say what to do next.
- Types make illegal states unrepresentable. A `LeaseState` is an enum, not three optional fields. Parse at boundaries, then trust the type.
- Every public item has a one-sentence doc comment in plain English. Every module's doc explains the invariant the module protects.
- No placeholder modules, no stubs that pretend to work, no TODO-driven skeletons, no dead code, no `utils` grab bags, no re-exports for show, no comments that narrate obvious code. No emoji. No em dash character anywhere: code, docs, commit messages.
- Plain English in docs. Short sentences. Data structures first.

### Tests

- Unit tests for pure logic. Property tests (`proptest`) for state machines and codecs: lease transitions, ledger record codec, chunk list equality.
- Integration tests for IO: store publish and restore in a `tempfile` directory, two nodes on loopback.
- Crash tests for anything that persists: the test re-executes the binary (`assert_cmd`) and sends `SIGKILL` at a named step, then asserts the invariant from the ADR.
- One test per named failure mode. No duplicate tests. Do not test the compiler.

### Git

- Conventional Commits, atomic, imperative, English: `feat(core): ...`, `docs(adr): ...`, `test(store): ...`. One logical change per commit.
- No `Co-Authored-By`, no "Generated with", no AI attribution anywhere.
- Never commit to `main`. Each unit of work is a branch in its own worktree under `/tmp/fabric-wt/<branch>`.
- Stacked PRs: a branch bases on the branch it depends on, not on `main`, and the PR says so. Rebase the stack when the base merges.
- Draft PR first. Body sections: What, Why, Design notes (link ADRs), How to review (file order), Verification (exact commands and results), Dependencies added. Mark ready only after review passes. Merge is the owner's.

### CI gates (to be added with the first code PR)

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo deny check` for licenses and advisories
- A grep that fails the build on an em dash character or an AI trailer in any tracked text file.

All four cargo commands must pass locally before every push, and their summary lines go in the PR body.

### Decisions

- Any choice that a future reader would ask "why" about gets an ADR: Context, Decision, Alternatives considered (with evidence paths), Consequences, Open questions.
- ADRs are numbered, never renumbered, and superseded by a new ADR rather than edited, except to fix errors of fact.

## Alternatives considered

1. **Looser rules, faster start.** The cost shows up as a review that cannot say why something is wrong. Rejected.
2. **`unsafe` allowed in `fabric-net` for performance.** No measured need. `quinn` and `rustls` carry their own audited unsafe. Revisit with a benchmark.
3. **Workspace-level `#![deny(missing_docs)]` from day one.** Yes, from the first code PR. It is the mechanical half of the doc rule.

## Consequences

- Reviews cite a rule, not a preference.
- Every persisted structure in the system has a crash test, or the PR is not ready.
- Dependency growth is slow and visible.

## Open questions

- MSRV policy once there are external users: pin or float? Pin until 1.0.
- Should docs be checked by a prose linter (`vale`) in CI? Try it once; keep it if it catches real things.
