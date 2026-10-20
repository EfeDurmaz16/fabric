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
//! waits for the harness to exit. An interactive harness launched from a
//! terminal is held before its exec until the daemon has recorded it and
//! its group owns the terminal (`terminal`).
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
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use fabric_core::env::{EnvVarName, SetupArg};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_env::materialize::{CacheHealth, MaterializeError, NodeEnv, TreeLock};
use fabric_harness::claude::{ClaudeCode, LeftReason, TrustOutcome};
use fabric_harness::codex::Codex;
use fabric_harness::{Harness, Launch, Running, SessionId, TurnReport};
use fabric_net::endpoint::DialOrder;
use fabricd::control::{
    self, ControlError, Decision, NO_DAEMON_EXIT, NamedSession, Refusal, Request, Response, RunPlan,
};
use fabricd::harnesses::{HarnessConfig, HarnessEnv, acp_adapter, parse_harness};
use fabricd::node::{Node, NodeConfig};
use fabricd::paths::{STATE_DIR_ENV, StateDir};
use tokio::io::{AsyncBufReadExt, BufReader};

mod terminal;

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
        /// Run one turn without a terminal, print the answer and the session, and exit.
        #[arg(long, requires = "prompt")]
        headless: bool,
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
        /// Run one turn without a terminal, print the answer and the session, and exit.
        #[arg(long, requires = "prompt")]
        headless: bool,
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
    let (request, mode) = match command {
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
                Mode::Interactive(None),
            )
        }
        JobCommand::Run {
            job,
            prompt,
            headless,
        } => (Request::JobRun { job }, Mode::new(prompt, headless)?),
        JobCommand::Suspend { job } => (
            Request::JobSuspend {
                job,
                path_var: path_var(),
            },
            Mode::Interactive(None),
        ),
        JobCommand::Resume {
            job,
            on,
            harness,
            prompt,
            headless,
            force,
        } => (
            Request::JobResume {
                job,
                on,
                harness,
                force,
                path_var: path_var(),
            },
            Mode::new(prompt, headless)?,
        ),
        JobCommand::Move { job, to, force } => (
            Request::JobMove {
                job,
                to,
                force,
                path_var: path_var(),
            },
            Mode::Interactive(None),
        ),
        JobCommand::Steal { job, force, reason } => (
            Request::JobSteal { job, reason, force },
            Mode::Interactive(None),
        ),
        JobCommand::Status { job } => (Request::JobStatus { job }, Mode::Interactive(None)),
        JobCommand::Ls => (Request::JobList, Mode::Interactive(None)),
        JobCommand::Log { job } => (Request::JobLog { job }, Mode::Interactive(None)),
        JobCommand::Approve { job, note } => (
            Request::JobDecide {
                job,
                decision: Decision::Approve,
                note,
            },
            Mode::Interactive(None),
        ),
        JobCommand::Reject { job, note } => (
            Request::JobDecide {
                job,
                decision: Decision::Reject,
                note,
            },
            Mode::Interactive(None),
        ),
    };
    let Some(plan) = ask(&socket, request).await? else {
        return Ok(ExitCode::SUCCESS);
    };
    launch(state, home, &plan, mode).await
}

/// How the harness runs: attached to this terminal, or for one turn without one.
#[derive(Debug)]
enum Mode {
    /// In this terminal, with the first message when there is one.
    Interactive(Option<String>),
    /// One turn without a terminal, started by this message (`--headless`).
    Headless(String),
}

impl Mode {
    fn new(prompt: Option<String>, headless: bool) -> Result<Self> {
        match (headless, prompt) {
            (false, prompt) => Ok(Mode::Interactive(prompt)),
            (true, Some(prompt)) => Ok(Mode::Headless(prompt)),
            (true, None) => bail!("--headless needs --prompt"),
        }
    }
}

/// How long the output of a headless harness that exited is still read,
/// for a process it started that holds its standard output open.
const DRAIN: Duration = Duration::from_secs(2);

/// Runs the plan's setup steps, launches the harness, registers it, and waits for it.
async fn launch(state: &StateDir, home: &Path, plan: &RunPlan, mode: Mode) -> Result<ExitCode> {
    let tree = materialize(state, &plan.root)?;
    let socket = state.socket();
    let headless = matches!(mode, Mode::Headless(_));
    let mut running = start(state, home, plan, mode).await?;
    let pid = running.pid();
    let registered = register(&socket, plan, &running).await;
    drop(tree);
    if let Err(e) = registered {
        kill_group(&running);
        return Err(e);
    }
    let read_line: Option<fn(&mut TurnReport, &str) -> bool> = match plan.harness {
        HarnessKind::ClaudeCode => Some(ClaudeCode::read_headless_line),
        HarnessKind::Codex => Some(Codex::read_headless_line),
        // The protocol client holds an ACP agent's output.
        HarnessKind::Acp { .. } => None,
    };
    if let (true, Some(read_line)) = (headless, read_line) {
        let (report, status) = headless_turn(&socket, plan, &mut running, read_line).await?;
        print_turn(plan, &report)?;
        return exited(&socket, plan, pid, status).await;
    }
    // A held harness has not run yet: it gets the terminal first, then starts.
    let handed = if running.is_held() {
        terminal::hand_over(pid)
    } else {
        None
    };
    if let Err(e) = running.release() {
        kill_group(&running);
        return Err(e.into());
    }
    let status = running.child_mut().wait().await?;
    drop(handed);
    if headless {
        print_turn(
            plan,
            &TurnReport {
                session: running.session().cloned(),
                ..TurnReport::default()
            },
        )?;
    }
    exited(&socket, plan, pid, status).await
}

/// Launches the plan's harness in `mode`; held when it is interactive and
/// standard input is a terminal.
async fn start(state: &StateDir, home: &Path, plan: &RunPlan, mode: Mode) -> Result<Running> {
    let env = environment();
    let resume = plan.session.clone();
    let harness_env = HarnessEnv::new(
        home,
        path_var(),
        HarnessConfig::load(&state.harness_config())?,
    )?;
    let launch = match mode {
        Mode::Headless(prompt) => Launch::Headless { resume, prompt },
        Mode::Interactive(prompt) => Launch::Interactive {
            prompt: prompt.or_else(|| resume.is_none().then(|| plan.goal.clone())),
            resume,
        },
    };
    Ok(match &plan.harness {
        HarnessKind::ClaudeCode => {
            let claude = ClaudeCode::new(harness_env.home.clone(), harness_env.path_var.clone());
            let trust = claude.preseed_trust(&plan.root, &env).await?;
            report_trust(&plan.root, &trust)?;
            claude.launch(&plan.root, launch, &env).await?
        }
        HarnessKind::Codex => {
            let codex = Codex::new(harness_env.home.clone(), harness_env.path_var.clone());
            codex.launch(&plan.root, launch, &env).await?
        }
        HarnessKind::Acp { program, args } => {
            let agent = acp_adapter(program, args, &harness_env)?;
            let launch = Launch::Headless {
                resume: launch.resume().cloned(),
                prompt: launch.prompt().unwrap_or("Continue.").to_owned(),
            };
            agent.launch(&plan.root, launch, &env).await?
        }
    })
}

/// Tells the node the harness started, so it records it and appends
/// `Resumed`, and prints what it answered. The caller kills the harness's
/// group on an error.
async fn register(socket: &Path, plan: &RunPlan, running: &Running) -> Result<()> {
    let registered = control::call(
        socket,
        &Request::HarnessStarted {
            job: plan.job,
            epoch: plan.epoch,
            pid: running.pid(),
            harness: plan.harness.clone(),
            session: running.session().cloned(),
        },
    )
    .await;
    match registered {
        Ok(Response::Done { lines }) => print_lines(&lines),
        Ok(Response::Failed {
            lines,
            error,
            refusal,
        }) => {
            print_lines(&lines)?;
            Err(Refused {
                refusal,
                message: format!(
                    "the node refused the launch, so the harness was stopped: {error}"
                ),
            }
            .into())
        }
        Ok(Response::Run { .. }) => bail!("the node answered a registration with a run plan"),
        Err(e) => bail!("the node could not record the launch, so the harness was stopped: {e}"),
    }
}

/// Reads a headless turn to its end: the harness's standard error goes to
/// this command's, its standard output is read line by line into a
/// report, and a session it names that the node does not hold yet is
/// recorded at once, so a suspend during the turn captures it.
async fn headless_turn(
    socket: &Path,
    plan: &RunPlan,
    running: &mut Running,
    read_line: fn(&mut TurnReport, &str) -> bool,
) -> Result<(TurnReport, std::process::ExitStatus)> {
    let pid = running.pid();
    let child = running.child_mut();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("the headless harness has no standard output"))?;
    if let Some(mut stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await;
        });
    }
    let mut report = TurnReport {
        session: running.session().cloned(),
        ..TurnReport::default()
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut status = None;
    loop {
        let line = if status.is_some() {
            match tokio::time::timeout(DRAIN, lines.next_line()).await {
                Ok(line) => line?,
                Err(_) => break,
            }
        } else {
            tokio::select! {
                line = lines.next_line() => line?,
                exit = running.child_mut().wait() => {
                    status = Some(exit?);
                    continue;
                }
            }
        };
        let Some(line) = line else {
            break;
        };
        if read_line(&mut report, &line)
            && let Some(session) = report.session.clone()
        {
            record_session(socket, plan, pid, session).await?;
        }
    }
    let status = match status {
        Some(status) => status,
        None => running.child_mut().wait().await?,
    };
    Ok((report, status))
}

/// Asks the node to record `session` for the harness `pid`; a refusal is a
/// warning, because the turn runs on either way.
async fn record_session(
    socket: &Path,
    plan: &RunPlan,
    pid: std::num::NonZeroU32,
    session: SessionId,
) -> Result<()> {
    let request = Request::HarnessSession(NamedSession {
        job: plan.job,
        epoch: plan.epoch,
        pid,
        harness: plan.harness.clone(),
        session: session.clone(),
    });
    let missed = match control::call(socket, &request).await {
        Ok(Response::Done { lines }) => {
            print_lines(&lines)?;
            return Ok(());
        }
        Ok(Response::Failed { lines, error, .. }) => {
            print_lines(&lines)?;
            error
        }
        Ok(Response::Run { .. }) => "it answered with a run plan".to_owned(),
        Err(e) => e.to_string(),
    };
    writeln!(
        std::io::stderr(),
        "warning: the node did not record {} session {session} ({missed}); a suspend of job {} does not carry it",
        plan.harness,
        plan.job
    )?;
    Ok(())
}

/// Prints what a headless turn reported: the answer on standard output,
/// then a line naming the session; a failure, a missing answer and a
/// missing session on standard error.
fn print_turn(plan: &RunPlan, report: &TurnReport) -> Result<()> {
    let harness = &plan.harness;
    if let Some(failure) = &report.failure {
        writeln!(std::io::stderr(), "the turn failed: {failure}")?;
    }
    if let Some(answer) = &report.answer {
        writeln!(std::io::stdout(), "{answer}")?;
    } else if report.failure.is_none() && !matches!(harness, HarnessKind::Acp { .. }) {
        writeln!(std::io::stderr(), "warning: {harness} printed no answer")?;
    }
    match (&report.session, &plan.session) {
        (Some(session), Some(planned)) if session != planned => writeln!(
            std::io::stderr(),
            "warning: {harness} continued in session {session}, not in {planned}"
        )?,
        (None, _) => writeln!(std::io::stderr(), "warning: {harness} named no session")?,
        _ => {}
    }
    if let Some(session) = &report.session {
        writeln!(std::io::stdout(), "{harness} session {session}")?;
    }
    Ok(())
}

/// Tells the node the harness exited and picks the exit code: the harness's
/// own, or 128 plus the signal that stopped it, with a line saying why.
async fn exited(
    socket: &Path,
    plan: &RunPlan,
    pid: std::num::NonZeroU32,
    status: std::process::ExitStatus,
) -> Result<ExitCode> {
    let told = control::call(socket, &Request::HarnessExited { job: plan.job, pid }).await;
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
    let lock = TreeLock::acquire(&state.tree_locks(), root).map_err(|e| match e {
        // Another run or resume of this workspace is starting its harness:
        // the same state as a harness that runs, so the same exit code.
        MaterializeError::TreeBusy { .. } => anyhow::Error::new(Refused {
            refusal: Refusal::JobState,
            message: e.to_string(),
        }),
        e => e.into(),
    })?;
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

fn kill_group(running: &Running) {
    if let Some(group) = pid_of(running) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

fn pid_of(running: &Running) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(i32::try_from(running.pid().get()).ok()?)
}
