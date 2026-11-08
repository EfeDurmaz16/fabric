//! `fabric gc`: what crashed or ended work left on this node
//! (docs/concepts/residuals.md, ADR 0030).
//!
//! Invariant: without `--yes` the command only reports, so the default
//! never removes anything. With `--yes` the node removes exactly what it
//! lists; a residual it cannot classify is listed among what it keeps. A
//! provider resource goes only through the worker named here and only
//! with an `--approve` given for that resource in this run: the CLI and
//! the node store no approval, so a second run asks again.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;
use fabric_client::Client;
use fabric_client::api::{GcApproval, GcReport, GcWorker, RefusalKind, Reply, Request, SystemPath};
use fabric_core::managed::OperationId;

use crate::{Refused, contract, render};

/// The options of `fabric gc`.
#[derive(Debug, Args)]
pub(crate) struct GcArgs {
    /// Remove what the report lists; without it gc only reports.
    #[arg(long)]
    yes: bool,
    /// Remove the provider resource this create made, with an approval given for it in this run alone; repeat for each.
    #[arg(
        long = "approve",
        value_name = "OPERATION=APPROVAL",
        value_parser = approval,
        requires = "yes",
        requires = "worker"
    )]
    approve: Vec<GcApproval>,
    /// The provider worker approved removals go through, such as Capsule's bridge worker.
    #[arg(long, value_name = "PATH", requires = "profile")]
    worker: Option<PathBuf>,
    /// The capability profile the worker serves.
    #[arg(long, value_name = "PROFILE", requires = "worker")]
    profile: Option<String>,
    /// The worker's configuration file.
    #[arg(long = "worker-config", value_name = "PATH", requires = "worker")]
    worker_config: Option<PathBuf>,
}

/// Reads `OPERATION=APPROVAL`.
fn approval(text: &str) -> Result<GcApproval, String> {
    let (operation, approval) = text
        .split_once('=')
        .ok_or_else(|| "expected OPERATION=APPROVAL".to_owned())?;
    let operation: OperationId = operation
        .parse()
        .map_err(|e| format!("{operation} is not an operation id: {e}"))?;
    if approval.is_empty() {
        return Err("the approval is empty".to_owned());
    }
    Ok(GcApproval {
        operation,
        approval: approval.to_owned(),
    })
}

fn system_path(path: &Path) -> Result<SystemPath> {
    let absolute = std::path::absolute(path)?;
    let text = absolute
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{} is not UTF-8", absolute.display()))?;
    SystemPath::parse(text).map_err(|e| anyhow::anyhow!(e))
}

/// Runs `fabric gc`: the report, then, under `--yes`, the outcome.
pub(crate) async fn run(client: &Client, args: GcArgs, run: &contract::Run) -> Result<ExitCode> {
    let worker = match (&args.worker, args.profile) {
        (Some(program), Some(profile)) => Some(GcWorker {
            program: system_path(program)?,
            profile,
            config: args.worker_config.as_deref().map(system_path).transpose()?,
        }),
        _ => None,
    };
    let request = Request::Gc {
        remove: args.yes,
        approvals: args.approve,
        worker,
    };
    let reply = crate::ask(client, &request, run.notes()).await?;
    let Reply::Gc(report) = &reply else {
        return Err(anyhow::anyhow!("the node answered gc with {}", reply.op()));
    };
    if let Some(mut refused) = outcome(report) {
        // In words the report prints first; under `--json` it is the
        // failure's data, so standard output stays empty on a failure.
        if run.modes.json {
            refused.data = Some(render::document(&reply, None));
        } else {
            crate::answer(run, &reply, None)?;
        }
        return Err(refused.into());
    }
    crate::answer(run, &reply, None)?;
    Ok(ExitCode::SUCCESS)
}

/// How a removal ended, when it did not end with everything listed
/// removed: a removal that failed is a failure (exit 1); a provider
/// resource no approval was given for is a refusal a human's approval
/// lets run (exit 84).
fn outcome(report: &GcReport) -> Option<Refused> {
    if !report.removing {
        return None;
    }
    let failed = report
        .residuals
        .iter()
        .filter(|r| r.state == "failed")
        .count();
    let unapproved = report
        .residuals
        .iter()
        .filter(|r| r.state == "approval_required")
        .count();
    if failed > 0 {
        return Some(Refused::local(
            RefusalKind::Failed,
            "gc_incomplete",
            format!("{failed} of the residuals gc listed were not removed; the report says why"),
        ));
    }
    (unapproved > 0).then(|| {
        Refused::local(
            RefusalKind::NeedsForce,
            "approval_required",
            format!(
                "{unapproved} provider resources need an approval each, given for that resource in this run: `fabric gc --yes --approve <OPERATION>=<APPROVAL> --worker <PATH> --profile <PROFILE>`"
            ),
        )
    })
}
