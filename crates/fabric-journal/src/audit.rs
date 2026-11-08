//! Reads for `inspect` and `audit export` (ADR 0031): the journal's rows
//! as they are, by job, and every table's rows in one time window in one
//! deterministic order.
//!
//! Invariant: these only read. The audit order is the sort key
//! `(at_ms, table, key)`: `at_ms` is when the row was made (an
//! operation's `accepted_ms`, an event's `at_ms`, an evidence row's
//! event's `at_ms`, a provider operation's `dispatched_ms`), the table
//! ranks operations, events, evidence, provider operations, and the key
//! is the row's id (an event's `seq`). No two rows share a key, so the
//! order is total and a page that ends at a key resumes after it.

use fabric_core::id::JobId;
use fabric_core::managed::ManagedEvent as E;
use fabric_core::managed::{AttemptId, DomainError, OperationId, StoredResult, TaskId};
use rusqlite::{Row, params};

use crate::project::{get_u64, sql_u64};
use crate::{CommitError, Journal};

/// One attempt row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRow {
    /// The attempt.
    pub attempt: AttemptId,
    /// Its task.
    pub task: TaskId,
    /// Its generation.
    pub generation: u64,
    /// Its status as the journal names it.
    pub status: String,
    /// The operation that granted it.
    pub granted_by: OperationId,
    /// That operation's kind: `create_job`, `start_attempt` or `continue`.
    pub granted_by_kind: Option<String>,
    /// The snapshot it starts from.
    pub initial_checkpoint: Option<[u8; 32]>,
    /// The newest checkpoint it left.
    pub last_checkpoint: Option<[u8; 32]>,
    /// Unix milliseconds of the grant.
    pub granted_ms: u64,
    /// Unix milliseconds of its end, once it ended.
    pub ended_ms: Option<u64>,
}

/// One operation row, accepted or rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRow {
    /// The operation.
    pub operation: OperationId,
    /// The caller's key.
    pub key: String,
    /// The job it named, if any.
    pub job: Option<JobId>,
    /// Its kind as the journal names it.
    pub kind: String,
    /// Its status as the journal names it.
    pub status: String,
    /// The revision it was accepted at; `None` for a rejected one.
    pub accepted_rev: Option<u64>,
    /// Why it was rejected, for a rejected one.
    pub refusal: Option<DomainError>,
    /// Unix milliseconds of its acceptance or rejection.
    pub accepted_ms: u64,
    /// Unix milliseconds of its last change.
    pub updated_ms: u64,
}

/// One event row, without its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    /// Its sequence number.
    pub seq: u64,
    /// The job.
    pub job: JobId,
    /// The revision of its batch.
    pub revision: u64,
    /// The operation whose command made it.
    pub operation: Option<OperationId>,
    /// Its kind as the journal names it.
    pub kind: String,
    /// Its digest.
    pub digest: [u8; 32],
    /// The digest of the job's previous event.
    pub prev_digest: Option<[u8; 32]>,
    /// Unix milliseconds of its commit.
    pub at_ms: u64,
}

/// One evidence row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRow {
    /// Its id.
    pub evidence: [u8; 16],
    /// The job.
    pub job: JobId,
    /// The attempt it is evidence of.
    pub attempt: Option<AttemptId>,
    /// The revision it was recorded at.
    pub revision: u64,
    /// `checkpoint` or `exit`.
    pub kind: String,
    /// What it refers to: a snapshot id, or how the attempt exited.
    pub reference: String,
    /// Its digest, when it has one.
    pub digest: Option<[u8; 32]>,
    /// The event that recorded it.
    pub seq: u64,
    /// Unix milliseconds of that event.
    pub at_ms: u64,
}

/// One provider operation row, without the text a worker or a caller
/// wrote: no target, locator, searched scope or reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRow {
    /// The operation.
    pub operation: OperationId,
    /// The job.
    pub job: JobId,
    /// The task.
    pub task: TaskId,
    /// The attempt.
    pub attempt: AttemptId,
    /// The generation.
    pub generation: u64,
    /// The worker profile.
    pub profile: String,
    /// The action.
    pub action: String,
    /// `create`, `mutate` or `destroy`.
    pub effect_class: String,
    /// `effect` or `cleanup`.
    pub purpose: String,
    /// Its status as the journal names it.
    pub status: String,
    /// The worker's code for its answer.
    pub code: Option<String>,
    /// `none`, `applied`, `pending` or `unknown`.
    pub effect: Option<String>,
    /// The digest of its locator, when it has one.
    pub locator_key: Option<[u8; 32]>,
    /// Unix milliseconds of its dispatch.
    pub dispatched_ms: u64,
    /// Unix milliseconds of its last change.
    pub updated_ms: u64,
}

/// One row of the audit order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditRow {
    /// From `operations`.
    Operation(OperationRow),
    /// From `events`.
    Event(EventRow),
    /// From `evidence`.
    Evidence(EvidenceRow),
    /// From `provider_operations`.
    Provider(ProviderRow),
}

/// A row's place in the audit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AuditKey {
    /// When the row was made.
    pub at_ms: u64,
    /// The table: 0 operations, 1 events, 2 evidence, 3 provider operations.
    pub table: u8,
    /// The row's id: 16 bytes, or an event's `seq` big-endian in the last 8.
    pub key: [u8; 16],
}

impl AuditRow {
    /// Its place in the audit order.
    pub fn key(&self) -> AuditKey {
        match self {
            AuditRow::Operation(r) => AuditKey {
                at_ms: r.accepted_ms,
                table: 0,
                key: r.operation.to_bytes(),
            },
            AuditRow::Event(r) => {
                let mut key = [0; 16];
                key[8..].copy_from_slice(&r.seq.to_be_bytes());
                AuditKey {
                    at_ms: r.at_ms,
                    table: 1,
                    key,
                }
            }
            AuditRow::Evidence(r) => AuditKey {
                at_ms: r.at_ms,
                table: 2,
                key: r.evidence,
            },
            AuditRow::Provider(r) => AuditKey {
                at_ms: r.dispatched_ms,
                table: 3,
                key: r.operation.to_bytes(),
            },
        }
    }
}

fn opt_u64(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<u64>> {
    let value: Option<i64> = row.get(idx)?;
    value
        .map(|v| u64::try_from(v).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(idx, v)))
        .transpose()
}

const OPERATION_COLUMNS: &str =
    "operation_id, op_key, job_id, kind, status, accepted_rev, result, accepted_ms, updated_ms";

fn operation_row(r: &Row<'_>) -> rusqlite::Result<(OperationRow, Vec<u8>)> {
    Ok((
        OperationRow {
            operation: OperationId::from_bytes(r.get(0)?),
            key: r.get(1)?,
            job: r.get::<_, Option<[u8; 16]>>(2)?.map(JobId::from_bytes),
            kind: r.get(3)?,
            status: r.get(4)?,
            accepted_rev: opt_u64(r, 5)?,
            refusal: None,
            accepted_ms: get_u64(r, 7)?,
            updated_ms: get_u64(r, 8)?,
        },
        r.get(6)?,
    ))
}

/// The refusal a rejected row's stored answer holds.
fn with_refusal((mut row, result): (OperationRow, Vec<u8>)) -> Result<OperationRow, CommitError> {
    if row.status == "rejected" {
        match StoredResult::decode(&result) {
            Ok(StoredResult::Rejected(refusal)) => row.refusal = Some(refusal),
            Ok(StoredResult::Accepted(_)) => {
                return Err(CommitError::Corrupt {
                    job: row.job,
                    reason: "a rejected accept".into(),
                });
            }
            Err(e) => {
                return Err(CommitError::Corrupt {
                    job: row.job,
                    reason: e.to_string(),
                });
            }
        }
    }
    Ok(row)
}

const EVENT_COLUMNS: &str = "seq, job_id, revision, operation_id, kind, digest, prev_digest, at_ms";

fn event_row(r: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        seq: get_u64(r, 0)?,
        job: JobId::from_bytes(r.get(1)?),
        revision: get_u64(r, 2)?,
        operation: r
            .get::<_, Option<[u8; 16]>>(3)?
            .map(OperationId::from_bytes),
        kind: r.get(4)?,
        digest: r.get(5)?,
        prev_digest: r.get(6)?,
        at_ms: get_u64(r, 7)?,
    })
}

const EVIDENCE_COLUMNS: &str =
    "v.evidence_id, v.job_id, v.attempt_id, v.revision, v.kind, v.ref, v.digest, v.seq, e.at_ms";

fn evidence_row(r: &Row<'_>) -> rusqlite::Result<EvidenceRow> {
    Ok(EvidenceRow {
        evidence: r.get(0)?,
        job: JobId::from_bytes(r.get(1)?),
        attempt: r.get::<_, Option<[u8; 16]>>(2)?.map(AttemptId::from_bytes),
        revision: get_u64(r, 3)?,
        kind: r.get(4)?,
        reference: r.get(5)?,
        digest: r.get(6)?,
        seq: get_u64(r, 7)?,
        at_ms: get_u64(r, 8)?,
    })
}

const PROVIDER_COLUMNS: &str = "operation_id, job_id, task_id, attempt_id, generation, profile, action, effect_class, purpose, status, code, effect, locator_key, dispatched_ms, updated_ms";

fn provider_row(r: &Row<'_>) -> rusqlite::Result<ProviderRow> {
    Ok(ProviderRow {
        operation: OperationId::from_bytes(r.get(0)?),
        job: JobId::from_bytes(r.get(1)?),
        task: TaskId::from_bytes(r.get(2)?),
        attempt: AttemptId::from_bytes(r.get(3)?),
        generation: get_u64(r, 4)?,
        profile: r.get(5)?,
        action: r.get(6)?,
        effect_class: r.get(7)?,
        purpose: r.get(8)?,
        status: r.get(9)?,
        code: r.get(10)?,
        effect: r.get(11)?,
        locator_key: r.get(12)?,
        dispatched_ms: get_u64(r, 13)?,
        updated_ms: get_u64(r, 14)?,
    })
}

/// The event kinds an executor's report or the authority's own check
/// records: what `last_report` looks for.
const REPORT_KINDS: &str = "'attempt_observed','receipts_recorded','checkpoint_cut_recorded',\
    'handoff_blocked','continuation_recorded','stale_observation_refused'";

impl Journal {
    /// `job`'s attempts in grant order, with the kind of the operation that granted each.
    pub fn attempt_rows(&self, job: JobId) -> Result<Vec<AttemptRow>, CommitError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.attempt_id, a.task_id, a.generation, a.status, a.granted_by, o.kind,
                    a.initial_checkpoint, a.last_checkpoint, a.granted_ms, a.ended_ms
             FROM attempts a LEFT JOIN operations o ON o.operation_id = a.granted_by
             WHERE a.job_id = ?1 ORDER BY a.generation",
        )?;
        let rows = stmt.query_map([job.to_bytes()], |r| {
            Ok(AttemptRow {
                attempt: AttemptId::from_bytes(r.get(0)?),
                task: TaskId::from_bytes(r.get(1)?),
                generation: get_u64(r, 2)?,
                status: r.get(3)?,
                granted_by: OperationId::from_bytes(r.get(4)?),
                granted_by_kind: r.get(5)?,
                initial_checkpoint: r.get(6)?,
                last_checkpoint: r.get(7)?,
                granted_ms: get_u64(r, 8)?,
                ended_ms: opt_u64(r, 9)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// `job`'s newest `limit` operations, accepted and rejected, oldest
    /// first, and how many the job holds in all.
    pub fn operation_rows(
        &self,
        job: JobId,
        limit: u32,
    ) -> Result<(Vec<OperationRow>, u64), CommitError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {OPERATION_COLUMNS} FROM operations WHERE job_id = ?1
             ORDER BY accepted_ms DESC, operation_id DESC LIMIT ?2"
        ))?;
        let mut rows = stmt
            .query_map(params![job.to_bytes(), limit], operation_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.reverse();
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE job_id = ?1",
            [job.to_bytes()],
            |r| r.get(0),
        )?;
        let rows = rows
            .into_iter()
            .map(with_refusal)
            .collect::<Result<_, _>>()?;
        Ok((rows, total.unsigned_abs()))
    }

    /// `job`'s newest `limit` evidence rows (of `attempt` only, when
    /// given), in event order, and how many there are in all.
    pub fn evidence_rows(
        &self,
        job: JobId,
        attempt: Option<AttemptId>,
        limit: u32,
    ) -> Result<(Vec<EvidenceRow>, u64), CommitError> {
        let attempt = attempt.map(AttemptId::to_bytes);
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVIDENCE_COLUMNS} FROM evidence v JOIN events e ON e.seq = v.seq
             WHERE v.job_id = ?1 AND (?2 IS NULL OR v.attempt_id = ?2)
             ORDER BY v.seq DESC LIMIT ?3"
        ))?;
        let mut rows = stmt
            .query_map(params![job.to_bytes(), attempt, limit], evidence_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.reverse();
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM evidence
             WHERE job_id = ?1 AND (?2 IS NULL OR attempt_id = ?2)",
            params![job.to_bytes(), attempt],
            |r| r.get(0),
        )?;
        Ok((rows, total.unsigned_abs()))
    }

    /// The newest event a report recorded for `job` (for `attempt` only,
    /// when given): its row and the attempt it names.
    pub fn last_report(
        &self,
        job: JobId,
        attempt: Option<AttemptId>,
    ) -> Result<Option<(EventRow, Option<AttemptId>)>, CommitError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS}, body FROM events
             WHERE job_id = ?1 AND kind IN ({REPORT_KINDS}) ORDER BY seq DESC"
        ))?;
        let mut rows = stmt.query([job.to_bytes()])?;
        while let Some(r) = rows.next()? {
            let row = event_row(r)?;
            let body: Vec<u8> = r.get(8)?;
            let frame = fabric_core::managed::EventFrame::decode(&body).map_err(|e| {
                CommitError::Corrupt {
                    job: Some(job),
                    reason: e.to_string(),
                }
            })?;
            let named = match frame.event {
                E::AttemptObserved { attempt, .. }
                | E::ReceiptsRecorded { attempt, .. }
                | E::CheckpointCutRecorded { attempt, .. }
                | E::HandoffBlocked { attempt, .. }
                | E::ContinuationRecorded { attempt, .. }
                | E::StaleObservationRefused { attempt, .. } => Some(attempt),
                _ => None,
            };
            if attempt.is_none() || named == attempt {
                return Ok(Some((row, named)));
            }
        }
        Ok(None)
    }

    /// Up to `limit` rows of every table made in `[since_ms, until_ms)`,
    /// in the audit order, after `after` when given. A page shorter than
    /// `limit` is the last.
    pub fn audit_page(
        &self,
        since_ms: u64,
        until_ms: u64,
        after: Option<AuditKey>,
        limit: u32,
    ) -> Result<Vec<AuditRow>, CommitError> {
        let since = sql_u64(since_ms)?;
        let until = i64::try_from(until_ms).unwrap_or(i64::MAX);
        let limit = i64::from(limit);
        let mut out: Vec<AuditRow> = Vec::new();
        // Each table is read past the cursor on its own (its rank is fixed,
        // so `(at, rank, key) > cursor` is `(at, key) > (at', key')` for the
        // cursor's table, `at > at'` for an earlier one and `at >= at'` for
        // a later one), then the four are merged and cut to `limit`.
        let bound = |table: u8| -> (i64, Vec<u8>, bool) {
            match after {
                None => (since, Vec::new(), true),
                Some(c) => {
                    let at = sql_u64(c.at_ms).unwrap_or(i64::MAX).max(since);
                    if table < c.table {
                        (at, vec![0xff; 17], false)
                    } else if table == c.table && sql_u64(c.at_ms).unwrap_or(i64::MAX) >= since {
                        (at, c.key.to_vec(), false)
                    } else {
                        (at, Vec::new(), true)
                    }
                }
            }
        };
        // `inclusive` with an empty key takes every row at `at`; otherwise a
        // row at `at` must have a key above `key`.
        let (at, key, inclusive) = bound(0);
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {OPERATION_COLUMNS} FROM operations
             WHERE accepted_ms < ?2 AND (accepted_ms > ?1 OR (accepted_ms = ?1 AND (?4 OR operation_id > ?3)))
             ORDER BY accepted_ms, operation_id LIMIT ?5"
        ))?;
        let rows = stmt
            .query_map(params![at, until, key, inclusive, limit], operation_row)?
            .collect::<Result<Vec<_>, _>>()?;
        for row in rows {
            out.push(AuditRow::Operation(with_refusal(row)?));
        }
        let (at, key, inclusive) = bound(1);
        let seq_after = if key.len() == 16 {
            let mut be = [0; 8];
            be.copy_from_slice(&key[8..]);
            i64::try_from(u64::from_be_bytes(be)).unwrap_or(i64::MAX)
        } else if key.is_empty() {
            -1
        } else {
            i64::MAX
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM events
             WHERE at_ms < ?2 AND (at_ms > ?1 OR (at_ms = ?1 AND (?4 OR seq > ?3)))
             ORDER BY at_ms, seq LIMIT ?5"
        ))?;
        let rows = stmt.query_map(params![at, until, seq_after, inclusive, limit], event_row)?;
        for row in rows {
            out.push(AuditRow::Event(row?));
        }
        let (at, key, inclusive) = bound(2);
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVIDENCE_COLUMNS} FROM evidence v JOIN events e ON e.seq = v.seq
             WHERE e.at_ms < ?2 AND (e.at_ms > ?1 OR (e.at_ms = ?1 AND (?4 OR v.evidence_id > ?3)))
             ORDER BY e.at_ms, v.evidence_id LIMIT ?5"
        ))?;
        let rows = stmt.query_map(params![at, until, key, inclusive, limit], evidence_row)?;
        for row in rows {
            out.push(AuditRow::Evidence(row?));
        }
        let (at, key, inclusive) = bound(3);
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {PROVIDER_COLUMNS} FROM provider_operations
             WHERE dispatched_ms < ?2 AND (dispatched_ms > ?1 OR (dispatched_ms = ?1 AND (?4 OR operation_id > ?3)))
             ORDER BY dispatched_ms, operation_id LIMIT ?5"
        ))?;
        let rows = stmt.query_map(params![at, until, key, inclusive, limit], provider_row)?;
        for row in rows {
            out.push(AuditRow::Provider(row?));
        }
        out.sort_by_key(AuditRow::key);
        out.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(out)
    }
}
