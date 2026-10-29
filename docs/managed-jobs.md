# Managed jobs: identity, journal v1, operation states and replay

Status: proposed with ADR 0019 and ADR 0020, 2026-10-27; implemented on the M2 branch the same day. This page is the M2 specification and records where the implementation differs from the first draft ("As built" notes below). The code is the authority on signatures: `crates/fabric-core/src/managed/`, `crates/fabric-journal/` and `crates/fabricd/src/authority.rs`. The section "Reconciliation and cleanup (M4)" was added 2026-10-29 with ADR 0022 and is a specification ahead of its code: there the text is the authority until the M4 implementation lands.

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
| No hot-path regression against the anchor (rule of 2026-10-27, which replaced 1.5x: a tolerance per PR compounds) | `scripts/bench-ab.sh`: anchor `176622e` against the branch, op by op in the same session, the median pair ratio with a 95% bootstrap interval per op from at least 15 pairs, and counters that do not depend on load; raw runs under `docs/evidence/perf-ab/`. As measured on 2026-10-27 (`docs/evidence/perf-ab/README.md`): headless Claude and Codex launch 1.18 to 1.21, release `cli ls` 1.025, Claude and Codex resume 1.03 to 1.04 in the test profile, daemon resident size 1.01 to 1.04; open, with the causes and the fixes listed there. |

## Implementation owner groups

Disjoint files, so the groups can work in parallel; the defect register in `docs/evidence/m2-baseline/defects.md` uses the same names.

| Group | Files |
|---|---|
| G1 managed domain | `crates/fabric-core/src/managed/*`, `crates/fabric-core/src/lib.rs` (module line), `crates/fabric-core/tests/managed_*.rs` |
| G2 journal | `crates/fabric-journal/*` (new crate, depends on `fabric-core` and the workspace `rusqlite`) |
| G3 authority wiring | `crates/fabricd/src/authority.rs` (new), `crates/fabricd/src/control.rs`, `crates/fabricd/src/node.rs`, `crates/fabric-client/src/api.rs`, `protocol/*`, `sdk/typescript/src/*`, `crates/fabric/src/managed.rs` (new). The one dispatch line for the managed subcommands in `crates/fabric/src/lib.rs` lands after group A's launch change, which owns that file. |

As built, the core change also touched, each in one place: `crates/fabricd/src/ops.rs` (one dispatch arm sending the four managed ops to `crates/fabricd/src/managed.rs`), `crates/fabricd/src/error.rs` (the `NodeError::Managed` variant and its reason), `crates/fabric/src/render.rs` (the reply lines of the four ops), and `crates/fabric/tests/e2e/main.rs` (two module lines). The owners of those files keep them; the lines above are the only ones this change added.

## Reconciliation and cleanup (M4)

Status: proposed with ADR 0022, 2026-10-29. Types and signatures only; method bodies are the implementation's. The wire side (the `attach` message, its answers and fixtures) is in docs/bridge-protocol.md, section "Reconciliation (M4)".

Acceptance (roadmap M4): a lost response or a worker restart never triggers a blind duplicate creation; unknown remains explicit; a stop failure blocks an unsafe replacement.

### What M3 already provides

M4 adds only what is missing. These are in the M3 code and tests and do not change:

| Need | M3 mechanism | Pinned by |
|---|---|---|
| Durable intent before any frame | `Journal::provider_begin` commits the row `dispatched` (or `rejected`) in its own `BEGIN IMMEDIATE` transaction; the client records it and queues the frame in one call on the log thread, so the record always precedes the frame | `fabric-journal` `provider::tests::a_key_or_an_id_is_dispatched_once`, `fabric-bridge` `tests/off_runtime.rs`, `tests/process.rs` |
| Nothing sent twice | key and id unique across `operations` and `provider_operations`; `Log(Used)` before any frame | `a_key_or_an_id_is_dispatched_once`, `a_command_cannot_take_a_provider_key` |
| Fencing of effects | `provider_begin` refuses a closed job and any attempt that is not the job's current grant, `granted` or `running`, at the job's generation | `only_the_current_live_attempt_dispatches` |
| Unknown on loss | worker lost, framing broken, deadline plus grace passed, or authority restarted: `dispatched` becomes `unknown`; `unknown` moves only to an observed terminal state | `statuses_move_only_forward`, `the_target_is_kept_from_dispatch_through_recovery`, scenario `worker_loss_is_not_cancellation` |
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
- `searched` is the canonical JSON of the scope the last `absent` attach searched (`{ "provider", "scope", "kind" }`), kept for the owner and for the abandonment acknowledgement.
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
| `error` `invalid`, `unsupported`, `denied`, `unavailable` | `dispatched` | `rejected` |
| `error` `pending`, `result` `accepted` or `running`, `progress` | `dispatched` | `dispatched`, locator and answer stored |
| `error` `unknown`; no answer by the deadline plus grace; worker lost; `recover` | `dispatched` | `unknown` |
| observe or attach `result` (an observed terminal state) | `unknown`, `attached`, `observed` | `succeeded` or `cancelled` (`failed` stays reachable from these for an observed terminal failure, as in M3) |
| attach `error` `pending`; attach `error` `unknown`/`observation_failed` with a locator | `unknown` | `attached`, locator stored |
| attach `error` `unknown`/`absent` | `unknown` | `unknown`, answer and `searched` stored |
| observe `error` `pending` | `unknown`, `attached`, `observed` | `observed`, locator stored |
| observe `error` `pending` | `dispatched` | `dispatched`, locator stored (M3, unchanged) |
| observe or attach refused, `unknown` without a new locator, worker lost or timed out during the read | any | unchanged: a read that learned nothing changes nothing |
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

What does not block: a destroy settled `succeeded` or `rejected` (no effect); an exec settled; a create `attached`, `observed`, settled or `abandoned`. What unblocks a stop that failed: a new destroy of the same resource, with its own fresh approval, that settles `succeeded`. Abandoning a destroy or an exec never unblocks.

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

### M4 acceptance mapped to tests

Each test is written first and fails on the M3 code (an M4 rule that already holds needs no test). Names are the ones the implementation uses; the shared fixtures are in `protocol/bridge/v1`.

| Acceptance | Tests |
|---|---|
| A lost response never triggers a blind duplicate creation | scenarios `attach_after_lost_create`, `attach_absent_stays_unknown`, `attach_then_observe_until_ready` (Fabric scripted run, Capsule fixture run, interop over the worker's stdio); `fabric-journal` `provider::tests::a_create_waits_while_a_create_of_the_job_is_unknown`; interop `a_create_lost_with_its_worker_is_attached_and_never_made_twice` (fixture profile, and the Docker profile on a local engine with an image it already holds) |
| A worker restart never triggers a blind duplicate creation | interop `a_create_lost_with_its_worker_is_attached_and_never_made_twice` kills the worker after the provider made the resource and before the answer, restarts it and attaches; `fabric-journal` `provider::tests::recovery_makes_a_dispatched_create_unknown_and_it_is_attached_not_resent` |
| Unknown remains explicit | `fabric-journal` `provider::tests::statuses_move_only_along_the_machine` (every pair of the table above), `provider::tests::recovery_leaves_attached_observed_and_abandoned_as_they_are`, `provider::tests::an_abandonment_needs_the_state_the_owner_saw`; scenario `attach_absent_stays_unknown` |
| A stop failure blocks an unsafe replacement | `fabric-journal` `provider::tests::a_failed_or_unknown_destroy_blocks_a_replacement_until_a_destroy_succeeds`, `provider::tests::an_unsettled_exec_blocks_a_replacement_until_its_sandbox_is_destroyed`, `provider::tests::abandoning_a_destroy_or_an_exec_never_unblocks_a_replacement`, `provider::tests::an_attached_or_abandoned_create_does_not_block`; the refusal is stored under its key like any domain refusal |
| Fresh cleanup approval | scenario `cleanup_after_attach_needs_fresh_approval`; `fabric-journal` `provider::tests::a_cleanup_runs_after_the_job_is_cancelled`, `provider::tests::a_cleanup_needs_a_named_resource_of_this_job`, `provider::tests::residuals_list_what_no_cleanup_removed`; `fabric-bridge` `tests/process.rs` `a_cleanup_without_a_named_resource_is_refused_before_anything_is_written` |
| Attach and observe only where the profile supports them | scenario `attach_unsupported`; cases `attach_*` and `error_*absent*`; `fabric-bridge` `golden` |
| Owner labels decide | scenario `attach_checks_owner` |
| Version 2 fixture | `crates/fabric-journal/tests/fixtures/v2.sqlite` regenerated with the amended table; `tests/fixtures.rs` opens and verifies it |
| No hot-path regression | the perf phase alone: bridge round trip and spawn (`scripts/bench-bridge.sh`), `budget::hot_paths`, against the anchor; counters (thread hops and committed writes per effect in `tests/off_runtime.rs`) unchanged at one write to dispatch and one read and one write to settle |
