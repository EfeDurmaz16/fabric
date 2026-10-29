//! The rules: which commands and observations a managed job accepts, the
//! events they make, and the one function that folds events into state.
//!
//! Invariant: `accept_command`, `apply_observation` and `mark_operation`
//! only ever return events that `apply_batch` accepts on the same state,
//! and `replay` is `apply_batch` over a stored history, so the state the
//! journal projects after a commit is the state a replay of its events
//! gives. Nothing here reads a clock or makes an id.

use super::frame::request_digest;
use super::ids::{Generation, OperationId, WorkRevision};
use super::model::{
    Attempt, AttemptStatus, Command, CommandKind, Dedupe, DomainError, Envelope, ExitOutcome,
    JobState, ManagedEvent, ManagedJob, ManagedState, NewIds, Observation, Operation,
    OperationMark, OperationProgress, OperationStatus, Task, TaskKind, TaskState, Transition,
};
use crate::id::JobId;

/// Decides what a command under a key that may have been seen gets.
#[must_use]
pub fn dedupe(stored: Option<&Operation>, digest: &super::ids::RequestDigest) -> Dedupe {
    match stored {
        None => Dedupe::New,
        Some(op) if op.digest == *digest => Dedupe::Replay,
        Some(_) => Dedupe::Conflict,
    }
}

fn next_revision(state: &ManagedState) -> Result<WorkRevision, DomainError> {
    state.job.revision.next().ok_or(DomainError::Exhausted)
}

/// The generation the job's next grant gets: one for its first, else one more than its latest.
fn next_generation(state: &ManagedState) -> Result<Generation, DomainError> {
    if state.attempts.is_empty() {
        Ok(Generation::FIRST)
    } else {
        state.job.generation.next().ok_or(DomainError::Exhausted)
    }
}

/// Checks a command against the job it names and returns its events.
///
/// `job` is the job the envelope names, or `None` when the journal has
/// none. The first event is always `CommandAccepted` for `ids.operation`.
pub fn accept_command(
    job: Option<&ManagedState>,
    envelope: &Envelope<Command>,
    ids: &NewIds,
) -> Result<Transition, DomainError> {
    let digest = request_digest(envelope).map_err(|e| DomainError::Broken {
        reason: e.to_string(),
    })?;
    let kind = envelope.body.kind();
    let accepted = ManagedEvent::CommandAccepted {
        operation: ids.operation,
        key: envelope.key.clone(),
        digest,
        kind,
    };
    if let Command::CreateJob {
        intent,
        source,
        policy,
    } = &envelope.body
    {
        if envelope.job.is_some() || envelope.expected.is_some() || job.is_some() {
            return Err(DomainError::BadEnvelope);
        }
        return Ok(Transition {
            revision: WorkRevision::FIRST,
            events: vec![
                accepted,
                ManagedEvent::JobCreated {
                    job: ids.job,
                    intent: intent.clone(),
                    source: source.clone(),
                    policy: *policy,
                    primary: ids.task,
                },
                ManagedEvent::TaskRegistered {
                    task: ids.task,
                    parent: None,
                    kind: TaskKind::Primary,
                    restart: policy.restart,
                },
            ],
        });
    }
    let Some(named) = envelope.job else {
        return Err(DomainError::BadEnvelope);
    };
    let Some(state) = job else {
        return Err(DomainError::UnknownJob { job: named });
    };
    if state.job.id != named {
        return Err(DomainError::BadEnvelope);
    }
    if let Some(expected) = envelope.expected
        && expected != state.job.revision
    {
        return Err(DomainError::RevisionMismatch {
            expected,
            current: state.job.revision,
        });
    }
    if state.job.state != JobState::Active {
        return Err(DomainError::JobClosed { job: named });
    }
    let revision = next_revision(state)?;
    let event = command_event(state, &envelope.body, ids)?;
    Ok(Transition {
        revision,
        events: vec![accepted, event],
    })
}

/// The event a command other than `CreateJob` makes on an active job.
fn command_event(
    state: &ManagedState,
    body: &Command,
    ids: &NewIds,
) -> Result<ManagedEvent, DomainError> {
    Ok(match body {
        Command::CreateJob { .. } => return Err(DomainError::BadEnvelope),
        Command::StartAttempt {
            task,
            resource,
            initial_checkpoint,
        } => {
            open_task(state, *task)?;
            if let Some(live) = state.live_attempt() {
                return Err(DomainError::AttemptStillLive { attempt: live.id });
            }
            ManagedEvent::AttemptGranted {
                attempt: ids.attempt,
                task: *task,
                resource: resource.clone(),
                generation: next_generation(state)?,
                initial_checkpoint: *initial_checkpoint,
                operation: ids.operation,
            }
        }
        Command::RegisterTask {
            parent,
            kind,
            restart,
        } => {
            if state.task(*parent).is_none() || *kind == TaskKind::Primary {
                return Err(DomainError::BadParent { task: *parent });
            }
            open_task(state, *parent)?;
            ManagedEvent::TaskRegistered {
                task: ids.task,
                parent: Some(*parent),
                kind: kind.clone(),
                restart: *restart,
            }
        }
        Command::FinishTask { task, outcome } => {
            open_task(state, *task)?;
            if let Some(live) = state
                .attempts
                .iter()
                .find(|a| a.task == *task && a.status.may_run())
            {
                return Err(DomainError::AttemptStillLive { attempt: live.id });
            }
            ManagedEvent::TaskFinished {
                task: *task,
                outcome: *outcome,
            }
        }
        Command::CancelJob { reason } => {
            if let Some(live) = state.live_attempt() {
                return Err(DomainError::AttemptStillLive { attempt: live.id });
            }
            ManagedEvent::JobCancelled {
                reason: reason.clone(),
            }
        }
    })
}

fn open_task(state: &ManagedState, task: super::ids::TaskId) -> Result<&Task, DomainError> {
    let found = state.task(task).ok_or(DomainError::UnknownTask { task })?;
    if found.state == TaskState::Open {
        Ok(found)
    } else {
        Err(DomainError::TaskClosed { task })
    }
}

/// No change: the transition the authority writes nothing for.
fn unchanged(state: &ManagedState) -> Transition {
    Transition {
        revision: state.job.revision,
        events: Vec::new(),
    }
}

/// Checks a report about an attempt or an operation and returns its events.
///
/// A report that repeats what the job already holds changes nothing. A
/// report at an older generation is recorded as refused and changes
/// nothing else (replay rule 7). A lost worker makes its attempt `Lost`
/// and leaves its task open (rule 5).
pub fn apply_observation(
    job: &ManagedState,
    observation: &Observation,
) -> Result<Transition, DomainError> {
    let (attempt_id, generation, status) = match observation {
        Observation::Unknown { operation } => {
            return mark_operation(job, *operation, OperationMark::Unknown);
        }
        Observation::Started {
            attempt,
            generation,
        } => (*attempt, *generation, AttemptStatus::Running),
        Observation::Checkpointed {
            attempt,
            generation,
            snapshot,
        } => (
            *attempt,
            *generation,
            AttemptStatus::Checkpointed {
                snapshot: *snapshot,
            },
        ),
        Observation::Exited {
            attempt,
            generation,
            outcome,
        } => (
            *attempt,
            *generation,
            match outcome {
                ExitOutcome::Succeeded => AttemptStatus::Succeeded,
                ExitOutcome::Failed { .. } => AttemptStatus::Failed,
                ExitOutcome::Cancelled => AttemptStatus::Cancelled,
            },
        ),
        Observation::Lost {
            attempt,
            generation,
        } => (*attempt, *generation, AttemptStatus::Lost),
    };
    let attempt = job.attempt(attempt_id).ok_or(DomainError::UnknownAttempt {
        attempt: attempt_id,
    })?;
    let current = job.job.generation;
    if generation > current {
        return Err(DomainError::StaleGeneration {
            given: generation,
            current,
        });
    }
    if generation < current {
        return Ok(Transition {
            revision: next_revision(job)?,
            events: vec![ManagedEvent::StaleObservationRefused {
                attempt: attempt_id,
                generation,
                current,
            }],
        });
    }
    if attempt.generation != generation {
        return Err(DomainError::StaleGeneration {
            given: generation,
            current: attempt.generation,
        });
    }
    if attempt.status == status {
        return Ok(unchanged(job));
    }
    if !attempt.status.may_run() {
        return Err(DomainError::AttemptEnded {
            attempt: attempt_id,
        });
    }
    let mut events = vec![ManagedEvent::AttemptObserved {
        attempt: attempt_id,
        generation,
        status: status.clone(),
    }];
    // An attempt seen started or ended was launched: the grant's dispatch
    // succeeded, whatever the attempt then did. A loss says nothing about it.
    if status != AttemptStatus::Lost
        && let Some(grant) = job.operation(attempt.granted_by)
        && matches!(
            grant.status,
            OperationStatus::Dispatched | OperationStatus::Unknown
        )
    {
        events.push(ManagedEvent::OperationMarked {
            operation: grant.id,
            status: OperationMark::Succeeded,
        });
    }
    Ok(Transition {
        revision: next_revision(job)?,
        events,
    })
}

/// Whether an operation may move from `from` to `to`; `Ok(false)` when it is already there.
fn mark_allowed(from: &OperationStatus, to: OperationMark) -> Result<bool, ()> {
    if *from == to.status() {
        return Ok(false);
    }
    let allowed = match to {
        OperationMark::Dispatched => *from == OperationStatus::Accepted,
        OperationMark::Succeeded
        | OperationMark::Failed
        | OperationMark::Cancelled
        | OperationMark::Unknown => {
            matches!(from, OperationStatus::Dispatched | OperationStatus::Unknown)
        }
    };
    if allowed { Ok(true) } else { Err(()) }
}

/// Moves an accepted operation on: `Dispatched` before its effect is
/// asked for, then an outcome or `Unknown`. Marking an operation with the
/// status it has changes nothing.
pub fn mark_operation(
    job: &ManagedState,
    operation: OperationId,
    mark: OperationMark,
) -> Result<Transition, DomainError> {
    let op = job
        .operation(operation)
        .ok_or(DomainError::UnknownOperation { operation })?;
    // A grant is dispatched only while its attempt is the current,
    // untouched grant; replay keeps the plain status rule, so stored
    // histories are read as before.
    if mark == OperationMark::Dispatched
        && op.kind == CommandKind::StartAttempt
        && op.status == OperationStatus::Accepted
        && !job.grant_may_dispatch(operation)
    {
        return Err(DomainError::OperationState {
            operation,
            to: mark,
        });
    }
    match mark_allowed(&op.status, mark) {
        Ok(false) => Ok(unchanged(job)),
        Ok(true) => Ok(Transition {
            revision: next_revision(job)?,
            events: vec![ManagedEvent::OperationMarked {
                operation,
                status: mark,
            }],
        }),
        Err(()) => Err(DomainError::OperationState {
            operation,
            to: mark,
        }),
    }
}

fn broken(reason: impl Into<String>) -> DomainError {
    DomainError::Broken {
        reason: reason.into(),
    }
}

/// Folds one committed batch of events, all at `revision`, into the job's state.
///
/// `state` is `None` only for the batch that creates the job. Every
/// structural rule is checked again here, so a stored history that breaks
/// one does not replay.
pub fn apply_batch(
    state: Option<ManagedState>,
    revision: WorkRevision,
    events: &[ManagedEvent],
) -> Result<ManagedState, DomainError> {
    let expected = match &state {
        None => WorkRevision::FIRST,
        Some(s) => next_revision(s)?,
    };
    if revision != expected {
        return Err(broken(format!(
            "a batch at revision {revision} follows revision {}",
            expected.get() - 1
        )));
    }
    if events.is_empty() {
        return Err(broken("an empty batch"));
    }
    let mut state = state;
    let mut pending: Option<OperationProgress> = None;
    for event in events {
        if let ManagedEvent::CommandAccepted {
            operation,
            key,
            digest,
            kind,
        } = event
        {
            let progress = OperationProgress {
                id: *operation,
                key: key.clone(),
                digest: *digest,
                kind: *kind,
                revision,
                status: if kind.has_effect() {
                    OperationStatus::Accepted
                } else {
                    OperationStatus::Succeeded
                },
            };
            match state.as_mut() {
                Some(s) => {
                    if s.operation(*operation).is_some() {
                        return Err(broken(format!("operation {operation} accepted twice")));
                    }
                    s.operations.push(progress);
                }
                None if pending.is_none() && *kind == CommandKind::CreateJob => {
                    pending = Some(progress);
                }
                None => return Err(broken("a command before its job exists")),
            }
            continue;
        }
        if let ManagedEvent::JobCreated {
            job,
            intent,
            source,
            policy,
            primary,
        } = event
        {
            if state.is_some() {
                return Err(broken(format!("job {job} created twice")));
            }
            let Some(op) = pending.take() else {
                return Err(broken("a job created by no command"));
            };
            state = Some(ManagedState {
                job: ManagedJob {
                    id: *job,
                    revision,
                    generation: Generation::FIRST,
                    intent: intent.clone(),
                    source: source.clone(),
                    policy: *policy,
                    state: JobState::Active,
                    primary: *primary,
                },
                tasks: Vec::new(),
                attempts: Vec::new(),
                operations: vec![op],
            });
            continue;
        }
        let Some(s) = state.as_mut() else {
            return Err(broken(format!("{} before the job exists", event.name())));
        };
        apply_one(s, event)?;
    }
    if pending.is_some() {
        return Err(broken("a create command that made no job"));
    }
    let mut state = state.ok_or_else(|| broken("a batch that made no job"))?;
    state.job.revision = revision;
    Ok(state)
}

#[allow(
    clippy::too_many_lines,
    reason = "one arm per event kind reads better than a helper per arm"
)]
fn apply_one(s: &mut ManagedState, event: &ManagedEvent) -> Result<(), DomainError> {
    let job = s.job.id;
    match event {
        ManagedEvent::CommandAccepted { .. } | ManagedEvent::JobCreated { .. } => {
            return Err(broken("handled by apply_batch"));
        }
        ManagedEvent::TaskRegistered {
            task,
            parent,
            kind,
            restart,
        } => {
            if s.task(*task).is_some() {
                return Err(broken(format!("task {task} registered twice")));
            }
            match (kind, parent) {
                (TaskKind::Primary, None) if *task == s.job.primary && s.tasks.is_empty() => {}
                (TaskKind::Child { .. }, Some(p)) if s.task(*p).is_some() => {}
                _ => return Err(broken(format!("task {task} has the wrong kind or parent"))),
            }
            s.tasks.push(Task {
                id: *task,
                job,
                parent: *parent,
                kind: kind.clone(),
                restart: *restart,
                state: TaskState::Open,
            });
        }
        ManagedEvent::AttemptGranted {
            attempt,
            task,
            resource,
            generation,
            initial_checkpoint,
            operation,
        } => {
            if s.attempt(*attempt).is_some() {
                return Err(broken(format!("attempt {attempt} granted twice")));
            }
            if s.task(*task).map(|t| t.state) != Some(TaskState::Open) {
                return Err(broken(format!(
                    "a grant for task {task}, which is not open"
                )));
            }
            if s.live_attempt().is_some() {
                return Err(broken("a grant while an attempt may still run"));
            }
            if *generation != next_generation(s)? {
                return Err(broken(format!("a grant at generation {generation}")));
            }
            if s.operation(*operation).map(|o| o.kind) != Some(CommandKind::StartAttempt) {
                return Err(broken(format!("a grant by operation {operation}")));
            }
            s.attempts.push(Attempt {
                id: *attempt,
                task: *task,
                job,
                resource: resource.clone(),
                generation: *generation,
                initial_checkpoint: *initial_checkpoint,
                granted_by: *operation,
                status: AttemptStatus::Granted,
            });
            s.job.generation = *generation;
        }
        ManagedEvent::AttemptObserved {
            attempt,
            generation,
            status,
        } => {
            let found = s
                .attempts
                .iter_mut()
                .find(|a| a.id == *attempt)
                .ok_or_else(|| broken(format!("an observation of attempt {attempt}")))?;
            if found.generation != *generation
                || !found.status.may_run()
                || *status == AttemptStatus::Granted
            {
                return Err(broken(format!(
                    "attempt {attempt} cannot become {status:?}"
                )));
            }
            found.status = status.clone();
        }
        ManagedEvent::TaskFinished { task, outcome } => {
            let primary = s.job.primary;
            let found = s
                .tasks
                .iter_mut()
                .find(|t| t.id == *task)
                .ok_or_else(|| broken(format!("task {task} finished before it existed")))?;
            if found.state != TaskState::Open {
                return Err(broken(format!("task {task} finished twice")));
            }
            found.state = outcome.state();
            if *task == primary {
                s.job.state = JobState::Finished;
            }
        }
        ManagedEvent::JobCancelled { .. } => {
            if s.job.state != JobState::Active {
                return Err(broken("a closed job cancelled"));
            }
            s.job.state = JobState::Cancelled;
            for task in &mut s.tasks {
                if task.state == TaskState::Open {
                    task.state = TaskState::Cancelled;
                }
            }
        }
        ManagedEvent::StaleObservationRefused {
            generation,
            current,
            ..
        } => {
            if generation >= current {
                return Err(broken("a refused observation that was not stale"));
            }
        }
        ManagedEvent::OperationMarked { operation, status } => {
            let found = s
                .operations
                .iter_mut()
                .find(|o| o.id == *operation)
                .ok_or_else(|| broken(format!("a mark of operation {operation}")))?;
            if mark_allowed(&found.status, *status) != Ok(true) {
                return Err(broken(format!(
                    "operation {operation} cannot become {status:?}"
                )));
            }
            found.status = status.status();
        }
    }
    Ok(())
}

/// Rebuilds a job's state from its whole history, batch by batch.
///
/// Consecutive events with one revision are one batch; revisions start at
/// one and grow by one. Every event must belong to `job`, which the
/// history's first batch creates.
pub fn replay(
    job: JobId,
    events: &[(WorkRevision, ManagedEvent)],
) -> Result<ManagedState, DomainError> {
    let mut state: Option<ManagedState> = None;
    let mut start = 0;
    while start < events.len() {
        let revision = events[start].0;
        let end = events[start..]
            .iter()
            .position(|(r, _)| *r != revision)
            .map_or(events.len(), |n| start + n);
        let batch: Vec<ManagedEvent> = events[start..end].iter().map(|(_, e)| e.clone()).collect();
        state = Some(apply_batch(state, revision, &batch)?);
        start = end;
    }
    let state = state.ok_or_else(|| broken(format!("job {job} has no events")))?;
    if state.job.id != job {
        return Err(broken(format!(
            "the history of {job} creates {}",
            state.job.id
        )));
    }
    Ok(state)
}
