# ADR 0020: The managed authority, its journal and fencing eligibility

Status: proposed, 2026-10-27

Adopts the parts of the external draft P02 ("Independent authority and fencing", 2026-10-25) that the M2 campaign needs, with its handoff selection of 2026-10-27, and the M2 boundary of draft P08 ("Versioned promotion, durability and rollback"). ADR 0003 stays the rule for standalone jobs. The managed jobs of ADR 0019 follow this ADR instead. The schema and the replay rules are in [docs/managed-jobs.md](../managed-jobs.md).

## Context

A standalone job's authority is its owner node: the node that holds the lease at the current epoch decides, and a cooperative move hands the lease over (ADR 0003). That is right for two machines one person owns, and it has two limits the audit recorded as release boundaries, not bugs (annex, daemon review R1 and R3): a forced steal during a partition can leave two live copies until the nodes meet, and a frozen fork has no repair command. A durable job that is independent of the machine running it needs one place that accepts commands, records progress and decides which execution is current, and that place must not be any one executor's opinion.

M2 does not build that service. It builds the contract and its first implementation on one host, so that a later campaign can move the authority off the executor without changing what a command, an operation or a generation means.

## Decision

### One authority, one writer

A managed job is bound to exactly one authority. In M2 the authority runs inside the local `fabric serve` daemon, with one serialized writer: a single task owns the journal connection and applies commands one at a time. There is no cluster, no consensus and no second writer. Authority unavailability blocks new commands; it never lets an executor decide on its own.

The writer's thread starts with the daemon only when a journal exists, and recovers it before it answers any call. Without a journal it starts with the first managed command, so a daemon that never runs a managed job pays one `stat` at start and neither the thread nor its queue; until then a read answers that there is no managed job, under the lock a command starts the thread under. A journal whose presence cannot be read counts as one that does not open: every managed command and every lookup of an id fails closed with `managed_unavailable`.

The authority does not claim executor-loss durability in M2. Its journal lives on the same host as the executor, so losing the host loses both. The capability is advertised only once the journal and the checkpoint artifacts are stored somewhere that survives the executor (draft P02 handoff selection).

### A dedicated, versioned SQLite journal

The journal is its own SQLite database, `<state>/managed/journal.sqlite`, separate from the store's catalog. The catalog is reconstructible bookkeeping for content-addressed objects (ADR 0014); the journal is the authoritative record of managed jobs, and the two never share a file, a schema or a transaction. The journal reuses the workspace's existing `rusqlite` dependency (0.40, bundled SQLite) and the catalog's discipline: WAL, `synchronous = FULL`, `fullfsync = ON`, foreign keys on, built under a temporary name and renamed into place. No new crate enters the workspace for it.

The file is versioned twice: a fixed `application_id`, so a file that is not a Fabric journal is refused, and `user_version`, the schema version. Version 1 is the first. Opening a journal whose version is newer than the binary knows is refused without a write. Migrations run forward only, one transaction each, and are tested from every earlier version's fixture.

Accepted changes are events in an append-only table whose rows carry canonical bytes, their digest and the digest of the job's previous event, so a job's history is tamper-evident as the ledger is. Triggers refuse an update or a delete of an event row. The job, task, attempt and operation tables are projections the authority keeps in the same transaction; they can be rebuilt from the events, and a rebuild that disagrees with the stored projection is an error that stops the authority rather than a silent repair.

### Command ordering and acknowledgement

For every command the authority, in one `BEGIN IMMEDIATE` transaction, deduplicates the operation key, checks the expected revision, appends the events, updates the projections and stores the operation's result (ADR 0019). It answers only after `COMMIT` returns. Intent is recorded before any effect is dispatched to an executor; an effect whose outcome is not observed stays `unknown`.

### Grants and generations

An executor acts for a managed job only under a grant: the job, its generation, the attempt, the executor and the scope of the canonical workspace. The generation increases by one whenever the authority grants a new attempt on the canonical execution domain. Exactly one grant is current per job. An observation, a checkpoint or a completion that names an older generation is refused as `StaleGeneration` and recorded as such; it never changes progress.

Fencing eligibility in M2 is cooperative and local:

- A new grant follows only a verified stop of the previous attempt, or an attempt that never started. A timeout, a lost heartbeat or a dead control connection is not a verified stop.
- A lost worker makes its attempt `lost`. It does not cancel the task, and it does not by itself allow a replacement grant. Starting a new attempt after a loss is an explicit command, refused while the previous attempt's process group may still run.
- There is no automatic failover. A later campaign may add it per resource only with proof that the previous execution is terminated or that every covered mutation is fenced (draft P02 invariants).

### Standalone jobs and promotion

Standalone jobs keep their ledger files, their snapshots and their behavior. The journal never writes into a job directory, and a ledger is never rewritten to fit the managed schema. A `JobId` lives in one mode only: creating a managed job refuses an id that has a ledger, and the standalone path refuses an id the journal holds.

Promotion of a standalone job into a managed one is out of M2. It needs its own gate: an executable seal that old readers refuse, a crash-safe transition and rollback tests (draft P08). Importing a standalone snapshot as the initial checkpoint of a new managed job is allowed; it makes a distinct `JobId` and does not stop, seal or revoke the standalone job, so any duplicate effect between the two stays the user's explicit choice.

### Local only

The first control surface is the existing owner-only control socket (ADR 0016, ADR 0018). No managed operation is reachable from a peer or a network listener in M2; a remote API waits for draft P07 and its authentication decision.

## Alternatives considered

1. Make the store's catalog the journal. Rejected: the catalog is reconstructible bookkeeping with its own migrations; mixing authoritative job history into it ties two lifecycles together and lets a catalog repair touch job truth.
2. A second append-only file beside each ledger. Rejected: the dedupe index, the revision check and the projections would need their own crash-safe update protocol; SQLite already gives an atomic multi-table commit the workspace trusts.
3. A server database or an HA cluster. Rejected for M2: it adds a dependency and an operations burden before the single-writer contract is proven.
4. Trust a heartbeat timeout to grant a replacement. Rejected: it violates the partition safety rule of ADR 0003 and draft P02.

## Consequences

- The daemon hosts a second kind of state with its own backup and restart tests. A backup is a SQLite online backup of the journal, taken by the authority between commands.
- Standalone behavior, ledger bytes and snapshot formats are unchanged, and legacy fixtures are part of the M2 test suite.
- Hot paths of standalone jobs must not slow down: M2 adds no work to suspend, move, resume, launch or CLI start of a standalone job, and the time budget (`budget.toml`) and `bench::m2_paths` measure that.

## Open questions

- Where the authority runs once it leaves the executor's host, and how it is backed up there.
- Authentication and expiry of grants, for the remote API.
- Which resources can prove termination or fencing well enough for automatic recovery.
