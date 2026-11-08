# Install

There is no release, no Homebrew formula and no package yet. You build the binary from source. A release train with unsigned binaries for three platforms and a Homebrew tap is on the [roadmap](../roadmap.md) (PRs 3 and 14).

## Needs

- macOS or Linux. A move between machines has run on two real Macs once, on 2026-10-21, before M2 to M4 merged; Linux runs in CI.
- A C compiler and `git`. On a Mac, the Xcode Command Line Tools give both: `xcode-select --install`. The store captures and restores git state with `git`, so it must be on the `PATH`.
- `rustup`, which installs Rust. `rust-toolchain.toml` pins Rust 1.97.1, and `rustup` fetches that version on the first build. Without `rustup`, install it as [rustup.rs](https://rustup.rs) says, then open a new terminal so `cargo` is on the `PATH`:

  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- To do real work: Claude Code or Codex installed and logged in, or any agent that speaks the Agent Client Protocol.

## Build

```sh
git clone https://github.com/EfeDurmaz16/fabric
cd fabric
cargo build --release -p fabric --locked
```

The binary is `target/release/fabric`. Copy it to a directory on your `PATH` that you can write without `sudo`, such as `~/.local/bin`:

```sh
mkdir -p ~/.local/bin
cp target/release/fabric ~/.local/bin/fabric
fabric --version
```

If `fabric` is not found, that directory is not on your `PATH` yet. Add it for zsh, the default shell on a Mac, and open a new terminal:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
```

`/usr/local/bin` works too, but on a Mac without Homebrew it belongs to `root`, so the copy needs `sudo cp`.

On the walks of `main` recorded in [the evidence](../evidence/usable-from-main/walk.md), the release build took 1 minute 28 seconds on 2026-11-03 and 2 minutes 10 seconds on 2026-11-08, on an Apple silicon Mac with the toolchain already installed. A first build also downloads the toolchain and every crate.

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
