# Fabric

A job is the unit of development work. Storage, compute, environment and harness stay decoupled, so a job started under Claude Code on one machine can be suspended, resumed on another machine with its dependencies, and continued by a different agent.

Private repository. Working name; see [ADR 0010](docs/adr/0010-name-and-scope-boundaries.md) on the name collision.

## Status, honestly

Design only. There is no working code.

| What | State |
|---|---|
| Architecture page and glossary | Written: [docs/architecture.md](docs/architecture.md), [docs/glossary.md](docs/glossary.md) |
| ADRs 0001 to 0010 | Written, see below |
| `crates/fabric-core` | A cargo template with an `add` function. It will be replaced in the core stage. |
| Every other crate in the layout | Does not exist yet |
| CI | Not configured yet |

Nothing here suspends or resumes anything. Do not install it.

## First gate

One job, started under Claude Code on a MacBook Pro, suspended, resumed on a MacBook Air over the network, `node_modules` included, safe under `kill -9` at every step. Two nodes, same OS and architecture, manual suspend and resume. When that passes, the next stages are the Codex adapter, the ACP adapter, and the sleep watcher.

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

Start with the architecture page, then read the ADRs in order. ADR 0003 deserves the most attention; it is the one that protects the user's work.

## Building

Rust 1.97.1, pinned in `rust-toolchain.toml`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These pass today because there is almost nothing to build.

## License

MIT. See [LICENSE](LICENSE).
