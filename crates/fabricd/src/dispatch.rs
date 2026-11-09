//! The authority's dispatch (ADR 0026 and docs/managed-jobs.md, "Launch
//! path"): every accepted effect is carried to the node it is for, and
//! every grant to the node it names.
//!
//! Invariant: what to do is read anew from the journal after every
//! commit, and each effect is done at most once per process until the
//! journal moves. A create is placed and its decision recorded with the
//! grant it makes in one batch, so it is never placed twice. An M5
//! command is marked `Dispatched` before its instruction is sent, so a
//! crash after the mark never sends it again; a spawn is settled by the
//! node's answer, a checkpoint, a continue's cut and a cancel by the
//! node's report. Two instructions are safe to send again and are: a
//! cancel's stop that did not reach its node or found it busy (a node
//! takes one stop per attempt, and a restart sends each unsettled stop
//! once more), and a spawn its node answered busy or frozen. A stop its
//! node answers `NotRunning` ends the attempt cancelled, since nothing of
//! it runs there, and a grant with a cancel pending is never launched. A grant is launched by sending its plan again after a
//! restart: the node answers a launch of an attempt it already runs
//! `AlreadyLaunched`, so nothing runs twice and the authority keeps no
//! "launch sent" fact. A launch the node refuses for any other reason
//! fails its attempt with the harness receipt `Blocked`, so the task is
//! free for another continue. A create with a repository source is the
//! M2 form and is settled by its explicit `StartAttempt`, as before M5;
//! its grant is the CLI's to start.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::id::JobId;
use fabric_core::managed::launch::{
    AfterCut, InstructAnswer, InstructRefusal, Instruction, LaunchAnswer, LaunchRefusal, TaskLaunch,
};
use fabric_core::managed::placement::{CurrentPlacement, PlacementKind};
use fabric_core::managed::{
    Attempt, AttemptId, AttemptStatus, CommandKind, EffectRequest, ExitOutcome, ManagedState,
    Observation, OperationId, OperationMark, OperationStatus, ReceiptStatus, ResourceRef,
    SourceBinding, TaskClass, TaskId, TaskReceipt, continue_start,
};

use crate::authority::{Authority, AuthorityError, PendingEffect};
use crate::launch::{Link, NodeLink, PlanError, plan, token_of};
use crate::node::Node;
use crate::placement::PlacementAsk;

/// How long the loop waits before it tries a launch again whose node
/// could not be reached, when no commit wakes it first.
const RETRY: Duration = Duration::from_secs(2);

/// What this process did already, so it does each at most once until
/// the journal moves; a restart starts with none.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// Creates whose placement failed; a restart tries them again.
    failed: HashSet<(JobId, OperationId)>,
    /// Continues placed as a planned handoff.
    placed: HashSet<(JobId, OperationId)>,
    /// Grants sent and taken, or refused for good.
    launched: HashSet<AttemptId>,
    /// Cancels whose stop its node took, or answered for good.
    stopped: HashSet<(JobId, OperationId)>,
    /// Cancels whose stop did not reach its node or found it busy.
    unreached: HashSet<(JobId, OperationId)>,
    /// Whether this process swept the journal once for the stops a
    /// previous one left unsettled.
    swept: bool,
    /// Spawns their node answered busy or frozen, sent again later.
    respawn: HashSet<(JobId, OperationId)>,
    /// Whether a launch failed at the link and is worth another try.
    retry: bool,
}

/// Dispatches every accepted effect and launches every grant, now and
/// after every later commit, until the authority's thread ends. It waits
/// without a thread of its own while the authority has none, so a node
/// with no managed job starts no authority thread for it.
pub(crate) async fn run(node: Arc<Node>) {
    let Some(mut commits) = node.authority.started().await else {
        return;
    };
    let link = NodeLink(Arc::clone(&node));
    let mut seen = Seen::default();
    loop {
        commits.borrow_and_update();
        pass(&node.authority, &link, &mut seen).await;
        if std::mem::take(&mut seen.retry) {
            tokio::select! {
                changed = commits.changed() => if changed.is_err() { return },
                () = tokio::time::sleep(RETRY) => {}
            }
        } else if commits.changed().await.is_err() {
            return;
        }
    }
}

/// One pass over the journal: grants are launched first, so an
/// instruction for an attempt finds it on its node, then each accepted
/// effect is dispatched, then each continue whose cut has come is
/// placed, and the grants those made are launched.
pub(crate) async fn pass(authority: &Authority, link: &dyn Link, seen: &mut Seen) {
    launch_grants(authority, link, seen).await;
    resend(authority, link, seen).await;
    match authority.pending().await {
        Ok(effects) => {
            for effect in effects {
                let outcome = match effect.kind {
                    CommandKind::CreateJob | CommandKind::Recover => {
                        if seen.failed.contains(&(effect.job, effect.operation)) {
                            continue;
                        }
                        let placed = if effect.kind == CommandKind::Recover {
                            place_failover(authority, link, &effect).await
                        } else {
                            place_create(authority, link, &effect).await
                        };
                        if placed.is_err() {
                            seen.failed.insert((effect.job, effect.operation));
                        }
                        placed
                    }
                    CommandKind::Checkpoint
                    | CommandKind::Continue
                    | CommandKind::CancelJob
                    | CommandKind::CancelTask
                    | CommandKind::RegisterTask => {
                        dispatch_effect(authority, link, seen, &effect).await
                    }
                    _ => Ok(()),
                };
                if let Err(e) = outcome {
                    tracing::warn!(job = %effect.job, operation = %effect.operation, kind = effect.kind.name(), error = %e, "an effect was not dispatched");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "the pending effects could not be read"),
    }
    place_cut_continues(authority, link, seen).await;
    launch_grants(authority, link, seen).await;
}

/// Places `effect`'s create once and records the decision.
async fn place_create(
    authority: &Authority,
    link: &dyn Link,
    effect: &PendingEffect,
) -> Result<(), AuthorityError> {
    let Some(state) = authority.job(effect.job).await? else {
        return Ok(());
    };
    let snapshot = match &state.job.source {
        SourceBinding::Repository { .. } => return Ok(()),
        SourceBinding::Snapshot { snapshot } => Some(*snapshot),
        SourceBinding::Git { .. } | SourceBinding::Empty => None,
    };
    let task = state.job.primary;
    let ask = PlacementAsk {
        task,
        kind: PlacementKind::Initial,
        required_layout: None,
        current: None,
        snapshot,
    };
    crate::faults::hit(crate::faults::Point::BeforePlacement).await;
    let decision = link.place(&state.job, &ask).await;
    tracing::info!(job = %effect.job, chosen = ?decision.chosen.map(|n| n.to_string()), "placed");
    authority
        .observe_authority(
            effect.job,
            Observation::Placed {
                operation: effect.operation,
                task,
                decision,
            },
        )
        .await?;
    Ok(())
}

/// Places `effect`'s recover once as a failover (ADR 0028) and records the
/// decision: the M5 placement with the superseded attempt's node named as
/// the current placement, not healthy, so it is excluded whatever it
/// answers, and the acknowledged checkpoint as the snapshot to start
/// from. The domain grants from that checkpoint, or the job parks with
/// every exclusion recorded; the grant is then launched like any other.
async fn place_failover(
    authority: &Authority,
    link: &dyn Link,
    effect: &PendingEffect,
) -> Result<(), AuthorityError> {
    let Some(state) = authority.job(effect.job).await? else {
        return Ok(());
    };
    let Some(EffectRequest::Recover { task, from, lost }) =
        state.request(effect.operation).cloned()
    else {
        return Ok(());
    };
    let ask = PlacementAsk {
        task,
        kind: PlacementKind::Failover,
        required_layout: None,
        current: Some(CurrentPlacement {
            node: lost,
            healthy: false,
        }),
        snapshot: Some(from),
    };
    let decision = link.place(&state.job, &ask).await;
    tracing::info!(job = %effect.job, lost = %lost, chosen = ?decision.chosen.map(|n| n.to_string()), "failover placed");
    authority
        .observe_authority(
            effect.job,
            Observation::Placed {
                operation: effect.operation,
                task,
                decision,
            },
        )
        .await?;
    Ok(())
}

/// The live attempt of `task`: granted, running or of unknown fate.
fn live_of(state: &ManagedState, task: TaskId) -> Option<&Attempt> {
    state
        .attempts
        .iter()
        .find(|a| a.task == task && a.status.may_run())
}

/// Places a continue of `task` from the task's restart point
/// (`continue_start`): a planned handoff from its last cut, or, when the
/// task's newest attempt was superseded, a failover with that attempt's
/// node excluded, as a recover is. The domain grants from the same point.
/// A continue with no restart point is failed here, as the domain refused it.
async fn place_handoff(
    authority: &Authority,
    link: &dyn Link,
    state: &ManagedState,
    operation: OperationId,
    task: TaskId,
) -> Result<(), AuthorityError> {
    let start = match continue_start(state, task) {
        Ok(start) => start,
        Err(e) => {
            tracing::warn!(job = %state.job.id, %operation, error = %e, "a continue has no restart point; it fails");
            authority
                .mark_authority(state.job.id, operation, OperationMark::Dispatched)
                .await?;
            authority
                .mark_authority(state.job.id, operation, OperationMark::Failed)
                .await?;
            return Ok(());
        }
    };
    let ask = PlacementAsk {
        task,
        kind: if start.lost.is_some() {
            PlacementKind::Failover
        } else {
            PlacementKind::PlannedHandoff
        },
        required_layout: None,
        current: start.lost.map(|node| CurrentPlacement {
            node,
            healthy: false,
        }),
        snapshot: start.from,
    };
    // A continue's pin names the node the work moves to, in place of the
    // job's own pin; without one the job's policy places it.
    let pin = match state.request(operation) {
        Some(EffectRequest::Continue { pin: Some(pin), .. }) => Some(*pin),
        _ => None,
    };
    let decision = match pin {
        Some(pin) => {
            let mut job = state.job.clone();
            job.policy.placement.pin = Some(pin);
            link.place(&job, &ask).await
        }
        None => link.place(&state.job, &ask).await,
    };
    tracing::info!(job = %state.job.id, chosen = ?decision.chosen.map(|n| n.to_string()), "continue placed");
    authority
        .observe_authority(
            state.job.id,
            Observation::Placed {
                operation,
                task,
                decision,
            },
        )
        .await?;
    Ok(())
}

/// Every continue that is dispatched, whose attempt has ended at its cut
/// and that granted nothing yet, placed once per process.
async fn place_cut_continues(authority: &Authority, link: &dyn Link, seen: &mut Seen) {
    let jobs = match authority.list().await {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!(error = %e, "the jobs could not be read");
            return;
        }
    };
    for state in jobs {
        for op in &state.operations {
            if op.kind != CommandKind::Continue
                || op.status != OperationStatus::Dispatched
                || state.attempts.iter().any(|a| a.granted_by == op.id)
                || seen.placed.contains(&(state.job.id, op.id))
            {
                continue;
            }
            let Some(EffectRequest::Continue { task, .. }) = state.request(op.id) else {
                continue;
            };
            if live_of(&state, *task).is_some() {
                continue;
            }
            seen.placed.insert((state.job.id, op.id));
            if let Err(e) = place_handoff(authority, link, &state, op.id, *task).await {
                tracing::warn!(job = %state.job.id, operation = %op.id, error = %e, "a continue was not placed");
            }
        }
    }
}

/// The attempt an accepted effect is for and what it is told; `None`
/// when the attempt has ended since, or for a continue with none live.
fn instruction_for(
    state: &ManagedState,
    request: &EffectRequest,
) -> Option<(AttemptId, Instruction)> {
    let stop = |reason: &String| Instruction::Stop {
        reason: reason.clone(),
    };
    match request {
        EffectRequest::Checkpoint { attempt } => state
            .attempt(*attempt)
            .filter(|a| a.status.may_run())
            .map(|a| {
                (
                    a.id,
                    Instruction::Checkpoint {
                        then: AfterCut::Resume,
                    },
                )
            }),
        EffectRequest::Continue { task, .. } => live_of(state, *task).map(|a| {
            (
                a.id,
                Instruction::Checkpoint {
                    then: AfterCut::End,
                },
            )
        }),
        EffectRequest::CancelJob { reason } => state.live_attempt().map(|a| (a.id, stop(reason))),
        EffectRequest::CancelTask { task, reason } => {
            live_of(state, *task).map(|a| (a.id, stop(reason)))
        }
        // A recover is placed, never instructed: its old attempt's grant
        // is already taken (`place_failover`).
        EffectRequest::Recover { .. } => None,
        EffectRequest::Spawn { task } => {
            let child = state.task(*task)?;
            let program = child.program.clone()?;
            let parent = child.parent?;
            state
                .attempts
                .iter()
                .find(|a| {
                    a.task == parent
                        && matches!(a.status, AttemptStatus::Granted | AttemptStatus::Running)
                })
                .map(|a| {
                    (
                        a.id,
                        Instruction::Spawn(TaskLaunch {
                            task: child.id,
                            class: child.class,
                            program,
                        }),
                    )
                })
        }
    }
}

/// Dispatches one accepted M5 command: marks it `Dispatched`, then sends
/// its instruction to the node of the attempt it is for. A continue with
/// no live attempt is placed instead; an effect whose attempt has ended
/// since fails. A refusal fails the command; a node that could not be
/// reached leaves it `Unknown`, since the node may have taken it.
async fn dispatch_effect(
    authority: &Authority,
    link: &dyn Link,
    seen: &mut Seen,
    effect: &PendingEffect,
) -> Result<(), AuthorityError> {
    let Some(state) = authority.job(effect.job).await? else {
        return Ok(());
    };
    let Some(request) = state.request(effect.operation).cloned() else {
        return Ok(());
    };
    let Some((attempt, instruction)) = instruction_for(&state, &request) else {
        if let EffectRequest::Continue { task, .. } = request {
            seen.placed.insert((effect.job, effect.operation));
            return place_handoff(authority, link, &state, effect.operation, task).await;
        }
        authority
            .mark_authority(effect.job, effect.operation, OperationMark::Dispatched)
            .await?;
        authority
            .mark_authority(effect.job, effect.operation, OperationMark::Failed)
            .await?;
        return Ok(());
    };
    let (Some(token), Some(found)) = (token_of(&state, attempt), state.attempt(attempt)) else {
        return Ok(());
    };
    let ResourceRef::Node { node: target } = found.resource;
    authority
        .mark_authority(effect.job, effect.operation, OperationMark::Dispatched)
        .await?;
    instruct(
        authority,
        link,
        seen,
        (effect.job, effect.operation),
        &request,
        (target, token),
        instruction,
    )
    .await
}

/// Sends `instruction` for the dispatched operation `key` and acts on the
/// answer. A spawn is settled by it, except a node that is preparing the
/// root or quiescing (`Busy`, `Frozen`): it is sent again later. A stop
/// is settled by the attempt's end; one that did not reach its node, or
/// that its node answered `Busy`, is sent again, which is safe since a
/// node takes one stop per attempt; one its node answers `NotRunning`
/// ends the attempt cancelled here, since nothing of it runs there. Any
/// other refusal fails the command, and a checkpoint that did not reach
/// its node is `Unknown`, since the node may have taken it.
async fn instruct(
    authority: &Authority,
    link: &dyn Link,
    seen: &mut Seen,
    key: (JobId, OperationId),
    request: &EffectRequest,
    (target, token): (
        fabric_core::id::NodeId,
        fabric_core::managed::launch::GrantToken,
    ),
    instruction: Instruction,
) -> Result<(), AuthorityError> {
    let (job, operation) = key;
    let spawn = matches!(request, EffectRequest::Spawn { .. });
    let stop = matches!(
        request,
        EffectRequest::CancelJob { .. } | EffectRequest::CancelTask { .. }
    );
    let mark = match link.instruct(target, token, instruction).await {
        Ok(InstructAnswer::Accepted) => {
            seen.respawn.remove(&key);
            seen.unreached.remove(&key);
            if stop {
                seen.stopped.insert(key);
            }
            spawn.then_some(OperationMark::Succeeded)
        }
        Ok(InstructAnswer::Refused {
            reason: InstructRefusal::NotRunning,
        }) if stop => {
            seen.stopped.insert(key);
            seen.unreached.remove(&key);
            tracing::info!(%job, %operation, attempt = %token.attempt, "the node runs nothing of the attempt; it ends cancelled");
            let ended = authority
                .observe_authority(
                    job,
                    Observation::Exited {
                        attempt: token.attempt,
                        generation: token.generation,
                        outcome: ExitOutcome::Cancelled,
                    },
                )
                .await;
            if let Err(e) = ended {
                // The attempt ended by another way since; its end settled
                // the cancel.
                tracing::info!(%job, %operation, error = %e, "the attempt was not ended here");
            }
            None
        }
        Ok(InstructAnswer::Refused {
            reason: InstructRefusal::Busy { .. },
        }) if stop => {
            seen.unreached.insert(key);
            seen.retry = true;
            None
        }
        Ok(InstructAnswer::Refused {
            reason: InstructRefusal::Busy { .. } | InstructRefusal::Frozen,
        }) if spawn => {
            seen.respawn.insert(key);
            seen.retry = true;
            None
        }
        Ok(InstructAnswer::Refused { reason }) => {
            tracing::info!(%job, %operation, %reason, "the node refused the instruction");
            seen.respawn.remove(&key);
            seen.unreached.remove(&key);
            if stop {
                seen.stopped.insert(key);
            }
            Some(OperationMark::Failed)
        }
        Err(e) if stop => {
            tracing::warn!(%job, %operation, error = %e, "the stop did not reach its node; it is sent again");
            seen.unreached.insert(key);
            seen.retry = true;
            None
        }
        Err(e) => {
            tracing::warn!(%job, %operation, error = %e, "the instruction did not reach its node");
            seen.respawn.remove(&key);
            Some(OperationMark::Unknown)
        }
    };
    if let Some(mark) = mark {
        authority.mark_authority(job, operation, mark).await?;
    }
    Ok(())
}

/// Sends again every stop of a cancel in flight that did not reach its
/// node or found it busy, every unsettled stop once after a restart (the
/// first pass of a process), and every spawn its node answered busy or
/// frozen; a spawn whose attempt has ended since fails, so its task rides
/// in the next launch. A pass with none of these reads nothing.
async fn resend(authority: &Authority, link: &dyn Link, seen: &mut Seen) {
    // Nothing to send again reads nothing: a pass after the first one
    // costs no journal read unless a stop or a spawn waits.
    let sweep = !std::mem::replace(&mut seen.swept, true);
    if !sweep && seen.unreached.is_empty() && seen.respawn.is_empty() {
        return;
    }
    let jobs = match authority.list().await {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!(error = %e, "the jobs could not be read");
            return;
        }
    };
    for state in jobs {
        for op in &state.operations {
            let key = (state.job.id, op.id);
            if !matches!(
                op.status,
                OperationStatus::Dispatched | OperationStatus::Unknown
            ) {
                // Settled since: nothing waits on it any more.
                seen.unreached.remove(&key);
                seen.respawn.remove(&key);
                continue;
            }
            let again = match op.kind {
                CommandKind::CancelJob | CommandKind::CancelTask => {
                    matches!(
                        op.status,
                        OperationStatus::Dispatched | OperationStatus::Unknown
                    ) && !seen.stopped.contains(&key)
                        && (sweep || seen.unreached.contains(&key))
                }
                CommandKind::RegisterTask => {
                    op.status == OperationStatus::Dispatched && seen.respawn.contains(&key)
                }
                _ => false,
            };
            if !again {
                continue;
            }
            let Some(request) = state.request(op.id) else {
                continue;
            };
            let target = instruction_for(&state, request).and_then(|(attempt, instruction)| {
                let token = token_of(&state, attempt)?;
                let ResourceRef::Node { node } = state.attempt(attempt)?.resource;
                Some((node, token, instruction))
            });
            let outcome = match target {
                Some((node, token, instruction)) => {
                    instruct(
                        authority,
                        link,
                        seen,
                        key,
                        request,
                        (node, token),
                        instruction,
                    )
                    .await
                }
                None if op.kind == CommandKind::RegisterTask => {
                    seen.respawn.remove(&key);
                    authority
                        .mark_authority(state.job.id, op.id, OperationMark::Failed)
                        .await
                        .map(|_| ())
                }
                None => {
                    seen.unreached.remove(&key);
                    Ok(())
                }
            };
            if let Err(e) = outcome {
                tracing::warn!(job = %state.job.id, operation = %op.id, error = %e, "an instruction was not sent again");
            }
        }
    }
}

/// Whether a cancel of the job, or of `task`, is accepted and not settled.
fn cancel_pending(state: &ManagedState, task: TaskId) -> bool {
    state.operations.iter().any(|op| {
        matches!(
            op.status,
            OperationStatus::Accepted | OperationStatus::Dispatched | OperationStatus::Unknown
        ) && match state.request(op.id) {
            Some(EffectRequest::CancelJob { .. }) => true,
            Some(EffectRequest::CancelTask { task: of, .. }) => *of == task,
            _ => false,
        }
    })
}

/// Every grant a placement made that may still be launched: granted, at
/// the job's generation, made by a create, a continue or a recover's
/// failover, with no cancel
/// of it pending (its stop ends it unlaunched).
fn grants(state: &ManagedState) -> Vec<AttemptId> {
    state
        .attempts
        .iter()
        .filter(|a| {
            a.status == AttemptStatus::Granted
                && a.generation == state.job.generation
                && !cancel_pending(state, a.task)
                && state.operation(a.granted_by).is_some_and(|op| {
                    matches!(
                        op.kind,
                        CommandKind::CreateJob | CommandKind::Continue | CommandKind::Recover
                    )
                })
        })
        .map(|a| a.id)
        .collect()
}

async fn launch_grants(authority: &Authority, link: &dyn Link, seen: &mut Seen) {
    let jobs = match authority.list().await {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!(error = %e, "the jobs could not be read");
            return;
        }
    };
    for state in jobs {
        for attempt in grants(&state) {
            if seen.launched.contains(&attempt) {
                continue;
            }
            if let Err(e) = launch_one(authority, link, seen, &state, attempt).await {
                tracing::warn!(job = %state.job.id, %attempt, error = %e, "a grant was not launched");
                seen.retry = true;
            }
        }
    }
}

/// Plans and sends one grant's launch, and acts on the answer.
async fn launch_one(
    authority: &Authority,
    link: &dyn Link,
    seen: &mut Seen,
    state: &ManagedState,
    attempt: AttemptId,
) -> Result<(), AuthorityError> {
    let job = state.job.id;
    crate::faults::hit(crate::faults::Point::BeforeLaunch).await;
    let planned = authority
        .with_journal(false, move |journal| {
            let Some(state) = journal.job(job)? else {
                return Ok(Err("the job left the journal".to_owned()));
            };
            match plan(journal, &state, attempt) {
                Ok(plan) => Ok(Ok(plan)),
                Err(PlanError::Blocked(reason)) => Ok(Err(reason)),
                Err(PlanError::Authority(e)) => Err(e),
            }
        })
        .await?;
    let plan = match planned {
        Ok(plan) => plan,
        Err(reason) => {
            seen.launched.insert(attempt);
            return fail_attempt(authority, state, attempt, reason).await;
        }
    };
    let target = plan.node;
    match link.launch(target, plan.request).await {
        Ok(LaunchAnswer::Accepted) => {
            seen.launched.insert(attempt);
            tracing::info!(%job, %attempt, node = %target, "launched");
            crate::faults::hit(crate::faults::Point::AfterLaunchAccepted).await;
            Ok(())
        }
        Ok(LaunchAnswer::Refused {
            reason: LaunchRefusal::AlreadyLaunched | LaunchRefusal::Superseded { .. },
        }) => {
            seen.launched.insert(attempt);
            Ok(())
        }
        Ok(LaunchAnswer::Refused { reason }) => {
            seen.launched.insert(attempt);
            tracing::info!(%job, %attempt, node = %target, %reason, "the node refused the launch");
            fail_attempt(authority, state, attempt, reason.to_string()).await
        }
        Err(e) => Err(AuthorityError::Unavailable(format!(
            "the launch did not reach {target}: {e}"
        ))),
    }
}

/// Fails a grant that cannot start: the harness receipt `Blocked` with
/// `reason`, then `Exited { Failed }`, so the attempt ends, a continue
/// that made it fails and the task is free.
async fn fail_attempt(
    authority: &Authority,
    state: &ManagedState,
    attempt: AttemptId,
    reason: String,
) -> Result<(), AuthorityError> {
    let Some(found) = state.attempt(attempt) else {
        return Ok(());
    };
    let (job, generation) = (state.job.id, found.generation);
    authority
        .observe_authority(
            job,
            Observation::Receipts {
                attempt,
                generation,
                receipts: vec![TaskReceipt {
                    task: found.task,
                    class: TaskClass::Harness,
                    status: ReceiptStatus::Blocked { reason },
                    exit: None,
                    interrupted: false,
                    workspace: None,
                }],
            },
        )
        .await?;
    authority
        .observe_authority(
            job,
            Observation::Exited {
                attempt,
                generation,
                outcome: ExitOutcome::Failed { code: None },
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use fabric_core::managed::launch::{GrantToken, Report, ReportAnswer};
    use fabric_core::managed::{AllowedFidelity, Command, JobState, TaskState};

    use super::*;
    use crate::launch::fixture::{Asked, FakeLink, OTHER, OWN, REMOTE, World};

    /// `observation`, reported by `from` as its node would.
    async fn report(
        world: &World,
        job: JobId,
        from: fabric_core::id::NodeId,
        observation: Observation,
    ) {
        let (attempt, generation) = Report::reported(&observation).unwrap();
        let token = GrantToken {
            job,
            attempt,
            generation,
        };
        let answer =
            crate::report::accept(&world.authority, Some(from), Report { token, observation })
                .await;
        assert!(
            matches!(answer, ReportAnswer::Recorded { .. }),
            "{answer:?}"
        );
    }

    fn continue_of(task: TaskId) -> Command {
        Command::Continue {
            task,
            fidelity: AllowedFidelity::DEFAULT,
            pin: None,
        }
    }

    /// A granted attempt on a peer is launched there once per process,
    /// with the job's intent and policy and the open tasks that have a
    /// program; the peer's `Started` settles the grant.
    #[tokio::test]
    async fn a_granted_remote_attempt_is_launched_once_per_process() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let test = world
            .register(job, "test", TaskClass::Test, &["cargo", "test"])
            .await;
        let attempt = world.granted(job, REMOTE).await;
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        pass(&world.authority, &link, &mut seen).await;
        let launches = link.launches();
        assert_eq!(launches.len(), 1, "{launches:?}");
        let (node, request) = &launches[0];
        assert_eq!(*node, REMOTE);
        let state = world.state(job).await;
        assert_eq!(request.token, token_of(&state, attempt).unwrap());
        assert_eq!(request.task, state.job.primary);
        assert_eq!(request.intent, state.job.intent);
        assert_eq!(request.policy, state.job.policy);
        assert_eq!(request.tasks.len(), 1);
        assert_eq!(request.tasks[0].task, test);
        assert_eq!(request.tasks[0].program.argv, ["cargo", "test"]);
        assert_eq!(request.continuation, None);
        assert_eq!(request.package, None);
        assert_eq!(request.initial_checkpoint, None);
        assert_eq!(state.operations[0].status, OperationStatus::Dispatched);
        report(
            &world,
            job,
            REMOTE,
            Observation::Started {
                attempt,
                generation: request.token.generation,
            },
        )
        .await;
        let state = world.state(job).await;
        assert_eq!(
            state.attempt(attempt).unwrap().status,
            AttemptStatus::Running
        );
        assert_eq!(state.operations[0].status, OperationStatus::Succeeded);
    }

    /// A node that already runs the attempt answers `AlreadyLaunched`;
    /// the authority records nothing for it.
    #[tokio::test]
    async fn an_already_launched_answer_is_not_a_failure() {
        let world = World::new();
        let link = FakeLink::new();
        *link.launch_answer.lock().unwrap() = LaunchAnswer::Refused {
            reason: LaunchRefusal::AlreadyLaunched,
        };
        let job = world.created("create").await;
        let attempt = world.granted(job, REMOTE).await;
        let before = world.state(job).await.job.revision;
        pass(&world.authority, &link, &mut Seen::default()).await;
        assert_eq!(link.launches().len(), 1);
        let state = world.state(job).await;
        assert_eq!(state.job.revision, before, "an AlreadyLaunched wrote");
        assert_eq!(
            state.attempt(attempt).unwrap().status,
            AttemptStatus::Granted
        );
    }

    /// Any other refusal fails the attempt with the harness receipt
    /// `Blocked`, and the task is free for a continue.
    #[tokio::test]
    async fn a_refused_launch_fails_the_attempt_and_frees_the_task() {
        let world = World::new();
        let link = FakeLink::new();
        *link.launch_answer.lock().unwrap() = LaunchAnswer::Refused {
            reason: LaunchRefusal::HarnessMissing {
                harness: fabric_core::harness::HarnessKind::Codex,
            },
        };
        let job = world.created("create").await;
        let attempt = world.granted(job, REMOTE).await;
        pass(&world.authority, &link, &mut Seen::default()).await;
        let state = world.state(job).await;
        assert_eq!(
            state.attempt(attempt).unwrap().status,
            AttemptStatus::Failed
        );
        assert_eq!(
            state.task(state.job.primary).unwrap().state,
            TaskState::Open
        );
        assert!(state.live_attempt().is_none());
        let receipts = world
            .authority
            .with_journal(false, move |j| Ok(j.receipts(job, attempt)?))
            .await
            .unwrap();
        assert!(
            matches!(
                &receipts[..],
                [TaskReceipt { status: ReceiptStatus::Blocked { reason }, class: TaskClass::Harness, .. }]
                    if reason.contains("not installed")
            ),
            "{receipts:?}"
        );
        world
            .submit(job, "continue", continue_of(state.job.primary))
            .await
            .unwrap();
    }

    /// A continue of a running attempt asks its node for the cut and the
    /// end; once the cut is reported the continue is placed as a planned
    /// handoff and its grant launched with the continuation and the
    /// package, from the node that cut it. Its attempt ending before
    /// `Continued` fails the continue.
    #[tokio::test]
    async fn a_continue_with_a_live_attempt_asks_for_its_cut_then_places_from_it() {
        let world = World::new();
        let link = FakeLink::new();
        *link.place_on.lock().unwrap() = Some(OTHER);
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let primary = world.state(job).await.job.primary;
        let cont = world
            .submit(job, "continue", continue_of(primary))
            .await
            .unwrap();
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        assert_eq!(
            state.operation(cont).unwrap().status,
            OperationStatus::Dispatched
        );
        assert_eq!(
            link.instructions(),
            vec![(
                REMOTE,
                token_of(&state, first).unwrap(),
                Instruction::Checkpoint {
                    then: AfterCut::End
                }
            )]
        );
        assert!(!link.asked().iter().any(|a| matches!(a, Asked::Place(_))));
        let snapshot = world.cut(job, first, REMOTE).await;
        pass(&world.authority, &link, &mut seen).await;
        assert!(
            link.asked()
                .contains(&Asked::Place(PlacementKind::PlannedHandoff))
        );
        let state = world.state(job).await;
        let second = state.attempts.last().unwrap().clone();
        assert_eq!(second.granted_by, cont);
        assert_eq!(second.initial_checkpoint, Some(snapshot));
        let launches = link.launches();
        let (node, request) = launches.last().unwrap();
        assert_eq!(*node, OTHER);
        assert_eq!(request.token.attempt, second.id);
        assert_eq!(request.initial_checkpoint, Some(snapshot));
        assert_eq!(request.snapshot_at, Some(REMOTE));
        assert_eq!(request.continuation.unwrap().from, first);
        let package = request.package.as_ref().unwrap();
        assert_eq!(package.workspace.bundle_head, snapshot);
        report(
            &world,
            job,
            OTHER,
            Observation::Exited {
                attempt: second.id,
                generation: second.generation,
                outcome: ExitOutcome::Failed { code: None },
            },
        )
        .await;
        assert_eq!(
            world.state(job).await.operation(cont).unwrap().status,
            OperationStatus::Failed
        );
    }

    /// A continue's pin is what the handoff is placed with, in place of
    /// the job's own: the operator names the node the work moves to.
    #[tokio::test]
    async fn a_continue_s_pin_places_the_handoff_where_it_names() {
        let world = World::new();
        let link = FakeLink::new();
        *link.place_on.lock().unwrap() = Some(OTHER);
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let primary = world.state(job).await.job.primary;
        world
            .submit(
                job,
                "continue",
                Command::Continue {
                    task: primary,
                    fidelity: AllowedFidelity::DEFAULT,
                    pin: Some(OTHER),
                },
            )
            .await
            .unwrap();
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        world.cut(job, first, REMOTE).await;
        pass(&world.authority, &link, &mut seen).await;
        assert_eq!(*link.placed_pins.lock().unwrap(), vec![Some(OTHER)]);
    }

    /// A checkpoint of a running attempt is a `Checkpoint { Resume }` to
    /// its node, settled by its cut; a cancel is a `Stop` to the node of
    /// the live attempt, settled by the attempt's cancelled exit.
    #[tokio::test]
    async fn a_checkpoint_and_a_cancel_are_instructed_to_the_live_node() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let check = world
            .submit(job, "checkpoint", Command::Checkpoint { attempt: first })
            .await
            .unwrap();
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        let token = token_of(&state, first).unwrap();
        assert_eq!(
            link.instructions(),
            vec![(
                REMOTE,
                token,
                Instruction::Checkpoint {
                    then: AfterCut::Resume
                }
            )]
        );
        assert_eq!(
            state.operation(check).unwrap().status,
            OperationStatus::Dispatched
        );
        world.cut(job, first, REMOTE).await;
        let state = world.state(job).await;
        assert_eq!(
            state.operation(check).unwrap().status,
            OperationStatus::Succeeded
        );
        assert_eq!(state.attempt(first).unwrap().status, AttemptStatus::Running);

        let cancel = world
            .submit(
                job,
                "cancel",
                Command::CancelJob {
                    reason: "operator".into(),
                },
            )
            .await
            .unwrap();
        pass(&world.authority, &link, &mut seen).await;
        assert_eq!(
            link.instructions().last().unwrap(),
            &(
                REMOTE,
                token,
                Instruction::Stop {
                    reason: "operator".into()
                }
            )
        );
        assert_eq!(
            world.state(job).await.operation(cancel).unwrap().status,
            OperationStatus::Dispatched
        );
        report(
            &world,
            job,
            REMOTE,
            Observation::Exited {
                attempt: first,
                generation: token.generation,
                outcome: ExitOutcome::Cancelled,
            },
        )
        .await;
        let state = world.state(job).await;
        assert_eq!(state.job.state, JobState::Cancelled);
        assert_eq!(
            state.operation(cancel).unwrap().status,
            OperationStatus::Succeeded
        );
    }

    /// A program registered while its parent's attempt runs is a `Spawn`
    /// to that attempt's node, succeeded when the node takes it and failed
    /// when it refuses for good; one refused while the node prepares the
    /// root or quiesces (`Busy`, `Frozen`) is sent again on a later pass.
    #[tokio::test]
    async fn a_spawn_is_instructed_and_settled_by_the_answer() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let mut seen = Seen::default();
        let build = world
            .register(job, "build", TaskClass::Build, &["cargo", "build"])
            .await;
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        let (node, token, instruction) = link.instructions().last().unwrap().clone();
        assert_eq!((node, token), (REMOTE, token_of(&state, first).unwrap()));
        assert!(matches!(
            instruction,
            Instruction::Spawn(TaskLaunch { task, class: TaskClass::Build, ref program })
                if task == build && program.argv == ["cargo", "build"]
        ));
        let spawned = |state: &ManagedState, task: TaskId| {
            state
                .requests
                .iter()
                .find(|(_, r)| *r == EffectRequest::Spawn { task })
                .map(|(op, _)| state.operation(*op).unwrap().status.clone())
        };
        assert_eq!(spawned(&state, build), Some(OperationStatus::Succeeded));
        *link.instruct_answer.lock().unwrap() = InstructAnswer::Refused {
            reason: InstructRefusal::Frozen,
        };
        let test = world
            .register(job, "test", TaskClass::Test, &["cargo", "test"])
            .await;
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        assert_eq!(spawned(&state, test), Some(OperationStatus::Dispatched));
        assert!(seen.retry, "a frozen spawn asks for another pass");
        *link.instruct_answer.lock().unwrap() = InstructAnswer::Accepted;
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        assert_eq!(spawned(&state, test), Some(OperationStatus::Succeeded));
        assert_eq!(
            link.instructions().len(),
            3,
            "the frozen spawn was sent again once"
        );
        *link.instruct_answer.lock().unwrap() = InstructAnswer::Refused {
            reason: InstructRefusal::Internal {
                reason: "no".into(),
            },
        };
        let refused = world
            .register(job, "lint", TaskClass::Test, &["cargo", "clippy"])
            .await;
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        assert_eq!(spawned(&state, refused), Some(OperationStatus::Failed));
        assert_eq!(state.task(refused).unwrap().state, TaskState::Open);
    }

    /// A cancel whose stop did not reach the node stays dispatched and its
    /// stop is sent again on the next pass, so the attempt does not run on
    /// while the heartbeat keeps saying `Live`.
    #[tokio::test]
    async fn a_stop_that_did_not_reach_its_node_is_sent_again() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        *link.instruct_failures.lock().unwrap() = 1;
        let cancel = world
            .submit(
                job,
                "cancel",
                Command::CancelJob {
                    reason: "operator".into(),
                },
            )
            .await
            .unwrap();
        pass(&world.authority, &link, &mut seen).await;
        assert_eq!(
            world.state(job).await.operation(cancel).unwrap().status,
            OperationStatus::Dispatched
        );
        assert!(seen.retry, "an unreached stop asks for another pass");
        pass(&world.authority, &link, &mut seen).await;
        pass(&world.authority, &link, &mut seen).await;
        let stops = link
            .instructions()
            .into_iter()
            .filter(|(_, _, i)| matches!(i, Instruction::Stop { .. }))
            .count();
        assert_eq!(stops, 2, "sent again once, then taken");
    }

    /// A stop its node answers `NotRunning` (the node restarted, or the
    /// launch never reached it) ends the attempt cancelled, which cancels
    /// A recover is placed by the M5 placement as a failover, with the
    /// superseded attempt's node named as the current placement, not
    /// healthy, and its grant is launched like any other: from the
    /// acknowledged checkpoint, held by this authority (ADR 0028).
    #[tokio::test]
    async fn a_recover_is_placed_as_a_failover_and_launched_like_any_grant() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let state = world.state(job).await;
        let generation = state.attempt(first).unwrap().generation;
        let snapshot = fabric_core::id::SnapshotId::of(b"acknowledged");
        let cut = fabric_core::managed::CutDigest::from_bytes([4; 32]);
        world
            .authority
            .observe_authority(
                job,
                Observation::Captured {
                    attempt: first,
                    generation,
                    snapshot,
                    cut,
                },
            )
            .await
            .unwrap();
        world
            .authority
            .acknowledge(
                job,
                fabric_core::managed::CheckpointAck {
                    attempt: first,
                    generation,
                    snapshot,
                    cut,
                    manifests: 1,
                    chunks: 1,
                    bytes: 1,
                    fetched_from: REMOTE,
                },
            )
            .await
            .unwrap();
        world
            .authority
            .observe_authority(
                job,
                Observation::ResourceUnavailable {
                    attempt: first,
                    generation,
                    last_heartbeat_ms: None,
                    at_ms: 1,
                },
            )
            .await
            .unwrap();
        let recover = world
            .submit(
                job,
                "recover",
                Command::Recover {
                    attempt: first,
                    from: snapshot,
                    reason: "silent".into(),
                },
            )
            .await
            .unwrap();
        *link.place_on.lock().unwrap() = Some(OTHER);
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        let asked = link.asked.lock().unwrap().clone();
        assert!(
            asked.contains(&Asked::Place(PlacementKind::Failover)),
            "{asked:?}"
        );
        let state = world.state(job).await;
        let next = state.attempts.last().unwrap();
        assert_eq!(next.granted_by, recover);
        assert_eq!(next.resource, ResourceRef::Node { node: OTHER });
        assert_eq!(next.initial_checkpoint, Some(snapshot));
        let launched = asked.iter().find_map(|a| match a {
            Asked::Launch(node, request) if request.token.attempt == next.id => {
                Some((*node, request.clone()))
            }
            _ => None,
        });
        let (node, request) = launched.expect("the recover's grant was launched");
        assert_eq!(node, OTHER);
        assert_eq!(request.initial_checkpoint, Some(snapshot));
        assert_eq!(
            request.snapshot_at,
            Some(OWN),
            "the authority holds the bytes"
        );
    }

    /// Review finding on PR #39: a continue after a supersede was placed
    /// as a planned handoff from the task's last cut, captured only and
    /// held by the lost node alone. It is placed as a failover with that
    /// node excluded, from the acknowledged checkpoint this authority
    /// holds.
    #[tokio::test]
    async fn a_continue_after_a_supersede_is_placed_as_a_failover_from_the_acknowledged_checkpoint()
    {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let generation = world.state(job).await.attempt(first).unwrap().generation;
        let snapshot = fabric_core::id::SnapshotId::of(b"acknowledged");
        let cut = fabric_core::managed::CutDigest::from_bytes([4; 32]);
        world
            .authority
            .observe_authority(
                job,
                Observation::Captured {
                    attempt: first,
                    generation,
                    snapshot,
                    cut,
                },
            )
            .await
            .unwrap();
        world
            .authority
            .acknowledge(
                job,
                fabric_core::managed::CheckpointAck {
                    attempt: first,
                    generation,
                    snapshot,
                    cut,
                    manifests: 1,
                    chunks: 1,
                    bytes: 1,
                    fetched_from: REMOTE,
                },
            )
            .await
            .unwrap();
        world
            .authority
            .observe_authority(
                job,
                Observation::ResourceUnavailable {
                    attempt: first,
                    generation,
                    last_heartbeat_ms: None,
                    at_ms: 1,
                },
            )
            .await
            .unwrap();
        world
            .submit(
                job,
                "supersede",
                Command::Supersede {
                    attempt: first,
                    reason: "silent".into(),
                },
            )
            .await
            .unwrap();
        let primary = world.state(job).await.job.primary;
        let cont = world
            .submit(
                job,
                "continue",
                Command::Continue {
                    task: primary,
                    fidelity: fabric_core::managed::AllowedFidelity::DEFAULT,
                    pin: None,
                },
            )
            .await
            .unwrap();
        *link.place_on.lock().unwrap() = Some(OTHER);
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        let asked = link.asked.lock().unwrap().clone();
        assert!(
            asked.contains(&Asked::Place(PlacementKind::Failover)),
            "{asked:?}"
        );
        let state = world.state(job).await;
        let next = state.attempts.last().unwrap();
        assert_eq!(next.granted_by, cont);
        assert_eq!(next.resource, ResourceRef::Node { node: OTHER });
        assert_eq!(next.initial_checkpoint, Some(snapshot));
        let launched = asked.iter().find_map(|a| match a {
            Asked::Launch(node, request) if request.token.attempt == next.id => {
                Some((*node, request.clone()))
            }
            _ => None,
        });
        let (node, request) = launched.expect("the continue's grant was launched");
        assert_eq!(node, OTHER);
        assert_eq!(
            request.snapshot_at,
            Some(OWN),
            "the authority holds the bytes"
        );
    }

    /// the job; a grant with a cancel pending is never launched.
    #[tokio::test]
    async fn a_stop_its_node_does_not_run_ends_the_attempt_and_the_job() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        *link.instruct_answer.lock().unwrap() = InstructAnswer::Refused {
            reason: InstructRefusal::NotRunning,
        };
        let cancel = world
            .submit(
                job,
                "cancel",
                Command::CancelJob {
                    reason: "operator".into(),
                },
            )
            .await
            .unwrap();
        let mut seen = Seen::default();
        pass(&world.authority, &link, &mut seen).await;
        let state = world.state(job).await;
        assert_eq!(
            state.attempt(first).unwrap().status,
            AttemptStatus::Cancelled
        );
        assert_eq!(state.job.state, JobState::Cancelled);
        assert_eq!(
            state.operation(cancel).unwrap().status,
            OperationStatus::Succeeded
        );

        // A grant not launched yet when the cancel comes is not launched.
        let world = World::new();
        let job = world.created("create").await;
        let granted = world.granted(job, REMOTE).await;
        world
            .submit(
                job,
                "cancel",
                Command::CancelJob {
                    reason: "operator".into(),
                },
            )
            .await
            .unwrap();
        let link = FakeLink::new();
        *link.instruct_answer.lock().unwrap() = InstructAnswer::Refused {
            reason: InstructRefusal::NotRunning,
        };
        pass(&world.authority, &link, &mut Seen::default()).await;
        let state = world.state(job).await;
        assert!(link.launches().is_empty(), "a cancelled grant was launched");
        assert_eq!(
            state.attempt(granted).unwrap().status,
            AttemptStatus::Cancelled
        );
        assert_eq!(state.job.state, JobState::Cancelled);
    }
}

/// A kill at a point of the launch, simulated by a dispatch that starts
/// again with nothing remembered: what the journal holds is what a
/// restarted node acts on.
#[cfg(test)]
mod crash_tests {
    use super::*;
    use crate::launch::fixture::{FakeLink, REMOTE, World};

    /// The node accepted a launch and the authority died before anything
    /// else (`after_launch_accepted`): the restarted dispatch sends the
    /// launch again, the node answers `AlreadyLaunched`, and the attempt
    /// runs once, with nothing written for the second send.
    #[tokio::test]
    async fn a_launch_killed_after_the_node_accepted_is_not_launched_twice() {
        let world = World::new();
        let link = FakeLink::new();
        let job = world.created("create").await;
        let attempt = world.granted(job, REMOTE).await;
        pass(&world.authority, &link, &mut Seen::default()).await;
        let revision = world.state(job).await.job.revision;
        pass(&world.authority, &link, &mut Seen::default()).await;
        assert_eq!(link.launches().len(), 2, "the restart did not send again");
        assert_eq!(link.started(), 1, "the attempt was launched twice");
        let state = world.state(job).await;
        assert_eq!(state.job.revision, revision);
        assert_eq!(
            state.attempt(attempt).unwrap().status,
            AttemptStatus::Granted
        );
    }
}
