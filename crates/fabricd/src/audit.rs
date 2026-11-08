//! `inspect` and `audit_export` (ADR 0031, docs/observability.md): the
//! journal's rows for one job, and every row in a time window, read in
//! the authority's thread. Both only read.
//!
//! Invariant: an audit row carries ids, codes, counters, times and
//! digests only. An event's body, an evidence ref, a provider operation's
//! locator, target, searched scope or reason, and an idempotency key never
//! leave the node in one: a ref and a key travel as SHA-256 hex, and a
//! worker code that is not a plain code is left out.

use std::fmt::Write as _;

use fabric_client::api::{
    AttemptRecord, AuditPage, AuditRecord, EvidenceRecord, GenerationRecord, INSPECT_SCHEMA,
    InspectReport, OperationRecord, Reason, ReportRecord,
};
use fabric_core::id::JobId;
use fabric_core::managed::{AttemptId, ResourceRef};
use fabric_journal::{AuditKey, AuditRow, EvidenceRow, Journal};
use ring::digest::{SHA256, digest};

use crate::authority::AuthorityError;
use crate::error::NodeError;
use crate::node::Node;

/// Rows one audit page reads at most.
const PAGE_ROWS: u32 = 512;

/// The JSON bytes one audit page may use: half a frame, as lists do.
const PAGE_BUDGET: usize = fabric_client::frame::MAX_FRAME / 2;

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn sha256(text: &str) -> String {
    hex(digest(&SHA256, text.as_bytes()).as_ref())
}

fn bad_request(message: String) -> NodeError {
    NodeError::Managed {
        reason: Box::new(Reason::BadRequest),
        message,
    }
}

/// The cursor text of `key`: `<at_ms>.<table>.<32 hex>`.
fn cursor(key: AuditKey) -> String {
    format!("{}.{}.{}", key.at_ms, key.table, hex(&key.key))
}

fn parse_cursor(text: &str) -> Option<AuditKey> {
    let mut parts = text.split('.');
    let at_ms = parts.next()?.parse().ok()?;
    let table: u8 = parts.next()?.parse().ok().filter(|t| *t < 4)?;
    let hex_key = parts.next()?;
    if parts.next().is_some() || hex_key.len() != 32 {
        return None;
    }
    let mut key = [0; 16];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex_key.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(AuditKey { at_ms, table, key })
}

/// A worker code as an audit row carries it: a plain code, or nothing.
fn code(text: Option<String>) -> Option<String> {
    text.filter(|c| {
        !c.is_empty()
            && c.len() <= 64
            && c.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
    })
}

fn evidence_record(row: EvidenceRow) -> EvidenceRecord {
    EvidenceRecord {
        seq: row.seq,
        kind: row.kind,
        attempt: row.attempt,
        revision: row.revision,
        reference: row.reference,
        digest: row.digest.map(|d| hex(&d)),
        at_ms: row.at_ms,
    }
}

/// One audit row as the client API carries it.
fn audit_record(row: AuditRow) -> AuditRecord {
    match row {
        AuditRow::Operation(r) => AuditRecord::Operation {
            at_ms: r.accepted_ms,
            operation: r.operation,
            job: r.job,
            kind: r.kind,
            decision: if r.refusal.is_some() {
                "reject"
            } else {
                "accept"
            }
            .to_owned(),
            status: r.status,
            refusal: r
                .refusal
                .as_ref()
                .map(|e| crate::managed::refusal_code(e).to_owned()),
            key_digest: sha256(&r.key),
        },
        AuditRow::Event(r) => AuditRecord::Event {
            at_ms: r.at_ms,
            seq: r.seq,
            job: r.job,
            revision: r.revision,
            operation: r.operation,
            kind: r.kind,
            digest: hex(&r.digest),
            prev_digest: r.prev_digest.map(|d| hex(&d)),
        },
        AuditRow::Evidence(r) => AuditRecord::Evidence {
            at_ms: r.at_ms,
            seq: r.seq,
            job: r.job,
            attempt: r.attempt,
            kind: r.kind,
            ref_digest: sha256(&r.reference),
            digest: r.digest.map(|d| hex(&d)),
        },
        AuditRow::Provider(r) => AuditRecord::ProviderOperation {
            at_ms: r.dispatched_ms,
            operation: r.operation,
            job: r.job,
            task: r.task,
            attempt: r.attempt,
            generation: r.generation,
            profile: r.profile,
            action: r.action,
            effect_class: r.effect_class,
            purpose: r.purpose,
            status: r.status,
            code: code(r.code),
            effect: r.effect,
            locator_key: r.locator_key.map(|k| hex(&k)),
        },
    }
}

/// The page of rows after `after` in `[since, until)`, cut to the budget.
fn audit_page(
    journal: &Journal,
    since: u64,
    until: u64,
    after: Option<AuditKey>,
) -> Result<AuditPage, AuthorityError> {
    let rows = journal.audit_page(since, until, after, PAGE_ROWS)?;
    let full = rows.len() == PAGE_ROWS as usize;
    let mut records = Vec::with_capacity(rows.len());
    let mut used = 0;
    let mut last = None;
    let mut cut = false;
    for row in rows {
        let key = row.key();
        let record = audit_record(row);
        let size = serde_json::to_vec(&record).map_or(PAGE_BUDGET, |b| b.len() + 1);
        if used + size > PAGE_BUDGET && !records.is_empty() {
            cut = true;
            break;
        }
        used += size;
        records.push(record);
        last = Some(key);
    }
    let next = if full || cut { last.map(cursor) } else { None };
    Ok(AuditPage { records, next })
}

/// The `inspect` report of the job `state` names.
fn inspect_report(
    journal: &Journal,
    state: &fabric_core::managed::ManagedState,
    attempt: Option<AttemptId>,
) -> Result<InspectReport, AuthorityError> {
    let job = state.job.id;
    let mut inspection = crate::facade::read_inspection(journal, state)?;
    let wanted = |a: Option<AttemptId>| attempt.is_none() || a == attempt;
    inspection
        .provider_operations
        .retain(|op| wanted(Some(op.attempt)));
    let rows = journal.attempt_rows(job)?;
    let mut attempts = Vec::new();
    let mut generations = Vec::new();
    for row in rows.into_iter().filter(|r| wanted(Some(r.attempt))) {
        let node = state.attempt(row.attempt).map(|a| match a.resource {
            ResourceRef::Node { node } => node,
        });
        let Some(node) = node else {
            continue;
        };
        generations.push(GenerationRecord {
            generation: row.generation,
            attempt: row.attempt,
            granted_by: row.granted_by,
            granted_by_kind: row
                .granted_by_kind
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            supersedes: row.generation.checked_sub(1).filter(|g| *g >= 1),
        });
        attempts.push(AttemptRecord {
            attempt: row.attempt,
            task: row.task,
            generation: row.generation,
            status: row.status,
            node,
            granted_by: row.granted_by,
            initial_checkpoint: row.initial_checkpoint.map(|d| hex(&d)),
            last_checkpoint: row.last_checkpoint.map(|d| hex(&d)),
            granted_ms: row.granted_ms,
            ended_ms: row.ended_ms,
        });
    }
    let operations = journal
        .operation_rows(job)?
        .into_iter()
        .map(|r| OperationRecord {
            operation: r.operation,
            operation_key: r.key,
            kind: r.kind,
            status: r.status,
            revision: r.accepted_rev,
            refusal: r
                .refusal
                .as_ref()
                .map(|e| crate::managed::refusal_code(e).to_owned()),
            accepted_ms: r.accepted_ms,
            updated_ms: r.updated_ms,
        })
        .collect();
    let evidence = journal
        .evidence_rows(job)?
        .into_iter()
        .filter(|r| wanted(r.attempt))
        .map(evidence_record)
        .collect();
    let last_report = journal
        .last_report(job, attempt)?
        .map(|(row, named)| ReportRecord {
            seq: row.seq,
            kind: row.kind,
            attempt: named,
            at_ms: row.at_ms,
        });
    Ok(InspectReport {
        schema: INSPECT_SCHEMA,
        attempt,
        inspection,
        attempts,
        generations,
        operations,
        evidence,
        last_report,
    })
}

fn no_journal(error: &AuthorityError) -> bool {
    matches!(error, AuthorityError::Unavailable(reason) if reason.starts_with("no managed journal"))
}

impl Node {
    /// `inspect`: the whole managed state of `job`, or of its `attempt`.
    pub(crate) async fn inspect_report(
        &self,
        job: JobId,
        attempt: Option<AttemptId>,
    ) -> Result<InspectReport, NodeError> {
        let read = self
            .authority
            .with_journal(false, move |journal| {
                let Some(state) = journal.job(job)? else {
                    return Ok(None);
                };
                if let Some(attempt) = attempt
                    && state.attempt(attempt).is_none()
                {
                    return Ok(Some(Err(attempt)));
                }
                Ok(Some(Ok(inspect_report(journal, &state, attempt)?)))
            })
            .await;
        match read {
            Ok(Some(Ok(report))) => Ok(report),
            Ok(Some(Err(attempt))) => {
                Err(bad_request(format!("job {job} has no attempt {attempt}")))
            }
            Ok(None) => Err(NodeError::UnknownJob(job)),
            Err(e) if no_journal(&e) => Err(NodeError::UnknownJob(job)),
            Err(e) => Err(e.into()),
        }
    }

    /// `audit_export`: one page of the audit rows in `[since, until)`.
    pub(crate) async fn audit_export(
        &self,
        since_ms: Option<u64>,
        until_ms: Option<u64>,
        after: Option<String>,
    ) -> Result<AuditPage, NodeError> {
        let since = since_ms.unwrap_or(0);
        let until = until_ms.unwrap_or(u64::MAX);
        if until <= since {
            return Err(bad_request(format!(
                "the window ends at {until} ms, not after its start at {since} ms"
            )));
        }
        let after = after
            .map(|text| {
                parse_cursor(&text)
                    .ok_or_else(|| bad_request(format!("{text:?} is not an audit cursor")))
            })
            .transpose()?;
        let read = self
            .authority
            .with_journal(false, move |journal| {
                audit_page(journal, since, until, after)
            })
            .await;
        match read {
            Ok(page) => Ok(page),
            Err(e) if no_journal(&e) => Ok(AuditPage {
                records: Vec::new(),
                next: None,
            }),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_reads_back_and_nothing_else_is_one() {
        let key = AuditKey {
            at_ms: 1_700_000_000_123,
            table: 2,
            key: [0xab; 16],
        };
        assert_eq!(parse_cursor(&cursor(key)), Some(key));
        for text in [
            "",
            "1.2",
            "1.4.abababababababababababababababab",
            "x.1.00",
            "1.1.zz",
        ] {
            assert_eq!(parse_cursor(text), None, "{text}");
        }
    }

    #[test]
    fn only_a_plain_worker_code_is_kept() {
        assert_eq!(code(Some("timeout".into())).as_deref(), Some("timeout"));
        assert_eq!(code(Some("free text: /Users/me".into())), None);
    }
}
