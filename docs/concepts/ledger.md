# Ledger

A job's ledger is its append-only, hash-chained log of events: created, run, suspended, move started, move imported, stolen, approved, and the rest. Every event names the previous one by hash, so a ledger cannot be edited without the chain showing it, and events are never edited or deleted ([ADR 0002](../adr/0002-job-ledger-and-record.md)).

The ledger is what keeps exactly one live copy of a standalone job ([ADR 0003](../adr/0003-exactly-one-live-copy.md)). A move is a short protocol over it:

1. The owner suspends and copies the ledger and the snapshot to the target.
2. The owner appends `MoveStarted`. From here it is not runnable.
3. The target appends its one decision for that move id, `MoveImported` or `MoveAborted`, and the owner appends the same.

The target writes one decision per move id and answers every later question about that move with the same record, so a lost or repeated answer, or `kill -9` of either node at any step, cannot leave two runnable copies. The end-to-end test breaks the move at five named points to prove it.

Each `MoveImported` or `Stolen` record raises the job's **epoch** by one. Anything that carries an older epoch is stale and is refused; see [fence](fence.md).

`fabric job log <job>` prints the ledger. When two chains of one job exist after a forced steal, the job freezes and `fabric job repair` settles it by the fork rule.
