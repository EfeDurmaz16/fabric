# ADR 0002: Job ledger and record

Status: accepted, 2026-10-19

Amended 2026-10-19 after step 1 research. The event kinds and the acceptance rule follow the move protocol of ADR 0003 (`2026-10-19-step1-design-input.md`, section 4.8 and D11): `LeaseClaimed` and `LeaseReleased` are replaced by `MoveStarted`, `MoveImported`, `MoveAborted` and `Stolen`, there is no epoch 0 and no free lease, and the arbiter is gone. Nothing else in this ADR changed.

Amended 2026-10-19 while writing `fabric-core` (PR `feat/core`). Four corrections of fact, each found by making the types compile and the replay test pass:

1. `Stolen` carries `by` as well as `from`. The first text named only the stolen-from node, so a replay from the file alone could not say who owns the job after the steal; the thief was known only to the transport. The record is now `Stolen { by, from, reason }` and `by` is the writer.
2. A new kind, `EnvSpecChanged { env_spec }`, written by the owner at its epoch. Without it the manifest's `env_spec` could not be rebuilt from the ledger, which this ADR promises for every manifest field. ADR 0007's "a `Note` in the ledger" for an edited spec is this record.
3. `JobManifest` gains `created_at` and `updated_at`, the `at` of the first and the last record. Informational, like `at` itself.
4. The record framing is made exact: `length` counts the bytes after itself (`format_version`, the postcard payload and the 16 checksum bytes); the checksum is the first 16 bytes of blake3 over `length | format_version | payload`; `prev` and the manifest's `head` are blake3 over the whole frame. `MoveStarted` is accepted only when the head is `Suspended`, which is step 1 of the move in ADR 0003 stated as a rule the ledger enforces.

Amended 2026-10-19 after the adversarial review of PR `feat/core`. Eight corrections, each with the test that pins it:

1. The length field has its own check. One flipped bit in the length of a middle record made `scan` report a torn tail at that offset, so a store following the torn-tail rule would have truncated every later record, `MoveImported` included. The frame is now `u32 length | u32 header_check | u8 format_version | postcard(Event) | checksum`, where `header_check` is the first 4 bytes of blake3 over the length field and `checksum` is the first 16 bytes of blake3 over everything before it. A header that does not verify is corruption, never a torn tail. A record is at most 1 MiB (`MAX_RECORD_LEN`, the control frame bound of ADR 0005); a header that verifies and declares more is refused before any allocation, in a file and in a frame from a peer.
2. The torn-tail rule below is restated with the header: a last record whose header verifies and whose declared end is at or past end of file is torn. The at-end-of-file case is the power-loss shape on APFS and ext4 where the inode size and the first page landed and the pages after it read back as zero; it is kept as torn, at the stated cost that a complete last record whose checksum fails is dropped too. A peer that holds that record offers it again on the next connection, because it links to the truncated head. The property test has no `Ok` arm any more: a flipped bit anywhere in a non-tail record is corruption at that offset.
3. `Created` carries `job_id`. Without it a ledger could not prove which job it was: records of job X delivered under job Y's directory replayed cleanly and minted fencing tokens for Y. `replay` and `open` take the job the caller expects and refuse a mismatch with `WrongJob`. `MoveOffer` carries `job_id` for the same reason (ADR 0003).
4. A decoder refuses bytes after the value. postcard ignores trailing bytes, so one event had many valid frames and a manifest with junk appended decoded to a value whose `id()` differed from blake3 of the bytes. Every decode in the crate is exact; `decode(bytes).id() == blake3(bytes)` holds for manifests and one event has one frame hash.
5. The format version is the first varint of a manifest's bytes, not a field of the struct. `JobManifest`, `SnapshotManifest` and `EnvSpec` have no `version` member; `encode` writes the constant and `decode` reads and checks it, so a wrong version cannot exist in memory and `UnknownVersion` is a decode-only error. The bytes did not change.
6. Who delivered a record is one of two things, and the ledger says which checks each gets. `Author::Node(peer)` is a record from a transport-proved peer, who must be the node the writer rule names. `Author::Relayed` is a record fetched from a paired peer that is not necessarily its writer, which happens whenever a replica fetches a suffix holding another node's records from the owner; it gets the link, epoch and content checks and not the sender check, because records carry no signature. A relayed record is trusted as far as the paired peer is. Per-record signatures by node key, already an open question below, are the way to close this and are one more field in a new format version. There is no `OwnFile`: a node's own file is replayed with the same checks as a relayed suffix.
7. Two content rules were missing: `SnapshotPublished.parent` must equal the latest published snapshot (`WrongParent`), and a `Resumed` that names a snapshot must name the latest one (`UnknownSnapshot`). A record whose epoch is above the one the rules require is `AheadOfUs`, as the fencing token already distinguishes, instead of `StaleEpoch`.
8. `Epoch` zero is refused on the wire, not only by `Epoch::new`. The record bytes of every event kind and of the job manifest are pinned by a test, as the snapshot manifest's already were, so a reordered variant or field is a failing test.

## Context

A job must be readable after any crash, on any node, by any future version of Fabric. It must also show what happened, in order, so that a user on a second machine can see why a job is in the state it is in. The T3 Code store is an event-sourced SQLite file with 64 migrations that the research judged unsafe to write from outside (`2026-10-19-t3code-vs-fabric.md`, section 5a). We want the opposite: a format small enough to describe on one page and safe to read with a hex editor.

## Decision

### Identifiers

- `JobId`, `EventId`, `MoveId`: ULID (128 bits, 48-bit millisecond timestamp plus 80 random bits, lexicographically sortable). Generated on the node that creates the job, event or move. No central allocator.
- `SnapshotId`, `ChunkId`: blake3 hash of the bytes (32 bytes).
- `NodeId`: ed25519 public key (ADR 0005).
- `Epoch`: `u64` (ADR 0003).

### The ledger

The ledger is an append-only file of events. One file per job per node, at `<fabric-data>/jobs/<job-id>/ledger`.

Each record on disk:

```text
u32 length | u32 header_check | u8 format_version | postcard(Event) | blake3(all before)[0..16]
```

`length` counts the bytes after the header. `header_check` is blake3 of the length field, first 4 bytes. A frame is at most 1 MiB.

```text
Event {
  id:    EventId,
  prev:  Option<[u8; 32]>,   blake3 of the previous record's bytes; None only for Created
  epoch: Epoch,              the lease epoch under which this event was written; 1 for Created
  at:    u64,                unix milliseconds, informational only, never used for ordering
  kind:  EventKind,
}

EventKind =
  | Created { job_id: JobId, goal: String, harness: HarnessKind, created_by: NodeId }   created_by owns the job at epoch 1; job_id is checked on every replay
  | MoveStarted { move_id: MoveId, target: NodeId }                           by the owner, at its epoch
  | MoveImported { move_id: MoveId, from: NodeId }                            by the target, at epoch + 1
  | MoveAborted { move_id: MoveId }                                           by the target, at the owner's epoch
  | Stolen { by: NodeId, from: NodeId, reason: String }                      by `by`, the new owner, at epoch + 1, behind --force
  | SnapshotPublished { snapshot: SnapshotId, parent: Option<SnapshotId> }
  | Suspended { snapshot: SnapshotId }
  | Resumed { node: NodeId, snapshot: Option<SnapshotId>, harness: HarnessKind }   None only for the Resumed that `fabric job init` appends
  | EnvSpecChanged { env_spec: EnvSpecId }                                   by the owner, at its epoch
  | DecisionRecorded { text: String }
  | Note { text: String }
```

Rules:

- Append only. No record is ever rewritten.
- Hash chained. `prev` makes insertion in the middle detectable. This is cheap tamper evidence, not security against a malicious node.
- Durable before visible. Every append is one `write` of the whole record followed by a full flush of the file: `fsync` on Linux, `fcntl(F_FULLFSYNC)` on macOS, where plain `fsync` leaves the bytes in the drive's write cache (ADR 0004, publish). The first append to a new ledger file is followed by the same flush on `jobs/<job-id>/`, since a new file is durable only once its directory is. A record is not reported to any caller and not sent to any peer before the flush returns. The `MoveImported` or `MoveAborted` a target writes is fsynced before `MoveDecision` leaves the node; otherwise a crash between the two forgets the decision while the source acts on it.
- Checksummed per record, header and body. On open, the reader walks forward. A failing record is a torn tail only if it is the last thing in the file: its header verifies and its declared end is at or past end of file, its header is cut short, or every byte from its start to end of file is zero. A header that does not verify is corruption wherever it is. A torn tail is truncated and the records before it are valid. Any other failure is corruption, not a crash: open fails with `LedgerCorrupt { offset }`, the file is not modified and the manifest is not rebuilt. A record in the middle is never dropped to make the rest readable, because the dropped record may be the `MoveImported` or `Stolen` that fences a live copy. `fabric job repair` is a later command; v0 refuses and says where.
- Owned by one writer at a time. While the lease is `Owned`, only the owner appends, and the kinds it may write are `MoveStarted`, `SnapshotPublished`, `Suspended`, `Resumed`, `EnvSpecChanged`, `DecisionRecorded` and `Note`. While the lease is `Departing`, only the target named by the open `MoveStarted` appends, and the only kinds it may write are `MoveImported` and `MoveAborted` for that `move_id`. `Stolen` is the one record a node writes without being the owner or the target (ADR 0003, The steal). Every other node holds a replica: a byte-for-byte copy of the writer's records, in the writer's order.
- Accepted from a peer only when it links. A replica appends a peer record only if `record.prev` equals the replica's head. Then `MoveImported` and `Stolen` must carry `epoch == current_epoch + 1`, every other kind must carry `epoch == current_epoch`, and the record must come from the node the previous rule allows to write at that moment when the transport proved the sender (`Author::Node`); a suffix relayed by another paired peer (`Author::Relayed`) skips only that sender check (amendment 6 above). A record whose `prev` does not match is answered with `Rejected { head }` and the sender fetches the missing suffix; if neither head is a prefix of the other the ledgers have forked and ADR 0003 says which chain wins. A record with any other epoch is answered with `StaleEpoch { yours, current }` and dropped.
- Ordered by file position, not by `at` or by ULID. Clocks are not trusted for ordering.

### The job manifest

A derived, versioned summary that any node can rebuild from the ledger. It exists so that `fabric status` does not replay the file, and so that a peer can fetch "the head" in one message.

```text
JobManifest {                    the bytes start with the format version as a varint; it is not a field
  job_id:          JobId,
  goal:            String,
  harness:         HarnessKind,
  env_spec:        Option<EnvSpecId>,
  latest_snapshot: Option<SnapshotId>,
  lease:           LeaseState,
  head:            [u8; 32],     blake3 of the last ledger record
  event_count:     u64,
  created_at:      u64,          the `at` of Created
  updated_at:      u64,          the `at` of the last record
}
```

Written to `<fabric-data>/jobs/<job-id>/manifest` by temp, `fsync`, rename, then a full flush of `jobs/<job-id>/`, after the append it summarizes. Every rename into `jobs/<job-id>/` (the manifest, the marker and the harness record of ADR 0003 and ADR 0004) ends with that directory flush, because the rename itself is not durable until its directory is. If the manifest and ledger disagree on open: when the ledger is longer, the manifest was not rewritten after the last append (the normal crash window) and it is rebuilt from the ledger. When the manifest's `event_count` is larger than the ledger's, or its `head` is not a record in the ledger, the node fails closed with `LedgerBehindManifest` and modifies nothing. A shorter ledger never wins over a manifest that remembers a later epoch.

### Opaque per-harness state

Each snapshot carries a `harness_state` tree (ADR 0004). The core copies it, hashes it and restores it. It never parses it. Only the adapter for that harness kind reads it (ADR 0006). This is the property that lets the core stay small while adapters change weekly.

### Serialization

postcard for the ledger and manifests. It is compact, deterministic and `no_std`, but it is not self-describing, so:

- every record and every manifest carries an explicit `format_version`, as the first thing in its bytes;
- a decoder refuses bytes after the value and bytes that are not what the encoder writes for the value they decode to (postcard reads an overlong varint as the same number), so a value has one byte form and its hash is the hash of that value alone; inside a record frame such bytes are corruption at that offset;
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
- Property tests will drive the record codec with arbitrary bytes and truncations; crash tests will `kill -9` a writer mid-append and assert the reader recovers every complete record. Power loss is not something a test can stage on a laptop; the full-flush calls are asserted by a unit test over the platform shim that records which `fcntl` and `fsync` calls were issued and on which descriptors, one case per commit point. One test flips a byte in a middle record and asserts that open fails with `LedgerCorrupt` and the file is byte-identical afterwards; one truncates the manifest's ledger behind its `event_count` and asserts `LedgerBehindManifest`.

## Open questions

- Do we need per-event signatures by node key for multi-user jobs later? Not for v0 (one user, own machines). The hash chain leaves room: a signature is one more field in a new format version.
- Ledger compaction: never for v0. A job that has thousands of events is a job that has been resumed thousands of times; that is fine.
