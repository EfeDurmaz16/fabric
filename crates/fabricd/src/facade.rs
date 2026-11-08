//! The job facade (ADR 0027, docs/facade-protocol.md): authenticated
//! remote job operations, served by this node's authority for a message
//! that arrives over the node link or from a local `facade_call`.
//!
//! Invariant: the authority checks in one order and stops at the first
//! failure: the message's schema, its version, the token known, not
//! revoked, not expired, the job scope, the resource scope, then the
//! message. Everything before "the message" answers `refused` and writes
//! nothing. A command is `accepted` only after the journal committed it,
//! under a key scoped to the token's actor, so a repeat reconciles the
//! same operation and the same key with another request is a conflict.
//! A watch is a read from the journal after a cursor: the journal is the
//! queue, so a slow reader costs the authority no memory; a reader waits
//! on the authority's commit counter and never the other way round, and
//! a revoke wakes every open watch, which ends with `closed: revoked`
//! within one delivery. The rendering of a job's events and its
//! inspection lives here too, so `managed_inspect` and the facade show
//! one object.

use std::fs;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use fabric_client::api::{
    BlockerWire, CandidateWire, ContinuationWire, ExclusionWire, FidelityName, Inspection,
    ManagedJob, ManagedPolicy, ManagedSource, Minted, NoteAt, NoteWire, PlacementWire,
    ProviderOperationWire, RankWire, Reason, ReceiptWire, ResidualWire, Revoked, VerdictWire,
};
use fabric_client::facade::{
    ClientMessage, DEFAULT_WAIT_MS, EventWire, FacadeAnswer, Refusal, RefusalCode, Submit,
    WATCH_WINDOW, harness_json,
};
use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::facade::{
    Actor, BEARER_BYTES, FacadeTokenRecord, JobAsk, JobScope, ResourceAsk, ResourceScope,
    ScopeKind, TokenCheck, TokenId, check_token, token_digest,
};
use fabric_core::managed::placement::{Exclusion, RankReason, Verdict};
use fabric_core::managed::secret_ref::{AttemptScope, ResourceKind, SecretScope};
use fabric_core::managed::source::{CommitId, GitUri};
use fabric_core::managed::{
    AllowedFidelity, AttemptStatus, Command, DomainError, EffectRequest, Envelope, EventFrame,
    EventSeq, HandoffBlocker, Intent, JobPolicy, JobState, ManagedEvent, ManagedState, Note,
    OperationKey, PlacementDecision, ReceiptStatus, RestartPolicy, SourceBinding, StoredResult,
    TaskKind, TaskReceipt, WorkRevision,
};
use fabric_core::managed::{SecretName, SecretRef};
use fabric_core::path::RelPath;
use fabric_journal::{Answer, Blocker, CommitError, Journal, ProviderOperation, Residual};
use serde_json::{Map, Value, json};
use ulid::Ulid;

use crate::authority::AuthorityError;
use crate::authority::request_digest;
use crate::clock::now_ms;
use crate::error::NodeError;
use crate::managed::refusal_code;
use crate::node::Node;

/// How long a facade call over the link may take beyond the watch's own
/// wait: the message, the authority's work and the answer.
const LINK_SLACK: Duration = Duration::from_secs(5);

fn refused(reason: Reason, message: impl Into<String>) -> NodeError {
    NodeError::Managed {
        reason: Box::new(reason),
        message: message.into(),
    }
}

/// The prefix of every facade key in the journal. The owner's socket
/// refuses a key that starts with it, so an owner's key and an actor's
/// never share a row.
pub(crate) const FACADE_KEY_PREFIX: &str = "facade:";

/// The journal's key for a facade command: the client's key under the
/// token's actor, in the facade's namespace, so two actors never share
/// one and no actor shares one with the owner (ADR 0027). An actor holds
/// no ':', so the actor ends at the first one after the prefix. A pair
/// too long for one key carries the key's digest instead; both forms are
/// deterministic, so a repeat lands on the same row.
pub(crate) fn journal_key(actor: &Actor, key: &OperationKey) -> OperationKey {
    let joined = format!("{FACADE_KEY_PREFIX}{actor}:{key}");
    if let Ok(key) = OperationKey::parse(&joined) {
        return key;
    }
    let digest = blake3::hash(key.as_str().as_bytes());
    let short = hex(&digest.as_bytes()[..16]);
    OperationKey::parse(&format!("{FACADE_KEY_PREFIX}{actor}:h:{short}"))
        .unwrap_or_else(|_| key.clone())
}

/// A stored answer as the facade gives it: `accepted`, or the refusal
/// the journal stored under the key.
fn facade_accepted(key: &OperationKey, answer: Answer) -> Result<FacadeAnswer, Refusal> {
    match answer.result {
        StoredResult::Accepted(accepted) => Ok(FacadeAnswer::Accepted {
            operation: accepted.operation,
            key: key.clone(),
            replayed: answer.replayed,
            job: accepted.job,
            revision: accepted.revision,
        }),
        StoredResult::Rejected(refusal) => Err(domain_refusal(&refusal)),
    }
}

/// A continue's fidelity list as a request digest's part: which names it
/// gives, in no order, or that it gives none.
pub(crate) fn fidelity_bytes(names: Option<&[FidelityName]>) -> Vec<u8> {
    names.map_or_else(
        || vec![0xff],
        |names| {
            let bit = |name: FidelityName, mask: u8| if names.contains(&name) { mask } else { 0 };
            vec![
                bit(FidelityName::NativeSession, 1)
                    | bit(FidelityName::NewNativeSessionWithPackage, 2)
                    | bit(FidelityName::WorkspaceOnly, 4),
            ]
        },
    )
}

/// An expected revision as a request digest's part: empty when none.
pub(crate) fn revision_bytes(expected: Option<WorkRevision>) -> Vec<u8> {
    expected
        .map(|r| r.get().to_be_bytes().to_vec())
        .unwrap_or_default()
}

/// 32 bytes from the system's random source, for a bearer; read from the
/// kernel's device so no crate enters the workspace for it.
fn random_bearer() -> Result<[u8; BEARER_BYTES], NodeError> {
    let mut bytes = [0u8; BEARER_BYTES];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| NodeError::io("/dev/urandom", e))?;
    Ok(bytes)
}

/// A domain refusal as the facade names it: `refused` with the refusal's
/// snake-case name, or its own code for the three the protocol names.
fn domain_refusal(error: &DomainError) -> Refusal {
    match error {
        DomainError::RevisionMismatch { current, .. } => Refusal::revision_mismatch(*current),
        DomainError::OperationConflict { .. } => Refusal::new(RefusalCode::Conflict, "key"),
        DomainError::UnknownJob { .. } => Refusal::new(RefusalCode::UnknownJob, "job"),
        DomainError::FidelityRefused { .. } => Refusal::new(RefusalCode::Refused, "fidelity"),
        other => Refusal::new(RefusalCode::Refused, refusal_code(other)).saying(other.to_string()),
    }
}

fn authority_refusal(error: &AuthorityError) -> Refusal {
    match error {
        AuthorityError::Commit(CommitError::Domain(e)) => domain_refusal(e),
        AuthorityError::Standalone(_) => Refusal::new(RefusalCode::UnknownJob, "job")
            .saying("the job is a standalone job, not a managed one"),
        other => Refusal::new(RefusalCode::Unavailable, "authority").saying(other.to_string()),
    }
}

/// The `JobPolicy` a client's policy asks for: the defaults with each
/// field the client set, checked by the domain's rules.
pub(crate) fn policy_from(policy: Option<&ManagedPolicy>) -> Result<JobPolicy, Refusal> {
    let mut out = JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly);
    let Some(policy) = policy else {
        return Ok(out);
    };
    let bad = |field: &str| {
        Refusal::new(RefusalCode::Refused, "bad_policy").saying(format!("policy.{field}"))
    };
    if let Some(ms) = policy.drain_budget_ms {
        out.drain_budget_ms = ms;
    }
    if let Some(allowed) = &policy.allowed_fidelity {
        out.allowed_fidelity = AllowedFidelity {
            native_session: allowed.contains(&FidelityName::NativeSession),
            new_native_session_with_package: allowed
                .contains(&FidelityName::NewNativeSessionWithPackage),
            workspace_only: allowed.contains(&FidelityName::WorkspaceOnly),
        };
    }
    if let Some(secrets) = &policy.secrets {
        out.secrets = secrets
            .iter()
            .map(|secret| {
                let name = SecretName::parse(&secret.name).map_err(|_| bad("secrets.name"))?;
                let attempt = match secret.attempt.as_str() {
                    "any" => AttemptScope::Any,
                    "current" => AttemptScope::Current,
                    _ => return Err(bad("secrets.attempt")),
                };
                let resource_kinds = secret
                    .resource_kinds
                    .iter()
                    .map(|kind| match kind.as_str() {
                        "node" => Ok(ResourceKind::Node),
                        _ => Err(bad("secrets.resource_kinds")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(SecretRef {
                    name,
                    scope: SecretScope {
                        attempt,
                        resource_kinds,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
    }
    if let Some(paths) = &policy.export_allowlist {
        out.export_allowlist = paths
            .iter()
            .map(|path| RelPath::parse(path).map_err(|_| bad("export_allowlist")))
            .collect::<Result<Vec<_>, _>>()?;
    }
    if let Some(bytes) = policy.disk_bytes {
        out.placement.disk_bytes = bytes;
    }
    out.placement.pin = policy.pin;
    out.check().map_err(|e| domain_refusal(&e))?;
    Ok(out)
}

/// The source binding a client named; a repository or a directory is
/// the local socket's to resolve, so here it is `source_local`.
pub(crate) fn remote_source(source: &ManagedSource) -> Result<SourceBinding, Refusal> {
    match source {
        ManagedSource::Git { uri, commit } => Ok(SourceBinding::Git {
            uri: GitUri::parse(uri).map_err(|e| {
                Refusal::new(RefusalCode::Refused, source_error_name(&e)).saying(e.to_string())
            })?,
            commit: CommitId::parse(commit).map_err(|e| {
                Refusal::new(RefusalCode::Refused, source_error_name(&e)).saying(e.to_string())
            })?,
        }),
        ManagedSource::Snapshot { snapshot } => Ok(SourceBinding::Snapshot {
            snapshot: *snapshot,
        }),
        ManagedSource::Empty => Ok(SourceBinding::Empty),
        ManagedSource::Repository { .. } | ManagedSource::Directory { .. } => {
            Err(Refusal::bad_request("source_local"))
        }
    }
}

fn source_error_name(error: &fabric_core::managed::SourceError) -> &'static str {
    use fabric_core::managed::SourceError as E;
    match error {
        E::BadUri { .. } => "bad_uri",
        E::CredentialInUri => "credential_in_uri",
        E::NotACommitId => "not_a_commit_id",
        E::Adoption => "adoption",
        E::RepositoryOverFacade => "repository_over_facade",
        E::BadSecretName => "bad_secret_name",
        E::Unavailable { .. } => "source_unavailable",
    }
}

/// The command a `submit` makes, once its source is bound.
pub(crate) fn create_command(submit: &Submit, source: SourceBinding) -> Result<Command, Refusal> {
    Ok(Command::CreateJob {
        intent: Intent {
            goal: submit.goal.clone(),
            harness: submit.harness.clone(),
            acceptance: submit.acceptance.clone(),
        },
        source,
        policy: policy_from(submit.policy.as_ref())?,
    })
}

/// The allowance a facade `continue` asks for: what it names, cut to what
/// the job's policy allows, or the policy's own when it names none. A
/// token holder never widens the allowance (ADR 0024: the owner does);
/// when nothing is left the domain refuses the continue as `fidelity`.
pub(crate) fn facade_allowance(names: Option<&[FidelityName]>, job: &JobPolicy) -> AllowedFidelity {
    let asked = allowance(names, job);
    let policy = job.allowed_fidelity;
    AllowedFidelity {
        native_session: asked.native_session && policy.native_session,
        new_native_session_with_package: asked.new_native_session_with_package
            && policy.new_native_session_with_package,
        workspace_only: asked.workspace_only && policy.workspace_only,
    }
}

/// The allowance a `continue` names, or the job's own; the owner's
/// socket passes it as named, since the owner may widen it.
pub(crate) fn allowance(names: Option<&[FidelityName]>, job: &JobPolicy) -> AllowedFidelity {
    names.map_or(job.allowed_fidelity, |names| AllowedFidelity {
        native_session: names.contains(&FidelityName::NativeSession),
        new_native_session_with_package: names.contains(&FidelityName::NewNativeSessionWithPackage),
        workspace_only: names.contains(&FidelityName::WorkspaceOnly),
    })
}

impl Node {
    /// Serves one facade message, JSON bytes in and JSON bytes out, as the
    /// node link and the local `facade_call` hand it over.
    pub(crate) async fn facade_serve(&self, bytes: &[u8]) -> Vec<u8> {
        let answer = match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => self.facade_answer(&value, now_ms()).await,
            Err(e) => FacadeAnswer::Refused(
                Refusal::bad_request("bad_string").saying(format!("not JSON: {e}")),
            ),
        };
        serde_json::to_vec(&answer.to_json()).unwrap_or_default()
    }

    /// Answers one facade message at `now_ms`, in the protocol's order.
    pub(crate) async fn facade_answer(&self, value: &Value, now_ms: u64) -> FacadeAnswer {
        let message = match ClientMessage::from_json(value) {
            Ok(message) => message,
            Err(refusal) => return FacadeAnswer::Refused(refusal),
        };
        let record = match self.token_record(message.token().as_bytes()).await {
            Ok(record) => record,
            Err(refusal) => return FacadeAnswer::Refused(refusal),
        };
        match check_token(
            record.as_ref(),
            now_ms,
            message.job_ask(),
            message.resource_ask(),
        ) {
            TokenCheck::Ok => {}
            TokenCheck::Unknown => {
                return FacadeAnswer::Refused(Refusal::new(
                    RefusalCode::Unauthorized,
                    "unknown_token",
                ));
            }
            TokenCheck::Revoked => {
                return FacadeAnswer::Refused(Refusal::new(RefusalCode::Unauthorized, "revoked"));
            }
            TokenCheck::Expired => {
                return FacadeAnswer::Refused(Refusal::new(RefusalCode::Unauthorized, "expired"));
            }
            TokenCheck::OutOfScope { what } => {
                return FacadeAnswer::Refused(Refusal::new(
                    RefusalCode::Scope,
                    match what {
                        ScopeKind::Job => "job",
                        ScopeKind::Resource => "resource",
                    },
                ));
            }
        }
        let Some(record) = record else {
            return FacadeAnswer::Refused(Refusal::new(RefusalCode::Unauthorized, "unknown_token"));
        };
        match self.facade_message(&record, message, now_ms).await {
            Ok(answer) => answer,
            Err(refusal) => FacadeAnswer::Refused(refusal),
        }
    }

    /// The token whose bearer this is; none when the journal holds no such
    /// token, or no journal exists yet.
    async fn token_record(
        &self,
        bearer: &[u8; BEARER_BYTES],
    ) -> Result<Option<FacadeTokenRecord>, Refusal> {
        let digest = token_digest(bearer);
        match self
            .authority
            .with_journal(false, move |journal| {
                journal
                    .token_by_digest(&digest)
                    .map_err(AuthorityError::from)
            })
            .await
        {
            Ok(record) => Ok(record),
            Err(AuthorityError::Unavailable(reason))
                if reason.starts_with("no managed journal") =>
            {
                Ok(None)
            }
            Err(e) => Err(authority_refusal(&e)),
        }
    }

    async fn facade_message(
        &self,
        record: &FacadeTokenRecord,
        message: ClientMessage,
        now_ms: u64,
    ) -> Result<FacadeAnswer, Refusal> {
        match message {
            ClientMessage::Submit { key, submit, .. } => {
                // The owner's socket refuses the same before the authority;
                // a secret in the harness arguments would be journaled and
                // shown to every token that covers the job.
                crate::harnesses::refuse_secret_kind(&submit.harness).map_err(|_| {
                    Refusal::new(RefusalCode::Refused, "credential_like")
                        .saying("the harness arguments hold what looks like a secret")
                })?;
                let source = remote_source(&submit.source)?;
                let command = create_command(&submit, source)?;
                self.facade_command(record, &key, None, None, command).await
            }
            ClientMessage::Inspect { job, .. } => {
                let inspection = self.inspect(job).await.map_err(|e| inspect_refusal(&e))?;
                Ok(FacadeAnswer::Inspection(Box::new(inspection)))
            }
            ClientMessage::Watch {
                token,
                job,
                after,
                wait_ms,
            } => {
                let bearer = *token.as_bytes();
                self.facade_watch(
                    bearer,
                    record.expires_ms,
                    job,
                    after,
                    wait_ms.unwrap_or(DEFAULT_WAIT_MS),
                    now_ms,
                )
                .await
            }
            ClientMessage::Cancel {
                key,
                job,
                expected_revision,
                task,
                reason,
                ..
            } => {
                let command = match task {
                    Some(task) => Command::CancelTask { task, reason },
                    None => Command::CancelJob { reason },
                };
                self.facade_command(record, &key, Some(job), expected_revision, command)
                    .await
            }
            ClientMessage::Checkpoint {
                key,
                job,
                expected_revision,
                ..
            } => {
                let request = request_digest(&[
                    b"checkpoint",
                    &job.to_bytes(),
                    &revision_bytes(expected_revision),
                ]);
                self.facade_derived(record, &key, job, request, move |state| {
                    let attempt = state
                        .live_attempt()
                        .or_else(|| state.attempts.last())
                        .map(|a| a.id)
                        .ok_or_else(|| {
                            Refusal::new(RefusalCode::Refused, "not_live")
                                .saying("the job has no attempt to checkpoint")
                        })?;
                    Ok((expected_revision, Command::Checkpoint { attempt }))
                })
                .await
            }
            ClientMessage::Continue {
                key,
                job,
                expected_revision,
                fidelity,
                pin,
                ..
            } => {
                let request = request_digest(&[
                    b"continue",
                    &job.to_bytes(),
                    &revision_bytes(expected_revision),
                    &fidelity_bytes(fidelity.as_deref()),
                    &pin.map(|p| p.to_bytes().to_vec()).unwrap_or_default(),
                ]);
                self.facade_derived(record, &key, job, request, move |state| {
                    let task = state.live_attempt().map_or(state.job.primary, |a| a.task);
                    let command = Command::Continue {
                        task,
                        fidelity: facade_allowance(fidelity.as_deref(), &state.job.policy),
                        pin,
                    };
                    Ok((expected_revision, command))
                })
                .await
            }
        }
    }

    /// Submits one command under the actor's key and answers `accepted`
    /// after the commit, or the refusal the journal stored.
    async fn facade_command(
        &self,
        record: &FacadeTokenRecord,
        key: &OperationKey,
        job: Option<JobId>,
        expected: Option<WorkRevision>,
        command: Command,
    ) -> Result<FacadeAnswer, Refusal> {
        let envelope = Envelope {
            key: journal_key(&record.actor, key),
            job,
            expected,
            body: command,
        };
        let answer = self
            .authority
            .submit(envelope)
            .await
            .map_err(|e| authority_refusal(&e))?;
        facade_accepted(key, answer)
    }

    /// Submits a command whose body `derive` makes from the job's state,
    /// deduplicated by the client's `request` (ADR 0027, failure mode 3:
    /// the same request repeated under its key is one operation however
    /// the job moved between the two sends).
    async fn facade_derived(
        &self,
        record: &FacadeTokenRecord,
        key: &OperationKey,
        job: JobId,
        request: [u8; 32],
        derive: impl FnOnce(&ManagedState) -> Result<(Option<WorkRevision>, Command), Refusal>
        + Send
        + 'static,
    ) -> Result<FacadeAnswer, Refusal> {
        let stored = journal_key(&record.actor, key);
        let envelope_key = stored.clone();
        let answer = self
            .authority
            .submit_derived(stored, request, job, move |state| {
                derive(state).map(|(expected, body)| Envelope {
                    key: envelope_key,
                    job: Some(job),
                    expected,
                    body,
                })
            })
            .await
            .map_err(|e| authority_refusal(&e))??;
        facade_accepted(key, answer)
    }

    /// A page of the job's events after `after`: the journal is the queue.
    /// With none and a wait, the read is repeated once the authority's
    /// commit counter moves, until the wait is over; a token revoked or
    /// expired meanwhile closes the watch with `revoked`.
    async fn facade_watch(
        &self,
        bearer: [u8; BEARER_BYTES],
        expires_ms: u64,
        job: JobId,
        after: Option<EventSeq>,
        wait_ms: u32,
        now_ms: u64,
    ) -> Result<FacadeAnswer, Refusal> {
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_millis(u64::from(wait_ms));
        // The watch wakes on a commit (a revocation is one) or at the
        // token's expiry, whichever is first; a read moves no counter.
        let expiry = started + Duration::from_millis(expires_ms.saturating_sub(now_ms));
        let mut commits = self
            .authority
            .commits()
            .map_err(|e| authority_refusal(&e))?;
        commits.mark_unchanged();
        let mut closed: Option<String> = None;
        loop {
            let limit = u32::try_from(WATCH_WINDOW + 1).unwrap_or(u32::MAX);
            let read = self
                .authority
                .with_journal(false, move |journal| {
                    let state = journal.job(job)?.map(|s| s.job.state);
                    let events = journal.events(job, after, limit)?;
                    Ok((state, events))
                })
                .await;
            let (state, mut events) = match read {
                Ok(read) => read,
                Err(AuthorityError::Unavailable(reason))
                    if reason.starts_with("no managed journal") =>
                {
                    return Err(Refusal::new(RefusalCode::UnknownJob, "job"));
                }
                Err(e) => return Err(authority_refusal(&e)),
            };
            let Some(state) = state else {
                return Err(Refusal::new(RefusalCode::UnknownJob, "job"));
            };
            if !events.is_empty() {
                let lagged = events.len() > WATCH_WINDOW;
                events.truncate(WATCH_WINDOW);
                let next = events.last().map(|e| e.seq);
                let events = events
                    .iter()
                    .map(|stored| EventWire {
                        seq: stored.seq,
                        revision: stored.frame.revision,
                        at_ms: stored.frame.at_ms,
                        event: event_json(&stored.frame),
                    })
                    .collect();
                return Ok(FacadeAnswer::Events {
                    job,
                    events,
                    next,
                    lagged,
                    closed,
                });
            }
            if closed.is_none() && matches!(state, JobState::Finished | JobState::Cancelled) {
                closed = Some("finished".to_owned());
            }
            if closed.is_some() || tokio::time::Instant::now() >= deadline {
                return Ok(FacadeAnswer::Events {
                    job,
                    events: Vec::new(),
                    next: after,
                    lagged: false,
                    closed,
                });
            }
            tokio::select! {
                changed = commits.changed() => {
                    if changed.is_err() {
                        return Err(Refusal::new(RefusalCode::Unavailable, "authority"));
                    }
                }
                () = tokio::time::sleep_until(deadline.min(expiry)) => {}
            }
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let live = self.token_record(&bearer).await?;
            let check = check_token(
                live.as_ref(),
                now_ms.saturating_add(elapsed),
                JobAsk::Existing(job),
                ResourceAsk::Nothing,
            );
            if check != TokenCheck::Ok {
                closed = Some("revoked".to_owned());
            }
        }
    }

    /// Sends one facade message to `authority`: this node's own authority
    /// without the link, another node's over it, and answers what it
    /// answered. A link that fails answers `unavailable`/`link`, which
    /// says nothing about the command.
    pub(crate) async fn facade_forward(
        self: &Arc<Self>,
        authority: NodeId,
        message: &Value,
    ) -> Value {
        let bytes = serde_json::to_vec(message).unwrap_or_default();
        if authority == self.id {
            let answer = self.facade_serve(&bytes).await;
            return serde_json::from_slice(&answer).unwrap_or(Value::Null);
        }
        let wait = message
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(u64::from(DEFAULT_WAIT_MS));
        let deadline = Duration::from_millis(wait) + LINK_SLACK;
        let message = match fabric_net::wire::FacadeMessage::new(bytes) {
            Ok(message) => message,
            Err(e) => {
                return Refusal::bad_request("bad_string")
                    .saying(e.to_string())
                    .to_json();
            }
        };
        let answer = self
            .call(authority, |session| {
                let message = message.clone();
                async move { session.facade(message, deadline).await }
            })
            .await;
        match answer {
            Ok(answer) => serde_json::from_slice(answer.as_bytes()).unwrap_or_else(|e| {
                Refusal::new(RefusalCode::Unavailable, "link")
                    .saying(format!("the authority's answer was not JSON: {e}"))
                    .to_json()
            }),
            Err(e) => Refusal::new(RefusalCode::Unavailable, "link")
                .saying(e.to_string())
                .to_json(),
        }
    }

    /// Mints a token on this node's authority; the bearer is returned once.
    pub(crate) async fn facade_mint(
        &self,
        actor: &str,
        jobs: JobScope,
        resources: ResourceScope,
        expires_ms: u64,
    ) -> Result<Minted, NodeError> {
        let actor =
            Actor::parse(actor).map_err(|e| refused(Reason::BadRequest, format!("actor: {e}")))?;
        let now = now_ms();
        if expires_ms <= now {
            return Err(refused(
                Reason::BadRequest,
                format!("expires_ms {expires_ms} is not after now ({now})"),
            ));
        }
        let bearer = random_bearer()?;
        let record = FacadeTokenRecord {
            id: TokenId::from_bytes(Ulid::generate().to_bytes()),
            digest: token_digest(&bearer),
            actor,
            jobs,
            resources,
            minted_ms: now,
            expires_ms,
            revoked_ms: None,
        };
        let id = record.id;
        self.authority
            .write_journal(true, move |journal| {
                journal.token_mint(&record).map_err(AuthorityError::from)
            })
            .await?;
        Ok(Minted {
            token_id: id.to_string(),
            bearer: hex(&bearer),
        })
    }

    /// Revokes a token; `false` when it is unknown or already revoked.
    pub(crate) async fn facade_revoke(&self, token_id: &str) -> Result<Revoked, NodeError> {
        let id: TokenId = token_id.parse().map_err(|_| {
            refused(
                Reason::BadRequest,
                format!("{token_id:?} is not a token id"),
            )
        })?;
        let now = now_ms();
        let revoked = match self
            .authority
            .write_journal(false, move |journal| {
                journal.token_revoke(id, now).map_err(AuthorityError::from)
            })
            .await
        {
            Ok(revoked) => revoked,
            Err(AuthorityError::Unavailable(reason))
                if reason.starts_with("no managed journal") =>
            {
                false
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Revoked { revoked })
    }

    /// Everything the journal holds about `job`, in one read.
    pub(crate) async fn inspect(&self, job: JobId) -> Result<Inspection, NodeError> {
        let read = self
            .authority
            .with_journal(false, move |journal| {
                let Some(state) = journal.job(job)? else {
                    return Ok(None);
                };
                Ok(Some(read_inspection(journal, &state)?))
            })
            .await;
        match read {
            Ok(Some(inspection)) => Ok(inspection),
            Ok(None) => Err(NodeError::UnknownJob(job)),
            Err(AuthorityError::Unavailable(reason))
                if reason.starts_with("no managed journal") =>
            {
                Err(NodeError::UnknownJob(job))
            }
            Err(e) => Err(e.into()),
        }
    }
}

fn inspect_refusal(error: &NodeError) -> Refusal {
    match error {
        NodeError::UnknownJob(_) => Refusal::new(RefusalCode::UnknownJob, "job"),
        other => Refusal::new(RefusalCode::Unavailable, "authority").saying(other.to_string()),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The inspection of `state`, read from the journal in the authority's thread.
fn read_inspection(journal: &Journal, state: &ManagedState) -> Result<Inspection, AuthorityError> {
    let job = state.job.id;
    let mut managed = ManagedJob::from(state);
    // The last receipt of each task: the newest attempt's receipts first.
    let mut receipts: Vec<(fabric_core::managed::TaskId, TaskReceipt)> = Vec::new();
    for attempt in state.attempts.iter().rev() {
        for receipt in journal.receipts(state.job.id, attempt.id)? {
            if !receipts.iter().any(|(task, _)| *task == receipt.task) {
                receipts.push((receipt.task, receipt));
            }
        }
    }
    for (task, wire) in managed.tasks.iter_mut().zip(&state.tasks) {
        task.class = Some(wire.class.name().to_owned());
        task.receipt = receipts
            .iter()
            .find(|(id, _)| *id == wire.id)
            .map(|(_, receipt)| receipt_wire(receipt));
    }
    let placements = journal
        .placements(job)?
        .iter()
        .map(|(seq, decision)| placement_wire(*seq, decision))
        .collect();
    let continuations = journal
        .continuations(job)?
        .into_iter()
        .map(|(attempt, from, achieved, package)| ContinuationWire {
            attempt,
            from,
            achieved: achieved.name().to_owned(),
            package: package.map(|p| hex(&p.0)),
        })
        .collect();
    let notes = journal
        .notes(job)?
        .iter()
        .map(|(revision, note)| NoteAt {
            revision: *revision,
            note: note_wire(note),
        })
        .collect();
    let provider_operations = journal
        .provider_operations(job)?
        .iter()
        .map(provider_wire)
        .collect();
    let blockers = journal
        .replacement_blockers(job)?
        .iter()
        .map(blocker_wire)
        .collect();
    let residuals = journal.residuals(job)?.iter().map(residual_wire).collect();
    Ok(Inspection {
        parked: state.parked(),
        job: managed,
        placements,
        continuations,
        notes,
        provider_operations,
        blockers,
        residuals,
    })
}

fn provider_wire(op: &ProviderOperation) -> ProviderOperationWire {
    ProviderOperationWire {
        operation: op.dispatch.operation,
        attempt: op.dispatch.attempt,
        generation: op.dispatch.generation,
        profile: op.dispatch.profile.clone(),
        action: op.dispatch.action.clone(),
        effect: op.dispatch.effect.name().to_owned(),
        status: op.status.name().to_owned(),
        locator: op.resource().map(|l| l.text.clone()),
        cleans: match op.dispatch.purpose {
            fabric_journal::Purpose::Effect => None,
            fabric_journal::Purpose::Cleanup { cleans } => Some(cleans),
        },
        purpose: Some(
            match op.dispatch.purpose {
                fabric_journal::Purpose::Effect => "effect",
                fabric_journal::Purpose::Cleanup { .. } => "cleanup",
            }
            .to_owned(),
        ),
        target: op.dispatch.target.as_ref().map(|t| t.text.clone()),
        searched: op.searched.clone(),
        abandon_reason: op.abandon_reason.clone(),
    }
}

fn blocker_wire(blocker: &Blocker) -> BlockerWire {
    let (kind, status) = match blocker {
        Blocker::CreateUnresolved { status, .. } => ("create_unresolved", status),
        Blocker::ExecMayRun { status, .. } => ("exec_may_run", status),
        Blocker::StopNotConfirmed { status, .. } => ("stop_not_confirmed", status),
    };
    BlockerWire {
        kind: kind.to_owned(),
        operation: blocker.operation(),
        status: status.name().to_owned(),
    }
}

fn residual_wire(residual: &Residual) -> ResidualWire {
    ResidualWire {
        creating: residual.creating,
        status: residual.status.name().to_owned(),
        locator: residual.locator.as_ref().map(|l| l.text.clone()),
        searched: residual.searched.clone(),
    }
}

/// A receipt as the client API shows it.
pub(crate) fn receipt_wire(receipt: &TaskReceipt) -> ReceiptWire {
    let (status, snapshot, reason) = match &receipt.status {
        ReceiptStatus::Running => ("running", None, None),
        ReceiptStatus::Exited => ("exited", None, None),
        ReceiptStatus::Interrupted => ("interrupted", None, None),
        ReceiptStatus::Checkpointed { snapshot } => ("checkpointed", Some(*snapshot), None),
        ReceiptStatus::Blocked { reason } => ("blocked", None, Some(reason.clone())),
    };
    ReceiptWire {
        status: status.to_owned(),
        exit: receipt.exit,
        interrupted: receipt.interrupted,
        snapshot,
        reason,
    }
}

/// A note as the client API shows it.
pub(crate) fn note_wire(note: &Note) -> NoteWire {
    match note {
        Note::Decision { what, reason } => NoteWire::Decision {
            what: what.clone(),
            reason: reason.clone(),
        },
        Note::Question { text } => NoteWire::Question { text: text.clone() },
        Note::Answer { question, text } => NoteWire::Answer {
            question: *question,
            text: text.clone(),
        },
    }
}

/// A placement decision as the client API shows it (ADR 0026).
pub(crate) fn placement_wire(seq: EventSeq, decision: &PlacementDecision) -> PlacementWire {
    PlacementWire {
        seq,
        kind: decision.kind.name().to_owned(),
        chosen: decision.chosen,
        candidates: decision
            .candidates
            .iter()
            .map(|candidate| CandidateWire {
                node: candidate.node,
                verdict: match &candidate.verdict {
                    Verdict::Eligible { rank } => VerdictWire::Eligible {
                        rank: rank.iter().map(rank_wire).collect(),
                    },
                    Verdict::Excluded { reasons } => VerdictWire::Excluded {
                        reasons: reasons.iter().map(exclusion_wire).collect(),
                    },
                },
            })
            .collect(),
    }
}

fn rank_wire(reason: &RankReason) -> RankWire {
    match reason {
        RankReason::CurrentHealthy => RankWire::Named("current_healthy".into()),
        RankReason::BundleChain => RankWire::Named("bundle_chain".into()),
        RankReason::Workspace => RankWire::Named("workspace".into()),
        RankReason::Identity => RankWire::Named("identity".into()),
        RankReason::Load { milli } => RankWire::Detailed(object(json!({ "load": milli }))),
    }
}

fn exclusion_wire(reason: &Exclusion) -> ExclusionWire {
    let detailed = |value: Value| ExclusionWire::Detailed(object(value));
    match reason {
        Exclusion::NoAnswer { .. } => ExclusionWire::Named("no_answer".into()),
        Exclusion::Stale { age_ms } => detailed(json!({ "stale": { "age_ms": age_ms } })),
        Exclusion::Platform { need, have } => detailed(json!({ "platform": {
            "need": { "os": need.os, "arch": need.arch },
            "have": { "os": have.os, "arch": have.arch },
        } })),
        Exclusion::HarnessMissing { kind } => {
            detailed(json!({ "harness_missing": { "harness": harness_json(kind) } }))
        }
        Exclusion::HarnessVersion { kind, need, have } => detailed(json!({ "harness_version": {
            "harness": harness_json(kind),
            "need": need.to_string(),
            "have": have.to_string(),
        } })),
        Exclusion::Layout { need, have } => detailed(json!({ "layout": {
            "need": { "home": need.home.as_str(), "root": need.root.as_str() },
            "have": { "home": have.home.as_str(), "root": have.root.as_str() },
        } })),
        Exclusion::Secret { name } => detailed(json!({ "secret": { "name": name.as_str() } })),
        Exclusion::Disk { need, free } => {
            detailed(json!({ "disk": { "need": need, "free": free } }))
        }
        Exclusion::Pin { pinned } => detailed(json!({ "pin": { "pinned": pinned.to_string() } })),
    }
}

fn attempt_status_name(status: &AttemptStatus) -> &'static str {
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

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".into(), other);
            map
        }
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn source_json(source: &SourceBinding) -> Value {
    match source {
        SourceBinding::Repository { path } => {
            json!({ "kind": "repository", "path": path.as_str() })
        }
        SourceBinding::Snapshot { snapshot } => to_json(&ManagedSource::Snapshot {
            snapshot: *snapshot,
        }),
        SourceBinding::Git { uri, commit } => to_json(&ManagedSource::Git {
            uri: uri.as_str().to_owned(),
            commit: commit.as_str().to_owned(),
        }),
        SourceBinding::Empty => to_json(&ManagedSource::Empty),
    }
}

fn fidelity_names(allowed: AllowedFidelity) -> Vec<FidelityName> {
    let mut out = Vec::new();
    if allowed.native_session {
        out.push(FidelityName::NativeSession);
    }
    if allowed.new_native_session_with_package {
        out.push(FidelityName::NewNativeSessionWithPackage);
    }
    if allowed.workspace_only {
        out.push(FidelityName::WorkspaceOnly);
    }
    out
}

fn policy_json(policy: &JobPolicy) -> Value {
    to_json(&ManagedPolicy {
        drain_budget_ms: Some(policy.drain_budget_ms),
        allowed_fidelity: Some(fidelity_names(policy.allowed_fidelity)),
        secrets: Some(
            policy
                .secrets
                .iter()
                .map(|s| fabric_client::api::SecretRefWire {
                    name: s.name.as_str().to_owned(),
                    attempt: match s.scope.attempt {
                        AttemptScope::Any => "any".into(),
                        AttemptScope::Current => "current".into(),
                    },
                    resource_kinds: s
                        .scope
                        .resource_kinds
                        .iter()
                        .map(|ResourceKind::Node| "node".to_owned())
                        .collect(),
                })
                .collect(),
        ),
        export_allowlist: Some(
            policy
                .export_allowlist
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect(),
        ),
        disk_bytes: Some(policy.placement.disk_bytes),
        pin: policy.placement.pin,
    })
}

/// A task's program as the client API writes it (docs/facade-protocol.md,
/// "Launch path"): the arguments, the directory under the root and the
/// variables of the node's environment it may see.
fn program_json(program: &fabric_core::managed::Program) -> Value {
    json!({
        "argv": program.argv,
        "cwd": program.cwd.as_ref().map(fabric_core::path::RelPath::as_str),
        "env_allowlist": program.env_allowlist.iter().map(fabric_core::env::EnvVarName::as_str).collect::<Vec<_>>(),
    })
}

fn blocker_json(blocker: &HandoffBlocker) -> Value {
    match blocker {
        HandoffBlocker::EscapedChild { pid, group } => {
            json!({ "escaped_child": { "pid": pid, "group": group } })
        }
        HandoffBlocker::UncontrolledWriter { pid, path } => {
            json!({ "uncontrolled_writer": { "pid": pid, "path": path.as_str() } })
        }
        HandoffBlocker::CheckpointFailed { task, reason } => {
            json!({ "checkpoint_failed": { "task": task.to_string(), "reason": reason } })
        }
        HandoffBlocker::Opaque { task } => json!({ "opaque": { "task": task.to_string() } }),
        HandoffBlocker::UnknownEffect { operation } => {
            json!({ "unknown_effect": { "operation": operation.to_string() } })
        }
    }
}

fn receipts_json(receipts: &[TaskReceipt]) -> Value {
    Value::Array(
        receipts
            .iter()
            .map(|r| {
                let mut wire = to_json(&receipt_wire(r));
                if let Value::Object(map) = &mut wire {
                    map.insert("task".into(), json!(r.task.to_string()));
                    map.insert("class".into(), json!(r.class.name()));
                }
                wire
            })
            .collect(),
    )
}

/// One event as the client API writes it: its `kind` and its fields,
/// every id in its text form and every counter as a decimal string.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per event kind, so the whole rendering reads in one place"
)]
pub(crate) fn event_json(frame: &EventFrame) -> Map<String, Value> {
    let kind = frame.event.name();
    let body = match &frame.event {
        ManagedEvent::CommandAccepted {
            operation,
            key,
            kind,
            effect,
            ..
        } => json!({
            "operation": operation.to_string(),
            "key": key.as_str(),
            "command": kind.name(),
            "effect": effect,
        }),
        ManagedEvent::JobCreated {
            job,
            intent,
            source,
            policy,
            primary,
        } => json!({
            "job": job.to_string(),
            "intent": {
                "goal": intent.goal,
                "harness": harness_json(&intent.harness),
                "acceptance": intent.acceptance,
            },
            "source": source_json(source),
            "policy": policy_json(policy),
            "primary": primary.to_string(),
        }),
        ManagedEvent::TaskRegistered {
            task,
            parent,
            kind,
            restart,
            class,
            program,
        } => json!({
            "task": task.to_string(),
            "parent": parent.map(|p| p.to_string()),
            "label": match kind {
                TaskKind::Primary => Value::Null,
                TaskKind::Child { label } => json!(label),
            },
            "restart": match restart {
                RestartPolicy::Block => "block",
                RestartPolicy::ExplicitOnly => "explicit_only",
            },
            "class": class.name(),
            "program": program.as_ref().map(program_json),
        }),
        ManagedEvent::AttemptGranted {
            attempt,
            task,
            resource,
            generation,
            initial_checkpoint,
            operation,
        } => json!({
            "attempt": attempt.to_string(),
            "task": task.to_string(),
            "node": match resource {
                fabric_core::managed::ResourceRef::Node { node } => node.to_string(),
            },
            "generation": generation.to_string(),
            "initial_checkpoint": initial_checkpoint.map(|s| s.to_string()),
            "operation": operation.to_string(),
        }),
        ManagedEvent::AttemptObserved {
            attempt,
            generation,
            status,
        } => {
            let mut map = object(json!({
                "attempt": attempt.to_string(),
                "generation": generation.to_string(),
                "status": attempt_status_name(status),
            }));
            if let AttemptStatus::Checkpointed { snapshot } = status {
                map.insert("snapshot".into(), json!(snapshot.to_string()));
            }
            Value::Object(map)
        }
        ManagedEvent::TaskFinished { task, outcome } => json!({
            "task": task.to_string(),
            "outcome": format!("{outcome:?}").to_lowercase(),
        }),
        ManagedEvent::JobCancelled { reason } => json!({ "reason": reason }),
        ManagedEvent::StaleObservationRefused {
            attempt,
            generation,
            current,
        } => json!({
            "attempt": attempt.to_string(),
            "generation": generation.to_string(),
            "current": current.to_string(),
        }),
        ManagedEvent::OperationMarked { operation, status } => json!({
            "operation": operation.to_string(),
            "status": status.status().name(),
        }),
        ManagedEvent::NoteRecorded { note } => to_json(&note_wire(note)),
        ManagedEvent::ReceiptsRecorded { attempt, receipts } => json!({
            "attempt": attempt.to_string(),
            "receipts": receipts_json(receipts),
        }),
        ManagedEvent::CheckpointCutRecorded {
            attempt,
            snapshot,
            cut,
            receipts,
        } => json!({
            "attempt": attempt.to_string(),
            "snapshot": snapshot.to_string(),
            "cut": hex(&cut.0),
            "receipts": receipts_json(receipts),
        }),
        ManagedEvent::HandoffBlocked { attempt, blockers } => json!({
            "attempt": attempt.to_string(),
            "blockers": blockers.iter().map(blocker_json).collect::<Vec<_>>(),
        }),
        ManagedEvent::PlacementDecided { task, decision } => {
            let mut map = object(to_json(&placement_wire(EventSeq::FIRST, decision)));
            map.remove("seq");
            map.insert("task".into(), json!(task.to_string()));
            map.insert("decided_at_ms".into(), json!(decision.decided_at_ms));
            Value::Object(map)
        }
        ManagedEvent::ContinuationRecorded {
            attempt,
            from,
            requested,
            achieved,
            package,
        } => json!({
            "attempt": attempt.to_string(),
            "from": from.to_string(),
            "requested": fidelity_names(*requested),
            "achieved": achieved.name(),
            "package": package.map(|p| hex(&p.0)),
        }),
        ManagedEvent::EffectRequested { operation, request } => json!({
            "operation": operation.to_string(),
            "request": match request {
                EffectRequest::CancelJob { reason } => json!({ "cancel_job": { "reason": reason } }),
                EffectRequest::CancelTask { task, reason } => {
                    json!({ "cancel_task": { "task": task.to_string(), "reason": reason } })
                }
                EffectRequest::Checkpoint { attempt } => {
                    json!({ "checkpoint": { "attempt": attempt.to_string() } })
                }
                EffectRequest::Continue { task, fidelity, pin } => json!({ "continue": {
                    "task": task.to_string(),
                    "fidelity": fidelity_names(*fidelity),
                    "pin": pin.map(|n| n.to_string()),
                } }),
                EffectRequest::Spawn { task } => json!({ "spawn": { "task": task.to_string() } }),
            },
        }),
    };
    let mut map = object(body);
    // An absent optional field is absent, not null (docs/facade-protocol.md).
    map.retain(|_, v| !v.is_null());
    map.insert("kind".into(), json!(kind));
    if let Some(operation) = frame.operation {
        map.entry("operation".to_owned())
            .or_insert_with(|| json!(operation.to_string()));
    }
    map
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fabric_core::managed::facade::{JobScope, ResourceScope};
    use fabric_core::managed::{Command, ResourceRef};
    use serde_json::{Value, json};

    use super::*;
    use crate::ops::tests::node;

    const NOW: u64 = 1_762_128_000_000;

    fn bearer(fill: u8) -> [u8; BEARER_BYTES] {
        [fill; BEARER_BYTES]
    }

    fn hex_of(fill: u8) -> String {
        hex(&bearer(fill))
    }

    /// Mints a token with a known bearer, as the fixtures give them.
    async fn seed_token(node: &Node, bearer: [u8; BEARER_BYTES], record: FacadeTokenRecord) {
        let record = FacadeTokenRecord {
            digest: token_digest(&bearer),
            ..record
        };
        node.authority
            .write_journal(true, move |j| {
                j.token_mint(&record).map_err(AuthorityError::from)
            })
            .await
            .unwrap();
    }

    fn any_token(id: u8, actor: &str, expires_ms: u64) -> FacadeTokenRecord {
        FacadeTokenRecord {
            id: TokenId::from_bytes([id; 16]),
            digest: token_digest(&bearer(0)),
            actor: Actor::parse(actor).unwrap(),
            jobs: JobScope::AnyOwned,
            resources: ResourceScope::Any,
            minted_ms: NOW - 1_000,
            expires_ms,
            revoked_ms: None,
        }
    }

    fn submit_json(token: &str, key: &str, goal: &str) -> Value {
        json!({
            "type": "submit", "facade": 1, "token": token, "key": key,
            "intent": { "goal": goal, "harness": { "kind": "codex" }, "acceptance": [] },
            "source": { "kind": "empty" }
        })
    }

    async fn submit(node: &Node, token: &str, key: &str, goal: &str) -> FacadeAnswer {
        node.facade_answer(&submit_json(token, key, goal), NOW)
            .await
    }

    fn refused_as(answer: &FacadeAnswer, code: RefusalCode, reason: &str) {
        match answer {
            FacadeAnswer::Refused(refusal) => {
                assert_eq!(
                    (refusal.code, refusal.reason.as_str()),
                    (code, reason),
                    "{refusal}"
                );
            }
            other => panic!("not refused: {other:?}"),
        }
    }

    fn accepted(answer: &FacadeAnswer) -> (JobId, bool) {
        match answer {
            FacadeAnswer::Accepted { job, replayed, .. } => (*job, *replayed),
            other => panic!("not accepted: {other:?}"),
        }
    }

    async fn jobs_held(node: &Node) -> usize {
        node.authority.list().await.unwrap().len()
    }

    /// A note bumps the job's revision and adds events, for cursors and revisions.
    async fn note(node: &Node, job: JobId, n: usize) {
        let envelope = Envelope {
            key: OperationKey::parse(&format!("note-{n}")).unwrap(),
            job: Some(job),
            expected: None,
            body: Command::RecordNote {
                note: Note::Question {
                    text: format!("question {n}"),
                },
            },
        };
        let answer = node.authority.submit(envelope).await.unwrap();
        assert!(matches!(answer.result, StoredResult::Accepted(_)));
    }

    #[tokio::test]
    async fn a_token_out_of_scope_is_refused_before_the_authority_is_asked() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "first").await);
        let other = JobId::from_bytes([9; 16]);
        let scoped = FacadeTokenRecord {
            jobs: JobScope::Jobs(vec![job]),
            resources: ResourceScope::Nodes(Vec::new()),
            ..any_token(2, "ci", NOW + 1_000)
        };
        seed_token(&node, bearer(2), scoped).await;
        let before = jobs_held(&node).await;
        // A job outside the scope is `scope`, not `unknown_job`: the token learns nothing.
        let inspect =
            json!({ "type": "inspect", "facade": 1, "token": hex_of(2), "job": other.to_string() });
        refused_as(
            &node.facade_answer(&inspect, NOW).await,
            RefusalCode::Scope,
            "job",
        );
        // A token limited to listed jobs creates none.
        refused_as(
            &submit(&node, &hex_of(2), "k-new", "new").await,
            RefusalCode::Scope,
            "job",
        );
        // A continue with no pin, from a token limited to listed nodes,
        // could be placed anywhere: refused.
        let unpinned = json!({ "type": "continue", "facade": 1, "token": hex_of(2),
            "key": "k-cont", "job": job.to_string(), "expected_revision": null, "pin": null });
        refused_as(
            &node.facade_answer(&unpinned, NOW).await,
            RefusalCode::Scope,
            "resource",
        );
        let nodes_only = FacadeTokenRecord {
            jobs: JobScope::AnyOwned,
            resources: ResourceScope::Nodes(vec![NodeId::from_bytes([9; 32])]),
            ..any_token(3, "ci-nodes", NOW + 1_000)
        };
        seed_token(&node, bearer(3), nodes_only).await;
        // A pin outside the resource scope is refused before the commit,
        // and so is no pin at all.
        let mut pinned = submit_json(&hex_of(3), "k-pin", "pinned");
        pinned["policy"] = json!({ "pin": node.id.to_string() });
        refused_as(
            &node.facade_answer(&pinned, NOW).await,
            RefusalCode::Scope,
            "resource",
        );
        refused_as(
            &submit(&node, &hex_of(3), "k-anywhere", "anywhere").await,
            RefusalCode::Scope,
            "resource",
        );
        assert_eq!(jobs_held(&node).await, before, "nothing was written");
        // The scoped token reaches its own job.
        let inspect =
            json!({ "type": "inspect", "facade": 1, "token": hex_of(2), "job": job.to_string() });
        assert!(matches!(
            node.facade_answer(&inspect, NOW).await,
            FacadeAnswer::Inspection(_)
        ));
    }

    #[tokio::test]
    async fn an_unknown_expired_or_revoked_token_is_refused_in_that_order() {
        let (_dir, node) = node();
        // No journal yet: an unknown token is unknown, not an error.
        refused_as(
            &submit(&node, &hex_of(7), "k", "g").await,
            RefusalCode::Unauthorized,
            "unknown_token",
        );
        seed_token(&node, bearer(3), any_token(3, "efe", NOW - 1)).await;
        refused_as(
            &submit(&node, &hex_of(3), "k", "g").await,
            RefusalCode::Unauthorized,
            "expired",
        );
        let revoked = FacadeTokenRecord {
            revoked_ms: Some(NOW - 5),
            ..any_token(4, "efe", NOW - 1)
        };
        seed_token(&node, bearer(4), revoked).await;
        refused_as(
            &submit(&node, &hex_of(4), "k", "g").await,
            RefusalCode::Unauthorized,
            "revoked",
        );
        refused_as(
            &submit(&node, &hex_of(8), "k", "g").await,
            RefusalCode::Unauthorized,
            "unknown_token",
        );
        assert_eq!(jobs_held(&node).await, 0);
    }

    #[tokio::test]
    async fn a_duplicate_key_reconciles_one_operation() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let first = submit(&node, &hex_of(1), "k1", "same").await;
        let again = submit(&node, &hex_of(1), "k1", "same").await;
        let (job, replayed) = accepted(&first);
        let (job_again, replayed_again) = accepted(&again);
        assert!(!replayed && replayed_again);
        assert_eq!(job, job_again);
        assert_eq!(jobs_held(&node).await, 1);
        // The key is the actor's: another actor's same key is another operation.
        seed_token(&node, bearer(2), any_token(2, "ci", NOW + 1_000)).await;
        let (other, replayed) = accepted(&submit(&node, &hex_of(2), "k1", "same").await);
        assert!(!replayed);
        assert_ne!(other, job);
        assert_eq!(jobs_held(&node).await, 2);
    }

    #[tokio::test]
    async fn the_same_key_with_another_request_is_a_conflict() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        accepted(&submit(&node, &hex_of(1), "k1", "one").await);
        refused_as(
            &submit(&node, &hex_of(1), "k1", "two").await,
            RefusalCode::Conflict,
            "key",
        );
        assert_eq!(jobs_held(&node).await, 1);
    }

    #[tokio::test]
    async fn a_cancel_is_a_command_with_its_own_outcome() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        let cancel = json!({
            "type": "cancel", "facade": 1, "token": hex_of(1), "key": "c1",
            "job": job.to_string(), "expected_revision": null, "task": null, "reason": "stop"
        });
        let (_, replayed) = accepted(&node.facade_answer(&cancel, NOW).await);
        assert!(!replayed);
        let (_, replayed) = accepted(&node.facade_answer(&cancel, NOW).await);
        assert!(replayed, "the same cancel again is the same operation");
        let state = node.authority.job(job).await.unwrap().unwrap();
        assert_eq!(state.job.state, JobState::Cancelled);
        // A revision the job has moved past is named with the current one.
        let behind = json!({
            "type": "checkpoint", "facade": 1, "token": hex_of(1), "key": "cp1",
            "job": job.to_string(), "expected_revision": "1"
        });
        match node.facade_answer(&behind, NOW).await {
            FacadeAnswer::Refused(refusal) => {
                assert!(
                    matches!(
                        refusal.code,
                        RefusalCode::RevisionMismatch | RefusalCode::Refused
                    ),
                    "{refusal}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    async fn watch(
        node: &Node,
        token: &str,
        job: JobId,
        after: Option<u64>,
        wait_ms: u32,
    ) -> FacadeAnswer {
        let message = json!({
            "type": "watch", "facade": 1, "token": token, "job": job.to_string(),
            "after": after.map(|n| n.to_string()), "wait_ms": wait_ms
        });
        node.facade_answer(&message, NOW).await
    }

    #[tokio::test]
    async fn a_lagged_reader_gets_a_resume_cursor_and_the_authority_holds_no_more_than_the_window()
    {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        for n in 0..200 {
            note(&node, job, n).await;
        }
        let FacadeAnswer::Events {
            events,
            next,
            lagged,
            closed,
            ..
        } = watch(&node, &hex_of(1), job, None, 0).await
        else {
            panic!("not events");
        };
        assert_eq!(events.len(), WATCH_WINDOW);
        assert!(lagged, "more than the window was behind the cursor");
        assert_eq!(closed, None);
        let next = next.unwrap();
        assert_eq!(next, events.last().unwrap().seq);
        assert!(events.windows(2).all(|w| w[0].seq < w[1].seq), "in order");
        // The rest, from the cursor, with nothing lost and nothing twice.
        let FacadeAnswer::Events {
            events: rest,
            lagged,
            ..
        } = watch(&node, &hex_of(1), job, Some(next.get()), 0).await
        else {
            panic!("not events");
        };
        assert!(!lagged);
        assert!(rest.first().unwrap().seq > next);
        let total = events.len() + rest.len();
        let all = node
            .authority
            .with_journal(false, move |j| Ok(j.events(job, None, u32::MAX)?.len()))
            .await
            .unwrap();
        assert_eq!(total, all);
    }

    /// Grants and starts an attempt of `job`'s primary task, as an
    /// executor's report would, and returns it.
    async fn live_attempt(node: &Node, job: JobId, key: &str) -> fabric_core::managed::Attempt {
        let state = node.authority.job(job).await.unwrap().unwrap();
        let start = Envelope {
            key: OperationKey::parse(key).unwrap(),
            job: Some(job),
            expected: None,
            body: Command::StartAttempt {
                task: state.job.primary,
                resource: ResourceRef::Node { node: node.id },
                initial_checkpoint: None,
            },
        };
        let answer = node.authority.submit(start).await.unwrap();
        assert!(
            matches!(answer.result, StoredResult::Accepted(_)),
            "{answer:?}"
        );
        let attempt = node
            .authority
            .job(job)
            .await
            .unwrap()
            .unwrap()
            .attempts
            .last()
            .unwrap()
            .clone();
        node.authority
            .observe(
                job,
                fabric_core::managed::Observation::Started {
                    attempt: attempt.id,
                    generation: attempt.generation,
                },
            )
            .await
            .unwrap();
        attempt
    }

    /// The provider read surface (carried from M4): one operation in each
    /// status, `unknown` with and without the scope an absent attach
    /// searched, a blocker of each kind and a residual of each kind, read
    /// through `managed_inspect` with every value as the journal holds it,
    /// and the journal unchanged by the read.
    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "the seed reads best as one script")]
    async fn an_inspection_lists_every_provider_status() {
        use fabric_core::managed::{OperationId, RequestDigest};
        use fabric_journal::{
            AbandonApproval, Acknowledged, EffectClass, LocatorRecord, ProviderAnswer,
            ProviderDispatch, ProviderStatus, Purpose, ResourceKey,
        };

        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        let attempt = live_attempt(&node, job, "start-1").await;
        let locator = LocatorRecord {
            text: r#"{"kind":"sandbox","native_id":"sb-1","provider":"fixture","scope":{"project":"p-1"}}"#.into(),
            key: ResourceKey::from_bytes([0x45; 32]),
        };
        let searched = r#"{"kind":"sandbox","provider":"fixture","scope":{"project":"p-1"}}"#;
        let op = |n: u8| OperationId::from_bytes([0x40 + n; 16]);
        let dispatch = move |n: u8, action: &str, target: Option<LocatorRecord>| ProviderDispatch {
            operation: op(n),
            key: OperationKey::parse(&format!("provider-{n}")).unwrap(),
            digest: RequestDigest::from_bytes([n; 32]),
            job,
            task: attempt.task,
            attempt: attempt.id,
            generation: attempt.generation,
            profile: "fixture".into(),
            action: action.into(),
            effect: match action {
                "sandbox.create" => EffectClass::Create,
                "sandbox.destroy" => EffectClass::Destroy,
                _ => EffectClass::Mutate,
            },
            purpose: Purpose::Effect,
            target,
        };
        let answer = |code: &str| ProviderAnswer {
            code: Some(code.into()),
            reason: Some(code.into()),
            effect: Some("unknown".into()),
        };
        let (l, s) = (locator.clone(), searched.to_owned());
        node.authority
            .write_journal(false, move |j| {
                let at = NOW;
                // The creates first: an unknown one refuses another create.
                for (n, _) in [
                    (1, "dispatched"),
                    (2, "unknown"),
                    (3, "attached"),
                    (4, "searched"),
                ] {
                    j.provider_begin(&dispatch(n, "sandbox.create", None), None, at)?;
                }
                j.provider_record(op(2), ProviderStatus::Unknown, &answer("timeout"), None, at)?;
                j.provider_record(op(3), ProviderStatus::Unknown, &answer("timeout"), None, at)?;
                j.provider_record(
                    op(3),
                    ProviderStatus::Attached,
                    &ProviderAnswer::default(),
                    Some(&l),
                    at,
                )?;
                j.provider_record(op(4), ProviderStatus::Unknown, &answer("timeout"), None, at)?;
                j.provider_searched(op(4), &answer("not_found"), &s, at)?;
                let exec = |n: u8| dispatch(n, "sandbox.exec", Some(l.clone()));
                j.provider_begin(&exec(5), None, at)?;
                j.provider_record(op(5), ProviderStatus::Unknown, &answer("timeout"), None, at)?;
                j.provider_record(
                    op(5),
                    ProviderStatus::Observed,
                    &ProviderAnswer::default(),
                    None,
                    at,
                )?;
                for (n, status) in [
                    (6, ProviderStatus::Succeeded),
                    (7, ProviderStatus::Failed),
                    (8, ProviderStatus::Cancelled),
                ] {
                    j.provider_begin(&exec(n), None, at)?;
                    j.provider_record(op(n), status, &ProviderAnswer::default(), None, at)?;
                }
                j.provider_begin(&exec(9), Some(&answer("denied")), at)?;
                j.provider_begin(&exec(10), None, at)?;
                j.provider_record(
                    op(10),
                    ProviderStatus::Unknown,
                    &answer("timeout"),
                    None,
                    at,
                )?;
                j.provider_abandon(
                    &AbandonApproval {
                        operation: op(10),
                        expected: ProviderStatus::Unknown,
                        acknowledged: Acknowledged::Resource(l.key),
                        reason: "the provider console shows it ended".into(),
                    },
                    at,
                )?;
                j.provider_begin(&dispatch(11, "sandbox.destroy", Some(l.clone())), None, at)?;
                Ok(())
            })
            .await
            .unwrap();
        let before = node
            .authority
            .with_journal(false, move |j| {
                Ok((
                    j.events(job, None, u32::MAX)?.len(),
                    j.provider_operations(job)?,
                ))
            })
            .await
            .unwrap();

        let inspection = node.inspect(job).await.unwrap();
        let status_of = |n: u8| {
            inspection
                .provider_operations
                .iter()
                .find(|o| o.operation == op(n))
                .map(|o| o.status.as_str())
        };
        let statuses: std::collections::BTreeSet<&str> = inspection
            .provider_operations
            .iter()
            .map(|o| o.status.as_str())
            .collect();
        let every: std::collections::BTreeSet<&str> =
            ProviderStatus::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(statuses, every, "each status shown as it is, none folded");
        assert_eq!(status_of(2), Some("unknown"));
        assert_eq!(status_of(4), Some("unknown"));
        let read = |n: u8| {
            inspection
                .provider_operations
                .iter()
                .find(|o| o.operation == op(n))
                .unwrap()
        };
        assert_eq!(read(2).searched, None, "unknown without a search");
        assert_eq!(
            read(4).searched.as_deref(),
            Some(searched),
            "unknown with its search"
        );
        assert_eq!(read(3).locator.as_deref(), Some(locator.text.as_str()));
        assert_eq!(read(5).target.as_deref(), Some(locator.text.as_str()));
        assert_eq!(read(5).effect, "mutate");
        assert_eq!(read(1).action, "sandbox.create");
        assert_eq!(read(1).purpose.as_deref(), Some("effect"));
        assert_eq!(read(1).cleans, None);
        assert_eq!(
            read(10).abandon_reason.as_deref(),
            Some("the provider console shows it ended")
        );
        assert_eq!(read(6).abandon_reason, None);
        let kinds: std::collections::BTreeSet<&str> = inspection
            .blockers
            .iter()
            .map(|b| b.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            ["create_unresolved", "exec_may_run", "stop_not_confirmed"]
                .into_iter()
                .collect()
        );
        for blocker in &inspection.blockers {
            assert_eq!(
                Some(blocker.status.as_str()),
                status_of((0..=11).find(|n| op(*n) == blocker.operation).unwrap())
            );
        }
        let with_locator = inspection
            .residuals
            .iter()
            .find(|r| r.creating == op(3))
            .unwrap();
        assert_eq!(with_locator.locator.as_deref(), Some(locator.text.as_str()));
        let searched_only = inspection
            .residuals
            .iter()
            .find(|r| r.creating == op(4))
            .unwrap();
        assert_eq!(
            (
                searched_only.locator.as_deref(),
                searched_only.searched.as_deref()
            ),
            (None, Some(searched))
        );

        let after = node
            .authority
            .with_journal(false, move |j| {
                Ok((
                    j.events(job, None, u32::MAX)?.len(),
                    j.provider_operations(job)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(before, after, "the read wrote nothing");
    }

    /// The same checkpoint request under the same key is one operation,
    /// though another attempt became the live one between the two sends:
    /// sameness is the request's, not the body the authority derived.
    #[tokio::test]
    async fn a_retried_checkpoint_replays_after_the_job_moved() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        let first = live_attempt(&node, job, "start-1").await;
        let checkpoint = json!({ "type": "checkpoint", "facade": 1, "token": hex_of(1),
            "key": "cp-1", "job": job.to_string(), "expected_revision": null });
        let FacadeAnswer::Accepted {
            operation,
            replayed,
            ..
        } = node.facade_answer(&checkpoint, NOW).await
        else {
            panic!("not accepted");
        };
        assert!(!replayed);
        node.authority
            .observe(
                job,
                fabric_core::managed::Observation::Exited {
                    attempt: first.id,
                    generation: first.generation,
                    outcome: fabric_core::managed::ExitOutcome::Succeeded,
                },
            )
            .await
            .unwrap();
        live_attempt(&node, job, "start-2").await;
        let FacadeAnswer::Accepted {
            operation: again,
            replayed,
            ..
        } = node.facade_answer(&checkpoint, NOW).await
        else {
            panic!("not accepted");
        };
        assert!(replayed);
        assert_eq!(again, operation);
        // Another request under the key is still a conflict.
        let other = json!({ "type": "checkpoint", "facade": 1, "token": hex_of(1),
            "key": "cp-1", "job": job.to_string(), "expected_revision": "1" });
        refused_as(
            &node.facade_answer(&other, NOW).await,
            RefusalCode::Conflict,
            "key",
        );
    }

    /// The owner's keys and the facade's never share a row: the owner's
    /// `efe:k1` and actor `efe`'s `k1` are two operations, and the owner
    /// cannot write a key in the facade's namespace.
    #[tokio::test]
    async fn owner_and_facade_keys_never_collide() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k0", "g").await);
        let owner = node
            .managed_submit(Envelope {
                key: OperationKey::parse("efe:k1").unwrap(),
                job: Some(job),
                expected: None,
                body: Command::RecordNote {
                    note: Note::Question {
                        text: "from the owner".into(),
                    },
                },
            })
            .await
            .unwrap();
        let cancel = json!({ "type": "cancel", "facade": 1, "token": hex_of(1), "key": "k1",
            "job": job.to_string(), "expected_revision": null, "task": null, "reason": "stop" });
        let FacadeAnswer::Accepted {
            operation,
            replayed,
            ..
        } = node.facade_answer(&cancel, NOW).await
        else {
            panic!("not accepted");
        };
        assert!(!replayed);
        assert_ne!(operation, owner.operation);
        let reserved = node
            .managed_submit(Envelope {
                key: journal_key(
                    &Actor::parse("efe").unwrap(),
                    &OperationKey::parse("k2").unwrap(),
                ),
                job: Some(job),
                expected: None,
                body: Command::RecordNote {
                    note: Note::Question { text: "q".into() },
                },
            })
            .await;
        assert!(reserved.is_err(), "{reserved:?}");
    }

    /// A facade `continue` cannot widen the job's fidelity allowance: what
    /// it names is cut to what the policy allows, and nothing left is a
    /// refusal, never a lower fidelity. Only the owner widens it.
    #[tokio::test]
    async fn a_continue_cannot_widen_the_jobs_fidelity_allowance() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let mut create = submit_json(&hex_of(1), "k1", "g");
        create["policy"] = json!({ "allowed_fidelity": ["native_session"], "pin": null });
        let (job, _) = accepted(&node.facade_answer(&create, NOW).await);
        let widen = json!({ "type": "continue", "facade": 1, "token": hex_of(1),
            "key": "k-cont", "job": job.to_string(), "expected_revision": null,
            "pin": null, "fidelity": ["workspace_only"] });
        refused_as(
            &node.facade_answer(&widen, NOW).await,
            RefusalCode::Refused,
            "fidelity",
        );
        let both = json!({ "type": "continue", "facade": 1, "token": hex_of(1),
            "key": "k-cont-2", "job": job.to_string(), "expected_revision": null,
            "pin": null, "fidelity": ["workspace_only", "native_session"] });
        accepted(&node.facade_answer(&both, NOW).await);
        let state = node.authority.job(job).await.unwrap().unwrap();
        let requested = state.requests.iter().find_map(|(_, r)| match r {
            fabric_core::managed::EffectRequest::Continue { fidelity, .. } => Some(*fidelity),
            _ => None,
        });
        assert_eq!(
            requested,
            Some(AllowedFidelity {
                native_session: true,
                new_native_session_with_package: false,
                workspace_only: false,
            })
        );
    }

    /// An ACP harness whose arguments carry a secret is refused before the
    /// authority sees the create, as on the owner's socket: the journal,
    /// `watch` and `inspect` never hold it.
    #[tokio::test]
    async fn a_submit_with_a_secret_in_the_harness_arguments_is_refused() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let message = json!({
            "type": "submit", "facade": 1, "token": hex_of(1), "key": "k-acp",
            "intent": { "goal": "g", "harness": { "kind": "acp", "program": "opencode",
                "args": ["--api-key", "sk-ant-aaaaaaaaaaaaaaaaaaaaaaaa"] }, "acceptance": [] },
            "source": { "kind": "empty" }
        });
        let answer = node.facade_answer(&message, NOW).await;
        refused_as(&answer, RefusalCode::Refused, "credential_like");
        assert_eq!(jobs_held(&node).await, 0);
    }

    /// A watch with nothing to deliver waits on commits, not on its own
    /// reads: the authority's commit counter barely moves while one idle
    /// watcher holds its wait open, so readers never crowd out a writer.
    #[tokio::test]
    async fn an_idle_watch_does_not_spin_the_authority() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        let FacadeAnswer::Events { next, .. } = watch(&node, &hex_of(1), job, None, 0).await else {
            panic!("not events");
        };
        let commits = node.authority.commits().unwrap();
        let before = *commits.borrow();
        let answer = watch(&node, &hex_of(1), job, next.map(EventSeq::get), 500).await;
        let after = *commits.borrow();
        assert!(
            matches!(&answer, FacadeAnswer::Events { events, .. } if events.is_empty()),
            "{answer:?}"
        );
        assert!(
            after.wrapping_sub(before) <= 2,
            "the counter moved {} times for one idle watch",
            after.wrapping_sub(before)
        );
    }

    /// A token that expires while its watch waits closes the watch at the
    /// expiry, not at the end of the wait, though nothing was committed.
    #[tokio::test]
    async fn a_token_expiring_during_a_watch_closes_it_at_the_expiry() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 300)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        let FacadeAnswer::Events { next, .. } = watch(&node, &hex_of(1), job, None, 0).await else {
            panic!("not events");
        };
        let started = tokio::time::Instant::now();
        let answer = watch(&node, &hex_of(1), job, next.map(EventSeq::get), 20_000).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "closed at the expiry"
        );
        let FacadeAnswer::Events { closed, .. } = answer else {
            panic!("not events: {answer:?}");
        };
        assert_eq!(closed.as_deref(), Some("revoked"));
    }

    #[tokio::test]
    async fn a_revoked_token_ends_an_open_watch_within_one_delivery() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        // Drain the events so the watch waits.
        let FacadeAnswer::Events { next, .. } = watch(&node, &hex_of(1), job, None, 0).await else {
            panic!("not events");
        };
        let waiting = {
            let node = Arc::clone(&node);
            tokio::spawn(async move {
                watch(&node, &hex_of(1), job, next.map(EventSeq::get), 20_000).await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        let revoked = node
            .facade_revoke(&TokenId::from_bytes([1; 16]).to_string())
            .await
            .unwrap();
        assert!(revoked.revoked);
        let answer = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        let FacadeAnswer::Events { events, closed, .. } = answer else {
            panic!("not events: {answer:?}");
        };
        assert_eq!(closed.as_deref(), Some("revoked"));
        assert!(events.is_empty());
        refused_as(
            &submit(&node, &hex_of(1), "k2", "g").await,
            RefusalCode::Unauthorized,
            "revoked",
        );
    }

    #[tokio::test]
    async fn a_disconnect_leaves_the_operation_running() {
        let (_dir, node) = node();
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        // A reader that goes away mid-wait costs the authority nothing: the
        // next command and the next read answer as before, and the accepted
        // operation is still the job's.
        let gone = {
            let node = Arc::clone(&node);
            tokio::spawn(
                async move { watch(&node, &hex_of(1), job, Some(u64::MAX / 2), 20_000).await },
            )
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        gone.abort();
        note(&node, job, 1).await;
        let state = node.authority.job(job).await.unwrap().unwrap();
        assert_eq!(state.operations.len(), 2);
        assert!(node.inspect(job).await.is_ok());
    }

    #[tokio::test]
    async fn an_authority_restart_keeps_the_cursor() {
        let dir = tempfile::Builder::new()
            .prefix("ffacade")
            .tempdir_in("/tmp")
            .unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let start = |root: &Path| {
            let home = root.join("home");
            fs::create_dir_all(&home).unwrap();
            Node::start(crate::node::NodeConfig {
                state: crate::paths::StateDir::resolve(Some(root.join("state").as_os_str()), &home)
                    .unwrap(),
                home,
                listen: "127.0.0.1:0".parse().unwrap(),
                dial: fabric_net::endpoint::DialOrder::TcpOnly,
                env: crate::tool_env::ToolEnv::current(),
                otlp: None,
            })
            .unwrap()
        };
        let node = start(&root);
        seed_token(&node, bearer(1), any_token(1, "efe", NOW + 1_000)).await;
        let (job, _) = accepted(&submit(&node, &hex_of(1), "k1", "g").await);
        note(&node, job, 1).await;
        let FacadeAnswer::Events { events, next, .. } =
            watch(&node, &hex_of(1), job, None, 0).await
        else {
            panic!("not events");
        };
        let last = events.last().unwrap().clone();
        let before_last = events[events.len() - 2].seq;
        node.stopping().await;
        drop(node);
        let node = start(&root);
        let FacadeAnswer::Events {
            events: again,
            next: next_again,
            ..
        } = watch(&node, &hex_of(1), job, Some(before_last.get()), 0).await
        else {
            panic!("not events");
        };
        assert_eq!(again, vec![last]);
        assert_eq!(next_again, next);
        let _: PathBuf = root;
    }

    /// Mints every token a case gives, with its bearer, scope, expiry and
    /// revocation; a job scope names fixture ids, mapped through `ids`.
    async fn seed_given_tokens(node: &Node, given: &Value, now: u64, ids: &[(String, String)]) {
        for token in given["tokens"].as_array().unwrap() {
            let bearer_hex = token["bearer"].as_str().unwrap();
            let mut bytes = [0u8; BEARER_BYTES];
            for (i, slot) in bytes.iter_mut().enumerate() {
                *slot = u8::from_str_radix(&bearer_hex[2 * i..2 * i + 2], 16).unwrap();
            }
            let jobs = match &token["jobs"] {
                Value::String(_) => JobScope::AnyOwned,
                Value::Array(list) => JobScope::Jobs(
                    list.iter()
                        .map(|j| {
                            let text = j.as_str().unwrap();
                            let mapped = ids
                                .iter()
                                .find(|(from, _)| from == text)
                                .map_or(text, |(_, to)| to.as_str());
                            mapped.parse().unwrap()
                        })
                        .collect(),
                ),
                other => panic!("{other}"),
            };
            let resources = match &token["resources"] {
                Value::String(_) => ResourceScope::Any,
                Value::Array(list) => ResourceScope::Nodes(
                    list.iter()
                        .map(|n| n.as_str().unwrap().parse().unwrap())
                        .collect(),
                ),
                other => panic!("{other}"),
            };
            let record = FacadeTokenRecord {
                id: token["token_id"].as_str().unwrap().parse().unwrap(),
                digest: token_digest(&bytes),
                actor: Actor::parse(token["actor"].as_str().unwrap()).unwrap(),
                jobs,
                resources,
                minted_ms: now.saturating_sub(1_000),
                expires_ms: token["expires_ms"].as_u64().unwrap(),
                revoked_ms: token["revoked_ms"].as_u64(),
            };
            seed_token(node, bytes, record).await;
        }
    }

    /// Every `client_to_authority` case of protocol/facade/v1 answers as
    /// its fixture expects once the authority holds the state the case
    /// gives: tokens by their bearers, jobs with their revisions and live
    /// attempts, earlier operations under their keys, and an event head.
    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one test seeds every kind of given state the fixtures name, in one place"
    )]
    async fn every_client_case_answers_as_its_fixture_expects() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/facade/v1/cases");
        let mut names: Vec<PathBuf> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        names.sort();
        let mut checked = 0;
        for path in names {
            let case: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if case["direction"] != "client_to_authority" {
                continue;
            }
            let name = case["case"].as_str().unwrap();
            let (_dir, node) = node();
            let given = &case["given"];
            let now = given["now_ms"].as_u64().unwrap();
            // Ids the fixture uses, mapped to the ones this authority made. A
            // given operation whose message is a submit is the one that made
            // the given job in its order, under its own token and key; a job
            // with no such operation is made under a seed token.
            let mut ids: Vec<(String, String)> = Vec::new();
            let operations = given["operations"].as_array().unwrap();
            let mut submits = operations
                .iter()
                .filter(|op| op["digest_of"]["type"] == "submit")
                .collect::<Vec<_>>()
                .into_iter();
            let mut seeded_tokens = false;
            for (n, job) in given["jobs"].as_array().unwrap().iter().enumerate() {
                let n8 = u8::try_from(n).unwrap();
                let want = job["revision"].as_str().unwrap().parse::<u64>().unwrap();
                let made = if let Some(operation) = submits.next() {
                    if !seeded_tokens {
                        seed_given_tokens(&node, given, now, &ids).await;
                        seeded_tokens = true;
                    }
                    accepted(&node.facade_answer(&operation["digest_of"], now).await).0
                } else {
                    seed_token(
                        &node,
                        bearer(200 + n8),
                        any_token(200 + n8, "seed", now + 10_000),
                    )
                    .await;
                    accepted(
                        &node
                            .facade_answer(
                                &submit_json(&hex_of(200 + n8), &format!("seed-{n}"), "seed"),
                                now,
                            )
                            .await,
                    )
                    .0
                };
                ids.push((job["job"].as_str().unwrap().to_owned(), made.to_string()));
                // The fixtures name the job's primary task T<n> after the job J<n>.
                let primary = node.authority.job(made).await.unwrap().unwrap().job.primary;
                ids.push((
                    job["job"].as_str().unwrap().replace('J', "T"),
                    primary.to_string(),
                ));
                if job["live_attempt"].as_bool().unwrap() {
                    let state = node.authority.job(made).await.unwrap().unwrap();
                    let start = Envelope {
                        key: OperationKey::parse(&format!("seed-start-{n}")).unwrap(),
                        job: Some(made),
                        expected: None,
                        body: Command::StartAttempt {
                            task: state.job.primary,
                            resource: ResourceRef::Node { node: node.id },
                            initial_checkpoint: None,
                        },
                    };
                    let answer = node.authority.submit(start).await.unwrap();
                    assert!(
                        matches!(answer.result, StoredResult::Accepted(_)),
                        "{name}: {answer:?}"
                    );
                }
                let mut k = 0;
                loop {
                    let state = node.authority.job(made).await.unwrap().unwrap();
                    if state.job.revision.get() >= want {
                        break;
                    }
                    note(&node, made, 1_000 + k).await;
                    k += 1;
                }
                if let Some(head) = given["events"]["head"].as_str() {
                    let head = head.parse::<u64>().unwrap();
                    loop {
                        let last = node
                            .authority
                            .with_journal(false, move |j| {
                                Ok(j.events(made, None, u32::MAX)?
                                    .last()
                                    .map_or(0, |e| e.seq.get()))
                            })
                            .await
                            .unwrap();
                        if last >= head {
                            break;
                        }
                        note(&node, made, 2_000 + k).await;
                        k += 1;
                    }
                }
            }
            if !seeded_tokens {
                seed_given_tokens(&node, given, now, &ids).await;
            }
            let map = |value: &Value| -> Value {
                let mut text = serde_json::to_string(value).unwrap();
                for (from, to) in &ids {
                    text = text.replace(from, to);
                }
                serde_json::from_str(&text).unwrap()
            };
            for operation in given["operations"].as_array().unwrap() {
                let answer = node.facade_answer(&map(&operation["digest_of"]), now).await;
                assert!(
                    matches!(answer, FacadeAnswer::Accepted { .. }),
                    "{name}: seeding {answer:?}"
                );
            }
            let answer = node
                .facade_answer(&map(&case["message"]), now)
                .await
                .to_json();
            let expect = &case["expect"];
            match expect["outcome"].as_str().unwrap() {
                "accept" => {
                    let want = map(&expect["answer"]);
                    for (key, value) in want.as_object().unwrap() {
                        assert_eq!(&answer[key], value, "{name}: {key} in {answer}");
                    }
                }
                "reject" => {
                    assert_eq!(answer["type"], "refused", "{name}: {answer}");
                    assert_eq!(answer["code"], expect["code"], "{name}: {answer}");
                    assert_eq!(answer["reason"], expect["reason"], "{name}: {answer}");
                }
                other => panic!("{other}"),
            }
            checked += 1;
        }
        assert!(checked >= 30, "{checked} cases");
    }
}
