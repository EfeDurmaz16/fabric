//! The spans of the node's operations, built from the request and its
//! answer. Each builder reads ids, counters and codes, never text.

use fabric_client::api::{Reason, Reply, Request};
use fabric_core::id::JobId;
use fabric_core::managed::{
    Command, Envelope, ManagedEvent, Observation, OperationKey, StoredResult, Transition,
};
use fabric_journal::{Answer, CommitError};

use super::span::{
    Code, OpSpan, SpanContext, SpanKind, SpanOp, SpanResult, attempt_context, digest,
    operation_context, snake, standalone_context, type_name,
};
use crate::authority::AuthorityError;
use crate::error::NodeError;

/// A managed command's span, begun before the journal sees it.
pub(crate) fn command_begin(envelope: &Envelope<Command>) -> OpSpan {
    let mut span = OpSpan::begin(
        SpanOp::managed(envelope.body.kind()),
        SpanKind::Internal,
        standalone_context(),
    );
    span.job = envelope.job;
    span.key_digest = Some(digest(envelope.key.as_str()));
    span
}

/// A derived command's span (a checkpoint or a continue of the job's
/// state): its kind is known once the journal answers.
pub(crate) fn derived_begin(key: &OperationKey, job: JobId) -> OpSpan {
    let mut span = OpSpan::begin(
        SpanOp::MANAGED_COMMAND,
        SpanKind::Internal,
        standalone_context(),
    );
    span.job = Some(job);
    span.key_digest = Some(digest(key.as_str()));
    span
}

/// The attempt `events` grant, if one: its id, task and generation, so
/// the grant's span names them and links to the attempt's trace.
fn granted(span: &mut OpSpan, events: &[ManagedEvent]) {
    for event in events {
        match event {
            ManagedEvent::AttemptGranted {
                attempt,
                task,
                generation,
                ..
            } => {
                span.attempt = Some(*attempt);
                span.task = Some(*task);
                span.generation = Some(generation.get());
                // Generation n's grant ends generation n - 1: a regrant.
                span.supersedes = generation.get().checked_sub(1).filter(|g| *g >= 1);
                span.links.push(attempt_context(*attempt));
            }
            ManagedEvent::CheckpointCutRecorded {
                attempt,
                snapshot,
                cut,
                ..
            } => {
                span.attempt.get_or_insert(*attempt);
                span.snapshot = Some(*snapshot);
                span.checkpoint = Some(super::span::Digest(cut.0));
            }
            ManagedEvent::CommandAccepted { kind, .. } => {
                span.op = SpanOp::managed(*kind);
            }
            ManagedEvent::StaleObservationRefused { .. } => {
                span.refusal = Code::new("stale_generation");
                span.result = SpanResult::Refused;
            }
            _ => {}
        }
    }
}

fn authority_failed(span: &mut OpSpan, error: &AuthorityError) {
    match error {
        AuthorityError::Commit(CommitError::Domain(domain)) => {
            span.refused(crate::managed::refusal_code(domain));
        }
        AuthorityError::Commit(commit) => span.failed(SpanResult::Failed, type_name(commit)),
        other => span.failed(SpanResult::Failed, type_name(other)),
    }
}

/// Ends a command's span with the journal's answer.
pub(crate) fn command_end(span: &mut OpSpan, answer: Result<&Answer, &AuthorityError>) {
    match answer {
        Ok(answer) => match &answer.result {
            StoredResult::Accepted(accepted) => {
                span.context = operation_context(accepted.operation);
                span.operation = Some(accepted.operation);
                span.job = Some(accepted.job);
                span.revision = Some(accepted.revision.get());
                granted(span, &accepted.events);
                span.end(if answer.replayed {
                    SpanResult::Replayed
                } else {
                    SpanResult::Done
                });
            }
            StoredResult::Rejected(refusal) => {
                span.refused(crate::managed::refusal_code(refusal));
            }
        },
        Err(error) => authority_failed(span, error),
    }
}

/// An observation's span: in its attempt's trace, a child of the
/// attempt's root (the launch); a placement or an unknown outcome in its
/// operation's trace instead.
pub(crate) fn observation_begin(job: JobId, observation: &Observation) -> OpSpan {
    let mut span = OpSpan::begin(
        SpanOp::observed(observation),
        SpanKind::Internal,
        standalone_context(),
    );
    span.job = Some(job);
    let (attempt, generation) = match observation {
        Observation::Started {
            attempt,
            generation,
        }
        | Observation::Checkpointed {
            attempt,
            generation,
            ..
        }
        | Observation::Receipts {
            attempt,
            generation,
            ..
        }
        | Observation::HandoffBlocked {
            attempt,
            generation,
            ..
        }
        | Observation::Continued {
            attempt,
            generation,
            ..
        }
        | Observation::Exited {
            attempt,
            generation,
            ..
        }
        | Observation::Lost {
            attempt,
            generation,
        } => (Some(*attempt), Some(*generation)),
        Observation::Placed {
            operation, task, ..
        } => {
            span.operation = Some(*operation);
            span.task = Some(*task);
            (None, None)
        }
        Observation::Unknown { operation } => {
            span.operation = Some(*operation);
            (None, None)
        }
    };
    if let Some(attempt) = attempt {
        span.attempt = Some(attempt);
        span.child_of(attempt_context(attempt));
    } else if let Some(operation) = span.operation {
        span.child_of(operation_context(operation));
    }
    span.generation = generation.map(fabric_core::managed::Generation::get);
    if let Observation::Checkpointed { snapshot, .. } = observation {
        span.snapshot = Some(*snapshot);
    }
    span
}

/// Ends an observation's span with what the journal recorded.
pub(crate) fn observation_end(span: &mut OpSpan, recorded: Result<&Transition, &AuthorityError>) {
    match recorded {
        Ok(transition) => {
            span.revision = Some(transition.revision.get());
            let observed = span.attempt;
            granted(span, &transition.events);
            // A placement's grant names the attempt it made, which is not
            // the observed one: the span stays in the operation's trace.
            if observed.is_some() {
                span.attempt = observed;
            }
            if span.result == SpanResult::Refused {
                span.end(SpanResult::Refused);
            } else {
                span.end(SpanResult::Done);
            }
        }
        Err(error) => authority_failed(span, error),
    }
}

/// The span of a client request that changes standalone state; `None`
/// for a read, and for a managed request, which the authority's command
/// span describes.
pub(crate) fn request_begin(request: &Request) -> Option<OpSpan> {
    let (op, job) = match request {
        Request::NodeAdd { .. } => (SpanOp::NODE_ADD, None),
        Request::JobNew { .. } => (SpanOp::JOB_NEW, None),
        Request::JobRun { job, .. } => (SpanOp::JOB_RUN, Some(*job)),
        Request::JobResume { job, .. } => (SpanOp::JOB_RESUME, Some(*job)),
        Request::JobSuspend { job, .. } => (SpanOp::JOB_SUSPEND, Some(*job)),
        Request::JobMove { job, .. } => (SpanOp::JOB_MOVE, Some(*job)),
        Request::JobSteal { job, .. } => (SpanOp::JOB_STEAL, Some(*job)),
        Request::JobDecide { job, .. } => (SpanOp::JOB_DECIDE, Some(*job)),
        Request::JobRepair { job } => (SpanOp::JOB_REPAIR, Some(*job)),
        Request::HarnessStarted { job, .. } => (SpanOp::HARNESS_STARTED, Some(*job)),
        Request::HarnessSession { job, .. } => (SpanOp::HARNESS_SESSION, Some(*job)),
        Request::HarnessExited { job, .. } => (SpanOp::HARNESS_EXITED, Some(*job)),
        _ => return None,
    };
    let mut span = OpSpan::begin(op, SpanKind::Internal, standalone_context());
    span.job = job;
    match request {
        Request::NodeAdd { node, .. } => span.peer = Some(*node),
        Request::JobResume { on, .. } => span.peer = *on,
        Request::JobMove { to, .. } => span.target = Some(*to),
        Request::HarnessStarted { epoch, .. } | Request::HarnessSession { epoch, .. } => {
            span.epoch = Some(epoch.get());
        }
        _ => {}
    }
    Some(span)
}

/// Whether a refusal with `reason` is the node failing, not refusing.
fn is_failure(reason: &Reason) -> bool {
    matches!(
        reason,
        Reason::Internal | Reason::ManagedUnavailable | Reason::Peer { .. } | Reason::Fetch { .. }
    )
}

/// The snake-case code of a refusal reason, as the client API names it.
pub(crate) fn reason_code(reason: &Reason) -> Code {
    Code::new(&snake(type_name(reason).as_str())).unwrap_or_else(|| type_name(reason))
}

/// Ends a client request's span with its reply or its refusal.
pub(crate) fn request_end(span: &mut OpSpan, reply: &Result<Reply, NodeError>) {
    match reply {
        Ok(reply) => {
            match reply {
                Reply::JobNew(created) => {
                    span.job = Some(created.job);
                    span.epoch = Some(created.epoch.get());
                }
                Reply::JobSuspend(suspended) => span.snapshot = Some(suspended.snapshot),
                _ => {}
            }
            span.end(SpanResult::Done);
        }
        Err(error) => {
            let refusal = error.to_refusal();
            span.error_type = Some(type_name(error));
            span.refusal = Some(reason_code(refusal.reason()));
            span.end(if is_failure(refusal.reason()) {
                SpanResult::Failed
            } else {
                SpanResult::Refused
            });
        }
    }
}

/// A launch's span on the node that runs the attempt: the root of the
/// attempt's trace, a server span for the authority that granted it.
pub(crate) fn launch_begin(
    op: SpanOp,
    token: &fabric_core::managed::launch::GrantToken,
    peer: Option<fabric_core::id::NodeId>,
) -> OpSpan {
    let context: SpanContext = attempt_context(token.attempt);
    let kind = if op == SpanOp::REPORT {
        SpanKind::Client
    } else {
        SpanKind::Server
    };
    let mut span = OpSpan::begin(op, kind, context);
    if op != SpanOp::PEER_LAUNCH {
        span.child_of(context);
    }
    span.job = Some(token.job);
    span.attempt = Some(token.attempt);
    span.generation = Some(token.generation.get());
    span.peer = peer;
    span
}

/// Ends a span with a refusal enum's variant as its code, or done.
pub(crate) fn answer_end<R: std::fmt::Debug>(span: &mut OpSpan, refusal: Option<&R>) {
    match refusal {
        Some(reason) => span.refused(&snake(type_name(reason).as_str())),
        None => span.end(SpanResult::Done),
    }
}
