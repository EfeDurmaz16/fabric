# Managed jobs: identity, journal v1, operation states and replay

Status: proposed with ADR 0019 and ADR 0020, 2026-10-27. This page is the M2 specification. It gives types and signatures only; bodies come with the implementation.

## Identity

| Name | Form | Made by | Meaning |
|---|---|---|---|
| `JobId` | ULID, existing type | authority, at creation | The durable work. Never reused, never changes mode. |
| `TaskId` | ULID | authority | One logical piece of a job's work. Every managed job gets its primary task at creation. |
| `AttemptId` | ULID | authority, when it grants | One execution of one task on one resource under one generation. |
| `OperationId` | ULID | authority, at commit | One accepted command. |
| `OperationKey` | 1 to 128 bytes of `[A-Za-z0-9._:-]` | caller | The caller's name for one command; the dedupe key. A ULID is a good key. |
| `RequestDigest` | 32 bytes | authority | blake3 of `"fabric.managed.command.v1\0"` followed by the command's canonical bytes. |
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

Rules the adapter keeps:

- `events` is the history; `jobs`, `tasks`, `attempts` and the `status` of `operations` are projections, written in the same transaction as the events that change them. `verify` replays every job's events and compares the result with the projection.
- An event's `prev_digest` is the `digest` of the job's previous event; the first event of a job has none. `verify` checks the chain.
- `operations.result` holds the canonical bytes of the answer the caller got, so a replayed key gets the same bytes.
- `UNIQUE (job_id, generation)` on `attempts` makes two grants at one generation impossible at the storage level, not only in the domain.

### Versioning and migration

- `create` builds the file as `journal.sqlite.tmp`, sets the pragmas, runs the version 1 DDL and the `meta` rows in one transaction, closes it, renames it into place and syncs the directory, as `Catalog::create` does.
- `open` checks `application_id` (else `NotAJournal`), then `user_version`: equal to the binary's version opens; lower runs each migration `n -> n + 1` in its own `BEGIN IMMEDIATE` transaction that ends by setting `user_version = n + 1`; higher is `NewerSchema` and nothing is written.
- Every released version keeps a fixture database under `crates/fabric-journal/tests/fixtures/v<n>.sqlite`; the tests open each with the current binary and run `verify`.
- A migration never rewrites an event's `body` or `digest`. A new event shape is a new `kind`, or a new codec format byte read beside the old one.

### Idempotency keys from command ids

- The CLI makes a fresh ULID per invocation and passes it as the key; `--operation-key <key>` lets a script choose its own so a retry after a lost answer is the same command.
- The client API takes `operation_key` as a required field on every managed op; the SDKs generate one when the caller gives none and expose it, so the caller can retry with it.
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
2. An operation left `accepted` at restart is still accepted: the command was durable, and its dispatch is the next step, done once.
3. An operation left `dispatched` at restart becomes `unknown` in a new event. It is not dispatched again.
4. An attempt left `granted` or `running` at restart stays as it is until the executor reports or a stop is verified. The authority checks the recorded process group (as `local.rs` does for standalone harnesses): a group that is gone is `lost`; a group that still runs is `running`. Neither grants a new attempt.
5. A worker loss is `lost` for the attempt, never `cancelled` for the task. The task stays open.
6. A timeout is never permission to replay an operation or grant a replacement. A replacement attempt is a new `StartAttempt` command from the caller, accepted only when no attempt of the task may still run, and it gets the next generation.
7. An observation at an older generation is recorded as `StaleObservationRefused` and changes nothing else.
8. A duplicate submission after restart is answered from `operations` like any other.

## Compatibility with standalone jobs

- The journal never opens, writes or renames anything under `<state>/jobs/<job>/`. Ledger bytes, snapshot manifests and the store catalog are untouched.
- `fabric job ls` lists standalone jobs from their ledgers and managed jobs from the journal, with the mode.
- A state directory without `managed/journal.sqlite` behaves exactly as today; the journal is created by the first managed command, not by `fabric serve`.
- M2 proves this with frozen fixtures: a state directory with standalone ledgers and snapshots made by the main branch before M2 (commit 176622e) opens, lists, suspends, moves and resumes under the M2 binary, and its files are byte-identical before and after a managed job is created beside it.

## M2 acceptance mapped to tests

| Acceptance (roadmap M2) | Test |
|---|---|
| Authority restart preserves identity and progress | Create a managed job, grant an attempt, kill the daemon with `SIGKILL` at each journal fault point, restart: same `JobId`, same revision, attempt not re-granted, `dispatched` operations `unknown`. |
| Duplicate command preserves identity | Submit the same key twice, before and after a restart: one operation, one event set, byte-identical answers. Same key, other body: `OperationConflict`. |
| Launch before registration closed | R2 tests (`a_headless_launch_runs_nothing_until_it_is_released`, `the_trait_launch_holds_the_agent_until_it_is_released`) pass, plus a CLI `SIGKILL` between spawn and registration leaves no harness process. |
| Fresh managed identity and replay | `replay(events) == projection` for every job after every test; property test over random command and observation sequences. |
| Legacy compatibility | The frozen standalone fixture state directory above. |
| Promotion separately gated | No command converts a standalone job; a test asserts a managed create of an id with a ledger is refused. |
| No hot-path regression above 1.5x | `scripts/bench-m2.sh` before and after, same machine, raw runs kept; `budget::hot_paths` process counts unchanged. |

## Implementation owner groups

Disjoint files, so the groups can work in parallel; the defect register in `docs/evidence/m2-baseline/defects.md` uses the same names.

| Group | Files |
|---|---|
| G1 managed domain | `crates/fabric-core/src/managed/*`, `crates/fabric-core/src/lib.rs` (module line), `crates/fabric-core/tests/managed_*.rs` |
| G2 journal | `crates/fabric-journal/*` (new crate, depends on `fabric-core` and the workspace `rusqlite`) |
| G3 authority wiring | `crates/fabricd/src/authority.rs` (new), `crates/fabricd/src/control.rs`, `crates/fabricd/src/node.rs`, `crates/fabric-client/src/api.rs`, `protocol/*`, `sdk/typescript/src/*`, `crates/fabric/src/managed.rs` (new). The one dispatch line for the managed subcommands in `crates/fabric/src/lib.rs` lands after group A's launch change, which owns that file. |
