# ADR 0009: Engineering conventions

Status: accepted, 2026-10-19

Amended 2026-10-28: adds the performance rule Efe set on 2026-10-27 (no regression against a fixed anchor, replacing a tolerance per pull request), and lists the CI gates as CI runs them, where the body still said they were to be added. Found while closing M2-25, the time budget that only reported in CI.

## Context

Fabric is small, Rust-only, and will be written fast by several people and agents in parallel. The failure mode of that setup is slop: placeholder modules, narrating comments, stubs that pretend, tests that test the compiler. These conventions exist so a reviewer can reject work on a stated rule rather than taste.

## Decision

### Language and toolchain

- Rust, edition 2024, toolchain pinned in `rust-toolchain.toml` (1.97.1 today). `unsafe_code = "forbid"` at the workspace level.
- Minor versions pinned in `Cargo.toml` (`tokio = "1.54"` style). Every new dependency is justified in the PR body with crate, version, last release date and license. Nothing without a release in the last twelve months. Noted 2026-10-19: `postcard` 1.1.3 was released 2025-07-24, fifteen months before the first code PR, and is kept anyway. Its repository has commits from 2026-09, the 1.x wire format is frozen on purpose, and a frozen codec from a maintained repository is what a ledger that must be readable for years wants. The rule reads the repository, not the crate, when the crate is a stable wire format; `cargo deny` still fails the build on an unmaintained advisory.

### Code

- Libraries: `thiserror` error enums per crate, one variant per distinct failure a caller can act on. No `unwrap` or `expect` on fallible paths. No panics. No `println`; `tracing` for logs.
- Binaries: `anyhow` at the edge, `clap` for the CLI, human messages that say what to do next.
- Types make illegal states unrepresentable. A `LeaseState` is an enum, not three optional fields. Parse at boundaries, then trust the type.
- Every public item has a one-sentence doc comment in plain English. Every module's doc explains the invariant the module protects.
- No placeholder modules, no stubs that pretend to work, no TODO-driven skeletons, no dead code, no `utils` grab bags, no re-exports for show, no comments that narrate obvious code. No emoji. No em dash character anywhere: code, docs, commit messages.
- Plain English in docs. Short sentences. Data structures first.

### Tests

- Unit tests for pure logic. Property tests (`proptest`) for state machines and codecs: lease and move transitions, ledger record codec, chunk list equality.
- Integration tests for IO: store publish and restore in a `tempfile` directory, two nodes on loopback.
- Crash tests for anything that persists: the test re-executes the binary (`assert_cmd`) and sends `SIGKILL` at a named step, then asserts the invariant from the ADR.
- One test per named failure mode. No duplicate tests. Do not test the compiler.

### Git

- Conventional Commits, atomic, imperative, English: `feat(core): ...`, `docs(adr): ...`, `test(store): ...`. One logical change per commit.
- No `Co-Authored-By`, no "Generated with", no AI attribution anywhere.
- Never commit to `main`. Each unit of work is a branch in its own worktree under `/tmp/fabric-wt/<branch>`.
- Stacked PRs: a branch bases on the branch it depends on, not on `main`, and the PR says so. Rebase the stack when the base merges.
- Draft PR first. Body sections: What, Why, Design notes (link ADRs), How to review (file order), Verification (exact commands and results), Dependencies added. Mark ready only after review passes. Merge is the owner's.

### CI gates

CI runs on macOS and Ubuntu; `CONTRIBUTING.md` lists the commands to run before every push.

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`, and `fabric-harness` with each adapter feature alone and with none
- `cargo test --workspace`, then the end-to-end test of two daemons on its own
- the time budget (`budget::hot_paths`), enforced: see Performance
- `cargo doc` with warnings as errors
- `cargo deny check` for licenses, bans, sources and advisories, also weekly
- `scripts/check-hygiene.sh`: no em dash character or AI trailer in any tracked file or commit message
- `scripts/check-fixtures.sh` and `scripts/check-budget.sh`, each with a script test that proves it fires
- the TypeScript SDK: typecheck, build, and its tests against a node built from the commit

The cargo commands must pass locally before every push, and their summary lines go in the PR body.

### Performance

- The bar is no regression against a fixed anchor: `main` at `176622e`, before the durable job campaign. A tolerance per pull request compounds (1.5 times per PR is 5 times after four), so every change is compared with the same anchor, on time and on memory, and the aim is to be faster and smaller than it.
- A slowdown measured on a hot path is a bug to fix, not a trade-off to document, and machine load is not an explanation. A correctness check is never removed or weakened to win time; a cheaper way to make the same check is.
- The evidence is `scripts/bench-ab.sh`: anchor and candidate built once each, run in the same session in alternating order, at least 15 pairs, the median ratio with a 95% bootstrap interval, and counters that do not depend on load (processes started, file flushes, bytes, allocations, resident size). A busy machine takes only counters (`--pairs 0`, `FABRIC_BUDGET=count`); times come from a quiet one.
- CI's guard is the time budget in `crates/fabric/tests/e2e/budget.toml`: on Linux each op is held to the anchor's time on that runner, and on every runner each op's process starts are held to exact counts, by program. A baseline is the anchor's time and never rises; `scripts/check-budget.sh` refuses a change that raises one or loosens the ratio, the grace, the percentile or the runs.

### Decisions

- Any choice that a future reader would ask "why" about gets an ADR: Context, Decision, Alternatives considered (with evidence paths), Consequences, Open questions.
- ADRs are numbered and never renumbered. Until the first release, an ADR may be amended in place: the change gets a dated amendment note under the status line that says what changed, why, and which PR or review found it, and the body is corrected to match. After the first release an ADR is superseded by a new ADR rather than edited, except to fix errors of fact. Amended 2026-10-20 after the review of PR `feat/env`, which found that ADRs 0004 and 0007 were already being amended this way while the rule said otherwise.

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
