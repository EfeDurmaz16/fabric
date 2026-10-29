# Managed jobs: identity, journal v1, operation states and replay

Status: proposed with ADR 0019 and ADR 0020, 2026-10-27; implemented on the M2 branch the same day. This page is the M2 specification and records where the implementation differs from the first draft ("As built" notes below). The code is the authority on signatures: `crates/fabric-core/src/managed/`, `crates/fabric-journal/` and `crates/fabricd/src/authority.rs`.

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

Version 2 (M3, ADR 0021) adds one table, `provider_operations`, and changes nothing in version 1's tables. A provider operation is an effect an attempt asks a provider worker for over the bridge (docs/bridge-protocol.md): its own `operation_id` and `op_key`, never one `operations` holds or the other way round, the request digest, the job, task and attempt it acts for and the attempt's generation, the bridge action, the `target` locator Fabric sent the action to (written once at dispatch, null for a create), a `status` of `dispatched`, `succeeded`, `failed`, `cancelled`, `rejected` or `unknown`, the last answer's `code`, `reason` and `effect`, and the last `locator` the worker reported. Both locators are stored as their JSON text, which the journal never parses. The target is what keeps an operation lost before any answer, such as a destroy whose worker died, tied to the resource it acted on.

- `provider_begin` commits the row `dispatched` before the request frame is written, or `rejected` when Fabric refused the request before sending it. It refuses a key or id the journal holds, a job that is not active, and an attempt that is not the job's current grant, `granted` or `running` at the job's generation.
- `provider_record` moves `dispatched` to any answer, `unknown` only to `succeeded`, `failed` or `cancelled`, and never moves a terminal status. Recording the status a row has stores the answer and the locator alone, as a `pending` answer does.
- `recover` makes every provider operation left `dispatched` `unknown` and lists it in `Recovery::provider_unknown`; nothing is sent again.

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
