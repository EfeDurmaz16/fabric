# Authority

The authority is the one place that accepts commands for a [managed job](managed-job.md), records them, and decides which execution of it is current ([ADR 0020](../adr/0020-managed-authority-journal.md)).

A standalone job has no separate authority: the node that holds its lease decides. That works for two machines one person owns, but a forced steal during a network partition can leave two live copies until the nodes meet again. The managed authority is built so that no executor decides on its own.

On `main`:

- The authority runs inside the local `fabric serve` node, with one serialized writer: one task owns the journal and applies commands one at a time. There is no cluster and no consensus.
- Its journal is one SQLite file under `<state>/managed/`. Accepted changes are events whose rows carry their canonical bytes and the digest of the previous event, so the history is tamper-evident; triggers refuse an update or a delete of an event row.
- Every command carries an operation key. The authority stores the answer each key got, so the same key and command sent again get the same answer and no second effect, and a changed body under the same key is refused.
- A command names the work revision it expects, and is refused when the job has moved on.

What it does not claim on `main`: survival of the host it runs on. The journal lives on the same host as the executor, so losing the host loses both. Placing work on other nodes is M5 (PR #38, a draft branch), and running it across a partition is M6, on a branch, not on `main`.
