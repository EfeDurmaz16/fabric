//! `fabric inspect` and `fabric audit export` (ADR 0031,
//! docs/observability.md): reads of the authority's journal. Both ask the
//! node, so both exit 80 when none runs.

use std::io::Write as _;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use fabric_client::Client;
use fabric_client::api::{Reply, Request};
use fabric_core::id::JobId;
use fabric_core::managed::AttemptId;
use serde_json::json;

use crate::contract;

/// `fabric inspect`.
#[derive(Debug, Args)]
pub(crate) struct InspectArgs {
    /// The managed job.
    job: JobId,
    /// Narrow the attempts, generations, evidence, provider operations
    /// and last report to this attempt of the job.
    #[arg(long, value_name = "ATTEMPT")]
    attempt: Option<AttemptId>,
}

/// `fabric audit`.
#[derive(Debug, Subcommand)]
pub(crate) enum AuditCommand {
    /// Export the journal's operations, events, evidence and provider
    /// operations as JSON lines, oldest first; `--json` wraps them as
    /// `{"records":[...]}`.
    Export {
        /// The window's start, inclusive: an RFC 3339 time, or a date,
        /// which is that day's midnight UTC.
        #[arg(long, value_name = "TIME", value_parser = parse_time)]
        since: Option<u64>,
        /// The window's end, exclusive: an RFC 3339 time, or a date, which
        /// is that day's midnight UTC, so the day itself is left out.
        #[arg(long, value_name = "TIME", value_parser = parse_time)]
        until: Option<u64>,
    },
}

/// Days from 1970-01-01 to `y-m-d` in the proleptic Gregorian calendar
/// (Howard Hinnant's days-from-civil).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn number(text: &str, digits: usize) -> Option<i64> {
    (text.len() == digits && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// `YYYY-MM-DD` as the Unix milliseconds of that day's midnight UTC.
fn parse_date(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let y = number(parts.next()?, 4)?;
    let m = number(parts.next()?, 2)?;
    let d = number(parts.next()?, 2)?;
    if parts.next().is_some() || !(1..=12).contains(&m) {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let last = days_in[usize::try_from(m - 1).ok()?];
    if !(1..=last).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400_000)
}

/// An RFC 3339 time (`2026-11-08T09:30:00Z`, fractions and offsets
/// allowed) or a date (`2026-11-08`, midnight UTC), as Unix milliseconds.
pub(crate) fn parse_time(text: &str) -> Result<u64, String> {
    let bad = || format!("{text:?} is not an RFC 3339 time or a YYYY-MM-DD date");
    if text.len() == 10 {
        let ms = parse_date(text).ok_or_else(bad)?;
        return u64::try_from(ms).map_err(|_| bad());
    }
    let (date, rest) = text.split_once(['T', 't']).ok_or_else(bad)?;
    let day = parse_date(date).ok_or_else(bad)?;
    let (clock, offset_ms) = if let Some(clock) = rest.strip_suffix(['Z', 'z']) {
        (clock, 0)
    } else {
        let at = rest.rfind(['+', '-']).ok_or_else(bad)?;
        let (clock, offset) = rest.split_at(at);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (h, m) = offset[1..].split_once(':').ok_or_else(bad)?;
        let (h, m) = (number(h, 2).ok_or_else(bad)?, number(m, 2).ok_or_else(bad)?);
        if h > 23 || m > 59 {
            return Err(bad());
        }
        (clock, sign * (h * 3_600_000 + m * 60_000))
    };
    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = whole.split(':');
    let h = number(parts.next().ok_or_else(bad)?, 2).ok_or_else(bad)?;
    let m = number(parts.next().ok_or_else(bad)?, 2).ok_or_else(bad)?;
    let s = number(parts.next().ok_or_else(bad)?, 2).ok_or_else(bad)?;
    if parts.next().is_some() || h > 23 || m > 59 || s > 60 {
        return Err(bad());
    }
    if clock.contains('.') && (fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(bad());
    }
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().map_err(|_| bad())?;
    let ms = day + h * 3_600_000 + m * 60_000 + s.min(59) * 1000 + millis - offset_ms;
    u64::try_from(ms).map_err(|_| bad())
}

/// Runs `fabric inspect`.
pub(crate) async fn inspect(
    client: &Client,
    args: InspectArgs,
    run: &contract::Run,
) -> Result<ExitCode> {
    let request = Request::Inspect {
        job: args.job,
        attempt: args.attempt,
    };
    let reply = crate::ask(client, &request, run.notes()).await?;
    crate::answer(run, &reply, None)?;
    Ok(ExitCode::SUCCESS)
}

/// Runs `fabric audit`: every page in order, each record a line as it
/// arrives, or one document of them all under `--json`.
pub(crate) async fn audit(
    client: &Client,
    command: AuditCommand,
    run: &contract::Run,
) -> Result<ExitCode> {
    let AuditCommand::Export { since, until } = command;
    if let (Some(since), Some(until)) = (since, until)
        && until <= since
    {
        return Err(contract::Usage(
            "--until must be later than --since: the window is [since, until)".to_owned(),
        )
        .into());
    }
    let mut after = None;
    let mut all = Vec::new();
    let mut written = 0usize;
    loop {
        let request = Request::AuditExport {
            since_ms: since,
            until_ms: until,
            after: after.take(),
        };
        let Reply::AuditExport(page) = crate::ask(client, &request, run.notes()).await? else {
            anyhow::bail!("the node answered audit_export with another reply");
        };
        if run.modes.json {
            all.extend(page.records);
        } else {
            let mut out = std::io::stdout().lock();
            for record in &page.records {
                writeln!(out, "{}", serde_json::to_string(record)?)?;
            }
            out.flush()?;
            written += page.records.len();
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    if run.modes.json {
        contract::print_document(run.modes, &json!({ "records": all }))?;
    } else if written == 0 {
        // An empty list is nothing on stdout and a hint on stderr.
        run.notes().print(&[
            "no audit records in this window; `fabric audit export` with no --since or --until writes every record the node's journal holds",
        ])?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::parse_time;

    #[test]
    fn a_time_is_rfc_3339_or_a_date_at_midnight_utc() {
        let day = 1_762_560_000_000; // 2025-11-08T00:00:00Z
        assert_eq!(parse_time("2025-11-08"), Ok(day));
        assert_eq!(parse_time("2025-11-08T00:00:00Z"), Ok(day));
        assert_eq!(parse_time("2025-11-08T03:00:00+03:00"), Ok(day));
        assert_eq!(parse_time("2025-11-07T23:00:00-01:00"), Ok(day));
        assert_eq!(parse_time("2025-11-08T00:00:00.25Z"), Ok(day + 250));
        assert_eq!(parse_time("1970-01-01"), Ok(0));
        assert_eq!(parse_time("2024-02-29"), Ok(1_709_164_800_000));
        for bad in [
            "",
            "2025-11-8",
            "2025-02-29",
            "2025-13-01",
            "2025-11-08T24:00:00Z",
            "2025-11-08T00:00:00",
            "2025-11-08 00:00:00Z",
            "2025-11-08T00:00:00.Z",
            "yesterday",
        ] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }
}
