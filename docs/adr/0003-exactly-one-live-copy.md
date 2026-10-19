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
- A claim is a request to the arbiter, never a local append. The arbiter appends `LeaseClaimed` with `epoch = previous_epoch + 1` on the claimant's behalf (Claim protocol, below). Epochs never repeat and never go backwards.
- `suspend` releases the lease: the holder appends `LeaseReleased` at the current epoch. The next claim gets `epoch + 1`.
- The holder renews before expiry by appending a fresh `LeaseClaimed` at the same epoch with a later `expires_at`. Renewal does not bump the epoch.

### Claim protocol

Two per-node ledgers can disagree only if two nodes append. So only one node may append a claim, and it is always the same one: the arbiter.

- The arbiter for a job is the node that wrote the latest `LeaseClaimed` or `LeaseReleased`. Before any claim it is `created_by`. After a release the arbiter is the node that released, which is also the node that holds the latest snapshot.
- A claim is `ClaimRequest { job, node: claimant, expected_head }` sent to the arbiter. `expected_head` is the hash of the last ledger record the claimant has seen, and the message's `epoch` is the epoch of that record.
- The arbiter grants only if `expected_head` equals its own head and its lease state is `Free` or `Released`. It appends `LeaseClaimed { node: claimant, epoch: current + 1, expires_at }`, `fsync`s the ledger, and only then replies `Granted { record }` with the exact record bytes. Otherwise it replies `Rejected { head, lease }` and appends nothing.
- The claimant appends the record it was sent, as a replica of the arbiter's record, and only then restores and launches. If `Granted` is lost, the claimant retries the identical request. The arbiter's ledger already names the claimant at the current epoch, so it replies `Granted` again with the same record. The retry is idempotent.
- `fabric resume` on the arbiter itself goes through the same function without a network round trip: the arbiter consults only its own ledger.
- A node that is not the arbiter never appends a claim on its own. If the arbiter is unreachable there is no claim. The only way past that is `--force-expired`, which is a user action (below).

The property test in Consequences is written against this protocol: with one arbiter and lossy messages, no interleaving of request, grant, lost reply and retry produces two `LeaseClaimed` records at one epoch.

### Fencing

Every message between nodes about a job carries the sender's epoch. Every write (ledger append, blob push, manifest update) carries it too. A node that knows a higher epoch for that job refuses the message with `StaleEpoch { yours, current }`. There is no override flag.

A harness launched at epoch N is killed by the node if the node learns of epoch N+1. The node checks the epoch before every write it performs on the harness's behalf.

### Expiry and clock skew

A lease is considered expired by a claimant only when:

```text
now_claimant > expires_at + skew_margin
```

with `skew_margin` fixed at 30 seconds in v0, and a lease length of 60 seconds with renewal every 20 seconds. Clocks are used only for this one comparison. A node whose clock is more than `skew_margin` off from its peer will see its claims refused and must say so to the user rather than retry. (The node process compares its clock with the peer's at connection time and reports a warning if they differ by more than 10 seconds.)

### What a claim after expiry means

If node A held the lease at epoch 1, went to sleep without suspending, and node B claims epoch 2 after expiry, then:

- B resumes from the latest snapshot B can fetch. If A never published one, B resumes from the previous snapshot and the ledger says so with a `Note`.
- Work A did after its last snapshot is not lost. It is on A's disk. When A wakes it learns epoch 2, is fenced, and the node stops A's harness. A's unpublished workspace is kept in place and reported as an orphan: `fabric status` lists it, `fabric orphan export` turns it into a snapshot that is not the job's head. Nothing is deleted automatically. Ever.

### Two-node limitation, stated plainly

With two nodes and no quorum, a network partition where both nodes are awake and both think the other is dead can produce two claims. Neither node can tell the difference between "peer is asleep" and "peer is unreachable". v0 therefore:

- never claims after expiry automatically. Expiry-based claims require the user to run `fabric resume --force-expired`, and the command prints what it is about to do and what will be orphaned.
- the normal path is suspend then resume, where the release is explicit and the claim is granted by the previous holder, which is the arbiter.

`--force-expired` is the one append a non-arbiter may make: the claimant appends `LeaseClaimed { epoch: current + 1 }` with `prev` set to its replica head, then a `Note` saying the claim was forced and against which `expires_at`. It is therefore the only way two ledgers can fork. When the old holder reconnects it is offered the forced record. If the record's `prev` equals the old holder's head, it appends it and is fenced. If not, the old holder appended at the old epoch while it was unreachable and the chains have forked. The chain that contains the higher epoch claim is the job's chain. The losing node moves its records after the fork point to `ledger.forked-<hex of fork head>` beside the ledger, lists every snapshot those records published as an orphan snapshot, then appends the winning suffix. Nothing is deleted.

Quorum (three or more nodes, or a small witness) is a later ADR. The epoch and fencing design does not change when quorum arrives; only the claim rule does.

### Idempotency

`suspend` and `resume` are safe to run twice. A second `suspend` on an already released job is a no-op that prints the state. A second `resume` on the node that already holds the lease replays the restore steps, which are idempotent (ADR 0004), and relaunches the harness if it is not running.

## Alternatives considered

1. **Tombstone the source after a successful import** (T3 Code). Depends on a message that can be lost; that is exactly the two-copies bug. Rejected.
2. **Lock file on a shared filesystem.** Needs a shared filesystem. Rejected by the no-vendor-dependency rule and because sleep makes NFS locks lie.
3. **Last writer wins with CRDT merge of workspaces.** Merging two agents' edits to the same files is a conflict resolution product, not a mover. Rejected.
4. **Always require quorum from day one.** Three nodes before the first gate passes is too much; the gate is two Macs. Deferred, with the claim rule isolated so it can change.
5. **No lease, user discipline only** ("just do not resume in two places"). This is the status quo of every tool in the landscape and the thing that burns users. Rejected.

## Consequences

- The lease state machine is small enough to property test exhaustively: arbitrary interleavings of claim, renew, release, expire and crash must never produce two `Held` states at the same epoch and must never decrease the epoch.
- Every network message type has an `epoch` field. There is no "control" channel without one.
- The orphan path is a real feature with a real command, not an error message.
- Users will sometimes be told "no" (stale epoch, lease held). The message must say which node holds the lease and when it expires.

## Open questions

- Lease length and skew margin are guesses. Measure on the real two-Mac setup and adjust once.
- Should renewal be a ledger event or a separate small file? Events are durable but grow the ledger by one record every 20 seconds while running. Decide in the core stage after measuring; a renewal file that is not part of the chain is acceptable because the chain only needs the claim.
- Witness design for quorum: a third Mac, a tiny VPS, or a phone? Later.
