# ADR 0003: Exactly one live copy

Status: accepted, 2026-10-19

Amended 2026-10-19 after step 1 research. The first version of this ADR used a time-based lease: a 60 second expiry, a renewal every 20 seconds, a 30 second skew margin, an arbiter that granted claims, and `fabric resume --force-expired` as the way past a holder that stopped renewing. The research removed the ground under it: there is no always-on node today (two laptops, `2026-10-19-step1-design-input.md`, section 10, A22), a sleeping laptop looks exactly like a dead one, and a lease that expires on a clock is only safe when the store rejects stale writers, which is the fencing this ADR already had. So the expiry is gone. What remains is the cooperative move protocol the research recommends (same file, section 4.8, options L1 and L1a; section 9, D11 and D30; `step1/H5-workspace-mobility.md`, sections 9.1 to 9.4), a manual steal with an epoch bump behind `--force`, and the epoch fence. An always-on witness that could bring expiry back is a later ADR (Open questions). Sections marked "unchanged" kept their text apart from renamed terms.

Amended 2026-10-19 while writing `fabric-core` (PR `feat/core`): `Stolen` names the thief as `by` next to `from`, so the owner after a steal is in the record and not only in the transport (ADR 0002, amendment of the same date). The lease table and the steal section below use the new shape. Nothing else changed.

Amended 2026-10-19 after the adversarial review of PR `feat/core`. Five points:

1. `MoveOffer` carries `job_id`, and the target's answer names its cause. `Decided` and `Import` are as below. `Rejected { head }` means the offer does not link and the source pushes the suffix after `head` and offers again. `SnapshotIncomplete { snapshot }` means the offer links but the store does not hold the snapshot whole, and the source pushes it and offers again. `Stale` means the offer's epoch is behind. An offer for another job, or for a move whose target is another node, is an error the target prints, not a protocol answer, because no retry fixes it; before this the source could loop on `Rejected` forever.
2. The core's `steal` takes no `--force`: that gate is the CLI's, as this ADR already said, and a bool in the core only moved the vocabulary. Both `steal` and the offer answer take a `CompleteSnapshot` that names the snapshot the caller checked in the store, and the core compares it with the snapshot the rules require; a bare "complete" flag could vouch for the wrong snapshot. The type carries the name and not a proof: the core cannot see the store, so the check itself is the store crate's, and the second review corrected the first wording that called the type a witness the store makes. The warning is printed before the `Stolen` record is persisted, not after.
3. The departing source and a target that vanishes, with two nodes. The source is still the owner of record while the move is open, so `steal` on the source is `StealBySelf`, and the sentence under Recovery, "the only way out that does not involve the target is the steal", means a steal by a third node. On the two-Mac gate that is a job frozen on the Pro until the Air answers or a third node exists; `fabric status` must say so in those words. The alternative, letting the source write `Stolen { by: source, from: source }` while `Departing`, is recorded under Open questions for the owner to decide; it needs the fork rule first, because a late `MoveImported` at the same epoch would then be a fork rather than a refused record.
4. The invariants under Consequences hold on one chain. A steal can fork the chain, and until the fork rule is implemented two nodes on two suffixes can each hold an owner at the same epoch; the fork rule picks one. The model test asserts a single owner across all nodes only in runs without a steal, and in every run asserts that no two owners share an epoch on a common chain.
5. The thief delivers `Stolen` to the stolen-from owner as its writer; the owner accepts it through the writer rule and is fenced. A `Stolen` record delivered by any other node is refused by the sender rule (ADR 0002, amendment 6).

Amended 2026-10-19 after the second adversarial review of PR `feat/core`. Three points:

1. The target of an open move cannot steal it (`StealByTarget`): it holds the two exits, import and abort, and a steal from it only forked the chain with the move left undecided. A relayed `Stolen` written by the target is refused on every node.
2. The steal warning carries `owner_stopped_at_snapshot`, true when the chain's last `Suspended` names the latest snapshot and no `Resumed` follows, which is the case this ADR says cannot lose pushed work; and `last_record_at` is the time of the owner's last record, not a target's `MoveAborted`.
3. `CompleteSnapshot` names the snapshot the caller checked in the store and is not a proof the store makes (amendment 2 above, corrected in place).

## Context

This is the decision that protects the user's work. Everything else can be slow or ugly; this cannot be wrong.

The case to prevent is documented in the T3 Code handoff stack. T3 moves a thread by packing it on the source, uploading it, and having the target create a new thread while the old one becomes a tombstone. The takeover notes for PR #16751 found that a lost response after import "can therefore leave two runnable copies" (`2026-10-19-t3code-vs-fabric.md`, sections 1 and 7, question 2). The mechanism, from the maintainer's own analysis: the source writes `failed` on any import error, `failed` accepts local turns, and a retry allocates a new handoff id that never reconciles the first (`step1/H5-workspace-mobility.md`, section 9.1; the issue was still open on 2026-10-19). Two runnable copies of one job means two agents editing two workspaces that both believe they are the job. Whichever the user notices second has done work that is now in conflict or lost.

Three other facts shape the design. First, we have two nodes and no third party to arbitrate. Second, neither node is always on: the source of a move may be asleep, and nothing distinguishes asleep from dead over the network. Third, the dangerous second writer after a move is often not a Fabric process at all but the harness's own picker, desktop app or supervisor opening the native session that still exists on the departed machine (design input, section 4.8.1). The lease in this ADR fences Fabric. The native tombstones in ADR 0006 fence everyone else.

## Decision

### The lease

A job has one lease. The lease has an epoch and no expiry.

```text
MoveId     = ULID, generated by the owner when it starts a move

LeaseState =
  | Owned     { node: NodeId, epoch: Epoch }
  | Departing { node: NodeId, epoch: Epoch, move_id: MoveId, target: NodeId }
```

- A node may run a harness for a job only while the lease is `Owned` by it. `Departing` is not runnable on any node.
- `Created` is written at epoch 1 and makes `created_by` the owner. There is no unowned job.
- The epoch increases by exactly one on each of two records, `MoveImported` and `Stolen`, and never otherwise. It never repeats and never goes backwards.
- The fencing token is `(job_id, epoch)`. Every message between nodes about a job carries the sender's epoch, and every write (ledger append, blob push, manifest update) carries it too. A node that knows a higher epoch for that job refuses the message with `StaleEpoch { yours, current }`. There is no override flag.
- The research's three names map onto this as follows. `Owned{node, epoch}` is the state above. `Departing{move_id, target}` is the state above. `Departed{target owns epoch + 1}` is what the source sees once the chain holds the target's `MoveImported`: the lease is `Owned { target, epoch + 1 }` and the source's workspace is a departed copy, kept in place as a warm cache for the next move back and never launched.

The lease is derived from the ledger (ADR 0002) and nothing else. The records that move it:

```text
MoveStarted  { move_id, target }     by the owner, at its epoch:       Owned -> Departing
MoveImported { move_id, from }       by the target, at epoch + 1:      Departing -> Owned { target, epoch + 1 }
MoveAborted  { move_id }             by the target, at the owner's epoch: Departing -> Owned { source, epoch }
Stolen       { by, from, reason }    by `by`, at epoch + 1, behind --force:   anything -> Owned { by, epoch + 1 }
```

### The move protocol

A move is `fabric job move <job> --to <node>` typed on the owner (a push), or `fabric resume <job>` typed on a node that is not the owner (a pull, ADR 0008). Both run the same protocol; only the direction of the first message differs. The nodes are the source (the owner) and the target.

1. **Stop and snapshot.** The source runs the suspend sequence of ADR 0004: stop the harness, capture, publish, append `SnapshotPublished` and `Suspended`, marker `Clean`. If the harness will not stop, the move fails here and nothing has changed.
2. **Push the snapshot.** The source pushes the ledger suffix and every chunk the target lacks (ADR 0005, blob transfer). This step changes no ownership. It can fail, be killed and be repeated any number of times, and the source stays `Owned`. This is the research's L1a split: the whole payload crosses the network before anything is committed, so a dropped connection during transfer costs nothing but time (design input, section 4.8, L1a; A35).
3. **Commit to departing.** Only when the target has acknowledged that the snapshot is complete in its store does the source append `MoveStarted { move_id, target }`, `fsync`, and send `MoveOffer { move_id, head, snapshot }`. From this append on the source is not runnable. The ambiguous window is one round trip.
4. **The target decides.** The target answers a `MoveOffer` by exactly one of three rules, checked in this order:
   1. Already decided: its ledger holds a `MoveImported` or `MoveAborted` for this `move_id`. It appends nothing and replies `MoveDecision { record }` with the exact record bytes. This is the retry after a lost reply.
   2. Import: its replica head is the offered `MoveStarted`, the named snapshot is complete in its store, and the offer names this node as target. It appends `MoveImported { move_id, from: source }` at `epoch + 1`, `fsync`s, and only then replies `MoveDecision { record }`.
   3. Otherwise it replies `Rejected { head }` or `StaleEpoch`, appends nothing, and the source fetches what it lacks and offers again.
5. **The source completes.** The source appends the record it was sent, as a replica of the target's record. Its lease view is now `Owned { target, epoch + 1 }`. It prints the new owner and the epoch. The workspace stays on disk, marker `Clean` at the snapshot, and `fabric status` lists it as departed.
6. **Launch on the target** is a separate user action, `fabric resume <job>` on the target, which restores, materializes and launches because the ledger says the target owns the job (ADR 0008). A push never launches a harness, because the source has no terminal on the target.

The target's decision is the single commit point of a move and it is written exactly once per `move_id`. The import receipt is the `MoveImported` record itself: durable before it is answered, content-addressed by the hash chain, and returned byte for byte to every later question about the same `move_id`.

**Abort.** A move that cannot finish is aborted only by the target. `fabric job move --abort <job>` on the source sends `MoveAbort { move_id }`; the target, if undecided, appends `MoveAborted { move_id }` at the source's epoch, `fsync`s, and replies with it; the source appends it and is `Owned` again. The target's user can also abort locally with the same command, and the source learns the decision from `MoveStatus` the next time it asks. If the target already imported, an abort request is answered with the `MoveImported` record and the source completes instead. There is no path from `Departing` to `Owned` on the source that does not pass through a target-written record. This is the exact rule T3's `failed` state skips, and the property test below makes it a theorem of the state machine rather than a convention.

**Recovery.** Every Fabric command on a job whose lease is `Departing` with this node as source first sends `MoveStatus { move_id }` to the target. `MoveDecision` finishes step 5. `MoveUndecided` leaves the choice to the user: offer again (`fabric job move` with no flags repeats steps 2 to 4 under the same `move_id`), or abort. An unreachable target leaves the job frozen on the source: not runnable, not lost, and `fabric status` says so with the target's name. The only way out that does not involve the target is the steal.

### The steal

`fabric job steal <job> --force` on a node that is not the owner bumps the epoch without the owner's consent. Without `--force` the command prints what it would do and exits with an error. With it:

- It refuses unless the local store holds a complete copy of the snapshot named by the latest `Suspended` or `SnapshotPublished` in the local replica; a steal must never resume from nothing.
- It prints a warning that states the owner's node id, the time of the last record from it, the snapshot it will resume from, and that any work the owner did after that snapshot will become an orphan on the owner's disk.
- It appends `Stolen { by: thief, from: owner, reason }` at `epoch + 1` with `prev` set to the local replica head, then a `Note` with the warning text. `reason` is the user's text from `--reason`, or empty.
- It does not launch. `fabric resume` afterwards launches as the owner.

When the stolen-from node next connects, it is offered the `Stolen` record. If the record's `prev` equals its head, it appends it, learns the higher epoch, and is fenced: the supervisor kills the harness, the workspace is marked an orphan workspace and nothing in it is deleted (Orphans, below). If the stolen-from node appended records of its own after the fork point, the chains have forked and the fork rule decides.

A steal of a job whose replica head is `Suspended` and whose snapshot is complete locally cannot lose work unless the owner resumed since the last push; the warning says that. Every other steal can. In this phase there is no automatic steal, no timeout that permits one, and no clock in the decision anywhere.

### Replication

A move needs the target to hold the source's head and the latest snapshot, and a steal needs the same on the thief. Neither can be fetched from a node that is asleep, so both are pushed while the owner is awake:

- The node that appends a record pushes it to every paired peer as `Append { record }` (ADR 0005), best effort, right after the `fsync`. A peer applies the acceptance rule of ADR 0002. A peer that was unreachable fetches the missing suffix on its next connection.
- `fabric suspend` pushes the new snapshot (manifest chunks, then file chunks) to every paired peer it can reach, and does so before it prints success. A peer that cannot be reached is a warning in the output, by name, not a failure. `fabric status` shows, per paired peer, the last record the peer acknowledged and whether the latest snapshot is complete there.

Replication never changes who may append. A replica is a copy; the acceptance rule keeps it one.

### Fencing (unchanged in substance)

Fencing covers the writes Fabric performs: ledger appends, blob pushes, manifest updates. The harness writes to its workspace directly, and Fabric performs no write on its behalf. The only control over the harness is the kill: a harness launched at epoch N is killed by its supervisor (below) when the supervisor learns of epoch N+1, and until it learns it (which, for a sleeping node, is when it wakes) the harness keeps editing. Everything it wrote after the last published snapshot is preserved by the orphan path below, never by fencing.

Fencing does not reach a writer that never asks Fabric: the harness's own picker, desktop app, remote control or background supervisor can reopen the native session on the departed machine. ADR 0006 therefore writes the harness's native tombstone during capture (Claude's `continued-in` record, Codex `thread/archive`) and refuses to capture a session that a harness supervisor would restart.

### Who supervises the harness

The kill needs a process that is alive to perform it. That process is the supervisor, and it is always the holder of the data directory lock (ADR 0004): `fabric node serve` when it runs, otherwise the `fabric resume` command itself, which then holds the lock until the harness exits instead of for the duration of a short command. There is no third case. `resume` refuses with `NoSupervisor` when it can neither reach a running node over the socket nor take the lock, and it never launches a harness and exits.

The harness process is spawned by the CLI, never by the daemon, because an interactive harness such as Claude Code needs the user's terminal and the user's shell environment, and a daemon has neither. The CLI spawns it in a new process group, makes that group the terminal's foreground group, and waits for it. Before the harness starts, the CLI registers it with the supervisor, which writes `<fabric-data>/jobs/<job-id>/harness` by temp, `fsync`, rename:

```text
HarnessRecord { epoch: Epoch, supervisor_pid: u32, running: RunningHarness }

RunningHarness =
  | Spawned  { pid: u32, pgid: u32, started_at: u64 }   started by a Fabric process on this node
  | External { pid: u32, pgid: u32, started_at: u64 }   started by the user, adopted by `fabric job init` (ADR 0001)
  | Daemon   { session_id: String }                     owned by the harness's own supervisor; has no pid Fabric may kill
```

`started_at` is the kernel's start time of the process, so a pid reused after the harness exited is never killed. The supervisor's duties are two. It kills on fencing: on learning a higher epoch for the job, from a peer message or from its own ledger, it sends `killpg(SIGKILL)` to the recorded group; for `Daemon` it calls the adapter's `stop` (ADR 0006) until `detect` reports nothing running, because a session under the harness's own supervisor is restarted after a plain kill (research, step1 adversarial review, C6). For that reason the Claude Code adapter launches a foreground `claude` in v0, never `claude --bg`. And it rewrites the record when the harness exits. There is no renewal duty, because there is no expiry.

The record is also how a supervisor that died comes back. Every command that opens the store reads `harness` first. If `supervisor_pid` is dead and the recorded process is alive, the command re-adopts it as the new supervisor when the lease is still `Owned` by this node at the recorded epoch, and kills it otherwise. Between the supervisor dying and the next Fabric command on that node, an unsupervised harness is in the same position as a harness on a sleeping node: a peer may steal, and whatever it writes is preserved by the orphan path. Running `fabric node serve` shrinks that window to the daemon's restart time. `fabric status` prints the record and whether its supervisor is alive.

A pull (`fabric resume` on the target) needs a process on the source that can run step 1: the node process, or the `fabric resume` session that launched the harness there. When neither listens, the pull fails with `OwnerUnreachable { node }` and the message names the push command to type on the owner and the steal. On the two-Mac gate the Pro can dial the Air and the Air cannot dial the Pro (`step1/preflight-2026-10-19.md`), so a move onto the Air is a push from the Pro, and a move back is a pull from the Pro against the Air's node process.

### Orphans

If node A owned the job at epoch 1, went to sleep without suspending, and node B stole epoch 2, then:

- B resumes from the latest snapshot B holds. If that is older than A's last work, the ledger says so with a `Note`.
- Work A did after its last snapshot is not lost. It is on A's disk. When A wakes it learns epoch 2, is fenced, and the supervisor stops A's harness. A's unpublished workspace is kept in place and reported as an orphan: `fabric status` lists it, `fabric orphan export` turns it into a snapshot that is not the job's head and pins it in the store so `store gc` keeps it (ADR 0004). Nothing is deleted automatically. Ever.
- The same holds for what the harness wrote outside the workspace. A fenced Claude Code harness also wrote its transcript under the user's Claude projects directory, and a later `resume` of the job on A would place the job's newer transcript over it. `fabric orphan export` therefore calls the adapter's `capture` as well and stores the result as the orphan snapshot's `harness_state`, and an adapter's `restore` refuses with `HarnessStateDiverged` rather than overwrite state that is not already contained in what arrives (ADR 0006). The message names `fabric orphan export`.

### Forks

Only a steal can fork two ledgers, because every other epoch bump is written by a target that holds the owner's head. When a steal and the old owner's own records sit on two suffixes after a common prefix, both nodes compute the winner from the two suffixes alone, with no message and no clock, and arrive at the same answer. Compare the highest epoch-bumping record (`MoveImported` or `Stolen`) in each suffix:

1. The higher epoch wins.
2. At equal epoch, `MoveImported` beats `Stolen`. The move was consented to by the owner of record; the steal was a guess that the owner was gone.
3. If both are `Stolen`, the lower `NodeId`, compared as bytes, wins.

A suffix with no epoch-bumping record counts at the fork point's epoch. That is the old owner's suffix when it kept appending `SnapshotPublished`, `Suspended` or `Note` at its epoch before it saw a steal: under rule 1 it loses to the steal, and the snapshots it published become orphan snapshots as described below. Both suffixes cannot lack one, because only a steal forks a chain.

Two suffixes cannot both hold a `MoveImported` at the same epoch, because an owner has at most one open `MoveStarted` and `Departing` has no exit except that target's single decision. The order is total, so there is never a tie. The losing node stops its harness (the kill of Fencing, above), moves its records after the fork point to `ledger.forked-<hex of fork head>` beside the ledger, lists every snapshot those records published as an orphan snapshot and pins it (ADR 0004), marks its workspace as an orphan workspace, then appends the winning suffix. Nothing is deleted.

The case that motivated rule 2: A owns epoch 1 and is partitioned. B steals and holds `Stolen` epoch 2. Meanwhile the user at A types `fabric job move --to C` and C imports, so A's chain holds `MoveImported` epoch 2. On reconnection rule 2 picks C's chain; B kills its harness and B's work since the fork is an orphan snapshot. If A's chain had reached epoch 3 through a second move, rule 1 picks it for the same reason. The user is told which side lost and where its work is, in both cases.

### Two laptops, stated plainly

With two nodes, no witness and no clock, Fabric cannot tell "peer is asleep" from "peer is gone". The protocol above is therefore safe and not live: a move whose target vanishes after step 3 freezes the job on the source until the target answers or the user steals; a job whose owner is asleep cannot be moved until it wakes or the user steals. Every frozen state is printed by `fabric status` with the one command that resolves it. This is the same stance meridian takes for sending mail: the one ambiguous window is resolved by a person, never by a retry that could double-execute (`step1/H5-workspace-mobility.md`, section 9.1).

The always-on witness is the later ADR that trades this for liveness: a third node (a Mac mini, a small VPS, or a phone holding a replica) that is the only node allowed to write `Stolen` without `--force`, after a time-based lease that the owner renews through it. The epoch, the fence, the move protocol and the fork rule do not change when it arrives; only who may write `Stolen` does.

### Idempotency

`suspend`, `move`, `resume` and `steal` are safe to run twice. A second `suspend` on a job whose head is `Suspended` is a no-op that prints the state. A `suspend` interrupted anywhere after the harness stopped is a sequence the next command finishes, keyed on the marker and the ledger head (ADR 0004, Suspend). A second `move` with the lease `Departing` repeats the offer under the same `move_id` and converges on the target's one decision. A second `resume` on the owner does not restore again: the workspace marker (ADR 0004) shows the snapshot is already there, with or without edits since, and only the harness is relaunched if it is not running. A second `steal` is refused, because the first made this node the owner.

## Alternatives considered

1. **Tombstone the source after a successful import** (T3 Code). Depends on a message that can be lost; that is exactly the two-copies bug. Rejected. The protocol above puts the tombstone, `MoveStarted`, before the decision and makes the decision the target's.
2. **A time-based lease with renewal and expiry** (this ADR's first version; research L2). Safe only with a fence, which exists, and live only with an authority that is awake, which does not. A paused or sleeping holder acts after its lease ended, and the fence catches the writes Fabric makes but not the harness's edits, so every expiry-based claim orphans work that a cooperative move would have carried. Deferred to the witness ADR.
3. **Lock file on a shared filesystem.** Needs a shared filesystem. Rejected by the no-vendor-dependency rule and because sleep makes NFS locks lie.
4. **External witness now** (a git remote ref with `--force-with-lease`, S3 conditional writes; research L3). Breaks ties and is always on, but puts a network and credentials on the hot path and ten of the owner's repositories have no remote. Deferred; it is one shape of the witness ADR.
5. **Last writer wins with CRDT merge of workspaces.** Merging two agents' edits to the same files is a conflict resolution product, not a mover. Rejected.
6. **Quorum from day one.** Three nodes before the first gate passes is too much; the gate is two Macs. Deferred, with the steal rule isolated so it can change.
7. **No lease, user discipline only** ("just do not resume in two places"). This is the status quo of every tool in the landscape and the thing that burns users; Claude itself interleaves two resumes of one session into one transcript (design input, A31). Rejected.

## Consequences

- The lease state machine is small enough to property test exhaustively. Arbitrary interleavings of stop, snapshot push, `MoveStarted`, offer, decision, lost reply, repeated offer, abort, steal, crash and message loss must keep four invariants: at any instant at most one node's ledger view says `Owned` by itself for a given epoch; for any epoch the chain holds at most one epoch-bumping record; the epoch never decreases; and a source in `Departing` reaches `Owned` only by appending a target-written record for the same `move_id`. The named interleaving is the T3 bug: the target imports, the decision is lost, the source retries; the test asserts the source never runs and converges on the same `MoveImported`. The red-first proof for it is the research's: make `Departing` fall back to `Owned` on a transport error and watch the first invariant fail (`step1/H5-workspace-mobility.md`, section 9.4).
- The same test drives the fork rule with the named interleavings: a steal against an owner that later moves at the same epoch, two steals at the same epoch, a steal against an owner that reaches a higher epoch, and a steal against an owner that keeps suspending at its old epoch; in every run both nodes compute the same winner, exactly one harness survives, and every snapshot on the losing suffix is pinned.
- The kill -9 harness adds the research's adversaries that are not Fabric processes: a fake harness supervisor that respawns the process after a kill, a fake picker that opens the native session on the source after a move, and a fake automation that targets the thread (design input, section 8.2, phase 7). The invariant must hold with them running, which is what the native tombstones of ADR 0006 are for.
- Every per-job network message carries an `epoch` in its envelope (ADR 0005). The only frame without one is `Hello`, which names no job.
- The orphan path is a real feature with a real command, not an error message.
- Users will sometimes be told "no" (stale epoch, frozen move, owner unreachable). The message must say which node owns the job, what state the move is in, and the one command that resolves it.

## Open questions

- Should a departing source be allowed to steal from itself when its target has vanished? Today it cannot (amendment 3 above): with two nodes the job stays frozen. Allowing `Stolen { by: source, from: source }` only while `Departing` would give the user a way out with the warning a steal prints, at the cost that a target which did import before vanishing holds `MoveImported` at the same epoch, which the fork rule resolves in the target's favour and leaves the source's later work as an orphan. Decide with the fork rule; the core keeps the refusal until then.
- The always-on witness: a third Mac, a tiny VPS, or a phone replica? It is the only place a time-based lease comes back, and it gets its own ADR with the lease length, renewal and skew rules this ADR dropped.
- Should `MoveStarted` carry a deadline after which the source may ask the target to abort automatically on the next `MoveStatus`? It would shorten frozen states without adding a clock to the decision, since the target still writes the record. Decide after the first frozen move on the real pair.
- Whether a departed copy should be moved out of the job root into a cache directory, so that a user who types `claude` in the old directory by hand cannot resume there. The research suggests it (`step1/H5-workspace-mobility.md`, section 9.3); the native tombstones of ADR 0006 may make it unnecessary. Measure after the tombstone experiments.
