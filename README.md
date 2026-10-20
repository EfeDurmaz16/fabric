# Fabric

A job is the unit of development work. Storage, compute, environment and harness stay decoupled, so a job started under Claude Code on one machine can be suspended, resumed on another machine with its dependencies, and continued by a different agent.

Private repository. Working name; see [ADR 0010](docs/adr/0010-name-and-scope-boundaries.md) on the name collision.

## Status, honestly

A job can move between two of your Macs by hand: made from a repository, run under a harness, suspended, moved, resumed on the other Mac with its uncommitted work and its harness session, and moved back, safe under `kill -9` of either node at every step the end-to-end test names. Nothing has run on the real two-Mac gate yet; the test runs two nodes on one machine. Only the fake ACP agent has gone through a full move; Claude Code and Codex are wired the same way and still need their live run (ADR 0015).

| What | State |
|---|---|
| Architecture page and glossary | Written: [docs/architecture.md](docs/architecture.md), [docs/glossary.md](docs/glossary.md) |
| ADRs 0001 to 0016 | Written, see below; amended 2026-10-19 after the step 1 research; 0013 to 0016 record the network protocol, the store, the harness trait, and the node and CLI as built |
| `crates/fabric-core` | Ids, paths, the job ledger with its record codec, the lease and the cooperative move protocol, snapshot manifests and the environment spec. Pure: no IO, no clock, no socket. Property-tested against the ADR 0003 invariants. |
| `crates/fabric-store` | Content-addressed objects (blake3, zstd, FastCDC above 8 MiB), the SQLite catalog with refcounts and a graced gc, workspace capture (git state as two private commits and a bundle, ignored files by class, secrets denied) and restore into an empty directory through a staging rename. Crash-tested with `SIGKILL` at every publish and restore checkpoint. |
| `crates/fabric-net` | Node key, pinned TLS with no CA, QUIC and TCP+TLS transports on one port behind one endpoint, framed and typed RPC with deadlines, paged ledger records, verified zstd chunk transfer, the facts doctor compares, the peer book. See [docs/net.md](docs/net.md). |
| `crates/fabric-harness` | The `Harness` trait and three adapters: Claude Code, Codex and any Agent Client Protocol agent. Captures a session into a redacted state tree, restores it all or nothing under the same workspace path and home, launches the harness in its own process group. |
| `crates/fabric-env` | `EnvSpec` detection from the files a workspace already has and from `.fabric/env.toml`, the secret policy, and the materializer that runs setup steps only when a cache is missing or torn. |
| `crates/fabricd` | The node: one process per state directory holding the store, one ledger file per job, the network service and the control socket. Suspend, the push move, the pull resume, the steal behind `--force`, doctor, and recovery after a restart. See [ADR 0016](docs/adr/0016-node-and-cli-as-built.md). |
| `crates/fabric` | The `fabric` command, and the end-to-end test: two daemons over loopback on QUIC and on TCP+TLS, a job moved and back, the move broken at five named points by `SIGKILL` of either node or a lost or repeated answer. |
| CI | fmt, clippy, test, the end-to-end test, doc, `cargo deny` and the hygiene script on macOS and Ubuntu |

## Two Macs, by hand

Both Macs need the same user name and the same home path (ADR 0011), and `git` on the `PATH` (the store restores git state with it). On the gate, the Pro is behind a NAT and the Air has an address the Pro can reach, so the Pro dials and the Air listens.

Build once, on the Pro, and copy the binary:

```sh
cargo build --release -p fabric
scp target/release/fabric air:/usr/local/bin/fabric   # any directory on the Air's PATH
cp target/release/fabric /usr/local/bin/fabric
```

Start a node on each Mac and leave it running (a terminal tab is enough):

```sh
fabric serve            # listens on 0.0.0.0:7777; --wire tcp-tls if UDP is blocked
```

Pair them. Each prints its id with `fabric node id`.

```sh
# on the Pro: the Air's id and the address the Pro reaches it at
fabric node add fab1<air-id> <air-address>:7777
# on the Air: the Pro's id only; the Air never dials the Pro
fabric node add fab1<pro-id>
# on the Pro
fabric node ping fab1<air-id>
fabric doctor --peer fab1<air-id>
```

Make a job and work on it on the Pro:

```sh
fabric job new --repo ~/oss/some-repo --goal "fix the flaky test"   # prints the job id
fabric job run <job>        # Claude Code in this terminal, at ~/fabric/jobs/<job>/work
```

Exit the harness, then move the job:

```sh
fabric job suspend <job>                 # stops the harness if it still runs, snapshots, copies to the Air
fabric job move <job> --to fab1<air-id>  # the Air owns the job from here; the Pro's copy never runs again
```

On the Air:

```sh
fabric job resume <job>     # restores at the same path, runs setup steps, continues the same session
```

To bring it back, on the Pro, `fabric job resume <job>` pulls it from the Air. `fabric job status <job>` and `fabric job log <job>` say where the job is and why on either Mac. `FABRIC_LOG=debug` shows what a node does.

On the gate as measured, the Macs run different Claude Code versions, which doctor refuses for a Claude job; `fabric job move ... --force` (and `fabric job resume ... --force` for a pull) waives that row and prints it. Close every `claude` on the Air before a resume there: its session registry is too old to tell which sessions run (ADR 0015).

## Known limits

- No triggers: nothing suspends on sleep or lid close; every move is a command you type (ADR 0008).
- Two nodes and no witness: a move whose target vanishes after `MoveStarted` freezes the job on the source until the target answers, or until a steal with `--force` on another node (ADR 0003). There is no `move --abort` yet.
- No phone or cloud client, and no third node.
- Same user and home on both nodes; nothing rewrites paths (ADR 0011).
- `fabric job init`, which adopts a checkout in place, is not built; `job new` clones the repository into `~/fabric/jobs/<id>/work`.
- No `fabric orphan export` and no fork rule: an orphan workspace is reported and left in place, and a ledger that forked after a steal stops catching up and says so in the log.
- The node must run: every job command talks to it, and it supervises the harnesses it launched.

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
| [0013](docs/adr/0013-network-protocol-as-built.md) | The network protocol as built |
| [0014](docs/adr/0014-store-as-built.md) | The store as built |
| [0015](docs/adr/0015-harness-trait-as-built.md) | The harness trait as built |
| [0016](docs/adr/0016-node-and-cli-as-built.md) | The node and the CLI as built |

Start with the architecture page, then read the ADRs in order. ADR 0003 deserves the most attention; it is the one that protects the user's work. The research notes the ADRs cite are private and live outside this repository; the architecture page says what they are.

## Building

Rust 1.97.1, pinned in `rust-toolchain.toml`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p fabric     # target/release/fabric
```

`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `cargo deny check` and `scripts/check-hygiene.sh origin/main` complete the set; see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
