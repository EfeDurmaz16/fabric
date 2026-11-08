//! The M5 managed ops of the client API (docs/managed-jobs.md, "Client
//! API"), served by the authority: a create with its acceptance and
//! policy, a submit from a source that is not a repository on this node,
//! a cancel of one task, a note, a checkpoint, a continue, the inspect
//! read, and the facade's mint, revoke and call.
//!
//! Invariant: every command goes through the authority's one writer and
//! answers after the journal committed, as the M2 ops do; a read writes
//! nothing and asks no worker. The facade's own messages are never
//! parsed here: `facade_call` hands the bytes to the authority named and
//! returns its answer unchanged. This file is owner B's beside
//! `managed.rs` (owner A's), so the two sets of ops do not share edits.

use std::sync::Arc;

use fabric_client::api::{
    FacadeEnvelope, JobScopeWire, ManagedOperation, ManagedPolicy, ManagedSource, NoteWire, Reason,
    Reply, Request, ResourceScopeWire, SystemPath,
};
use fabric_client::facade::{Refusal, RefusalCode, Submit};
use fabric_core::harness::HarnessKind;
use fabric_core::id::JobId;
use fabric_core::managed::facade::{JobScope, ResourceScope};
use fabric_core::managed::{Command, Envelope, Note, OperationKey, SourceBinding, TaskId};
use fabric_core::path::AbsPath;

use crate::authority::request_digest;
use crate::error::NodeError;
use crate::facade::{allowance, create_command, fidelity_bytes, remote_source, revision_bytes};
use crate::harnesses::refuse_secret_kind;
use crate::node::Node;

fn refused(reason: Reason, message: impl Into<String>) -> NodeError {
    NodeError::Managed {
        reason: Box::new(reason),
        message: message.into(),
    }
}

/// A refusal the facade's parsers name, as a client API refusal: the
/// domain's name under `managed_refused` when it is the domain's, a bad
/// request otherwise.
fn from_refusal(refusal: &Refusal, key: &OperationKey, job: Option<JobId>) -> NodeError {
    match refusal.code {
        RefusalCode::Refused => refused(
            Reason::ManagedRefused {
                refusal: refusal.reason.clone(),
                job,
                operation_key: key.clone(),
                replayed: false,
            },
            refusal.message.clone(),
        ),
        _ => refused(Reason::BadRequest, refusal.message.clone()),
    }
}

/// A repository on this node as the journal records it.
fn repository(repo: &SystemPath) -> Result<SourceBinding, NodeError> {
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
    /// Serves one M5 managed op or facade op of the client API.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per op, so the whole mapping reads in one place"
    )]
    pub(crate) async fn managed_m5(self: &Arc<Self>, request: Request) -> Result<Reply, NodeError> {
        match request {
            Request::ManagedCreate {
                operation_key,
                repo,
                goal,
                harness,
                acceptance,
                policy,
            } => {
                refuse_secret_kind(&harness)?;
                let source = repository(&repo)?;
                let operation = self
                    .submit_create(operation_key, goal, harness, acceptance, source, policy)
                    .await?;
                Ok(Reply::ManagedCreate(Box::new(operation)))
            }
            Request::ManagedSubmit {
                operation_key,
                goal,
                harness,
                acceptance,
                source,
                policy,
            } => {
                refuse_secret_kind(&harness)?;
                let source = match &source {
                    ManagedSource::Repository { path } => repository(path)?,
                    ManagedSource::Directory { path } => {
                        let snapshot = self.import_directory(path, &[]).await?;
                        SourceBinding::Snapshot { snapshot }
                    }
                    remote => {
                        remote_source(remote).map_err(|r| from_refusal(&r, &operation_key, None))?
                    }
                };
                let operation = self
                    .submit_create(operation_key, goal, harness, acceptance, source, policy)
                    .await?;
                Ok(Reply::ManagedSubmit(Box::new(operation)))
            }
            Request::ManagedCancel {
                operation_key,
                job,
                expected_revision,
                reason,
                task,
            } => {
                let body = match task {
                    Some(task) => Command::CancelTask { task, reason },
                    None => Command::CancelJob { reason },
                };
                let operation = self
                    .managed_submit(Envelope {
                        key: operation_key,
                        job: Some(job),
                        expected: expected_revision,
                        body,
                    })
                    .await?;
                Ok(Reply::ManagedCancel(Box::new(operation)))
            }
            Request::ManagedNote {
                operation_key,
                job,
                expected_revision,
                note,
            } => {
                let note = match note {
                    NoteWire::Decision { what, reason } => Note::Decision { what, reason },
                    NoteWire::Question { text } => Note::Question { text },
                    NoteWire::Answer { question, text } => Note::Answer { question, text },
                };
                let operation = self
                    .managed_submit(Envelope {
                        key: operation_key,
                        job: Some(job),
                        expected: expected_revision,
                        body: Command::RecordNote { note },
                    })
                    .await?;
                Ok(Reply::ManagedNote(Box::new(operation)))
            }
            Request::ManagedCheckpoint {
                operation_key,
                job,
                expected_revision,
            } => {
                let request = request_digest(&[
                    b"checkpoint",
                    &job.to_bytes(),
                    &revision_bytes(expected_revision),
                ]);
                let key = operation_key.clone();
                let operation = self
                    .managed_submit_derived(operation_key, request, job, move |state| {
                        let attempt = state
                            .live_attempt()
                            .or_else(|| state.attempts.last())
                            .map(|a| a.id)
                            .ok_or_else(|| {
                                refused(
                                    Reason::ManagedRefused {
                                        refusal: "not_live".into(),
                                        job: Some(job),
                                        operation_key: key.clone(),
                                        replayed: false,
                                    },
                                    "the job has no attempt to checkpoint",
                                )
                            })?;
                        Ok(Envelope {
                            key,
                            job: Some(job),
                            expected: expected_revision,
                            body: Command::Checkpoint { attempt },
                        })
                    })
                    .await?;
                Ok(Reply::ManagedCheckpoint(Box::new(operation)))
            }
            Request::ManagedContinue {
                operation_key,
                job,
                expected_revision,
                fidelity,
                pin,
            } => {
                let request = request_digest(&[
                    b"continue",
                    &job.to_bytes(),
                    &revision_bytes(expected_revision),
                    &fidelity_bytes(fidelity.as_deref()),
                    &pin.map(|p| p.to_bytes().to_vec()).unwrap_or_default(),
                ]);
                let key = operation_key.clone();
                let operation = self
                    .managed_submit_derived(operation_key, request, job, move |state| {
                        let task: TaskId =
                            state.live_attempt().map_or(state.job.primary, |a| a.task);
                        let fidelity = allowance(fidelity.as_deref(), &state.job.policy);
                        Ok(Envelope {
                            key,
                            job: Some(job),
                            expected: expected_revision,
                            body: Command::Continue {
                                task,
                                fidelity,
                                pin,
                            },
                        })
                    })
                    .await?;
                Ok(Reply::ManagedContinue(Box::new(operation)))
            }
            Request::ManagedInspect { job } => {
                Ok(Reply::ManagedInspect(Box::new(self.inspect(job).await?)))
            }
            Request::ManagedImport {
                path,
                export_allowlist,
            } => {
                let snapshot = self.import_directory(&path, &export_allowlist).await?;
                Ok(Reply::ManagedImport(fabric_client::api::Imported {
                    snapshot,
                }))
            }
            Request::FacadeMint {
                actor,
                jobs,
                resources,
                expires_ms,
            } => {
                let jobs = match jobs {
                    JobScopeWire::Any(_) => JobScope::AnyOwned,
                    JobScopeWire::Jobs(jobs) => JobScope::Jobs(jobs),
                };
                let resources = match resources {
                    ResourceScopeWire::Any(_) => ResourceScope::Any,
                    ResourceScopeWire::Nodes(nodes) => ResourceScope::Nodes(nodes),
                };
                Ok(Reply::FacadeMint(
                    self.facade_mint(&actor, jobs, resources, expires_ms)
                        .await?,
                ))
            }
            Request::FacadeRevoke { token_id } => {
                Ok(Reply::FacadeRevoke(self.facade_revoke(&token_id).await?))
            }
            Request::FacadeCall { authority, message } => {
                let message = self.facade_forward(authority, &message).await;
                Ok(Reply::FacadeCall(FacadeEnvelope { message }))
            }
            other => Err(refused(
                Reason::Internal,
                format!("{} is not an M5 managed op", other.op()),
            )),
        }
    }

    async fn submit_create(
        &self,
        key: OperationKey,
        goal: String,
        harness: HarnessKind,
        acceptance: Vec<String>,
        source: SourceBinding,
        policy: Option<ManagedPolicy>,
    ) -> Result<ManagedOperation, NodeError> {
        let submit = Submit {
            goal,
            harness,
            acceptance,
            source: ManagedSource::Empty,
            policy,
        };
        let body = create_command(&submit, source).map_err(|r| from_refusal(&r, &key, None))?;
        self.managed_submit(Envelope {
            key,
            job: None,
            expected: None,
            body,
        })
        .await
    }

    /// Captures `path` into a snapshot through the existing capture path
    /// (`sources::import`), leaving the directory untouched; the allowlist
    /// names the secret-looking paths that may leave with it.
    async fn import_directory(
        &self,
        path: &SystemPath,
        export_allowlist: &[String],
    ) -> Result<fabric_core::id::SnapshotId, NodeError> {
        let directory = path
            .as_path()
            .to_str()
            .and_then(|text| AbsPath::parse(text).ok())
            .ok_or_else(|| {
                refused(
                    Reason::BadRequest,
                    format!(
                        "{} is not a normalized UTF-8 absolute path",
                        path.as_path().display()
                    ),
                )
            })?;
        let allowlist = export_allowlist
            .iter()
            .map(|p| {
                fabric_core::path::RelPath::parse(p).map_err(|e| {
                    refused(Reason::BadRequest, format!("export_allowlist {p:?}: {e}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::sources::import(self, &directory, &allowlist)
            .await
            .map_err(|e| {
                refused(
                    Reason::BadRequest,
                    format!("import {}: {e}", directory.as_str()),
                )
            })
    }
}
