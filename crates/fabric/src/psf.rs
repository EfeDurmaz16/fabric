//! The Portable Session Format on the command line (ADR 0029):
//! `fabric job import --psf`, and `fabric psf inspect` and `verdicts`.
//! `fabric job export --psf` is a node request, in `job_request`.
//!
//! Each keeps the CLI contract (docs/cli-contract.md): its data on
//! standard output, in words or as one JSON document keyed `container`
//! or `verdicts`, its hints on standard error, and a container that does
//! not read as a failure under the CLI's own reason `invalid_container`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;
use fabric_client::api::{DoctorReport, JobStatus, Op, RefusalKind, Reply, Request};
use fabric_client::{Client, ClientError};
use fabric_core::id::JobId;
use fabric_core::version::HarnessVersion;
use fabric_psf::{Achieved, Container, Target, achievable, hex, read_container, read_manifest};
use serde_json::{Value, json};

use crate::{Notes, Refused, contract, write_lines};

/// Read a Portable Session Format container without a node.
#[derive(Debug, Subcommand)]
pub(crate) enum PsfCommand {
    /// Check a container or a manifest and print its digest, the fidelity
    /// it offers and the attachments it lacks.
    Inspect {
        /// A container directory, or a `manifest.json`.
        #[arg(value_name = "CONTAINER")]
        container: PathBuf,
    },
    /// Print one verdict line per fixture of a fixture set such as
    /// protocol/psf/v1, the lines the format's other readers print.
    #[command(hide = true)]
    Verdicts {
        /// The fixture set.
        #[arg(value_name = "DIR")]
        dir: PathBuf,
    },
}

pub(crate) fn run(command: PsfCommand, run: &contract::Run) -> Result<ExitCode> {
    match command {
        PsfCommand::Inspect { container: path } => {
            let (container, digest) = read(&path)?;
            if run.modes.json {
                let doc = json!({ "container": {
                    "path": path.display().to_string(),
                    "digest": digest,
                    "offered": container.manifest.parts().fidelity.to_string(),
                    "present": container.present,
                    "detached": container.detached,
                }});
                contract::print_document(run.modes, &doc)?;
                return Ok(ExitCode::SUCCESS);
            }
            let mut lines = vec![
                format!("digest {digest}"),
                format!("offered {}", container.manifest.parts().fidelity),
            ];
            lines.push(if container.detached.is_empty() {
                "detached none".to_owned()
            } else {
                format!("detached {}", container.detached.join(" "))
            });
            Notes::Stdout.print(&lines)?;
            Ok(ExitCode::SUCCESS)
        }
        PsfCommand::Verdicts { dir } => {
            let lines = fabric_psf::fixture_verdicts(&dir).map_err(|e| {
                invalid(
                    format!("The fixture set {} does not read: {e}.", dir.display()),
                    &dir,
                )
            })?;
            if run.modes.json {
                contract::print_document(run.modes, &json!({ "verdicts": lines }))?;
            } else {
                // The verdict lines are the format's own, compared byte for
                // byte with the other readers', so they print as they are.
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                for line in lines {
                    writeln!(out, "{line}")?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// A container, a manifest or a fixture set that does not read: the
/// CLI's own reason `invalid_container`, exit 1, with the path as data.
fn invalid(message: String, path: &Path) -> anyhow::Error {
    let mut refused = Refused::local(RefusalKind::Failed, "invalid_container", message);
    refused.data = Some(json!({ "path": path.display().to_string() }));
    refused.into()
}

/// Reads a container directory, or a lone manifest as a container with
/// every attachment detached.
fn read(path: &Path) -> Result<(Container, String)> {
    if path.is_dir() {
        let container = read_container(path).map_err(|r| {
            invalid(
                format!("The container {} is refused: {r}.", path.display()),
                path,
            )
        })?;
        let digest = hex(&container.manifest.digest());
        return Ok((container, digest));
    }
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    let manifest = read_manifest(&bytes).map_err(|r| {
        invalid(
            format!("The manifest {} is refused: {r}.", path.display()),
            path,
        )
    })?;
    let digest = hex(&manifest.digest());
    let detached = manifest
        .parts()
        .attachments
        .iter()
        .map(|a| a.name.clone())
        .collect();
    Ok((
        Container {
            manifest,
            present: Vec::new(),
            detached,
        },
        digest,
    ))
}

/// `fabric job import --psf <dir>`: checks the container, matches it to
/// the job on this node, and says what this node can continue and how.
///
/// The container names a revision, it does not hold one. The import
/// succeeds when this node holds the job at the revision the document
/// names; then it reports the fidelity this node achieves, and on
/// standard error the command that continues the job. Otherwise it fails
/// and names the node that wrote the document, from which `fabric job
/// resume --on` pulls it. It changes nothing, so it answers as a read.
pub(crate) async fn import(
    client: &Client,
    home: &Path,
    dir: &Path,
    run: &contract::Run,
) -> Result<ExitCode> {
    let container = read_container(dir).map_err(|r| {
        invalid(
            format!("The container {} is refused: {r}.", dir.display()),
            dir,
        )
    })?;
    let parts = container.manifest.parts();
    let job: JobId = parts.job.id.parse().map_err(|e| {
        invalid(
            format!(
                "The container {} names job {}: {e}.",
                dir.display(),
                parts.job.id
            ),
            dir,
        )
    })?;
    let by = &parts.produced.by;
    let named = parts.workspace.bundle_head.as_str();
    let status = held(client, job, by, named).await?;
    let root = status.workspace.as_ref().map_or_else(
        || parts.workspace.root.clone(),
        |w| w.root.display().to_string(),
    );
    let target = Target {
        root,
        home: home.display().to_string(),
        harnesses: installed(client).await,
    };
    let Achieved {
        offered,
        achieved,
        why,
    } = achievable(&container.manifest, &target, &container.detached);
    let next = format!("continue with `fabric job resume {job}`");
    if run.modes.json {
        let doc: Value = json!({ "container": {
            "path": dir.display().to_string(),
            "digest": hex(&container.manifest.digest()),
            "job": job.to_string(),
            "snapshot": named,
            "produced_by": by,
            "offered": offered.to_string(),
            "achieved": achieved.to_string(),
            "why": why,
            "left_behind": parts.left_behind,
        }});
        contract::print_document(run.modes, &doc)?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut lines = vec![format!(
        "{} matches job {job} at snapshot {named}: offered {offered}, achieved here {achieved}",
        dir.display(),
    )];
    lines.extend(why.into_iter().map(|reason| format!("  {reason}")));
    if !parts.left_behind.is_empty() {
        lines.push(format!(
            "recreate what the writer left behind: {}",
            parts.left_behind.join(", ")
        ));
    }
    write_lines(&mut std::io::stdout().lock(), &lines)?;
    run.notes().print(&[next])?;
    Ok(ExitCode::SUCCESS)
}

/// The job's status on this node, when this node's head for it is the
/// snapshot `named` that the document written by `by` names. A refusal of
/// the job keeps the node's reason and names the node to resume from; a
/// head that differs is `stale_container`.
async fn held(client: &Client, job: JobId, by: &str, named: &str) -> Result<Box<JobStatus>> {
    let status = match client.call(&Request::JobStatus { job }, |_| {}).await {
        Ok(Reply::JobStatus(status)) => status,
        Ok(other) => {
            return Err(ClientError::Unexpected {
                op: Op::JobStatus,
                got: other.op(),
            }
            .into());
        }
        Err(ClientError::Refused(refusal)) => {
            let mut refused = Refused::from_refusal(&refusal);
            refused.message = format!(
                "{} The document was written by {by}: pair it, then run `fabric job resume {job} --on {by}`.",
                sentence(&refused.message)
            );
            return Err(refused.into());
        }
        Err(e) => return Err(e.into()),
    };
    let head = status.snapshot.as_ref().map(|s| s.id.to_string());
    if head.as_deref() == Some(named) {
        return Ok(status);
    }
    let mut refused = Refused::local(
        RefusalKind::JobState,
        "stale_container",
        format!(
            "The document names snapshot {named} of job {job}, and this node's head for it is {}. \
             Resume the job where the document was written ({by}), then export it again.",
            head.as_deref().unwrap_or("none"),
        ),
    );
    refused.data = Some(json!({
        "job": job.to_string(),
        "snapshot": named,
        "head": head,
        "produced_by": by,
    }));
    Err(refused.into())
}

/// `text` as a sentence that ends in a period, for another to follow it.
fn sentence(text: &str) -> String {
    let text = text.trim_end();
    if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// The harnesses this node's doctor reports, by the format's kind names,
/// with the version each version line names.
async fn installed(client: &Client) -> Vec<(String, Option<String>)> {
    let request = Request::Doctor {
        peer: None,
        job: None,
        env: fabric_client::api::ClientEnv::current().ok(),
    };
    let Ok(Reply::Doctor {
        report: DoctorReport::Facts { facts },
    }) = client.call(&request, |_| {}).await
    else {
        return Vec::new();
    };
    facts
        .iter()
        .filter_map(|fact| {
            let kind = match fact.name.as_str() {
                "claude" | "claude.version" => "claude_code",
                "codex" | "codex.version" => "codex",
                _ => return None,
            };
            Some((kind.to_owned(), version(&fact.value)))
        })
        .collect()
}

/// The version a `--version` line names, as the doctor and the resume
/// guard read it, pre-release and build included: `codex-cli
/// 0.163.0-alpha.10.2` names `0.163.0-alpha.10.2`.
fn version(text: &str) -> Option<String> {
    HarnessVersion::from_output(text)
        .ok()
        .map(|v| v.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn a_pre_release_version_line_names_its_version() {
        assert_eq!(
            version("codex-cli 0.163.0-alpha.10.2").as_deref(),
            Some("0.163.0-alpha.10.2")
        );
        assert_eq!(version("2.1.294 (Claude Code)").as_deref(), Some("2.1.294"));
        assert_eq!(version("codex-cli").as_deref(), None);
    }
}
