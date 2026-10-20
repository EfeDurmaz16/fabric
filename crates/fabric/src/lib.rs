//! The `fabric` command, as a library so its tests can build it with the
//! node's fault points; `main.rs` only calls [`main`].
//!
//! Every command but `serve` is a client of the node daemon on this
//! machine: it sends one request over the control socket in the state
//! directory and prints the lines the daemon answers with. The one thing
//! the CLI does itself is the harness, because an interactive agent needs
//! the user's terminal and the user's shell environment and a daemon has
//! neither (ADR 0003, Who supervises the harness): it materializes the
//! environment, launches the harness in its own process group, tells the
//! daemon, which records `Resumed` under the planned epoch or refuses, and
//! waits for the harness to exit.
//!
//! The exit code says what happened without parsing text (ADR 0016, Exit
//! codes): 0 for success, the harness's own code when it exits, 128 plus
//! the signal when it was stopped by one, 80 to 85 for a refusal of a kind
//! the daemon names, and 1 for any other failure.

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use fabric_core::env::{EnvVarName, SetupArg};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_env::materialize::{CacheHealth, NodeEnv, TreeLock};
use fabric_harness::claude::{ClaudeCode, LeftReason, TrustOutcome};
use fabric_harness::codex::Codex;
use fabric_harness::{Harness, Launch, Running};
use fabric_net::endpoint::DialOrder;
use fabricd::control::{self, ControlError, NO_DAEMON_EXIT, Refusal, Request, Response, RunPlan};
use fabricd::harnesses::{HarnessConfig, HarnessEnv, acp_adapter, parse_harness};
use fabricd::node::{Node, NodeConfig};
use fabricd::paths::{STATE_DIR_ENV, StateDir};

/// Move a development job between your machines and harnesses.
#[derive(Debug, Parser)]
#[command(name = "fabric", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node daemon in the foreground.
    Serve(ServeArgs),
    /// This node's identity and its paired nodes.
    #[command(subcommand)]
    Node(NodeCommand),
    /// Make, run, suspend, move and resume jobs.
    #[command(subcommand)]
    Job(JobCommand),
    /// Print this node's facts, or compare them with a paired node's before a move.
    Doctor {
        /// The node to compare with.
        #[arg(long)]
        peer: Option<NodeId>,
        /// The job a move would carry; only with --peer.
        #[arg(long, requires = "peer")]
        job: Option<JobId>,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Where both transports listen.
    #[arg(long, default_value = "0.0.0.0:7777")]
    listen: SocketAddr,
    /// Which wires this node dials: QUIC then TCP+TLS, or only one of them.
    #[arg(long, value_enum, default_value_t = Wire::Auto)]
    wire: Wire,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Wire {
    /// QUIC first, TCP with TLS when QUIC does not connect.
    Auto,
    /// QUIC only.
    Quic,
    /// TCP with TLS only.
    TcpTls,
}

#[derive(Debug, Subcommand)]
enum NodeCommand {
    /// Print this node's id and address.
    Id,
    /// Pair a node; give its address when this node can dial it.
    Add {
        /// The node's id, as `fabric node id` prints it there.
        node: NodeId,
        /// Where it listens, `host:port`; leave it out for a node behind a NAT that dials in.
        addr: Option<String>,
    },
    /// List the paired nodes.
    Ls,
    /// Dial a paired node and ping it.
    Ping {
        /// The node.
        node: NodeId,
    },
}

#[derive(Debug, Subcommand)]
enum JobCommand {
    /// Make a job from a repository, rooted at ~/fabric/jobs/<id>/work.
    New {
        /// The repository to clone into the job's root.
        #[arg(long)]
        repo: PathBuf,
        /// The goal in your words.
        #[arg(long)]
        goal: String,
        /// The harness: claude, codex or acp:<program>.
        #[arg(long, default_value = "claude", value_parser = harness_arg)]
        harness: HarnessKind,
    },
    /// Run the job's harness here, attached to this terminal.
    Run {
        /// The job.
        job: JobId,
        /// The first message to send; the goal for a new session.
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Stop the harness, snapshot the workspace and record the suspend.
    Suspend {
        /// The job.
        job: JobId,
    },
    /// Pull the job here if another node owns it, restore it, and run it.
    Resume {
        /// The job.
        job: JobId,
        /// The node that holds the job, when this node has never seen it.
        #[arg(long)]
        on: Option<NodeId>,
        /// Continue under another harness: claude, codex or acp:<program>.
        #[arg(long, value_parser = harness_arg)]
        harness: Option<HarnessKind>,
        /// The first message to send.
        #[arg(long)]
        prompt: Option<String>,
        /// Waive the preflight refusals that can be waived when pulling.
        #[arg(long)]
        force: bool,
    },
    /// Move the job to a paired node by the cooperative move protocol.
    Move {
        /// The job.
        job: JobId,
        /// The node to move it to.
        #[arg(long)]
        to: NodeId,
        /// Waive the preflight refusals that can be waived.
        #[arg(long)]
        force: bool,
    },
    /// Take the job without its owner's consent; prints what it would do without --force.
    Steal {
        /// The job.
        job: JobId,
        /// Do it.
        #[arg(long)]
        force: bool,
        /// Why, for the ledger.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// The job's state on this node.
    Status {
        /// The job.
        job: JobId,
    },
    /// Every job this node knows.
    Ls,
    /// The job's ledger.
    Log {
        /// The job.
        job: JobId,
    },
    /// Record an approval in the ledger.
    Approve {
        /// The job.
        job: JobId,
        /// Your words.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Record a rejection in the ledger.
    Reject {
        /// The job.
        job: JobId,
        /// Your words.
        #[arg(long, default_value = "")]
        note: String,
    },
}

fn harness_arg(text: &str) -> Result<HarnessKind, String> {
    parse_harness(text).map_err(|e| e.to_string())
}

/// Where this user's files live: `$HOME`, resolved to its physical path.
fn home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    std::fs::canonicalize(&home).with_context(|| format!("HOME {}", Path::new(&home).display()))
}

fn state_dir(home: &Path) -> Result<StateDir> {
    Ok(StateDir::resolve(
        std::env::var_os(STATE_DIR_ENV).as_deref(),
        home,
    )?)
}

fn path_var() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

fn print_lines(lines: &[String]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        writeln!(stdout, "{line}")?;
    }
    Ok(())
}

/// A refusal the daemon answered, kept typed so `main` can pick the exit code.
#[derive(Debug)]
struct Refused {
    refusal: Refusal,
    message: String,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

/// The exit code for an error that ended a command.
fn exit_code(error: &anyhow::Error) -> ExitCode {
    if let Some(refused) = error.downcast_ref::<Refused>() {
        return ExitCode::from(refused.refusal.exit_code());
    }
    if let Some(ControlError::NoDaemon { .. }) = error.downcast_ref::<ControlError>() {
        return ExitCode::from(NO_DAEMON_EXIT);
    }
    ExitCode::FAILURE
}

/// Sends a request and prints what the daemon printed; a refusal becomes an error.
async fn ask(socket: &Path, request: Request) -> Result<Option<RunPlan>> {
    match control::call(socket, &request).await? {
        Response::Done { lines } => {
            print_lines(&lines)?;
            Ok(None)
        }
        Response::Run { lines, plan } => {
            print_lines(&lines)?;
            Ok(Some(plan))
        }
        Response::Failed {
            lines,
            error,
            refusal,
        } => {
            print_lines(&lines)?;
            Err(Refused {
                refusal,
                message: error,
            }
            .into())
        }
    }
}

/// Parses the command line, runs the command and returns the exit code.
#[must_use]
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let default = if matches!(cli.command, Command::Serve(_)) {
        tracing::Level::INFO
    } else {
        tracing::Level::WARN
    };
    let level = std::env::var("FABRIC_LOG")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(default);
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(level)
        .init();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!(error = %e, "no async runtime");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "error: {e:#}");
            exit_code(&e)
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let home = home()?;
    let state = state_dir(&home)?;
    let socket = state.socket();
    match cli.command {
        Command::Serve(args) => serve(state, home, args).await,
        Command::Node(command) => {
            let request = match command {
                NodeCommand::Id => Request::NodeId,
                NodeCommand::Add { node, addr } => Request::NodeAdd { node, addr },
                NodeCommand::Ls => Request::NodeList,
                NodeCommand::Ping { node } => Request::NodePing { node },
            };
            ask(&socket, request).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { peer, job } => {
            ask(
                &socket,
                Request::Doctor {
                    peer,
                    job,
                    path_var: path_var(),
                },
            )
            .await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Job(command) => job(&state, &home, command).await,
    }
}

async fn serve(state: StateDir, home: PathBuf, args: ServeArgs) -> Result<ExitCode> {
    let dial = match args.wire {
        Wire::Auto => DialOrder::QuicThenTcp,
        Wire::Quic => DialOrder::QuicOnly,
        Wire::TcpTls => DialOrder::TcpOnly,
    };
    let node = Node::start(NodeConfig {
        state,
        home,
        listen: args.listen,
        dial,
        path_var: path_var(),
        cargo_home: std::env::var_os("CARGO_HOME").map(PathBuf::from),
    })?;
    {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "node {}", node.id())?;
        writeln!(stdout, "listening on {}", node.local_addr()?)?;
        stdout.flush()?;
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        ended = node.run() => ended?,
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    Ok(ExitCode::SUCCESS)
}

async fn job(state: &StateDir, home: &Path, command: JobCommand) -> Result<ExitCode> {
    let socket = state.socket();
    let (request, prompt) = match command {
        JobCommand::New {
            repo,
            goal,
            harness,
        } => {
            let repo = std::fs::canonicalize(&repo)
                .with_context(|| format!("repository {}", repo.display()))?;
            (
                Request::JobNew {
                    repo,
                    goal,
                    harness,
                },
                None,
            )
        }
        JobCommand::Run { job, prompt } => (Request::JobRun { job }, prompt),
        JobCommand::Suspend { job } => (
            Request::JobSuspend {
                job,
                path_var: path_var(),
            },
            None,
        ),
        JobCommand::Resume {
            job,
            on,
            harness,
            prompt,
            force,
        } => (
            Request::JobResume {
                job,
                on,
                harness,
                force,
                path_var: path_var(),
            },
            prompt,
        ),
        JobCommand::Move { job, to, force } => (
            Request::JobMove {
                job,
                to,
                force,
                path_var: path_var(),
            },
            None,
        ),
        JobCommand::Steal { job, force, reason } => {
            (Request::JobSteal { job, reason, force }, None)
        }
        JobCommand::Status { job } => (Request::JobStatus { job }, None),
        JobCommand::Ls => (Request::JobList, None),
        JobCommand::Log { job } => (Request::JobLog { job }, None),
        JobCommand::Approve { job, note } => (
            Request::JobDecide {
                job,
                approve: true,
                note,
            },
            None,
        ),
        JobCommand::Reject { job, note } => (
            Request::JobDecide {
                job,
                approve: false,
                note,
            },
            None,
        ),
    };
    let Some(plan) = ask(&socket, request).await? else {
        return Ok(ExitCode::SUCCESS);
    };
    launch(state, home, &plan, prompt).await
}

/// Runs the plan's setup steps, launches the harness, registers it, and waits for it.
async fn launch(
    state: &StateDir,
    home: &Path,
    plan: &RunPlan,
    prompt: Option<String>,
) -> Result<ExitCode> {
    let tree = materialize(state, &plan.root)?;
    let socket = state.socket();
    let env = environment();
    let resume = plan.session.clone();
    let harness_env = HarnessEnv::new(
        home,
        path_var(),
        HarnessConfig::load(&state.harness_config())?,
    )?;
    let first = prompt.or_else(|| resume.is_none().then(|| plan.goal.clone()));
    let (mut running, interactive) = match &plan.harness {
        HarnessKind::ClaudeCode => {
            let claude = ClaudeCode::new(harness_env.home.clone(), harness_env.path_var.clone());
            let trust = claude.preseed_trust(&plan.root, &env).await?;
            report_trust(&plan.root, &trust)?;
            let launch = Launch::Interactive {
                resume,
                prompt: first,
            };
            (claude.launch(&plan.root, launch, &env).await?, true)
        }
        HarnessKind::Codex => {
            let codex = Codex::new(harness_env.home.clone(), harness_env.path_var.clone());
            let launch = Launch::Interactive {
                resume,
                prompt: first,
            };
            (codex.launch(&plan.root, launch, &env).await?, true)
        }
        HarnessKind::Acp { program, args } => {
            let agent = acp_adapter(program, args, &harness_env)?;
            let launch = Launch::Headless {
                resume,
                prompt: first.unwrap_or_else(|| "Continue.".to_owned()),
            };
            (agent.launch(&plan.root, launch, &env).await?, false)
        }
    };
    let pid = running.pid();
    let registered = control::call(
        &socket,
        &Request::HarnessStarted {
            job: plan.job,
            epoch: plan.epoch,
            pid,
            harness: plan.harness.clone(),
            session: running.session().cloned(),
        },
    )
    .await;
    drop(tree);
    match registered {
        Ok(Response::Done { lines }) => print_lines(&lines)?,
        Ok(Response::Failed { error, refusal, .. }) => {
            kill_group(&running);
            return Err(Refused {
                refusal,
                message: format!(
                    "the node refused the launch, so the harness was stopped: {error}"
                ),
            }
            .into());
        }
        Ok(Response::Run { .. }) => {
            kill_group(&running);
            bail!("the node answered a registration with a run plan");
        }
        Err(e) => {
            kill_group(&running);
            bail!("the node could not record the launch, so the harness was stopped: {e}");
        }
    }
    if interactive {
        give_terminal(&running);
    }
    let status = running.child_mut().wait().await?;
    exited(&socket, plan, pid, status).await
}

/// Tells the node the harness exited and picks the exit code: the harness's
/// own, or 128 plus the signal that stopped it, with a line saying why.
async fn exited(
    socket: &Path,
    plan: &RunPlan,
    pid: std::num::NonZeroU32,
    status: std::process::ExitStatus,
) -> Result<ExitCode> {
    let told = control::call(
        socket,
        &Request::HarnessExited {
            job: plan.job,
            pid: pid.get(),
        },
    )
    .await;
    let missed = match told {
        Ok(Response::Done { .. }) => None,
        Ok(Response::Failed { error, .. }) => Some(error),
        Ok(Response::Run { .. }) => Some("it answered with a run plan".to_owned()),
        Err(e) => Some(e.to_string()),
    };
    if let Some(reason) = missed {
        writeln!(
            std::io::stderr(),
            "warning: the node did not record that the harness exited ({reason}); `fabric job status {}` shows what it holds",
            plan.job
        )?;
    }
    if let Some(signal) = status.signal() {
        writeln!(
            std::io::stderr(),
            "the harness was stopped by signal {signal}; the node stops it when the job is suspended, moves away or is stolen. Run `fabric job status {}` to see why",
            plan.job
        )?;
        return Ok(ExitCode::from(
            u8::try_from(128 + signal).unwrap_or(u8::MAX),
        ));
    }
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}

/// Runs the setup steps a restored workspace needs on this node (ADR 0007),
/// printing each, and returns the tree lock it took.
///
/// The caller holds the lock until the node has answered the harness's
/// registration, so a second `run` or `resume` of the same workspace
/// refuses with the lock's path from the setup steps until the first
/// harness is recorded, and the node's own check refuses it after that.
fn materialize(state: &StateDir, root: &Path) -> Result<TreeLock> {
    let lock = TreeLock::acquire(&state.tree_locks(), root)?;
    let detected = fabric_env::detect::detect(root, &[])?;
    let plan = fabric_env::materialize::plan(&detected, &lock, &NodeEnv::current())?;
    if plan.is_noop() {
        return Ok(lock);
    }
    let mut stdout = std::io::stdout().lock();
    for cache in plan.caches() {
        writeln!(
            stdout,
            "cache {}: {}",
            cache.path,
            health_words(&cache.health)
        )?;
    }
    for step in plan.steps() {
        let step = step.step();
        let args: Vec<&str> = step.args.iter().map(SetupArg::as_str).collect();
        writeln!(stdout, "setup: {} {}", step.program, args.join(" "))?;
    }
    drop(stdout);
    plan.ready()?.execute(&lock)?;
    Ok(lock)
}

/// A cache's state before the setup steps run, in words.
fn health_words(health: &CacheHealth) -> &'static str {
    match health {
        CacheHealth::Healthy => "present and complete",
        CacheHealth::Missing => "missing",
        CacheHealth::NotADirectory => "not a directory",
        CacheHealth::Empty => "empty",
        CacheHealth::Incomplete => "left unfinished by an earlier install",
    }
}

/// Says where Claude's trust in `root` came from: nothing when its key
/// was already set, the key when Fabric wrote it, and a one-line warning
/// when Fabric left it alone, because then Claude either asks in its own
/// dialog or skips it and ignores the workspace's project settings.
fn report_trust(root: &Path, outcome: &TrustOutcome) -> Result<()> {
    match outcome {
        TrustOutcome::AlreadyTrusted { .. } => {}
        TrustOutcome::Written { key } => {
            writeln!(std::io::stdout(), "trusted {}", key.display())?;
        }
        TrustOutcome::Inherited { by, reason } => writeln!(
            std::io::stderr(),
            "warning: Fabric did not trust {} ({}); Claude trusts it through {}, so it skips its trust dialog and ignores the workspace's project settings",
            root.display(),
            left_reason(reason),
            by.display()
        )?,
        TrustOutcome::LeftToClaude { reason } => writeln!(
            std::io::stderr(),
            "warning: Fabric did not trust {} ({}); Claude asks in its own trust dialog",
            root.display(),
            left_reason(reason)
        )?,
    }
    Ok(())
}

fn left_reason(reason: &LeftReason) -> String {
    match reason {
        LeftReason::ClaudeRunning { pids } => {
            let pids: Vec<String> = pids.iter().map(|pid| pid.to_string()).collect();
            format!("claude runs as pid {}", pids.join(", "))
        }
        LeftReason::NoConfig { path } => format!("{} does not exist yet", path.display()),
        LeftReason::ConfigIsSymlink { path } => format!("{} is a symlink", path.display()),
    }
}

/// The user's environment as the harness gets it: every variable with a valid name and a UTF-8 value.
fn environment() -> Vec<(EnvVarName, String)> {
    std::env::vars_os()
        .filter_map(|(name, value)| {
            let name = EnvVarName::parse(name.to_str()?).ok()?;
            Some((name, value.into_string().ok()?))
        })
        .collect()
}

/// Makes the harness's process group the terminal's foreground group, so it can read the keyboard.
///
/// The harness starts in a group of its own, so the node can kill exactly
/// it on fencing; without this, its first read of the terminal would stop
/// it. The shell takes the terminal back when this command exits.
fn give_terminal(running: &Running) {
    let stdin = std::io::stdin();
    if !rustix::termios::isatty(&stdin) {
        return;
    }
    let Some(group) = pid_of(running) else {
        return;
    };
    if let Err(e) = rustix::termios::tcsetpgrp(&stdin, group) {
        tracing::warn!(error = %e, "the harness could not take the terminal");
        return;
    }
    // A read before the hand-over stopped it; let it go on.
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::CONT);
}

fn kill_group(running: &Running) {
    if let Some(group) = pid_of(running) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

fn pid_of(running: &Running) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(i32::try_from(running.pid().get()).ok()?)
}
