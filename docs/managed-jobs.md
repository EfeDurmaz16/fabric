# Managed jobs: identity, journal v1, operation states and replay

Status: proposed with ADR 0019 and ADR 0020, 2026-10-27; implemented on the M2 branch the same day; on `main` since 2026-11-03 (PR #31), with schema version 2 (PR #32) beside it. This page is the M2 specification and records where the implementation differs from the first draft ("As built" notes below). The code is the authority on signatures: `crates/fabric-core/src/managed/`, `crates/fabric-journal/` and `crates/fabricd/src/authority.rs`. The section "Reconciliation and cleanup (M4)" was added 2026-10-29 with ADR 0022 and implemented in Fabric the same day; it is on `main` since 2026-11-08 (PR #36), and its "As built (M4)" notes record where the code differs. The section "Placement and planned continuity (M5)" was added 2026-11-03 with ADRs 0023 to 0027 and implemented on the M5 branch; its "As built (M5)" notes record where the code differs. The section "Unexpected-loss recovery (M6)" was added 2026-11-02 with ADR 0028 and implemented on the M6 branch; its "As built (M6)" notes record where the code differs, and its "On the M5 base" notes record what changed when M6 merged M5.

The spans a node exports for these operations, `fabric inspect --json` over the journal's tables and `fabric audit export` of them are in [observability.md](observability.md) (ADR 0031).

## Identity

| Name | Form | Made by | Meaning |
|---|---|---|---|
| `JobId` | ULID, existing type | authority, at creation | The durable work. Never reused, never changes mode. |
| `TaskId` | ULID | authority | One logical piece of a job's work. Every managed job gets its primary task at creation. |
| `AttemptId` | ULID | authority, when it grants | One execution of one task on one resource under one generation. |
| `OperationId` | ULID | authority, at commit | One accepted command. |
| `OperationKey` | 1 to 128 bytes of `[A-Za-z0-9._:-]` | caller | The caller's name for one command; the dedupe key. A ULID is a good key. |
| `RequestDigest` | 32 bytes | authority | blake3 of `"fabric.managed.command.v1\0"` followed by the canonical bytes of the envelope's job, expected revision and command (not the key). |
| `WorkRevision` | u64, at least 1 | authority | Accepted changes to one job. Creation is revision 1. |
| `Generation` | u64, at least 1 | authority | The fencing generation of the job's canonical execution domain. |
| `EventSeq` | u64, at least 1 | journal | The global order of events in one journal. |

On the client API (ADR 0018) `WorkRevision`, `Generation` and `EventSeq` travel as canonical decimal strings, not JSON numbers, so a JavaScript reader never rounds them. An id travels in its text form, as every id does today.

`ResourceRef` names where an attempt runs. In M2 it has one form, the local node: `{ "kind": "node", "node": "<NodeId>" }`. Providers add their forms in M3 (draft P09), always as an open tagged object that an older reader refuses rather than guesses.

## Rust types (fabric-core, `managed` module)

Pure: no IO, no clock, no randomness. Ids and times come in as arguments.

```rust
pub struct TaskId([u8; 16]);
pub struct AttemptId([u8; 16]);
pub struct OperationId([u8; 16]);
pub struct OperationKey(String);
pub struct RequestDigest([u8; 32]);
pub struct WorkRevision(NonZeroU64);
pub struct Generation(NonZeroU64);
pub struct EventSeq(NonZeroU64);

pub enum JobMode { Standalone, Managed }

pub struct ManagedJob {
    pub id: JobId,
    pub revision: WorkRevision,
    pub generation: Generation,
    pub intent: Intent,
    pub source: SourceBinding,
    pub policy: JobPolicy,
    pub state: JobState,
    pub primary: TaskId,
}

pub struct Intent { pub goal: String, pub harness: HarnessKind }
pub enum SourceBinding { Repository { path: AbsPath }, Snapshot { snapshot: SnapshotId } }
pub struct JobPolicy { pub restart: RestartPolicy }
pub enum JobState { Active, Finished, Cancelled }

pub struct Task {
    pub id: TaskId,
    pub job: JobId,
    pub parent: Option<TaskId>,
    pub kind: TaskKind,
    pub restart: RestartPolicy,
    pub state: TaskState,
}
pub enum TaskKind { Primary, Child { label: String } }
pub enum RestartPolicy { Block, ExplicitOnly }
pub enum TaskState { Open, Succeeded, Failed, Cancelled }

pub struct Attempt {
    pub id: AttemptId,
    pub task: TaskId,
    pub job: JobId,
    pub resource: ResourceRef,
    pub generation: Generation,
    pub initial_checkpoint: Option<SnapshotId>,
    pub status: AttemptStatus,
}
pub enum ResourceRef { Node { node: NodeId } }
pub enum AttemptStatus { Granted, Running, Succeeded, Failed, Cancelled, Checkpointed { snapshot: SnapshotId }, Lost, Unknown }

pub struct Operation {
    pub id: OperationId,
    pub key: OperationKey,
    pub job: JobId,
    pub expected_revision: Option<WorkRevision>,
    pub digest: RequestDigest,
    pub status: OperationStatus,
}
pub enum OperationStatus { Accepted, Dispatched, Succeeded, Failed, Cancelled, Rejected { refusal: Refusal }, Unknown }

pub enum Command {
    CreateJob { intent: Intent, source: SourceBinding, policy: JobPolicy },
    StartAttempt { task: TaskId, resource: ResourceRef, initial_checkpoint: Option<SnapshotId> },
    RegisterTask { parent: TaskId, kind: TaskKind, restart: RestartPolicy },
    FinishTask { task: TaskId, outcome: TaskOutcome },
    CancelJob { reason: String },
}
pub enum TaskOutcome { Succeeded, Failed, Cancelled }

pub enum Observation {
    Started { attempt: AttemptId, generation: Generation },
    Checkpointed { attempt: AttemptId, generation: Generation, snapshot: SnapshotId },
    Exited { attempt: AttemptId, generation: Generation, outcome: ExitOutcome },
    Lost { attempt: AttemptId, generation: Generation },
    Unknown { operation: OperationId },
}
pub enum ExitOutcome { Succeeded, Failed { code: Option<i32> }, Cancelled }

pub struct Envelope<T> { pub key: OperationKey, pub job: Option<JobId>, pub expected: Option<WorkRevision>, pub body: T }
pub struct AcceptedOperation { pub operation: OperationId, pub job: JobId, pub revision: WorkRevision, pub events: Vec<ManagedEvent> }
pub enum Dedupe { New, Replay { stored: StoredResult }, Conflict }
pub struct Transition { pub revision: WorkRevision, pub events: Vec<ManagedEvent> }

pub enum ManagedEvent {
    JobCreated { job: JobId, intent: Intent, source: SourceBinding, policy: JobPolicy, primary: TaskId },
    TaskRegistered { task: TaskId, parent: Option<TaskId>, kind: TaskKind, restart: RestartPolicy },
    AttemptGranted { attempt: AttemptId, task: TaskId, resource: ResourceRef, generation: Generation, initial_checkpoint: Option<SnapshotId> },
    AttemptObserved { attempt: AttemptId, generation: Generation, status: AttemptStatus },
    TaskFinished { task: TaskId, outcome: TaskOutcome },
    JobCancelled { reason: String },
    StaleObservationRefused { attempt: AttemptId, generation: Generation, current: Generation },
}

pub enum DomainError {
    OperationConflict { key: OperationKey },
    RevisionMismatch { expected: WorkRevision, current: WorkRevision },
    StaleGeneration { given: Generation, current: Generation },
    UnknownTask { task: TaskId },
    UnknownAttempt { attempt: AttemptId },
    AttemptStillLive { attempt: AttemptId },
    TaskClosed { task: TaskId },
    WrongMode { job: JobId, mode: JobMode },
    BadParent { task: TaskId },
}

pub fn request_digest(command: &Command) -> RequestDigest;
pub fn dedupe(stored: Option<&Operation>, digest: &RequestDigest) -> Dedupe;
pub fn accept_command(job: Option<&ManagedState>, envelope: &Envelope<Command>, ids: &NewIds) -> Result<Transition, DomainError>;
pub fn apply_observation(job: &ManagedState, observation: &Observation) -> Result<Transition, DomainError>;
pub fn replay(events: &[ManagedEvent]) -> Result<ManagedState, DomainError>;

pub struct NewIds { pub job: JobId, pub task: TaskId, pub attempt: AttemptId, pub operation: OperationId, pub at_ms: u64 }
pub struct ManagedState { pub job: ManagedJob, pub tasks: Vec<Task>, pub attempts: Vec<Attempt> }
```

Codec: every `ManagedEvent` and `Command` encodes with the existing canonical postcard codec of ADR 0002 (`fabric_core::codec`, exact decode, re-encode compare), prefixed by a format byte `1`. An event's digest is blake3 of its frame.

As built (fabric-core `managed`), where the code differs from the sketch above:

- The digest covers the envelope's `job` and `expected` revision as well as the command, so the same key with another expected revision is a conflict, not a replay.
- `ManagedEvent` has two more kinds. `CommandAccepted { operation, key, digest, kind }` opens every batch a command makes, so the history says which operation made which events and the operations' progress replays from events. `OperationMarked { operation, status }` records `dispatched`, an outcome or `unknown`.
- `Attempt` records `granted_by: OperationId`; `ManagedState` also holds `operations: Vec<OperationProgress>`. A command without an effect (every one but `StartAttempt`) is `succeeded` once committed; a grant is `accepted` until it is dispatched.
- `OperationStatus::Rejected` holds the `DomainError` itself, and `StoredResult` is `Accepted(AcceptedOperation) | Rejected(DomainError)`. `dedupe` returns `New`, `Replay` or `Conflict`; the journal reads the stored bytes.
- The stored frame is `EventFrame { job, revision, operation, prev, at_ms, event }`: `prev` is inside the hashed bytes, so the chain covers every field.
- `replay(job, &[(WorkRevision, ManagedEvent)])` takes each event with its revision; `apply_batch(state, revision, events)` folds one committed batch and is what both the journal and `replay` use. `mark_operation(state, operation, mark)` is the third rule function beside `accept_command` and `apply_observation`.
- Every committed change, a command, an observation that changes something or an operation mark, takes the next revision. A report that repeats what the job holds changes nothing and writes nothing.
- First grant is generation 1; each later grant is the latest plus one. `StartAttempt`, `FinishTask` and `CancelJob` wait while any attempt is `granted`, `running` or `unknown`. `DomainError` gained `UnknownJob`, `UnknownOperation`, `AttemptEnded`, `JobClosed`, `BadEnvelope`, `OperationState`, `Exhausted` and `Broken`.

## Journal adapter (`fabric-journal` crate, new, no new external dependency)

```rust
pub struct Journal { /* one rusqlite::Connection, owned by one writer task */ }
pub struct JournalPath(PathBuf);
pub enum OpenError { NotAJournal, NewerSchema { found: u32, known: u32 }, Corrupt { reason: String }, Sqlite(rusqlite::Error), Io(std::io::Error) }
pub enum CommitError { Domain(DomainError), Sqlite(rusqlite::Error), ProjectionMismatch { job: JobId } }
pub enum Committed { New(AcceptedOperation), Replayed(StoredResult), Rejected(Refusal) }

impl Journal {
    pub fn create(path: &JournalPath, authority: NodeId) -> Result<Journal, OpenError>;
    pub fn open(path: &JournalPath) -> Result<Journal, OpenError>;
    pub fn schema_version(&self) -> u32;
    pub fn submit(&mut self, envelope: Envelope<Command>, clock_ms: u64, ids: NewIds) -> Result<Committed, CommitError>;
    pub fn observe(&mut self, job: JobId, observation: Observation, clock_ms: u64) -> Result<Transition, CommitError>;
    pub fn job(&self, job: JobId) -> Result<Option<ManagedState>, CommitError>;
    pub fn operation(&self, key: &OperationKey) -> Result<Option<Operation>, CommitError>;
    pub fn events(&self, job: JobId, after: Option<EventSeq>, limit: u32) -> Result<Vec<(EventSeq, ManagedEvent)>, CommitError>;
    pub fn verify(&self) -> Result<VerifyReport, CommitError>;
    pub fn backup_to(&self, target: &Path) -> Result<(), CommitError>;
}
```

`fabricd` owns one `Journal` in an authority task and feeds it through a bounded channel; the control socket's managed ops send to that channel and wait for its answer. Nothing else opens the file.

As built (fabric-journal): `Journal::create(path, authority, at_ms)`, `open(path)`, `schema_version()`, `authority()`, `submit(&envelope, ids) -> Answer { result, bytes, replayed }`, `observe(job, &observation, at_ms)`, `mark(job, operation, mark, at_ms)`, `job(job)`, `contains(job)`, `jobs()`, `operation(&key)`, `events(job, after, limit) -> Vec<StoredEvent>`, `verify() -> VerifyReport`, `recover(at_ms, probe) -> Recovery` and `backup_to(target)` (`VACUUM INTO`). `OpenError` is `NotAJournal`, `NewerSchema`, `Exists`, `Sqlite`, `Io`; `CommitError` is `Domain`, `Sqlite`, `Encode`, `Overflow`, `Corrupt`, `ProjectionMismatch`. The `SQLite` error stays opaque, as the store's does. `fabricd`'s authority is one OS thread fed by a bounded queue of 64 calls.

## Journal schema, version 1

```sql
PRAGMA application_id = 1178748490;  -- 0x4642524A, "FBRJ"
PRAGMA user_version = 1;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA fullfsync = ON;
PRAGMA foreign_keys = ON;

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;
-- rows: authority_node, created_at_ms, codec_format = '1'

CREATE TABLE jobs (
  job_id       BLOB PRIMARY KEY CHECK (length(job_id) = 16),
  mode         TEXT NOT NULL CHECK (mode = 'managed'),
  revision     INTEGER NOT NULL CHECK (revision >= 1),
  generation   INTEGER NOT NULL CHECK (generation >= 1),
  state        TEXT NOT NULL CHECK (state IN ('active','finished','cancelled')),
  primary_task BLOB NOT NULL,
  intent       BLOB NOT NULL,
  source       BLOB NOT NULL,
  policy       BLOB NOT NULL,
  head_seq     INTEGER NOT NULL,
  created_ms   INTEGER NOT NULL,
  updated_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE tasks (
  task_id     BLOB PRIMARY KEY CHECK (length(task_id) = 16),
  job_id      BLOB NOT NULL REFERENCES jobs(job_id),
  parent_id   BLOB REFERENCES tasks(task_id),
  kind        TEXT NOT NULL,
  restart     TEXT NOT NULL CHECK (restart IN ('block','explicit_only')),
  state       TEXT NOT NULL CHECK (state IN ('open','succeeded','failed','cancelled')),
  created_rev INTEGER NOT NULL
) STRICT;

CREATE TABLE attempts (
  attempt_id         BLOB PRIMARY KEY CHECK (length(attempt_id) = 16),
  task_id            BLOB NOT NULL REFERENCES tasks(task_id),
  job_id             BLOB NOT NULL REFERENCES jobs(job_id),
  generation         INTEGER NOT NULL CHECK (generation >= 1),
  resource           TEXT NOT NULL,
  initial_checkpoint BLOB CHECK (initial_checkpoint IS NULL OR length(initial_checkpoint) = 32),
  status             TEXT NOT NULL CHECK (status IN ('granted','running','succeeded','failed','cancelled','checkpointed','lost','unknown')),
  last_checkpoint    BLOB,
  granted_ms         INTEGER NOT NULL,
  ended_ms           INTEGER,
  UNIQUE (job_id, generation)
) STRICT;

CREATE TABLE operations (
  operation_id      BLOB PRIMARY KEY CHECK (length(operation_id) = 16),
  op_key            TEXT NOT NULL UNIQUE,
  request_digest    BLOB NOT NULL CHECK (length(request_digest) = 32),
  job_id            BLOB REFERENCES jobs(job_id),
  kind              TEXT NOT NULL,
  expected_revision INTEGER,
  status            TEXT NOT NULL CHECK (status IN ('accepted','dispatched','succeeded','failed','cancelled','rejected','unknown')),
  result            BLOB NOT NULL,
  first_seq         INTEGER,
  accepted_ms       INTEGER NOT NULL,
  updated_ms        INTEGER NOT NULL
) STRICT;

CREATE TABLE events (
  seq         INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id      BLOB NOT NULL REFERENCES jobs(job_id),
  revision    INTEGER NOT NULL,
  operation_id BLOB REFERENCES operations(operation_id),
  kind        TEXT NOT NULL,
  body        BLOB NOT NULL,
  digest      BLOB NOT NULL CHECK (length(digest) = 32),
  prev_digest BLOB CHECK (prev_digest IS NULL OR length(prev_digest) = 32),
  at_ms       INTEGER NOT NULL
) STRICT;
CREATE INDEX events_by_job ON events (job_id, seq);

CREATE TABLE evidence (
  evidence_id BLOB PRIMARY KEY CHECK (length(evidence_id) = 16),
  job_id      BLOB NOT NULL REFERENCES jobs(job_id),
  attempt_id  BLOB REFERENCES attempts(attempt_id),
  revision    INTEGER NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('checkpoint','exit')),
  ref         TEXT NOT NULL,
  digest      BLOB CHECK (digest IS NULL OR length(digest) = 32),
  seq         INTEGER NOT NULL REFERENCES events(seq)
) STRICT;

CREATE TRIGGER events_append_only_update BEFORE UPDATE ON events
  BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER events_append_only_delete BEFORE DELETE ON events
  BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER evidence_append_only_update BEFORE UPDATE ON evidence
  BEGIN SELECT RAISE(ABORT, 'evidence is append-only'); END;
CREATE TRIGGER evidence_append_only_delete BEFORE DELETE ON evidence
  BEGIN SELECT RAISE(ABORT, 'evidence is append-only'); END;
```

As built, version 1 differs from the DDL above in these columns, and `crates/fabric-journal/src/schema.rs` is the text that ships:

- `jobs.primary_task` checks `length = 16`.
- `operations.kind` is checked against the five command kinds, gains `accepted_rev` (the revision the command was accepted at, NULL exactly when the operation is rejected) and an index on `(job_id, first_seq)`.
- `tasks.kind` is `primary` or `child`, and `tasks.kind_body` holds the framed `TaskKind` with its label; an index on `(job_id, created_rev)`.
- `attempts.resource` is the framed `ResourceRef` (a BLOB), `attempts.granted_by` references the granting operation, and `attempts.status_body` holds the framed `AttemptStatus`; `last_checkpoint` checks `length = 32`.
- `events.revision` checks `>= 1`.
- `evidence_id` is the first 16 bytes of the event's digest; an evidence row is written for a checkpoint (`ref` is the snapshot id) and for an attempt that exits (`ref` is the status).

Rules the adapter keeps:

- `events` is the history; `jobs`, `tasks`, `attempts` and the `status` of `operations` are projections, written in the same transaction as the events that change them. `verify` replays every job's events and compares the result with the projection.
- An event's `prev_digest` is the `digest` of the job's previous event; the first event of a job has none. `verify` checks the chain.
- `operations.result` holds the canonical bytes of the answer the caller got, so a replayed key gets the same bytes.
- `UNIQUE (job_id, generation)` on `attempts` makes two grants at one generation impossible at the storage level, not only in the domain.
- A projection row is written only by `write_transition`, which folds the events with the core's `apply_batch` in the same transaction.

### Version 2: provider operations

Version 2 (M3, ADR 0021) adds one table, `provider_operations`, and changes nothing in version 1's tables. Efe approved the table on 2026-10-29. Version 2 has not shipped in any release. A provider operation is an effect an attempt asks a provider worker for over the bridge (docs/bridge-protocol.md): its own `operation_id` and `op_key`, never one `operations` holds or the other way round, the request digest, the job, task and attempt it acts for and the attempt's generation, the bridge action, the `target` locator Fabric sent the action to (written once at dispatch, null for a create), a `status` of `dispatched`, `succeeded`, `failed`, `cancelled`, `rejected` or `unknown`, the last answer's `code`, `reason` and `effect`, and the last `locator` the worker reported. Both locators are stored as their JSON text, which the journal never parses. The target is what keeps an operation lost before any answer, such as a destroy whose worker died, tied to the resource it acted on.

- `provider_begin` commits the row `dispatched` before the request frame is written, or `rejected` when Fabric refused the request before sending it. It refuses a key or id the journal holds, a job that is not active, and an attempt that is not the job's current grant, `granted` or `running` at the job's generation.
- `provider_record` moves `dispatched` to any answer, `unknown` only to `succeeded`, `failed` or `cancelled`, and never moves a terminal status. Recording the status a row has stores the answer and the locator alone, as a `pending` answer does.
- `recover` makes every provider operation left `dispatched` `unknown` and lists it in `Recovery::provider_unknown`; nothing is sent again.

M4 (ADR 0022) amends this table in place, since version 2 has never shipped: it gains the columns and statuses of "Reconciliation and cleanup (M4)" below, and `tests/fixtures/v2.sqlite` is regenerated with them.

### Versioning and migration

- `create` builds the file as `journal.sqlite.tmp`, sets the pragmas, runs the version 1 DDL, every migration and the `meta` rows in one transaction, closes it, renames it into place and syncs the directory, as `Catalog::create` does.
- `open` checks `application_id` (else `NotAJournal`), then `user_version`: equal to the binary's version opens; lower runs each migration `n -> n + 1` in its own `BEGIN IMMEDIATE` transaction that ends by setting `user_version = n + 1`; higher is `NewerSchema` and nothing is written.
- Every released version keeps a fixture database under `crates/fabric-journal/tests/fixtures/v<n>.sqlite`; the tests open each with the current binary and run `verify`.
- A migration never rewrites an event's `body` or `digest`. A new event shape is a new `kind`, or a new codec format byte read beside the old one.

### Idempotency keys from command ids

- The CLI makes a fresh ULID per invocation and passes it as the key; `--operation-key <key>` lets a script choose its own so a retry after a lost answer is the same command. As built (2026-10-28): `fabric managed create --repo <repo> --goal <goal> [--harness <h>] [--operation-key <key>]`, `fabric managed cancel <job> [--expected-revision <n>] [--reason <text>] [--operation-key <key>]`, `fabric managed status <job>` and `fabric managed ls` (`crates/fabric/src/managed.rs`). A command run without `--operation-key` prints the key it made on standard error, so a script can retry with it; a reply answered again for a key says so. The client API and both SDKs carry the key too.
- The client API takes `operation_key` as a required field on every managed op; the SDKs generate one when the caller gives none and expose it, so the caller can retry with it. As built the Rust client takes the key as an argument, and the TypeScript SDK takes `operationKey` and exports `newOperationKey()`; without one it makes a fresh key and the reply names it.
- Keys are global to one journal in M2, since the only principal is the local owner. When remote principals arrive (draft P07) the key's scope becomes `(principal, key)` in a migration.
- A key is kept as long as its job's events are kept. M2 deletes nothing.

## Durable operation states

```text
            submit
caller ---------------> [dedupe] --- same key, same digest ---> stored answer (no new commit)
                            |  --- same key, other digest ---> OperationConflict
                            v
                     [revision check] --- mismatch ---> rejected (stored under the key)
                            |
                            v
                       accepted  (committed; the answer is sent now)
                            |
               dispatch to executor (an effect may now happen)
                            v
                       dispatched
               /        |        \           \
        succeeded    failed    cancelled    unknown
```

- `accepted`: the command is durable and nothing was dispatched.
- `dispatched`: recorded before the executor is asked; an effect may have happened from here on.
- `succeeded`, `failed`, `cancelled`: an observation said so, at the current generation.
- `rejected`: refused before any effect. Terminal.
- `unknown`: the authority cannot tell; a timeout, a lost answer from the executor or an authority restart while `dispatched` lands here. Only an observation moves it on.

Attempt status follows the same rule: `granted` (recorded, not started), `running` (observed started), then `succeeded`, `failed`, `cancelled`, `checkpointed`, `lost` or `unknown`.

## Replay rules

1. On open the authority runs `verify`. A broken chain, an event that does not decode or a projection that disagrees stops the authority with an error naming the job; it does not repair or drop rows.
2. An operation left `accepted` at restart is still accepted: the command was durable, and its dispatch is the next step, done once. A grant is offered for dispatch, and may be marked `dispatched`, only while its attempt is still `granted` at the job's current generation. A grant whose attempt recovery found `lost` or `running`, or that a newer generation replaced, is never dispatched.
3. An operation left `dispatched` at restart becomes `unknown` in a new event. It is not dispatched again.
4. An attempt left `granted` or `running` at restart stays as it is until the executor reports or a stop is verified. The authority checks the recorded process group (as `local.rs` does for standalone harnesses): a group that is gone is `lost`; a group that still runs is `running`. Neither grants a new attempt. As built, `Journal::recover` takes the check as a probe; no attempt records a process group until an executor reports one (M3), so the daemon's probe answers "cannot tell" and attempts stay as they are.
5. A worker loss is `lost` for the attempt, never `cancelled` for the task. The task stays open.
6. A timeout is never permission to replay an operation or grant a replacement. A replacement attempt is a new `StartAttempt` command from the caller, accepted only when no attempt of the task may still run, and it gets the next generation.
7. An observation at an older generation is recorded as `StaleObservationRefused` and changes nothing else.
8. A duplicate submission after restart is answered from `operations` like any other.

## Compatibility with standalone jobs

- The journal never opens, writes or renames anything under `<state>/jobs/<job>/`. Ledger bytes, snapshot manifests and the store catalog are untouched.
- `fabric job ls` lists standalone jobs from their ledgers and managed jobs from the journal, with the mode.
- A state directory without `managed/journal.sqlite` behaves exactly as today; the journal is created by the first managed command, not by `fabric serve`.
- M2 proves this with frozen fixtures: a state directory with standalone ledgers and snapshots made by the main branch before M2 (commit 176622e) opens, lists, suspends, moves and resumes under the M2 binary, and its files are byte-identical before and after a managed job is created beside it.

As built: `fabric job ls` still lists standalone jobs only; managed jobs are listed by `fabric managed ls`, the client API's `managed_list`. A standalone op that names a managed job is refused as `managed_job`, and a managed op that names a standalone job as `standalone_job`. A peer's push never makes a standalone job under an id the journal holds. The authority publishes the journal's ids once the file opens, before `verify` and recovery, so that check and every standalone lookup of an unknown id never wait behind recovery or a `managed_list`. A journal that does not open leaves its ids unknown, and those lookups fail closed as `managed_unavailable`. The fixture is `crates/fabricd/tests/fixtures/legacy-176622e`, made by the e2e generator with production code byte-identical to 176622e. It leaves out `node.key`, since a private key is not committed, so the node that opens it is a new node: the test opens, lists, reads the lease and the log, steals the job and restores its frozen snapshot whole, rather than suspending and moving it as its owner. Suspend and move of standalone jobs are covered unchanged by the existing e2e suite.

## M2 acceptance mapped to tests

| Acceptance (roadmap M2) | Test |
|---|---|
| Authority restart preserves identity and progress | Create a managed job, grant an attempt, kill the daemon with `SIGKILL` at each journal fault point, restart: same `JobId`, same revision, attempt not re-granted, `dispatched` operations `unknown`. As built: `fabric-journal` `crash_tests` (a child killed at `create.before_rename`, `submit.before_commit`, `submit.after_commit`, `dispatch.after_commit`), `tests::a_restart_keeps_identity_and_progress_and_makes_a_dispatch_unknown`, `fabricd` `authority::tests::a_restart_keeps_identity_progress_and_the_answer_to_a_key`, e2e `managed::e2e_a_managed_job_keeps_identity_and_progress_across_a_daemon_kill`. |
| Duplicate command preserves identity | Submit the same key twice, before and after a restart: one operation, one event set, byte-identical answers. Same key, other body: `OperationConflict`. As built: `fabric-journal` `tests::a_duplicate_command_gets_the_same_identity_and_bytes_before_and_after_a_restart`, `tests::a_refusal_is_stored_under_its_key_and_changes_no_job`, e2e `managed::e2e_managed_refusals_are_kept_and_modes_stay_apart`, the TypeScript e2e `managed_create answers the same operation for the same key`. |
| Crash between journal write and effect | As built: `crash_tests::a_dispatch_killed_between_its_commit_and_its_effect_is_never_replayed` and `a_grant_killed_between_its_commit_and_its_dispatch_is_dispatched_once`. |
| Launch before registration closed (group A, not this core change) | R2 tests (`a_headless_launch_runs_nothing_until_it_is_released`, `the_trait_launch_holds_the_agent_until_it_is_released`) pass, plus a CLI `SIGKILL` between spawn and registration leaves no harness process. As built (integrated 2026-10-27): both pass, and e2e `e2e_a_headless_run_killed_before_its_harness_is_recorded_leaves_nothing_running` and `e2e_an_acp_run_killed_before_its_agent_is_recorded_leaves_nothing_running` kill the CLI with `SIGKILL` while the node holds the registration at `before_harness_record`. |
| Fresh managed identity and replay | `replay(events) == projection` for every job after every test; property test over random command and observation sequences. As built: `fabric-core/tests/managed_rules.rs` (`replay_equals_projection` and the rule tests); `Journal::verify` replays every job in every journal test. |
| Legacy compatibility | The frozen standalone fixture state directory above. As built: e2e `legacy::e2e_a_state_directory_from_main_loads_lists_and_resumes`, `fabricd` `authority::tests::legacy_state_from_main_is_untouched_by_a_managed_job_beside_it`; journal schema fixture `crates/fabric-journal/tests/fixtures/v1.sqlite` with `tests/fixtures.rs`. |
| Promotion separately gated | No command converts a standalone job; a test asserts a managed create of an id with a ledger is refused. As built: the authority draws job ids until one has no standalone directory, and refuses any managed command naming a standalone job (`legacy_state_from_main_is_untouched...`, `e2e_managed_refusals_are_kept_and_modes_stay_apart`). No conversion command exists. |
| No hot-path regression against the anchor (rule of 2026-10-27, which replaced 1.5x: a tolerance per PR compounds), in the release build (amended 2026-10-29: Efe ruled that the rule gates the release build only; the test profile is tracked, not gated) | `scripts/bench-ab.sh --release`: anchor `176622e` against the branch, op by op in the same session, the median pair ratio with a 95% bootstrap interval per op from at least 15 pairs, and counters that do not depend on load; raw runs under `docs/evidence/perf-ab/` and `docs/evidence/m2-review-pr31/`. The first measure, on 2026-10-27, found regressions (headless Claude and Codex launch 1.18 to 1.21, release `cli ls` 1.025, daemon resident size 1.01 to 1.04), fixed since. As measured on 2026-10-29 on the code of `f8742a2` in release (`docs/evidence/m2-review-pr31/ab-vs-anchor-release/`): no op slower; 24 of 26 faster with the interval below 1, and Claude launch 0.968 (0.938 to 1.011) and `cli version` 0.953 (0.849 to 1.104) hold 1; both daemons' resident size 0.87 to 0.89 and physical footprint 0.89 to 0.96 of the anchor's, and every CLI's peak resident size below it. Met. The test profile's runs are kept in the same places as tracked numbers. |

## Implementation owner groups

Disjoint files, so the groups can work in parallel; the defect register in `docs/evidence/m2-baseline/defects.md` uses the same names.

| Group | Files |
|---|---|
| G1 managed domain | `crates/fabric-core/src/managed/*`, `crates/fabric-core/src/lib.rs` (module line), `crates/fabric-core/tests/managed_*.rs` |
| G2 journal | `crates/fabric-journal/*` (new crate, depends on `fabric-core` and the workspace `rusqlite`) |
| G3 authority wiring | `crates/fabricd/src/authority.rs` (new), `crates/fabricd/src/control.rs`, `crates/fabricd/src/node.rs`, `crates/fabric-client/src/api.rs`, `protocol/*`, `sdk/typescript/src/*`, `crates/fabric/src/managed.rs` (new). The one dispatch line for the managed subcommands in `crates/fabric/src/lib.rs` lands after group A's launch change, which owns that file. |

As built, the core change also touched, each in one place: `crates/fabricd/src/ops.rs` (one dispatch arm sending the four managed ops to `crates/fabricd/src/managed.rs`), `crates/fabricd/src/error.rs` (the `NodeError::Managed` variant and its reason), `crates/fabric/src/render.rs` (the reply lines of the four ops), and `crates/fabric/tests/e2e/main.rs` (two module lines). The owners of those files keep them; the lines above are the only ones this change added.

## Reconciliation and cleanup (M4)

Status: proposed with ADR 0022, 2026-10-29; implemented in Fabric the same day (`crates/fabric-journal/src/provider.rs`, `crates/fabric-bridge`), and the Capsule worker's `attach` on Capsule `m4/reconciliation`; the seam is run both ways in docs/evidence/m4. The wire side (the `attach` message, its answers and fixtures) is in docs/bridge-protocol.md, section "Reconciliation (M4)".

Acceptance (roadmap M4): a lost response or a worker restart never triggers a blind duplicate creation; unknown remains explicit; a stop failure blocks an unsafe replacement.

### What M3 already provides

M4 adds only what is missing. These are in the M3 code and tests and do not change:

| Need | M3 mechanism | Pinned by |
|---|---|---|
| Durable intent before any frame | `Journal::provider_begin` commits the row `dispatched` (or `rejected`) in its own `BEGIN IMMEDIATE` transaction; the client records it and queues the frame in one call on the log thread, so the record always precedes the frame | `fabric-journal` `provider::tests::a_key_or_an_id_is_dispatched_once`, `fabric-bridge` `tests/off_runtime.rs`, `tests/process.rs` |
| Nothing sent twice | key and id unique across `operations` and `provider_operations`; `Log(Used)` before any frame | `a_key_or_an_id_is_dispatched_once`, `a_command_cannot_take_a_provider_key` |
| Fencing of effects | `provider_begin` refuses a closed job and any attempt that is not the job's current grant, `granted` or `running`, at the job's generation | `only_the_current_live_attempt_dispatches` |
| Unknown on loss | worker lost, framing broken, deadline plus grace passed, the caller stopped waiting, or authority restarted: `dispatched` becomes `unknown`; `unknown` moves only to an observed terminal state | `statuses_move_only_forward`, `the_target_is_kept_from_dispatch_through_recovery`, scenario `worker_loss_is_not_cancellation`, `tests/process.rs` `a_late_answer_records_the_locator_it_names_and_not_its_status`, `tests/stream.rs` `a_request_whose_caller_stopped_waiting_is_unknown_once_the_worker_is_lost` |
| Target kept from dispatch | the target locator is written once with the dispatch, so a destroy lost before any answer still names its resource | `the_target_is_kept_from_dispatch_through_recovery` |
| Locator as early as known | `progress` and late answers store the locator (`provider_locate`, `record_late`) | `tests/process.rs`, `tests/capsule_worker.rs` |
| Observe by original identity | `Bridge::observe` for `dispatched` or `unknown` only, with the journal's scope, action and target or locator (`ObserveMismatch` otherwise); the worker checks owner labels | scenarios `unknown_after_dispatch_with_locator`, `observe_checks_owner`, `exec_observed_until_exit`, `exec_observe_after_worker_loss` |
| Fresh per-operation approval | the `approval` slot; destroy needs its own grant in both profiles; a create grant or an expired grant is refused before any effect | scenario `fresh_approval_required` |
| Worker-side find by identity | the worker's `findCreated` (fixture: `fx-<key>`; Docker: `docker ps` by the `fabric.operation` label) deduplicates a create; Docker create, start and rm clients run in groups of their own, so losing the worker never aborts an effect | Capsule `packages/bridge-worker` tests, `docker.test.ts` |

What is missing, and what M4 adds:

| Gap in M3 | M4 addition |
|---|---|
| A create lost before any answer has no locator, so it can never be observed | the `attach` message and `Bridge::attach` |
| `unknown` has no exit except an observed terminal state | the statuses `attached`, `observed`, `abandoned` and `Journal::provider_abandon` |
| A runner can create again while a create is `unknown` | `provider_begin` refuses a create while the job holds a create `unknown` |
| `StartAttempt` ignores provider effects | the replacement barrier, `DomainError::ReplacementBlocked` |
| No cleanup once the attempt ended or the job closed | purpose `cleanup`, `Bridge::cleanup`, `Journal::residuals` |
| Locators compared as text, which depends on key order | `ResourceKey`, a digest of the locator's identity |

### The pre-dispatch intent record

The `provider_operations` row, committed before the frame, is the intent. M4 completes it; it stays one write per dispatch.

```rust
// fabric-journal, provider module

/// What an effect does to its resource: the `effect` of its action in the
/// capability profile it was dispatched under.
pub enum EffectClass { Create, Mutate, Destroy }

/// Why a provider operation was dispatched.
pub enum Purpose {
    /// An effect the attempt asked for.
    Effect,
    /// A destroy of the resource that the create `cleans` made (same job).
    Cleanup { cleans: OperationId },
}

/// blake3 of "fabric.bridge.locator.v1\0" followed by the canonical JSON
/// (keys sorted) of a locator's provider, scope, kind and native_id;
/// native_operation_id is left out. Computed by fabric-bridge, compared by
/// the journal, never parsed by it.
pub struct ResourceKey([u8; 32]);

/// A locator as the journal stores it.
pub struct LocatorRecord {
    /// Fabric's canonical JSON of the locator (keys sorted), as it would send it.
    pub text: String,
    /// Its key.
    pub key: ResourceKey,
}

pub struct ProviderDispatch {
    pub operation: OperationId,
    pub key: OperationKey,
    pub digest: RequestDigest,
    pub job: JobId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub generation: Generation,
    /// New: the profile negotiated with the worker the frame goes to.
    pub profile: String,
    pub action: String,
    /// New: from the profile, so the journal never reads a profile.
    pub effect: EffectClass,
    /// New.
    pub purpose: Purpose,
    /// Was `Option<String>`: now the canonical text and its key; `None` for a create.
    pub target: Option<LocatorRecord>,
}
```

Rules at `provider_begin`, in this order, all inside its one transaction, nothing written on a refusal:

1. Key and id unused in `operations` and `provider_operations` (M3).
2. Purpose `effect`: the job is active and the attempt is its current grant, `granted` or `running`, at the job's generation (M3). Purpose `cleanup`: the rules of "Fresh cleanup approval flow" below instead.
3. Effect class `create`: refused `ProviderError::CreateUnresolved { operation }` while any create of the same job is `unknown`.
4. Insert `dispatched`, or `rejected` with Fabric's own refusal.

### Schema version 2, amended

Version 2 has never shipped, so M4 changes its one table in place rather than adding version 3. The table after M4:

```sql
CREATE TABLE provider_operations (
  operation_id   BLOB PRIMARY KEY CHECK (length(operation_id) = 16),
  op_key         TEXT NOT NULL UNIQUE,
  request_digest BLOB NOT NULL CHECK (length(request_digest) = 32),
  job_id         BLOB NOT NULL REFERENCES jobs(job_id),
  task_id        BLOB NOT NULL REFERENCES tasks(task_id),
  attempt_id     BLOB NOT NULL REFERENCES attempts(attempt_id),
  generation     INTEGER NOT NULL CHECK (generation >= 1),
  profile        TEXT NOT NULL,
  action         TEXT NOT NULL,
  effect_class   TEXT NOT NULL CHECK (effect_class IN ('create','mutate','destroy')),
  purpose        TEXT NOT NULL CHECK (purpose IN ('effect','cleanup')),
  cleans         BLOB REFERENCES provider_operations(operation_id),
  target         TEXT,
  target_key     BLOB CHECK (target_key IS NULL OR length(target_key) = 32),
  status         TEXT NOT NULL CHECK (status IN ('dispatched','unknown','attached','observed',
                   'succeeded','failed','cancelled','rejected','abandoned')),
  code           TEXT,
  reason         TEXT,
  effect         TEXT CHECK (effect IS NULL OR effect IN ('none','applied','pending','unknown')),
  locator        TEXT,
  locator_key    BLOB CHECK (locator_key IS NULL OR length(locator_key) = 32),
  searched       TEXT,
  abandon_reason TEXT,
  dispatched_ms  INTEGER NOT NULL,
  updated_ms     INTEGER NOT NULL,
  CHECK ((target IS NULL) = (target_key IS NULL)),
  CHECK ((locator IS NULL) = (locator_key IS NULL)),
  CHECK ((purpose = 'cleanup') = (cleans IS NOT NULL)),
  CHECK (purpose = 'effect' OR effect_class = 'destroy'),
  CHECK ((status = 'abandoned') = (abandon_reason IS NOT NULL)),
  CHECK (status NOT IN ('attached','observed') OR locator IS NOT NULL OR target IS NOT NULL)
) STRICT;
CREATE INDEX provider_operations_by_status ON provider_operations (status, dispatched_ms);
CREATE INDEX provider_operations_by_target ON provider_operations (target_key)
  WHERE target_key IS NOT NULL;
```

- `profile`, `effect_class` and `purpose` are written once at dispatch, like `target` and `target_key`.
- `locator` and `locator_key` are the last locator the worker reported, re-encoded canonically by the bridge before it is stored, so two reports of one resource store equal text.
- `searched` is the canonical JSON of the scope the last `absent` answer searched (`{ "provider", "scope", "kind" }`): an attach of a create, or the observe that settled a destroy (2026-11-02). It is kept for the owner and for the abandonment acknowledgement.
- `abandon_reason` is set exactly when the status is `abandoned`.
- The `by_status` index serves recovery, the barrier and the create guard (they read the few non-settled rows); the partial `by_target` index serves "a destroy of this resource settled `succeeded`". No other index is added, so an effect without a target writes the same index entries as in M3.

### Reconciliation state machine

```text
                     provider_begin (row committed before the frame)
  rejected <---- Fabric refused it before sending
                          |
                          v
                     dispatched ---- result completed / cancelled ------------> settled
                      |      ^ \---- provider_failed; invalid, unsupported,
                      |      |       denied, unavailable ---------------------> settled (failed / rejected)
                      |      +------ pending, result accepted or running,
                      |              progress: locator and answer stored
    unknown answer,   |
    deadline + grace, |
    worker lost,      v
    restart ------> unknown ---- observe or attach result ---------------------> settled
                    |  |  \---- attach pending, or attach unknown with a
                    |  |        locator --------------------------------------> attached
                    |  \------- observe pending ------------------------------> observed
                    |            attached --- observe pending ----------------> observed
                    |            attached, observed --- observe result -------> settled
                    \---------- owner approval (from unknown, attached,
                                 observed) -----------------------------------> abandoned
```

`settled` is one of `succeeded`, `failed`, `cancelled` and `rejected`; `abandoned` and every settled status are terminal. As a table, the status after each event:

| Event | From | To |
|---|---|---|
| `provider_begin` | none | `dispatched`, or `rejected` when Fabric refused it before sending |
| `result` `completed` / `cancelled` to the request | `dispatched` | `succeeded` / `cancelled` |
| `error` `provider_failed` | `dispatched` | `failed` |
| `error` `provider_failed`/`not_found` to a `sandbox.destroy` request | `dispatched` | `unknown`, answer stored: the provider did not find the resource, which does not say it is gone from the scope (2026-11-02, docs/bridge-protocol.md "Absence settles a destroy") |
| `error` `invalid`, `unsupported`, `denied`, `unavailable` | `dispatched` | `rejected` |
| `error` `pending`, `result` `accepted` or `running`, `progress` | `dispatched` | `dispatched`, locator and answer stored |
| `error` `unknown`; no answer by the deadline plus grace; worker lost; the caller dropped the request; `recover` | `dispatched` | `unknown` |
| observe or attach `result` (an observed terminal state) | `unknown`, `attached`, `observed` | `succeeded` or `cancelled` (`failed` stays reachable from these for an observed terminal failure, as in M3) |
| attach `error` `pending`; attach `error` `unknown`/`observation_failed` with a locator | `unknown` | `attached`, locator stored |
| attach `error` `unknown`/`absent` | `unknown` | `unknown`, answer and `searched` stored |
| observe `error` `unknown`/`absent` of a destroy's resource | `dispatched`, `unknown`, `observed` | `succeeded`, answer and `searched` stored: the resource is absent from the scope the destroy was to remove it from (`provider_absent`, 2026-11-02) |
| observe `error` `unknown`/`absent` of anything but a destroy | any | unchanged; `searched` stored for an `unknown` operation |
| observe `error` `pending` | `unknown`, `attached`, `observed` | `observed`, locator stored |
| observe `error` `pending` | `dispatched` | `dispatched`, locator stored (M3, unchanged) |
| observe or attach refused, `unknown` without a new locator, worker lost or timed out during the read | any | unchanged: a read that learned nothing changes nothing |
| observe `error` `unknown`, or the observe lost or timed out, of a `dispatched` operation whose request is in flight on this bridge | `dispatched` | unchanged: that request's own answer, deadline or loss settles it, and a refusal it answers is still recorded `rejected` (`tests/stream.rs` `an_observe_that_learned_nothing_leaves_a_dispatched_request_to_its_answer`) |
| observe `error` `unknown`, or the observe lost or timed out, of a `dispatched` operation with no request in flight (one answered `running`) | `dispatched` | `unknown` (M3, scenario `exec_observe_after_worker_loss`) |
| `provider_abandon` with a matching approval | `unknown`, `attached`, `observed` | `abandoned` |

Meaning of the new states:

- `attached`: the operation's resource was found by its identity after its answer was lost; its locator is now known; its action has not been seen terminal.
- `observed`: after the operation went `unknown`, a read saw its effect in a non-terminal state (a destroy `stopping`, an exec `running`).
- `abandoned`: the owner approved giving up on certainty for this operation. It says nothing about whether the effect happened or stopped.

Restart recovery is unchanged: only `dispatched` becomes `unknown`. `attached`, `observed` and `abandoned` hold no request in flight and stay as they are.

### Which profiles observe or attach

`observe` is supported for an action exactly when its profile entry says `observe: true` (v1). `attach` is supported for an action exactly when its `effect` is `create` and its `dedupe` is `provider_key` or `label_query`; there is no separate profile field. An action outside these is refused before anything is sent, with the code and reason the worker would answer (`unsupported`/`observe`, `unsupported`/`attach`).

| Profile | `sandbox.create` | `sandbox.exec` | `sandbox.destroy` |
|---|---|---|---|
| fixture | observe: yes. attach: yes (`provider_key`: the resource is `fx-<operation key>`; its owner and operation labels must name the attaching job and operation) | observe: yes, from the worker's exec ledger; a restarted worker answers `unknown`. attach: no (`dedupe: none`) | observe: yes (gone is terminal). attach: no (not a create) |
| docker | observe: yes (`docker inspect`; running is terminal). attach: yes (`label_query`: the container labelled `fabric.operation=<operation id>` in the bound engine; its `fabric.job` label must name the attaching job) | observe: yes, from the exec ledger only. attach: no: an exec instance cannot be attached again after its client ended | observe: yes (container gone is terminal). attach: no |

No profile retries a create: Fabric never resends one, whatever its `dedupe`.

### The replacement barrier

```rust
// fabric-journal

/// A provider operation that keeps a replacement attempt from being granted.
pub enum Blocker {
    /// A create that is `dispatched` or `unknown`: its resource cannot be named.
    CreateUnresolved { operation: OperationId, status: ProviderStatus },
    /// An exec not settled (`abandoned` included) whose sandbox has no
    /// destroy settled `succeeded`.
    ExecMayRun { operation: OperationId, status: ProviderStatus },
    /// A destroy `dispatched`, `unknown`, `observed`, `failed`, `cancelled`
    /// or `abandoned` whose resource has no destroy settled `succeeded`.
    StopNotConfirmed { operation: OperationId, status: ProviderStatus },
}

impl Journal {
    /// Every blocker of the job, oldest dispatch first.
    pub fn replacement_blockers(&self, job: JobId) -> Result<Vec<Blocker>, CommitError>;
}

// fabric-core, managed: DomainError gains, after its last variant,
ReplacementBlocked { operation: OperationId },
```

Rule: `Journal::submit` evaluates the barrier in the transaction of every `StartAttempt` the domain accepts. When the job has a blocker, the command is stored `rejected` with `ReplacementBlocked` naming the oldest blocker, like any domain refusal: a retry under the same key gets the same refusal, and a new key is needed once the blocker is gone. Matching "the same resource" is equality of `ResourceKey`: an exec's or a destroy's `target_key` against a destroy's `target_key`.

What does not block: a destroy settled `succeeded` or `rejected` (no effect); an exec settled; a create `attached`, `observed`, settled or `abandoned`. What unblocks a stop that failed: a new destroy of the same resource, with its own fresh approval, that settles `succeeded`. What unblocks a destroy the provider answered `not_found`, which is `unknown` with that reason on its row: an observe of its resource that answers `absent` with the scope it searched, which settles that destroy `succeeded` with the scope stored (2026-11-02); the `not_found` answer alone keeps the barrier. Abandoning a destroy or an exec never unblocks.

### Fresh cleanup approval flow

1. `Journal::residuals(job)` lists what the job's creates may have left: every create `succeeded`, `attached`, `observed`, `unknown` or `abandoned`, and every create `failed` with a locator, whose resource has no destroy settled `succeeded`. A create with no locator is listed without one, with `searched` when an attach looked.
2. The caller asks for a cleanup of one listed create with an approval reference supplied for this call: `Bridge::cleanup`. Fabric stores no approval reference, so it can never reuse one.
3. Fabric refuses before writing anything, with `CleanupRefusal`, when: the named operation is not a create of a provider operation the journal holds (`NotACreate`); it has no locator (`Unlocated`: attach it first, or it stays a residual); it is still `dispatched` (`StillDispatched`); its resource has a destroy settled `succeeded` (`AlreadyDestroyed`); or a destroy of that resource is `dispatched`, `unknown` or `observed` (`InFlight`: observe that one first). A cleanup with no approval reference does not type-check.
4. Fabric commits the cleanup `dispatched` with purpose `cleanup`, `cleans` the create, the create's own scope (job, task, attempt, generation), and the create's locator as its target. Unlike an effect it is accepted when the attempt has ended and when the job has finished or been cancelled. Then it writes an ordinary `request` for `sandbox.destroy` with that scope, target and approval reference. There is no cleanup message on the wire.
5. The worker resolves the reference to a grant scoped to `sandbox.destroy` on that resource and refuses before any effect when there is none (`approval_required`), when it names another action or resource (`approval_scope`, so a create grant never authorizes a destroy) or when it expired (`approval_expired`). Fabric records each refusal `rejected`; the residual stays listed. A destroy that settles `succeeded` removes it from `residuals` and satisfies the barrier for that resource.
6. Fabric never starts a cleanup itself, after a cancel or otherwise.

```rust
// fabric-journal
pub struct Residual {
    pub creating: OperationId,
    pub status: ProviderStatus,
    pub locator: Option<LocatorRecord>,
    pub searched: Option<String>,
}
pub enum CleanupRefusal {
    NotACreate,
    Unlocated,
    StillDispatched,
    AlreadyDestroyed,
    InFlight { operation: OperationId },
}
impl Journal {
    pub fn residuals(&self, job: JobId) -> Result<Vec<Residual>, CommitError>;
}

// fabric-bridge
pub struct Cleanup {
    /// The create whose resource is destroyed.
    pub creating: OperationId,
    /// A fresh operation id and key for the cleanup itself.
    pub id: OperationId,
    pub key: OperationKey,
    /// The approval reference for this call; never stored.
    pub approval: String,
    pub deadline_ms: u64,
}
impl Bridge {
    /// Builds the destroy from the journal (scope, target), computes its
    /// digest with `wire::request_digest`, records it and queues it.
    pub async fn cleanup<L: Recorder>(&self, log: &mut L, cleanup: Cleanup) -> Result<InFlight, BridgeError>;
}
```

### Abandonment

```rust
// fabric-journal

/// The owner's approval to stop reconciling one provider operation. It is
/// bound to the row as the owner saw it, so it cannot apply to a later state.
pub struct AbandonApproval {
    pub operation: OperationId,
    /// `Unknown`, `Attached` or `Observed`: the status the owner saw.
    pub expected: ProviderStatus,
    /// What the owner accepts may remain.
    pub acknowledged: Acknowledged,
    /// 1 to 512 characters, no control character.
    pub reason: String,
}
pub enum Acknowledged {
    /// The resource the row names (`locator_key`, else `target_key`).
    Resource(ResourceKey),
    /// The scope the row's last absent attach searched, as `searched` holds it.
    Searched(String),
    /// The row names no resource and no search.
    Nothing,
}
pub enum AbandonRefusal {
    /// The status is not `unknown`, `attached` or `observed`.
    NotAbandonable { status: ProviderStatus },
    /// The status is not the one the approval expects.
    Stale { status: ProviderStatus },
    /// The acknowledgement does not match the row.
    Acknowledgement,
    /// The reason is empty, too long or holds a control character.
    Reason,
}
impl Journal {
    pub fn provider_abandon(&mut self, approval: &AbandonApproval, at_ms: u64) -> Result<(), CommitError>;
}
```

M4 has one principal, the local owner, and no control surface drives the bridge yet, so the approval is the owner's call itself. A principal and an expiry come with remote principals (draft P07).

### Signatures M4 changes or adds

```rust
// fabric-journal
pub enum ProviderStatus { Dispatched, Unknown, Attached, Observed, Succeeded, Failed, Cancelled, Rejected, Abandoned }
pub struct ProviderOperation {
    pub dispatch: ProviderDispatch,
    pub status: ProviderStatus,
    pub answer: ProviderAnswer,
    pub locator: Option<LocatorRecord>,
    pub searched: Option<String>,
    pub abandon_reason: Option<String>,
}
pub enum ProviderError {
    KeyUsed { key: OperationKey },
    OperationUsed { operation: OperationId },
    UnknownOperation { operation: OperationId },
    Transition { operation: OperationId, from: &'static str, to: &'static str },
    CreateUnresolved { operation: OperationId },
    Cleanup { creating: OperationId, refusal: CleanupRefusal },
    Abandon { operation: OperationId, refusal: AbandonRefusal },
}
impl Journal {
    pub fn provider_begin(&mut self, dispatch: &ProviderDispatch, refusal: Option<&ProviderAnswer>, at_ms: u64) -> Result<(), CommitError>;
    pub fn provider_record(&mut self, operation: OperationId, status: ProviderStatus, answer: &ProviderAnswer, locator: Option<&LocatorRecord>, at_ms: u64) -> Result<(), CommitError>;
    pub fn provider_searched(&mut self, operation: OperationId, answer: &ProviderAnswer, searched: &str, at_ms: u64) -> Result<(), CommitError>;
    /// 2026-11-02: an absent observe of a destroy's resource settles the destroy succeeded with the scope stored.
    pub fn provider_absent(&mut self, operation: OperationId, answer: &ProviderAnswer, searched: &str, at_ms: u64) -> Result<(), CommitError>;
    pub fn provider_locate(&mut self, operation: OperationId, locator: &LocatorRecord, at_ms: u64) -> Result<(), CommitError>;
    pub fn provider_abandon(&mut self, approval: &AbandonApproval, at_ms: u64) -> Result<(), CommitError>;
    pub fn provider_operation(&self, operation: OperationId) -> Result<Option<ProviderOperation>, CommitError>;
    pub fn replacement_blockers(&self, job: JobId) -> Result<Vec<Blocker>, CommitError>;
    pub fn residuals(&self, job: JobId) -> Result<Vec<Residual>, CommitError>;
}

// fabric-bridge, log module: OperationLog gains, and Entry carries more of the row
pub trait OperationLog {
    fn begin(&mut self, dispatch: &ProviderDispatch, refusal: Option<&ProviderAnswer>) -> Result<(), LogError>;
    fn record(&mut self, operation: OperationId, status: ProviderStatus, answer: &ProviderAnswer, locator: Option<&LocatorRecord>) -> Result<(), LogError>;
    fn searched(&mut self, operation: OperationId, answer: &ProviderAnswer, searched: &str) -> Result<(), LogError>;
    fn absent(&mut self, operation: OperationId, answer: &ProviderAnswer, searched: &str) -> Result<(), LogError>;
    fn locate(&mut self, operation: OperationId, locator: &LocatorRecord) -> Result<(), LogError>;
    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError>;
    fn cleanup_target(&self, creating: OperationId) -> Result<Result<(Scope, LocatorRecord), CleanupRefusal>, LogError>;
}
pub struct Entry {
    pub scope: Scope,
    pub key: OperationKey,
    pub profile: String,
    pub action: String,
    pub effect: EffectClass,
    pub purpose: Purpose,
    pub target: Option<LocatorRecord>,
    pub status: ProviderStatus,
    pub locator: Option<LocatorRecord>,
}

// fabric-bridge, wire module
pub fn resource_key(locator: &Locator) -> ResourceKey;
pub fn attachable(action: &str, wait_ms: u64, profile: &Profile) -> Result<(), Refusal>;

// fabric-bridge, client
impl Bridge {
    /// Sends `attach` for an operation the log holds `unknown` with no
    /// target and no locator, under the profile it was dispatched with, by
    /// its original id, key, scope and action.
    pub async fn attach<L: Recorder>(&self, log: &mut L, operation: OperationId, wait_ms: u64) -> Result<InFlight, BridgeError>;
    /// M3, widened: also for an operation `attached` or `observed`.
    pub async fn observe<L: Recorder>(&self, log: &mut L, operation: OperationId, action: &str, resource: Locator, wait_ms: u64) -> Result<InFlight, BridgeError>;
}
pub enum BridgeError {
    // M3 variants, and:
    /// The log holds no operation `unknown` without a locator under this
    /// profile for this id. Nothing was sent.
    NotAttachable { operation: OperationId, why: String },
    /// Refused before anything was written.
    Cleanup { creating: OperationId, refusal: CleanupRefusal },
}
```

`LogError` carries the journal's `CreateUnresolved` and `Cleanup` refusals as `Fenced` with their reason, as it does for M3's fencing refusals.

### As built (M4)

- The barrier is evaluated by `provider::accept`, which `Journal::submit` calls in the command's transaction; a `StartAttempt` it refuses is stored `rejected` with `ReplacementBlocked`, and fabricd reports it through the client API as `managed_refused` with refusal `replacement_blocked`.
- `Journal::provider_destroys(resource)` lists the destroys of one resource with their status (the `by_target` index); the journal's `OperationLog::cleanup_target` reads it to refuse a cleanup before anything is written, and `provider_begin` checks the same rules again in its transaction.
- A cleanup whose target is not the create's locator, whose scope is not the create's, or that is not a destroy is refused `NotACreate`.
- An action outside the negotiated profile, which Fabric refuses before sending, is recorded with effect class `mutate`: it neither names a new resource nor removes one.
- An observe of an operation found after a loss (`unknown`, `attached`) that answers a `result` `accepted` or `running` records `observed`, as a `pending` does.
- `Journal::open` refuses a version 2 journal whose `provider_operations` table is the draft that unreleased M3 builds wrote (`OpenError::DraftSchema`) instead of failing later on a missing column.
- Locator text is written straight from the locator in key order (`wire::locator_record`), one buffer and one blake3 per locator, so an exec or a destroy costs no extra JSON tree on its hot path. The dispatch stays one committed write and the settle one read and one write (`tests/off_runtime.rs`, unchanged).
- The test double attaches `fx-<key>` by the operation that made it and keeps what it made in `FABRIC_BRIDGE_DOUBLE_PROVIDER`, so `tests/process.rs` runs a lost create through a worker and an authority restart without Capsule.
- 2026-11-02: a `sandbox.destroy` answered `provider_failed`/`not_found` is recorded `unknown` with the answer on its row, keeps the barrier and holds a cleanup back; an observe of its resource answered `unknown`/`absent` with `searched` settles it `succeeded` through `OperationLog::absent` and `Journal::provider_absent`, which store the scope searched. An absent observe of anything but a destroy settles nothing, and an absent observe whose `searched` is not the observed resource's provider, scope and kind settles and stores nothing (2026-11-03). The bridge refuses before sending an exec above the profile's `max_timeout_ms` or `max_output_bytes` (`invalid`/`limit`) and a destroy or exec whose target `native_id` is `null` or empty (`invalid`/`missing_field`).

### M4 acceptance mapped to tests

Each test is written first and fails on the M3 code (an M4 rule that already holds needs no test). Names are the ones the implementation uses; the shared fixtures are in `protocol/bridge/v1`.

| Acceptance | Tests |
|---|---|
| A lost response never triggers a blind duplicate creation | scenarios `attach_after_lost_create`, `attach_absent_stays_unknown`, `attach_then_observe_until_ready` (Fabric scripted run, Capsule fixture run, interop over the worker's stdio); `fabric-journal` `provider::tests::a_create_waits_while_a_create_of_the_job_is_unknown`; interop `a_create_lost_with_its_worker_is_attached_and_never_made_twice` (fixture profile, and the Docker profile on a local engine with an image it already holds); interop `a_create_whose_frame_never_left_stays_unknown_after_an_absent_attach_until_abandoned` (Fabric stopped between the intent and the frame: attached absent, the scope searched kept, no create of the job until the owner abandons it acknowledging that search) |
| A worker restart never triggers a blind duplicate creation | interop `a_create_lost_with_its_worker_is_attached_and_never_made_twice` kills the worker after the provider made the resource and before the answer, restarts it and attaches; `fabric-journal` `provider::tests::recovery_makes_a_dispatched_create_unknown_and_it_is_attached_not_resent` |
| Unknown remains explicit | `fabric-journal` `provider::tests::statuses_move_only_along_the_machine` (every pair of the table above), `provider::tests::recovery_leaves_attached_observed_and_abandoned_as_they_are`, `provider::tests::an_abandonment_needs_the_state_the_owner_saw`; scenario `attach_absent_stays_unknown` |
| A `not_found` destroy keeps the barrier until absence is observed | `fabric-journal` `provider::tests::a_not_found_destroy_stays_unknown_until_an_observe_finds_its_resource_absent`, `provider::tests::an_absent_observe_settles_nothing_but_a_destroy`; `fabric-bridge` `tests/stream.rs` `a_not_found_destroy_is_unknown_until_an_observe_finds_its_resource_absent` (the barrier exit) and `a_not_found_destroy_keeps_the_barrier_while_nothing_finds_its_resource_absent` (no evidence, no exit); `tests/stream.rs` `a_request_above_the_profile_bounds_or_at_no_resource_is_refused_before_sending` and `wire::tests::a_locator_with_a_null_or_empty_native_id_is_a_missing_field` for the two refusals |
| A stop failure blocks an unsafe replacement | `fabric-journal` `provider::tests::a_failed_or_unknown_destroy_blocks_a_replacement_until_a_destroy_succeeds`, `provider::tests::an_unsettled_exec_blocks_a_replacement_until_its_sandbox_is_destroyed`, `provider::tests::abandoning_a_destroy_or_an_exec_never_unblocks_a_replacement`, `provider::tests::an_attached_or_abandoned_create_does_not_block`; the refusal is stored under its key like any domain refusal; interop `a_destroy_lost_with_its_worker_blocks_a_replacement_until_a_stop_is_confirmed` (the worker killed mid-destroy, Fabric restarted: the replacement is refused while the destroy is unknown, observed and abandoned, and granted once a fresh destroy under its own grant is confirmed gone) and `a_docker_destroy_lost_with_its_worker_blocks_a_replacement_until_no_container_is_left` (the same on the local engine: one container, never two) |
| Fresh cleanup approval | scenario `cleanup_after_attach_needs_fresh_approval`; `fabric-journal` `provider::tests::a_cleanup_runs_after_the_job_is_cancelled`, `provider::tests::a_cleanup_needs_a_named_resource_of_this_job`, `provider::tests::residuals_list_what_no_cleanup_removed`; `fabric-bridge` `tests/process.rs` `a_cleanup_without_a_named_resource_is_refused_before_anything_is_written` |
| Attach and observe only where the profile supports them | scenario `attach_unsupported`; cases `attach_*` and `error_*absent*`; `fabric-bridge` `golden` |
| Owner labels decide | scenario `attach_checks_owner` |
| Version 2 fixture | `crates/fabric-journal/tests/fixtures/v2.sqlite` regenerated with the amended table; `tests/fixtures.rs` opens and verifies it |
| No hot-path regression | the perf phase alone: bridge round trip and spawn (`scripts/bench-bridge.sh`), `budget::hot_paths`, against the anchor; counters (thread hops and committed writes per effect in `tests/off_runtime.rs`) unchanged at one write to dispatch and one read and one write to settle |

### Carried to M5: the read surface for provider operations

M4 exposes provider operations as library calls only (ADR 0022, selection 6). Efe decided on 2026-10-29 that their read surface in the client API, the TypeScript SDK and the CLI belongs to M5, where fabricd starts driving the bridge, so it is M5 scope and not an open M4 item. It enters M5 as this acceptance row:

| Acceptance (M5) | Surfaces | Tests |
|---|---|---|
| A client reads, for one managed job, every provider operation as the journal holds it (its status, with `unknown`, `attached`, `observed` and `abandoned` shown as they are and never folded into another; its action, effect class, purpose and the create a cleanup cleans; its target and locator; the scope its last absent attach searched; its abandonment reason), the job's replacement blockers (`CreateUnresolved`, `ExecMayRun`, `StopNotConfirmed`, each with its operation and status, from `Journal::replacement_blockers`) and its residuals (each create's locator, or the scope an absent attach searched, from `Journal::residuals`). The read writes nothing and sends nothing to a worker. | one read op of the node's client API beside `managed_status`; the matching `fabric-client` method; the TypeScript SDK method beside `managedStatus`; a `fabric managed` subcommand beside `status` | written first and failing on the M4 code: a node test over a journal seeded with an operation in each of `unknown` (with and without a searched scope), `attached`, `observed` and `abandoned`, a blocker of each kind and a residual of each kind, answered through the client API with each value as the journal holds it and the journal unchanged after the read; the TypeScript SDK test of the same op against the node built in the same run; a CLI test of the subcommand's sentences for each status, blocker and residual |

## Placement and planned continuity (M5)

Status: proposed with ADRs 0023 to 0027, 2026-11-03; parts P03 to P05 built on 2026-11-03 (see "As built (M5): P03 to P05" below). This section is the M5 contract: the types and signatures the two implementation owners build against, written before any M5 code. Its selections were selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02 and are recorded in each ADR; none was approved by Efe. The code is the authority on signatures once it lands, and each part gains an "As built (M5)" note where the code differs. The facade's wire contract is [docs/facade-protocol.md](facade-protocol.md) with its fixtures in [protocol/facade/v1](../protocol/facade/v1).

Acceptance (roadmap M5): submit without choosing a machine; the same job observed after a reconnect; a planned handoff with child-task outcomes and the fidelity actually achieved.

### What M2 to M4 already provide

M5 adds only what is missing. These exist, are tested, and do not change:

| Need | Existing mechanism | Where |
| --- | --- | --- |
| Durable job, task, attempt and operation identity; dedupe by key; expected revision; replay | ADR 0019 domain, `accept_command`, `dedupe`, `apply_batch`, `replay` | `crates/fabric-core/src/managed/`, `managed_rules.rs` |
| `JobPolicy` (restart policy), `Intent`, `SourceBinding` (`Repository`, `Snapshot`), `TaskKind`, `RestartPolicy` | ADR 0019 model | `crates/fabric-core/src/managed/model.rs` |
| One authority, one writer; grants with a `Generation`; `StaleGeneration` refused; no grant while an attempt may run | ADR 0020 | `crates/fabricd/src/authority.rs`, `fabric-journal` |
| The journal: versioned SQLite, hash-chained events, projections, `verify`, `recover`, `events(job, after, limit)` with `EventSeq` | ADR 0020, schema versions 1 and 2 | `crates/fabric-journal/` |
| Provider effects fenced on the live grant, `unknown` on loss, attach, the replacement barrier, cleanup with fresh approval, residuals | ADR 0021, ADR 0022 | `crates/fabric-bridge`, `fabric-journal/src/provider.rs` |
| Capture of a workspace and a harness session into a content-addressed snapshot (bundle chain, private index, carry keys), restore in place or fresh, the `Clean`/`Dirty`/`Suspending`/`Restoring` marker | ADR 0004, ADR 0012, ADR 0014 | `crates/fabric-store/src/capture.rs`, `restore.rs`; `fabric-harness` adapters' `capture` and `restore` |
| The suspend path that stops a harness cleanly and publishes a snapshot; Claude Code and Codex tombstones; headless launch and hold | ADR 0006, ADR 0015, M2 group A | `crates/fabricd/src/job.rs`, `local.rs`, `fabric-harness` |
| A harness process group the node records and signals; what leaves the group is out of reach and reported | `local.rs`, the harness record | `crates/fabricd/src/local.rs` |
| Facts a node measures about itself per operation, with no cache: user, home, os, arch, tool versions, logins as yes or no; a peer asks with `FactsFor { harness, tools }` within a budget | ADR 0008, ADR 0013 amended | `crates/fabricd/src/facts.rs`, `fabric-net/src/wire.rs` |
| Known-writer rule: the newest harness version that wrote a session, compared with the target's | ADR 0006 | `fabric-core/src/version.rs`, `fabricd/src/doctor.rs` |
| Same root and home on every node | ADR 0011 | `fabric-harness` restore refusals, `doctor.rs` |
| Peers: the peer book of paired node ids and addresses, pinned identities on QUIC and TCP with TLS, one stream per request, budgets, per-peer caps, protocol 4 (unshipped) | ADR 0005, ADR 0013 | `crates/fabric-net/src/peers.rs`, `endpoint.rs`, `rpc.rs`, `wire.rs` |
| Snapshot transfer between nodes: `WantSnapshot`, `WantChunks`, chunks trusted after hashing | ADR 0013 | `fabric-net`, `fabricd/src/transfer.rs` |
| Secret paths that never leave a node, growing per job, with consent for a tracked secret (`TrackedSecrets`); credential-shaped text detection | ADR 0004, ADR 0007 | `fabric-core/src/secret.rs`, `credential.rs` |
| The client API: length-prefixed JSON, api 1 additive, fixtures under `protocol/fixtures`, `managed_create`, `managed_cancel`, `managed_status`, `managed_list` with operation keys; the Rust client and the TypeScript SDK | ADR 0018, M2 | `crates/fabric-client`, `sdk/typescript` |
| The bridge fixture discipline: a fixture set shared by two implementations, a sorted `manifest.sha256`, a pin test on each side | ADR 0021 | `protocol/bridge/v1`, `crates/fabric-client/tests/bridge_fixtures.rs` |

What is missing, and which part adds it:

| Gap after M4 | M5 addition | Part |
| --- | --- | --- |
| A task has no behavioral class; an interrupted test is indistinguishable from a finished one | `TaskClass`, `OutcomePolicy`, `TaskReceipt` with `interrupted` | P03 |
| Nothing bounds quiescence or reports an escaped child or a foreign writer | the executor, `DrainBudget`, `Quiescence`, `HandoffBlocker` | P03 |
| A checkpoint is a snapshot without receipts, facts or evidence | `CheckpointCut`, `CheckpointCutRecorded` | P03 |
| Nothing portable survives a harness change | `WorkPackage`, `Note`, `RecordNote` | P04 |
| Continuation has one fidelity and no record of it | `Fidelity`, `AllowedFidelity`, `ContinuationRecorded` | P04 |
| A source must be a path on the authority | `SourceBinding::Git`, `Empty`, import through capture | P05 |
| A task cannot say which secret it needs; nothing checks it before launch | `SecretRef`, the local resolver, the `Secret` exclusion | P05 |
| Secret paths leave or stay by pattern alone | `export_allowlist` as the capture's consent | P05 |
| The user names the machine | `NodeAdvert`, `place`, `PlacementDecision`, `PlacementDecided`, parking | P06 |
| The authority answers only its local socket | the facade over the node link, `FacadeTokenRecord`, `facade_call` | P07 |
| Provider operations have no read surface (ADR 0022, selection 6) | `managed_inspect` and the facade's `inspect` | P07 |

### Codec and schema version 3

Every shape below that is stored in the journal is carried by codec format byte `2`. The frame reader dispatches on the byte: `1` decodes with the M2 shapes, frozen in `fabric_core::managed::frame::v1`, and widens them with the M2 defaults (`JobPolicy::m2_defaults`, `TaskClass::Harness` for every task, `acceptance` empty); `2` decodes the shapes below. A format 1 frame keeps its bytes and its digest, so no chain changes, and `v1.sqlite` and `v2.sqlite` still verify. `request_digest` is computed over the format 2 bytes of a command; a key stored under a format 1 digest answers a repeat of that command as `OperationConflict`, which no shipped journal can hold. New command and event kinds are new `kind` strings.

Schema version 3 (migration `2 -> 3`, one transaction, tested from `tests/fixtures/v2.sqlite`; `tests/fixtures/v3.sqlite` is the new fixture):

```sql
PRAGMA user_version = 3;

ALTER TABLE tasks ADD COLUMN class TEXT NOT NULL DEFAULT 'harness'
  CHECK (class IN ('build','test','harness','opaque'));

-- operations.kind is checked against the command kinds; SQLite cannot widen a
-- CHECK in place, so the migration rebuilds the table with the kinds
-- 'record_note', 'checkpoint', 'continue' and 'cancel_task' added and copies
-- every row.

CREATE TABLE facade_tokens (
  token_id   BLOB PRIMARY KEY CHECK (length(token_id) = 16),
  digest     BLOB NOT NULL UNIQUE CHECK (length(digest) = 32),
  actor      TEXT NOT NULL,
  jobs       BLOB NOT NULL,        -- framed JobScope
  resources  BLOB NOT NULL,        -- framed ResourceScope
  minted_ms  INTEGER NOT NULL,
  expires_ms INTEGER NOT NULL,
  revoked_ms INTEGER
) STRICT;

CREATE TABLE placements (
  seq        INTEGER PRIMARY KEY REFERENCES events(seq),
  job_id     BLOB NOT NULL REFERENCES jobs(job_id),
  task_id    BLOB NOT NULL REFERENCES tasks(task_id),
  kind       TEXT NOT NULL CHECK (kind IN ('initial','planned_handoff','failover')),
  chosen     BLOB CHECK (chosen IS NULL OR length(chosen) = 32),
  decided_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX placements_by_job ON placements (job_id, seq);

CREATE TABLE continuations (
  attempt_id   BLOB PRIMARY KEY REFERENCES attempts(attempt_id),
  from_attempt BLOB NOT NULL REFERENCES attempts(attempt_id),
  achieved     TEXT NOT NULL CHECK (achieved IN ('native_session','new_native_session_with_package','workspace_only')),
  package      BLOB CHECK (package IS NULL OR length(package) = 32),
  seq          INTEGER NOT NULL REFERENCES events(seq)
) STRICT;
```

`placements` and `continuations` are projections of the events `PlacementDecided` and `ContinuationRecorded`, written by `write_transition` in the same transaction, and `verify` rebuilds them as it does the others. `facade_tokens` is authority state, not job history: it has no event, and a backup carries it as it is. Notes are read from the events (`NoteRecorded`), with no projection.

### Shared types in fabric-core (`managed` module)

Pure: no IO, no clock, no randomness. Everything below is additive to the M2 types of this page; a type named there and not here is unchanged.

```rust
// model.rs (owner A)

pub struct Intent {
    pub goal: String,                       // M2
    pub harness: HarnessKind,               // M2
    pub acceptance: Vec<String>,            // M5: each at most TEXT_MAX_BYTES
}

pub enum SourceBinding {
    Repository { path: AbsPath },           // M2; local socket only
    Snapshot { snapshot: SnapshotId },      // M2; also what an import becomes
    Git { uri: GitUri, commit: CommitId },   // M5
    Empty,                                  // M5
}

pub struct JobPolicy {
    pub restart: RestartPolicy,             // M2
    pub drain_budget_ms: u32,               // DrainBudget::MIN_MS ..= MAX_MS; default DEFAULT_MS
    pub allowed_fidelity: AllowedFidelity,  // default AllowedFidelity::DEFAULT
    pub secrets: Vec<SecretRef>,            // default empty; at most 32
    pub export_allowlist: Vec<RelPath>,     // default empty; at most 256
    pub placement: PlacementPolicy,         // default PlacementPolicy::DEFAULT
}
impl JobPolicy {
    pub fn m2_defaults(restart: RestartPolicy) -> JobPolicy;
    pub fn check(&self) -> Result<(), DomainError>;   // BadPolicy { field } on any bound broken
}

pub struct PlacementPolicy {
    pub platform: Option<Platform>,         // None: any; a snapshot source fills it from the manifest when the caller gave none
    pub disk_bytes: u64,                    // default DEFAULT_DISK_BYTES (2 GiB)
    pub pin: Option<NodeId>,
}

pub struct Task {
    // M2 fields, and:
    pub class: TaskClass,
}

pub enum Command {
    CreateJob { intent: Intent, source: SourceBinding, policy: JobPolicy },   // M2; in M5 its dispatch is the initial placement
    StartAttempt { task: TaskId, resource: ResourceRef, initial_checkpoint: Option<SnapshotId> }, // M2
    RegisterTask { parent: TaskId, kind: TaskKind, restart: RestartPolicy, class: TaskClass },   // class added
    FinishTask { task: TaskId, outcome: TaskOutcome },                        // M2
    CancelJob { reason: String },            // M2; in M5 accepted while an attempt is live and dispatched to stop it
    CancelTask { task: TaskId, reason: String },                              // M5
    RecordNote { note: Note },                                                // M5
    Checkpoint { attempt: AttemptId },                                        // M5: quiescence and a cut, the attempt stays live
    Continue { task: TaskId, fidelity: AllowedFidelity, pin: Option<NodeId> }, // M5: the planned handoff
}

pub enum Observation {
    // M2 kinds, with Checkpointed widened:
    Checkpointed { attempt: AttemptId, generation: Generation, snapshot: SnapshotId, cut: CheckpointCut },
    Receipts { attempt: AttemptId, generation: Generation, receipts: Vec<TaskReceipt> },
    HandoffBlocked { attempt: AttemptId, generation: Generation, blockers: Vec<HandoffBlocker> },
    Placed { operation: OperationId, task: TaskId, decision: PlacementDecision },
    Continued { attempt: AttemptId, generation: Generation, from: AttemptId, achieved: Fidelity, package: Option<PackageDigest> },
}

pub enum ManagedEvent {
    // M2 kinds, and:
    NoteRecorded { note: Note },
    ReceiptsRecorded { attempt: AttemptId, receipts: Vec<TaskReceipt> },
    CheckpointCutRecorded { attempt: AttemptId, snapshot: SnapshotId, cut: CutDigest, receipts: Vec<TaskReceipt> },
    HandoffBlocked { attempt: AttemptId, blockers: Vec<HandoffBlocker> },
    PlacementDecided { task: TaskId, decision: PlacementDecision },
    ContinuationRecorded { attempt: AttemptId, from: AttemptId, requested: AllowedFidelity, achieved: Fidelity, package: Option<PackageDigest> },
}

pub enum DomainError {
    // M2 and M4 kinds, and:
    BadPolicy { field: &'static str },
    UnsupportedSource { reason: &'static str },   // "adoption", "repository_over_facade"
    FidelityRefused { allowed: AllowedFidelity, offered: Vec<Fidelity> },
    NotLive { attempt: AttemptId },                // Checkpoint of an attempt that is not granted or running
    NoteTooLong { bytes: usize },
}

pub enum Note {
    Decision { what: String, reason: String },     // reason required, non-empty
    Question { text: String },
    Answer { question: WorkRevision, text: String }, // the revision the question was recorded at
}
```

Rules the transition functions keep:

- `CreateJob` is `accepted` at commit and `dispatched` when the authority starts the initial placement; `Placed` records the decision and, when a node was chosen, grants the primary task's first attempt there in the same batch (`AttemptGranted`, generation 1, `initial_checkpoint` from a snapshot source). With no chosen node the job is parked: active, primary open, no attempt, the decision recorded; the operation is `succeeded`.
- `Continue` is refused `UnknownTask` or `TaskClosed` as `StartAttempt` is, and `FidelityRefused` when `fidelity` allows nothing. Its dispatch: a quiescence and a cut of the live attempt (a `Checkpointed` observation, the attempt ends `checkpointed`), then `Placed` with kind `planned_handoff`; when a node is chosen, `AttemptGranted` at the next generation with the cut's snapshot as `initial_checkpoint`; `Continued` once the new attempt's executor reports what it achieved. `HandoffBlocked` ends the dispatch with the operation `failed` and the old attempt still live. The replacement barrier of ADR 0022 applies to the new grant as to any.
- `Checkpoint` needs a live attempt (`NotLive` otherwise); its dispatch is a quiescence and a cut after which the attempt continues; the operation is `succeeded` at `Checkpointed` and `failed` at `HandoffBlocked`.
- `CancelJob` and `CancelTask` with a live attempt are `accepted` and `dispatched`; the executor stops the attempt's groups (SIGTERM, the drain budget, SIGKILL) and reports `Exited` with `Cancelled` and the receipts `interrupted`; the job or task is then `cancelled` and the operation `succeeded`. With no live attempt they are `succeeded` at commit as in M2. A cancel never writes an outcome for a provider operation (ADR 0022).
- `RecordNote` is `succeeded` at commit; a text over `TEXT_MAX_BYTES` is `NoteTooLong`.
- A parent task cannot `FinishTask` while a registered child of it is `open` (`BadParent` is reused with the child named).

```rust
// lifecycle.rs (owner A), P03

pub enum TaskClass { Build, Test, Harness, Opaque }
pub enum OutcomePolicy { CancelAndRestart, Checkpoint, Block }
impl TaskClass { pub fn outcome_policy(self) -> OutcomePolicy; }

pub struct DrainBudget(u32);
impl DrainBudget {
    pub const MIN_MS: u32 = 1_000;
    pub const DEFAULT_MS: u32 = 30_000;
    pub const MAX_MS: u32 = 600_000;
    pub fn new(ms: u32) -> Result<Self, DomainError>;
}

pub enum ReceiptStatus {
    Running,
    Exited,
    Interrupted,                              // cut by a cancel or a quiescence; never passed, never failed
    Checkpointed { snapshot: SnapshotId },
    Blocked { reason: String },
}
pub struct TaskReceipt {
    pub task: TaskId,
    pub class: TaskClass,
    pub status: ReceiptStatus,
    pub exit: Option<i32>,                    // Some only for Exited
    pub interrupted: bool,                    // true exactly for Interrupted and for a harness Checkpointed mid-turn
    pub workspace: Option<WorkspaceRevision>,
}

pub struct WorkspaceRevision {
    pub bundle_head: SnapshotId,              // the snapshot whose manifest names the bundle chain head
    pub ledger_head: EventDigest,             // the digest of the job's latest journal event at the cut
}
pub struct EventDigest(pub [u8; 32]);

pub enum EvidenceKind { Checkpoint, Exit, Receipt }
pub struct EvidenceRef { pub digest: [u8; 32], pub attempt: AttemptId, pub kind: EvidenceKind }

pub struct CheckpointCut {
    pub snapshot: SnapshotId,
    pub receipts: Vec<TaskReceipt>,
    pub facts: Vec<(String, String)>,         // the node's facts for the job, as `NodeFacts::iter` gives them
    pub evidence: Vec<EvidenceRef>,
}
pub struct CutDigest(pub [u8; 32]);
impl CheckpointCut {
    pub fn canonical_json(&self) -> Vec<u8>;  // keys sorted, counters as decimal strings
    pub fn digest(&self) -> CutDigest;        // blake3 of "fabric.managed.cut.v1\0" and the canonical JSON
}

pub enum HandoffBlocker {
    EscapedChild { pid: u32, group: u32 },
    UncontrolledWriter { pid: u32, path: RelPath },
    CheckpointFailed { task: TaskId, reason: String },
    Opaque { task: TaskId },
    UnknownEffect { operation: OperationId },  // a provider operation dispatched or unknown (ADR 0022)
}
```

```rust
// package.rs (owner A), P04

pub const PACKAGE_VERSION: u32 = 1;
pub const PACKAGE_MAX_BYTES: usize = 256 * 1024;
pub const TEXT_MAX_BYTES: usize = 16 * 1024;

pub struct WorkPackage {
    pub version: u32,
    pub job: JobId,
    pub revision: WorkRevision,
    pub intent: Intent,
    pub decisions: Vec<Decision>,
    pub tasks: Vec<TaskSummary>,
    pub workspace: WorkspaceRevision,
    pub evidence: Vec<EvidenceRef>,          // this attempt's accepted evidence, then prior attempts' by digest
    pub questions: Vec<Question>,            // open questions: each Question with no Answer naming its revision
    pub artifacts: Vec<ArtifactRef>,
    pub next_actions: Vec<NextAction>,
}
pub struct Decision { pub what: String, pub reason: String, pub revision: WorkRevision }
pub struct Question { pub text: String, pub revision: WorkRevision }
pub struct TaskSummary { pub task: TaskId, pub class: TaskClass, pub state: TaskState, pub receipt: Option<TaskReceipt> }
pub struct ArtifactRef { pub name: String, pub digest: [u8; 32], pub bytes: u64 }
pub enum NextAction { Continue, Checkpoint, Cancel, FinishTask { task: TaskId }, RegisterTask { parent: TaskId } }
pub struct PackageDigest(pub [u8; 32]);

pub enum PackageError {
    TooLarge { bytes: usize },
    TextTooLong { field: &'static str, bytes: usize },
    CredentialLike { field: &'static str },  // fabric_core::credential::looks_like_credential on any text
}
pub struct PackageInput<'a> {
    pub state: &'a ManagedState,
    pub notes: &'a [(WorkRevision, Note)],
    pub receipts: &'a [TaskReceipt],
    pub workspace: WorkspaceRevision,
    pub evidence: &'a [EvidenceRef],
    pub artifacts: &'a [ArtifactRef],
}
pub fn build_package(input: &PackageInput<'_>) -> Result<WorkPackage, PackageError>;
impl WorkPackage {
    pub fn canonical_json(&self) -> Vec<u8>;
    pub fn digest(&self) -> PackageDigest;    // blake3 of "fabric.managed.package.v1\0" and the canonical JSON
}

pub enum Fidelity { NativeSession, NewNativeSessionWithPackage, WorkspaceOnly, Unsupported }
pub struct AllowedFidelity {
    pub native_session: bool,
    pub new_native_session_with_package: bool,
    pub workspace_only: bool,                // opt-in
}
impl AllowedFidelity {
    pub const DEFAULT: AllowedFidelity = AllowedFidelity { native_session: true, new_native_session_with_package: true, workspace_only: false };
    pub fn is_empty(&self) -> bool;
    /// The highest fidelity both allowed and offered; `Unsupported` is never allowed.
    pub fn highest(&self, offered: &[Fidelity]) -> Result<Fidelity, FidelityRefusal>;
}
pub struct FidelityRefusal { pub allowed: AllowedFidelity, pub offered: Vec<Fidelity> }

/// What a target can offer for a session captured under `source`:
/// `NativeSession` when the target has the same harness kind, version-compatible under the
/// known-writer rule, and the same layout; `NewNativeSessionWithPackage` when it has any
/// harness that launches with a prompt; `WorkspaceOnly` always. Pure; the adapter's own
/// restore refusals still apply at launch.
pub fn offered_fidelity(source: &HarnessKind, known_writer: Option<&HarnessVersion>, target: &NodeAdvert, same_layout: bool) -> Vec<Fidelity>;
```

```rust
// source.rs and secret_ref.rs (owner A), P05

pub struct GitUri(String);      // 1 to 2048 bytes; scheme https, ssh, git+ssh or file; no userinfo (CredentialInUri); no control character
pub struct CommitId(String);    // 40 or 64 lowercase hex characters; never a ref name
pub enum SourceError { BadUri { reason: &'static str }, CredentialInUri, NotACommitId, Adoption, RepositoryOverFacade }
impl GitUri { pub fn parse(text: &str) -> Result<Self, SourceError>; pub fn as_str(&self) -> &str; }
impl CommitId { pub fn parse(text: &str) -> Result<Self, SourceError>; pub fn as_str(&self) -> &str; }

pub struct SecretName(String);  // 1 to 64 characters of [A-Z0-9_]
pub enum AttemptScope { Any, Current }
pub enum ResourceKind { Node }  // grows with ResourceRef
pub struct SecretScope { pub attempt: AttemptScope, pub resource_kinds: Vec<ResourceKind> }
pub struct SecretRef { pub name: SecretName, pub scope: SecretScope }
pub const SECRET_ENV_PREFIX: &str = "FABRIC_SECRET_";

/// A resolved value. No Serialize, no Clone; Debug and Display print "<redacted>";
/// the bytes are overwritten with zero on drop (by hand, no new crate).
pub struct SecretValue(Vec<u8>);
impl SecretValue { pub fn expose(&self) -> &[u8]; }
```

```rust
// placement.rs (owner A: the types; owner B: place.rs with the function), P06

pub const PROBE_BOUND_MS: u32 = 5_000;
pub const MAX_CANDIDATES: usize = 64;
pub const DEFAULT_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub struct HarnessInventory { pub kind: HarnessKind, pub version: HarnessVersion }
pub struct PathLayout { pub home: AbsPath, pub root: AbsPath }
pub struct Capacity { pub load_1m_milli: u32, pub free_disk_bytes: u64 }
pub struct Warmth { pub bundle_chain: bool, pub workspace: bool }
pub struct NodeAdvert {
    pub node: NodeId,
    pub platform: Platform,
    pub harnesses: Vec<HarnessInventory>,
    pub layout: PathLayout,
    pub capacity: Capacity,
    pub warmth: Warmth,
    pub measured_at_ms: u64,
}

pub enum PlacementKind { Initial, PlannedHandoff, Failover }
pub struct CurrentPlacement { pub node: NodeId, pub healthy: bool }
pub struct Probe {
    pub node: NodeId,
    pub advert: Option<NodeAdvert>,          // None: no answer within PROBE_BOUND_MS
    pub secrets_unresolved: Vec<SecretName>, // from the node's own resolver, asked in the same probe
    pub probe_ms: u32,
}
pub struct PlacementInput<'a> {
    pub job: &'a ManagedJob,
    pub task: TaskId,
    pub kind: PlacementKind,
    pub required_layout: Option<PathLayout>, // the captured root and home when native_session is allowed
    pub current: Option<CurrentPlacement>,
    pub candidates: Vec<Probe>,              // at most MAX_CANDIDATES
    pub now_ms: u64,
}
pub enum Exclusion {
    NoAnswer { within_ms: u32 },
    Stale { age_ms: u64 },                   // measured_at_ms older than now_ms minus PROBE_BOUND_MS; fabricd stamps measured_at_ms with its own clock when the answer arrives and decides no later than the round's bound, so neither a silent peer nor a peer's clock makes an answer stale
    Platform { need: Platform, have: Platform },
    HarnessMissing { kind: HarnessKind },
    HarnessVersion { kind: HarnessKind, need: HarnessVersion, have: HarnessVersion },
    Layout { need: PathLayout, have: PathLayout },
    Secret { name: SecretName },
    Disk { need: u64, free: u64 },
    Pin { pinned: NodeId },
}
pub enum RankReason { CurrentHealthy, BundleChain, Workspace, Load { milli: u32 }, Identity }
pub enum Verdict { Eligible { rank: Vec<RankReason> }, Excluded { reasons: Vec<Exclusion> } }
pub struct Candidate { pub node: NodeId, pub verdict: Verdict }
pub struct PlacementDecision {
    pub kind: PlacementKind,
    pub decided_at_ms: u64,
    pub candidates: Vec<Candidate>,          // every probe, in rank order then exclusions, each with every reason
    pub chosen: Option<NodeId>,
}

// place.rs (owner B)
/// Deterministic: the same input gives the same decision. Filters first, every
/// exclusion of a node listed; then ranks the eligible set by CurrentHealthy,
/// BundleChain, Workspace, lowest Load, then Identity.
pub fn place(input: &PlacementInput<'_>) -> PlacementDecision;
```

```rust
// facade.rs (owner A: token types and the check; owner B: everything on the wire), P07

pub struct TokenId([u8; 16]);               // a ULID; the revocation id
pub struct TokenDigest([u8; 32]);
pub struct Actor(String);                   // 1 to 64 characters of [A-Za-z0-9._-]
pub enum JobScope { AnyOwned, Jobs(Vec<JobId>) }
pub enum ResourceScope { Any, Nodes(Vec<NodeId>) }
pub struct FacadeTokenRecord {
    pub id: TokenId,
    pub digest: TokenDigest,
    pub actor: Actor,
    pub jobs: JobScope,
    pub resources: ResourceScope,
    pub minted_ms: u64,
    pub expires_ms: u64,
    pub revoked_ms: Option<u64>,
}
pub const BEARER_BYTES: usize = 32;
pub fn token_digest(bearer: &[u8; BEARER_BYTES]) -> TokenDigest;  // blake3 of "fabric.facade.token.v1\0" and the bearer
pub enum TokenCheck { Ok, Unknown, Revoked, Expired, OutOfScope { what: ScopeKind } }
pub enum ScopeKind { Job, Resource }
/// In this order: unknown, revoked, expired, job scope, resource scope.
pub fn check_token(record: Option<&FacadeTokenRecord>, now_ms: u64, job: Option<JobId>, resource: Option<NodeId>) -> TokenCheck;

pub const WATCH_WINDOW: usize = 256;
pub const WATCH_MAX_WAIT_MS: u32 = 25_000;
pub const WATCH_DEFAULT_WAIT_MS: u32 = 10_000;
```

### Journal (owner A, `fabric-journal`)

```rust
impl Journal {
    // M2 to M4, unchanged, and:
    pub fn notes(&self, job: JobId) -> Result<Vec<(WorkRevision, Note)>, CommitError>;
    pub fn placements(&self, job: JobId) -> Result<Vec<(EventSeq, PlacementDecision)>, CommitError>;
    pub fn continuations(&self, job: JobId) -> Result<Vec<(AttemptId, AttemptId, Fidelity, Option<PackageDigest>)>, CommitError>;
    pub fn receipts(&self, attempt: AttemptId) -> Result<Vec<TaskReceipt>, CommitError>;
}
// src/facade.rs (owner B), rows of facade_tokens
impl Journal {
    pub fn token_mint(&mut self, record: &FacadeTokenRecord) -> Result<(), CommitError>;  // refuses a known id or digest
    pub fn token_by_digest(&self, digest: &TokenDigest) -> Result<Option<FacadeTokenRecord>, CommitError>;
    pub fn token_revoke(&mut self, id: TokenId, at_ms: u64) -> Result<bool, CommitError>;  // false when unknown or already revoked
}
```

`Journal::events(job, after, limit)` is the watch's cursor read; `limit` is at most `WATCH_WINDOW`.

### fabricd (owner A: executor, secrets, sources; owner B: placement, facade)

```rust
// executor.rs (owner A)
pub struct Executor { /* the attempts this node runs, their groups and registered children */ }
pub struct Spawned { pub pid: u32, pub group: u32, pub task: TaskId }
pub enum ExecutorError { Spawn(std::io::Error), NotRunning { attempt: AttemptId }, Frozen { attempt: AttemptId }, Secret(SecretError), Source(SourceError) }
pub struct Quiescence { pub receipts: Vec<TaskReceipt>, pub blockers: Vec<HandoffBlocker> }
impl Executor {
    /// Spawns `command` for `task` in a group of its own, with the job's secrets in its
    /// environment under their names, and registers it. Refused while the attempt is frozen.
    pub fn spawn(&self, attempt: AttemptId, task: TaskId, class: TaskClass, command: std::process::Command) -> Result<Spawned, ExecutorError>;
    /// Freezes spawning for the attempt, settles leaves before parents under each task's
    /// policy within `budget`, and reports receipts and blockers. Never kills a harness task:
    /// it goes through the suspend path and comes back Checkpointed or CheckpointFailed.
    /// (As built: `quiesce(attempt, budget, then: AfterCut)`; under `End` the harness is
    /// stopped before its capture, see "As built (owner B)".)
    pub async fn quiesce(&self, attempt: AttemptId, budget: DrainBudget) -> Result<Quiescence, ExecutorError>;
    /// Stops every group of the attempt: SIGTERM, `budget`, SIGKILL; every receipt Interrupted.
    pub async fn stop(&self, attempt: AttemptId, budget: DrainBudget) -> Result<Vec<TaskReceipt>, ExecutorError>;
    /// Processes in the attempt's groups that no registered task owns, and writers under the
    /// root outside every group (lsof, where the facts found it).
    pub fn blockers(&self, attempt: AttemptId, root: &AbsPath) -> Vec<HandoffBlocker>;
    /// The cut: the capture through the existing path, then the document and its digest.
    pub async fn cut(&self, attempt: AttemptId, quiescence: &Quiescence) -> Result<CheckpointCut, ExecutorError>;
}

// secrets.rs (owner A)
pub struct LocalResolver { /* the daemon's environment and <config>/secrets */ }
pub enum SecretError { Missing { name: SecretName }, FileMode { name: SecretName, mode: u32 }, FileOwner { name: SecretName }, NotUtf8 { name: SecretName }, OutOfScope { name: SecretName } }
impl LocalResolver {
    pub fn open(config_dir: &Path) -> LocalResolver;
    /// Which refs this node cannot resolve right now; read anew every call, cached never.
    pub fn unresolved(&self, refs: &[SecretRef]) -> Vec<SecretName>;
    pub fn resolve(&self, job: JobId, attempt: AttemptId, secret: &SecretRef) -> Result<SecretValue, SecretError>;
}

// sources.rs (owner A)
pub enum Prepared { Cloned { root: AbsPath, commit: CommitId }, Restored { root: AbsPath, snapshot: SnapshotId }, Empty { root: AbsPath } }
pub async fn prepare(node: &Node, job: JobId, source: &SourceBinding) -> Result<Prepared, SourceError>;
/// Captures `directory` into a snapshot through the existing capture path without touching it.
pub async fn import(node: &Node, directory: &AbsPath, allowlist: &[RelPath]) -> Result<SnapshotId, SourceError>;

// placement.rs (owner B)
pub struct Placer { /* the peer book, the endpoint, this node's own advert */ }
impl Placer {
    /// This node's advert for `job`, measured now.
    pub fn own_advert(&self, job: &ManagedJob, root: &AbsPath) -> NodeAdvert;
    /// Asks every peer with `Advertise` within PROBE_BOUND_MS, in parallel, and runs `place`.
    pub async fn place(&self, input_without_candidates: PlacementInput<'_>, secrets: &[SecretRef]) -> PlacementDecision;
}

// facade.rs (owner B)
pub struct Facade { /* the authority handle, the journal's token rows, one bounded broadcast of WATCH_WINDOW events per watched job */ }
impl Facade {
    /// Checks version, token and scope in that order, then serves one message; never blocks the authority's writer.
    pub async fn serve(&self, message: FacadeRequest, now_ms: u64) -> FacadeResponse;
    pub fn mint(&self, actor: Actor, jobs: JobScope, resources: ResourceScope, expires_ms: u64, now_ms: u64) -> Result<(TokenId, [u8; BEARER_BYTES]), NodeError>;
    pub fn revoke(&self, id: TokenId, now_ms: u64) -> Result<bool, NodeError>;  // ends open watches of the token
}
```

### fabric-net (owner B), protocol 4, additive

```rust
pub enum RequestBody {
    // existing, and:
    /// Asks the peer for its advert for a job; answered with `Advert`.
    Advertise { job: JobId, harness: HarnessKind, root: AbsPath, snapshot: Option<SnapshotId>, secrets: Vec<SecretName> },
    /// One facade message, JSON, at most MAX_REQUEST bytes; answered with `Facade`.
    Facade(FacadeBytes),
}
pub enum ResponseBody {
    // existing, and:
    Advert(AdvertWire),   // NodeAdvert plus `secrets_unresolved: Vec<SecretName>`
    Facade(FacadeBytes),  // at most MAX_FRAME bytes
}
pub struct FacadeBytes(Vec<u8>);
```

A facade request runs under the per-peer caps of ADR 0013 like a per-job request; a watch holds one of them for at most `WATCH_MAX_WAIT_MS`.

### Client API (owner B), api 1, additive

| Op | Request fields | Reply |
| --- | --- | --- |
| `managed_create` | M2 fields, and optional `source` (`repository`, `snapshot`, `git`, `empty`, or `directory { path }`, which the node imports first), `acceptance`, `policy` (the `JobPolicy` fields, each optional) | `ManagedOperation`, whose job carries the placement decision |
| `managed_cancel` | M2 fields, and optional `task` | `ManagedOperation` |
| `managed_note` | `operation_key`, `job`, optional `expected_revision`, `note` | `ManagedOperation` |
| `managed_checkpoint` | `operation_key`, `job`, optional `expected_revision` | `ManagedOperation` |
| `managed_continue` | `operation_key`, `job`, optional `expected_revision`, optional `fidelity`, optional `pin` | `ManagedOperation` |
| `managed_inspect` | `job` | `Inspection`: the `ManagedJob` of M2 with each task's `class` and last receipt, `placements`, `continuations`, `provider_operations`, `blockers`, `residuals`, `notes`, `parked: bool`; the read writes nothing and sends nothing to a worker (ADR 0022, selection 6) |
| `managed_import` | `path`, optional `export_allowlist` | `{ snapshot }` |
| `facade_mint` | `actor`, `jobs`, `resources`, `expires_ms` | `{ token_id, bearer }`; the bearer is shown once and never stored |
| `facade_revoke` | `token_id` | `{ revoked }` |
| `facade_call` | `authority` (a node id; this node's own is allowed), `message` (one facade request as docs/facade-protocol.md defines it) | `{ message }`: the authority's answer, unchanged |

The same `Inspection` object is the body of the facade's `inspection`. Every counter travels as a canonical decimal string. CLI: `fabric managed create` gains `--git <uri> --commit <id>`, `--snapshot <id>`, `--empty`, `--import <dir>`, `--acceptance <text>` (repeatable), `--drain-budget <ms>`, `--allow-fidelity <name>` (repeatable), `--secret <NAME>` (repeatable), `--export <path>` (repeatable), `--pin <node>`; `fabric managed cancel [--task <id>]`, `note`, `checkpoint`, `continue`, `inspect`; `fabric facade mint|revoke|submit|inspect|watch|cancel|checkpoint|continue --authority <node>`.

### M5 acceptance mapped to tests

Each test is written first and fails on the M4 code. Names are what the implementation uses; a name below that the code changes is recorded under "As built (M5)".

| Acceptance | Tests |
| --- | --- |
| Submit without choosing a machine | `fabricd` `placement::tests::{two_eligible_nodes_rank_the_same_every_time, no_eligible_node_parks_the_submit_with_every_exclusion, a_pin_to_an_ineligible_node_parks_naming_the_pin, a_silent_or_stale_node_is_excluded_for_this_placement_only, a_healthy_current_placement_is_never_moved_for_a_warmer_node, an_unresolvable_secret_excludes_before_launch, warmth_never_admits_an_incompatible_node}`; `fabric-core` `managed_place.rs` property test (`place` is a pure function of its input); e2e `managed::e2e_a_submit_runs_on_the_eligible_node_without_naming_it` (two daemons on this host, separate state roots, the submit on one, the attempt on the other, the decision in `inspect`) |
| The same job observed after a reconnect | `fabricd` `facade::tests::{a_token_out_of_scope_is_refused_before_the_authority_is_asked, an_unknown_expired_or_revoked_token_is_refused_in_that_order, a_revoked_token_ends_an_open_watch_within_one_delivery, a_duplicate_key_reconciles_one_operation, the_same_key_with_another_request_is_a_conflict, a_lagged_reader_gets_a_resume_cursor_and_the_authority_holds_no_more_than_the_window, a_disconnect_leaves_the_operation_running, a_cancel_is_a_command_with_its_own_outcome, an_authority_restart_keeps_the_cursor}`; `fabric-client` `facade_fixtures.rs` and the TypeScript `facade_fixtures.test.ts` (the pins, this contract step), later each side's parser against every case; e2e `facade::e2e_the_same_job_is_observed_after_a_reconnect_from_another_node` (watch, kill the client, resume from the cursor, every event once); the TypeScript `facade.test.ts` against the node built in the same run; `e2e_a_socket_only_op_on_a_managed_job_is_refused_as_managed_job` |
| A planned handoff with child-task outcomes and the fidelity achieved | `fabricd` `executor::tests::{an_interrupted_test_is_recorded_interrupted_never_passed, an_interrupted_build_is_restarted_not_resumed, a_harness_task_is_checkpointed_by_the_suspend_path, a_failed_harness_checkpoint_blocks_the_handoff, an_opaque_task_blocks, an_escaped_child_blocks_a_strong_handoff_and_fails_nothing, an_uncontrolled_writer_blocks, a_parent_waits_for_its_leaves, the_drain_budget_bounds_quiescence_not_the_task, a_client_killed_at_each_launch_boundary_leaves_no_task_running}` (crash points `managed_launch.before_record`, `managed_launch.after_record`); `fabric-core` `managed_package.rs::{a_package_over_a_limit_is_refused_naming_the_field, planted_credentials_and_approvals_never_enter_a_package, evidence_is_by_digest_only, a_refusal_is_never_lowered, the_highest_allowed_and_offered_fidelity_wins}`; e2e `continuity::e2e_a_planned_handoff_records_child_outcomes_and_the_fidelity_achieved` (a Claude Code job with a registered test task, continued under the fake ACP agent with the package: different native session ids, the same intent, decisions, evidence refs and open tasks, the test recorded `interrupted`, `achieved: new_native_session_with_package`), `e2e_a_native_session_continues_on_a_node_with_the_same_layout` (`achieved: native_session`) |
| Source bindings and secrets | `fabricd` `sources::tests::{a_git_source_is_cloned_at_its_exact_revision, an_import_leaves_the_directory_untouched, an_empty_source_is_empty, adoption_is_refused_with_its_reason, a_credential_in_a_uri_is_refused, a_secret_path_leaves_only_when_the_allowlist_names_it}`; `secrets::tests::{env_then_file_then_missing, a_file_not_0600_or_not_owned_is_refused, a_value_reaches_the_task_environment_and_nothing_else, eligibility_is_read_anew_every_call}`; e2e `secrets::e2e_a_secret_value_appears_in_no_journal_snapshot_package_receipt_or_log` |
| Codec and schema | `fabric-core` `managed_frame.rs::a_format_1_frame_decodes_and_widens_to_the_m2_defaults`; `fabric-journal` `tests/fixtures.rs` opens `v1`, `v2` and the new `v3.sqlite`, migrates `v2 -> 3` and verifies; `crash_tests` at the new fault points (`placement.after_commit`, `continue.after_cut`) |
| The read surface for provider operations (carried from M4) | as the row under "Carried to M5": the node test over a seeded journal, the TypeScript test of `managedInspect`, the CLI test of `fabric managed inspect` |
| Counters, not time | the facade: one journal read per `inspect` and `watch`, one commit per command; a placement: one probe round per decision; the executor: no fsync beyond the capture's own; `tests/off_runtime.rs` unchanged. Time is the Perf step's alone |

### As built (M5): the review fixes

Made after the M5 review of 2026-11-08; each fix has a regression test that failed on the code before it. The selections below stand as selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe.

- Placement: every advert is stamped with the authority's own clock when its answer arrives, all asks of a round start together, and the decision's time is never later than the round's bound, so a silent peer makes no answer stale and a peer's clock does not matter (`placement::tests::{a_silent_peer_does_not_make_the_answering_nodes_stale, a_peer_with_a_skewed_clock_is_not_stale}`). The rank keys on the node the probe reached, never on the id the advert claims.
- Facade scope, deny by default (selected): a token limited to listed jobs may not `submit`, and a token limited to listed nodes may not `submit` or `continue` without a pin to one of them (`check_token` takes `JobAsk` and `ResourceAsk`; fixtures `submit_job_list_token`, `submit_unpinned_with_node_list`, `continue_unpinned_with_node_list`).
- Facade `continue`: a named fidelity list is cut to the job's `allowed_fidelity` (selected: only the owner widens it, on the socket); nothing left is `refused`/`fidelity`. A facade `submit` whose ACP arguments hold a secret is `refused`/`credential_like`, and the domain's credential guard covers the ACP arguments on every path.
- Keys: facade keys are `facade:<actor>:<key>` and the owner's socket refuses that prefix. `checkpoint` and `continue` are deduplicated by the caller's request through `Authority::submit_derived` and the `request_keys` table, so a retry after the job moved replays.
- The authority's commit counter moves only for calls that write (`with_journal` reads, `write_journal` writes), so an idle watch costs no authority work; a watch also wakes at its token's expiry. `Journal::receipts(job, attempt)` searches `events_by_job` for the one job.
- Domain: the state keeps every cut (`ManagedState.cuts`) and a continue starts from the newest cut of its task; a `Checkpointed` observation settles only the checkpoint command that names its attempt, and an attempt that ends first fails that command; a cut must hold a receipt for every open task of its attempt (`MissingReceipt`); `new_native_session_with_package` needs a package (`MissingPackage`); an accepted, not yet dispatched checkpoint or continue counts as in flight.
- Executor: no `FABRIC_SECRET_*` variable reaches a task but the declared ones under their bare names; each attempt has its own lock, so one attempt's drain does not stall another; `resume` lifts a quiescence's freeze after a checkpoint command's cut; a spawn that fails after its process exists kills the group before it returns.
- The TypeScript free-text rule refuses a carriage return, as the Rust parser does (fixture `submit_text_carriage_return`).

### As built (M5): the initial placement's dispatch and what the acceptance runs prove

Added in the acceptance step, after both owners merged. The selections stand as selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe.

- `fabricd::dispatch` places every accepted `CreateJob` whose source is `snapshot`, `git` or `empty` (an imported directory is a snapshot by then): `Node::place_job` with `PlacementKind::Initial`, then the `Placed` observation, which records the decision and grants the primary task's first attempt on the chosen node in one batch, or parks the job. The list is `Authority::pending()`, read anew after every commit the authority counts, so a create accepted before a crash is placed after the restart, once. A create with a `repository` source is the M2 form and is still settled by its explicit `StartAttempt`; the loop leaves it alone, so the M2 tests and journals see no new event. The loop waits on `Authority::started()` and never starts the authority's thread itself, so a node with no managed job still starts none. A placement whose observation is refused is logged and not tried again in that process. Fault point `before_placement` (after the create's commit, before any decision) is the crash test's.
- A harness version is advertised as the version in the harness's `--version` output (`codex-cli 0.155.1` is `0.155.1`); before this, every real Codex and Claude Code was advertised in a form the asking node could not parse and every node was excluded `harness_missing`.
- Not built: the launch of a granted attempt, on this node or another. fabric-net protocol 4 carries `Advertise` and the facade, and no message that asks the chosen node to run a grant or carries its observations (`Started`, `Exited`, `Receipts`, `Checkpointed`, `HandoffBlocked`, `Continued`) back to the authority; the node has no `Capturer` but the tests' fake; `RegisterTask` names no program for a build or test task, so the executor has nothing to spawn for one; and no work package is built or carried. So a `Continue` stays `accepted` and the M5 rows "A planned handoff with child-task outcomes and the fidelity achieved" and the e2e tests the table names for it (`continuity::*`, `secrets::e2e_a_secret_value_appears_in_no_journal_snapshot_package_receipt_or_log`, and the attempt half of `e2e_a_submit_runs_on_the_eligible_node_without_naming_it`) are not run end to end. The question this needs is in docs/evidence/m5/README.md, "Blocked". Answered 2026-11-08 by the selection "M5 launch path" (option (a), under the delegation): the contract is "Launch path (M5, as built)" below.

The acceptance tests that ran through real daemons (two `fabricd` on one host, separate state roots, a TCP and TLS node link):

| Acceptance | Test |
| --- | --- |
| Submit without choosing a machine: two eligible nodes, the decision recorded, the grant on the first ranked | `placement::e2e_a_submit_names_no_machine_and_the_decision_is_recorded` |
| A secret only one node resolves; no eligible node parks the submit; a pin | the same test |
| A create accepted before a crash is placed once after the restart | `placement::e2e_a_create_accepted_before_a_crash_is_placed_once_after_the_restart` |
| The same job observed after a client reconnect, from another node | `facade::e2e_the_same_job_is_observed_after_a_reconnect_from_another_node` |
| The watch cursor across an authority restart | `facade::e2e_a_watch_cursor_holds_across_an_authority_restart` |

### As built (M5): P03 to P05

Owner A's parts landed in two commits, the model (`fabric-core`, format byte 2, schema version 3) and the node modules (`fabricd` executor, secrets, sources). The code is the authority on signatures; this note lists where it differs from the contract above and why. The selections stand as selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe.

Model and transition rules (owner A, shared):

- `ManagedEvent::CommandAccepted` carries `effect: bool`: whether an executor is asked for an effect after the commit. `apply_batch` reads the operation's first status from it, not from the kind, so a format 1 history (where only a grant had an effect) and an M5 history (where a cancel has one only while an attempt is live) replay to the status the journal stored. `CommandKind::has_effect` says whether a kind may have one.
- An accepted M5 effect is kept in the history: `ManagedEvent::EffectRequested { operation, request: EffectRequest }` follows `CommandAccepted` for a `CancelJob` or `CancelTask` with a live attempt, a `Checkpoint` and a `Continue`. `ManagedState.requests` holds them (the journal reads them from the events, no projection), so a restart dispatches an accepted command with what it was asked with, and the observations that settle a cancel or a continue read the reason and the requested fidelity from state. `ManagedState::request`, `in_flight(kind)` and `parked()` are the reads.
- `DomainError::BadPolicy` and `UnsupportedSource` carry a `String`, not a `&'static str`, so a stored refusal decodes. `CredentialLike { field }` and `EffectInFlight { operation }` are added: a goal, an acceptance text or a note that looks like a credential (`fabric_core::credential::looks_like_credential`) is refused before the journal records it, and a second `Checkpoint` or `Continue` while one is dispatched waits. An acceptance text over `TEXT_MAX_BYTES` is `BadPolicy { field: "acceptance" }`; a decision with an empty reason is `BadPolicy { field: "note.reason" }`. An answer's `question` revision is recorded, not checked against the notes.
- The initial placement: `Placed` on an `accepted` create marks it `dispatched` in the same batch as the decision and the grant, so a crash after that commit recovers it as `unknown` with the attempt `granted`, never placed or launched again (`crash_tests::a_placement_killed_after_its_commit_never_places_or_launches_again`). With no chosen node the operation is `succeeded` and the job parked, for a create and for a continue alike. The attempt a placement grants takes an id derived from the operation and the generation (`blake3` under `fabric.managed.attempt.v1`), because an observation carries no fresh id. An explicit `StartAttempt` on a job whose create still awaits its placement settles the create (`dispatched`, then `succeeded`): the grant stood in for the placement, and a v1 history, where every create was complete at its commit, replays unchanged.
- The planned handoff: the authority marks the `Continue` `dispatched` before the quiescence, so the cut is an effect under a recorded dispatch; the `Checkpointed` observation ends the attempt (`crash_tests::a_continue_killed_after_its_cut_is_retried_from_the_cut` shows the owner's retry placing from the cut); `Placed` with kind `planned_handoff` grants at the next generation with the task's last checkpoint as `initial_checkpoint`; `Started` never settles a continue's grant, `Continued` does, and a `Continued` whose `achieved` the continue did not allow is `FidelityRefused`, never recorded lower. A `Continue` of a task with no live attempt skips the cut and places from the last checkpoint.
- `Checkpointed` while a `Checkpoint` command is dispatched records `CheckpointCutRecorded` and the attempt goes on; otherwise (a continue in flight, or an M2-style suspend) the attempt ends `checkpointed` and the cut is recorded beside it. `HandoffBlocked` records the blockers and fails the dispatched checkpoint or continue; the attempt stays live.
- `Exited` with `Cancelled` while a `CancelJob` is dispatched appends `JobCancelled` with the request's reason and marks the cancel `succeeded`; while a `CancelTask` of that task is dispatched it appends `TaskFinished { Cancelled }`. `CancelTask` and `FinishTask` of a task with an open registered child are `BadParent` naming the child.
- `lifecycle.rs` reuses `ids::EventDigest` for `WorkspaceRevision.ledger_head` instead of a second type. `SecretScope::node_any()` is a function (a `Vec` cannot sit in a `const`). `SourceError` gained `BadSecretName` and `Unavailable { reason }`. `canon.rs` is the private canonical JSON writer the cut and the package share.

Journal (owner A):

- Version 3 is applied with foreign keys off, as `SQLite`'s table rebuild needs, and `open` runs `foreign_key_check` after the commit and refuses the file on any dangling reference. `facade_tokens` is in the version 3 DDL for owner B's `src/facade.rs`.
- `Recovery.to_effect` lists every M5 command still `accepted` (a create awaiting placement among them); `Authority::pending()` answers the same list from the journal at any time.
- `receipts(attempt)` is the last receipts a cut or a report recorded for the attempt. `verify` checks that `placements` and `continuations` hold one row per event of their kind.
- The v1 fixture's old keys answer a repeat with `OperationConflict`, as the codec section says; `the_v1_fixture_holds_what_it_was_made_with` pins it.

fabricd (owner A):

- `Executor::new(records, resolver, capturer)` with `begin(attempt, job, root, secrets)` and `end(attempt)`; `spawn` is `async` and takes a `std::process::Command`; `blockers` is `async`. The capture side is the `Capturer` trait (the harness checkpoint through the suspend path, the workspace capture, the node's facts), which the node implements in its own files and the tests fake. Leaves before parents is the newest spawned task first: a child is spawned after its parent, and the executor keeps no parent map. The launch boundaries `managed_launch.before_record` and `after_record` are the two states of the durable launch record (an intent with no pid; a pid and start time), both cleared by `Executor::recover`, which stops a recorded group only when its pid still starts when the record says; the test exercises both by record state rather than by stopping a process. `Counters` are spawns, signals and captures.
- `LocalResolver::resolve(job, attempt, ref)` checks the ref's resource scope; the attempt scope (`Current`) is the executor's check: `Executor::begin` takes the attempt's generation, the executor keeps each job's current generation (the newest attempt begun on the node, or what `Executor::supersede` reports), and a superseded attempt's tasks get no `Current` secret. A file value's trailing newline is dropped.
- `sources::prepare_in(root, source, store)` and `import_from(store, node, dir, allowlist)` are the blocking cores; `prepare(node, job, source)` and `import(node, dir, allowlist)` wrap them with the node's home and store. The root of a managed attempt on a node is `paths::job_root(home, job)`. `Repository { path }` prepares as `SourceError::Adoption`: the M2 form is accepted by the authority on the local socket and never adopted by an executor. The clone runs `git` with no credential helper, no askpass and no prompt. `fabric-store`'s `IgnorePolicy::allow_export(path)` is the export allowlist: a named path, or one under it, is not denied by a secret pattern and gets a negation line in the excludes file.

### As built (M5): P06 and P07

Owner B's parts landed in five commits: the client API and the facade parsers (`fabric-client`, the TypeScript SDK), the node link's two messages (`fabric-net`), the pure placement function (`fabric-core`), the token rows (`fabric-journal`), and the node modules with the CLI (`fabricd` facade, placement and the M5 client ops; `fabric managed` and `fabric facade`). The code is the authority on signatures; this note lists where it differs from the contract above and why. The selections stand as selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe.

Client API (owner B):

- `managed_create` keeps its required `repo`: `protocol/required.txt` pins it under api 1, and making it optional is a change `scripts/check-fixtures.sh` refuses without a bump. A source that is not a repository on the node (`git`, `snapshot`, `empty`, `directory`) travels in a new op, `managed_submit`, whose body is the facade's `submit` (`goal`, `harness`, `acceptance`, `source`, `policy`). `managed_create` gains `acceptance` and `policy`, `managed_cancel` gains `task`, and a task of `ManagedJob` gains `class` and `receipt`, all optional on the wire.
- `managed_inspect` and the facade's `inspection` are one object (`fabric_client::api::Inspection`): the job with each task's class and last receipt (the CLI prints the receipt under its task: status, exit code, cut, snapshot and reason), `parked`, `placements`, `continuations`, `notes`, `provider_operations`, `blockers` and `residuals`. A rank reason and an exclusion reason are kept by a client as they came (a name, or an object keyed by the reason's name), so a reason a newer node sends still shows; their forms are in docs/facade-protocol.md, "Placements in an inspection".
- The nine new ops are `managed_note`, `managed_checkpoint`, `managed_continue`, `managed_inspect`, `managed_submit`, `managed_import`, `facade_mint`, `facade_revoke` and `facade_call`; `facade_call` carries the facade message and its answer as JSON values the node never rewrites.

Placement (owner B, P06):

- `place` is a pure function of its input, with a property test over the order of the probes (`crates/fabric-core/tests/managed_place.rs`). The eligible set is ordered by one total key, the current healthy placement, the bundle chain, the workspace, the lowest 1-minute load, the node identity; the excluded set keeps the order it was probed in. A node probed twice counts once.
- The known-writer version check is not a placement filter: `PlacementInput` carries no writer, and the fidelity a target can offer is decided at the continuation (`offered_fidelity`). `Exclusion::HarnessVersion` is defined and `place` never emits it. A harness is matched by adapter, and for an ACP agent by program; an agent found on the target's `PATH` is advertised at the placeholder version `0.0.0`.
- `fabricd::placement` is functions on `Node`, not a `Placer` struct: `own_advert` measures now, `probe_candidates` asks every paired peer in parallel within `PROBE_BOUND`, and `place_job(&ManagedJob, &PlacementAsk)` decides. The secrets a node cannot resolve are asked of the executor's own `LocalResolver`, so the exclusion and the launch agree. The load is read from `/proc/loadavg` or `sysctl -n vm.loadavg`; a node whose load cannot be read ranks last. The authority's dispatch of a create's and a continue's placement through `place_job` is owner A's wiring and had not landed when this was written.
- On the wire (`fabric-net`), `Advertise` carries the job, its harness, its root on the asking node, the snapshot for warmth and the secret names; `Advert` carries the advert in bounded forms (`AdvertWire`) plus `secrets_unresolved`, and versions travel as text the asking node parses.

Facade (owner B, P07):

- The parsers are shared by the two clients: `fabric_client::facade` and `sdk/typescript/src/facade.ts`, each tested against every case of `protocol/facade/v1` on both directions. A message that is not JSON is `bad_request`/`bad_string`.
- A command's key is stored under the token's actor as `facade:<actor>:<key>`; a pair longer than an operation key carries the key's digest instead (`facade:<actor>:h:<16 bytes of blake3, hex>`). Both forms are deterministic, so a repeat lands on the same row and two actors never share one. The owner's socket refuses a key that starts with `facade:`, so the owner's keys and an actor's never share a row. A `checkpoint` or `continue`, whose body the authority derives from the job's state (the live attempt, the task), is deduplicated by the client's request instead of the derived body: `Authority::submit_derived` keeps the request's digest in the journal's `request_keys` table (schema version 3) and answers the same request under the same key with the stored answer, however the job moved since; the owner's socket ops `managed_checkpoint` and `managed_continue` go the same way.
- The watch keeps no per-job broadcast buffer. The journal is the queue: a watch reads up to the window plus one after its cursor, answers at most the window with `lagged` when more was behind, and with nothing after the cursor waits on the authority's commit counter (one tick per call that changed the journal) and reads again when it moves, until its wait is over. A revoke moves the counter, so an open watch under a revoked token re-checks and closes `revoked` within one delivery; a read moves nothing (`Authority::with_journal` reads, `write_journal` writes), and a token that expires during the wait closes the watch at its expiry. With nothing after the cursor on a finished or cancelled job the answer closes `finished`. `next` is `null` only when the job has no event after a `null` cursor.
- `checkpoint` on a job with no attempt at all is refused `not_live` without a stored key; with an attempt that is not live the domain refuses it under the key. `continue` names the live attempt's task, or the primary task when none is live.
- The bearer is 32 bytes read from `/dev/urandom`, shown once. A mint before any managed job makes the journal, as a first create does. `facade_revoke` on a node without a journal answers `revoked: false`.
- The authority gained two small additions for this part, in owner A's `authority.rs`: `with_journal`, which runs a closure in the authority's thread (the token rows, the inspect read, the watch read), and a `watch::Receiver<u64>` commit counter. `fabric-journal` gained `provider_operations(job)` beside `residuals`, for the inspection.

### Launch path (M5, as built)

Status: contract written 2026-11-08, before the code, under the selection "M5 launch path, selected 2026-11-08" of the decisions file (option (a): M5 adds the launch path). The selection was made by the implementation agent under Efe's delegation of 2026-11-02 and is not approved by Efe. The shared types below are in `crates/fabric-core/src/managed/launch.rs` and the amended `model.rs` since this contract's commit; the fabric-net messages, the authority side and the node side follow, each owner appending an "as built" paragraph here where the code differs. The gap this closes is docs/evidence/m5/README.md, "Blocked": M5 placed and granted attempts and never launched one.

Failure modes first:

1. **A node acts on a grant only while it is the job's live grant.** Every message from a node carries a `GrantToken { job, attempt, generation }`. A `Report` whose generation is not the job's is refused `StaleGeneration { current }`, recorded as `StaleObservationRefused` (replay rule 7) and never applied; a heartbeat is answered with the fence's own verdict for each grant (`FenceVerdict`, ADR 0028), and a verdict that ends the grant (`FenceVerdict::ends_grant`: every verdict but `Live` and `NotWritable { unknown }`) stops the attempt's groups. A report the fence refuses is answered with the refusal that names the same verdict (`ReportRefusal::fenced`), and a refusal that ends the grant (`ReportRefusal::ends_grant`) stops it the same way.
2. **A launch is idempotent by attempt.** The node answers a second `Launch` of an attempt it runs `AlreadyLaunched`; so an authority that restarts after it sent one sends it again, and nothing is launched twice. The authority keeps no "launch sent" event: the grant is the durable fact, the node's record is the other half.
3. **An answer says the node took the message, never what came of it.** `Launch` and `Instruct` are answered at once (`Accepted` or `Refused { reason }`) inside the link's reply window; a clone, a pull or a drain budget takes longer than that window. Every outcome (`Started`, `Receipts`, `Checkpointed`, `HandoffBlocked`, `Continued`, `Exited`) is a `Report`, in order, one at a time per attempt.
4. **A refusal is never lowered.** A target that can offer no allowed fidelity starts nothing: it reports the harness receipt `Blocked { reason }` and `Exited { Failed }`; the continue fails and the task stays open. Nothing starts at `workspace_only` because `native_session` was asked.
5. **A report that cannot reach the authority is kept, in order, and retried after the node's next heartbeat answer.** The attempt runs on; disconnect is not cancel (facade failure mode 5). A node restart loses the queue: `Executor::recover` stops what the launch records name, and the attempt's fate is M6's (`unknown` for want of heartbeats).
6. **Nothing a node reads from a message is trusted past its check.** `Program::check`, `JobPolicy::check`, `WorkPackage` size and digest, `tasks.len() <= MAX_TASK_LAUNCHES` and `attempts.len() <= MAX_HEARTBEAT_ATTEMPTS` run on the receiving side before anything is served, on top of the frame bound.

#### Shared types (fabric-core, `managed::launch`, landed with this contract)

```rust
pub const MAX_ARGV: usize = 64;
pub const MAX_ARG_BYTES: usize = 4096;
pub const MAX_ENV_ALLOWLIST: usize = 64;
pub const MAX_TASK_LAUNCHES: usize = 256;
pub const MAX_HEARTBEAT_ATTEMPTS: usize = 64;
pub const HEARTBEAT_MS: u64 = 2_000;

/// What a build or test task runs; a harness task has none.
pub struct Program { pub argv: Vec<String>, pub cwd: Option<RelPath>, pub env_allowlist: Vec<EnvVarName> }
pub enum ProgramError { EmptyArgv, TooManyArgs { count }, ArgTooLong { index, bytes }, ControlCharacter { index }, TooManyEnv { count }, CredentialLike { index } }
impl Program { pub fn check(&self) -> Result<(), ProgramError>; }

pub struct GrantToken { pub job: JobId, pub attempt: AttemptId, pub generation: Generation }   // M6's token
pub struct TaskLaunch { pub task: TaskId, pub class: TaskClass, pub program: Program }
pub struct ContinuationAsk { pub from: AttemptId, pub allowed: AllowedFidelity }

pub struct LaunchRequest {
    pub token: GrantToken,
    pub task: TaskId,                         // the harness task the attempt runs
    pub intent: Intent,
    pub source: SourceBinding,
    pub policy: JobPolicy,
    pub initial_checkpoint: Option<SnapshotId>,
    pub snapshot_at: Option<NodeId>,          // whose store holds the chain; required with initial_checkpoint
    pub tasks: Vec<TaskLaunch>,               // open build and test tasks under `task` with a program
    pub continuation: Option<ContinuationAsk>,
    pub package: Option<WorkPackage>,         // Some exactly when continuation is
}
pub enum LaunchAnswer { Accepted, Refused { reason: LaunchRefusal } }
pub enum LaunchRefusal { AlreadyLaunched, Superseded { current: Generation }, HarnessMissing { harness: HarnessKind }, SecretUnresolved { name: SecretName }, BadProgram { task: TaskId, error: ProgramError }, Inconsistent { reason: String }, Internal { reason: String } }

pub enum Instruction { Spawn(TaskLaunch), Checkpoint { then: AfterCut }, Stop { reason: String } }
pub enum AfterCut { Resume, End }
pub enum InstructAnswer { Accepted, Refused { reason: InstructRefusal } }
pub enum InstructRefusal { NotRunning, Superseded { current: Generation }, Frozen, Busy { what: String }, BadProgram(ProgramError), Internal { reason: String } }

pub struct Report { pub token: GrantToken, pub observation: Observation }
pub enum ReportAnswer { Recorded { revision: WorkRevision }, Refused { reason: ReportRefusal } }
pub enum ReportRefusal { StaleGeneration { current: Generation }, UnknownJob, UnknownAttempt, WrongNode, TokenMismatch, NotReportable, Domain { reason: String }, Unavailable }
impl Report {
    /// The attempt and generation of a reportable observation (Started, Receipts, Checkpointed, HandoffBlocked, Continued, Exited); None for Placed, Lost, Unknown.
    pub fn reported(observation: &Observation) -> Option<(AttemptId, Generation)>;
    pub fn check(&self) -> Result<(), ReportRefusal>;   // NotReportable, then TokenMismatch
}

pub struct Heartbeat { pub attempts: Vec<GrantToken> }
pub struct HeartbeatAnswer { pub verdicts: Vec<(AttemptId, FenceVerdict)> }
// HEARTBEAT_MS is HEARTBEAT_INTERVAL_MS, the interval presence counts in.
```

Model amendments (fabric-core `model.rs`, codec format 2 amended in place, unshipped):

```rust
pub enum Command {
    // and:
    RegisterTask { parent: TaskId, kind: TaskKind, restart: RestartPolicy, class: TaskClass, program: Option<Program> },
}
pub enum ManagedEvent {
    // and:
    TaskRegistered { task: TaskId, parent: Option<TaskId>, kind: TaskKind, restart: RestartPolicy, class: TaskClass, program: Option<Program> },
}
pub struct Task { /* M5 fields, and */ pub program: Option<Program> }
pub enum EffectRequest { /* and */ Spawn { task: TaskId } }
pub enum DomainError { /* and */ ProgramRequired { class: TaskClass }, ProgramNotAllowed { class: TaskClass }, BadProgram { error: ProgramError } }
```

Rules (owner A, `transition.rs`, each with a test in `managed_rules.rs` that fails on this commit):

- `RegisterTask` with class `Build` or `Test` needs `Some(program)` (`ProgramRequired`); with `Harness` or `Opaque` it refuses one (`ProgramNotAllowed`); `program.check()` failing is `BadProgram`. All three are refused before the journal records anything, like `BadPolicy`.
- `RegisterTask` with a program is `accepted` with `effect: true` and `EffectRequested { Spawn { task } }` when the parent task has a live attempt (`granted` or `running`); its dispatch is `Instruct(Spawn)` to that attempt's node, `succeeded` on `Accepted`, `failed` on `Refused`. With no live attempt it is `succeeded` at commit as in M2, and the task rides in the next `LaunchRequest.tasks`. `CommandKind::has_effect(RegisterTask)` is `true`.
- A granted attempt of a continue that ends (`Exited`, any outcome) before `Continued` marks the continue `failed`: the target could not offer an allowed fidelity or could not start, and the task stays open for another `Continue`.
- Format 1 widens `TaskRegistered` with `program: None` (`frame::v1`, done). The `tasks` projection gains `program BLOB` (framed `Program`, null for none) in the version 3 DDL, amended in place; `tests/fixtures/v3.sqlite` is made again by `make_v3_fixture` and `the_v3_fixture_holds_what_it_was_made_with` pins a child with a program. A version 3 file without the column was written by the earlier draft and is refused at `open` as `DraftSchema { version: 3 }`, the refusal M6 adds for the same reason, so the two branches converge on it.

#### fabric-net, protocol 4 (owner A), additive

```rust
pub enum RequestBody {
    // M5 so far, and:
    Launch(Box<LaunchRequest>),                              // authority to the chosen node
    Instruct { token: GrantToken, instruction: Instruction }, // authority to the node that runs the attempt
    Report(Box<Report>),                                      // node to the authority
    Heartbeat(Heartbeat),                                     // node to the authority, every HEARTBEAT_MS while it runs an attempt
}
pub enum ResponseBody {
    // and:
    Launch(LaunchAnswer), Instruct(InstructAnswer), Report(ReportAnswer), Heartbeat(HeartbeatAnswer),
}
pub trait Service {
    // and, each with a default that refuses (`Fault::BadRequest`), as `advertise` and `facade` have:
    fn launch(&self, peer: NodeId, request: LaunchRequest) -> BoxFuture<'_, Result<LaunchAnswer, Fault>>;
    fn instruct(&self, peer: NodeId, token: GrantToken, instruction: Instruction) -> BoxFuture<'_, Result<InstructAnswer, Fault>>;
    fn report(&self, peer: NodeId, report: Report) -> BoxFuture<'_, Result<ReportAnswer, Fault>>;
    fn heartbeat(&self, peer: NodeId, beat: Heartbeat) -> BoxFuture<'_, Result<HeartbeatAnswer, Fault>>;
}
impl Session {
    pub async fn launch(&self, request: LaunchRequest, deadline: Duration) -> Result<LaunchAnswer, RpcError>;
    pub async fn instruct(&self, token: GrantToken, instruction: Instruction, deadline: Duration) -> Result<InstructAnswer, RpcError>;
    pub async fn report(&self, report: Report, deadline: Duration) -> Result<ReportAnswer, RpcError>;
    pub async fn heartbeat(&self, beat: Heartbeat, deadline: Duration) -> Result<HeartbeatAnswer, RpcError>;
}
```

All four travel in a control frame (`MAX_CONTROL_FRAME`, which holds a 1 MiB record page): a `LaunchRequest` is at most `PACKAGE_MAX_BYTES` of package plus the job's own fields, and a `Report` carries one `CheckpointCut` at most. They run under the per-peer job work cap like `Advertise`. The fixtures are [protocol/net/v4](../protocol/net/v4) (docs/net.md, "Launch path fixtures"); `crates/fabric-net/tests/launch_fixtures.rs` pins the manifest and builds every case.

#### fabricd, the authority side (owner A)

```rust
// dispatch.rs (extend): one loop, read anew after every commit, as now for creates
pub(crate) async fn run(node: Arc<Node>);
// For each job, in this order, each at most once per process until the journal moves:
//   CreateJob accepted            -> place (as built), the Placed grants or parks
//   Continue dispatched/accepted  -> live attempt: mark Dispatched, Instruct(Checkpoint { then: End }) to its node;
//                                    no live attempt: place with PlannedHandoff from the task's last cut (domain, as built)
//   Checkpoint accepted           -> mark Dispatched, Instruct(Checkpoint { then: Resume })
//   CancelJob, CancelTask accepted with a live attempt -> mark Dispatched, Instruct(Stop { reason })
//   RegisterTask accepted with Spawn requested -> mark Dispatched, Instruct(Spawn(TaskLaunch)); Accepted: Succeeded,
//                                    Refused Busy or Frozen: sent again on a later pass, any other refusal: Failed
//   a cancel's Stop that did not reach its node or was answered Busy is sent again until its node takes it (a restart
//     sends each unsettled one once more); NotRunning ends the attempt Exited { Cancelled } here, which settles the cancel
//   a grant with a cancel pending is not launched; its Stop ends it unlaunched
//   every attempt `granted` on a remote node -> launch::plan then launch::send; Accepted and AlreadyLaunched record nothing
//     (Started comes by report); Superseded records nothing; any other refusal is observed as Receipts (harness receipt
//     Blocked { reason }) then Exited { Failed { code: None } }, so the attempt fails and the task is free
// Fault points: `before_launch` (after the grant's commit, before Launch is sent), `after_launch_accepted`.

// launch.rs (new)
pub(crate) struct LaunchPlan { pub request: LaunchRequest, pub node: NodeId }
/// Builds the request from the journal: intent, source and policy from the job; `initial_checkpoint` from the grant;
/// `snapshot_at` the node of the attempt that cut it (an import's snapshot: this node); `tasks` the open Build and Test
/// tasks under `task` with a program; `continuation` and `package` for a grant a Continue made.
pub(crate) fn plan(journal: &Journal, state: &ManagedState, attempt: AttemptId) -> Result<LaunchPlan, AuthorityError>;
/// This node: `Node::runner_launch(None, request)` (owner B); a peer: `Session::launch` with the link's deadline.
pub(crate) async fn send(node: &Arc<Node>, plan: LaunchPlan) -> Result<LaunchAnswer, NodeError>;
pub(crate) async fn instruct(node: &Arc<Node>, target: NodeId, token: GrantToken, instruction: Instruction) -> Result<InstructAnswer, NodeError>;

// package.rs (new): the package of a planned handoff, from journal state at the cut
pub(crate) fn build_for_continue(journal: &Journal, state: &ManagedState, from: AttemptId) -> Result<WorkPackage, PackageError>;
// PackageInput: notes = Journal::notes(job); receipts = Journal::receipts(job, from); workspace = { bundle_head: the cut's
// snapshot, ledger_head: the digest of the job's latest event }; evidence = the cut's; artifacts = none in M5.

// report.rs (new): what a node sends, accepted into the journal
impl Node {
    /// `Report::check`, then the grant's node against `from` (`None` is this node; another node is `WrongNode`), then
    /// `Authority::observe`; `DomainError::StaleGeneration` answers `StaleGeneration { current }` (the domain recorded it),
    /// another domain refusal answers `Domain { reason }` by its snake-case name, a closed authority `Unavailable`.
    pub(crate) async fn accept_report(&self, from: Option<NodeId>, report: Report) -> ReportAnswer;
    /// One verdict per token: `Live` when the token is the job's live grant on `from`, `Stale { current }` when the job's
    /// generation is newer, `Unknown` otherwise. Writes nothing.
    pub(crate) async fn accept_heartbeat(&self, from: Option<NodeId>, beat: &Heartbeat) -> HeartbeatAnswer;
}
```

#### As built (owner A)

Where the code differs from the shapes above, or adds to them:

- `LaunchRequest::check` (fabric-core) is the receiving side's check of a launch: at most `MAX_TASK_LAUNCHES` tasks, every program through `Program::check` (`BadProgram`), `snapshot_at` with an initial checkpoint, a package exactly with a continuation, for the token's job and at most `PACKAGE_MAX_BYTES` (`Inconsistent`). The authority runs it on every plan; the runner runs it again before it records a launch.
- fabric-net: `RequestBody::check_bounds` refuses a launch past `MAX_TASK_LAUNCHES` tasks and a heartbeat past `MAX_HEARTBEAT_ATTEMPTS` grants when a request is parsed (`Request::from_control_payload`) and before `Session::launch` or `Session::heartbeat` sends one. The four messages are served under the per-peer job work cap like a facade message: the work runs to its end, only the answer races the budget.
- `launch::send` and `launch::instruct` are the `Link` trait, which the dispatch takes so its tests can record what is sent; `NodeLink` is the node's. A grant on this node is sent through the node's own `Service::launch` with its own id as the peer, not by calling `Node::runner_launch(None, ..)`: owner B's service maps its own id to `None`, so the runner takes a launch from its own authority by the same path as from a peer, and this branch builds without the runner. `plan` answers `PlanError::{Authority, Blocked}`: a journal that cannot be read is tried again, a grant that cannot be launched as recorded (no package, a failing check) is failed like a refused launch.
- `package::build_for_continue` answers `BuildError::{Journal, NoCut, Package}`. It reads two journal reads added for it: `Journal::head` (the job's newest event digest, the ledger head) and `Journal::last_cut` (the newest cut's snapshot and digest, the evidence; the cut's own evidence list is not journaled).
- `report::accept` and `report::heartbeat` take the `Authority` and the node's id, so their tests need no daemon; `Node::accept_report` and `Node::accept_heartbeat` call them. A report the domain answers with `StaleObservationRefused` is answered `StaleGeneration { current }`; one that names a generation newer than the job's is the domain's `StaleGeneration` refusal and answered the same.
- The dispatch launches only grants a `CreateJob` or a `Continue` made; a `StartAttempt` grant stays the CLI's to start, as in M2. One pass launches the grants first, then dispatches the pending effects, then places every dispatched continue whose attempt ended at its cut, then launches what that granted. A launch that does not reach its node is tried again after the next commit or `RETRY` (2 s); a checkpoint or a spawn that does not reach its node marks its command `Unknown`, since the node may have taken it; a cancel's stop that does not reach its node, or finds it busy, is sent again until taken, and one answered `NotRunning` ends the attempt cancelled; a spawn answered `Busy` or `Frozen` is sent again on a later pass; any other refusal marks the command `Failed`; a cancel is settled by any exit of its attempt, not only a cancelled one; an effect whose attempt ended before it was dispatched is marked `Dispatched` then `Failed`.
- `crash_tests::a_launch_killed_after_the_node_accepted_is_not_launched_twice` is `dispatch::crash_tests`, a restart simulated by a dispatch that remembers nothing over the same journal; the kill at `after_launch_accepted` with two daemons needs owner B's runner and belongs with the e2e acceptance.
- The v3 fixture's test child is registered while the first job's grant may still run, so it asks for a spawn, marked dispatched then succeeded. `tests/fixtures/v3-draft.sqlite` is the v3 fixture before the `program` column, for the `DraftSchema { version: 3 }` test.

#### fabricd, the node side (owner B)

```rust
// runner.rs (new): the attempts this node runs for an authority
impl Node {
    /// Takes a grant: checks (`Superseded`, `HarnessMissing`, `SecretUnresolved`, `BadProgram`, `Inconsistent`), writes
    /// the launch record, answers, and runs the attempt in a task of its own. `from` is the authority (`None`: this node).
    pub(crate) async fn runner_launch(self: &Arc<Self>, from: Option<NodeId>, request: LaunchRequest) -> LaunchAnswer;
    pub(crate) async fn runner_instruct(self: &Arc<Self>, token: GrantToken, instruction: Instruction) -> InstructAnswer;
}
// The runner loop of one attempt:
//   1. Executor::begin(attempt, job, generation, paths::job_root(home, job), policy.secrets)
//   2. the root: initial_checkpoint Some -> transfer::fetch_snapshot from snapshot_at when the store lacks it, restore the
//      workspace, and the harness state too when the continuation allows native_session and the adapter can; None ->
//      sources::prepare(node, job, &source). A failure here is reported Receipts (the harness receipt Blocked { reason })
//      then Exited { Failed { code: None } }; nothing was started.
//   3. a continuation: offered_fidelity(&intent.harness, the restored manifest's known writer, own advert, same layout),
//      then allowed.highest(offered); Err -> the Blocked and Exited reports of step 2 with the refusal as the reason.
//   4. the harness task through its adapter: Launch::Headless { resume: the restored session for native_session, else None;
//      prompt: the goal, and the package's canonical JSON written beside the root for new_native_session_with_package },
//      the job's secrets in its environment under their names; the process is registered with the executor
//      (Executor::adopt, below). workspace_only starts no harness.
//   5. Report Started; for a continuation then Report Continued { from, achieved, package: Some(digest) when used };
//      workspace_only then reports Exited { Succeeded } and ends.
//   6. Executor::spawn each TaskLaunch (class Build or Test) in the root, cwd under it, env_allowlist applied; one that
//      does not start is reported in Receipts as Blocked { reason }.
//   7. Heartbeat every HEARTBEAT_MS with every token this node runs; Stale or Unknown -> Executor::stop(budget), end,
//      no report; a failed heartbeat, or an answer with no verdict for the grant (the authority could not read the
//      job), is retried next interval and flushes the report queue when it answers.
//   8. the harness process exits -> Report Receipts (every task as it stands) then Exited { Succeeded | Failed { code } }; end.
//   9. Instruct Checkpoint { then }: Executor::quiesce(budget); blockers -> Report HandoffBlocked, Executor::resume;
//      none -> Executor::cut; End -> Executor::stop(budget) of what still runs before the cut is reported, since the
//      authority places the replacement once it is recorded; Report Checkpointed { snapshot, cut }; Recorded and
//      Resume -> Executor::resume; Recorded and End -> Executor::end; End refused -> Report Receipts then
//      Exited { Failed { code: None } }; Refused StaleGeneration -> stop, end.
//  10. Instruct Stop { reason }: Executor::stop(budget) -> Report Receipts (interrupted) then Exited { Cancelled }; end.
//  11. Instruct Spawn(t): Executor::spawn; Frozen or Busy are refused as such.
//  Reports go out in order per attempt; one in flight; a link failure or `Unavailable` keeps the queue for the next
//  heartbeat answer. Counters: launches taken, reports sent, reports refused, heartbeats, stale verdicts obeyed.

// executor.rs (owner B this step): one addition
impl Executor {
    /// Registers a harness process the adapter spawned, in its own group, as the attempt's harness task; the launch
    /// record is written as for `spawn`. Refused while the attempt is frozen.
    pub async fn adopt(&self, attempt: AttemptId, task: TaskId, pid: u32, started: Option<String>) -> Result<Spawned, ExecutorError>;
}

// capturer.rs (new): the real `Capturer`
pub(crate) struct NodeCapturer { /* the node, the job, the root, the harness adapter and env, the store, the export allowlist */ }
impl Capturer for NodeCapturer {
    // checkpoint_harness: the adapter's suspend path (`capture`, the marker, the manifest) into the store, as `job suspend` does
    // capture: fabric-store capture of the root with IgnorePolicy::allow_export for the allowlist
    // facts: NodeFacts::measure for the job's harness, as `NodeFacts::iter` gives them
}

// service.rs (owner B): the four Service methods, each one line, launch and instruct to the runner, report and
// heartbeat to owner A's accept_report and accept_heartbeat with `Some(peer)`.
```

The client surfaces (owner B): the client API gains `managed_register` (api 1, additive): `operation_key`, `job`, optional `expected_revision`, `parent`, `label`, `class` (`build` or `test`), `program { argv, cwd?, env_allowlist }`, answered with `ManagedOperation` like `managed_note`; its fixtures `protocol/fixtures/request/managed_register.json`, `managed_register_minimal.json` and `reply/managed_register.json` are in this contract's commit, and `protocol/required.txt` is regenerated by the contract test once the parsers land. `fabric managed register <job> --parent <task> --label <text> --class build|test [--cwd <rel>] [--env NAME]... -- <argv...>`. A task in `managed_status`, `managed_inspect` and the facade's `inspection` carries `program` when it has one; the `task_registered` event carries it too and `effect_requested` has the form `{ "spawn": { "task" } }` (docs/facade-protocol.md, "Launch path"). The TypeScript SDK gains `managedRegister`. A `managed_finish` for a child task is not in this step: the acceptance needs an open, interrupted task, not a finished one.

The provider read-surface row, carried from M4, closes in this step on owner B's side: `fabricd::facade::tests::an_inspection_lists_every_provider_status` over a journal seeded with one provider operation per status and the blockers and residuals they make, and `managed::e2e_the_cli_inspects_provider_operations_blockers_and_residuals` in the e2e suite for `fabric managed inspect`.

#### As built (owner B)

Where the code differs from the shapes above, or adds to them:

- The runner is `runner::Runner`, held by the node beside its executor: `Runner::launch(from, request)` and `Runner::instruct(from, token, instruction)`. There is no `Node::runner_launch` or `Node::runner_instruct`; the four `Service` methods are the entry, and each maps the node's own id to `None`, so a grant from this node's authority, sent through `NodeLink` with its own id, takes the same path as one from a peer. An instruction from another authority than the one that launched the attempt is answered `NotRunning`.
- The node's effects (the root, the harness, the task command, the report and heartbeat links, the capturer's attempt) are the `runner::Host` trait; `NodeHost` is the node's, and `runner::tests` runs the loop against a fake host and a real executor.
- A launch taken is recorded in `runs/taken/<attempt>.json` before it is answered `Accepted`, so a launch sent again, after either side restarted, is `AlreadyLaunched`. The executor's launch records are under `runs/records`, and the node stops what they name at start-up (`Executor::recover`) before anything new is launched.
- The root is prepared (step 2) before `Executor::begin` (step 1): a failed preparation begins nothing. A root a previous attempt of the job left on this node is set aside as `<root>.before-<attempt>`, never removed.
- A heartbeat names one grant: each attempt runs its own heartbeat loop. A `Live` answer, or one that names nothing of the grant, lets a waiting report go again.
- Native session continuity (selection "M5 native session continue", made 2026-11-08 by the implementation agent under Efe's delegation of 2026-11-02 and not approved by Efe). The cut carries the harness state: `NodeCapturer::capture` puts the state the same quiescence's harness checkpoint captured through the adapter's suspend path into the cut's snapshot, once, so a later cut without a checkpoint carries none. On a continuation that allows `native_session`, `NodeHost::prepare` restores the cut's harness state with the workspace into the node's own copy (`runs/restored/<attempt>`, removed once used) and reads its manifest. The layout is the same when the harness, the physical job root and the home all equal the ones the manifest was captured under (ADR 0011): then `offered_fidelity` is asked with the same layout and the manifest's writer as the known writer, the adapter's `restore` places the state as `job resume` does for Claude Code and Codex, and the harness starts with `resume` set to the captured session; `Continued` records `native_session` and no package. Any other layout, or a cut with no harness state, offers `new_native_session_with_package` as before. A restore the adapter refuses is the launch's failure: the harness receipt `Blocked { reason }` and `Exited { Failed }`, never a lower fidelity. The decision is the pure `runner::continuation_offer` (`runner::tests::a_native_session_is_restored_only_for_the_same_layout`); the cut's state is `capturer::tests::a_cut_after_a_harness_checkpoint_holds_its_state`.
- The harness runs with the daemon's environment less every `FABRIC_SECRET_*` variable, plus the job's secrets under their names, as a CLI run passes the user's environment; its output is read for the session (`read_headless_line`, or an ACP agent's turn feed) and dropped. A build or test task gets `PATH`, `HOME` and its allowlist only, the job's secrets from the executor, and no terminal.
- `NodeCapturer` captures the harness's state through its adapter once its session is known, and the workspace alone before that; the facts a cut records are measured at its capture. `Executor::adopt` refuses a pid that leads no group (`NotAGroupLeader`).
- The executor marks a task it signalled, so a test a checkpoint interrupted is still `Interrupted` at the next cut (`an_interrupted_test_stays_interrupted_at_the_next_cut`), not read as exited.
- `managed_register` registers with the `explicit_only` restart. A program in a request or a reply skips a field it does not know, like any object outside a run plan; the node checks it again. The provider read surface adds `purpose`, `target`, `searched` and `abandon_reason` to a provider operation and `searched` to a residual, each optional; the fixtures are `reply/managed_inspect_provider_operations.json` and `hello_answer/welcome_with_the_launch_path.json`.
- The e2e fake codex gains `FAKE_CODEX_HOLD`, one process that holds its turn open: its `FAKE_CODEX_WAIT` loop forks a `sleep` per tick, an escaped child that rightly blocks a strong handoff. Under `FAKE_CODEX_HOLD` it keeps its rollout open until it is stopped, as a Codex with the thread loaded does, so the adapter's `lsof` check refuses a capture of it while it runs, as it refuses a real one.
- The harness's own state is captured only from a harness that has stopped, as `job suspend` stops it before its capture (ADR 0015): a running Claude Code refuses with `HarnessRunning`, and a running Codex holds its writer lock and its rollout open. `Executor::quiesce(attempt, budget, then)` takes the `AfterCut`: under `End` it settles every other task and computes the group blockers first, and only when nothing blocks the handoff stops the harness task (`SIGTERM`, the drain budget left, at least the minimum, `SIGKILL`) before `Capturer::checkpoint_harness(attempt, task, HarnessRun::Stopped)`; a blocked handoff leaves the harness running, its receipt `Blocked` and nothing captured. Under `Resume` the harness is never signalled and `HarnessRun::Running` captures the workspace alone, so a checkpoint that resumes offers no native session from its cut. A capture that fails after the stop reports `HandoffBlocked`, and the harness's exit then ends the attempt `Failed`, as a `job suspend` whose capture fails after its stop fails (`executor::tests::{a_cut_that_ends_the_attempt_stops_the_harness_before_its_capture, a_blocked_cut_that_would_end_the_attempt_leaves_the_harness_running, a_cut_that_resumes_leaves_the_harness_running_and_its_state_alone}`; e2e `continuity::e2e_a_native_session_continues_on_a_node_with_the_same_layout` with the rollout held open).
- A cut's departure is confirmed once the authority records it: `Capturer::confirm_departure(attempt)`, after the `Checkpointed` report is answered `Recorded`, reads the manifest of the state the cut carried and calls the adapter's `confirm_departure`, as `job suspend` does after `SnapshotPublished` (ADR 0015). A refused cut confirms nothing, so its state never counts at a restore here; a confirmation that fails is logged, and the next restore of a rewritten file on this node then refuses (`StateDiverged`) rather than overwrite it. So a Claude Code session that continues natively to another node and back takes the project memory and task files it rewrote there (`capturer::tests::a_recorded_cut_confirms_its_departure_so_a_rewritten_session_comes_back`, which fails with `StateDiverged` on `MEMORY.md` without the confirmation).
- A restore failure is never a lower fidelity: `runner::offer_restoring` reads the cut's state, asks `continuation_offer` and restores through the adapter; an unreadable state or a refused restore is an error that fails the launch (`runner::tests::a_restore_failure_fails_the_launch_never_a_lower_fidelity`, over a real Codex capture refused as divergence after this node's copy of the thread grew, and a manifest that does not parse). The continuity test runs the fake codex, not the fake ACP agent, and follows the handoff's placement. A continue's `pin` now reaches `dispatch::place_handoff`, which places the handoff with it in place of the job's pin (`dispatch::tests::a_continue_s_pin_places_the_handoff_where_it_names`).
- Counters: launches taken, reports sent, reports refused, heartbeats answered, stale verdicts obeyed (`Runner::counters`).

#### Acceptance mapped to tests, the launch path

Each test is written first and fails on this contract's commit.

| Item | Tests |
| --- | --- |
| The program rules and the Spawn effect (owner A) | `fabric-core` `managed_rules::{a_build_or_test_task_needs_a_program, a_harness_or_opaque_task_takes_no_program, a_bad_program_is_refused_before_the_journal, a_program_registered_under_a_live_attempt_asks_for_a_spawn, a_continue_s_grant_that_ends_before_continued_fails_the_continue}`; `fabric-journal` `fixtures::the_v3_fixture_holds_what_it_was_made_with` (a child with a program), `a_draft_v3_without_the_program_column_is_refused` |
| Protocol 4 carries the launch path (owner A) | `fabric-net` `tests/launch_fixtures.rs::{the_manifest_is_the_pinned_one, every_case_builds_and_round_trips_in_a_control_frame, a_case_marked_reject_is_refused_where_it_says}`; `wire::tests` property round trips widened to the four messages |
| The authority dispatches and fences (owner A) | `fabricd` `report::tests::{a_report_at_an_older_generation_is_refused_and_recorded, a_report_from_another_node_than_the_grant_is_refused, a_report_whose_token_and_observation_differ_is_refused_before_the_journal, a_heartbeat_names_each_grant_live_stale_or_unknown}`; `dispatch::tests::{a_granted_remote_attempt_is_launched_once_per_process, an_already_launched_answer_is_not_a_failure, a_refused_launch_fails_the_attempt_and_frees_the_task, a_continue_with_a_live_attempt_asks_for_its_cut_then_places_from_it, a_checkpoint_and_a_cancel_are_instructed_to_the_live_node, a_spawn_is_instructed_and_settled_by_the_answer}`; `package::tests::a_package_is_built_from_the_journal_at_the_cut`; `crash_tests::a_launch_killed_after_the_node_accepted_is_not_launched_twice` |
| The node runs, heartbeats, cuts and reports (owner B) | `fabricd` `runner::tests::{a_launch_is_idempotent_by_attempt, a_launch_of_a_superseded_generation_is_refused, a_stale_or_unknown_heartbeat_verdict_stops_the_attempt, a_checkpoint_instruction_cuts_then_resumes_or_ends, a_stop_instruction_reports_every_task_interrupted_then_cancelled, a_report_refused_stale_stops_the_attempt, a_preparation_failure_is_reported_blocked_then_failed, an_unallowed_fidelity_starts_nothing, reports_leave_in_order_and_wait_for_the_link}`; `executor::tests::an_adopted_harness_process_is_in_its_own_group_and_recorded`; `capturer::tests::{a_harness_checkpoint_goes_through_the_suspend_path, a_capture_honours_the_export_allowlist}` |
| The client surfaces (owner B) | `fabric-client` `contract.rs` and the TypeScript `contract.test.ts` over the three new fixtures; `fabric-client` `facade_fixtures.rs` and `facade_fixtures.test.ts` over `events_launch_path` and `inspection_continued` (the pins moved in this contract's commit); `fabric` CLI test `managed::e2e_the_cli_registers_a_test_task_with_its_program` |
| The provider read surface (owner B, carried from M4) | `fabricd` `facade::tests::an_inspection_lists_every_provider_status`; e2e `managed::e2e_the_cli_inspects_provider_operations_blockers_and_residuals` |
| M5 acceptance end to end (both owners, two daemons on one host) | `placement::e2e_a_submit_runs_on_the_eligible_node_without_naming_it` (the attempt half: `Started` on the chosen node, the inspection's attempt `running`); `continuity::e2e_a_planned_handoff_records_child_outcomes_and_the_fidelity_achieved` (a registered test task with a program `sleep`, continued under the fake agent with the package: the test recorded `interrupted`, `achieved: new_native_session_with_package`, the package digest in the inspection); `continuity::e2e_a_native_session_continues_on_a_node_with_the_same_layout` (two daemons sharing one home, so the same user, home and job root; the first turn on `b`, a continue pinned to `a`, every rollout removed from the home before `a` launches: the resumed thread holds the first turn, which the fake harness answers from, `achieved: native_session` in the inspection and in `fabric managed inspect`); `continuity::e2e_a_continue_on_a_node_with_another_layout_starts_a_new_session_with_the_package` (the same with a home per daemon: `new_native_session_with_package`, a package digest, no resume); `secrets::e2e_a_secret_value_appears_in_no_journal_snapshot_package_receipt_or_log`; `managed::e2e_a_cancel_stops_a_live_attempt_and_records_the_tasks_interrupted`; `managed::e2e_a_checkpoint_cuts_a_live_attempt_and_it_goes_on` |
| Counters, not time | launches taken, reports sent and refused, heartbeats and stale verdicts (the runner's counters); one commit per report; one probe round per placement as before. Time is the Perf step's alone |

### Cleanup: residuals and `fabric gc` (ADR 0030)

A node killed with `SIGKILL` leaves work behind; `fabric gc` lists it and `fabric gc --yes` removes exactly what it listed. The inventory, generated from the node's gc module, is [concepts/residuals.md](concepts/residuals.md).

- The live set is never touched: the tenures the node owns, the jobs a move or an operation holds now, the managed attempts the journal holds `granted`, `running` or `unknown` at any generation, and the attempts the node's runner and executor run now. The journal's part and the provider creates are read in one call on the authority's thread (`Authority::read_journal`).
- On the launch path, an executor launch record (`runs/records/<attempt>/<task>.json`) and the group it names, a restored harness state (`runs/restored/<attempt>`), a captured one (`runs/harness-states/<attempt>`) and a work package (`<home>/fabric/jobs/<job>/package-<attempt>.json`) are residuals only when the journal on this node holds the attempt ended. An attempt this journal does not hold is kept as `unknown_attempt`; a taken record (`runs/taken/<attempt>.json`) is always kept, since it answers a launch sent again. A launch group that outlives `SIGKILL` keeps its record, in gc (the row `failed`) and in the executor's recovery at start, so a later run still finds it.
- A job record gc cannot read keeps what gc would judge by it as `unreadable`, and the run goes on to the next job, so `--yes` always answers with the report of what it removed.
- A provider resource an ended attempt made is removed through `Bridge::cleanup` and the worker named in the request, with one approval per create given in that request; nothing stores an approval. The cleanup records each step through the authority's queue (`Authority::recorder`, `fabric_bridge::Hand`), so the authority serves every other call while the worker acts.
- Tests: `fabricd` `gc::tests::{a_destroy_leaves_the_authority_serving_while_the_worker_acts, an_attempt_that_may_still_run_is_live, the_launch_path_leaves_residuals_only_for_ended_attempts, an_unreadable_job_record_keeps_that_job_and_the_run_goes_on}`, `executor::tests::a_group_that_outlives_sigkill_keeps_its_record`; `fabric-store` `crash_tests::restore_killed_at_any_checkpoint_is_detectable_and_finishes_on_rerun` (the label at every checkpoint); e2e `gc::e2e_gc_after_a_kill_lists_and_removes_only_what_superseded_tenures_left`.

#### Owners and files, the launch path

Two owners, disjoint files. Owner A lands the fabric-net messages first; owner B's first commit that uses them merges `m5` after that commit. Merge commits only. The shared types are in `fabric-core` since this contract's commit; a change to them is owner A's, announced in the commit.

| Owner | Scope | Files |
| --- | --- | --- |
| A | the program rules and the Spawn effect; protocol 4's four messages; the authority side: launch planning and sending, the dispatch of Checkpoint, Continue, CancelJob, CancelTask and Spawn, the package build, report and heartbeat acceptance with the fence; the journal's `program` column and the v3 fixture | `crates/fabric-core/src/managed/{launch,model,transition,frame}.rs`, `crates/fabric-core/tests/managed_rules.rs`; `crates/fabric-net/*` (new `tests/launch_fixtures.rs`); `crates/fabric-journal/*`; `crates/fabricd/src/{dispatch,authority}.rs`, `authority/*`, new `{launch,package,report}.rs`, `faults.rs`, `managed.rs`; `protocol/net/v4/*`; this subsection's "as built" for A and docs/net.md |
| B | the runner loop, the real `Capturer`, `Executor::adopt`, the four `Service` methods; `managed_register` on the client API, the CLI and the TypeScript SDK; the provider read-surface tests; the e2e acceptance tests | `crates/fabricd/src/{executor,service,node,ops,managed_ops,control,error,facade,harnesses}.rs`, new `{runner,capturer}.rs`; `crates/fabric-harness/*`; `crates/fabric-store/*`; `crates/fabric-client/*`; `sdk/typescript/*`; `crates/fabric/*`; `protocol/fixtures/*`, `protocol/required.txt`, `protocol/facade/v1/*`; docs/facade-protocol.md and this subsection's "as built" for B |

`crates/fabricd/src/lib.rs` gains the five `mod` lines (`launch`, `package`, `report` by A; `runner`, `capturer` by B); each owner adds its own and the merge keeps both. A file not listed is touched by neither without a note in the commit saying why.

### Owners and files

Two owners, disjoint files. Owner A lands the model commit (`fabric-core` types, format byte 2, schema version 3 DDL) first; owner B's first commit that uses it merges `m5` after that commit. Merge commits only.

| Owner | Scope | Files |
| --- | --- | --- |
| A | P03 task lifecycle and executor, P04 work package and fidelity, P05 source bindings and the local secret resolver | `crates/fabric-core/src/managed/{mod,model,transition,frame,ids}.rs`, new `{lifecycle,package,source,secret_ref,placement,facade}.rs` (the last two: types only), `crates/fabric-core/tests/managed_*.rs` except `managed_place.rs`; `crates/fabric-journal/*` except `src/facade.rs`; `crates/fabricd/src/{executor,secrets,sources}.rs` (new), `authority.rs`, `authority/*`, `managed.rs`, `harnesses.rs`, `local.rs`; `crates/fabric-harness/*`; `crates/fabric-store/*`; ADRs 0023 to 0025 and this section's parts P03 to P05 for their "As built (M5)" notes |
| B | P06 placement, P07 facade, the client surfaces for every M5 op | `crates/fabric-core/src/managed/place.rs` (new) and `tests/managed_place.rs`, plus the one `pub mod place;` line; `crates/fabric-journal/src/facade.rs` (new) plus its `mod` line; `crates/fabric-net/*`; `crates/fabricd/src/{placement,facade}.rs` (new), `ops.rs`, `control.rs`, `node.rs`, `error.rs`, `service.rs`; `crates/fabric-client/*`; `sdk/typescript/*`; `crates/fabric/*`; `protocol/fixtures/*`, `protocol/required.txt`, `protocol/facade/v1/*`; `docs/facade-protocol.md`, `docs/net.md`, ADRs 0026 and 0027 and this section's parts P06 and P07 |

A file not listed is touched by neither without a note in the commit saying why.

## Unexpected-loss recovery (M6)

Status: proposed with ADR 0028, 2026-11-02; implemented on the M6 branch on 2026-11-03 against the M4 base, with the parts that need the M5 executor and facade recorded under "As built (M6)"; merged with M5 (`m5/placement-continuity` at `7aa46d6`, then at `f09cc8d`) on 2026-11-08, recorded under "On the M5 base (M6)". Every selection in this section was selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02, never approved by Efe. This section is types and signatures; the code is the authority once it exists, and "As built (M6)" notes will say where it differs. It stacks on the M5 sections (ADR 0023 to 0027: task kinds and outcome policies, the checkpoint cut, source bindings, placement, the facade protocol) and names their types where it needs them.

Acceptance (roadmap M6): a real partition and an old-runner wake prove that covered writes cannot resume under obsolete authority; an independent acknowledged checkpoint; termination and fencing coverage stated; recovery policies per job.

### What M2 and M4 already provide

M6 adds only what is missing. These are in the M2 to M4 code and tests and do not change:

| Need | Existing mechanism | Where |
|---|---|---|
| One generation per grant, unique in the job | `Generation` on every `Attempt`; first grant is 1, each later grant the latest plus one; `UNIQUE (job_id, generation)` on `attempts` | `fabric-core` `managed::model`, `fabric-journal` `schema.rs` |
| The live grant | `ManagedState::live_attempt()`: the one attempt whose status `may_run()` (`granted`, `running`, `unknown`); `grant_may_dispatch` offers a grant for dispatch only while its attempt is `granted` at the job's current generation | `fabric-core` `managed::model` |
| A report at an older generation changes nothing | `apply_observation` records `StaleObservationRefused { attempt, generation, current }` and refuses with `DomainError::StaleGeneration` (replay rule 7) | `fabric-core` `managed::transition`, `fabric-core/tests/managed_rules.rs` |
| No new grant behind an attempt that may run | `StartAttempt`, `FinishTask` and `CancelJob` wait while any attempt is `granted`, `running` or `unknown` (`DomainError::AttemptStillLive`) | `fabric-core` `managed::transition` |
| A provider effect from a stale attempt is refused before any frame | `provider_begin` refuses an attempt that is not the job's current grant, `granted` or `running`, at the job's generation (M3), and the replacement barrier refuses a `StartAttempt` while an effect may still act (M4) | `fabric-journal` `provider.rs`, `only_the_current_live_attempt_dispatches`, `replacement_blockers` |
| One serialized authority writer | `fabricd::authority`: one OS thread over one `Journal`, fed by a bounded queue of 64 calls (`Submit`, `Observe`, `Mark`, `Job`, `List`); the answer is sent after the commit | `crates/fabricd/src/authority.rs` |
| Hash-chained history, append-only, replayed on open | `events` with `digest` and `prev_digest`; `verify` replays every job and compares the projection; a new event shape is a new `kind` or a new format byte | `fabric-journal` `lib.rs`, `project.rs` |
| A checkpoint the runner captured | `Observation::Checkpointed { attempt, generation, snapshot }` moves the attempt to `checkpointed`, writes `attempts.last_checkpoint` and an `evidence` row of kind `checkpoint` whose `ref` is the snapshot id | `fabric-journal` `project.rs` `write_evidence` |
| A verified stop on the authority's own host | `Journal::recover(at_ms, probe)` with `Liveness::{Gone, Running, CannotTell}`: `Gone` is `Observation::Lost`; `CannotTell` leaves the attempt as it is; nothing is granted (replay rules 2 to 4) | `fabric-journal` `lib.rs` |
| Bytes move by content, verified by the receiver | `fetch_snapshot` walks the bundle chain, fetches each manifest and its data chunks, keeps a chunk only when its bytes hash to the id, publishes each manifest after every object it names is on disk and checks the published id equals the asked id; `WantSnapshot` and `WantChunks` carry no fencing token because the id is the bytes | `crates/fabricd/src/transfer.rs`, ADR 0013, ADR 0014 |
| A snapshot is whole or absent on a node | `Store::snapshot(id)` answers only after `publish` wrote the row, which happens after every object was written or verified (`is_complete`) | `fabric-store` `store.rs`, `transfer.rs` |
| Authenticated peers, and a connection that notices silence | the peer book (ed25519 ids, TLS 1.3, no resumption); a side that wrote nothing for 10 s sends `KeepAlive` and a side that read no byte for 30 s ends the connection; an unpair ends live connections within 5 s; `Session::ping` | `fabric-net` `peers.rs`, `rpc.rs`, ADR 0013 |
| Fencing on the wire for standalone jobs | per-job requests carry `FencingToken { job_id, epoch }` and are answered inside `Fenced<T>` with `StaleEpoch` or `AheadOfUs`; an epoch is never written for a managed job | `fabric-net` `rpc.rs`, ADR 0019 |
| Crash points and two-process tests | `checkpoint::reached` in `fabric-net` and `fabric-store`, the e2e suite that runs daemons under `FABRIC_HOME` with separate state roots | `crates/fabric-net/src/checkpoint.rs`, `crates/fabric/tests/e2e` |

What is missing, and what M6 adds:

| Gap after M5 | M6 addition |
|---|---|
| A captured checkpoint says nothing about where its bytes are | `Observation::Captured`, the authority's fetch and verify, and `CheckpointAck` written as `CheckpointAcknowledged` |
| The generation fences the journal and the bridge only | the covered write set, `GrantToken` on each covered path and one `fence` rule run before any of them writes |
| A silent runner has no state and no recorded reason | heartbeats per live attempt, `ResourceUnavailable` and `ResourceReachable` as observations, the attempt `unknown` while unavailable |
| Nothing can grant while an attempt may still run, even when fencing makes it safe | `Command::Supersede` and `Command::Recover`, the status `superseded`, `GrantSuperseded` |
| No policy says what the authority may do by itself | `JobPolicy.recovery: RecoveryPolicy { Block, RestartFromAcknowledged }`, default `Block`, with its admission rule and its fallback |
| The executor never learns it lost its grant | the `Fenced` answer on every covered path, and the executor's stop of the attempt's group on `superseded` |
| No proof under a real partition | the relay, the two-daemon harness and the three scenarios of "Proof harness" |

### Checkpoint acknowledgement

A checkpoint has two states and the journal keeps them apart. Captured: the runner published the bundle chain into its own store and reported it; the bytes may exist only there. Acknowledged: the authority node fetched the chain through the existing store transfer, verified it, and committed what it verified. Only an acknowledged checkpoint is a restart point.

```rust
// fabric-core, managed::model
/// The digest of a checkpoint cut (ADR 0024): the receipts, facts and
/// evidence refs the cut names, canonically encoded and hashed with blake3.
pub struct CutDigest([u8; 32]);

pub enum Observation {
    // M2 kinds, and:
    /// The attempt captured a checkpoint and keeps running. `Checkpointed`
    /// stays the report of an attempt that ended at one.
    Captured { attempt: AttemptId, generation: Generation, snapshot: SnapshotId, cut: CutDigest },
}

/// What the authority verified before it accepted a checkpoint as the
/// job's. Written to the journal as `CheckpointAcknowledged`, never to the
/// store: the store's row says the bytes are complete on this node, the
/// event says the authority accepted them under the generation that made them.
pub struct CheckpointAck {
    pub attempt: AttemptId,
    pub generation: Generation,
    pub snapshot: SnapshotId,
    /// Equal to the `cut` the runner reported in `Captured`.
    pub cut: CutDigest,
    /// How many manifests `fetch_snapshot` published and how many chunks it
    /// kept, each after its bytes hashed to the id asked for (ADR 0014).
    pub manifests: u32,
    pub chunks: u64,
    pub bytes: u64,
    /// The node the bytes were fetched from: the attempt's resource.
    pub fetched_from: NodeId,
}

pub enum ManagedEvent {
    // M2 to M4 kinds, and:
    CheckpointAcknowledged { ack: CheckpointAck },
}

pub enum DomainError {
    // M2 to M4 kinds, and:
    /// `Captured` or an acknowledgement names a snapshot the attempt did not report.
    UnknownCheckpoint { attempt: AttemptId, snapshot: SnapshotId },
    /// The acknowledgement's cut digest is not the one the runner reported.
    CutMismatch { snapshot: SnapshotId, reported: CutDigest, verified: CutDigest },
    /// A `Recover` names a snapshot no `CheckpointAcknowledged` covers.
    NotAcknowledged { snapshot: SnapshotId },
}

impl ManagedState {
    /// Every acknowledged checkpoint of the job, newest first, each with
    /// the attempt and generation that made it.
    pub fn acknowledged(&self) -> Vec<&CheckpointAck>;
    /// Checkpoints reported `Captured` and never acknowledged; listed, and
    /// a restart point for nothing.
    pub fn captured_only(&self) -> Vec<(AttemptId, Generation, SnapshotId)>;
}
```

- `apply_observation(Captured)` runs the M2 generation check first (a stale report is `StaleObservationRefused`, replay rule 7), then records `AttemptObserved` with the status unchanged and an `evidence` row of kind `checkpoint` whose `ref` is the snapshot id and whose `digest` is the cut. The attempt stays `running`.
- The acknowledgement is an authority-side rule, `acknowledge(state, ack) -> Result<Transition, DomainError>`, the fourth rule function beside `accept_command`, `apply_observation` and `mark_operation`. It refuses a snapshot the attempt never reported (`UnknownCheckpoint`), a cut that differs from the reported one (`CutMismatch`) and a generation that is not the attempt's (`StaleGeneration`). It accepts an acknowledgement for an attempt in any status while the attempt's generation is the job's: the bytes of a checkpoint an attempt made before it became `unknown`, `superseded` with nothing granted, or `lost` are still its checkpoint. Once a later generation is granted it is `StaleGeneration` (a repeat of one already held is still unchanged), so a verifier that finishes after a `Recover` never makes the superseded attempt's bundle a restart point beside the new lineage (`managed_rules::an_acknowledgement_that_lands_after_a_regrant_is_stale`). It writes `CheckpointAcknowledged` and an `evidence` row of kind `checkpoint_ack` (`ref` the snapshot id, `digest` the cut), and sets the projection column `attempts.last_acknowledged`.

```rust
// fabric-journal
impl Journal {
    /// Commits `CheckpointAcknowledged` for `ack` in its own transaction
    /// after the core rule accepted it; the evidence row and the projection
    /// column are written in the same transaction (`write_transition`).
    pub fn acknowledge(&mut self, job: JobId, ack: &CheckpointAck, at_ms: u64) -> Result<Transition, CommitError>;
    /// The job's acknowledged checkpoints, newest first, from the projection.
    pub fn acknowledged(&self, job: JobId) -> Result<Vec<CheckpointAck>, CommitError>;
}

// fabricd, authority: one more call on the writer's queue
pub enum Call {
    // M2 to M4 calls, and:
    Acknowledge(JobId, CheckpointAck, Reply<Transition>),
}

// fabricd, recovery (new module): the fetch runs on a tokio task, never on
// the writer thread; only the commit goes through `Call::Acknowledge`.
pub struct Verifier { /* the node's store and its peer sessions */ }
impl Verifier {
    /// For a `Captured` the authority committed: opens a session to the
    /// attempt's node, runs `transfer::fetch_snapshot` for the chain, reads
    /// the cut from the published manifest, recomputes its digest, and
    /// returns the ack to commit, or why it cannot.
    pub async fn verify(&self, job: JobId, attempt: &Attempt, snapshot: SnapshotId, reported: CutDigest) -> Result<CheckpointAck, VerifyError>;
}
pub enum VerifyError {
    /// The attempt's node did not answer within the bound; retried on the
    /// next heartbeat, never recorded as a refusal.
    Unreachable { node: NodeId },
    /// A chunk or manifest did not hash to its id, or the published id is
    /// not the asked id (`fetch_snapshot`'s own refusals); recorded as
    /// `CoveredWriteRefused` on the bundle publish path.
    Corrupt { snapshot: SnapshotId, reason: String },
    CutMismatch { reported: CutDigest, verified: CutDigest },
    Store(StoreError),
}
```

The verifier is driven by the authority's event stream: every committed `Captured` queues one verification; a verification that fails `Unreachable` is retried when the attempt's next heartbeat arrives. As built (2026-11-09): a capture is fetched only while the domain may acknowledge it (`recovery::due`: the job's generation, an attempt not superseded), and one whose fetches failed `MAX_VERIFY_FAILURES` (3) times for another reason is not fetched again by this node; the transfer cannot tell a corrupt chunk from a link that dropped mid-fetch, so the bound replaces a fetch on every heartbeat. The M5 facade `checkpoint` command and the harness drain of ADR 0023 are the two sources of `Captured` in M6; a timer that takes checkpoints is a later gate (ADR 0028, open questions).

### Covered writes and the fence

```rust
// fabric-core, managed::fence (new module, pure)
/// What a runner presents with every covered write for a managed job: the
/// grant it acts under. The managed counterpart of `FencingToken`; an
/// epoch is never written for a managed job (ADR 0019).
pub struct GrantToken { pub job: JobId, pub attempt: AttemptId, pub generation: Generation }

/// The covered write set. Exactly these four; a test lists them and fails
/// when a variant is added without a path.
pub enum CoveredPath {
    /// An `Append` to a managed job's ledger (fabric-net, per-job request).
    LedgerAppend,
    /// A bundle publish the authority accepts as the job's checkpoint
    /// (the `Captured` report and the verifier's fetch).
    BundlePublish,
    /// A journal operation an executor reports: every observation,
    /// acknowledgement and operation mark.
    JournalOperation,
    /// The outcome of a facade command (ADR 0027).
    FacadeOutcome,
}

pub enum FenceVerdict {
    /// The token names the job's live attempt at the job's generation, in
    /// status `granted` or `running`, from the node it was granted to.
    Live,
    /// The token's generation is older than the job's.
    Stale { current: Generation },
    /// The token's attempt was superseded by a later grant.
    Superseded { by: Generation },
    /// The attempt is the job's but may not write: `unknown`, or ended.
    NotWritable { status: AttemptStatus },
    /// The write came over the node link from a node other than the
    /// attempt's resource.
    WrongNode { granted: NodeId, from: NodeId },
    UnknownAttempt,
    UnknownJob,
}

impl FenceVerdict {
    /// `Live` alone.
    pub fn allows(&self) -> bool;
    /// `Stale` and `Superseded`: the executor stops the attempt's group.
    pub fn is_obsolete(&self) -> bool;
}

/// The one rule every covered path runs before it writes anything. `from`
/// is the authenticated peer the write arrived from, `None` for a write
/// made on the authority's own host.
pub fn fence(state: Option<&ManagedState>, token: &GrantToken, from: Option<NodeId>) -> FenceVerdict;

pub enum ManagedEvent {
    // and:
    /// A covered write other than a report was refused; a refused report
    /// stays `StaleObservationRefused`.
    CoveredWriteRefused { path: CoveredPath, attempt: AttemptId, generation: Generation, verdict: FenceVerdict },
}
```

The pure rule answers `NotWritable`, not `Live`, for an `unknown` attempt. At the authority a heartbeat token or a covered write from the node an `unknown` attempt was granted to, at the job's generation, is presence: the authority observes `ResourceReachable` first and then runs the fence, so the write meets a `running` attempt and a report that arrives before the healing heartbeat is kept (next section). From any other node the write is refused `NotWritable` with the status and recorded, and the executor retries after its next heartbeat answer; nothing is stopped by a `NotWritable` verdict.

```rust
// fabric-journal
impl Journal {
    /// Runs `fence` against the job's projection in the write's own
    /// transaction. `Live` returns `Ok(())` and the caller writes, inside
    /// the authority's turn (`Authority::fenced_write` runs the write on
    /// the authority's thread right after this, before the next call, so
    /// no `Recover` commits between the check and the write); any other
    /// verdict commits `CoveredWriteRefused` (or, for a report,
    /// `StaleObservationRefused`) and returns the verdict, once per
    /// attempt, path and verdict while the journal is open; a token that
    /// names no attempt of the job is refused without a record. Nothing
    /// else changes.
    pub fn fence(&mut self, token: &GrantToken, path: CoveredPath, from: Option<NodeId>, at_ms: u64) -> Result<Result<(), FenceVerdict>, CommitError>;
    /// `observe` and `mark` gain the token and the peer and run `fence`
    /// first; the M2 generation check inside `apply_observation` stays as
    /// the second check and is what a replay re-runs.
    pub fn observe(&mut self, token: &GrantToken, from: Option<NodeId>, observation: &Observation, at_ms: u64) -> Result<Result<Transition, FenceVerdict>, CommitError>;
    pub fn mark(&mut self, token: &GrantToken, from: Option<NodeId>, operation: OperationId, mark: OperationMark, at_ms: u64) -> Result<Result<Transition, FenceVerdict>, CommitError>;
    /// The authority's own observations (`ResourceUnavailable`,
    /// `ResourceReachable`, and `Lost` from `recover`): no runner presents a
    /// grant, so no fence runs; the M2 generation check still does.
    pub fn observe_authority(&mut self, job: JobId, observation: &Observation, at_ms: u64) -> Result<Transition, CommitError>;
}

// fabric-net, wire: a per-job request for a managed job carries the grant
// in place of the epoch, and its answer carries the verdict.
pub enum JobToken { Standalone(FencingToken), Managed(GrantToken) }
pub enum Fenced<T> {
    // ADR 0003 kinds, for a standalone epoch, and:
    /// The receiver's authority refused the grant. Never answered to a
    /// standalone request, as `StaleEpoch` and `AheadOfUs` are never
    /// answered to a managed one.
    GrantRefused { verdict: FenceVerdict },
}

// fabricd, executor: what a runner does with a verdict.
impl Executor {
    /// `Stale` or `Superseded`: stops the attempt's process group (SIGTERM,
    /// then SIGKILL at the stop bound), records the verdict beside the
    /// attempt's local marker, and reports nothing more for it. `NotWritable`
    /// or `WrongNode`: holds the write and retries after the next heartbeat
    /// answer. Never a journal write: the authority already recorded it.
    pub async fn on_verdict(&self, attempt: AttemptId, verdict: &FenceVerdict) -> Result<(), ExecutorError>;
}
```

Bundle publish is covered at the authority, not at the runner: a runner publishes into its own store freely (that is local capture), and the write that counts is the authority's acceptance, which runs `fence` on the `Captured` report before the verifier fetches anything, and in `acknowledge` refuses an acknowledgement whose generation is no longer the job's, the fence's `Stale` rule applied to the grant that captured it (the attempt itself may be `unknown` by then, which is when its checkpoint is needed). The harness's own files inside the workspace (the working tree, git refs, the harness state directory) are not covered, and nothing in Fabric prevents a superseded runner that is alive from editing them until its executor stops it; those bytes reach the job only through a bundle publish.

### Loss as an availability observation

```rust
// fabric-core, managed::model
pub const HEARTBEAT_INTERVAL_MS: u64 = 2_000;
pub const UNAVAILABLE_AFTER_MS: u64 = 10_000;

pub enum Observation {
    // and, made by the authority itself, never by a reporter:
    /// No heartbeat for the attempt in `UNAVAILABLE_AFTER_MS`. Moves
    /// `granted` or `running` to `unknown`; says nothing about whether the
    /// attempt stopped.
    ResourceUnavailable { attempt: AttemptId, generation: Generation, last_heartbeat_ms: Option<u64> },
    /// A heartbeat returned for an attempt `unknown` by `ResourceUnavailable`
    /// and nothing else happened to it. Moves it back to `running`, or to
    /// `granted` if its grant never succeeded (it was never seen launched).
    ResourceReachable { attempt: AttemptId, generation: Generation },
}

pub enum ManagedEvent {
    // and:
    AttemptUnavailable { attempt: AttemptId, generation: Generation, last_heartbeat_ms: Option<u64> },
    AttemptReachable { attempt: AttemptId, generation: Generation },
}

// fabric-net, wire: the launch path's one heartbeat, `managed::launch::
// {Heartbeat, HeartbeatAnswer}`, every HEARTBEAT_INTERVAL_MS per executor
// node, not per attempt, answered with the fence's verdicts. Not a
// per-job request: no `JobToken`.

// fabricd, authority: the last heartbeat per live attempt, in memory.
pub struct Presence { /* attempt -> (generation, last_seen_ms, node) */ }
impl Presence {
    /// Records the heartbeat and returns the attempts whose status should
    /// change: `unknown` ones now reachable. Writes nothing.
    pub fn seen(&mut self, from: NodeId, beat: &Heartbeat, now_ms: u64) -> Vec<AttemptId>;
    /// The live attempts not heard from for `UNAVAILABLE_AFTER_MS`; the
    /// caller observes each `ResourceUnavailable`. Run every
    /// `HEARTBEAT_INTERVAL_MS` on the authority's clock.
    pub fn overdue(&self, now_ms: u64) -> Vec<(JobId, AttemptId, Generation, Option<u64>)>;
}
```

- A heartbeat is not a journal write. The authority answers it with the verdict of each token (`fence` with `from` the peer), so a reachable runner learns within one interval that an attempt of its is superseded, and stops it. A heartbeat from a node for an attempt granted to another node is answered `WrongNode` and otherwise ignored.
- `unknown` still `may_run`, so the M2 wait rule is unchanged: `StartAttempt`, `FinishTask` and `CancelJob` wait behind it (`AttemptStillLive`). What the status enables is `Recover` and `Supersede` below.
- `AttemptUnavailable` carries the last heartbeat time the authority holds, `None` for an attempt `granted` and never heard from. The projection column `attempts.unavailable_since_ms` is set by `AttemptUnavailable` and cleared by `AttemptReachable`.
- On authority restart `Presence` is rebuilt from the journal: every `granted` or `running` attempt of a remote node becomes overdue `UNAVAILABLE_AFTER_MS` after the restart unless a heartbeat arrives first, and an attempt `unknown` for want of heartbeats is tracked as unavailable from the restart, so its heartbeat or covered write brings it back and its recovery policy still runs. Silence is measured on the authority's monotonic clock, never on the wall clock, so a clock step or a host sleep names nothing overdue. `Journal::recover` is unchanged and still probes only the authority's own host; an attempt on the authority's own node never heartbeats and never becomes `unknown` this way (replay rule 4 covers it).
- `Observation::Lost` stays a verified stop and is never produced from a missed heartbeat.

### Supersede and regrant

```rust
// fabric-core, managed::model
pub enum AttemptStatus {
    // and:
    /// Its grant was taken away by a later grant while it may have been
    /// alive. Does not `may_run`; the task stays open.
    Superseded { by: Generation },
}

pub enum Command {
    // and:
    /// Takes the grant away from an `unknown` attempt and grants nothing.
    Supersede { attempt: AttemptId, reason: String },
    /// `Supersede` and `StartAttempt` in one transaction: the next attempt
    /// at the next generation, placed on `resource` by the M5 rules, from
    /// the acknowledged checkpoint `from`.
    // Takes the grant now; the failover is placed by the M5 placement
    // (EffectRequest::Recover { task, from, lost }), never on `lost`.
    Recover { attempt: AttemptId, from: SnapshotId, reason: String },
}

pub enum ManagedEvent {
    // and:
    GrantSuperseded { attempt: AttemptId, generation: Generation, by: Option<Generation>, reason: String },
}

pub enum DomainError {
    // and:
    /// `Supersede` or `Recover` on an attempt that is not `unknown`: a
    /// reachable attempt is handed over by the M5 planned handoff, never recovered.
    AttemptReachable { attempt: AttemptId, status: AttemptStatus },
}
```

- `Supersede` is accepted only for the job's live attempt in status `unknown` (`AttemptReachable` otherwise, `UnknownAttempt` for any other attempt). It records `GrantSuperseded { by: None }`; the job's generation does not move; the task stays `open`; the job now has no live attempt, so a later `StartAttempt` is admitted by the M2 rules. In the same batch it settles the old attempt's open effects (accepted, dispatched or `unknown`): a `Checkpoint` of it and a `Continue` of its task are marked `failed`, since nothing the old attempt reports can settle them any more, and a cancel of the job or of its task is done (`JobCancelled` or `TaskFinished { cancelled }`, the operation `succeeded`), since the superseded attempt can write nothing more. An operation never dispatched is marked `dispatched` first, as an explicit grant settles a create (`managed_rules::a_recover_fails_the_old_attempt_s_checkpoint_so_the_new_one_may_take_one`, `managed_rules::an_open_cancel_refuses_a_recover_and_a_supersede_completes_it`).
- `Recover` runs the `Supersede` checks, then `NotAcknowledged` unless `from` is in `acknowledged()`, then the M4 replacement barrier exactly as `StartAttempt` does (`ReplacementBlocked` with the blockers; a provider effect of the old attempt that may still act blocks it), then `EffectInFlight` while a cancel of the job or of the old attempt's task is open: no work is granted again on a job or task someone asked to stop, and the operator supersedes instead. It records `GrantSuperseded { by: Some(next) }`, the failed checkpoint or continue `Supersede` would fail, and `AttemptGranted` at `next` with `initial_checkpoint: Some(from)` in one batch at one revision. `UNIQUE (job_id, generation)` holds as before.
- Operation kinds `supersede` and `recover` join `operations.kind`. Both take the operation key and expected revision like every command; a repeat with the same key replays, the same key with another request conflicts.
- Nothing resumes under an old generation: a superseded attempt's every covered write is refused `Superseded { by }` and recorded, and `GrantSuperseded` is never undone. An attempt that was `superseded` and later reports `Exited` has its report refused and recorded the same way; its local exit is in the executor's marker, not in the journal.

### Recovery policy

```rust
// fabric-core, managed::model
pub struct JobPolicy {
    pub restart: RestartPolicy,
    // M5 fields (ADR 0023, ADR 0024), and:
    pub recovery: RecoveryPolicy,
}
/// What the authority may do by itself when an attempt is unavailable.
pub enum RecoveryPolicy {
    /// Record it and wait for an operator's `Recover` or `Supersede`. The default.
    Block,
    /// Issue `Recover` itself once the attempt has been `unknown` for
    /// `UNAVAILABLE_AFTER_MS`, when every admission condition holds.
    RestartFromAcknowledged,
}
impl Default for RecoveryPolicy { fn default() -> Self { RecoveryPolicy::Block } }

/// Why an automatic restart did not happen; the job stays under `Block`
/// with this recorded, never under a weaker restart.
pub enum RecoveryBlock {
    /// The task's outcome policy (ADR 0023) is `block`: an `opaque` task.
    OutcomeBlocks { kind: String },
    /// Nothing acknowledged to restart from: for a `checkpoint` task, no
    /// acknowledged checkpoint of this attempt; for a `cancel_and_restart`
    /// task, none of the job. An operator may still `Supersede` and
    /// `StartAttempt` from the source binding by hand.
    NoAcknowledgedCheckpoint,
    ReplacementBlocked { blockers: Vec<Blocker> },
    /// A cancel of the job or of the attempt's task is open; judged again
    /// on the next tick (as built, 2026-11-09).
    EffectInFlight { operation: OperationId },
    /// A refusal of the authority's `Recover` the decision did not
    /// foresee, stored under the recover's derived key (as built).
    Refused { refusal: String },
    // A failover the M5 placement finds no node for parks with every
    // exclusion recorded in its placement decision, as a create does.
}

pub enum ManagedEvent {
    // and:
    RecoveryBlocked { attempt: AttemptId, generation: Generation, reason: RecoveryBlock },
}

pub enum DomainError {
    // and:
    /// `CreateJob` with `RestartFromAcknowledged` for a primary task whose
    /// outcome policy is `block` (an `opaque` task). Refused at admission.
    RecoveryUnsupported { kind: String },
}

/// The admission rule, pure: given the job, the unavailable attempt, its
/// task's outcome policy and what the authority found, either the
/// `Recover` to submit or why not.
pub fn recovery_decision(state: &ManagedState, attempt: AttemptId, outcome: OutcomePolicy, blockers: &[Blocker], placement: Result<ResourceRef, Vec<Exclusion>>) -> Result<Command, RecoveryBlock>;
```

- `OutcomePolicy` is the M5 type of ADR 0023 (`cancel_and_restart` for build and test, `checkpoint` for harness, `block` for opaque); `Exclusion` is the M5 placement exclusion of ADR 0026. The M6 PR takes their names as the M5 code lands them at the restack.
- `recovery_decision` restarts a `cancel_and_restart` task from the newest acknowledged checkpoint of the job (the task's own work is interrupted and recorded so, never passed or failed, as ADR 0023 says), and a `checkpoint` task only from the newest acknowledged checkpoint of the unavailable attempt; with none, `NoAcknowledgedCheckpoint`. A `block` task is never restarted automatically. Under `Block` the decision never runs and nothing but `AttemptUnavailable` is recorded.
- The authority runs the decision for an attempt whose job's policy is `RestartFromAcknowledged` once the attempt has been `unknown` for `UNAVAILABLE_AFTER_MS` (the dwell: a silence of the threshold made it `unknown`, and as long again passed without a heartbeat), and on every tick after that while it stays unavailable, so a new acknowledgement or a settled blocker is seen; the same `RecoveryBlock` reason is recorded once per silence. A `Recover` it submits carries an operation key derived from `(job, attempt, generation)`, so a restart after an authority crash replays the same operation and never grants twice.
- `MANAGED_FORMAT` becomes `2` for every managed frame M6 writes. Format `1` is read beside it ("Versioning and migration"): a format `1` `JobPolicy`, inside a `JobCreated` event or the `jobs.policy` blob, decodes through its M5 shape and maps to `recovery: Block`. A stored event's bytes and digest never change, so the chain of an M2 to M5 job verifies as before. A format `2` frame read by an M5 binary is `FrameError::UnknownFormat(2)`, and that binary lists the job with its policy unreadable and runs nothing of it.

### Journal schema, version 2 amended for M6

Version 2 has not shipped (ADR 0020, ADR 0022), so M6 amends it in place under the rule Efe set on 2026-10-29. The DDL differences from the M4 text:

```sql
-- attempts: one status, two projection columns
status             TEXT NOT NULL CHECK (status IN ('granted','running','succeeded','failed','cancelled',
                     'checkpointed','lost','unknown','superseded')),
last_acknowledged  BLOB CHECK (last_acknowledged IS NULL OR length(last_acknowledged) = 32),
unavailable_since_ms INTEGER,

-- evidence: one kind
kind        TEXT NOT NULL CHECK (kind IN ('checkpoint','exit','checkpoint_ack')),

-- operations: two command kinds
kind              TEXT NOT NULL CHECK (kind IN ('create_job','start_attempt','register_task','finish_task',
                     'cancel_job','supersede','recover')),
```

- The new event kinds (`checkpoint_acknowledged`, `covered_write_refused`, `attempt_unavailable`, `attempt_reachable`, `grant_superseded`, `recovery_blocked`) are new `kind` strings in `events`; no DDL.
- `crates/fabric-journal/tests/fixtures/v2-pre-m6.sqlite` is the M4 `v2.sqlite`, renamed and kept. `Journal::open` refuses it with `OpenError::DraftSchema { version: 2, amendment: "m6" }`, reading the `attempts` CHECK text from `sqlite_master`, and writes nothing; `tests/fixtures.rs` asserts the refusal and that the file's bytes are unchanged after it. `DraftSchema` gains the `amendment` field; the M3 draft is `amendment: "m4"`.
- `v2.sqlite` is regenerated with the amended tables, with one job that holds a `captured` and an `acknowledged` checkpoint, a `superseded` attempt and a `recovered` one, so `verify` replays every new kind.
- `verify` replays the new events through `apply_batch` and compares `last_acknowledged`, `unavailable_since_ms` and the `superseded` status like every other projection column.

### Proof harness

This subsection is the plan written on the M4 base, kept for the record. The harness as built differs, and what it asserts is in "As built on the merged tree (M6)" below and in docs/evidence/m6: the runner's harness is the fake Codex held with `FAKE_CODEX_HOLD` (no ledger writer, no `SIGUSR1` capture); the checkpoint is cut by `fabric managed checkpoint`; `Partition::start(env)` takes no policy, the job's `--recovery` does; the five scenarios assert the events, the failover placement (the runner excluded, the authority chosen, generation 2), the refused and recorded old writes, the stopped group and one live attempt. They do not assert the failover's starting snapshot (the client inspection does not carry `initial_checkpoint`; `dispatch::tests::a_recover_is_placed_as_a_failover_and_launched_like_any_grant` does), nor the checkpoint and ledger heads, nor the `AttemptReachable` and `ReplacementBlocked` refusals, which `managed_rules::supersede_needs_an_unknown_attempt`, `managed_rules::recover_supersedes_then_its_failover_grants_the_next_generation_elsewhere` and `managed_rules::recovery_decision_falls_back_to_block_with_the_failed_condition` cover in process.

The acceptance of M6 is a real partition and an old-runner wake, run locally with what this repository has: two `fabricd` processes on one host with separate state roots (the e2e `Env` of `crates/fabric/tests/e2e/world.rs`, one `FABRIC_HOME` per node), paired through the existing peer book, and a userspace relay the test owns. The evidence note (`docs/evidence/m6/`) states plainly that this is a two-process partition on one host, not two hosts, and that no firewall or system setting was touched.

```rust
// crates/fabric/tests/e2e/relay.rs (test code only; no new dependency)
/// Forwards UDP datagrams (QUIC) and TCP bytes (the TLS fallback) between
/// a loopback port it owns and `upstream`. Paused, it drops every datagram
/// in both directions and stops reading the TCP sockets, so the bytes
/// wait in the kernel's buffers and the sender is held by flow control;
/// the sockets stay open. Resumed, it forwards again and the held bytes
/// arrive first, in order, so a TLS record stream resumes where it
/// stopped. The runner node's peer book names the relay's address as the
/// authority's; on this base every datagram between the two crosses the
/// relay (the authority's own dial of the runner, for the verifier's
/// fetch, comes with the M5 executor).
pub struct Relay { pub addr: SocketAddr }
pub struct RelayCounters { pub udp_forwarded: [u64; 2], pub udp_dropped: [u64; 2], pub tcp_forwarded: [u64; 2], pub tcp_held: [u64; 2] }
impl Relay {
    pub fn start(upstream: SocketAddr) -> Fallible<Relay>;
    pub fn pause(&self);
    pub fn resume(&self);
    /// Datagrams and bytes forwarded and dropped per direction, for the evidence.
    pub fn counters(&self) -> RelayCounters;
    pub fn stop(self);
}

// crates/fabric/tests/e2e/partition.rs
/// The two nodes, the relay and one managed job whose attempt runs on the
/// runner with a fake harness that writes a ledger record every 500 ms and
/// captures a checkpoint on `SIGUSR1`.
pub struct Partition { pub authority: Node, pub runner: Node, pub relay: Relay, pub job: JobId }
impl Partition {
    pub fn start(env: &Arc<Env>, policy: RecoveryPolicy) -> Fallible<Partition>;
    /// Waits for `Captured` and then `CheckpointAcknowledged` on the authority.
    pub fn acknowledged_checkpoint(&self) -> Fallible<SnapshotId>;
    /// The authority's event list for the job after `after`.
    pub fn events(&self, after: Option<EventSeq>) -> Fallible<Vec<StoredEvent>>;
    /// The runner's process group of the attempt, for the stop assertions.
    pub fn runner_group(&self) -> Fallible<ProcessGroup>;
}
```

Three scenarios, each under `Block` (an operator `Recover` through the CLI) and under `RestartFromAcknowledged` (the authority's own `Recover`):

| Scenario | Steps | Evidence asserted |
|---|---|---|
| `relay_paused_runner_alive` | acknowledged checkpoint; `relay.pause()`; wait past `UNAVAILABLE_AFTER_MS`; `Recover` (operator or automatic); `relay.resume()` | `AttemptUnavailable`, `GrantSuperseded`, `AttemptGranted` at the next generation on the authority; after resume, the old runner's ledger appends and its `Exited` answered `Superseded` and recorded `CoveredWriteRefused` or `StaleObservationRefused`; the authority's checkpoint head and the job's ledger head unchanged by anything the old runner sent after the supersede; the old runner's group gone within the stop bound; the new attempt starts from the acknowledged snapshot |
| `old_runner_sigstop_then_sigcont` | acknowledged checkpoint; `SIGSTOP` to the runner's `fabricd` and the attempt's group; wait; `Recover`; `SIGCONT` to both | the same event list; the woken runner's first heartbeat answered `Superseded` and the group stopped; every write it had buffered refused; no second `AttemptGranted` at the same generation (the `UNIQUE` holds) |
| `old_runner_sigkill_then_restart` | acknowledged checkpoint; `SIGKILL` to the runner's `fabricd` (its attempt's group survives as orphans); wait; `Recover`; start the runner's `fabricd` again in the same state root | the restarted runner reads its local marker, asks the authority and gets `Superseded`, stops the orphan group, and reports nothing; the orphan group's ledger records written while no `fabricd` ran are not in the job's ledger on the authority |

Each scenario also asserts the negative: with no acknowledged checkpoint (`captured_only` not empty, `acknowledged` empty) a `Recover` is refused `NotAcknowledged` and an automatic one records `RecoveryBlocked { NoAcknowledgedCheckpoint }`; with the runner reachable, `Recover` is refused `AttemptReachable`; with a provider destroy of the old attempt `unknown`, `Recover` is refused `ReplacementBlocked`. The relay's counters go into the evidence note so a reader can see bytes were dropped and later forwarded.

### M6 acceptance mapped to tests

| Acceptance | Tests |
|---|---|
| Independent acknowledged checkpoint | `fabric-core` `managed_lifecycle::a_checkpoint_leaves_the_attempt_running_and_a_blocked_one_fails_the_command` (the cut is captured under its own digest), `managed_rules::a_captured_checkpoint_restarts_nothing_until_acknowledged`, `managed_rules::an_acknowledgement_that_lands_after_a_regrant_is_stale`; `fabricd` `recovery::tests::a_corrupt_chunk_or_a_wrong_head_is_never_acknowledged`; e2e every scenario acknowledges a cut fetched over the relay, and scenario (5) holds one captured |
| Fencing coverage stated and enforced | `fabric-core` `fence::tests::the_covered_write_set_is_exactly_four`, `managed_rules::fence_only_the_live_attempt_from_its_node_is_live`; `fabric-net` `wire::tests::a_managed_request_carries_a_grant_and_never_an_epoch` (one heartbeat type, one verdict type); `fabricd` `report::tests::a_heartbeat_names_each_grant_by_the_fence`, `report::tests::a_report_at_an_older_generation_is_refused_and_recorded`; e2e scenarios (1) and (2): the old runner's report and ledger append refused and recorded |
| The runner stops on every verdict that ends its grant | `fabricd` `runner::tests::every_heartbeat_verdict_that_ends_the_grant_stops_the_attempt`, `runner::tests::every_report_refusal_that_ends_the_grant_stops_the_attempt`, `runner::tests::an_unknown_heartbeat_verdict_keeps_the_attempt`; e2e scenarios (1), (2) and (4): the runner's group is gone after the resume or the wake |
| A restarted runner resumes nothing | `fabricd` `runner::tests::a_restarted_runner_presents_its_taken_grants_and_resumes_none`; e2e scenario (3) |
| Loss is an availability observation | `fabric-core` `managed_rules::unavailable_is_unknown_and_still_may_run`; `fabricd` `authority::tests::presence_marks_overdue_on_the_authority_clock_and_writes_no_heartbeat`, `authority::tests::an_attempt_unknown_at_a_restart_comes_back_on_its_heartbeat_and_still_recovers` |
| Recovery policies per job | `fabric-core` `managed_rules::recovery_decision_falls_back_to_block_with_the_failed_condition`, `managed_rules::an_attempt_is_judged_by_its_own_task_not_the_primary`; `fabricd` `authority::tests::restart_from_acknowledged_recovers_onto_this_node_and_fences_the_old_runner`; e2e scenarios (1) to (3) under `restart_from_acknowledged`, (4) under `block`, (5) refused for want of an acknowledgement |
| A recover is a failover | `fabric-core` `managed_place::a_failover_excludes_the_unavailable_node_whatever_it_offers`, `managed_rules::recover_supersedes_then_its_failover_grants_the_next_generation_elsewhere`; `fabricd` `dispatch::tests::a_recover_is_placed_as_a_failover_and_launched_like_any_grant`, `authority::tests::a_failover_onto_the_unavailable_node_is_refused` |
| Operator commands | `fabric` `cli_contract` (examples and help of `managed supersede` and `managed recover`), `fabric-client` `contract` (request, reply and welcome fixtures); e2e scenario (4) `managed recover`, scenario (5) `managed supersede` and the refused recover |

### As built on the merged tree (M6)

M6 merged `main` with the M5 launch path on 2026-11-09 (`db7f63f`). The code is the authority on signatures; what the merged tree does, in the order a loss is handled:

- **One heartbeat, one verdict.** The launch path's `Heartbeat` is the only heartbeat on the wire; `HeartbeatAnswer` carries `FenceVerdict`, and `GrantVerdict` is gone. `HEARTBEAT_MS` is `HEARTBEAT_INTERVAL_MS`. `ReportRefusal` gained `Superseded { by }` and `NotWritable { status }`, so a refused report names the verdict the fence gave. `PROTOCOL_VERSION` is 5 (`ManagedJob`, `GrantRefused`, the fenced heartbeat answer); the launch path's messages were never released under 4.
- **Every report and heartbeat through the fence and presence.** `Node::accept_report` goes through `Authority::report` and `Node::accept_heartbeat` through `Authority::heartbeat`: presence first (a report or heartbeat from the node an `unknown` attempt was granted to brings it back), then the fence in the write's own transaction. The authority's own observations and marks (`observe_authority`, `mark_authority`) are the dispatch's: a placement, a launch the node refused, a stop the node had nothing to run for.
- **The runner obeys.** A heartbeat verdict or a report refusal that ends the grant stops every group of the attempt and nothing more is reported; `unknown` waits. The counter is `stale_obeyed`.
- **The cut is the checkpoint.** A recorded `Checkpointed` also records `CheckpointCaptured` under `CheckpointCut::digest`, in the same transaction; that digest is the job's checkpoint (selected by the implementation agent under Efe's delegation of 2026-11-02). The node's `Verifier` starts when a cut or capture report is recorded and again on every live heartbeat while the capture is unacknowledged: it fetches the snapshot's chain from the runner (`fetch_snapshot`), checks the head manifest read back hashes to the snapshot id, and commits `CheckpointAcknowledged` under the captured digest; a capture on the authority's own node is counted in its own store. `before_acknowledge` is the fault point that holds it. The head-manifest digest `cut_of` is gone.
- **A recover is a failover.** `Recover { attempt, from, reason }` takes the grant at once (`GrantSuperseded { by: None }`) and asks for `EffectRequest::Recover { task, from, lost }`. The dispatch places it with the M5 placement as `PlacementKind::Failover`, the lost node named as the current placement, not healthy: the pure placement excludes it (`Exclusion::Unavailable`) whatever it answers, and the domain refuses a failover placed on it (`FailoverToLostNode`). The grant starts from `from`, held by the authority (`snapshot_at` is the authority), names the superseding generation on the old attempt, and is launched like a create's or a continue's grant. A failover with no eligible node parks with every exclusion recorded; `RecoveryBlock::NoEligibleNode`, `RecoveryExclusion` and the executor flag are gone. An acknowledgement of the old attempt that lands while its recover is still being placed is refused `NotLive`.
- **A restarted runner resumes nothing.** The launch record is the grant's local marker (`runs/taken/<attempt>.json`): the token, the authority that granted it, and how this node's part ended (`Finished`, `GrantEnded { verdict }`, `Abandoned { verdict }`). At start the executor stops what the previous run left, then the runner presents every grant whose marker has not ended in a heartbeat with `running: false` until the authority answers, and writes the verdict into the marker. The presentation is never presence: an `unknown` attempt stays `unknown` and a grant still live at the authority is left to fall silent, so its loss handling decides it (`authority::tests::a_restarted_runner_s_presentation_is_answered_and_never_presence`).
- **A continue after a supersede.** A continue placed once no attempt of its task runs starts where `continue_start` says: the task's last cut when the attempt that cut it ended by its runner's word (the M5 handoff), else, when that attempt was superseded, the newest acknowledged checkpoint of the task, else it is refused `NotAcknowledged`. After a supersede it is placed as `PlacementKind::Failover` with the superseded attempt's node excluded and refused there by the domain (`FailoverToLostNode`), and a grant from an acknowledged checkpoint fetches it from the authority (`managed_rules::a_continue_after_a_supersede_starts_from_an_acknowledged_checkpoint_never_on_the_lost_node`, `dispatch::tests::a_continue_after_a_supersede_is_placed_as_a_failover_from_the_acknowledged_checkpoint`).
- **Every automatic restart that does not happen says why.** An open cancel is `RecoveryBlock::EffectInFlight`, recorded before anything is submitted and judged again on the next tick; any other refusal of the authority's `Recover` is recorded as `RecoveryBlock::Refused` (`authority::tests::an_open_cancel_blocks_the_automatic_restart_with_its_reason_until_it_ends`).
- **Policy and operator commands.** A job chooses its policy with `recovery` in `ManagedPolicy` (`--recovery` on `fabric managed create` and `fabric facade submit`; `block` when absent). `fabric managed supersede` and `fabric managed recover` (control ops `managed_supersede` and `managed_recover`, api 2, additive; the TS SDK's `managedSupersede` and `managedRecover`) name the job's live attempt unless one is given; recover starts from the newest acknowledged checkpoint unless one is given, under any policy. Facade protocol v1 does not carry them yet.
- **What the covered write set has behind it.** `JournalOperation`: every report a runner sends, the cut among them. `LedgerAppend`: the node link's managed append, fenced before its body is read; no node holds a managed ledger, so a live append is refused after the fence. `BundlePublish` is covered only as the fenced cut report that makes a bundle the job's checkpoint, and `FacadeOutcome` has no runner writer: the facade's outcomes are the authority's own commands. The claim of ADR 0028 is narrowed to these.
- **The proof.** `crates/fabric/tests/e2e/partition.rs`: the five scenarios on two daemons behind `fabric-relay`, run by default; `scripts/m6-partition.sh` records them under `docs/evidence/m6/`. The old runner's writes are sent under its node key over the relay, because the runner itself sends nothing once told its grant ended.
