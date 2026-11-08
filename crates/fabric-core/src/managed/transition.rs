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
use super::lifecycle::{CheckpointCut, OutcomePolicy, TaskClass};
use super::model::{
    Attempt, AttemptStatus, Checkpoint, CheckpointAck, Command, CommandKind, Dedupe, DomainError,
    EffectRequest, Envelope, ExitOutcome, JobState, ManagedEvent, ManagedJob, ManagedState, NewIds,
    Observation, Operation, OperationMark, OperationProgress, OperationStatus, RecoveryBlock,
    RecoveryPolicy, ResourceRef, SourceBinding, Task, TaskKind, TaskOutcome, TaskState, Transition,
};
use super::package::{Fidelity, TEXT_MAX_BYTES};
use crate::credential::looks_like_credential;
use crate::id::{JobId, SnapshotId};

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
        // The primary task is a harness task (`TaskClass::Harness` below).
        admit_recovery(policy.recovery, TaskClass::Harness.outcome_policy())?;
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
                    program: None,
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

/// `RestartFromAcknowledged` is admitted only for a primary task the
/// authority may restart by itself: never an `opaque` one (ADR 0028).
pub fn admit_recovery(recovery: RecoveryPolicy, outcome: OutcomePolicy) -> Result<(), DomainError> {
    match (recovery, outcome) {
        (RecoveryPolicy::RestartFromAcknowledged, OutcomePolicy::Block) => {
            Err(DomainError::RecoveryUnsupported {
                kind: outcome.name().to_owned(),
            })
        }
        _ => Ok(()),
    }
}

/// The snapshot a `StartAttempt` resumes from. The first attempt may start
/// from any snapshot the operator names (M2). Once the job has had an
/// attempt, a start resumes only from a checkpoint the authority
/// acknowledged: a captured one, or one the job never saw, restarts
/// nothing (ADR 0028), the same gate `Recover` runs.
fn resume_point(state: &ManagedState, from: Option<SnapshotId>) -> Result<(), DomainError> {
    if let Some(snapshot) = from
        && !state.attempts.is_empty()
        && !state.acknowledged().iter().any(|a| a.snapshot == snapshot)
    {
        return Err(DomainError::NotAcknowledged { snapshot });
    }
    Ok(())
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

/// Refuses a checkpoint or a continue while one is in flight: dispatched,
/// or accepted and not yet dispatched, so they never pile up. An accepted
/// checkpoint of an attempt that has ended since has nothing left to do
/// and blocks nothing.
fn no_effect_in_flight(state: &ManagedState) -> Result<(), DomainError> {
    let waiting = state.operations.iter().find(|op| match op.kind {
        CommandKind::Continue => matches!(
            op.status,
            OperationStatus::Accepted | OperationStatus::Dispatched
        ),
        CommandKind::Checkpoint => match op.status {
            OperationStatus::Dispatched => true,
            OperationStatus::Accepted => match state.request(op.id) {
                Some(EffectRequest::Checkpoint { attempt }) => {
                    state.attempt(*attempt).is_some_and(|a| a.status.may_run())
                }
                _ => false,
            },
            _ => false,
        },
        _ => false,
    });
    match waiting {
        Some(op) => Err(DomainError::EffectInFlight { operation: op.id }),
        None => Ok(()),
    }
}

/// The dispatched checkpoint command of `attempt`, if one is.
fn checkpoint_of(job: &ManagedState, attempt: AttemptId) -> Option<&OperationProgress> {
    job.in_flight(CommandKind::Checkpoint).filter(|op| {
        matches!(
            job.request(op.id),
            Some(EffectRequest::Checkpoint { attempt: a }) if *a == attempt
        )
    })
}

/// Every open task of an attempt that runs `task`: the task and its open
/// descendants, each of which a cut must have a receipt for (ADR 0023).
fn open_tasks_of(job: &ManagedState, task: TaskId) -> Vec<TaskId> {
    let mut out = Vec::new();
    if job.task(task).is_some_and(|t| t.state == TaskState::Open) {
        out.push(task);
    }
    let mut next = 0;
    while next < out.len() {
        let parent = out[next];
        out.extend(
            job.tasks
                .iter()
                .filter(|t| t.parent == Some(parent) && t.state == TaskState::Open)
                .map(|t| t.id),
        );
        next += 1;
    }
    out
}

/// Refuses a cut that misses a receipt for an open task of the attempt.
fn cut_covers(
    job: &ManagedState,
    attempt: &Attempt,
    cut: &CheckpointCut,
) -> Result<(), DomainError> {
    match open_tasks_of(job, attempt.task)
        .into_iter()
        .find(|task| !cut.receipts.iter().any(|r| r.task == *task))
    {
        Some(task) => Err(DomainError::MissingReceipt { task }),
        None => Ok(()),
    }
}

/// The marks that settle `op` as `mark`: an operation never dispatched
/// is marked dispatched first, as an explicit grant settles a create.
fn settle(op: &OperationProgress, mark: OperationMark) -> Vec<ManagedEvent> {
    let mut events = Vec::with_capacity(2);
    if op.status == OperationStatus::Accepted {
        events.push(ManagedEvent::OperationMarked {
            operation: op.id,
            status: OperationMark::Dispatched,
        });
    }
    events.push(ManagedEvent::OperationMarked {
        operation: op.id,
        status: mark,
    });
    events
}

/// The effect operations of `old` that are not settled: a checkpoint of
/// it, a continue or a cancel of its task, a spawn of an open task under
/// its task, or a cancel of the job.
fn open_effects<'a>(
    state: &'a ManagedState,
    old: &'a Attempt,
) -> impl Iterator<Item = (&'a OperationProgress, &'a EffectRequest)> + 'a {
    state
        .operations
        .iter()
        .filter(|o| {
            matches!(
                o.status,
                OperationStatus::Accepted | OperationStatus::Dispatched | OperationStatus::Unknown
            )
        })
        .filter_map(move |o| {
            let request = state.request(o.id)?;
            let mine = match request {
                EffectRequest::Checkpoint { attempt } => *attempt == old.id,
                EffectRequest::Continue { task, .. }
                | EffectRequest::CancelTask { task, .. }
                | EffectRequest::Recover { task, .. } => *task == old.task,
                EffectRequest::CancelJob { .. } => true,
                // A spawn under the attempt's task is the attempt's to run.
                EffectRequest::Spawn { task } => open_tasks_of(state, old.task).contains(task),
            };
            mine.then_some((o, request))
        })
}

/// What a supersede of `old` does to its open effects (ADR 0028): a
/// checkpoint of it, a continue of its task or a spawn under it can no
/// longer be reported by it and fails; a cancel it was asked to obey is done, because the
/// superseded attempt can write nothing more. A `Recover` is refused while
/// a cancel is open (`EffectInFlight`): no work is granted again on a job
/// or task someone asked to stop.
fn settle_superseded(
    state: &ManagedState,
    old: &Attempt,
    regrant: bool,
) -> Result<Vec<ManagedEvent>, DomainError> {
    let mut events = Vec::new();
    for (op, request) in open_effects(state, old) {
        match request {
            EffectRequest::Checkpoint { .. }
            | EffectRequest::Continue { .. }
            | EffectRequest::Spawn { .. }
            | EffectRequest::Recover { .. } => {
                events.extend(settle(op, OperationMark::Failed));
            }
            EffectRequest::CancelJob { .. } | EffectRequest::CancelTask { .. } if regrant => {
                return Err(DomainError::EffectInFlight { operation: op.id });
            }
            EffectRequest::CancelJob { reason } => {
                events.push(ManagedEvent::JobCancelled {
                    reason: reason.clone(),
                });
                events.extend(settle(op, OperationMark::Succeeded));
            }
            EffectRequest::CancelTask { task, .. } => {
                events.push(ManagedEvent::TaskFinished {
                    task: *task,
                    outcome: TaskOutcome::Cancelled,
                });
                events.extend(settle(op, OperationMark::Succeeded));
            }
        }
    }
    Ok(events)
}

/// The `unknown` live attempt a `Supersede` or `Recover` names; a
/// reachable one is handed over, never recovered (ADR 0028).
fn unknown_attempt(state: &ManagedState, attempt: AttemptId) -> Result<&Attempt, DomainError> {
    let found = state
        .attempt(attempt)
        .ok_or(DomainError::UnknownAttempt { attempt })?;
    if found.status != AttemptStatus::Unknown || found.generation != state.job.generation {
        return Err(DomainError::AttemptReachable {
            attempt,
            status: found.status.clone(),
        });
    }
    Ok(found)
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
        Command::Supersede { attempt, reason } => {
            let old = unknown_attempt(state, *attempt)?;
            let mut events = vec![ManagedEvent::GrantSuperseded {
                attempt: old.id,
                generation: old.generation,
                by: None,
                reason: reason.clone(),
            }];
            events.extend(settle_superseded(state, old, false)?);
            (false, events)
        }
        Command::Recover {
            attempt,
            from,
            reason,
        } => {
            let old = unknown_attempt(state, *attempt)?;
            if !state.acknowledged().iter().any(|a| a.snapshot == *from) {
                return Err(DomainError::NotAcknowledged { snapshot: *from });
            }
            open_task(state, old.task)?;
            let ResourceRef::Node { node: lost } = old.resource;
            let settled = settle_superseded(state, old, true)?;
            let mut events = vec![ManagedEvent::GrantSuperseded {
                attempt: old.id,
                generation: old.generation,
                by: None,
                reason: reason.clone(),
            }];
            events.extend(settled);
            // The grant follows the failover placement, as a continue's
            // does (`place`): the dispatch places the task with `lost`
            // excluded and the domain grants from `from`.
            events.push(ManagedEvent::EffectRequested {
                operation: ids.operation,
                request: EffectRequest::Recover {
                    task: old.task,
                    from: *from,
                    lost,
                },
            });
            return Ok((true, events));
        }
        Command::StartAttempt {
            task,
            resource,
            initial_checkpoint,
        } => {
            open_task(state, *task)?;
            if let Some(live) = state.live_attempt() {
                return Err(DomainError::AttemptStillLive { attempt: live.id });
            }
            resume_point(state, *initial_checkpoint)?;
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
            program,
        } => {
            if state.task(*parent).is_none() || *kind == TaskKind::Primary {
                return Err(DomainError::BadParent { task: *parent });
            }
            open_task(state, *parent)?;
            // A build or test task runs a program; a harness or opaque one
            // is started by its harness or not at all (the launch path).
            match (class, program) {
                (TaskClass::Build | TaskClass::Test, None) => {
                    return Err(DomainError::ProgramRequired { class: *class });
                }
                (TaskClass::Harness | TaskClass::Opaque, Some(_)) => {
                    return Err(DomainError::ProgramNotAllowed { class: *class });
                }
                (_, Some(program)) => program
                    .check()
                    .map_err(|error| DomainError::BadProgram { error })?,
                (_, None) => {}
            }
            let registered = ManagedEvent::TaskRegistered {
                task: ids.task,
                parent: Some(*parent),
                kind: kind.clone(),
                restart: *restart,
                class: *class,
                program: program.clone(),
            };
            // A program registered while its parent's attempt runs is
            // spawned there; with none live it rides the next launch.
            let live = state.attempts.iter().any(|a| {
                a.task == *parent
                    && matches!(a.status, AttemptStatus::Granted | AttemptStatus::Running)
            });
            if program.is_some() && live {
                (
                    true,
                    vec![
                        registered,
                        ManagedEvent::EffectRequested {
                            operation: ids.operation,
                            request: EffectRequest::Spawn { task: ids.task },
                        },
                    ],
                )
            } else {
                (false, vec![registered])
            }
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
        Observation::Captured {
            attempt,
            generation,
            snapshot,
            cut,
        } => return captured(job, *attempt, *generation, *snapshot, *cut),
        Observation::ResourceUnavailable {
            attempt,
            generation,
            last_heartbeat_ms,
            at_ms,
        } => return unavailable(job, *attempt, *generation, *last_heartbeat_ms, *at_ms),
        Observation::ResourceReachable {
            attempt,
            generation,
        } => return reachable(job, *attempt, *generation),
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
            // The cut is the job's captured checkpoint (ADR 0028): the
            // snapshot under the cut's own digest, the one the verifier
            // acknowledges once the authority holds the snapshot's bytes.
            // Recorded once per snapshot; a cut of a snapshot already
            // captured by another attempt or under another digest is the
            // `Captured` rule's refusal.
            let capture = match job.checkpoint(*snapshot) {
                Some(known) if known.attempt == *attempt && known.cut == cut.digest() => None,
                Some(_) => {
                    return Err(DomainError::UnknownCheckpoint {
                        attempt: *attempt,
                        snapshot: *snapshot,
                    });
                }
                None => Some(ManagedEvent::CheckpointCaptured {
                    attempt: *attempt,
                    generation: *generation,
                    snapshot: *snapshot,
                    cut: cut.digest(),
                }),
            };
            if let Some(command) = checkpoint_of(job, *attempt) {
                // This attempt's checkpoint command: the cut is recorded,
                // the attempt goes on.
                if !found.status.may_run() {
                    return Err(DomainError::AttemptEnded { attempt: *attempt });
                }
                cut_covers(job, found, cut)?;
                let mut events = vec![recorded];
                events.extend(capture);
                events.push(ManagedEvent::OperationMarked {
                    operation: command.id,
                    status: OperationMark::Succeeded,
                });
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
            cut_covers(job, found, cut)?;
            // Captured while the attempt may still capture, then ended at the cut.
            let mut events: Vec<ManagedEvent> = capture.into_iter().collect();
            events.push(ManagedEvent::AttemptObserved {
                attempt: *attempt,
                generation: *generation,
                status: ended,
            });
            events.push(recorded);
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
            events.extend(
                checkpoint_of(job, *attempt).map(|op| ManagedEvent::OperationMarked {
                    operation: op.id,
                    status: OperationMark::Failed,
                }),
            );
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
            if *achieved == Fidelity::NewNativeSessionWithPackage && package.is_none() {
                return Err(DomainError::MissingPackage { attempt: *attempt });
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
        } => (*attempt, *generation, exit_status(*outcome)),
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
    // A continue's grant that ends before `Continued` fails the continue:
    // the target offered no allowed fidelity or could not start, and the
    // task stays open for another one.
    if matches!(observation, Observation::Exited { .. })
        && let Some(grant) = job.operation(attempt.granted_by)
        && grant.kind == CommandKind::Continue
        && matches!(
            grant.status,
            OperationStatus::Dispatched | OperationStatus::Unknown
        )
    {
        events.push(ManagedEvent::OperationMarked {
            operation: grant.id,
            status: OperationMark::Failed,
        });
    }
    // An attempt that ended before its checkpoint command was cut fails
    // the command, so it never waits on, or claims, another attempt's cut.
    if !status.may_run()
        && let Some(op) = checkpoint_of(job, attempt_id)
    {
        events.push(ManagedEvent::OperationMarked {
            operation: op.id,
            status: OperationMark::Failed,
        });
    }
    // A cancel in flight is settled by the attempt's end, whatever ended
    // it: the cancel's stop, the harness exiting on its own first, or a
    // start that failed before the stop was taken. Nothing of the attempt
    // runs any more, so the job or the task ends as the cancel asked.
    if matches!(observation, Observation::Exited { .. }) {
        if let Some(op) = cancel_in_flight(job, CommandKind::CancelJob)
            && let Some(EffectRequest::CancelJob { reason }) = job.request(op.id)
        {
            events.push(ManagedEvent::JobCancelled {
                reason: reason.clone(),
            });
            events.push(ManagedEvent::OperationMarked {
                operation: op.id,
                status: OperationMark::Succeeded,
            });
        } else if let Some(op) = cancel_in_flight(job, CommandKind::CancelTask)
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

/// The cancel of `kind` whose stop was sent: dispatched, or of unknown
/// outcome (its node was not reached, or a restart came between).
fn cancel_in_flight(job: &ManagedState, kind: CommandKind) -> Option<&OperationProgress> {
    job.operations.iter().find(|o| {
        o.kind == kind
            && matches!(
                o.status,
                OperationStatus::Dispatched | OperationStatus::Unknown
            )
    })
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
    if !matches!(
        op.kind,
        CommandKind::CreateJob | CommandKind::Continue | CommandKind::Recover
    ) || !matches!(
        op.status,
        OperationStatus::Accepted | OperationStatus::Dispatched
    ) || job.attempts.iter().any(|a| a.granted_by == operation)
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
    let recover = match job.request(operation) {
        Some(EffectRequest::Recover { from, lost, .. }) if op.kind == CommandKind::Recover => {
            Some((*from, *lost))
        }
        _ => None,
    };
    if let (Some((_, lost)), Some(chosen)) = (recover, decision.chosen)
        && chosen == lost
    {
        // A failover never goes back to the node whose attempt it
        // replaces: that attempt may still run there (ADR 0028).
        return Err(DomainError::FailoverToLostNode { node: lost });
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
            let initial_checkpoint = match (op.kind, recover) {
                (CommandKind::Recover, Some((from, _))) => Some(from),
                (CommandKind::Continue, _) => last_checkpoint(job, task),
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

fn exit_status(outcome: ExitOutcome) -> AttemptStatus {
    match outcome {
        ExitOutcome::Succeeded => AttemptStatus::Succeeded,
        ExitOutcome::Failed { .. } => AttemptStatus::Failed,
        ExitOutcome::Cancelled => AttemptStatus::Cancelled,
    }
}

/// `check_generation` as a transition: `Ok(Some(..))` is the refusal to
/// record; `Ok(None)` means the report is current.
fn generation_check(
    job: &ManagedState,
    attempt: &Attempt,
    generation: Generation,
) -> Result<Option<Transition>, DomainError> {
    match check_generation(job, attempt, generation)? {
        Some(events) => Ok(Some(Transition {
            revision: next_revision(job)?,
            events,
        })),
        None => Ok(None),
    }
}

fn one(job: &ManagedState, event: ManagedEvent) -> Result<Transition, DomainError> {
    Ok(Transition {
        revision: next_revision(job)?,
        events: vec![event],
    })
}

/// `Captured`: the attempt keeps running; the checkpoint is recorded as
/// captured, a restart point for nothing until acknowledged (ADR 0028).
fn captured(
    job: &ManagedState,
    attempt_id: AttemptId,
    generation: Generation,
    snapshot: SnapshotId,
    cut: super::lifecycle::CutDigest,
) -> Result<Transition, DomainError> {
    let attempt = job.attempt(attempt_id).ok_or(DomainError::UnknownAttempt {
        attempt: attempt_id,
    })?;
    if let Some(refused) = generation_check(job, attempt, generation)? {
        return Ok(refused);
    }
    if let Some(known) = job.checkpoint(snapshot) {
        if known.attempt == attempt_id && known.cut == cut {
            return Ok(unchanged(job));
        }
        return Err(DomainError::UnknownCheckpoint {
            attempt: attempt_id,
            snapshot,
        });
    }
    if !attempt.status.may_run() {
        return Err(DomainError::AttemptEnded {
            attempt: attempt_id,
        });
    }
    one(
        job,
        ManagedEvent::CheckpointCaptured {
            attempt: attempt_id,
            generation,
            snapshot,
            cut,
        },
    )
}

/// `ResourceUnavailable`: `granted` or `running` becomes `unknown`; an
/// attempt already `unknown` is unchanged; an ended one is refused.
fn unavailable(
    job: &ManagedState,
    attempt_id: AttemptId,
    generation: Generation,
    last_heartbeat_ms: Option<u64>,
    at_ms: u64,
) -> Result<Transition, DomainError> {
    let attempt = job.attempt(attempt_id).ok_or(DomainError::UnknownAttempt {
        attempt: attempt_id,
    })?;
    if let Some(refused) = generation_check(job, attempt, generation)? {
        return Ok(refused);
    }
    match attempt.status {
        AttemptStatus::Granted | AttemptStatus::Running => one(
            job,
            ManagedEvent::AttemptUnavailable {
                attempt: attempt_id,
                generation,
                last_heartbeat_ms,
                since_ms: at_ms,
            },
        ),
        AttemptStatus::Unknown => Ok(unchanged(job)),
        _ => Err(DomainError::AttemptEnded {
            attempt: attempt_id,
        }),
    }
}

/// `ResourceReachable`: only an attempt `AttemptUnavailable` made
/// `unknown` returns, to `running` if its grant succeeded and to `granted`
/// if it was never seen launched; any other `unknown` is unchanged.
fn reachable(
    job: &ManagedState,
    attempt_id: AttemptId,
    generation: Generation,
) -> Result<Transition, DomainError> {
    let attempt = job.attempt(attempt_id).ok_or(DomainError::UnknownAttempt {
        attempt: attempt_id,
    })?;
    if let Some(refused) = generation_check(job, attempt, generation)? {
        return Ok(refused);
    }
    match attempt.status {
        AttemptStatus::Unknown if attempt.unavailable_since_ms.is_some() => one(
            job,
            ManagedEvent::AttemptReachable {
                attempt: attempt_id,
                generation,
            },
        ),
        AttemptStatus::Granted | AttemptStatus::Running | AttemptStatus::Unknown => {
            Ok(unchanged(job))
        }
        _ => Err(DomainError::AttemptEnded {
            attempt: attempt_id,
        }),
    }
}

/// The authority's acceptance of a checkpoint it fetched and verified
/// (ADR 0028): the fourth rule function. It refuses a snapshot the attempt
/// never reported, a cut that differs from the reported one and a
/// generation that is not the attempt's. It accepts an acknowledgement
/// for an attempt in any status while its grant is the job's current
/// generation: the bytes of a checkpoint an attempt made before it became
/// `unknown`, `superseded` with nothing granted, or `lost` are still its
/// checkpoint. Once a later generation is granted the acknowledgement is
/// `StaleGeneration`, as the fence answers the old grant's covered
/// writes: a verifier that finished after a regrant never makes the
/// superseded attempt's bundle a restart point beside the new lineage.
/// The same acknowledgement twice changes nothing.
pub fn acknowledge(job: &ManagedState, ack: &CheckpointAck) -> Result<Transition, DomainError> {
    let attempt = job
        .attempt(ack.attempt)
        .ok_or(DomainError::UnknownAttempt {
            attempt: ack.attempt,
        })?;
    if attempt.generation != ack.generation {
        return Err(DomainError::StaleGeneration {
            given: ack.generation,
            current: attempt.generation,
        });
    }
    let known = job
        .checkpoint(ack.snapshot)
        .filter(|c| c.attempt == ack.attempt)
        .ok_or(DomainError::UnknownCheckpoint {
            attempt: ack.attempt,
            snapshot: ack.snapshot,
        })?;
    if known.cut != ack.cut {
        return Err(DomainError::CutMismatch {
            snapshot: ack.snapshot,
            reported: known.cut,
            verified: ack.cut,
        });
    }
    if known.ack.is_some() {
        return Ok(unchanged(job));
    }
    if matches!(attempt.status, AttemptStatus::Superseded { .. })
        && open_recover_of(job, attempt.task)
    {
        // A recover took its grant and its failover is still being
        // placed: the job's generation does not say so yet, and the new
        // lineage starts from the recover's own checkpoint.
        return Err(DomainError::NotLive {
            attempt: ack.attempt,
        });
    }
    if ack.generation != job.job.generation {
        return Err(DomainError::StaleGeneration {
            given: ack.generation,
            current: job.job.generation,
        });
    }
    one(
        job,
        ManagedEvent::CheckpointAcknowledged { ack: ack.clone() },
    )
}

/// Whether a `Recover` of `task` is accepted or dispatched and has not
/// granted yet.
fn open_recover_of(job: &ManagedState, task: TaskId) -> bool {
    job.operations.iter().any(|o| {
        o.kind == CommandKind::Recover
            && matches!(
                o.status,
                OperationStatus::Accepted | OperationStatus::Dispatched
            )
            && matches!(job.request(o.id), Some(EffectRequest::Recover { task: t, .. }) if *t == task)
            && !job.attempts.iter().any(|a| a.granted_by == o.id)
    })
}

/// The admission rule of `RestartFromAcknowledged`, pure (ADR 0028): given
/// the job, the unavailable attempt, its task's outcome policy and what
/// the authority found, either the `Recover` to submit or why not. A
/// `cancel_and_restart` task restarts from the job's newest acknowledged
/// checkpoint, a `checkpoint` task only from the attempt's own newest one,
/// and a `block` task never.
pub fn recovery_decision(
    state: &ManagedState,
    attempt: AttemptId,
    outcome: OutcomePolicy,
    blockers: &[OperationId],
) -> Result<Command, RecoveryBlock> {
    let from = match outcome {
        OutcomePolicy::Block => {
            return Err(RecoveryBlock::OutcomeBlocks {
                kind: outcome.name().to_owned(),
            });
        }
        OutcomePolicy::CancelAndRestart => state.acknowledged().first().map(|a| a.snapshot),
        OutcomePolicy::Checkpoint => state
            .acknowledged()
            .iter()
            .find(|a| a.attempt == attempt)
            .map(|a| a.snapshot),
    };
    let from = from.ok_or(RecoveryBlock::NoAcknowledgedCheckpoint)?;
    if !blockers.is_empty() {
        return Err(RecoveryBlock::ReplacementBlocked {
            blockers: blockers.to_vec(),
        });
    }
    Ok(Command::Recover {
        attempt,
        from,
        reason: format!("automatic restart from acknowledged checkpoint {from}"),
    })
}

/// Records why the authority did not restart an unavailable attempt by
/// itself (ADR 0028): the job stays under `Block` with the reason in its
/// history. The attempt must be the job's live attempt in status `unknown`.
pub fn record_recovery_block(
    job: &ManagedState,
    attempt: AttemptId,
    reason: RecoveryBlock,
) -> Result<Transition, DomainError> {
    let found = unknown_attempt(job, attempt)?;
    one(
        job,
        ManagedEvent::RecoveryBlocked {
            attempt: found.id,
            generation: found.generation,
            reason,
        },
    )
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

/// The snapshot a continue of `task` starts from: the newest cut of the
/// task's newest attempt that has one, a checkpoint command's cut
/// included, else that attempt's own checkpointed end (an M2 suspend
/// records no cut).
fn last_checkpoint(job: &ManagedState, task: TaskId) -> Option<crate::id::SnapshotId> {
    job.attempts
        .iter()
        .rev()
        .filter(|a| a.task == task)
        .find_map(|a| {
            job.cuts
                .iter()
                .rev()
                .find(|(id, _)| *id == a.id)
                .map(|(_, snapshot)| *snapshot)
                .or(match a.status {
                    AttemptStatus::Checkpointed { snapshot } => Some(snapshot),
                    _ => None,
                })
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
        && op.kind.grants()
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
                checkpoints: Vec::new(),
                requests: Vec::new(),
                cuts: Vec::new(),
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
            program,
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
                program: program.clone(),
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
                Some(
                    CommandKind::StartAttempt
                        | CommandKind::CreateJob
                        | CommandKind::Continue
                        | CommandKind::Recover
                )
            ) {
                return Err(broken(format!("a grant by operation {operation}")));
            }
            // A recover's failover grant is the generation that took the
            // superseded attempt's place: its `by` is named now.
            if s.operation(*operation).map(|o| o.kind) == Some(CommandKind::Recover) {
                let previous = s.job.generation;
                for old in s.attempts.iter_mut().filter(|a| {
                    a.task == *task
                        && a.generation == previous
                        && a.status == AttemptStatus::Superseded { by: None }
                }) {
                    old.status = AttemptStatus::Superseded {
                        by: Some(*generation),
                    };
                }
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
                unavailable_since_ms: None,
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
                || matches!(
                    status,
                    AttemptStatus::Granted | AttemptStatus::Superseded { .. }
                )
            {
                return Err(broken(format!(
                    "attempt {attempt} cannot become {status:?}"
                )));
            }
            found.status = status.clone();
            found.unavailable_since_ms = None;
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
        ManagedEvent::CheckpointCaptured {
            attempt,
            generation,
            snapshot,
            cut,
        } => {
            let found = s
                .attempt(*attempt)
                .ok_or_else(|| broken(format!("a checkpoint of attempt {attempt}")))?;
            if found.generation != *generation || !found.status.may_run() {
                return Err(broken(format!("attempt {attempt} cannot capture")));
            }
            if s.checkpoint(*snapshot).is_some() {
                return Err(broken(format!("checkpoint {snapshot} captured twice")));
            }
            s.checkpoints.push(Checkpoint {
                attempt: *attempt,
                generation: *generation,
                snapshot: *snapshot,
                cut: *cut,
                ack: None,
            });
        }
        ManagedEvent::CheckpointAcknowledged { ack } => {
            let found = s
                .checkpoints
                .iter_mut()
                .find(|c| c.snapshot == ack.snapshot && c.attempt == ack.attempt)
                .ok_or_else(|| broken(format!("an acknowledgement of {}", ack.snapshot)))?;
            if found.generation != ack.generation || found.cut != ack.cut || found.ack.is_some() {
                return Err(broken(format!(
                    "checkpoint {} cannot be acknowledged so",
                    ack.snapshot
                )));
            }
            found.ack = Some(ack.clone());
        }
        ManagedEvent::CoveredWriteRefused { verdict, .. } => {
            if verdict.allows() {
                return Err(broken("a refused covered write with a live verdict"));
            }
        }
        ManagedEvent::AttemptUnavailable {
            attempt,
            generation,
            since_ms,
            ..
        } => {
            let found = s
                .attempts
                .iter_mut()
                .find(|a| a.id == *attempt)
                .ok_or_else(|| broken(format!("an unavailable attempt {attempt}")))?;
            if found.generation != *generation
                || !matches!(
                    found.status,
                    AttemptStatus::Granted | AttemptStatus::Running
                )
            {
                return Err(broken(format!(
                    "attempt {attempt} cannot become unavailable"
                )));
            }
            found.status = AttemptStatus::Unknown;
            found.unavailable_since_ms = Some(*since_ms);
        }
        ManagedEvent::AttemptReachable {
            attempt,
            generation,
        } => {
            // Back to what it was: `running` once it was seen launched (its
            // grant succeeded), `granted` otherwise, so a grant still
            // waiting for its dispatch is dispatched and its `Started`
            // settles it. A continue's grant settles on `Continued`, so an
            // attempt seen started before that returns `granted` until its
            // next report.
            let launched = s
                .attempt(*attempt)
                .and_then(|a| s.operation(a.granted_by))
                .is_some_and(|op| op.status == OperationStatus::Succeeded);
            let found = s
                .attempts
                .iter_mut()
                .find(|a| a.id == *attempt)
                .ok_or_else(|| broken(format!("a reachable attempt {attempt}")))?;
            if found.generation != *generation
                || found.status != AttemptStatus::Unknown
                || found.unavailable_since_ms.is_none()
            {
                return Err(broken(format!("attempt {attempt} cannot become reachable")));
            }
            found.status = if launched {
                AttemptStatus::Running
            } else {
                AttemptStatus::Granted
            };
            found.unavailable_since_ms = None;
        }
        ManagedEvent::GrantSuperseded {
            attempt,
            generation,
            by,
            ..
        } => {
            let current = s.job.generation;
            let found = s
                .attempts
                .iter_mut()
                .find(|a| a.id == *attempt)
                .ok_or_else(|| broken(format!("a superseded attempt {attempt}")))?;
            if found.generation != *generation
                || found.generation != current
                || found.status != AttemptStatus::Unknown
                || by.is_some_and(|by| Some(by) != current.next())
            {
                return Err(broken(format!("attempt {attempt} cannot be superseded")));
            }
            found.status = AttemptStatus::Superseded { by: *by };
            found.unavailable_since_ms = None;
        }
        ManagedEvent::RecoveryBlocked {
            attempt,
            generation,
            ..
        } => {
            let found = s
                .attempt(*attempt)
                .ok_or_else(|| broken(format!("a blocked recovery of attempt {attempt}")))?;
            if found.generation != *generation {
                return Err(broken(format!("a blocked recovery of attempt {attempt}")));
            }
        }
        ManagedEvent::NoteRecorded { .. } => {}
        ManagedEvent::ReceiptsRecorded { attempt, .. } => {
            known_attempt(s, *attempt, "receipts")?;
        }
        ManagedEvent::CheckpointCutRecorded {
            attempt, snapshot, ..
        } => {
            known_attempt(s, *attempt, "a cut")?;
            s.cuts.push((*attempt, *snapshot));
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
