//! `fabric managed`: the managed jobs of ADR 0019, through the client API's
//! managed calls (docs/managed-jobs.md).
//!
//! A managed command that changes a job carries an operation key, the
//! caller's name for the command: the node answers a key it has seen with
//! the answer it gave the first time and never runs the command twice, so a
//! script that lost an answer runs the same command again with the same
//! key. Without `--operation-key` the CLI makes a fresh ULID and prints the
//! key it used, for such a retry.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use fabric_client::Client;
use fabric_client::api::{Reply, Request};
use fabric_core::harness::HarnessKind;
use fabric_core::id::JobId;
use fabric_core::managed::{OperationKey, WorkRevision};

use crate::{Notes, ask, harness_arg, repository};

/// Make, cancel and read managed jobs.
#[derive(Debug, Subcommand)]
pub(crate) enum ManagedCommand {
    /// Make a managed job from a repository.
    Create {
        /// The repository to clone into the job's workspace.
        #[arg(long)]
        repo: PathBuf,
        /// The goal in your words.
        #[arg(long)]
        goal: String,
        /// The harness: claude, codex or acp:<program>.
        #[arg(long, default_value = "claude", value_parser = harness_arg)]
        harness: HarnessKind,
        /// The command's key; the same key and arguments again give the same job.
        #[arg(long, value_parser = operation_key_arg)]
        operation_key: Option<OperationKey>,
    },
    /// Cancel a managed job none of whose attempts may still run.
    Cancel {
        /// The job.
        job: JobId,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg)]
        expected_revision: Option<WorkRevision>,
        /// Why, for the journal.
        #[arg(long, default_value = "")]
        reason: String,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg)]
        operation_key: Option<OperationKey>,
    },
    /// One managed job: its revision, state, tasks and attempts.
    Status {
        /// The job.
        job: JobId,
    },
    /// Every managed job this node's authority holds.
    #[command(visible_alias = "list")]
    Ls,
}

fn operation_key_arg(text: &str) -> Result<OperationKey, String> {
    OperationKey::parse(text).map_err(|e| e.to_string())
}

fn revision_arg(text: &str) -> Result<WorkRevision, String> {
    text.parse::<u64>()
        .ok()
        .and_then(WorkRevision::new)
        .ok_or_else(|| format!("a revision is a whole number of at least 1, not {text:?}"))
}

/// The key a changing command runs under: the caller's, or a fresh one,
/// printed so a retry can name it.
fn key_for(given: Option<OperationKey>) -> Result<OperationKey> {
    if let Some(key) = given {
        return Ok(key);
    }
    let key = fabricd::clock::operation_key()
        .map_err(|e| anyhow!("no operation key could be made: {e}"))?;
    Notes::Stderr.print(&[format!(
        "operation key {key}; run the command again with --operation-key {key} after a lost answer"
    )])?;
    Ok(key)
}

/// The request `command` sends.
fn request(command: ManagedCommand) -> Result<Request> {
    Ok(match command {
        ManagedCommand::Create {
            repo,
            goal,
            harness,
            operation_key,
        } => Request::ManagedCreate {
            operation_key: key_for(operation_key)?,
            repo: repository(&repo)?,
            goal,
            harness,
        },
        ManagedCommand::Cancel {
            job,
            expected_revision,
            reason,
            operation_key,
        } => Request::ManagedCancel {
            operation_key: key_for(operation_key)?,
            job,
            expected_revision,
            reason,
        },
        ManagedCommand::Status { job } => Request::ManagedStatus { job },
        ManagedCommand::Ls => Request::ManagedList,
    })
}

/// Runs `command` against the node `client` speaks to and prints its answer.
pub(crate) async fn run(client: &Client, command: ManagedCommand) -> Result<Reply> {
    ask(client, &request(command)?, Notes::Stdout).await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command};

    fn parsed(args: &[&str]) -> ManagedCommand {
        match Cli::try_parse_from(args).unwrap().command {
            Command::Managed(command) => command,
            other => panic!("{other:?}"),
        }
    }

    /// A key given is the command's key; none given makes a fresh one per
    /// invocation, so two runs never share a key by accident.
    #[test]
    fn a_changing_command_carries_the_key_given_or_a_fresh_one() {
        let given = parsed(&[
            "fabric",
            "managed",
            "cancel",
            "01JABCDEFGHJKMNPQRSTVWXYZ0",
            "--operation-key",
            "retry-1",
            "--expected-revision",
            "3",
        ]);
        let Request::ManagedCancel {
            operation_key,
            expected_revision,
            ..
        } = request(given).unwrap()
        else {
            panic!("not a cancel");
        };
        assert_eq!(operation_key.as_str(), "retry-1");
        assert_eq!(expected_revision, WorkRevision::new(3));

        let fresh = || {
            let command = parsed(&["fabric", "managed", "cancel", "01JABCDEFGHJKMNPQRSTVWXYZ0"]);
            match request(command).unwrap() {
                Request::ManagedCancel { operation_key, .. } => operation_key,
                other => panic!("{other:?}"),
            }
        };
        assert_ne!(fresh(), fresh());
        assert!(
            Cli::try_parse_from([
                "fabric",
                "managed",
                "cancel",
                "01JABCDEFGHJKMNPQRSTVWXYZ0",
                "--operation-key",
                "not a key"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "fabric",
                "managed",
                "cancel",
                "01JABCDEFGHJKMNPQRSTVWXYZ0",
                "--expected-revision",
                "0"
            ])
            .is_err()
        );
        assert!(matches!(
            parsed(&["fabric", "managed", "list"]),
            ManagedCommand::Ls
        ));
    }
}
