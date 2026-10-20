# Fabric architecture

Status: design only. No code behind this page yet. See the [README](../README.md) for what exists.

Amended 2026-10-19 after step 1 research. The lease lost its clock and gained a move protocol (ADR 0003), the root is the same absolute path on every node (ADR 0011), caches carry under per-ecosystem keys (ADR 0012), and the move onto the second machine is an explicit push (ADR 0008). The research files are listed under Evidence paths.

## The one idea

The unit of development work is a job. Not a machine, not a chat session, not a folder.

A job is a goal plus the state needed to keep working on it. Where that state is stored, which machine runs it, what toolchain it needs and which agent drives it are four separate questions. Fabric keeps them separate so that each can change without the others noticing.

## The four decoupled parts

| Part | What it holds | Who owns it in code | ADR |
|---|---|---|---|
| Job record | Goal, append-only event ledger, versioned manifest, pending decisions, an opaque state directory per harness | `fabric-core` | [0001](adr/0001-job-as-unit-of-work.md), [0002](adr/0002-job-ledger-and-record.md) |
| Storage | Content-addressed snapshots of the workspace: git objects and refs in a bundle, the index and the working tree as two snapshot commits, ignored files by class (deps, build, tool caches and precious files carried, secrets never), harness state | `fabric-store` | [0004](adr/0004-content-addressed-snapshots.md), [0011](adr/0011-path-identity.md) |
| Compute | The node that owns the job and runs the harness right now. Replaceable by an explicit move. Exactly one live copy. | `fabric-node`, `fabric-net` | [0003](adr/0003-exactly-one-live-copy.md), [0005](adr/0005-transport-and-node-identity.md), [0008](adr/0008-placement-and-triggers.md) |
| Environment | `EnvSpec`: tool pins as the workspace files write them, setup commands, env var names, cache paths, the job's added secret patterns. Caches arrive when their carry key matches; otherwise the node materializes. | `fabric-env` | [0007](adr/0007-environment-spec.md), [0012](adr/0012-dependency-carry.md) |

Harnesses (Claude Code, Codex, anything that speaks the Agent Client Protocol) are adapters on top of these four. The core never reads a harness's files; it moves them as opaque bytes. See [0006](adr/0006-harness-adapters.md).

## Crate layout (planned)

```text
crates/
  fabric-core      job, ledger, manifest, lease and move state machine, trait definitions
  fabric-store     objects, blob store, SQLite catalog, snapshot publish and restore, ignored-file classes
  fabric-net       node identity, Transport trait, QUIC and TCP+TLS, blob transfer, pairing
  fabric-harness   Harness trait, ACP adapter, Claude Code adapter, Codex adapter
  fabric-env       EnvSpec detection, the .fabric/env.toml override file, the secret policy capture applies, materialization, carry keys, doctor facts
  fabric-node      the long-running node process: owner of the store lock, blob server, move target
  fabric-cli       the `fabric` binary: job init, suspend, resume, job move, job steal, status, doctor, orphan export, node pair, node serve
```

`fabric-core` and `fabric-env` exist today; in `fabric-env`, carry keys and doctor facts are still to come. Dependency direction is strictly downward: `fabric-cli` depends on everything, `fabric-core` depends on nothing in the workspace. `fabric-core` defines the `LedgerStore` trait today; the `Store`, `Transport` and `Harness` traits will live there too, and the other crates will implement them. Nothing in the core knows about QUIC, SQLite or any agent vendor.

## Data structures first

```text
JobId        = ULID
MoveId       = ULID
NodeId       = ed25519 public key (32 bytes)
Epoch        = u64, 1 at creation, increases by exactly 1 on every MoveImported or Stolen
SnapshotId   = blake3 hash of the manifest bytes
ChunkId      = blake3 hash of an object: a whole file up to 8 MiB, or one FastCDC chunk above that

Event        = { id: ULID, prev: blake3 of previous event, epoch, kind }
EventKind    = Created { job_id, goal, harness, created_by } | MoveStarted { move_id, target } | MoveImported { move_id, from }
             | MoveAborted { move_id } | Stolen { by, from, reason }
             | SnapshotPublished | Suspended | Resumed | EnvSpecChanged | DecisionRecorded | Note

JobManifest  = { job_id, goal, harness, env_spec: Option<EnvSpecId>,
                 latest_snapshot: Option<SnapshotId>, lease: LeaseState, head, event_count,
                 created_at, updated_at }
LeaseState   = Owned { node, epoch } | Departing { node, epoch, move_id, target }

SnapshotManifest = { job_id, root: AbsPath, parent: Option<SnapshotId>, created_at, created_by,
                     platform: { os, arch }, git: Option<GitState>, files: [Entry],
                     harness_state: [Entry], env_spec: Option<EnvSpecId>, caches: [CacheEntry] }
GitState     = { bundle, bundle_base, refs, head, index_commit, worktree_commit,
                 worktree_mtimes, config, exclude }
CacheEntry   = { path: RelPath, class: Deps | Build | ToolCache, carry: Carried | Partial | NotCarried, key: CarryKey }
CarryKey     = Node { node_major, package_manager, pm_major, pnpm_store_path }
             | Rust { rustc_vv, cargo_home } | Python { python_version, python_home } | Other { tool, version }
Marker       = { root, job_id, base: Option<SnapshotId>,
                 state: Clean | Dirty | Suspending { epoch, head } | Restoring { targets } }
               node-local, at <fabric-data>/jobs/<job-id>/marker, never inside the workspace
Entry        = { rel_path: RelPath, kind: File { size, chunks: [ChunkId] } | Dir | Symlink { target }, mode, mtime_ns }

EnvSpec      = { tools: [{ name, version, source }], setup: [{ program, args, cwd, when }],
                 env_var_names: [EnvVarName], cache_paths: [RelPath], secret_patterns: [SecretPattern] }
```

Every list in a manifest is strictly sorted by its key, and every manifest's bytes start with the format version. Every id is either content-derived (hashes) or time-sortable and collision-free (ULIDs), and the core makes none of them: the edge draws ULIDs from its clock and hands the core the bytes. The one host path is `SnapshotManifest.root`, the directory the job was captured in, which is also where every node restores it (ADR 0011). Nothing is a vendor session id. There is no clock in any state transition: the lease has an epoch and no expiry (ADR 0003).

## Data flow: move from one node to another

The first gate: a job started under Claude Code on the MacBook Pro, moved to the MacBook Air over the network, resumed there, dependencies included, safe under `kill -9` at any step. The Pro dials the Air and the Air cannot dial the Pro, so the move onto the Air is a push typed on the Pro (ADR 0005, ADR 0008).

```mermaid
sequenceDiagram
    participant U as User
    participant A as Node A (Pro, owner)
    participant B as Node B (Air, node serve running)
    U->>A: fabric job init (in the checkout, Claude Code already running)
    A->>A: append Created (owner A, epoch 1), Resumed
    U->>A: fabric job move job --to air
    A->>B: fabric doctor --against air (exact-string preflight)
    A->>A: stop harness, marker Suspending, capture, publish snapshot
    A->>A: append SnapshotPublished, Suspended; marker Clean
    A->>B: push ledger suffix and missing chunks (no ownership change)
    B-->>A: snapshot complete
    A->>A: append MoveStarted { move_id, target B } (epoch 1), fsync
    A->>B: MoveOffer { move_id }
    B->>B: append MoveImported { move_id, from A } (epoch 2), fsync
    B-->>A: MoveDecision (the record)
    A->>A: append the record; A is a departed copy
    U->>B: fabric resume job
    B->>B: owner is B: restore at the same root
    B->>B: compare carry keys, materialize if needed
    B->>B: launch harness, append Resumed
```

Step by step, with the failure each step must survive:

0. `fabric job init` on A, inside the checkout where Claude Code is already running. It records the directory as the job's root in the node-local marker, finds the Claude session with the adapter's `detect`, appends `Created`, which makes A the owner at epoch 1, and writes the first `EnvSpec`. Nothing is moved and nothing is stopped (ADR 0001). A crash here leaves at most a job directory with no `Created` record, which the next `init` replaces.
1. `fabric job move <job> --to air` on A runs `fabric doctor --against air` first (ADR 0008). A refused row stops the move before anything changes; a fallback row is printed and carried into the snapshot's cache entries (ADR 0012).
2. A stops the harness and publishes a snapshot in the order ADR 0004 fixes: the marker `Suspending` with the ledger head; new objects to `tmp/`, `fsync`, rename into `chunks/`; chunk rows; the manifest by the same temp and rename; the snapshot rows; directory `fsync`; and only then the `SnapshotPublished` ledger event, which is the single commit point. Then `Suspended`, then the marker `Clean`. A `kill -9` between any two of these steps is not a special case: the next command on the job reads the marker and the head and finishes the sequence without a second capture (ADR 0004, Suspend). If the harness will not stop, nothing is written and the job stays live on A.
3. A pushes the ledger suffix and every object B lacks. B asks per object id; a dropped connection restarts at the next missing object. Nothing about ownership has changed, so this step can fail or be killed any number of times and A is still the owner (ADR 0003, step 2).
4. Once B reports the snapshot complete, A appends `MoveStarted { move_id, target: B }`, `fsync`s, and sends `MoveOffer`. From this append on A is not runnable. This is the one ambiguous window, and it is one round trip long.
5. B checks that its replica head is that `MoveStarted` and the snapshot is complete, appends `MoveImported { move_id, from: A }` at epoch 2, `fsync`s, and replies with the record. If the reply is lost, A asks `MoveStatus { move_id }` and gets the same record back; B never writes a second decision for the same `move_id`. If A crashed after step 4, the next command on A asks the same question. If B is unreachable, A stays frozen: not runnable, not lost, and `fabric status` names B and the two commands that resolve it, `fabric job move` again or `fabric job steal --force` (ADR 0003). This is the T3 Code bug made impossible: no path leads A back to runnable without a record B wrote.
6. A appends B's record. The chain now says B owns epoch 2. A refuses any write, blob push or ledger append carrying epoch 1, and A's workspace stays on disk, marker `Clean`, as a departed copy that makes the next move back cheap.
7. `fabric resume <job>` on B. The ledger says B owns the job, so no network step is needed. B restores at `SnapshotManifest.root`, the same absolute path as on A (ADR 0011). If that path already holds a workspace, its marker decides: the same snapshot means skip, a cleanly suspended older snapshot means update in place, anything else means refuse and point at `fabric orphan export`. A fresh restore writes into a temp directory and renames at the end, so a crash leaves no half workspace. Nothing inside harness state is rewritten; Claude Code's signed thinking blocks would not survive it (ADR 0006).
8. B compares each cache entry's carry key with its own measurement (ADR 0012). A matching `node_modules` is written and nothing runs. A mismatched one is skipped, printed with the field that differed, and `fabric-env` runs the `EnvSpec` setup step for it (ADR 0007). On the two Macs as measured, Node 22 against Node 26 means the install path.
9. The `fabric resume` process pre-seeds trust for the root, launches the agent in the user's terminal with the job's overlay flags, in its own process group, registers its pid and start time with the supervisor, and appends `Resumed`. The supervisor is the node process when one runs and otherwise the `resume` command itself, which stays in the foreground until the agent exits and kills the process group if it learns a higher epoch (ADR 0003). `resume` never launches an agent and exits.

If B dies between steps 7 and 9, B owns epoch 2 and the job is not lost: the snapshot is on both nodes and the ledger says B imported. Running `fabric resume` again on B finishes the restore if it did not complete, skips it when the workspace marker already names the snapshot, and relaunches (ADR 0004). Nothing bumps the epoch by itself: bringing the job back to A is `fabric resume` on A, which pulls from B's node process by the same protocol, or, if B is gone, `fabric job steal <job> --force` on A, which prints what B may have left behind. Once epoch 3 exists, the epoch 2 owner can never write again.

## In scope now

- One job, two nodes, both Efe's own Macs, same user and home, same OS and CPU architecture, different tool versions (`fabric doctor` reports them).
- Claude Code first, Codex second, ACP as the generic path.
- Explicit `suspend`, `job move --to`, `resume` and `job steal --force`. No automatic triggers, no placement policy, no time-based lease; every automatic choice prints its reason.
- The root is the same absolute path on every node. `node_modules` and similar caches carried in the snapshot when their carry key matches, rebuilt otherwise.
- Own QUIC transport with node keys, TCP+TLS fallback, the Pro dialing the Air. Tailscale is a network Fabric can run over, not a dependency.
- Rust only. Library crates plus one CLI binary. Not an MCP server.

## Later, with an ADR each

- An always-on witness that may write `Stolen` without `--force` after a time-based lease, which is the only place expiry comes back (ADR 0003).
- Sleep and lid-close triggers (ADR 0008 names the watcher as later).
- Different home paths between nodes: the P2 rewrites per tool (ADR 0011).
- Cross-harness resume through a neutral brief (ADR 0006 calls this an experiment, not a promise); OpenCode and Pi as the next adapters.
- Three or more nodes with quorum-based leases.
- Phone and cloud nodes. GPU nodes as a placement target.
- `devcontainer.json` import into `EnvSpec`.
- Cross-platform cache carry, and carrying pnpm store files instead of `node_modules`. Out of scope until same-platform carry is proven (ADR 0012).

## How to read the ADRs

Read them in number order. 0001 and 0002 define the data. 0003 is the one that protects the user's work and is worth the most review time. 0004 and 0005 are the storage and network mechanics. 0006 and 0007 are the adapters. 0008 is the explicit move and the preflight. 0009 is how we work. 0010 is what Fabric is not. 0011 and 0012 are the two decisions the preflight measurements forced: the root path and which caches travel.

### Evidence paths

The ADRs cite research notes by file name: `2026-10-19-thesis-as-understood.md`, `2026-10-19-competitor-landscape.md`, `2026-10-19-t3code-vs-fabric.md`, `2026-10-19-step1-adversarial-review.md`, `2026-10-19-step1-design-input.md`, `step1/H1` to `H6`, and `step1/preflight-2026-10-19.md`. These are private working notes outside this repository, kept under `~/Documents/fabric-research/` on the owner's machines. They are not vendored, because they quote third-party sources at length and name people. A citation in an ADR is a pointer for the owner and a label for everyone else: the ADR text itself must carry enough of the fact for the decision to be judged without the note. Where it does not, that is a defect in the ADR.
