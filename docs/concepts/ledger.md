# Ledger

A job's ledger is its append-only, hash-chained log of events: created, run, suspended, move started, move imported, stolen, approved, and the rest. Every event names the previous one by hash, so a ledger cannot be edited without the chain showing it, and events are never edited or deleted ([ADR 0002](../adr/0002-job-ledger-and-record.md)).

The ledger is what keeps exactly one live copy of a standalone job ([ADR 0003](../adr/0003-exactly-one-live-copy.md)). A move is a short protocol over it:

1. The owner suspends and copies the ledger and the snapshot to the target.
2. The owner appends `MoveStarted`. From here it is not runnable.
3. The target appends its one decision for that move id, `MoveImported` or `MoveAborted`, and the owner appends the same.

The target writes one decision per move id and answers every later question about that move with the same record, so a lost or repeated answer, or `kill -9` of either node at any step, cannot leave two runnable copies. To prove it, the end-to-end test arms each fault point below (`crates/fabricd/src/faults.rs`): the node stops there and the test kills it or its peer with `SIGKILL`, or the node drops or repeats the answer it was about to send. The list is generated from that file and the tests that arm the points:

<!-- faults:begin -->
- `after_quiesce`: The source stopped the harness and has written nothing yet.
- `after_snapshot_publish`: The snapshot is published in the store and the ledger does not name it yet.
- `after_departing_write`: `MoveStarted` is durable and no offer was sent.
- `mid_transfer`: The target stored some of a snapshot's chunks and not all.
- `before_import`: A pulling node holds the owner's `MoveStarted` toward it and has not imported yet.
- `after_import_receipt`: The target made `MoveImported` durable and did not answer yet; on the source, the answer just arrived.
- `before_harness_record`: A CLI asked to record the harness it launched, and the node has written nothing for it yet.
- `before_placement`: An accepted managed create is about to be placed, and no decision is recorded yet.
- `before_launch`: A grant is committed and its launch is not sent yet.
- `while_stopping`: `fabric serve` was told to stop and still holds the state directory.
<!-- faults:end -->

Each `MoveImported` or `Stolen` record raises the job's **epoch** by one. Anything that carries an older epoch is stale and is refused; see [fence](fence.md).

`fabric job log <job>` prints the ledger. When two chains of one job exist after a forced steal, the job freezes and `fabric job repair` settles it by the fork rule.
