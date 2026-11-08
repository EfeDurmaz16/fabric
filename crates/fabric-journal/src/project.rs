//! Reading and writing the projection tables and the event rows.
//!
//! Invariant: `write_transition` is the only writer of `jobs`, `tasks`,
//! `attempts`, `events` and `evidence`, and of the status of an accepted
//! operation. It runs inside the caller's transaction, folds the events
//! into the state with the core's `apply_batch` and writes that state, so
//! the projection is never anything but the replay of the events written
//! beside it.

use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{
    Attempt, AttemptId, AttemptStatus, Checkpoint, CommandKind, CutDigest, EffectRequest,
    EventDigest, EventFrame, Generation, JobState, ManagedEvent, ManagedJob, ManagedState,
    OperationId, OperationKey, OperationProgress, OperationStatus, Program, RequestDigest,
    RestartPolicy, StoredPart, Task, TaskClass, TaskId, TaskKind, TaskState, Transition,
    WorkRevision, apply_batch,
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
        AttemptStatus::Superseded { .. } => "superseded",
    }
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
        checkpoints: load_checkpoints(conn, job)?,
        requests: load_requests(conn, job)?,
        cuts: load_cuts(conn, job)?,
    }))
}

/// The job's checkpoints, captured and acknowledged, folded from the two
/// event kinds that make them (ADR 0028); the attempts table keeps only
/// each attempt's newest acknowledged snapshot.
fn load_checkpoints(conn: &Connection, job: JobId) -> Result<Vec<Checkpoint>, CommitError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, body FROM events
         WHERE job_id = ?1 AND kind IN ('checkpoint_captured', 'checkpoint_acknowledged')
         ORDER BY seq",
    )?;
    let rows = stmt.query_map([job.to_bytes()], |r| {
        Ok((get_u64(r, 0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut checkpoints: Vec<Checkpoint> = Vec::new();
    for row in rows {
        let (seq, body) = row?;
        let frame =
            EventFrame::decode(&body).map_err(|e| corrupt(job, format!("event {seq}: {e}")))?;
        match frame.event {
            ManagedEvent::CheckpointCaptured {
                attempt,
                generation,
                snapshot,
                cut,
            } => checkpoints.push(Checkpoint {
                attempt,
                generation,
                snapshot,
                cut,
                ack: None,
            }),
            ManagedEvent::CheckpointAcknowledged { ack } => {
                let found = checkpoints
                    .iter_mut()
                    .find(|c| c.snapshot == ack.snapshot && c.attempt == ack.attempt)
                    .ok_or_else(|| corrupt(job, format!("event {seq} acknowledges nothing")))?;
                found.ack = Some(ack);
            }
            other => {
                return Err(corrupt(
                    job,
                    format!("event {seq} is a {} under a checkpoint kind", other.name()),
                ));
            }
        }
    }
    Ok(checkpoints)
}

/// The job's checkpoint cuts, read from their events: one per
/// `checkpoint_cut_recorded`, in order, as the replay folds them.
fn load_cuts(
    conn: &Connection,
    job: JobId,
) -> Result<Vec<(fabric_core::managed::AttemptId, fabric_core::id::SnapshotId)>, CommitError> {
    let mut stmt = conn.prepare_cached(
        "SELECT body FROM events WHERE job_id = ?1 AND kind = 'checkpoint_cut_recorded' ORDER BY seq",
    )?;
    let rows = stmt.query_map([job.to_bytes()], |r| r.get::<_, Vec<u8>>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let frame = EventFrame::decode(&row?).map_err(|e| corrupt(job, e))?;
        if let ManagedEvent::CheckpointCutRecorded {
            attempt, snapshot, ..
        } = frame.event
        {
            out.push((attempt, snapshot));
        }
    }
    Ok(out)
}

/// The effects the job's M5 commands asked for, read from their events:
/// one row per `effect_requested`, in order, the same whichever way the
/// state is built.
fn load_requests(
    conn: &Connection,
    job: JobId,
) -> Result<Vec<(OperationId, EffectRequest)>, CommitError> {
    let mut stmt = conn.prepare_cached(
        "SELECT body FROM events WHERE job_id = ?1 AND kind = 'effect_requested' ORDER BY seq",
    )?;
    let rows = stmt.query_map([job.to_bytes()], |r| r.get::<_, Vec<u8>>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let frame = EventFrame::decode(&row?).map_err(|e| corrupt(job, e))?;
        if let ManagedEvent::EffectRequested { operation, request } = frame.event {
            out.push((operation, request));
        }
    }
    Ok(out)
}

fn load_tasks(conn: &Connection, job: JobId) -> Result<Vec<Task>, CommitError> {
    let id = job.to_bytes();
    let mut stmt = conn.prepare_cached(
        "SELECT task_id, parent_id, kind_body, restart, state, class, program FROM tasks
         WHERE job_id = ?1 ORDER BY created_rev, rowid",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok((
            r.get::<_, [u8; 16]>(0)?,
            r.get::<_, Option<[u8; 16]>>(1)?,
            r.get::<_, Vec<u8>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<Vec<u8>>>(6)?,
        ))
    })?;
    let mut tasks = Vec::new();
    for row in rows {
        let (task, parent, kind, restart_name, state, class, program) = row?;
        tasks.push(Task {
            id: TaskId::from_bytes(task),
            job,
            parent: parent.map(TaskId::from_bytes),
            kind: part::<TaskKind>(job, &kind, "task kind")?,
            restart: restart(&restart_name)
                .ok_or_else(|| corrupt(job, format!("restart {restart_name}")))?,
            state: task_state(&state).ok_or_else(|| corrupt(job, format!("task state {state}")))?,
            class: TaskClass::from_name(&class)
                .ok_or_else(|| corrupt(job, format!("task class {class}")))?,
            program: program
                .map(|bytes| part::<Program>(job, &bytes, "task program"))
                .transpose()?,
        });
    }

    Ok(tasks)
}

fn load_attempts(conn: &Connection, job: JobId) -> Result<Vec<Attempt>, CommitError> {
    let id = job.to_bytes();
    let mut stmt = conn.prepare_cached(
        "SELECT attempt_id, task_id, generation, resource, initial_checkpoint, granted_by, status_body,
                unavailable_since_ms
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
            r.get::<_, Option<i64>>(7)?
                .map(|n| {
                    u64::try_from(n).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(7, n))
                })
                .transpose()?,
        ))
    })?;
    let mut attempts = Vec::new();
    for row in rows {
        let (attempt, task, generation, resource, checkpoint, granted_by, status, unavailable) =
            row?;
        attempts.push(Attempt {
            id: AttemptId::from_bytes(attempt),
            task: TaskId::from_bytes(task),
            job,
            resource: part(job, &resource, "resource")?,
            generation,
            initial_checkpoint: checkpoint.map(SnapshotId::from_bytes),
            granted_by: OperationId::from_bytes(granted_by),
            status: part(job, &status, "attempt status")?,
            unavailable_since_ms: unavailable,
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
            kind: CommandKind::from_name(&kind)
                .ok_or_else(|| corrupt(job, format!("kind {kind}")))?,
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
        let evidence = match event {
            ManagedEvent::AttemptObserved {
                attempt, status, ..
            } => match status {
                AttemptStatus::Checkpointed { snapshot } => Some((
                    *attempt,
                    "checkpoint",
                    snapshot.to_string(),
                    Some(snapshot.to_bytes()),
                )),
                AttemptStatus::Succeeded | AttemptStatus::Failed | AttemptStatus::Cancelled => {
                    Some((
                        *attempt,
                        "exit",
                        attempt_status_name(status).to_owned(),
                        None,
                    ))
                }
                _ => None,
            },
            ManagedEvent::CheckpointCaptured {
                attempt,
                snapshot,
                cut,
                ..
            } => Some((
                *attempt,
                "checkpoint",
                snapshot.to_string(),
                Some(cut.to_bytes()),
            )),
            ManagedEvent::CheckpointAcknowledged { ack } => Some((
                ack.attempt,
                "checkpoint_ack",
                ack.snapshot.to_string(),
                Some(ack.cut.to_bytes()),
            )),
            _ => None,
        };
        if let Some((attempt, kind, reference, evidence_digest)) = evidence {
            write_evidence(
                conn,
                job,
                attempt,
                transition.revision,
                kind,
                &reference,
                evidence_digest,
                digest,
                last_seq,
            )?;
        }
        match event {
            ManagedEvent::CheckpointCutRecorded {
                attempt,
                snapshot,
                cut,
                ..
            } => write_cut_evidence(
                conn,
                job,
                *attempt,
                transition.revision,
                *snapshot,
                *cut,
                digest,
                last_seq,
            )?,
            ManagedEvent::PlacementDecided { task, decision } => {
                conn.execute(
                    "INSERT INTO placements (seq, job_id, task_id, kind, chosen, decided_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        last_seq,
                        id,
                        task.to_bytes(),
                        decision.kind.name(),
                        decision.chosen.map(NodeId::to_bytes),
                        sql_u64(decision.decided_at_ms)?,
                    ],
                )?;
            }
            ManagedEvent::ContinuationRecorded {
                attempt,
                from,
                achieved,
                package,
                ..
            } => {
                conn.execute(
                    "INSERT INTO continuations (attempt_id, from_attempt, achieved, package, seq)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        attempt.to_bytes(),
                        from.to_bytes(),
                        achieved.name(),
                        package.map(|p| p.0),
                        last_seq,
                    ],
                )?;
            }
            _ => {}
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
            "INSERT INTO tasks (task_id, job_id, parent_id, kind, kind_body, restart, state, created_rev, class, program)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
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
                task.class.name(),
                task.program.as_ref().map(stored).transpose()?,
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
        let acknowledged = after
            .acknowledged()
            .into_iter()
            .find(|a| a.attempt == attempt.id)
            .map(|a| a.snapshot.to_bytes());
        let ended = (!attempt.status.may_run()).then_some(at);
        conn.execute(
            "INSERT INTO attempts (attempt_id, task_id, job_id, generation, resource,
                                   initial_checkpoint, granted_by, status, status_body,
                                   last_checkpoint, granted_ms, ended_ms, last_acknowledged,
                                   unavailable_since_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT (attempt_id) DO UPDATE SET status = ?8, status_body = ?9,
                                                    last_checkpoint = ?10, ended_ms = ?12,
                                                    last_acknowledged = ?13,
                                                    unavailable_since_ms = ?14",
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
                acknowledged,
                attempt.unavailable_since_ms.map(sql_u64).transpose()?,
            ],
        )?;
    }
    // An acknowledgement changes no attempt row by itself; its column does.
    if let Some(old) = &old {
        for attempt in &after.attempts {
            if old.attempt(attempt.id) != Some(attempt) {
                continue;
            }
            let newest = |s: &ManagedState| {
                s.acknowledged()
                    .into_iter()
                    .find(|a| a.attempt == attempt.id)
                    .map(|a| a.snapshot.to_bytes())
            };
            let (before, now) = (newest(old), newest(&after));
            if before != now {
                conn.execute(
                    "UPDATE attempts SET last_acknowledged = ?2 WHERE attempt_id = ?1",
                    params![attempt.id.to_bytes(), now],
                )?;
            }
        }
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

/// Records evidence for an attempt's checkpoint, acknowledgement or end:
/// a reference, never a claim.
#[allow(
    clippy::too_many_arguments,
    reason = "one row's columns, written from one place"
)]
fn write_evidence(
    conn: &Connection,
    job: JobId,
    attempt: AttemptId,
    revision: WorkRevision,
    kind: &str,
    reference: &str,
    digest: Option<[u8; 32]>,
    event: EventDigest,
    seq: i64,
) -> Result<(), CommitError> {
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

/// Records a checkpoint cut as evidence: the snapshot by name, the cut's
/// digest as the evidence digest (ADR 0023).
#[allow(
    clippy::too_many_arguments,
    reason = "one write with the columns of the evidence row"
)]
fn write_cut_evidence(
    conn: &Connection,
    job: JobId,
    attempt: AttemptId,
    revision: WorkRevision,
    snapshot: SnapshotId,
    cut: CutDigest,
    event: EventDigest,
    seq: i64,
) -> Result<(), CommitError> {
    let mut evidence_id = [0u8; 16];
    evidence_id.copy_from_slice(&event.to_bytes()[..16]);
    conn.execute(
        "INSERT INTO evidence (evidence_id, job_id, attempt_id, revision, kind, ref, digest, seq)
         VALUES (?1, ?2, ?3, ?4, 'checkpoint', ?5, ?6, ?7)",
        params![
            evidence_id,
            job.to_bytes(),
            attempt.to_bytes(),
            sql_u64(revision.get())?,
            snapshot.to_string(),
            cut.0,
            seq,
        ],
    )?;
    Ok(())
}
