# Fabric

A job is the unit of development work. Storage, compute, environment and harness stay decoupled, so a job started under Claude Code on one machine can be suspended, resumed on another machine with its dependencies, and continued by a different agent.

Private repository. Working name; see [ADR 0010](docs/adr/0010-name-and-scope-boundaries.md) on the name collision.

## Status, honestly

Core types only. Nothing suspends or resumes yet.

| What | State |
|---|---|
| Architecture page and glossary | Written: [docs/architecture.md](docs/architecture.md), [docs/glossary.md](docs/glossary.md) |
| ADRs 0001 to 0013 | Written, see below; amended 2026-10-19 after the step 1 research; 0013 records the harness trait as built |
| `crates/fabric-core` | Ids, paths, the job ledger with its record codec, the lease and the cooperative move protocol, snapshot manifests and the environment spec. Pure: no IO, no clock, no socket. Property-tested against the ADR 0003 invariants. |
| `crates/fabric-harness` | The `Harness` trait and three adapters: Claude Code, Codex and any Agent Client Protocol agent. Captures a session into a redacted state tree with a manifest, restores it all or nothing under the same workspace path and home, never over state that would lose bytes, launches the harness in a new process group with typed flags and without the markers of a Claude Code or Codex session Fabric runs under. Pre-seeds Claude's workspace trust only when Claude would not already trust the workspace, and leaves it to Claude while one runs or when `~/.claude.json` is missing or a symlink. A hard-coded denylist keeps credentials and host state out of the tree. Crash-tested with `SIGKILL` after every staged and placed Claude file, after a pending departure record, and in a Codex restore. |
| Every other crate in the layout | Does not exist yet |
| CI | fmt, clippy, test, doc, `cargo deny` and the hygiene script on macOS and Ubuntu |

Nothing here suspends or resumes anything. Do not install it.

## First gate

One job, adopted with `fabric job init` from a checkout where Claude Code is already running on a MacBook Pro, pushed to a MacBook Air with `fabric job move --to`, resumed there at the same absolute path, dependencies included when their carry key matches and rebuilt when it does not, safe under `kill -9` at every step. Two nodes, same user and home, same OS and architecture, different tool versions, explicit commands only. When that passes, the next stages are the Codex adapter, the ACP adapter, and the sleep watcher.

## Decisions

| ADR | Title |
|---|---|
| [0001](docs/adr/0001-job-as-unit-of-work.md) | The job is the unit of work |
| [0002](docs/adr/0002-job-ledger-and-record.md) | Job ledger and record |
| [0003](docs/adr/0003-exactly-one-live-copy.md) | Exactly one live copy |
| [0004](docs/adr/0004-content-addressed-snapshots.md) | Content-addressed snapshots |
| [0005](docs/adr/0005-transport-and-node-identity.md) | Transport and node identity |
| [0006](docs/adr/0006-harness-adapters.md) | Harness adapters |
| [0007](docs/adr/0007-environment-spec.md) | Environment spec |
| [0008](docs/adr/0008-placement-and-triggers.md) | Placement and triggers |
| [0009](docs/adr/0009-engineering-conventions.md) | Engineering conventions |
| [0010](docs/adr/0010-name-and-scope-boundaries.md) | Name and scope boundaries |
| [0011](docs/adr/0011-path-identity.md) | Path identity |
| [0012](docs/adr/0012-dependency-carry.md) | Dependency carry |
| [0013](docs/adr/0013-harness-trait-as-built.md) | Harness trait as built |

Start with the architecture page, then read the ADRs in order. ADR 0003 deserves the most attention; it is the one that protects the user's work. The research notes the ADRs cite are private and live outside this repository; the architecture page says what they are.

## Building

Rust 1.97.1, pinned in `rust-toolchain.toml`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `cargo deny check` and `scripts/check-hygiene.sh origin/main` complete the set; see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
