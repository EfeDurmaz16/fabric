# Install

There is no release, no Homebrew formula and no package yet. You build the binary from source. A release train with unsigned binaries for three platforms and a Homebrew tap is on the [roadmap](../roadmap.md) (PRs 3 and 14).

## Needs

- macOS or Linux. Two real Macs are the only pair a move has been proved on; Linux runs in CI.
- Rust 1.97.1. `rust-toolchain.toml` pins it, so `rustup` installs it on the first build.
- `git` on the `PATH`. The store captures and restores git state with it.
- To do real work: Claude Code or Codex installed and logged in, or any agent that speaks the Agent Client Protocol.

## Build

```sh
git clone https://github.com/EfeDurmaz16/fabric
cd fabric
cargo build --release -p fabric --locked
```

The binary is `target/release/fabric`. Copy it to a directory on your `PATH`:

```sh
cp target/release/fabric /usr/local/bin/fabric
fabric --version
```

On the walk of `main` recorded in [the evidence](../evidence/usable-from-main/walk.md), the release build took 1 minute 28 seconds on an Apple silicon Mac with the toolchain already installed.

## Check it

Every command but `serve` talks to a running node, so start one first. `FABRIC_HOME` names the node's state directory (`~/.fabric` without it). Keep its path short: the control socket `fabric.sock` lives there, and a Unix socket path has a small limit, which `serve` reports before it listens.

```sh
export FABRIC_HOME=/tmp/fabric-try
fabric serve --listen 127.0.0.1:7777     # leave it running; Ctrl-C stops it
```

In another terminal, with the same `FABRIC_HOME`:

```sh
fabric node id
fabric doctor
```

`doctor` prints the node's facts: harness versions, whether `claude` and `codex` are logged in, toolchains. A command that finds no node exits 80 and says to run `fabric serve`. Next: [your first move](first-move.md).
