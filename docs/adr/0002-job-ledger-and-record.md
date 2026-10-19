# ADR 0002: Job ledger and record

Status: accepted, 2026-10-19

## Context

A job must be readable after any crash, on any node, by any future version of Fabric. It must also show what happened, in order, so that a user on a second machine can see why a job is in the state it is in. The T3 Code store is an event-sourced SQLite file with 64 migrations that the research judged unsafe to write from outside (`2026-10-19-t3code-vs-fabric.md`, section 5a). We want the opposite: a format small enough to describe on one page and safe to read with a hex editor.

## Decision

### Identifiers

- `JobId`, `EventId`: ULID (128 bits, 48-bit millisecond timestamp plus 80 random bits, lexicographically sortable). Generated on the node that creates the job or event. No central allocator.
- `SnapshotId`, `ChunkId`: blake3 hash of the bytes (32 bytes).
- `NodeId`: ed25519 public key (ADR 0005).
- `Epoch`: `u64` (ADR 0003).

### The ledger

The ledger is an append-only file of events. One file per job per node, at `<fabric-data>/jobs/<job-id>/ledger`.

Each record on disk:

```text
u32 length | u8 format_version | postcard(Event) | blake3(prev 3 fields)[0..16]
```

```text
Event {
  id:    EventId,
  prev:  Option<[u8; 32]>,   blake3 of the previous record's bytes; None only for Created
  epoch: Epoch,              the lease epoch under which this event was written; 0 while the lease is Free
  at:    u64,                unix milliseconds, informational only, never used for ordering
  kind:  EventKind,
}

EventKind =
  | Created { goal: String, harness: HarnessKind, created_by: NodeId }
  | LeaseClaimed { node: NodeId, expires_at: u64 }
  | LeaseReleased
  | SnapshotPublished { snapshot: SnapshotId, parent: Option<SnapshotId> }
  | Suspended { snapshot: SnapshotId }
  | Resumed { node: NodeId, snapshot: SnapshotId, harness: HarnessKind }
  | DecisionRecorded { text: String }
  | Note { text: String }
```

Rules:

- Append only. No record is ever rewritten.
- Hash chained. `prev` makes insertion in the middle detectable. This is cheap tamper evidence, not security against a malicious node.
- Durable before visible. Every append is one `write` of the whole record followed by `fsync` of the file. A record is not reported to any caller and not sent to any peer before `fsync` returns. The `LeaseClaimed` an arbiter writes is fsynced before `Granted` leaves the node; otherwise a crash between the two forgets the grant while the claimant proceeds.
- Checksummed per record. On open, the reader walks forward. A failing record is a torn tail only if it is the last thing in the file: its declared end is at or past end of file, or every byte from its start to end of file is zero. A torn tail is truncated and the records before it are valid. Any other failure is corruption, not a crash: open fails with `LedgerCorrupt { offset }`, the file is not modified and the manifest is not rebuilt. A record in the middle is never dropped to make the rest readable, because the dropped record may be the claim that fences a live copy. `fabric job repair` is a later command; v0 refuses and says where.
- Owned by one writer. Only the lease holder appends events. The one exception is `LeaseClaimed`, which the arbiter appends on the claimant's behalf (ADR 0003, Claim protocol). Every other node holds a replica: a byte-for-byte copy of the owner's records, in the owner's order.
- Accepted from a peer only when it links. A replica appends a peer record only if `record.prev` equals the replica's head. Then a `LeaseClaimed` must carry `epoch == current_epoch + 1`, and every other kind must carry `epoch == current_epoch` and come from the node allowed to write at that moment: the holder while the lease is `Held`, and the arbiter (ADR 0003) while it is `Free` or `Released`, when the only kinds it may write are `Note` and `DecisionRecorded`. A `Note` before the first claim and a `DecisionRecorded` after a release are both ordinary records. A record whose `prev` does not match is answered with `Rejected { head }` and the sender fetches the missing suffix; if neither head is a prefix of the other the ledgers have forked and ADR 0003 says which chain wins. A record with any other epoch is answered with `StaleEpoch { yours, current }` and dropped.
- Ordered by file position, not by `at` or by ULID. Clocks are not trusted for ordering.

### The job manifest

A derived, versioned summary that any node can rebuild from the ledger. It exists so that `fabric status` does not replay the file, and so that a peer can fetch "the head" in one message.

```text
JobManifest {
  version:         u16,          format version of this struct
  job_id:          JobId,
  goal:            String,
  harness:         HarnessKind,
  env_spec:        Option<EnvSpecId>,
  latest_snapshot: Option<SnapshotId>,
  lease:           LeaseState,
  head:            [u8; 32],     blake3 of the last ledger record
  event_count:     u64,
}
```

Written to `<fabric-data>/jobs/<job-id>/manifest` by temp, `fsync`, rename, after the append it summarizes. If the manifest and ledger disagree on open: when the ledger is longer, the manifest was not rewritten after the last append (the normal crash window) and it is rebuilt from the ledger. When the manifest's `event_count` is larger than the ledger's, or its `head` is not a record in the ledger, the node fails closed with `LedgerBehindManifest` and modifies nothing. A shorter ledger never wins over a manifest that remembers a later epoch.

### Opaque per-harness state

Each snapshot carries a `harness_state` tree (ADR 0004). The core copies it, hashes it and restores it. It never parses it. Only the adapter for that harness kind reads it (ADR 0006). This is the property that lets the core stay small while adapters change weekly.

### Serialization

postcard for the ledger and manifests. It is compact, deterministic and `no_std`, but it is not self-describing, so:

- every record and every manifest carries an explicit `format_version`;
- a reader that sees a version it does not know fails with a clear error instead of guessing;
- field order is append-only within a version; a new version is a new struct.

## Alternatives considered

1. **SQLite for the ledger.** Good transactions, but then the ledger is a schema with migrations, and reading it from another language or a future version means carrying those migrations forever. The T3 Code store is the cautionary example (t3code note, section 5a). SQLite is used for the chunk catalog (ADR 0004) where its indexes earn their keep.
2. **JSON lines.** Self-describing and easy to inspect, three to five times larger, and float and string escaping differ across libraries. Rejected for the ledger. A `fabric ledger dump` command will print JSON for humans.
3. **UUIDv7 instead of ULID.** Equivalent properties. ULID has a stable Rust crate (`ulid` 3.0, 2026-07) and a shorter text form. No strong reason either way; picking one.
4. **Mutable job state file only, no ledger.** Simpler, but loses the "why is it in this state" story and makes two-node reconciliation guesswork. Rejected.

## Consequences

- A job's full history is one flat file. Copying a job is copying a directory.
- Clock skew cannot reorder events. It can only make `at` look odd, which is accepted.
- Adding an event kind is a format version bump. That is deliberate friction.
- Property tests will drive the record codec with arbitrary bytes and truncations; crash tests will `kill -9` a writer mid-append and assert the reader recovers every complete record. One test flips a byte in a middle record and asserts that open fails with `LedgerCorrupt` and the file is byte-identical afterwards; one truncates the manifest's ledger behind its `event_count` and asserts `LedgerBehindManifest`.

## Open questions

- Do we need per-event signatures by node key for multi-user jobs later? Not for v0 (one user, own machines). The hash chain leaves room: a signature is one more field in a new format version.
- Ledger compaction: never for v0. A job that has thousands of events is a job that has been resumed thousands of times; that is fine.
