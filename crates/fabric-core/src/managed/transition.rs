//! The rules: which commands and observations a managed job accepts, the
//! events they make, and the one function that folds events into state.
//!
//! Invariant: `accept_command`, `apply_observation` and `mark_operation`
//! only ever return events that `apply_batch` accepts on the same state,
//! and `replay` is `apply_batch` over a stored history, so the state the
//! journal projects after a commit is the state a replay of its events
//! gives. Nothing here reads a clock or makes an id.

use super::frame::request_digest;
use super::ids::{AttemptId, Generation, OperationId, TaskId, WorkRevision};
use super::lifecycle::TaskClass;
use super::model::{
    Attempt, AttemptStatus, Command, CommandKind, Dedupe, DomainError, EffectRequest, Envelope,
    ExitOutcome, JobState, ManagedEvent, ManagedJob, ManagedState, NewIds, Observation, Operation,
    OperationMark, OperationProgress, OperationStatus, ResourceRef, SourceBinding, Task, TaskKind,
    TaskOutcome, TaskState, Transition,
};
use super::package::TEXT_MAX_BYTES;
use crate::credential::looks_like_credential;
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

fn bad_policy(field: &str) -> DomainError {
    DomainError::BadPolicy {
        field: field.to_owned(),
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
    let accepted = |effect: bool| ManagedEvent::CommandAccepted {
        operation: ids.operation,
        key: envelope.key.clone(),
        digest,
        kind,
        effect,
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
        policy.check()?;
        if intent.acceptance.iter().any(|a| a.len() > TEXT_MAX_BYTES) {
            return Err(bad_policy("acceptance"));
        }
        let acp_args: &[String] = match &intent.harness {
            crate::harness::HarnessKind::Acp { args, .. } => args,
            _ => &[],
        };
        if looks_like_credential(&intent.goal)
            || intent.acceptance.iter().any(|a| looks_like_credential(a))
            || acp_args.iter().any(|a| looks_like_credential(a))
            || looks_like_credential(&acp_args.join(" "))
        {
            return Err(DomainError::CredentialLike {
                field: "intent".into(),
            });
        }
        // The create's effect is the initial placement (ADR 0026).
        return Ok(Transition {
            revision: WorkRevision::FIRST,
            events: vec![
                accepted(true),
                ManagedEvent::JobCreated {
                    job: ids.job,
                    intent: intent.clone(),
                    source: source.clone(),
                    policy: policy.clone(),
                    primary: ids.task,
                },
                ManagedEvent::TaskRegistered {
                    task: ids.task,
                    parent: None,
                    kind: TaskKind::Primary,
                    restart: policy.restart,
                    class: TaskClass::Harness,
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
    let (effect, rest) = command_events(state, &envelope.body, ids)?;
    let mut events = vec![accepted(effect)];
    events.extend(rest);
    Ok(Transition { revision, events })
}

/// The live attempt of `task`, if one may still run.
fn live_of(state: &ManagedState, task: TaskId) -> Option<&Attempt> {
    state
        .attempts
        .iter()
        .find(|a| a.task == task && a.status.may_run())
}

/// Refuses a finish or a cancel of a task with an open registered child.
fn no_open_child(state: &ManagedState, task: TaskId) -> Result<(), DomainError> {
    match state
        .tasks
        .iter()
        .find(|t| t.parent == Some(task) && t.state == TaskState::Open)
    {
        Some(child) => Err(DomainError::BadParent { task: child.id }),
        None => Ok(()),
    }
}

/// Refuses a checkpoint or a continue while one is in flight.
fn no_effect_in_flight(state: &ManagedState) -> Result<(), DomainError> {
    for kind in [CommandKind::Checkpoint, CommandKind::Continue] {
        if let Some(op) = state.in_flight(kind) {
            return Err(DomainError::EffectInFlight { operation: op.id });
        }
    }
    Ok(())
}

/// The effect flag and the events a command other than `CreateJob` makes
/// on an active job.
#[allow(
    clippy::too_many_lines,
    reason = "one arm per command kind reads better than a helper per arm"
)]
fn command_events(
    state: &ManagedState,
    body: &Command,
    ids: &NewIds,
) -> Result<(bool, Vec<ManagedEvent>), DomainError> {
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
            let mut events = vec![ManagedEvent::AttemptGranted {
                attempt: ids.attempt,
                task: *task,
                resource: resource.clone(),
                generation: next_generation(state)?,
                initial_checkpoint: *initial_checkpoint,
                operation: ids.operation,
            }];
            // An explicit grant stands in for the initial placement: a
            // create still awaiting it is complete.
            if let Some(create) = state
                .operations
                .iter()
                .find(|o| o.kind == CommandKind::CreateJob && o.status == OperationStatus::Accepted)
            {
                events.push(ManagedEvent::OperationMarked {
                    operation: create.id,
                    status: OperationMark::Dispatched,
                });
                events.push(ManagedEvent::OperationMarked {
                    operation: create.id,
                    status: OperationMark::Succeeded,
                });
            }
            (true, events)
        }
        Command::RegisterTask {
            parent,
            kind,
            restart,
            class,
        } => {
            if state.task(*parent).is_none() || *kind == TaskKind::Primary {
                return Err(DomainError::BadParent { task: *parent });
            }
            open_task(state, *parent)?;
            (
                false,
                vec![ManagedEvent::TaskRegistered {
                    task: ids.task,
                    parent: Some(*parent),
                    kind: kind.clone(),
                    restart: *restart,
                    class: *class,
                }],
            )
        }
        Command::FinishTask { task, outcome } => {
            open_task(state, *task)?;
            if let Some(live) = live_of(state, *task) {
                return Err(DomainError::AttemptStillLive { attempt: live.id });
            }
            no_open_child(state, *task)?;
            (
                false,
                vec![ManagedEvent::TaskFinished {
                    task: *task,
                    outcome: *outcome,
                }],
            )
        }
        Command::CancelJob { reason } => {
            if state.live_attempt().is_some() {
                (
                    true,
                    vec![ManagedEvent::EffectRequested {
                        operation: ids.operation,
                        request: EffectRequest::CancelJob {
                            reason: reason.clone(),
                        },
                    }],
                )
            } else {
                (
                    false,
                    vec![ManagedEvent::JobCancelled {
                        reason: reason.clone(),
                    }],
                )
            }
        }
        Command::CancelTask { task, reason } => {
            open_task(state, *task)?;
            no_open_child(state, *task)?;
            if live_of(state, *task).is_some() {
                (
                    true,
                    vec![ManagedEvent::EffectRequested {
                        operation: ids.operation,
                        request: EffectRequest::CancelTask {
                            task: *task,
                            reason: reason.clone(),
                        },
                    }],
                )
            } else {
                (
                    false,
                    vec![ManagedEvent::TaskFinished {
                        task: *task,
                        outcome: TaskOutcome::Cancelled,
                    }],
                )
            }
        }
        Command::RecordNote { note } => {
            note.check()?;
            (
                false,
                vec![ManagedEvent::NoteRecorded { note: note.clone() }],
            )
        }
        Command::Checkpoint { attempt } => {
            let found = state
                .attempt(*attempt)
                .ok_or(DomainError::UnknownAttempt { attempt: *attempt })?;
            if !matches!(
                found.status,
                AttemptStatus::Granted | AttemptStatus::Running
            ) {
                return Err(DomainError::NotLive { attempt: *attempt });
            }
            no_effect_in_flight(state)?;
            (
                true,
                vec![ManagedEvent::EffectRequested {
                    operation: ids.operation,
                    request: EffectRequest::Checkpoint { attempt: *attempt },
                }],
            )
        }
        Command::Continue {
            task,
            fidelity,
            pin,
        } => {
            open_task(state, *task)?;
            if fidelity.is_empty() {
                return Err(DomainError::FidelityRefused {
                    allowed: *fidelity,
                    offered: Vec::new(),
                });
            }
            if let Some(live) = state.live_attempt()
                && live.task != *task
            {
                return Err(DomainError::AttemptStillLive { attempt: live.id });
            }
            no_effect_in_flight(state)?;
            (
                true,
                vec![ManagedEvent::EffectRequested {
                    operation: ids.operation,
                    request: EffectRequest::Continue {
                        task: *task,
                        fidelity: *fidelity,
                        pin: *pin,
                    },
                }],
            )
        }
    })
}

fn open_task(state: &ManagedState, task: TaskId) -> Result<&Task, DomainError> {
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

/// Checks the generation a report names against the attempt's and the
/// job's. `Ok(Some(events))` is a stale report, recorded as refused.
fn check_generation(
    job: &ManagedState,
    attempt: &Attempt,
    generation: Generation,
) -> Result<Option<Vec<ManagedEvent>>, DomainError> {
    let current = job.job.generation;
    if generation > current {
        return Err(DomainError::StaleGeneration {
            given: generation,
            current,
        });
    }
    if generation < current {
        return Ok(Some(vec![ManagedEvent::StaleObservationRefused {
            attempt: attempt.id,
            generation,
            current,
        }]));
    }
    if attempt.generation != generation {
        return Err(DomainError::StaleGeneration {
            given: generation,
            current: attempt.generation,
        });
    }
    Ok(None)
}

/// The mark that says the grant's dispatch succeeded, once an attempt is
/// seen started or ended; a continue's grant waits for `Continued`.
fn grant_succeeded(job: &ManagedState, attempt: &Attempt) -> Option<ManagedEvent> {
    let grant = job.operation(attempt.granted_by)?;
    (grant.kind != CommandKind::Continue
        && matches!(
            grant.status,
            OperationStatus::Dispatched | OperationStatus::Unknown
        ))
    .then_some(ManagedEvent::OperationMarked {
        operation: grant.id,
        status: OperationMark::Succeeded,
    })
}

/// The marks that move the in-flight operation of `kind` to `mark`.
fn settle_in_flight(
    job: &ManagedState,
    kind: CommandKind,
    mark: OperationMark,
) -> Vec<ManagedEvent> {
    job.in_flight(kind)
        .map(|op| ManagedEvent::OperationMarked {
            operation: op.id,
            status: mark,
        })
        .into_iter()
        .collect()
}

/// Checks a report about an attempt or an operation and returns its events.
///
/// A report that repeats what the job already holds changes nothing. A
/// report at an older generation is recorded as refused and changes
/// nothing else (replay rule 7). A lost worker makes its attempt `Lost`
/// and leaves its task open (rule 5).
#[allow(
    clippy::too_many_lines,
    reason = "one arm per observation kind reads better than a helper per arm"
)]
pub fn apply_observation(
    job: &ManagedState,
    observation: &Observation,
) -> Result<Transition, DomainError> {
    let transition = |events: Vec<ManagedEvent>| -> Result<Transition, DomainError> {
        Ok(Transition {
            revision: next_revision(job)?,
            events,
        })
    };
    let (attempt_id, generation, status) = match observation {
        Observation::Unknown { operation } => {
            return mark_operation(job, *operation, OperationMark::Unknown);
        }
        Observation::Placed {
            operation,
            task,
            decision,
        } => return place(job, *operation, *task, decision),
        Observation::Started {
            attempt,
            generation,
        } => (*attempt, *generation, AttemptStatus::Running),
        Observation::Checkpointed {
            attempt,
            generation,
            snapshot,
            cut,
        } => {
            let found = job
                .attempt(*attempt)
                .ok_or(DomainError::UnknownAttempt { attempt: *attempt })?;
            if let Some(stale) = check_generation(job, found, *generation)? {
                return transition(stale);
            }
            let recorded = ManagedEvent::CheckpointCutRecorded {
                attempt: *attempt,
                snapshot: *snapshot,
                cut: cut.digest(),
                receipts: cut.receipts.clone(),
            };
            if job.in_flight(CommandKind::Checkpoint).is_some() {
                // A checkpoint command: the cut is recorded, the attempt goes on.
                if !found.status.may_run() {
                    return Err(DomainError::AttemptEnded { attempt: *attempt });
                }
                let mut events = vec![recorded];
                events.extend(settle_in_flight(
                    job,
                    CommandKind::Checkpoint,
                    OperationMark::Succeeded,
                ));
                return transition(events);
            }
            let ended = AttemptStatus::Checkpointed {
                snapshot: *snapshot,
            };
            if found.status == ended {
                return Ok(unchanged(job));
            }
            if !found.status.may_run() {
                return Err(DomainError::AttemptEnded { attempt: *attempt });
            }
            let mut events = vec![
                ManagedEvent::AttemptObserved {
                    attempt: *attempt,
                    generation: *generation,
                    status: ended,
                },
                recorded,
            ];
            events.extend(grant_succeeded(job, found));
            return transition(events);
        }
        Observation::Receipts {
            attempt,
            generation,
            receipts,
        } => {
            let found = job
                .attempt(*attempt)
                .ok_or(DomainError::UnknownAttempt { attempt: *attempt })?;
            if let Some(stale) = check_generation(job, found, *generation)? {
                return transition(stale);
            }
            return transition(vec![ManagedEvent::ReceiptsRecorded {
                attempt: *attempt,
                receipts: receipts.clone(),
            }]);
        }
        Observation::HandoffBlocked {
            attempt,
            generation,
            blockers,
        } => {
            let found = job
                .attempt(*attempt)
                .ok_or(DomainError::UnknownAttempt { attempt: *attempt })?;
            if let Some(stale) = check_generation(job, found, *generation)? {
                return transition(stale);
            }
            let mut events = vec![ManagedEvent::HandoffBlocked {
                attempt: *attempt,
                blockers: blockers.clone(),
            }];
            events.extend(settle_in_flight(
                job,
                CommandKind::Checkpoint,
                OperationMark::Failed,
            ));
            events.extend(settle_in_flight(
                job,
                CommandKind::Continue,
                OperationMark::Failed,
            ));
            return transition(events);
        }
        Observation::Continued {
            attempt,
            generation,
            from,
            achieved,
            package,
        } => {
            let found = job
                .attempt(*attempt)
                .ok_or(DomainError::UnknownAttempt { attempt: *attempt })?;
            if let Some(stale) = check_generation(job, found, *generation)? {
                return transition(stale);
            }
            if job.attempt(*from).is_none() || *from == *attempt {
                return Err(DomainError::UnknownAttempt { attempt: *from });
            }
            let requested = match job.request(found.granted_by) {
                Some(EffectRequest::Continue { fidelity, .. }) => *fidelity,
                _ => job.job.policy.allowed_fidelity,
            };
            if !requested.allows(*achieved) {
                return Err(DomainError::FidelityRefused {
                    allowed: requested,
                    offered: vec![*achieved],
                });
            }
            let mut events = vec![ManagedEvent::ContinuationRecorded {
                attempt: *attempt,
                from: *from,
                requested,
                achieved: *achieved,
                package: *package,
            }];
            if let Some(grant) = job.operation(found.granted_by)
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
            return transition(events);
        }
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
    if let Some(stale) = check_generation(job, attempt, generation)? {
        return transition(stale);
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
    if status != AttemptStatus::Lost {
        events.extend(grant_succeeded(job, attempt));
    }
    // A cancel that stopped the attempt now ends the job or the task.
    if status == AttemptStatus::Cancelled {
        if let Some(op) = job.in_flight(CommandKind::CancelJob)
            && let Some(EffectRequest::CancelJob { reason }) = job.request(op.id)
        {
            events.push(ManagedEvent::JobCancelled {
                reason: reason.clone(),
            });
            events.push(ManagedEvent::OperationMarked {
                operation: op.id,
                status: OperationMark::Succeeded,
            });
        } else if let Some(op) = job.in_flight(CommandKind::CancelTask)
            && let Some(EffectRequest::CancelTask { task, .. }) = job.request(op.id)
            && *task == attempt.task
        {
            events.push(ManagedEvent::TaskFinished {
                task: *task,
                outcome: TaskOutcome::Cancelled,
            });
            events.push(ManagedEvent::OperationMarked {
                operation: op.id,
                status: OperationMark::Succeeded,
            });
        }
    }
    transition(events)
}

/// The events of a `Placed` observation: the decision, and the grant on
/// the chosen node or the parking of the job.
fn place(
    job: &ManagedState,
    operation: OperationId,
    task: TaskId,
    decision: &super::placement::PlacementDecision,
) -> Result<Transition, DomainError> {
    let op = job
        .operation(operation)
        .ok_or(DomainError::UnknownOperation { operation })?;
    if !matches!(op.kind, CommandKind::CreateJob | CommandKind::Continue)
        || !matches!(
            op.status,
            OperationStatus::Accepted | OperationStatus::Dispatched
        )
        || job.attempts.iter().any(|a| a.granted_by == operation)
    {
        return Err(DomainError::OperationState {
            operation,
            to: OperationMark::Dispatched,
        });
    }
    open_task(job, task)?;
    if let Some(live) = job.live_attempt() {
        return Err(DomainError::AttemptStillLive { attempt: live.id });
    }
    let mut events = vec![ManagedEvent::PlacementDecided {
        task,
        decision: decision.clone(),
    }];
    if op.status == OperationStatus::Accepted {
        events.push(ManagedEvent::OperationMarked {
            operation,
            status: OperationMark::Dispatched,
        });
    }
    match decision.chosen {
        Some(node) => {
            let initial_checkpoint = match op.kind {
                CommandKind::Continue => last_checkpoint(job, task),
                _ => match job.job.source {
                    SourceBinding::Snapshot { snapshot } => Some(snapshot),
                    _ => None,
                },
            };
            events.push(ManagedEvent::AttemptGranted {
                // The grant's attempt id is derived from the operation and
                // the generation, so a retried placement cannot mint a
                // second id for one grant; the journal never saw this id
                // before because the generation is new.
                attempt: derived_attempt(operation, next_generation(job)?),
                task,
                resource: ResourceRef::Node { node },
                generation: next_generation(job)?,
                initial_checkpoint,
                operation,
            });
        }
        None => events.push(ManagedEvent::OperationMarked {
            operation,
            status: OperationMark::Succeeded,
        }),
    }
    Ok(Transition {
        revision: next_revision(job)?,
        events,
    })
}

/// The id of the attempt a placement grants: a function of the operation
/// and the generation, so the authority needs no fresh id for an observation.
fn derived_attempt(operation: OperationId, generation: Generation) -> AttemptId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fabric.managed.attempt.v1\0");
    hasher.update(&operation.to_bytes());
    hasher.update(&generation.get().to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    AttemptId::from_bytes(bytes)
}

/// The snapshot of the task's latest checkpointed attempt.
fn last_checkpoint(job: &ManagedState, task: TaskId) -> Option<crate::id::SnapshotId> {
    job.attempts
        .iter()
        .rev()
        .find_map(|a| match (a.task == task, &a.status) {
            (true, AttemptStatus::Checkpointed { snapshot }) => Some(*snapshot),
            _ => None,
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
            effect,
        } = event
        {
            let progress = OperationProgress {
                id: *operation,
                key: key.clone(),
                digest: *digest,
                kind: *kind,
                revision,
                status: if *effect {
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
                    policy: policy.clone(),
                    state: JobState::Active,
                    primary: *primary,
                },
                tasks: Vec::new(),
                attempts: Vec::new(),
                operations: vec![op],
                requests: Vec::new(),
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

fn known_attempt(s: &ManagedState, attempt: AttemptId, what: &str) -> Result<(), DomainError> {
    if s.attempt(attempt).is_none() {
        return Err(broken(format!(
            "{what} of attempt {attempt}, which does not exist"
        )));
    }
    Ok(())
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
            class,
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
                class: *class,
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
            if !matches!(
                s.operation(*operation).map(|o| o.kind),
                Some(CommandKind::StartAttempt | CommandKind::CreateJob | CommandKind::Continue)
            ) {
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
        ManagedEvent::NoteRecorded { .. } => {}
        ManagedEvent::ReceiptsRecorded { attempt, .. } => {
            known_attempt(s, *attempt, "receipts")?;
        }
        ManagedEvent::CheckpointCutRecorded { attempt, .. } => {
            known_attempt(s, *attempt, "a cut")?;
        }
        ManagedEvent::HandoffBlocked { attempt, .. } => {
            known_attempt(s, *attempt, "blockers")?;
        }
        ManagedEvent::PlacementDecided { task, .. } => {
            if s.task(*task).is_none() {
                return Err(broken(format!(
                    "a placement of task {task}, which does not exist"
                )));
            }
        }
        ManagedEvent::ContinuationRecorded { attempt, from, .. } => {
            known_attempt(s, *attempt, "a continuation")?;
            known_attempt(s, *from, "a continuation")?;
            if attempt == from {
                return Err(broken(format!("attempt {attempt} continues itself")));
            }
        }
        ManagedEvent::EffectRequested { operation, request } => {
            if s.operation(*operation).is_none() {
                return Err(broken(format!("a request of operation {operation}")));
            }
            if s.request(*operation).is_some() {
                return Err(broken(format!("operation {operation} requested twice")));
            }
            s.requests.push((*operation, request.clone()));
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
