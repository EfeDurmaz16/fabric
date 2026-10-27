//! The managed ops of the client API (ADR 0019), served by the authority.
//!
//! Invariant: every managed op goes to the authority's one writer and
//! answers only after the journal committed. A refusal the journal kept
//! under its key is answered as `managed_refused`, the same for a retry;
//! a key reused for another command is `operation_conflict` and wrote
//! nothing. A managed op never reads or writes a standalone job's files,
//! and a standalone op that names a managed job is refused as
//! `managed_job`, never answered as an unknown job.

use std::sync::Arc;

use fabric_client::api::{
    ManagedJob, ManagedJobs, ManagedOperation, Reason, Reply, Request, SystemPath,
};
use fabric_core::id::JobId;
use fabric_core::managed::{
    Command, DomainError, Envelope, Intent, JobPolicy, OperationKey, RestartPolicy, SourceBinding,
    StoredResult,
};
use fabric_core::path::AbsPath;
use fabric_journal::{Answer, CommitError};

use crate::authority::AuthorityError;
use crate::error::NodeError;
use crate::node::Node;

/// The JSON bytes a list answer may use, half a frame as `job_list` uses.
const LIST_BUDGET: usize = fabric_client::frame::MAX_FRAME / 2;

fn refused(reason: Reason, message: impl Into<String>) -> NodeError {
    NodeError::Managed {
        reason: Box::new(reason),
        message: message.into(),
    }
}

/// The snake-case name of a domain refusal, as `managed_refused` carries it.
pub(crate) fn refusal_code(error: &DomainError) -> &'static str {
    match error {
        DomainError::OperationConflict { .. } => "operation_conflict",
        DomainError::RevisionMismatch { .. } => "revision_mismatch",
        DomainError::StaleGeneration { .. } => "stale_generation",
        DomainError::UnknownTask { .. } => "unknown_task",
        DomainError::UnknownAttempt { .. } => "unknown_attempt",
        DomainError::UnknownJob { .. } => "unknown_job",
        DomainError::UnknownOperation { .. } => "unknown_operation",
        DomainError::AttemptStillLive { .. } => "attempt_still_live",
        DomainError::AttemptEnded { .. } => "attempt_ended",
        DomainError::TaskClosed { .. } => "task_closed",
        DomainError::JobClosed { .. } => "job_closed",
        DomainError::WrongMode { .. } => "wrong_mode",
        DomainError::BadParent { .. } => "bad_parent",
        DomainError::BadEnvelope => "bad_envelope",
        DomainError::OperationState { .. } => "operation_state",
        DomainError::Exhausted => "exhausted",
        DomainError::Broken { .. } => "broken",
    }
}

impl From<AuthorityError> for NodeError {
    fn from(error: AuthorityError) -> Self {
        let message = error.to_string();
        let reason = match error {
            AuthorityError::Unavailable(_) => Reason::ManagedUnavailable,
            AuthorityError::Standalone(job) => Reason::StandaloneJob { job },
            AuthorityError::Commit(CommitError::Domain(DomainError::OperationConflict { key })) => {
                Reason::OperationConflict { operation_key: key }
            }
            AuthorityError::Commit(CommitError::Domain(DomainError::UnknownJob { job })) => {
                Reason::UnknownJob { job }
            }
            AuthorityError::Open(_) | AuthorityError::Commit(_) => Reason::Internal,
        };
        refused(reason, message)
    }
}

/// The repository of a managed create as the journal records it.
fn source(repo: &SystemPath) -> Result<SourceBinding, NodeError> {
    let path = repo
        .as_path()
        .to_str()
        .and_then(|text| AbsPath::parse(text).ok())
        .ok_or_else(|| {
            refused(
                Reason::BadRequest,
                format!(
                    "{} is not a normalized UTF-8 absolute path, which a managed job records",
                    repo.as_path().display()
                ),
            )
        })?;
    if !repo.as_path().is_dir() {
        return Err(refused(
            Reason::BadRequest,
            format!(
                "{} is not a directory on this node",
                repo.as_path().display()
            ),
        ));
    }
    Ok(SourceBinding::Repository { path })
}

impl Node {
    /// Serves one managed op.
    pub(crate) async fn managed(self: &Arc<Self>, request: Request) -> Result<Reply, NodeError> {
        match request {
            Request::ManagedCreate {
                operation_key,
                repo,
                goal,
                harness,
            } => {
                let envelope = Envelope {
                    key: operation_key,
                    job: None,
                    expected: None,
                    body: Command::CreateJob {
                        intent: Intent { goal, harness },
                        source: source(&repo)?,
                        policy: JobPolicy {
                            restart: RestartPolicy::ExplicitOnly,
                        },
                    },
                };
                let operation = self.managed_submit(envelope).await?;
                Ok(Reply::ManagedCreate(Box::new(operation)))
            }
            Request::ManagedCancel {
                operation_key,
                job,
                expected_revision,
                reason,
            } => {
                let envelope = Envelope {
                    key: operation_key,
                    job: Some(job),
                    expected: expected_revision,
                    body: Command::CancelJob { reason },
                };
                let operation = self.managed_submit(envelope).await?;
                Ok(Reply::ManagedCancel(Box::new(operation)))
            }
            Request::ManagedStatus { job } => {
                Ok(Reply::ManagedStatus(Box::new(self.managed_job(job).await?)))
            }
            Request::ManagedList => {
                let mut jobs: Vec<ManagedJob> = self
                    .authority
                    .list()
                    .await?
                    .iter()
                    .map(ManagedJob::from)
                    .collect();
                let mut start = jobs.len();
                let mut bytes = 0_usize;
                while let Some(job) = start.checked_sub(1).map(|i| &jobs[i]) {
                    let size = serde_json::to_vec(job).map_or(usize::MAX, |json| json.len());
                    if bytes.saturating_add(size) > LIST_BUDGET {
                        break;
                    }
                    bytes += size;
                    start -= 1;
                }
                let jobs = jobs.split_off(start);
                Ok(Reply::ManagedList(ManagedJobs {
                    jobs,
                    left_out: u64::try_from(start).unwrap_or(u64::MAX),
                }))
            }
            other => Err(refused(
                Reason::Internal,
                format!("{} is not a managed op", other.op()),
            )),
        }
    }

    async fn managed_job(&self, job: JobId) -> Result<ManagedJob, NodeError> {
        match self.authority.job(job).await? {
            Some(state) => Ok(ManagedJob::from(&state)),
            None if self.cfg.state.job(job).ledger().exists() => {
                Err(AuthorityError::Standalone(job).into())
            }
            None => Err(NodeError::UnknownJob(job)),
        }
    }

    async fn managed_submit(
        &self,
        envelope: Envelope<Command>,
    ) -> Result<ManagedOperation, NodeError> {
        let key: OperationKey = envelope.key.clone();
        let named = envelope.job;
        let Answer {
            result, replayed, ..
        } = self.authority.submit(envelope).await?;
        match result {
            StoredResult::Accepted(accepted) => Ok(ManagedOperation {
                operation: accepted.operation,
                operation_key: key,
                replayed,
                job: self.managed_job(accepted.job).await?,
            }),
            StoredResult::Rejected(refusal) => Err(refused(
                Reason::ManagedRefused {
                    refusal: refusal_code(&refusal).to_owned(),
                    job: named,
                    operation_key: key,
                    replayed,
                },
                refusal.to_string(),
            )),
        }
    }

    /// Refuses a standalone op on a job only the authority holds; `None` when it holds none.
    pub(crate) async fn managed_only(&self, job: JobId) -> Option<NodeError> {
        match self.authority.job(job).await {
            Ok(Some(_)) => Some(refused(
                Reason::ManagedJob { job },
                format!(
                    "job {job} is a managed job; `managed_status` shows it, and standalone commands do not act on it"
                ),
            )),
            Ok(None) | Err(_) => None,
        }
    }
}
