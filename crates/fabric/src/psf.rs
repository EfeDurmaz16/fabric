//! The Portable Session Format on the command line (ADR 0029):
//! `fabric job import --psf`, and `fabric psf inspect` and `verdicts`.
//! `fabric job export --psf` is a node request, in `job_request`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use fabric_client::api::{DoctorReport, Reply, Request};
use fabric_client::{Client, ClientError};
use fabric_core::id::JobId;
use fabric_core::version::HarnessVersion;
use fabric_psf::{Achieved, Container, Target, achievable, hex, read_container, read_manifest};

use crate::{Notes, write_lines};

/// Read a Portable Session Format container without a node.
#[derive(Debug, Subcommand)]
pub(crate) enum PsfCommand {
    /// Check a container or a manifest and print its digest, the fidelity
    /// it offers and the attachments it lacks.
    Inspect {
        /// A container directory, or a `manifest.json`.
        path: PathBuf,
    },
    /// Print one verdict line per fixture of a fixture set such as
    /// protocol/psf/v1, the lines the format's other readers print.
    #[command(hide = true)]
    Verdicts {
        /// The fixture set.
        dir: PathBuf,
    },
}

pub(crate) fn run(command: PsfCommand) -> Result<ExitCode> {
    match command {
        PsfCommand::Inspect { path } => {
            let (container, digest) = read(&path)?;
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
            let lines = fabric_psf::fixture_verdicts(&dir)?;
            let mut out = std::io::stdout().lock();
            for line in lines {
                writeln!(out, "{line}")?;
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Reads a container directory, or a lone manifest as a container with
/// every attachment detached.
fn read(path: &Path) -> Result<(Container, String)> {
    if path.is_dir() {
        let container =
            read_container(path).map_err(|r| anyhow!("{} is refused: {r}", path.display()))?;
        let digest = hex(&container.manifest.digest());
        return Ok((container, digest));
    }
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    let manifest =
        read_manifest(&bytes).map_err(|r| anyhow!("{} is refused: {r}", path.display()))?;
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
/// names; then it reports the fidelity this node achieves and the command
/// that continues the job. Otherwise it fails and names the node that
/// wrote the document, from which `fabric job resume --on` pulls it.
pub(crate) async fn import(client: &Client, home: &Path, dir: &Path) -> Result<ExitCode> {
    let container =
        read_container(dir).map_err(|r| anyhow!("{} is refused: {r}", dir.display()))?;
    let parts = container.manifest.parts();
    let job: JobId = parts
        .job
        .id
        .parse()
        .map_err(|e| anyhow!("job id {}: {e}", parts.job.id))?;
    let status = match client.call(&Request::JobStatus { job }, |_| {}).await {
        Ok(Reply::JobStatus(status)) => status,
        Ok(other) => return Err(anyhow!("the node answered {} to job_status", other.op())),
        Err(ClientError::Refused(refusal)) => {
            return Err(anyhow!(
                "{}; the document was written by {}: pair it, then run `fabric job resume {job} --on {}`",
                refusal.message(),
                parts.produced.by,
                parts.produced.by
            ));
        }
        Err(e) => return Err(e.into()),
    };
    let head = status.snapshot.as_ref().map(|s| s.id.to_string());
    if head.as_deref() != Some(parts.workspace.bundle_head.as_str()) {
        return Err(anyhow!(
            "the document names snapshot {} of job {job}, and this node's head for it is {}; \
             resume the job where the document was written ({}), then export it again",
            parts.workspace.bundle_head,
            head.as_deref().unwrap_or("none"),
            parts.produced.by
        ));
    }
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
    let mut lines = vec![format!(
        "{} matches job {job} at snapshot {}: offered {offered}, achieved here {achieved}",
        dir.display(),
        parts.workspace.bundle_head
    )];
    lines.extend(why.into_iter().map(|reason| format!("  {reason}")));
    if !parts.left_behind.is_empty() {
        lines.push(format!(
            "recreate what the writer left behind: {}",
            parts.left_behind.join(", ")
        ));
    }
    lines.push(format!("continue with `fabric job resume {job}`"));
    write_lines(&mut std::io::stdout().lock(), &lines)?;
    Ok(ExitCode::SUCCESS)
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
        assert_eq!(
            version("2.1.294 (Claude Code)").as_deref(),
            Some("2.1.294")
        );
        assert_eq!(version("codex-cli").as_deref(), None);
    }
}
