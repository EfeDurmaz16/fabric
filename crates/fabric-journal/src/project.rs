//! Reading and writing the projection tables and the event rows.
//!
//! Invariant: `write_transition` is the only writer of `jobs`, `tasks`,
//! `attempts`, `events` and `evidence`, and of the status of an accepted
//! operation. It runs inside the caller's transaction, folds the events
//! into the state with the core's `apply_batch` and writes that state, so
//! the projection is never anything but the replay of the events written
//! beside it.

use fabric_core::id::{JobId, SnapshotId};
use fabric_core::managed::{
    Attempt, AttemptId, AttemptStatus, CommandKind, EventDigest, EventFrame, Generation, JobState,
    ManagedEvent, ManagedJob, ManagedState, OperationId, OperationKey, OperationProgress,
    OperationStatus, RequestDigest, RestartPolicy, StoredPart, Task, TaskId, TaskKind, TaskState,
    Transition, WorkRevision, apply_batch,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::CommitError;

pub(crate) fn sql_u64(n: u64) -> Result<i64, CommitError> {
    i64::try_from(n).map_err(|_| CommitError::Overflow)
}

pub(crate) fn get_u64(row: &Row<'_>, idx: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(idx)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(idx, value))
}

fn counter<T>(row: &Row<'_>, idx: usize, make: fn(u64) -> Option<T>) -> rusqlite::Result<T> {
    let n = get_u64(row, idx)?;
    make(n).ok_or(rusqlite::Error::IntegralValueOutOfRange(idx, 0))
}

fn corrupt(job: JobId, reason: impl std::fmt::Display) -> CommitError {
    CommitError::Corrupt {
        job: Some(job),
        reason: reason.to_string(),
    }
}

fn part<T: StoredPart>(job: JobId, bytes: &[u8], what: &str) -> Result<T, CommitError> {
    T::from_stored(bytes).map_err(|e| corrupt(job, format!("{what}: {e}")))
}

fn stored<T: StoredPart>(value: &T) -> Result<Vec<u8>, CommitError> {
    value
        .to_stored()
        .map_err(|e| CommitError::Encode(e.to_string()))
}

pub(crate) fn job_state_name(state: JobState) -> &'static str {
    match state {
        JobState::Active => "active",
        JobState::Finished => "finished",
        JobState::Cancelled => "cancelled",
    }
}

fn job_state(name: &str) -> Option<JobState> {
    Some(match name {
        "active" => JobState::Active,
        "finished" => JobState::Finished,
        "cancelled" => JobState::Cancelled,
        _ => return None,
    })
}

fn task_state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "open",
        TaskState::Succeeded => "succeeded",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

fn task_state(name: &str) -> Option<TaskState> {
    Some(match name {
        "open" => TaskState::Open,
        "succeeded" => TaskState::Succeeded,
        "failed" => TaskState::Failed,
        "cancelled" => TaskState::Cancelled,
        _ => return None,
    })
}

fn restart_name(restart: RestartPolicy) -> &'static str {
    match restart {
        RestartPolicy::Block => "block",
        RestartPolicy::ExplicitOnly => "explicit_only",
    }
}

fn restart(name: &str) -> Option<RestartPolicy> {
    Some(match name {
        "block" => RestartPolicy::Block,
        "explicit_only" => RestartPolicy::ExplicitOnly,
        _ => return None,
    })
}

pub(crate) fn attempt_status_name(status: &AttemptStatus) -> &'static str {
    match status {
        AttemptStatus::Granted => "granted",
        AttemptStatus::Running => "running",
        AttemptStatus::Succeeded => "succeeded",
        AttemptStatus::Failed => "failed",
        AttemptStatus::Cancelled => "cancelled",
        AttemptStatus::Checkpointed { .. } => "checkpointed",
        AttemptStatus::Lost => "lost",
        AttemptStatus::Unknown => "unknown",
    }
}

pub(crate) fn command_kind(name: &str) -> Option<CommandKind> {
    Some(match name {
        "create_job" => CommandKind::CreateJob,
        "start_attempt" => CommandKind::StartAttempt,
        "register_task" => CommandKind::RegisterTask,
        "finish_task" => CommandKind::FinishTask,
        "cancel_job" => CommandKind::CancelJob,
        _ => return None,
    })
}

/// The status of an accepted operation from its column; `None` for `rejected`, which has no progress.
fn progress_status(name: &str) -> Option<OperationStatus> {
    Some(match name {
        "accepted" => OperationStatus::Accepted,
        "dispatched" => OperationStatus::Dispatched,
        "succeeded" => OperationStatus::Succeeded,
        "failed" => OperationStatus::Failed,
        "cancelled" => OperationStatus::Cancelled,
        "unknown" => OperationStatus::Unknown,
        _ => return None,
    })
}

/// Reads a job's projection, or `None` when the journal has no such job.
pub(crate) fn load_state(
    conn: &Connection,
    job: JobId,
) -> Result<Option<ManagedState>, CommitError> {
    let id = job.to_bytes();
    let row = conn
        .query_row(
            "SELECT revision, generation, state, primary_task, intent, source, policy
             FROM jobs WHERE job_id = ?1",
            [id],
            |r| {
                Ok((
                    counter(r, 0, WorkRevision::new)?,
                    counter(r, 1, Generation::new)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, [u8; 16]>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, Vec<u8>>(5)?,
                    r.get::<_, Vec<u8>>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((revision, generation, state, primary, intent, source, policy)) = row else {
        return Ok(None);
    };
    let managed = ManagedJob {
        id: job,
        revision,
        generation,
        intent: part(job, &intent, "intent")?,
        source: part(job, &source, "source")?,
        policy: part(job, &policy, "policy")?,
        state: job_state(&state).ok_or_else(|| corrupt(job, format!("job state {state}")))?,
        primary: TaskId::from_bytes(primary),
    };

    Ok(Some(ManagedState {
        job: managed,
        tasks: load_tasks(conn, job)?,
        attempts: load_attempts(conn, job)?,
        operations: load_operations(conn, job)?,
    }))
}

fn load_tasks(conn: &Connection, job: JobId) -> Result<Vec<Task>, CommitError> {
    let id = job.to_bytes();
    let mut stmt = conn.prepare_cached(
        "SELECT task_id, parent_id, kind_body, restart, state FROM tasks
         WHERE job_id = ?1 ORDER BY created_rev, rowid",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok((
            r.get::<_, [u8; 16]>(0)?,
            r.get::<_, Option<[u8; 16]>>(1)?,
            r.get::<_, Vec<u8>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut tasks = Vec::new();
    for row in rows {
        let (task, parent, kind, restart_name, state) = row?;
        tasks.push(Task {
            id: TaskId::from_bytes(task),
            job,
            parent: parent.map(TaskId::from_bytes),
            kind: part::<TaskKind>(job, &kind, "task kind")?,
            restart: restart(&restart_name)
                .ok_or_else(|| corrupt(job, format!("restart {restart_name}")))?,
            state: task_state(&state).ok_or_else(|| corrupt(job, format!("task state {state}")))?,
        });
    }

    Ok(tasks)
}

fn load_attempts(conn: &Connection, job: JobId) -> Result<Vec<Attempt>, CommitError> {
    let id = job.to_bytes();
    let mut stmt = conn.prepare_cached(
        "SELECT attempt_id, task_id, generation, resource, initial_checkpoint, granted_by, status_body
         FROM attempts WHERE job_id = ?1 ORDER BY generation",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok((
            r.get::<_, [u8; 16]>(0)?,
            r.get::<_, [u8; 16]>(1)?,
            counter(r, 2, Generation::new)?,
            r.get::<_, Vec<u8>>(3)?,
            r.get::<_, Option<[u8; 32]>>(4)?,
            r.get::<_, [u8; 16]>(5)?,
            r.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    let mut attempts = Vec::new();
    for row in rows {
        let (attempt, task, generation, resource, checkpoint, granted_by, status) = row?;
        attempts.push(Attempt {
            id: AttemptId::from_bytes(attempt),
            task: TaskId::from_bytes(task),
            job,
            resource: part(job, &resource, "resource")?,
            generation,
            initial_checkpoint: checkpoint.map(SnapshotId::from_bytes),
            granted_by: OperationId::from_bytes(granted_by),
            status: part(job, &status, "attempt status")?,
        });
    }

    Ok(attempts)
}

fn load_operations(conn: &Connection, job: JobId) -> Result<Vec<OperationProgress>, CommitError> {
    let id = job.to_bytes();
    let mut stmt = conn.prepare_cached(
        "SELECT operation_id, op_key, request_digest, kind, accepted_rev, status FROM operations
         WHERE job_id = ?1 AND status != 'rejected' ORDER BY first_seq",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok((
            r.get::<_, [u8; 16]>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, [u8; 32]>(2)?,
            r.get::<_, String>(3)?,
            counter(r, 4, WorkRevision::new)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    let mut operations = Vec::new();
    for row in rows {
        let (op, key, digest, kind, revision, status) = row?;
        operations.push(OperationProgress {
            id: OperationId::from_bytes(op),
            key: OperationKey::parse(&key).map_err(|e| corrupt(job, e))?,
            digest: RequestDigest::from_bytes(digest),
            kind: command_kind(&kind).ok_or_else(|| corrupt(job, format!("kind {kind}")))?,
            revision,
            status: progress_status(&status)
                .ok_or_else(|| corrupt(job, format!("operation status {status}")))?,
        });
    }

    Ok(operations)
}

/// The digest of the job's last event, which its next event links to.
fn head_digest(conn: &Connection, job: JobId) -> Result<Option<EventDigest>, CommitError> {
    Ok(conn
        .query_row(
            "SELECT digest FROM events WHERE job_id = ?1 ORDER BY seq DESC LIMIT 1",
            [job.to_bytes()],
            |r| r.get::<_, [u8; 32]>(0),
        )
        .optional()?
        .map(EventDigest::from_bytes))
}

/// A new operation row, inserted with its command's events.
pub(crate) struct NewOperation<'a> {
    pub id: OperationId,
    pub key: &'a OperationKey,
    pub digest: RequestDigest,
    pub kind: CommandKind,
    pub expected: Option<WorkRevision>,
    pub result: &'a [u8],
}

/// Writes one transition: its events, chained to the job's last one, and
/// the state they fold into. `op` is the new operation whose command made
/// the transition, if one did. Returns the new state.
#[allow(
    clippy::too_many_lines,
    reason = "one write per table, in the order the foreign keys need"
)]
pub(crate) fn write_transition(
    conn: &Connection,
    before: Option<ManagedState>,
    transition: &Transition,
    op: Option<&NewOperation<'_>>,
    at_ms: u64,
) -> Result<ManagedState, CommitError> {
    let old = before.clone();
    let after = apply_batch(before, transition.revision, &transition.events)?;
    let job = after.job.id;
    let id = job.to_bytes();
    let at = sql_u64(at_ms)?;

    conn.execute(
        "INSERT INTO jobs (job_id, mode, revision, generation, state, primary_task, intent,
                           source, policy, head_seq, created_ms, updated_ms)
         VALUES (?1, 'managed', ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?9)
         ON CONFLICT (job_id) DO UPDATE SET revision = ?2, generation = ?3, state = ?4,
                                            updated_ms = ?9",
        params![
            id,
            sql_u64(after.job.revision.get())?,
            sql_u64(after.job.generation.get())?,
            job_state_name(after.job.state),
            after.job.primary.to_bytes(),
            stored(&after.job.intent)?,
            stored(&after.job.source)?,
            stored(&after.job.policy)?,
            at,
        ],
    )?;

    if let Some(op) = op {
        let status = after
            .operation(op.id)
            .map(|p| p.status.name())
            .ok_or_else(|| corrupt(job, "the new operation is not in the new state"))?;
        conn.execute(
            "INSERT INTO operations (operation_id, op_key, request_digest, job_id, kind,
                                     expected_revision, accepted_rev, status, result,
                                     accepted_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                op.id.to_bytes(),
                op.key.as_str(),
                op.digest.to_bytes(),
                id,
                op.kind.name(),
                op.expected.map(|r| sql_u64(r.get())).transpose()?,
                sql_u64(transition.revision.get())?,
                status,
                op.result,
                at,
            ],
        )?;
    }

    let mut prev = head_digest(conn, job)?;
    let mut first_seq = None;
    let mut last_seq = 0i64;
    for event in &transition.events {
        let frame = EventFrame {
            job,
            revision: transition.revision,
            operation: op.map(|o| o.id),
            prev,
            at_ms,
            event: event.clone(),
        };
        let body = frame
            .encode()
            .map_err(|e| CommitError::Encode(e.to_string()))?;
        let digest = EventFrame::digest(&body);
        conn.execute(
            "INSERT INTO events (job_id, revision, operation_id, kind, body, digest, prev_digest, at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                sql_u64(transition.revision.get())?,
                op.map(|o| o.id.to_bytes()),
                event.name(),
                body,
                digest.to_bytes(),
                prev.map(EventDigest::to_bytes),
                at,
            ],
        )?;
        last_seq = conn.last_insert_rowid();
        first_seq.get_or_insert(last_seq);
        if let ManagedEvent::AttemptObserved {
            attempt, status, ..
        } = event
        {
            write_evidence(
                conn,
                job,
                *attempt,
                transition.revision,
                status,
                digest,
                last_seq,
            )?;
        }
        prev = Some(digest);
    }
    conn.execute(
        "UPDATE jobs SET head_seq = ?2 WHERE job_id = ?1",
        params![id, last_seq],
    )?;
    if let (Some(op), Some(first)) = (op, first_seq) {
        conn.execute(
            "UPDATE operations SET first_seq = ?2 WHERE operation_id = ?1",
            params![op.id.to_bytes(), first],
        )?;
    }

    for task in &after.tasks {
        let unchanged = old
            .as_ref()
            .and_then(|o| o.task(task.id))
            .is_some_and(|t| t == task);
        if unchanged {
            continue;
        }
        conn.execute(
            "INSERT INTO tasks (task_id, job_id, parent_id, kind, kind_body, restart, state, created_rev)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (task_id) DO UPDATE SET state = ?7",
            params![
                task.id.to_bytes(),
                id,
                task.parent.map(TaskId::to_bytes),
                match task.kind {
                    TaskKind::Primary => "primary",
                    TaskKind::Child { .. } => "child",
                },
                stored(&task.kind)?,
                restart_name(task.restart),
                task_state_name(task.state),
                sql_u64(transition.revision.get())?,
            ],
        )?;
    }

    for attempt in &after.attempts {
        let previous = old.as_ref().and_then(|o| o.attempt(attempt.id));
        if previous == Some(attempt) {
            continue;
        }
        let checkpoint = match attempt.status {
            AttemptStatus::Checkpointed { snapshot } => Some(snapshot.to_bytes()),
            _ => None,
        };
        let ended = (!attempt.status.may_run()).then_some(at);
        conn.execute(
            "INSERT INTO attempts (attempt_id, task_id, job_id, generation, resource,
                                   initial_checkpoint, granted_by, status, status_body,
                                   last_checkpoint, granted_ms, ended_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (attempt_id) DO UPDATE SET status = ?8, status_body = ?9,
                                                    last_checkpoint = ?10, ended_ms = ?12",
            params![
                attempt.id.to_bytes(),
                attempt.task.to_bytes(),
                id,
                sql_u64(attempt.generation.get())?,
                stored(&attempt.resource)?,
                attempt.initial_checkpoint.map(SnapshotId::to_bytes),
                attempt.granted_by.to_bytes(),
                attempt_status_name(&attempt.status),
                stored(&attempt.status)?,
                checkpoint,
                at,
                ended,
            ],
        )?;
    }

    for progress in &after.operations {
        let previous = old.as_ref().and_then(|o| o.operation(progress.id));
        if previous.is_none() || previous == Some(progress) {
            // A new operation was inserted above with its status.
            continue;
        }
        conn.execute(
            "UPDATE operations SET status = ?2, updated_ms = ?3 WHERE operation_id = ?1",
            params![progress.id.to_bytes(), progress.status.name(), at],
        )?;
    }
    Ok(after)
}

/// Records evidence for an attempt's checkpoint or end: a reference, never a claim.
fn write_evidence(
    conn: &Connection,
    job: JobId,
    attempt: AttemptId,
    revision: WorkRevision,
    status: &AttemptStatus,
    event: EventDigest,
    seq: i64,
) -> Result<(), CommitError> {
    let (kind, reference, digest) = match status {
        AttemptStatus::Checkpointed { snapshot } => (
            "checkpoint",
            snapshot.to_string(),
            Some(snapshot.to_bytes()),
        ),
        AttemptStatus::Succeeded | AttemptStatus::Failed | AttemptStatus::Cancelled => {
            ("exit", attempt_status_name(status).to_owned(), None)
        }
        _ => return Ok(()),
    };
    let mut evidence_id = [0u8; 16];
    evidence_id.copy_from_slice(&event.to_bytes()[..16]);
    conn.execute(
        "INSERT INTO evidence (evidence_id, job_id, attempt_id, revision, kind, ref, digest, seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            evidence_id,
            job.to_bytes(),
            attempt.to_bytes(),
            sql_u64(revision.get())?,
            kind,
            reference,
            digest,
            seq,
        ],
    )?;
    Ok(())
}
