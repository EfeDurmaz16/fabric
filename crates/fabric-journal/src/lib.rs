//! The managed authority's journal (ADR 0020): one versioned `SQLite`
//! database, `<state>/managed/journal.sqlite`, that records every managed
//! job, every operation a caller sent and every event the authority
//! accepted.
//!
//! Invariant: one `Journal` is the only writer of its file, and every
//! change is one `BEGIN IMMEDIATE` transaction that deduplicates the
//! operation key, checks the command with `fabric_core::managed`, appends
//! the events chained to the job's last one, writes the projection they
//! fold into and stores the answer under the key. A method returns only
//! after `COMMIT`, so an answer a caller sees is durable, a retry with the
//! same key gets the same bytes, and a crash before `COMMIT` leaves nothing
//! of the change. Events and evidence are append-only, enforced by
//! triggers. The journal never reads or writes a standalone job's files.
//!
//! What restart recovery does is [`Journal::recover`]: an operation left
//! `dispatched` becomes `unknown`, never dispatched again; an attempt left
//! `granted` or `running` changes only on what the caller's probe can
//! verify; an operation left `accepted` is returned for its one dispatch.

mod crashpoint;
mod project;
mod provider;
mod schema;

#[cfg(test)]
mod crash_tests;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::Command;
use fabric_core::managed::{
    AcceptedOperation, Attempt, AttemptId, AttemptStatus, CheckpointAck, CoveredPath, DomainError,
    Envelope, EventFrame, EventSeq, FenceVerdict, GrantToken, ManagedEvent, ManagedState, NewIds,
    Observation, Operation, OperationId, OperationKey, OperationMark, OperationStatus,
    RecoveryBlock, RequestDigest, StoredResult, Transition, WorkRevision, acknowledge,
    apply_observation, fence, mark_operation, record_recovery_block, replay, request_digest,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

pub use provider::{
    AbandonApproval, AbandonRefusal, Acknowledged, Blocker, CleanupRefusal, EffectClass,
    LocatorRecord, MAX_ABANDON_REASON, ProviderAnswer, ProviderDispatch, ProviderError,
    ProviderOperation, ProviderStatus, Purpose, Residual, ResourceKey,
};
pub use schema::{APPLICATION_ID, SCHEMA_VERSION};

use project::{NewOperation, get_u64, load_state, sql_u64, write_transition};

/// The file name of the journal inside its directory.
pub const FILE_NAME: &str = "journal.sqlite";

/// An error `SQLite` reported, kept opaque so the binding is not part of this crate's API.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct SqliteError(rusqlite::Error);

/// Why a journal did not open or could not be made.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The file is not a Fabric journal.
    #[error("{0} is not a Fabric managed journal")]
    NotAJournal(PathBuf),
    /// The journal was written by an unreleased draft of its schema
    /// version (version 2 before ADR 0022 completed it, or before ADR 0028
    /// amended it); nothing released wrote one, so it is refused rather
    /// than read with missing columns.
    #[error(
        "the journal has a draft of schema version {version} from before the {amendment} amendment that no release wrote; recreate it"
    )]
    DraftSchema {
        /// Its version.
        version: u32,
        /// The amendment the file predates: `m4` (ADR 0022) or `m6` (ADR 0028).
        amendment: &'static str,
    },
    /// The file was written by a newer binary; nothing was written to it.
    #[error("the journal has schema version {found}, this binary knows up to {known}")]
    NewerSchema {
        /// The version in the file.
        found: u32,
        /// The newest version this binary knows.
        known: u32,
    },
    /// The file already exists where a new journal was to be made.
    #[error("{0} already exists")]
    Exists(PathBuf),
    /// `SQLite` failed.
    #[error("journal: {0}")]
    Sqlite(SqliteError),
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        OpenError::Sqlite(SqliteError(e))
    }
}

/// Why a change or a read of the journal failed. A failed change wrote nothing.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// The domain refused it.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// `SQLite` failed.
    #[error("journal: {0}")]
    Sqlite(SqliteError),
    /// A value did not encode.
    #[error("a value did not encode: {0}")]
    Encode(String),
    /// A number does not fit a `SQLite` integer.
    #[error("a number does not fit the journal")]
    Overflow,
    /// Stored rows are not what this binary writes.
    #[error("the journal is corrupt{}: {reason}", job.map(|j| format!(" at job {j}")).unwrap_or_default())]
    Corrupt {
        /// The job, when one is to blame.
        job: Option<JobId>,
        /// What is wrong.
        reason: String,
    },
    /// A provider operation could not be recorded.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The stored projection of a job is not the replay of its events.
    #[error("the projection of job {job} is not the replay of its events")]
    ProjectionMismatch {
        /// The job.
        job: JobId,
    },
}

impl From<rusqlite::Error> for CommitError {
    fn from(e: rusqlite::Error) -> Self {
        CommitError::Sqlite(SqliteError(e))
    }
}

/// The answer to a submitted command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The answer.
    pub result: StoredResult,
    /// Its stored bytes: what every submission under the key gets.
    pub bytes: Vec<u8>,
    /// Whether the key was seen before and nothing new was committed.
    pub replayed: bool,
}

/// One stored event with its place in the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    /// Its sequence number.
    pub seq: EventSeq,
    /// The frame.
    pub frame: EventFrame,
}

/// What `verify` checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyReport {
    /// Managed jobs.
    pub jobs: u64,
    /// Events.
    pub events: u64,
    /// Operations, rejected ones included.
    pub operations: u64,
}

/// What the caller's probe can tell about an attempt the journal holds as
/// `granted` or `running` (replay rule 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Its process group is verified gone: the attempt is lost.
    Gone,
    /// Its process group is verified running.
    Running,
    /// Nothing can be verified; the attempt stays as it is.
    CannotTell,
}

/// What restart recovery did and what it leaves to the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Operations that were `dispatched` and are now `unknown` (rule 3).
    pub unknown: Vec<(JobId, OperationId)>,
    /// Provider operations that were `dispatched` and are now `unknown`:
    /// their request may have reached the worker, and nothing sends it
    /// again (ADR 0021).
    pub provider_unknown: Vec<(JobId, OperationId)>,
    /// Attempts now `lost` because their group is gone (rule 4).
    pub lost: Vec<(JobId, AttemptId)>,
    /// Attempts now `running` because their group runs.
    pub running: Vec<(JobId, AttemptId)>,
    /// Grants still `accepted` whose attempt is still `granted` at the
    /// job's current generation: one dispatch each is the caller's next step (rule 2).
    pub to_dispatch: Vec<(JobId, OperationId)>,
}

/// The journal: one connection, owned by one writer.
pub struct Journal {
    conn: Connection,
    path: PathBuf,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Journal({})", self.path.display())
    }
}

fn connect(path: &Path) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.execute_batch(schema::CONNECTION_PRAGMAS)?;
    Ok(conn)
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> OpenError + '_ {
    move |source| OpenError::Io {
        path: path.to_owned(),
        source,
    }
}

fn fsync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Runs each migration from `version` to this binary's, one transaction
/// each, with foreign keys off: a migration may rebuild a table (ADR
/// 0028), which drops and renames tables other tables reference. Every
/// foreign key is checked before each commit.
fn migrate(conn: &mut Connection, version: u32, path: &Path) -> Result<(), OpenError> {
    conn.pragma_update(None, "foreign_keys", false)?;
    let mut run = || -> Result<(), OpenError> {
        for from in version..SCHEMA_VERSION {
            let Some(step) = usize::try_from(from)
                .ok()
                .and_then(|n| schema::MIGRATIONS.get(n - 1))
            else {
                return Err(OpenError::NotAJournal(path.to_owned()));
            };
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(step)?;
            foreign_keys_hold(&tx)?;
            tx.pragma_update(None, "user_version", from + 1)?;
            tx.commit()?;
        }
        Ok(())
    };
    let outcome = run();
    conn.pragma_update(None, "foreign_keys", true)?;
    outcome
}

/// Fails when any row violates a foreign key, as `PRAGMA foreign_key_check` reports.
fn foreign_keys_hold(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    if rows.next()?.is_some() {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
            Some("a foreign key does not hold after the migration".into()),
        ));
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), OpenError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(path)(e)),
    }
}

impl Journal {
    /// Makes a new journal at `path` for the authority `authority`.
    ///
    /// The database is built as `<path>.tmp`, its schema and `meta` rows
    /// committed, closed, then renamed into place and the directory synced,
    /// so a journal file that exists is always whole.
    pub fn create(path: &Path, authority: NodeId, at_ms: u64) -> Result<Journal, OpenError> {
        if path.exists() {
            return Err(OpenError::Exists(path.to_owned()));
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = tmp.clone().into_os_string();
            name.push(suffix);
            remove_if_present(Path::new(&name))?;
        }
        {
            let mut conn = connect(&tmp)?;
            // A migration rebuilds tables; see `migrate`.
            conn.pragma_update(None, "foreign_keys", false)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(schema::V1)?;
            for step in schema::MIGRATIONS {
                tx.execute_batch(step)?;
            }
            foreign_keys_hold(&tx)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            for (key, value) in [
                ("authority_node", authority.to_string()),
                ("created_at_ms", at_ms.to_string()),
                (
                    "codec_format",
                    fabric_core::managed::MANAGED_FORMAT.to_string(),
                ),
            ] {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                    params![key, value],
                )?;
            }
            tx.commit()?;
            conn.close().map_err(|(_, e)| e)?;
        }
        crashpoint::reached("create.before_rename");
        fs::rename(&tmp, path).map_err(io_error(path))?;
        if let Some(parent) = path.parent() {
            fsync_dir(parent).map_err(io_error(parent))?;
        }
        Journal::open(path)
    }

    /// Opens an existing journal, running each migration from its version
    /// to this binary's. A file that is not a journal, or one from a newer
    /// binary, is refused before anything is written.
    pub fn open(path: &Path) -> Result<Journal, OpenError> {
        if !path.is_file() {
            return Err(OpenError::NotAJournal(path.to_owned()));
        }
        // Read the header without opening a connection that could write:
        // the application id at byte 68 and the user version at byte 60.
        let mut header = [0u8; 100];
        {
            use std::io::Read;
            let mut file = fs::File::open(path).map_err(io_error(path))?;
            if file.read_exact(&mut header).is_err() || &header[..16] != b"SQLite format 3\0" {
                return Err(OpenError::NotAJournal(path.to_owned()));
            }
        }
        let application = i32::from_be_bytes([header[68], header[69], header[70], header[71]]);
        let version = u32::from_be_bytes([header[60], header[61], header[62], header[63]]);
        if application != APPLICATION_ID || version == 0 {
            return Err(OpenError::NotAJournal(path.to_owned()));
        }
        if version > SCHEMA_VERSION {
            return Err(OpenError::NewerSchema {
                found: version,
                known: SCHEMA_VERSION,
            });
        }
        let mut conn = connect(path)?;
        // The header can lag the WAL; ask SQLite as well.
        let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(OpenError::NewerSchema {
                found: version,
                known: SCHEMA_VERSION,
            });
        }
        if version < SCHEMA_VERSION {
            migrate(&mut conn, version, path)?;
        }
        // Version 2 was amended in place before any release (ADR 0022,
        // then ADR 0028): a journal an unreleased build made has a draft
        // table, and is refused before anything is written.
        if conn.prepare(schema::V2_COMPLETE).is_err() {
            return Err(OpenError::DraftSchema {
                version: 2,
                amendment: "m4",
            });
        }
        let attempts: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'attempts'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if !attempts.is_some_and(|sql| sql.contains(schema::V2_M6_STATUS)) {
            return Err(OpenError::DraftSchema {
                version: 2,
                amendment: "m6",
            });
        }
        Ok(Journal {
            conn,
            path: path.to_owned(),
        })
    }

    /// The schema version of the open file.
    pub fn schema_version(&self) -> Result<u32, CommitError> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// The node this journal's authority runs on.
    pub fn authority(&self) -> Result<NodeId, CommitError> {
        let text: String = self.conn.query_row(
            "SELECT value FROM meta WHERE key = 'authority_node'",
            [],
            |r| r.get(0),
        )?;
        text.parse().map_err(|e| CommitError::Corrupt {
            job: None,
            reason: format!("authority_node: {e}"),
        })
    }

    /// The file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Applies a command once.
    ///
    /// A key seen before with the same command answers with the stored
    /// bytes and commits nothing; with another command it is refused as
    /// `OperationConflict`. A command the domain refuses is stored as
    /// rejected under its key, so a retry gets the same refusal. `ids`
    /// are fresh ids the command may use and the time of the commit.
    pub fn submit(
        &mut self,
        envelope: &Envelope<Command>,
        ids: NewIds,
    ) -> Result<Answer, CommitError> {
        let digest = request_digest(envelope).map_err(|e| CommitError::Encode(e.to_string()))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        provider::refuse_provider_key(&tx, &envelope.key)?;
        if let Some((stored_digest, bytes)) = tx
            .query_row(
                "SELECT request_digest, result FROM operations WHERE op_key = ?1",
                [envelope.key.as_str()],
                |r| Ok((r.get::<_, [u8; 32]>(0)?, r.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
        {
            drop(tx);
            if RequestDigest::from_bytes(stored_digest) != digest {
                return Err(DomainError::OperationConflict {
                    key: envelope.key.clone(),
                }
                .into());
            }
            let result = StoredResult::decode(&bytes).map_err(|e| CommitError::Corrupt {
                job: envelope.job,
                reason: format!("the answer under {}: {e}", envelope.key),
            })?;
            return Ok(Answer {
                result,
                bytes,
                replayed: true,
            });
        }
        let state = match envelope.job {
            Some(job) => load_state(&tx, job)?,
            None => None,
        };
        let kind = envelope.body.kind();
        let accepted = provider::accept(&tx, state.as_ref(), envelope, &ids)?;
        let (result, bytes) = match accepted {
            Err(DomainError::Broken { reason }) => {
                return Err(CommitError::Corrupt {
                    job: envelope.job,
                    reason,
                });
            }
            Err(refusal) => {
                let result = StoredResult::Rejected(refusal);
                let bytes = result
                    .encode()
                    .map_err(|e| CommitError::Encode(e.to_string()))?;
                tx.execute(
                    "INSERT INTO operations (operation_id, op_key, request_digest, job_id, kind,
                                             expected_revision, accepted_rev, status, result,
                                             accepted_ms, updated_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'rejected', ?7, ?8, ?8)",
                    params![
                        ids.operation.to_bytes(),
                        envelope.key.as_str(),
                        digest.to_bytes(),
                        state.as_ref().map(|s| s.job.id.to_bytes()),
                        kind.name(),
                        envelope.expected.map(|r| sql_u64(r.get())).transpose()?,
                        bytes,
                        sql_u64(ids.at_ms)?,
                    ],
                )?;
                (result, bytes)
            }
            Ok(transition) => {
                let job = state.as_ref().map_or(ids.job, |s| s.job.id);
                let result = StoredResult::Accepted(AcceptedOperation {
                    operation: ids.operation,
                    job,
                    revision: transition.revision,
                    events: transition.events.clone(),
                });
                let bytes = result
                    .encode()
                    .map_err(|e| CommitError::Encode(e.to_string()))?;
                let op = NewOperation {
                    id: ids.operation,
                    key: &envelope.key,
                    digest,
                    kind,
                    expected: envelope.expected,
                    result: &bytes,
                };
                write_transition(&tx, state, &transition, Some(&op), ids.at_ms)?;
                (result, bytes)
            }
        };
        crashpoint::reached("submit.before_commit");
        tx.commit()?;
        crashpoint::reached("submit.after_commit");
        Ok(Answer {
            result,
            bytes,
            replayed: false,
        })
    }

    /// Records what an executor reports about an attempt or an operation,
    /// behind the fence (ADR 0028): `token` is the grant the executor
    /// acts under and `from` the authenticated peer it reported from,
    /// `None` on the authority's own host. A token the fence refuses
    /// writes `StaleObservationRefused` (when stale) or
    /// `CoveredWriteRefused` and nothing else, and the verdict is
    /// returned. A report that changes nothing writes nothing.
    pub fn observe(
        &mut self,
        token: &GrantToken,
        from: Option<NodeId>,
        observation: &Observation,
        at_ms: u64,
    ) -> Result<Result<Transition, FenceVerdict>, CommitError> {
        self.fenced_change(
            token,
            from,
            CoveredPath::JournalOperation,
            true,
            at_ms,
            |state| apply_observation(state, observation),
        )
    }

    /// Marks an operation on an executor's report, behind the fence.
    pub fn mark(
        &mut self,
        token: &GrantToken,
        from: Option<NodeId>,
        operation: OperationId,
        mark: OperationMark,
        at_ms: u64,
    ) -> Result<Result<Transition, FenceVerdict>, CommitError> {
        self.fenced_change(
            token,
            from,
            CoveredPath::JournalOperation,
            false,
            at_ms,
            |state| mark_operation(state, operation, mark),
        )
    }

    /// Runs the fence for a covered write other than a journal operation
    /// (a ledger append, a bundle publish, a facade outcome), in a
    /// transaction of its own. `Live` returns `Ok(())` and the caller
    /// writes; any other verdict commits `CoveredWriteRefused` and returns
    /// the verdict. Nothing else changes.
    pub fn fence(
        &mut self,
        token: &GrantToken,
        path: CoveredPath,
        from: Option<NodeId>,
        at_ms: u64,
    ) -> Result<Result<(), FenceVerdict>, CommitError> {
        Ok(self
            .fenced_change(token, from, path, false, at_ms, |state| {
                Ok(Transition {
                    revision: state.job.revision,
                    events: Vec::new(),
                })
            })?
            .map(|_| ()))
    }

    /// The authority's own observations (`ResourceUnavailable`,
    /// `ResourceReachable`, and `Lost` from `recover`): no runner presents
    /// a grant, so no fence runs; the M2 generation check still does.
    pub fn observe_authority(
        &mut self,
        job: JobId,
        observation: &Observation,
        at_ms: u64,
    ) -> Result<Transition, CommitError> {
        self.change(job, at_ms, |state| apply_observation(state, observation))
    }

    /// The authority's own marks: `Dispatched` right before an effect is
    /// asked for, so a crash after this commit can never dispatch it
    /// again, and `Unknown` from `recover`. No fence runs.
    pub fn mark_authority(
        &mut self,
        job: JobId,
        operation: OperationId,
        mark: OperationMark,
        at_ms: u64,
    ) -> Result<Transition, CommitError> {
        let transition = self.change(job, at_ms, |state| mark_operation(state, operation, mark))?;
        if mark == OperationMark::Dispatched {
            crashpoint::reached("dispatch.after_commit");
        }
        Ok(transition)
    }

    /// Commits `CheckpointAcknowledged` for `ack` after the core rule
    /// accepted it; the evidence row and the projection column are
    /// written in the same transaction (ADR 0028).
    pub fn acknowledge(
        &mut self,
        job: JobId,
        ack: &CheckpointAck,
        at_ms: u64,
    ) -> Result<Transition, CommitError> {
        self.change(job, at_ms, |state| acknowledge(state, ack))
    }

    /// Records why an automatic restart of `attempt` did not happen (ADR
    /// 0028); the job stays under `Block`.
    pub fn recovery_blocked(
        &mut self,
        job: JobId,
        attempt: AttemptId,
        reason: RecoveryBlock,
        at_ms: u64,
    ) -> Result<Transition, CommitError> {
        self.change(job, at_ms, |state| {
            record_recovery_block(state, attempt, reason)
        })
    }

    /// The job's acknowledged checkpoints, newest first.
    pub fn acknowledged(&self, job: JobId) -> Result<Vec<CheckpointAck>, CommitError> {
        let state = load_state(&self.conn, job)?.ok_or(DomainError::UnknownJob { job })?;
        Ok(state.acknowledged().into_iter().cloned().collect())
    }

    /// One covered write: the fence in the write's own transaction, then
    /// `rule` when the token is live. `report` says a stale token is
    /// recorded as `StaleObservationRefused` (a report) rather than
    /// `CoveredWriteRefused`.
    fn fenced_change(
        &mut self,
        token: &GrantToken,
        from: Option<NodeId>,
        path: CoveredPath,
        report: bool,
        at_ms: u64,
        rule: impl FnOnce(&ManagedState) -> Result<Transition, DomainError>,
    ) -> Result<Result<Transition, FenceVerdict>, CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = load_state(&tx, token.job)?;
        let verdict = fence(state.as_ref(), token, from);
        if !verdict.allows() {
            // No job: nothing to record it against.
            let Some(state) = state else {
                return Ok(Err(verdict));
            };
            let event = match verdict {
                FenceVerdict::Stale { current } if report => {
                    ManagedEvent::StaleObservationRefused {
                        attempt: token.attempt,
                        generation: token.generation,
                        current,
                    }
                }
                ref verdict => ManagedEvent::CoveredWriteRefused {
                    path,
                    attempt: token.attempt,
                    generation: token.generation,
                    verdict: verdict.clone(),
                },
            };
            let refused = Transition {
                revision: state.job.revision.next().ok_or(DomainError::Exhausted)?,
                events: vec![event],
            };
            write_transition(&tx, Some(state), &refused, None, at_ms)?;
            tx.commit()?;
            return Ok(Err(verdict));
        }
        let state = state.ok_or(DomainError::UnknownJob { job: token.job })?;
        let transition = rule(&state)?;
        if transition.events.is_empty() {
            return Ok(Ok(transition));
        }
        write_transition(&tx, Some(state), &transition, None, at_ms)?;
        tx.commit()?;
        Ok(Ok(transition))
    }

    fn change(
        &mut self,
        job: JobId,
        at_ms: u64,
        rule: impl FnOnce(&ManagedState) -> Result<Transition, DomainError>,
    ) -> Result<Transition, CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = load_state(&tx, job)?.ok_or(DomainError::UnknownJob { job })?;
        let transition = rule(&state)?;
        if transition.events.is_empty() {
            return Ok(transition);
        }
        write_transition(&tx, Some(state), &transition, None, at_ms)?;
        tx.commit()?;
        Ok(transition)
    }

    /// A managed job's state, or `None` when the journal has no such job.
    pub fn job(&self, job: JobId) -> Result<Option<ManagedState>, CommitError> {
        load_state(&self.conn, job)
    }

    /// Whether the journal holds `job`.
    pub fn contains(&self, job: JobId) -> Result<bool, CommitError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM jobs WHERE job_id = ?1",
                [job.to_bytes()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Every managed job, oldest first.
    pub fn jobs(&self) -> Result<Vec<JobId>, CommitError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT job_id FROM jobs ORDER BY created_ms, job_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, [u8; 16]>(0))?;
        Ok(rows
            .map(|r| r.map(JobId::from_bytes))
            .collect::<Result<_, _>>()?)
    }

    /// The operation stored under `key`, accepted or rejected.
    pub fn operation(&self, key: &OperationKey) -> Result<Option<Operation>, CommitError> {
        let row = self
            .conn
            .query_row(
                "SELECT operation_id, request_digest, job_id, expected_revision, status, result
                 FROM operations WHERE op_key = ?1",
                [key.as_str()],
                |r| {
                    Ok((
                        r.get::<_, [u8; 16]>(0)?,
                        r.get::<_, [u8; 32]>(1)?,
                        r.get::<_, Option<[u8; 16]>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, digest, job, expected, status, result)) = row else {
            return Ok(None);
        };
        let job = job.map(JobId::from_bytes);
        let corrupt = |reason: String| CommitError::Corrupt { job, reason };
        let status = match status.as_str() {
            "rejected" => {
                match StoredResult::decode(&result).map_err(|e| corrupt(e.to_string()))? {
                    StoredResult::Rejected(refusal) => OperationStatus::Rejected { refusal },
                    StoredResult::Accepted(_) => return Err(corrupt("a rejected accept".into())),
                }
            }
            "accepted" => OperationStatus::Accepted,
            "dispatched" => OperationStatus::Dispatched,
            "succeeded" => OperationStatus::Succeeded,
            "failed" => OperationStatus::Failed,
            "cancelled" => OperationStatus::Cancelled,
            "unknown" => OperationStatus::Unknown,
            other => return Err(corrupt(format!("operation status {other}"))),
        };
        Ok(Some(Operation {
            id: OperationId::from_bytes(id),
            key: key.clone(),
            job,
            expected_revision: expected
                .map(|n| {
                    u64::try_from(n)
                        .ok()
                        .and_then(WorkRevision::new)
                        .ok_or_else(|| corrupt(format!("expected revision {n}")))
                })
                .transpose()?,
            digest: RequestDigest::from_bytes(digest),
            status,
        }))
    }

    /// Up to `limit` events of `job` after `after`, in order.
    pub fn events(
        &self,
        job: JobId,
        after: Option<EventSeq>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, CommitError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, body FROM events WHERE job_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let after = sql_u64(after.map_or(0, EventSeq::get))?;
        let rows = stmt.query_map(params![job.to_bytes(), after, limit], |r| {
            Ok((get_u64(r, 0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, body) = row?;
            let corrupt = |reason: String| CommitError::Corrupt {
                job: Some(job),
                reason,
            };
            out.push(StoredEvent {
                seq: EventSeq::new(seq).ok_or_else(|| corrupt("event seq 0".into()))?,
                frame: EventFrame::decode(&body).map_err(|e| corrupt(e.to_string()))?,
            });
        }
        Ok(out)
    }

    /// Checks every job: each event decodes, belongs to its job, carries
    /// its digest and links to the job's previous event; the replay of the
    /// events is the stored projection; the stored answer of every accepted
    /// operation decodes. Repairs nothing (replay rule 1).
    pub fn verify(&self) -> Result<VerifyReport, CommitError> {
        let mut report = VerifyReport {
            jobs: 0,
            events: 0,
            operations: 0,
        };
        for job in self.jobs()? {
            report.jobs += 1;
            let corrupt = |reason: String| CommitError::Corrupt {
                job: Some(job),
                reason,
            };
            let mut stmt = self.conn.prepare_cached(
                "SELECT seq, revision, body, digest, prev_digest FROM events
                 WHERE job_id = ?1 ORDER BY seq",
            )?;
            let rows = stmt.query_map([job.to_bytes()], |r| {
                Ok((
                    get_u64(r, 0)?,
                    get_u64(r, 1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, [u8; 32]>(3)?,
                    r.get::<_, Option<[u8; 32]>>(4)?,
                ))
            })?;
            let mut history = Vec::new();
            let mut prev = None;
            let mut last_seq = 0;
            for row in rows {
                let (seq, revision, body, digest, prev_digest) = row?;
                report.events += 1;
                let frame =
                    EventFrame::decode(&body).map_err(|e| corrupt(format!("event {seq}: {e}")))?;
                if EventFrame::digest(&body).to_bytes() != digest {
                    return Err(corrupt(format!("event {seq} does not match its digest")));
                }
                if frame.prev.map(fabric_core::managed::EventDigest::to_bytes) != prev_digest
                    || prev_digest != prev
                {
                    return Err(corrupt(format!(
                        "event {seq} does not link to the one before"
                    )));
                }
                if frame.job != job || frame.revision.get() != revision {
                    return Err(corrupt(format!("event {seq} belongs elsewhere")));
                }
                prev = Some(digest);
                last_seq = seq;
                history.push((frame.revision, frame.event));
            }
            let replayed = replay(job, &history).map_err(|e| corrupt(e.to_string()))?;
            let stored = load_state(&self.conn, job)?;
            if stored.as_ref() != Some(&replayed) {
                return Err(CommitError::ProjectionMismatch { job });
            }
            let head: i64 = self.conn.query_row(
                "SELECT head_seq FROM jobs WHERE job_id = ?1",
                [job.to_bytes()],
                |r| r.get(0),
            )?;
            if u64::try_from(head).ok() != Some(last_seq) {
                return Err(CommitError::ProjectionMismatch { job });
            }
        }
        let mut stmt = self
            .conn
            .prepare_cached("SELECT op_key, kind, status, result FROM operations")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        for row in rows {
            let (key, kind, status, result) = row?;
            report.operations += 1;
            let corrupt = |reason: String| CommitError::Corrupt { job: None, reason };
            let answer = StoredResult::decode(&result)
                .map_err(|e| corrupt(format!("the answer under {key}: {e}")))?;
            let rejected = matches!(answer, StoredResult::Rejected(_));
            if rejected != (status == "rejected") || project::command_kind(&kind).is_none() {
                return Err(corrupt(format!(
                    "operation {key} does not match its answer"
                )));
            }
        }
        Ok(report)
    }

    /// Restart recovery (docs/managed-jobs.md, replay rules 2 to 4), one
    /// committed change per operation or attempt it touches.
    ///
    /// An operation left `dispatched` becomes `unknown` and is never
    /// dispatched again, and so does a provider operation (ADR 0021),
    /// listed in `provider_unknown`. An attempt left `granted` or `running` becomes
    /// `lost` only when `probe` verifies its group is gone, and `running`
    /// only when `probe` verifies it runs; `CannotTell` leaves it as it
    /// is. Nothing is granted. Grants still `accepted` whose attempt is
    /// still `granted` at the job's current generation, read after the
    /// probes, are returned for the caller to dispatch once, each after
    /// `mark(Dispatched)`; a lost or running attempt is never offered.
    pub fn recover(
        &mut self,
        at_ms: u64,
        mut probe: impl FnMut(&Attempt) -> Liveness,
    ) -> Result<Recovery, CommitError> {
        let mut out = Recovery::default();
        for (job, operation) in self.provider_dispatched()? {
            self.provider_record(
                operation,
                ProviderStatus::Unknown,
                &ProviderAnswer::default(),
                None,
                at_ms,
            )?;
            out.provider_unknown.push((job, operation));
        }
        for job in self.jobs()? {
            let Some(state) = self.job(job)? else {
                continue;
            };
            for op in &state.operations {
                if op.status == OperationStatus::Dispatched {
                    self.mark_authority(job, op.id, OperationMark::Unknown, at_ms)?;
                    out.unknown.push((job, op.id));
                }
            }
            let mut observed = false;
            for attempt in state
                .attempts
                .iter()
                .filter(|a| matches!(a.status, AttemptStatus::Granted | AttemptStatus::Running))
            {
                let report = match (probe(attempt), &attempt.status) {
                    (Liveness::Gone, _) => Observation::Lost {
                        attempt: attempt.id,
                        generation: attempt.generation,
                    },
                    (Liveness::Running, AttemptStatus::Granted) => Observation::Started {
                        attempt: attempt.id,
                        generation: attempt.generation,
                    },
                    _ => continue,
                };
                let lost = matches!(report, Observation::Lost { .. });
                self.observe_authority(job, &report, at_ms)?;
                observed = true;
                if lost {
                    out.lost.push((job, attempt.id));
                } else {
                    out.running.push((job, attempt.id));
                }
            }
            let now = if observed {
                match self.job(job)? {
                    Some(now) => now,
                    None => continue,
                }
            } else {
                state
            };
            for op in &now.operations {
                if now.grant_may_dispatch(op.id) {
                    out.to_dispatch.push((job, op.id));
                }
            }
        }
        Ok(out)
    }

    /// Writes a consistent copy of the journal to `target`, which must not exist.
    pub fn backup_to(&self, target: &Path) -> Result<(), CommitError> {
        let text = target.to_str().ok_or_else(|| CommitError::Corrupt {
            job: None,
            reason: "the backup path is not UTF-8".into(),
        })?;
        self.conn.execute("VACUUM INTO ?1", [text])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
