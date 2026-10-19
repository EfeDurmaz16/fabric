# ADR 0003: Exactly one live copy

Status: accepted, 2026-10-19

## Context

This is the decision that protects the user's work. Everything else can be slow or ugly; this cannot be wrong.

The case to prevent is documented in the T3 Code handoff stack. T3 moves a thread by packing it on the source, uploading it, and having the target create a new thread while the old one becomes a tombstone. The takeover notes for PR #16751 found that a lost response after import "can therefore leave two runnable copies" (`2026-10-19-t3code-vs-fabric.md`, sections 1 and 7, question 2). Two runnable copies of one job means two agents editing two workspaces that both believe they are the job. Whichever the user notices second has done work that is now in conflict or lost.

Two other facts shape the design. First, the source may be asleep or dead; a push-from-source design cannot handle that at all (same note, section 3, "Host offline, failover"). Second, we have two nodes in v0 and no third party to arbitrate.

## Decision

### The lease

A job has one lease. The lease has an epoch and an expiry.

```text
LeaseState =
  | Free
  | Held { node: NodeId, epoch: Epoch, expires_at: u64 }
  | Released { epoch: Epoch }
```

- A node may run a harness for a job only while it holds the lease.
- `Created`, and any event written while the lease is `Free`, carries epoch 0. The first claim is epoch 1. While the lease is `Free` or `Released` only the arbiter (below) may append, and only `Note` and `DecisionRecorded`; replicas accept those from the arbiter at the current epoch (ADR 0002).
- A claim is a request to the arbiter, never a local append. The arbiter appends `LeaseClaimed` with `epoch = previous_epoch + 1` on the claimant's behalf (Claim protocol, below). Epochs never repeat and never go backwards.
- `suspend` releases the lease: the holder appends `LeaseReleased` at the current epoch. The next claim gets `epoch + 1`.
- The holder renews before expiry by writing `<fabric-data>/jobs/<job-id>/renewal`, a small postcard file `{ epoch, node, expires_at }` written by temp, `fsync`, rename. Renewal is not a ledger event and does not bump the epoch. `Held.expires_at` is the later of the claim record's `expires_at` and the renewal file's, when the file names the same epoch and node; a file naming any other epoch is ignored. The holder pushes each renewal to paired peers as `LeaseRenewed { epoch, expires_at }`, best effort; a peer stores it in its own renewal file under the same rule.

### Claim protocol

Two per-node ledgers can disagree only if two nodes append. So only one node may append a claim, and it is always the same one: the arbiter.

- The arbiter for a job is the node that wrote the latest `LeaseClaimed` or `LeaseReleased`. Before any claim it is `created_by`. After a release the arbiter is the node that released, which is also the node that holds the latest snapshot.
- A claim is `ClaimRequest { job, node: claimant, expected_head }` sent to the arbiter. `expected_head` is the hash of the last ledger record the claimant has seen, and the message's `epoch` is the epoch of that record.
- The arbiter answers a `ClaimRequest` by exactly one of three rules, checked in this order:
  1. New grant: `expected_head` equals its own head and its lease state is `Free` or `Released`. It appends `LeaseClaimed { node: claimant, epoch: current + 1, expires_at }`, `fsync`s the ledger, and only then replies `Granted { record }` with the exact record bytes.
  2. Repeated grant: its lease is `Held` by the requesting node and the record immediately after `expected_head` in its ledger is that `LeaseClaimed`. It appends nothing and replies `Granted { record }` with the same record bytes. This is the retry after a lost reply, and it is the only case in which the arbiter's head is allowed to differ from `expected_head`.
  3. Otherwise it replies `Rejected { head, lease }` and appends nothing.
- The claimant appends the record it was sent, as a replica of the arbiter's record, and only then restores and launches. A claimant must not write a renewal or push `LeaseRenewed` before it holds the record; a renewal naming an epoch the ledger does not have is ignored by every reader. If `Granted` is lost, the claimant retries the identical request and rule 2 answers it. A grant whose `expires_at` has already passed when it arrives is still a grant: the claimant appends it and renews at once. The retry is idempotent and the property test below covers it.
- `fabric resume` on the arbiter itself goes through the same function without a network round trip: the arbiter consults only its own ledger.
- A node that is not the arbiter never appends a claim on its own. If the arbiter is unreachable there is no claim. The only way past that is `--force-expired`, which is a user action (below).

The property test in Consequences is written against this protocol: with one arbiter and lossy messages, no interleaving of request, grant, lost reply and retry produces two `LeaseClaimed` records at one epoch.

### Replication

A claim needs the claimant to hold the arbiter's head, and a resume needs the latest snapshot. Neither can be fetched from a node that is asleep, so both are pushed while the holder is awake:

- The node that appends a record pushes it to every paired peer as `Append { record }` (ADR 0005), best effort, right after the `fsync`. A peer applies the acceptance rule of ADR 0002. A peer that was unreachable fetches the missing suffix on its next connection.
- `fabric suspend` pushes the new snapshot (manifest chunks, then file chunks) to every paired peer it can reach, and does so before it prints success. A peer that cannot be reached is a warning in the output, by name, not a failure: suspend is complete locally, and the user is told that the other machine does not yet hold the snapshot. `fabric status` shows, per paired peer, the last record the peer acknowledged and whether the latest snapshot is complete there, so the user can see replication lag before closing the lid.

Replication never changes who may append. A replica is a copy; the acceptance rule keeps it one.

### Fencing

Every message between nodes about a job carries the sender's epoch. Every write (ledger append, blob push, manifest update) carries it too. A node that knows a higher epoch for that job refuses the message with `StaleEpoch { yours, current }`. There is no override flag.

Fencing covers the writes Fabric performs: ledger appends, blob pushes, manifest updates, renewals. The harness writes to its workspace directly, and Fabric performs no write on its behalf. The only control over the harness is the kill: a harness launched at epoch N is killed by its supervisor (below) when the supervisor learns of epoch N+1, and until it learns it (which, for a sleeping node, is when it wakes) the harness keeps editing. Everything it wrote after the last published snapshot is preserved by the orphan path below, never by fencing.

### Who supervises the harness

The kill needs a process that is alive to perform it, and the lease needs a process that renews it. That process is the supervisor, and it is always the holder of the data directory lock (ADR 0004): `fabric node serve` when it runs, otherwise the `fabric resume` command itself, which then holds the lock until the harness exits instead of for the duration of a short command. There is no third case. `resume` refuses with `NoSupervisor` when it can neither reach a running node over the socket nor take the lock, and it never launches a harness and exits.

The harness process is spawned by the CLI, never by the daemon, because an interactive harness such as Claude Code needs the user's terminal and the user's shell environment, and a daemon has neither. The CLI spawns it in a new process group, makes that group the terminal's foreground group, and waits for it. Before the harness starts, the CLI registers it with the supervisor, which writes `<fabric-data>/jobs/<job-id>/harness` by temp, `fsync`, rename:

```text
HarnessRecord { epoch: Epoch, supervisor_pid: u32, running: RunningHarness }

RunningHarness =
  | Spawned  { pid: u32, pgid: u32, started_at: u64 }   started by a Fabric process on this node
  | External { pid: u32, pgid: u32, started_at: u64 }   started by the user, adopted by `fabric job init` (ADR 0001)
  | Daemon   { session_id: String }                     owned by the harness's own supervisor; has no pid Fabric may kill
```

`started_at` is the kernel's start time of the process, so a pid reused after the harness exited is never killed. The supervisor's duties are three. It renews the lease every 20 seconds while the recorded process is alive, and stops renewing when it exits: a lease with no harness behind it is allowed to expire, and `fabric suspend` is what releases it. It kills on fencing: on learning a higher epoch for the job, from a peer message or from its own ledger, it sends `killpg(SIGKILL)` to the recorded group; for `Daemon` it calls the adapter's `stop` (ADR 0006) until `detect` reports nothing running, because a session under the harness's own supervisor is restarted after a plain kill (research, step1 adversarial review, C6). For that reason the Claude Code adapter launches a foreground `claude` in v0, never `claude --bg`. And it rewrites the record when the harness exits.

The record is also how a supervisor that died comes back. Every command that opens the store reads `harness` first. If `supervisor_pid` is dead and the recorded process is alive, the command re-adopts it as the new supervisor when the lease is still `Held` by this node at the recorded epoch, and kills it otherwise. Between the supervisor dying and the next Fabric command on that node, an unsupervised harness is in the same position as a harness on a sleeping node: nobody renews, a peer may force a claim after expiry, and whatever it writes is preserved by the orphan path. Running `fabric node serve` shrinks that window to the daemon's restart time, and the record is what lets the restarted daemon find the process. `fabric status` prints the record and whether its supervisor is alive.

### Expiry and clock skew

A lease is considered expired by a claimant only when:

```text
now_claimant > expires_at + skew_margin
```

where `expires_at` is the latest value the claimant has seen for the current epoch, from the claim record or a pushed renewal, `skew_margin` is fixed at 30 seconds in v0, the lease length is 60 seconds and renewal runs every 20 seconds. A claimant that has lost contact with the holder sees only the last renewal that reached it, so a sleeping holder looks expired to its peer about 60 seconds after its last successful push, and a reachable holder never does. `--force-expired` prints the local `expires_at`, the time of the last message from the holder and the holder's node id, so the user can judge. Clocks are used only for this one comparison; the grant rule reads heads and lease states, never clocks, so a skewed clock cannot make a claim fail. What it can do is make a node misjudge expiry. The node process therefore compares its clock with the peer's `Hello` at connection time, and when they differ by more than 10 seconds the warning is printed before `--force-expired` is offered, so the user judges the expiry with that fact in view.

### What a claim after expiry means

If node A held the lease at epoch 1, went to sleep without suspending, and node B claims epoch 2 after expiry, then:

- B resumes from the latest snapshot B can fetch. If A never published one, B resumes from the previous snapshot and the ledger says so with a `Note`.
- Work A did after its last snapshot is not lost. It is on A's disk. When A wakes it learns epoch 2, is fenced, and the supervisor stops A's harness. A's unpublished workspace is kept in place and reported as an orphan: `fabric status` lists it, `fabric orphan export` turns it into a snapshot that is not the job's head and pins it in the store so `store gc` keeps it (ADR 0004). Nothing is deleted automatically. Ever.
- The same holds for what the harness wrote outside the workspace. A fenced Claude Code harness also wrote its transcript under the user's Claude projects directory, and a later `resume` of the job on A would place the job's newer transcript over it. `fabric orphan export` therefore calls the adapter's `capture` as well and stores the result as the orphan snapshot's `harness_state`, and an adapter's `restore` refuses with `HarnessStateDiverged` rather than overwrite state that is not already contained in what arrives (ADR 0006). The message names `fabric orphan export`.

### Two-node limitation, stated plainly

With two nodes and no quorum, a network partition where both nodes are awake and both think the other is dead can produce two claims. Neither node can tell the difference between "peer is asleep" and "peer is unreachable". v0 therefore:

- never claims after expiry automatically. Expiry-based claims require the user to run `fabric resume --force-expired`, and the command prints what it is about to do and what will be orphaned.
- the normal path is suspend then resume, where the release is explicit and the claim is granted by the previous holder, which is the arbiter.

The most common sleep case has no expiry at all: the holder suspended cleanly, appended `LeaseReleased`, pushed the record and the snapshot, and closed the lid. It is now the arbiter and unreachable, and nothing is held. A claim against an unreachable arbiter whose lease is `Released` is allowed without a flag, under two conditions checked on the claimant: its replica head is that `LeaseReleased` record, and the snapshot the matching `Suspended` names is complete in its local store. Then the claimant appends `LeaseClaimed { epoch: current + 1, origin: Forced }` and a `Note` saying the arbiter was unreachable, and prints both. If either condition fails it refuses with `ArbiterUnreachable { node }` and says that the node holds the latest records or snapshot; `--force-expired` does not apply, because nothing is expired. This is safe for live copies, since a released lease has no harness behind it, and it cannot resume stale work, since the claimant holds the head and the snapshot it names. It can still fork if the arbiter wakes, resumes locally without hearing of the claim, and grants itself the same epoch; the fork rule below decides that, and rule 2 favors the arbiter.

`--force-expired` is the one append a non-arbiter may make: the claimant appends `LeaseClaimed { epoch: current + 1, origin: Forced }` with `prev` set to its replica head, then a `Note` saying the claim was forced and against which `expires_at`. A claim the arbiter appends carries `origin: Granted`. A forced claim is therefore the only way two ledgers can fork. When the old holder reconnects it is offered the forced record. If the record's `prev` equals the old holder's head, it appends it and is fenced. If not, the old holder appended at the old epoch while it was unreachable and the chains have forked.

**Fork rule.** Both nodes compute the winner from the two suffixes alone, with no message and no clock, and arrive at the same answer. Compare the highest `LeaseClaimed` in each suffix:

1. The higher epoch wins.
2. At equal epoch, `Granted` beats `Forced`. The arbiter's grant was legitimate at the time; the forced claim was a guess that the arbiter was dead.
3. If both are `Forced`, the lower `NodeId`, compared as bytes, wins.

Two suffixes cannot both hold a `Granted` claim at the same epoch, because only the arbiter grants and the arbiter is one node. The order is total, so there is never a tie. The losing node stops its harness (the kill of Fencing, above), moves its records after the fork point to `ledger.forked-<hex of fork head>` beside the ledger, lists every snapshot those records published as an orphan snapshot and pins it (ADR 0004), marks its workspace as an orphan workspace, then appends the winning suffix. Nothing is deleted.

The case that motivated rule 2: A holds epoch 1 and is partitioned. B runs `--force-expired` and gets `Forced` epoch 2. Meanwhile the user on A suspends, which appends `LeaseReleased { 1 }`, and then resumes on A, where A as arbiter grants itself `Granted` epoch 2. Both chains now hold a claim at epoch 2 and both harnesses run. On reconnection rule 2 picks A; B kills its harness and B's work since the fork is an orphan snapshot. If A had cycled twice and reached epoch 3, rule 1 picks A for the same reason. The user is told which side lost and where its work is, in both cases.

Quorum (three or more nodes, or a small witness) is a later ADR. The epoch and fencing design does not change when quorum arrives; only the claim rule does.

### Idempotency

`suspend` and `resume` are safe to run twice. A second `suspend` on an already released job is a no-op that prints the state. A `suspend` interrupted anywhere after the harness stopped is a sequence the next command finishes, keyed on the marker and the ledger head (ADR 0004, Suspend); it never captures twice and never relaunches in between. A second `resume` on the node that already holds the lease does not restore again: the workspace marker (ADR 0004) shows the snapshot is already there, with or without edits since, and only the harness is relaunched if it is not running.

## Alternatives considered

1. **Tombstone the source after a successful import** (T3 Code). Depends on a message that can be lost; that is exactly the two-copies bug. Rejected.
2. **Lock file on a shared filesystem.** Needs a shared filesystem. Rejected by the no-vendor-dependency rule and because sleep makes NFS locks lie.
3. **Last writer wins with CRDT merge of workspaces.** Merging two agents' edits to the same files is a conflict resolution product, not a mover. Rejected.
4. **Always require quorum from day one.** Three nodes before the first gate passes is too much; the gate is two Macs. Deferred, with the claim rule isolated so it can change.
5. **No lease, user discipline only** ("just do not resume in two places"). This is the status quo of every tool in the landscape and the thing that burns users. Rejected.

## Consequences

- The lease state machine is small enough to property test exhaustively. Arbitrary interleavings of claim request, grant, lost reply, retry, renew, release, expire, crash, message loss and forced claim must keep three invariants: for any epoch the job's chain holds at most one `LeaseClaimed`; the chain never has two `Held` states naming different nodes without a `LeaseReleased` or a higher-epoch claim between them; the epoch never decreases. The only divergence between two nodes' views is the one a forced claim creates, and it resolves by the fork rule. The same test drives the fork rule with the named interleavings: a forced claim against a holder that later self-reclaims at the same epoch, two forced claims at the same epoch, and a forced claim against a holder that cycles to a higher epoch; in every run both nodes compute the same winner, exactly one harness survives, and every snapshot on the losing suffix is pinned.
- Every per-job network message carries an `epoch` in its envelope (ADR 0005). The only frame without one is `Hello`, which names no job.
- The orphan path is a real feature with a real command, not an error message.
- Users will sometimes be told "no" (stale epoch, lease held). The message must say which node holds the lease and when it expires.

## Open questions

- Lease length and skew margin are guesses. Measure on the real two-Mac setup and adjust once.
- The renewal file is not part of the hash chain, so a renewal cannot be proven after the fact. That is accepted because v0 never acts on expiry without the user; revisit if automatic failover arrives.
- Witness design for quorum: a third Mac, a tiny VPS, or a phone? Later.
