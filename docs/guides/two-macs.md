# Two Macs

Move a job from one Mac to another and back, by hand. This is the walk the 2026-10-21 gate ran with a Codex job. The commands that check it again on today's `main` are in [the pending gate checks](../evidence/two-mac-gate-pending.md).

## Before you start

- The same user name and the same home path on both Macs. Nothing rewrites paths ([ADR 0011](../adr/0011-path-identity.md)).
- `git` on the `PATH` of both.
- One Mac must be able to reach the other's address. In the gate, one Mac sits behind a NAT and dials; the other listens.
- The harness installed and logged in on both. For Claude Code, the same version on both, or `--force` on the move (below).

## Install on both

Build once and copy the binary:

```sh
cargo build --release -p fabric --locked
scp target/release/fabric other-mac:/usr/local/bin/fabric   # any directory on its PATH
cp target/release/fabric /usr/local/bin/fabric
```

## Start a node on each

```sh
fabric serve            # listens on 0.0.0.0:7777; --wire tcp-tls if UDP is blocked
```

Start it from the Mac's desktop session: a Terminal window there, or a launchd user agent. A node started over SSH cannot read the Claude Code login in the Keychain, and `fabric doctor` then says `claude.login: no`.

## Pair

Each Mac prints its id with `fabric node id`.

```sh
# on the Mac that dials: the other's id and an address it reaches today
fabric node add fab1<other-id> <other-address>:7777
# on the other Mac: the first one's id only
fabric node add fab1<first-id>
# back on the first Mac
fabric node ping fab1<other-id>
fabric doctor --peer fab1<other-id>
```

If the dialing Mac changes network and can no longer reach that address, run `fabric node add` again with the new one. Nothing finds a peer that moved.

## Work, suspend, move

```sh
fabric job new --repo ~/code/some-repo --goal "fix the flaky test"   # prints the job id
fabric job run <job>                       # the harness in this terminal
fabric job suspend <job>                   # stops the harness if it runs, snapshots, copies to peers
fabric job move <job> --to fab1<other-id>  # the other Mac owns the job from here
```

You can type these inside a Claude Code session: the harness starts without the markers of the session it was typed in, so it is a session of its own.

Before anything changes hands, `move` prints one line per ignored path, `carry`, `rebuild`, `refuse` or `leave`, and why. A refusal stops the move, and `--force` carries the path as it is. `move` also says when the snapshot holds no harness session, because then the conversation would not continue.

## Resume on the other Mac

```sh
fabric job resume <job>     # restores at the same path, runs setup steps, continues the same session
```

The resume carries the settings the session ran with, not the ones this Mac has configured, and prints them on the `continuing ... session` line. It refuses to start a harness older than the one that last wrote the session; update the harness there and resume again.

To bring the job back, run `fabric job resume <job>` on the first Mac. It pulls the job from the other one.

## When it refuses

- Different Claude Code versions: `doctor` refuses the row for a Claude job; `fabric job move ... --force` (or `fabric job resume ... --force` for a pull) waives it and prints the row.
- A Claude older than 2.1.294 on the target: close every `claude` there before the resume, because its session registry cannot tell which sessions run.
- The target stops answering after `MoveStarted`: the job stays frozen on the source until the target answers, or until a steal with `--force` on another node. There is no `move --abort` yet.
- `fabric job status <job>` and `fabric job log <job>` say where the job is and why, on either Mac. `FABRIC_LOG=debug` shows what a node does.
