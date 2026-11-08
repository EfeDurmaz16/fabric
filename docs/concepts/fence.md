# Fence

To fence is to refuse work that comes from a stale copy. Fabric fences by a counter that only goes up, and refuses anything that carries an older value.

**Standalone jobs: the epoch.** A job's lease has an epoch that starts at 1 and rises by one on every `MoveImported` or `Stolen` record. The fencing token is `(job, epoch)`. A message or a write carrying an older epoch is refused, and a node that learns a higher epoch for a job whose harness it runs stops that harness. The harness's own stores are fenced by native tombstones where the harness has one ([ADR 0003](../adr/0003-exactly-one-live-copy.md)).

**Managed jobs: the generation.** A managed job's generation rises by one with every attempt the authority grants. An observation, a checkpoint or a completion that names an older generation is refused as `StaleGeneration` and recorded; it never changes progress ([ADR 0020](../adr/0020-managed-authority-journal.md)). A provider operation is fenced on the job's current live attempt.

On `main`, epoch fencing is proved end to end with two nodes and `kill -9`. Generation fencing is enforced by the journal and the core's transition rules, and tested there; fencing a running attempt's writes after a real partition (ledger append, bundle publish, journal operation) is M6 work on a branch, proved only in-process so far.
