//! Provider operations: the effects a managed attempt asks a provider
//! worker for over the bridge (ADR 0021, docs/bridge-protocol.md).
//!
//! Invariant: a provider operation is written `dispatched` in its own
//! committed transaction before its request frame is written, and only
//! after the attempt it acts for is checked to be the job's current,
//! live grant: the journal owns identity and fencing, the worker owns
//! none. Its key and id are never used twice, so nothing the journal holds
//! is ever sent again. It leaves `dispatched` only for what the worker
//! answered or for `unknown`; it leaves `unknown` only for an observed
//! terminal state. The locator the worker reported is stored as the JSON
//! text it came in and is never parsed here: Fabric reads only the
//! capability profile, never a provider's names.

use fabric_core::id::JobId;
use fabric_core::managed::{
    AttemptId, AttemptStatus, DomainError, Generation, JobState, OperationId, OperationKey,
    RequestDigest, TaskId,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::project::{get_u64, load_state, sql_u64};
use crate::{CommitError, Journal};

/// Where a provider operation is (docs/bridge-protocol.md, the `error` table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderStatus {
    /// Recorded before the request was written; an effect may have happened.
    Dispatched,
    /// A terminal success was observed.
    Succeeded,
    /// The provider gave a definite failure.
    Failed,
    /// A terminal cancellation was observed.
    Cancelled,
    /// Refused before any effect, by Fabric or by the worker.
    Rejected,
    /// Nobody can tell; only an observed terminal state moves it on.
    Unknown,
}

impl ProviderStatus {
    /// The name the journal stores, the one the bridge fixtures use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ProviderStatus::Dispatched => "dispatched",
            ProviderStatus::Succeeded => "succeeded",
            ProviderStatus::Failed => "failed",
            ProviderStatus::Cancelled => "cancelled",
            ProviderStatus::Rejected => "rejected",
            ProviderStatus::Unknown => "unknown",
        }
    }

    fn parse(name: &str) -> Option<ProviderStatus> {
        Some(match name {
            "dispatched" => ProviderStatus::Dispatched,
            "succeeded" => ProviderStatus::Succeeded,
            "failed" => ProviderStatus::Failed,
            "cancelled" => ProviderStatus::Cancelled,
            "rejected" => ProviderStatus::Rejected,
            "unknown" => ProviderStatus::Unknown,
            _ => return None,
        })
    }

    /// Whether nothing moves the operation on any more.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, ProviderStatus::Dispatched | ProviderStatus::Unknown)
    }

    /// Whether an operation may move from `self` to `to`: `dispatched`
    /// to any other status, `unknown` only to an observed terminal one.
    #[must_use]
    pub fn may_become(self, to: ProviderStatus) -> bool {
        match self {
            ProviderStatus::Dispatched => to != ProviderStatus::Dispatched,
            ProviderStatus::Unknown => matches!(
                to,
                ProviderStatus::Succeeded | ProviderStatus::Failed | ProviderStatus::Cancelled
            ),
            _ => false,
        }
    }
}

/// What a provider operation acts for, as Fabric sends it in `request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDispatch {
    /// A fresh operation id; never an id the journal holds.
    pub operation: OperationId,
    /// The idempotency key; never a key the journal holds.
    pub key: OperationKey,
    /// Fabric's digest of the request.
    pub digest: RequestDigest,
    /// The job.
    pub job: JobId,
    /// The task.
    pub task: TaskId,
    /// The attempt the effect belongs to.
    pub attempt: AttemptId,
    /// The attempt's generation, which must be the job's.
    pub generation: Generation,
    /// The bridge action, such as `sandbox.create`.
    pub action: String,
}

/// Why the worker or Fabric answered as it did, kept with the status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderAnswer {
    /// The bridge error code, such as `denied`; `None` for a result.
    pub code: Option<String>,
    /// The bridge error reason, such as `policy_denied`.
    pub reason: Option<String>,
    /// The effect the answer declared: `none`, `applied`, `pending` or `unknown`.
    pub effect: Option<String>,
}

/// A provider operation as the journal holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperation {
    /// What it acts for.
    pub dispatch: ProviderDispatch,
    /// Where it is.
    pub status: ProviderStatus,
    /// The last answer recorded.
    pub answer: ProviderAnswer,
    /// The last locator the worker reported, as its JSON text.
    pub locator: Option<String>,
}

/// Why a provider operation could not be recorded. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// The key is already the key of an operation: it is never sent again.
    #[error("operation key {key} is already in the journal")]
    KeyUsed {
        /// The key.
        key: OperationKey,
    },
    /// The id is already the id of an operation.
    #[error("operation {operation} is already in the journal")]
    OperationUsed {
        /// The id.
        operation: OperationId,
    },
    /// The journal has no provider operation with this id.
    #[error("no provider operation {operation}")]
    UnknownOperation {
        /// The id.
        operation: OperationId,
    },
    /// The operation cannot move from where it is to that status.
    #[error("provider operation {operation} cannot go from {from} to {to}")]
    Transition {
        /// The id.
        operation: OperationId,
        /// Its status.
        from: &'static str,
        /// The status asked for.
        to: &'static str,
    },
}

impl Journal {
    /// Records a provider operation as `dispatched`, or as `rejected` with
    /// `refusal` when Fabric refused it before sending. Returns after
    /// `COMMIT`, so a request written after this returns can never be
    /// written again by a restart: recovery makes it `unknown`.
    ///
    /// Refused, writing nothing, when the key or the id is already the
    /// journal's (provider or command), when the job is not active, and
    /// when the attempt is not the job's current grant, granted or running
    /// at the job's generation (`StaleGeneration`, `AttemptEnded`).
    pub fn provider_begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key_used = tx
            .query_row(
                "SELECT 1 FROM operations WHERE op_key = ?1
                 UNION ALL SELECT 1 FROM provider_operations WHERE op_key = ?1",
                [dispatch.key.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if key_used {
            return Err(ProviderError::KeyUsed {
                key: dispatch.key.clone(),
            }
            .into());
        }
        let id = dispatch.operation.to_bytes();
        let id_used = tx
            .query_row(
                "SELECT 1 FROM operations WHERE operation_id = ?1
                 UNION ALL SELECT 1 FROM provider_operations WHERE operation_id = ?1",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if id_used {
            return Err(ProviderError::OperationUsed {
                operation: dispatch.operation,
            }
            .into());
        }
        let state =
            load_state(&tx, dispatch.job)?.ok_or(DomainError::UnknownJob { job: dispatch.job })?;
        if state.job.state != JobState::Active {
            return Err(DomainError::JobClosed { job: dispatch.job }.into());
        }
        let attempt = state
            .attempt(dispatch.attempt)
            .ok_or(DomainError::UnknownAttempt {
                attempt: dispatch.attempt,
            })?;
        if attempt.task != dispatch.task {
            return Err(DomainError::UnknownTask {
                task: dispatch.task,
            }
            .into());
        }
        if dispatch.generation != attempt.generation || attempt.generation != state.job.generation {
            return Err(DomainError::StaleGeneration {
                given: dispatch.generation,
                current: state.job.generation,
            }
            .into());
        }
        if !matches!(
            attempt.status,
            AttemptStatus::Granted | AttemptStatus::Running
        ) {
            return Err(DomainError::AttemptEnded {
                attempt: dispatch.attempt,
            }
            .into());
        }
        let status = if refusal.is_some() {
            ProviderStatus::Rejected
        } else {
            ProviderStatus::Dispatched
        };
        let answer = refusal.cloned().unwrap_or_default();
        tx.execute(
            "INSERT INTO provider_operations
               (operation_id, op_key, request_digest, job_id, task_id, attempt_id, generation,
                action, status, code, reason, effect, locator, dispatched_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13, ?13)",
            params![
                id,
                dispatch.key.as_str(),
                dispatch.digest.to_bytes(),
                dispatch.job.to_bytes(),
                dispatch.task.to_bytes(),
                dispatch.attempt.to_bytes(),
                sql_u64(dispatch.generation.get())?,
                dispatch.action,
                status.name(),
                answer.code,
                answer.reason,
                answer.effect,
                sql_u64(at_ms)?,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Records what the worker answered for a provider operation: its new
    /// status, the answer and, when given, the locator it reported. The
    /// status may stay as it is, as for `pending`, to store the answer and
    /// the locator alone. A status already reached changes nothing else;
    /// a terminal operation never changes.
    pub fn provider_record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = operation.to_bytes();
        let from: Option<String> = tx
            .query_row(
                "SELECT status FROM provider_operations WHERE operation_id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let from = from.ok_or(ProviderError::UnknownOperation { operation })?;
        let from = ProviderStatus::parse(&from).ok_or_else(|| CommitError::Corrupt {
            job: None,
            reason: format!("provider operation {operation} has status {from}"),
        })?;
        if from.is_terminal() || (from != status && !from.may_become(status)) {
            return Err(ProviderError::Transition {
                operation,
                from: from.name(),
                to: status.name(),
            }
            .into());
        }
        tx.execute(
            "UPDATE provider_operations
             SET status = ?2, code = ?3, reason = ?4, effect = ?5,
                 locator = coalesce(?6, locator), updated_ms = ?7
             WHERE operation_id = ?1",
            params![
                id,
                status.name(),
                answer.code,
                answer.reason,
                answer.effect,
                locator,
                sql_u64(at_ms)?,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Stores the locator the worker reported for a provider operation
    /// that is still `dispatched` or `unknown`, as `progress` reports it
    /// before the answer (ADR 0020, record the observed locator early).
    /// The status and the answer stay as they are; a terminal operation
    /// keeps the locator its answer gave.
    pub fn provider_locate(
        &mut self,
        operation: OperationId,
        locator: &str,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let changed = self.conn.execute(
            "UPDATE provider_operations SET locator = ?2, updated_ms = ?3
             WHERE operation_id = ?1 AND status IN ('dispatched', 'unknown')",
            params![operation.to_bytes(), locator, sql_u64(at_ms)?],
        )?;
        if changed == 0 && self.provider_operation(operation)?.is_none() {
            return Err(ProviderError::UnknownOperation { operation }.into());
        }
        Ok(())
    }

    /// The provider operation with this id, or `None`.
    pub fn provider_operation(
        &self,
        operation: OperationId,
    ) -> Result<Option<ProviderOperation>, CommitError> {
        let row = self
            .conn
            .query_row(
                "SELECT op_key, request_digest, job_id, task_id, attempt_id, generation, action,
                        status, code, reason, effect, locator
                 FROM provider_operations WHERE operation_id = ?1",
                [operation.to_bytes()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, [u8; 32]>(1)?,
                        r.get::<_, [u8; 16]>(2)?,
                        r.get::<_, [u8; 16]>(3)?,
                        r.get::<_, [u8; 16]>(4)?,
                        get_u64(r, 5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, Option<String>>(11)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            key,
            digest,
            job,
            task,
            attempt,
            generation,
            action,
            status,
            code,
            reason,
            effect,
            locator,
        )) = row
        else {
            return Ok(None);
        };
        let job = JobId::from_bytes(job);
        let corrupt = |reason: String| CommitError::Corrupt {
            job: Some(job),
            reason,
        };
        Ok(Some(ProviderOperation {
            dispatch: ProviderDispatch {
                operation,
                key: OperationKey::parse(&key).map_err(|e| corrupt(e.to_string()))?,
                digest: RequestDigest::from_bytes(digest),
                job,
                task: TaskId::from_bytes(task),
                attempt: AttemptId::from_bytes(attempt),
                generation: Generation::new(generation).ok_or_else(|| {
                    corrupt(format!("provider operation {operation} generation 0"))
                })?,
                action,
            },
            status: ProviderStatus::parse(&status)
                .ok_or_else(|| corrupt(format!("provider operation status {status}")))?,
            answer: ProviderAnswer {
                code,
                reason,
                effect,
            },
            locator,
        }))
    }

    /// Every provider operation still `dispatched`, oldest first.
    pub(crate) fn provider_dispatched(&self) -> Result<Vec<(JobId, OperationId)>, CommitError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT job_id, operation_id FROM provider_operations
             WHERE status = 'dispatched' ORDER BY dispatched_ms, operation_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, [u8; 16]>(0)?, r.get::<_, [u8; 16]>(1)?))
        })?;
        Ok(rows
            .map(|r| r.map(|(job, op)| (JobId::from_bytes(job), OperationId::from_bytes(op))))
            .collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use fabric_core::managed::{Observation, OperationMark};

    use super::*;
    use crate::tests::{Fixture, create, start};

    const JOB: JobId = JobId::from_bytes([0x01; 16]);
    const TASK: TaskId = TaskId::from_bytes([1; 16]);

    /// A job whose primary task has a granted attempt at generation one.
    fn granted() -> Fixture {
        let mut f = Fixture::new();
        f.submit("create", None, create());
        f.submit("start", Some(JOB), start(TASK));
        f
    }

    fn dispatch(n: u8) -> ProviderDispatch {
        ProviderDispatch {
            operation: OperationId::from_bytes([0x80 + n; 16]),
            key: OperationKey::parse(&format!("p-{n}")).unwrap(),
            digest: RequestDigest::from_bytes([n; 32]),
            job: JOB,
            task: TASK,
            attempt: AttemptId::from_bytes([2; 16]),
            generation: Generation::FIRST,
            action: "sandbox.create".into(),
        }
    }

    fn status(f: &Fixture, n: u8) -> ProviderStatus {
        f.journal
            .provider_operation(OperationId::from_bytes([0x80 + n; 16]))
            .unwrap()
            .unwrap()
            .status
    }

    /// A key or id the journal holds, a command's included, is never
    /// recorded again, so nothing the journal holds is sent twice.
    #[test]
    fn a_key_or_an_id_is_dispatched_once() {
        let mut f = granted();
        f.journal.provider_begin(&dispatch(1), None, 1).unwrap();
        let again = f.journal.provider_begin(&dispatch(1), None, 2);
        assert!(matches!(
            again,
            Err(CommitError::Provider(ProviderError::KeyUsed { .. }))
        ));
        let mut command_key = dispatch(2);
        command_key.key = OperationKey::parse("start").unwrap();
        assert!(matches!(
            f.journal.provider_begin(&command_key, None, 3),
            Err(CommitError::Provider(ProviderError::KeyUsed { .. }))
        ));
        let mut same_id = dispatch(3);
        same_id.operation = OperationId::from_bytes([0x81; 16]);
        assert!(matches!(
            f.journal.provider_begin(&same_id, None, 4),
            Err(CommitError::Provider(ProviderError::OperationUsed { .. }))
        ));
        let mut command_id = dispatch(4);
        command_id.operation = OperationId::from_bytes([2; 16]);
        assert!(matches!(
            f.journal.provider_begin(&command_id, None, 5),
            Err(CommitError::Provider(ProviderError::OperationUsed { .. }))
        ));
    }

    /// Fabric fences: only the job's current, live attempt at its own
    /// generation can ask for an effect.
    #[test]
    fn only_the_current_live_attempt_dispatches() {
        let mut f = granted();
        let mut stale = dispatch(1);
        stale.generation = Generation::new(2).unwrap();
        assert!(matches!(
            f.journal.provider_begin(&stale, None, 1),
            Err(CommitError::Domain(DomainError::StaleGeneration { .. }))
        ));
        let mut other_task = dispatch(2);
        other_task.task = TaskId::from_bytes([9; 16]);
        assert!(matches!(
            f.journal.provider_begin(&other_task, None, 2),
            Err(CommitError::Domain(DomainError::UnknownTask { .. }))
        ));
        let attempt = AttemptId::from_bytes([2; 16]);
        f.journal
            .mark(
                JOB,
                OperationId::from_bytes([2; 16]),
                OperationMark::Dispatched,
                3,
            )
            .unwrap();
        f.journal
            .observe(
                JOB,
                &Observation::Lost {
                    attempt,
                    generation: Generation::FIRST,
                },
                4,
            )
            .unwrap();
        assert!(matches!(
            f.journal.provider_begin(&dispatch(3), None, 5),
            Err(CommitError::Domain(DomainError::AttemptEnded { .. }))
        ));
        assert!(
            f.journal
                .provider_operation(OperationId::from_bytes([0x83; 16]))
                .unwrap()
                .is_none()
        );
    }

    /// Dispatched moves to any answer; unknown only to an observed
    /// terminal state; a terminal status never moves; pending keeps the
    /// status and stores the locator.
    #[test]
    fn statuses_move_only_forward() {
        let mut f = granted();
        let op = |n: u8| OperationId::from_bytes([0x80 + n; 16]);
        let none = ProviderAnswer::default();
        f.journal.provider_begin(&dispatch(1), None, 1).unwrap();
        f.journal
            .provider_record(
                op(1),
                ProviderStatus::Dispatched,
                &none,
                Some("{\"a\":1}"),
                2,
            )
            .unwrap();
        assert_eq!(status(&f, 1), ProviderStatus::Dispatched);
        f.journal
            .provider_record(op(1), ProviderStatus::Unknown, &none, None, 3)
            .unwrap();
        assert!(matches!(
            f.journal
                .provider_record(op(1), ProviderStatus::Rejected, &none, None, 4),
            Err(CommitError::Provider(ProviderError::Transition { .. }))
        ));
        f.journal
            .provider_record(op(1), ProviderStatus::Succeeded, &none, None, 5)
            .unwrap();
        let done = f.journal.provider_operation(op(1)).unwrap().unwrap();
        assert_eq!(done.status, ProviderStatus::Succeeded);
        assert_eq!(done.locator.as_deref(), Some("{\"a\":1}"));
        assert!(matches!(
            f.journal
                .provider_record(op(1), ProviderStatus::Unknown, &none, None, 6),
            Err(CommitError::Provider(ProviderError::Transition { .. }))
        ));
        let refusal = ProviderAnswer {
            code: Some("denied".into()),
            reason: Some("policy_denied".into()),
            effect: Some("none".into()),
        };
        f.journal
            .provider_begin(&dispatch(2), Some(&refusal), 7)
            .unwrap();
        assert_eq!(status(&f, 2), ProviderStatus::Rejected);
        assert!(matches!(
            f.journal
                .provider_record(op(9), ProviderStatus::Unknown, &none, None, 8),
            Err(CommitError::Provider(
                ProviderError::UnknownOperation { .. }
            ))
        ));
    }
}
