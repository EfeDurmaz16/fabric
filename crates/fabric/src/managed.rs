//! `fabric managed` and `fabric facade`: the managed jobs of ADR 0019 and
//! the M5 operations on them (docs/managed-jobs.md), through the client
//! API's managed calls, and the job facade of ADR 0027 through
//! `facade_call`.
//!
//! A managed command that changes a job carries an operation key, the
//! caller's name for the command: the node answers a key it has seen with
//! the answer it gave the first time and never runs the command twice, so a
//! script that lost an answer runs the same command again with the same
//! key. Without `--operation-key` the CLI makes a fresh ULID and names the
//! key it used, for such a retry: before the request in words, in the
//! answer's operation under `--json`, and in every failure's
//! `data.operation_key`, with the retry as the `next` command when the
//! answer was lost (`contract::Keyed`). A facade command carries its key the same
//! way, and its answer is the JSON the authority wrote, since that JSON
//! is the contract (docs/facade-protocol.md).

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use fabric_client::Client;
use fabric_client::api::{
    AnyWord, FidelityName, JobScopeWire, ManagedPolicy, ManagedSource, NoteWire, ProgramWire,
    RegisterClass, Reply, Request, ResourceScopeWire, SecretRefWire,
};
use fabric_client::facade::{Bearer, ClientMessage, Submit};
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{EventSeq, OperationKey, TaskId, WorkRevision};
use fabric_core::path::RelPath;

use crate::{Notes, ask, contract, harness_arg, repository, system_path};

// The source flags a create and a facade submit share. The flattened
// argument structs here carry plain comments: clap makes a doc comment
// the summary of the command that flattens the struct, and since the
// commands defer their arguments it would replace the command's own.
#[derive(Debug, Args, Clone)]
#[group(required = true, multiple = true)]
pub(crate) struct SourceArgs {
    /// A repository on this node to clone into the job's workspace.
    #[arg(long, conflicts_with_all = ["git", "snapshot", "empty", "import"], value_name = "PATH")]
    repo: Option<PathBuf>,
    /// A git URI (https, ssh, git+ssh or file) to clone; needs --commit.
    #[arg(long, requires = "commit", conflicts_with_all = ["snapshot", "empty", "import"], value_name = "URI")]
    git: Option<String>,
    /// The exact commit, 40 or 64 lowercase hex characters.
    #[arg(long, requires = "git", value_name = "COMMIT")]
    commit: Option<String>,
    /// A published snapshot to start from.
    #[arg(long, conflicts_with_all = ["empty", "import"], value_name = "SNAPSHOT")]
    snapshot: Option<SnapshotId>,
    /// Start from nothing.
    #[arg(long, conflicts_with = "import")]
    empty: bool,
    /// A directory on this node the node imports into a snapshot first, untouched.
    #[arg(long, value_name = "PATH")]
    import: Option<PathBuf>,
}

// The source flags of a facade submit: only what names the same thing on
// every node, since a path is a path on one node.
#[derive(Debug, Args, Clone)]
#[group(required = true, multiple = true)]
pub(crate) struct FacadeSourceArgs {
    /// Start from nothing.
    #[arg(long, conflicts_with_all = ["git", "snapshot"])]
    empty: bool,
    /// A published snapshot to start from.
    #[arg(long, conflicts_with = "git", value_name = "SNAPSHOT")]
    snapshot: Option<SnapshotId>,
    /// A git URI (https, ssh, git+ssh or file) to clone; needs --commit.
    #[arg(long, requires = "commit", value_name = "URI")]
    git: Option<String>,
    /// The exact commit, 40 or 64 lowercase hex characters.
    #[arg(long, requires = "git", value_name = "COMMIT")]
    commit: Option<String>,
}

impl From<FacadeSourceArgs> for SourceArgs {
    fn from(args: FacadeSourceArgs) -> Self {
        SourceArgs {
            repo: None,
            git: args.git,
            commit: args.commit,
            snapshot: args.snapshot,
            empty: args.empty,
            import: None,
        }
    }
}

// The policy flags.
#[derive(Debug, Args, Clone, Default)]
pub(crate) struct PolicyArgs {
    /// What the work must satisfy, in your words; repeatable.
    #[arg(long)]
    acceptance: Vec<String>,
    /// How long a quiescence may take, in milliseconds (1000 to 600000).
    #[arg(long)]
    drain_budget: Option<u32>,
    /// A continuation fidelity to allow, by its name on the fidelity ladder; repeatable.
    #[arg(long = "allow-fidelity", value_parser = fidelity_arg)]
    allow_fidelity: Vec<FidelityName>,
    /// A secret the tasks need by name ([A-Z0-9_]); repeatable.
    #[arg(long = "secret")]
    secrets: Vec<String>,
    /// A workspace path that may leave the node although it looks like a secret; repeatable.
    #[arg(long = "export")]
    export: Vec<String>,
    /// The free disk a node needs to run the job, in bytes.
    #[arg(long)]
    disk_bytes: Option<u64>,
    /// The one node the job may run on.
    #[arg(long)]
    pin: Option<NodeId>,
}

impl PolicyArgs {
    fn policy(&self) -> Option<ManagedPolicy> {
        let given = self.drain_budget.is_some()
            || !self.allow_fidelity.is_empty()
            || !self.secrets.is_empty()
            || !self.export.is_empty()
            || self.disk_bytes.is_some()
            || self.pin.is_some();
        given.then(|| ManagedPolicy {
            drain_budget_ms: self.drain_budget,
            allowed_fidelity: (!self.allow_fidelity.is_empty())
                .then(|| self.allow_fidelity.clone()),
            secrets: (!self.secrets.is_empty()).then(|| {
                self.secrets
                    .iter()
                    .map(|name| SecretRefWire {
                        name: name.clone(),
                        attempt: "any".into(),
                        resource_kinds: vec!["node".into()],
                    })
                    .collect()
            }),
            export_allowlist: (!self.export.is_empty()).then(|| self.export.clone()),
            disk_bytes: self.disk_bytes,
            pin: self.pin,
        })
    }
}

/// Make, cancel and read managed jobs.
#[derive(Debug, Subcommand)]
#[command(defer = true)]
#[expect(
    clippy::large_enum_variant,
    reason = "clap flattens the source and policy flags into the variants; one command is parsed per run"
)]
pub(crate) enum ManagedCommand {
    /// Make a managed job from a repository, a git revision, a snapshot, an imported directory or nothing.
    Create {
        #[command(flatten)]
        source: SourceArgs,
        /// The goal in your words.
        #[arg(long, value_name = "TEXT")]
        goal: String,
        /// The harness: claude, codex, opencode, pi, gemini, cursor or acp:<program>.
        #[arg(long, default_value = "claude", value_parser = harness_arg, value_name = "HARNESS")]
        harness: HarnessKind,
        #[command(flatten)]
        policy: PolicyArgs,
        /// The command's key; the same key and arguments again give the same job.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Cancel a managed job, or one task of it; a live attempt is stopped first.
    Cancel {
        /// The job.
        job: JobId,
        /// The task to cancel; the whole job without it.
        #[arg(long)]
        task: Option<TaskId>,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// Why, for the journal.
        #[arg(long, default_value = "", value_name = "TEXT")]
        reason: String,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Record a decision with its reason, a question, or an answer on a job's history.
    #[command(group(clap::ArgGroup::new("note").required(true).args(["question", "decision", "answer"])))]
    Note {
        /// The job.
        job: JobId,
        /// What was decided; needs --reason.
        #[arg(long, requires = "reason", conflicts_with_all = ["question", "answer"])]
        decision: Option<String>,
        /// Why it was decided.
        #[arg(long)]
        reason: Option<String>,
        /// A question left open.
        #[arg(long, conflicts_with = "answer")]
        question: Option<String>,
        /// The revision of the question this answers; needs --text.
        #[arg(long, requires = "text", value_parser = revision_arg, value_name = "REVISION")]
        answer: Option<WorkRevision>,
        /// The answer's text.
        #[arg(long)]
        text: Option<String>,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Register a build or test task with the program it runs under a task of a job.
    Register {
        /// The job.
        job: JobId,
        /// The task it is registered under.
        #[arg(long)]
        parent: TaskId,
        /// Your label for it.
        #[arg(long)]
        label: String,
        /// build or test.
        #[arg(long, value_parser = class_arg)]
        class: RegisterClass,
        /// Where it runs, relative to the attempt's root; the root without it.
        #[arg(long, value_parser = rel_path_arg)]
        cwd: Option<RelPath>,
        /// A variable of the node's environment it may see, by name; repeatable.
        #[arg(long = "env", value_parser = env_name_arg)]
        env: Vec<EnvVarName>,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
        /// The command and its arguments, after `--`.
        #[arg(last = true, required = true, num_args = 1..)]
        argv: Vec<String>,
    },
    /// Take a checkpoint cut of the job's live attempt; the attempt goes on.
    Checkpoint {
        /// The job.
        job: JobId,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Continue the job on a newly placed attempt: a quiescence, a cut, a placement.
    Continue {
        /// The job.
        job: JobId,
        /// A fidelity this continuation may achieve; repeatable; the job's allowance without it.
        #[arg(long = "allow-fidelity", value_parser = fidelity_arg)]
        allow_fidelity: Vec<FidelityName>,
        /// The one node the new attempt may run on.
        #[arg(long)]
        pin: Option<NodeId>,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Everything the journal holds about a job: placements, continuations, notes, provider operations.
    Inspect {
        /// The job.
        job: JobId,
    },
    /// Capture a directory on this node into a snapshot a job can start from, untouched.
    Import {
        /// The directory.
        dir: PathBuf,
        /// A path that may leave although it looks like a secret; repeatable.
        #[arg(long = "export")]
        export: Vec<String>,
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

/// Mint and revoke facade tokens, and reach a job's authority from any of your nodes.
#[derive(Debug, Subcommand)]
#[command(defer = true)]
#[expect(
    clippy::large_enum_variant,
    reason = "clap flattens the source and policy flags into the variants; one command is parsed per run"
)]
pub(crate) enum FacadeCommand {
    /// Mint a token on this node's authority; the bearer is printed once.
    #[command(group(clap::ArgGroup::new("expiry").required(true).args(["ttl_ms", "expires_ms"])))]
    Mint {
        /// Who acts under the token: 1 to 64 characters of [A-Za-z0-9._-].
        #[arg(long)]
        actor: String,
        /// A job the token may act on; repeatable; every owned job without it.
        #[arg(long = "job")]
        jobs: Vec<JobId>,
        /// A node the token may pin or place on; repeatable; any node without it.
        #[arg(long = "node")]
        nodes: Vec<NodeId>,
        /// When the token stops working, in Unix milliseconds.
        #[arg(long, conflicts_with = "ttl_ms")]
        expires_ms: Option<u64>,
        /// How long the token works from now, in milliseconds.
        #[arg(long)]
        ttl_ms: Option<u64>,
    },
    /// Revoke a token by its id.
    Revoke {
        /// The token.
        token_id: String,
    },
    /// Submit a job to an authority.
    Submit {
        #[command(flatten)]
        to: Authority,
        #[command(flatten)]
        source: FacadeSourceArgs,
        /// The goal in your words.
        #[arg(long, value_name = "TEXT")]
        goal: String,
        /// The harness: claude, codex or acp:<program>.
        #[arg(long, default_value = "claude", value_parser = harness_arg, value_name = "HARNESS")]
        harness: HarnessKind,
        #[command(flatten)]
        policy: PolicyArgs,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Inspect a job at its authority.
    Inspect {
        #[command(flatten)]
        to: Authority,
        /// The job.
        job: JobId,
    },
    /// A page of a job's events after a cursor.
    Watch {
        #[command(flatten)]
        to: Authority,
        /// The job.
        job: JobId,
        /// The last event sequence you have; the first event without it.
        #[arg(long, value_parser = seq_arg)]
        after: Option<EventSeq>,
        /// How long the authority waits for a first event, in milliseconds (0 to 25000).
        #[arg(long)]
        wait_ms: Option<u32>,
    },
    /// Cancel a job or one task at its authority.
    Cancel {
        #[command(flatten)]
        to: Authority,
        /// The job.
        job: JobId,
        /// The task; the whole job without it.
        #[arg(long)]
        task: Option<TaskId>,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// Why.
        #[arg(long, default_value = "", value_name = "TEXT")]
        reason: String,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Checkpoint a job at its authority.
    Checkpoint {
        #[command(flatten)]
        to: Authority,
        /// The job.
        job: JobId,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
    /// Continue a job at its authority.
    Continue {
        #[command(flatten)]
        to: Authority,
        /// The job.
        job: JobId,
        /// A fidelity this continuation may achieve; repeatable.
        #[arg(long = "allow-fidelity", value_parser = fidelity_arg)]
        allow_fidelity: Vec<FidelityName>,
        /// The one node the new attempt may run on.
        #[arg(long)]
        pin: Option<NodeId>,
        /// Refuse unless the job is at this revision.
        #[arg(long, value_parser = revision_arg, value_name = "REVISION")]
        expected_revision: Option<WorkRevision>,
        /// The command's key; the same key and arguments again give the same answer.
        #[arg(long, value_parser = operation_key_arg, value_name = "KEY")]
        operation_key: Option<OperationKey>,
    },
}

// Which authority a facade message goes to, and the bearer it carries.
#[derive(Debug, Args, Clone)]
pub(crate) struct Authority {
    /// The authority node; this node's own id is allowed.
    #[arg(long)]
    authority: NodeId,
    /// The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it.
    #[arg(long, value_parser = bearer_arg)]
    token: Bearer,
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

fn class_arg(text: &str) -> Result<RegisterClass, String> {
    match text {
        "build" => Ok(RegisterClass::Build),
        "test" => Ok(RegisterClass::Test),
        other => Err(format!("a registered task is build or test, not {other:?}")),
    }
}

fn rel_path_arg(text: &str) -> Result<RelPath, String> {
    RelPath::parse(text).map_err(|e| format!("{text:?}: {e}"))
}

fn env_name_arg(text: &str) -> Result<EnvVarName, String> {
    EnvVarName::parse(text).map_err(|e| format!("{text:?}: {e}"))
}

fn seq_arg(text: &str) -> Result<EventSeq, String> {
    text.parse::<u64>()
        .ok()
        .and_then(EventSeq::new)
        .ok_or_else(|| format!("an event sequence is a whole number of at least 1, not {text:?}"))
}

fn fidelity_arg(text: &str) -> Result<FidelityName, String> {
    FidelityName::ALL
        .into_iter()
        .find(|f| f.name() == text)
        .ok_or_else(|| {
            format!(
                "a fidelity is one of {}, not {text:?}",
                FidelityName::ALL
                    .iter()
                    .map(|f| f.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn bearer_arg(text: &str) -> Result<Bearer, String> {
    Bearer::parse_hex(text).map_err(|e| e.to_string())
}

/// The key a changing command runs under: the caller's, or a fresh one,
/// printed so a retry can name it and kept in `minted` for its failure.
fn key_for(
    given: Option<OperationKey>,
    notes: Notes,
    minted: &mut Option<OperationKey>,
) -> Result<OperationKey> {
    if let Some(key) = given {
        return Ok(key);
    }
    let key = fabricd::clock::operation_key()
        .map_err(|e| anyhow!("no operation key could be made: {e}"))?;
    // Under `--json` the key is in the answer's operation, or in the failure's data.
    notes.print(&[format!(
        "operation key {key}; run the command again with --operation-key {key} after a lost answer"
    )])?;
    *minted = Some(key.clone());
    Ok(key)
}

/// The source the flags name; a repository needs its path on this node.
fn source_of(args: &SourceArgs) -> Result<ManagedSource> {
    if let Some(repo) = &args.repo {
        return Ok(ManagedSource::Repository {
            path: repository(repo)?,
        });
    }
    if let (Some(uri), Some(commit)) = (&args.git, &args.commit) {
        return Ok(ManagedSource::Git {
            uri: uri.clone(),
            commit: commit.clone(),
        });
    }
    if let Some(snapshot) = args.snapshot {
        return Ok(ManagedSource::Snapshot { snapshot });
    }
    if let Some(dir) = &args.import {
        return Ok(ManagedSource::Directory {
            path: system_path(dir)?,
        });
    }
    if args.empty {
        return Ok(ManagedSource::Empty);
    }
    Err(anyhow!(
        "name a source: --repo <path>, --git <uri> --commit <id>, --snapshot <id>, --import <dir> or --empty"
    ))
}

fn note_of(
    decision: Option<String>,
    reason: Option<String>,
    question: Option<String>,
    answer: Option<WorkRevision>,
    text: Option<String>,
) -> Result<NoteWire> {
    match (decision, question, answer) {
        (Some(what), None, None) => Ok(NoteWire::Decision {
            what,
            reason: reason.unwrap_or_default(),
        }),
        (None, Some(text), None) => Ok(NoteWire::Question { text }),
        (None, None, Some(question)) => Ok(NoteWire::Answer {
            question,
            text: text.unwrap_or_default(),
        }),
        _ => Err(anyhow!(
            "a note is one of --decision <what> --reason <why>, --question <text>, or --answer <revision> --text <text>"
        )),
    }
}

/// The request `command` sends, and the key the CLI made for it, if it made one.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command, so the whole mapping reads in one place"
)]
fn request(command: ManagedCommand, notes: Notes) -> Result<(Request, Option<OperationKey>)> {
    let mut minted = None;
    let request = match command {
        ManagedCommand::Create {
            source,
            goal,
            harness,
            policy,
            operation_key,
        } => {
            let operation_key = key_for(operation_key, notes, &mut minted)?;
            let acceptance = policy.acceptance.clone();
            let policy = policy.policy();
            match source_of(&source)? {
                ManagedSource::Repository { path } => Request::ManagedCreate {
                    operation_key,
                    repo: path,
                    goal,
                    harness,
                    acceptance,
                    policy,
                },
                source => Request::ManagedSubmit {
                    operation_key,
                    goal,
                    harness,
                    acceptance,
                    source,
                    policy,
                },
            }
        }
        ManagedCommand::Cancel {
            job,
            task,
            expected_revision,
            reason,
            operation_key,
        } => Request::ManagedCancel {
            operation_key: key_for(operation_key, notes, &mut minted)?,
            job,
            expected_revision,
            reason,
            task,
        },
        ManagedCommand::Note {
            job,
            decision,
            reason,
            question,
            answer,
            text,
            expected_revision,
            operation_key,
        } => Request::ManagedNote {
            operation_key: key_for(operation_key, notes, &mut minted)?,
            job,
            expected_revision,
            note: note_of(decision, reason, question, answer, text)?,
        },
        ManagedCommand::Register {
            job,
            parent,
            label,
            class,
            cwd,
            env,
            expected_revision,
            operation_key,
            argv,
        } => Request::ManagedRegister {
            operation_key: key_for(operation_key, notes, &mut minted)?,
            job,
            expected_revision,
            parent,
            label,
            class,
            program: ProgramWire {
                argv,
                cwd,
                env_allowlist: env,
            },
        },
        ManagedCommand::Checkpoint {
            job,
            expected_revision,
            operation_key,
        } => Request::ManagedCheckpoint {
            operation_key: key_for(operation_key, notes, &mut minted)?,
            job,
            expected_revision,
        },
        ManagedCommand::Continue {
            job,
            allow_fidelity,
            pin,
            expected_revision,
            operation_key,
        } => Request::ManagedContinue {
            operation_key: key_for(operation_key, notes, &mut minted)?,
            job,
            expected_revision,
            fidelity: (!allow_fidelity.is_empty()).then_some(allow_fidelity),
            pin,
        },
        ManagedCommand::Inspect { job } => Request::ManagedInspect { job },
        ManagedCommand::Import { dir, export } => Request::ManagedImport {
            path: system_path(&dir)?,
            export_allowlist: export,
        },
        ManagedCommand::Status { job } => Request::ManagedStatus { job },
        ManagedCommand::Ls => Request::ManagedList,
    };
    Ok((request, minted))
}

/// Runs `command` against the node `client` speaks to and returns its answer.
pub(crate) async fn run(
    client: &Client,
    command: ManagedCommand,
    run: &contract::Run,
) -> Result<Reply> {
    let (request, minted) = request(command, run.notes())?;
    ask(client, &request, run.notes())
        .await
        .map_err(|error| match minted {
            Some(key) => contract::Keyed {
                key: key.as_str().to_owned(),
                error,
            }
            .into(),
            None => error,
        })
}

/// The request a facade command sends: a mint or a revoke to this node,
/// or a facade message for an authority; and the key the CLI made for it,
/// if it made one.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per facade command, so the whole mapping reads in one place"
)]
fn facade_request(command: FacadeCommand, notes: Notes) -> Result<(Request, Option<OperationKey>)> {
    let mut minted = None;
    let message = |to: Authority, message: ClientMessage| Request::FacadeCall {
        authority: to.authority,
        message: message.to_json(),
    };
    let request = match command {
        FacadeCommand::Mint {
            actor,
            jobs,
            nodes,
            expires_ms,
            ttl_ms,
        } => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let expires_ms = match (expires_ms, ttl_ms) {
                (Some(at), _) => at,
                (None, Some(ttl)) => now.saturating_add(ttl),
                (None, None) => return Err(anyhow!("name --expires-ms or --ttl-ms")),
            };
            Request::FacadeMint {
                actor,
                jobs: if jobs.is_empty() {
                    JobScopeWire::Any(AnyWord::Any)
                } else {
                    JobScopeWire::Jobs(jobs)
                },
                resources: if nodes.is_empty() {
                    ResourceScopeWire::Any(AnyWord::Any)
                } else {
                    ResourceScopeWire::Nodes(nodes)
                },
                expires_ms,
            }
        }
        FacadeCommand::Revoke { token_id } => Request::FacadeRevoke { token_id },
        FacadeCommand::Submit {
            to,
            source,
            goal,
            harness,
            policy,
            operation_key,
        } => {
            let source = source_of(&source.into())?;
            let token = to.token.clone();
            message(
                to,
                ClientMessage::Submit {
                    token,
                    key: key_for(operation_key, notes, &mut minted)?,
                    submit: Submit {
                        goal,
                        harness,
                        acceptance: policy.acceptance.clone(),
                        source,
                        policy: policy.policy(),
                    },
                },
            )
        }
        FacadeCommand::Inspect { to, job } => {
            let token = to.token.clone();
            message(to, ClientMessage::Inspect { token, job })
        }
        FacadeCommand::Watch {
            to,
            job,
            after,
            wait_ms,
        } => {
            let token = to.token.clone();
            message(
                to,
                ClientMessage::Watch {
                    token,
                    job,
                    after,
                    wait_ms,
                },
            )
        }
        FacadeCommand::Cancel {
            to,
            job,
            task,
            expected_revision,
            reason,
            operation_key,
        } => {
            let token = to.token.clone();
            message(
                to,
                ClientMessage::Cancel {
                    token,
                    key: key_for(operation_key, notes, &mut minted)?,
                    job,
                    expected_revision,
                    task,
                    reason,
                },
            )
        }
        FacadeCommand::Checkpoint {
            to,
            job,
            expected_revision,
            operation_key,
        } => {
            let token = to.token.clone();
            message(
                to,
                ClientMessage::Checkpoint {
                    token,
                    key: key_for(operation_key, notes, &mut minted)?,
                    job,
                    expected_revision,
                },
            )
        }
        FacadeCommand::Continue {
            to,
            job,
            allow_fidelity,
            pin,
            expected_revision,
            operation_key,
        } => {
            let token = to.token.clone();
            message(
                to,
                ClientMessage::Continue {
                    token,
                    key: key_for(operation_key, notes, &mut minted)?,
                    job,
                    expected_revision,
                    fidelity: (!allow_fidelity.is_empty()).then_some(allow_fidelity),
                    pin,
                },
            )
        }
    };
    Ok((request, minted))
}

/// Runs a facade command and returns its answer; a `refused` answer from
/// the authority is an error, since the command did not happen.
pub(crate) async fn run_facade(
    client: &Client,
    command: FacadeCommand,
    run: &contract::Run,
) -> Result<Reply> {
    let (request, minted) = facade_request(command, run.notes())?;
    let keyed = |error: anyhow::Error| match &minted {
        Some(key) => contract::Keyed {
            key: key.as_str().to_owned(),
            error,
        }
        .into(),
        None => error,
    };
    let reply = ask(client, &request, run.notes()).await.map_err(keyed)?;
    if let Reply::FacadeCall(envelope) = &reply
        && envelope
            .message
            .get("type")
            .and_then(serde_json::Value::as_str)
            == Some("refused")
    {
        let code = envelope.message["code"].as_str().unwrap_or("refused");
        let reason = envelope.message["reason"].as_str().unwrap_or("");
        return Err(keyed(anyhow!("the authority refused: {code}/{reason}")));
    }
    Ok(reply)
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
            task,
            ..
        } = request(given, Notes::Silent).unwrap().0
        else {
            panic!("not a cancel");
        };
        assert_eq!(operation_key.as_str(), "retry-1");
        assert_eq!(expected_revision, WorkRevision::new(3));
        assert_eq!(task, None);

        let fresh = || {
            let command = parsed(&["fabric", "managed", "cancel", "01JABCDEFGHJKMNPQRSTVWXYZ0"]);
            match request(command, Notes::Silent).unwrap() {
                (Request::ManagedCancel { operation_key, .. }, Some(minted)) => {
                    assert_eq!(operation_key, minted, "the key sent is the key kept");
                    operation_key
                }
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

    /// A create from a git revision, a snapshot or nothing travels as
    /// `managed_submit` with the policy the flags name; from a repository
    /// as `managed_create` with the same policy; the sources exclude each
    /// other; a fidelity name outside the ladder is refused by the parser.
    #[test]
    fn a_create_names_one_source_and_its_policy() {
        let command = parsed(&[
            "fabric",
            "managed",
            "create",
            "--git",
            "https://example.com/org/repo.git",
            "--commit",
            "0123456789abcdef0123456789abcdef01234567",
            "--goal",
            "fix",
            "--harness",
            "codex",
            "--acceptance",
            "tests pass",
            "--allow-fidelity",
            "workspace_only",
            "--secret",
            "API_TOKEN",
            "--disk-bytes",
            "1024",
            "--operation-key",
            "k1",
        ]);
        let Request::ManagedSubmit {
            source,
            acceptance,
            policy,
            ..
        } = request(command, Notes::Silent).unwrap().0
        else {
            panic!("not a submit");
        };
        assert!(matches!(source, ManagedSource::Git { .. }));
        assert_eq!(acceptance, vec!["tests pass".to_owned()]);
        let policy = policy.unwrap();
        assert_eq!(
            policy.allowed_fidelity,
            Some(vec![FidelityName::WorkspaceOnly])
        );
        assert_eq!(policy.secrets.unwrap()[0].name, "API_TOKEN");
        assert_eq!(policy.disk_bytes, Some(1024));
        assert_eq!(policy.pin, None);

        let empty = parsed(&[
            "fabric",
            "managed",
            "create",
            "--empty",
            "--goal",
            "g",
            "--operation-key",
            "k2",
        ]);
        assert!(matches!(
            request(empty, Notes::Silent).unwrap().0,
            Request::ManagedSubmit {
                source: ManagedSource::Empty,
                policy: None,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from([
                "fabric",
                "managed",
                "create",
                "--empty",
                "--snapshot",
                &"a".repeat(64),
                "--goal",
                "g"
            ])
            .is_err(),
            "two sources"
        );
        assert!(
            Cli::try_parse_from([
                "fabric",
                "managed",
                "create",
                "--empty",
                "--goal",
                "g",
                "--allow-fidelity",
                "best"
            ])
            .is_err(),
            "not a fidelity"
        );
        assert!(
            Cli::try_parse_from(["fabric", "managed", "create", "--goal", "g"]).is_err(),
            "no source named"
        );
    }

    /// A facade submit takes only a source that names the same thing on every node.
    #[test]
    fn a_facade_submit_takes_no_path() {
        assert!(
            Cli::try_parse_from([
                "fabric",
                "facade",
                "submit",
                "--authority",
                "fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq",
                "--token",
                &"ab".repeat(32),
                "--repo",
                ".",
                "--goal",
                "g"
            ])
            .is_err(),
            "a path is not a facade source"
        );
    }

    /// A facade command builds the message docs/facade-protocol.md gives
    /// and carries it unchanged in `facade_call`; the bearer never prints.
    #[test]
    fn a_facade_command_carries_its_message_unchanged() {
        let token = "1".repeat(64);
        let command = match Cli::try_parse_from([
            "fabric",
            "facade",
            "watch",
            "--authority",
            "fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq",
            "--token",
            &token,
            "01K8W3A00000000000000000J1",
            "--after",
            "3",
        ])
        .unwrap()
        .command
        {
            Command::Facade(command) => command,
            other => panic!("{other:?}"),
        };
        let Request::FacadeCall { message, .. } = facade_request(command, Notes::Silent).unwrap().0
        else {
            panic!("not a facade call");
        };
        assert_eq!(message["type"], "watch");
        assert_eq!(message["facade"], 1);
        assert_eq!(message["after"], "3");
        assert_eq!(message["token"], token);
        assert!(message.get("wait_ms").is_none());
        assert!(!format!("{:?}", Bearer::parse_hex(&token).unwrap()).contains('1'));
    }
}
