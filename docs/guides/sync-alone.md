# Sync alone

`fabric-sync` moves a working tree between machines without a node, a pairing or a job: push it to a directory or over SSH, pull it onto another machine. It carries what `git push` leaves behind, staged and unstaged changes, untracked files and the caches such as `node_modules`, and it leaves `.env` and every other secret-shaped untracked or ignored file where it is. It is the store half of Fabric used on its own, so a tree it moves is captured and restored by the same code a suspend and a resume run.

The name is a working name, like Fabric's; a rename changes it in one place.

## What travels

A push captures the tree with the store's capture ([what moves](../start/what-moves.md) has the full table):

- Commits, branches, the index and the working tree: a git bundle plus two private commits, one for the index and one for the working tree. Your own refs and index are not touched.
- Untracked files, and ignored files that are nobody's cache: your work.
- Caches by class: `node_modules`, `.pnpm-store`, `.venv`, `target` beside a `Cargo.toml`, `dist` and `build` beside a `package.json` or a Python project, `.next`, `.turbo`, `__pycache__` and the other tool caches. A sync has no node to measure a toolchain, so every cache travels as your files, byte for byte, modes and times included.
- Left behind and named: `.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa` and the rest of the secret list, wherever they lie, every pattern of `.fabricignore`, and every `secret_patterns` entry of `.fabric/env.toml`. A cache that file declares under `[[cache]]` keeps its class, as a suspend keeps it. A secret that is tracked or stashed is in git history already; it travels with the history, and the push names it.

A pull onto another platform warns about each cache with native Node addons (`*.node`) built for the first one.

## Install

```sh
cargo build --release -p fabric-sync --locked
cp target/release/fabric-sync /usr/local/bin/      # any directory on the PATH
```

For an SSH target, the same binary must be on the remote machine's `PATH` (or named by `FABRIC_SYNC_REMOTE`): `ssh` starts `fabric-sync serve` there for each sync and it exits after it. There is no daemon.

## A first sync

On the first machine, from the top of the tree:

```sh
fabric-sync push box:sync
```

On the second machine:

```sh
fabric-sync pull box:sync --name app --tree ~/code/app
```

The target is a directory on the remote machine, here `~/sync` on `box`, that holds a store and one head per tree name. The name is the tree's directory name unless `--name` gives another. Standard output carries the snapshot id and nothing else; the summary is on standard error, here for the second push of the bench tree below:

```text
pushed app to box:sync: 2313 files, sent 0.2 MiB of 5.3 MiB (2 of 2012 objects, 99.9% already there)
left behind: .env
```

A second push sends only the objects the target does not have. In the bench tree (300 tracked files, 2,000 files in `node_modules`, a `dist`), after ten tracked files and one dependency changed, the second push sent 2 of 2,012 objects and 3% of the tree's bytes; most of what it sent was the snapshot's manifest, which lists every file (below). `--json` prints the counters as one document:

```json
{"outcome":"pushed","sync":{"bytes_sent":174325,"files":2313,"hit_ratio":0.9990059642147118,"left_behind":[".env"],"name":"app","objects_needed":2012,"objects_sent":2,"snapshot":"<SNAPSHOT>","snapshot_bytes":5543843,"snapshots":1,"tracked_secrets":[],"warnings":[]}}
```

## Targets

| Target | Means |
|---|---|
| `/Volumes/Backup/sync`, `./sync` | A directory on this machine, made by the first push |
| `box:sync`, `me@box:~/sync` | `~/sync` on `box`, over `ssh`, as `scp` reads it |
| `ssh://me@box:2222/srv/sync` | `/srv/sync` on `box`, port 2222 |

A word whose first `:` comes before any `/` is an SSH target; write `./name:with-colon` for a local directory with a colon in its name. The host goes to `ssh` after `--`, so no host is read as an option. `FABRIC_SYNC_SSH` names another `ssh` program.

## Rules that keep your work

- **Pushes take turns.** A push moves the target's head only if this tree has seen it: pulled it or pushed it. A push from a tree that has not is refused with exit 84 and `target_moved`; pull first, or pass `--force` to drop what the target holds (it stays in the target's store). Two pushes to one target at once take turns on the store's lock.
- **A pull never writes over a tree it did not make empty.** A pull onto an absent or empty directory restores into it. A pull onto a tree with content is refused with exit 84 unless `--replace` is given; then the new tree is restored beside it and put in place by a rename, and the old tree is kept beside it as `<tree>.fabric-sync-previous` until the next replacing pull.
- **A pull cut anywhere leaves no half tree.** The tree path holds the old tree or the whole new one. A pull killed while copying or restoring leaves the old tree as it was; one killed between its two renames leaves the old tree aside and the new one whole, and the next sync of that tree finishes the rename first. Each case is a crash test: the process kills itself with `SIGKILL` at the point.
- **A copy cut anywhere resumes.** A snapshot enters a store only after every object it names arrived and hashed to its id; the next sync sends only what is still missing.

## Commands

### `fabric-sync push`

Capture a tree and push it to a target. `--tree <PATH>` (default `.`) is the tree, the top level when it is a repository; `--name <NAME>` its name on the target; `--force` moves the target's head even when this tree has not pulled it. A repository in the middle of a rebase, a merge or a conflict is refused (`workspace_busy`), as a suspend refuses it.

### `fabric-sync pull`

Pull a tree from a target onto a directory. `--tree <PATH>` (default `.`) is the directory to write; `--name <NAME>` the tree's name on the target; `--replace` replaces a tree with content and keeps the old one beside it. A pull of the snapshot the tree already holds writes nothing and answers `unchanged`.

### `fabric-sync serve`

Answer one sync on standard input and output. `ssh` runs it on the target machine; you do not run it yourself. Standard output carries the protocol, so `--json` is a usage error here.

### `fabric-sync help`

Print the help of a command, the same text as `fabric-sync <command> --help`.

Every command keeps [the CLI contract](../cli-contract.md): standard output only for the answer, `--json`, one error shape with a stable reason, help with examples. Exit codes: 0 done, 1 failure, 2 usage error, 83 the SSH side could not be started or stopped answering, 84 refused without `--force` or `--replace`.

## State

This machine's side keeps a store and a small file per tree under `FABRIC_SYNC_HOME`, else `$XDG_DATA_HOME/fabric-sync`, else `~/.local/share/fabric-sync`. The store is a `fabric-store` store like a node's; the tree file holds the job id the tree syncs under and the last snapshot it pushed or pulled.

## Known limits

- **No node link yet.** v1 syncs to a directory and over SSH only. A paired Fabric node's authenticated link is not a target: the node's RPC carries chunks for a job it owns, and sync has no job or lease there. A target that speaks to a node is a follow-up.
- **Every sync sends the whole manifest.** It lists every carried file, about 75 bytes each, so a tree with 100,000 files in `node_modules` sends a manifest of several MiB on every sync, even when nothing changed.
- **Stores only grow.** Neither store forgets old snapshots yet; their objects are shared, so a store grows by what changed, not by the tree's size each time. A `gc` command is a follow-up.
- **A pull survives a killed process, not a power loss.** Restore runs without a flush per file (the store's `ProcessCrash` durability), so a power loss during a pull can leave files whose bytes did not reach the drive; a node's resume flushes them.
- **Large untracked files stop a push.** An untracked, not ignored file over 64 MiB is refused, as a suspend refuses it; ignore it or commit it.
- **No harness session.** The agent's session files (`~/.claude`, `~/.codex`) do not travel; a Fabric job move carries those.
- **Same path rules as a move.** Paths are not rewritten. A tree restored at another path works for the tree itself; tools that recorded absolute paths in a cache (a `.venv`'s scripts) keep the first machine's path.
