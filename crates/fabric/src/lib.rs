//! The `fabric` command, as a library so its tests can build it with the
//! node's fault points; `main.rs` only calls [`main`].
//!
//! Every command but `serve` is a client of the node daemon on this
//! machine: it sends one request over the control socket in the state
//! directory through `fabric-client` (ADR 0018) and prints the node's
//! typed events and reply as lines (`render`) as they arrive. The one thing
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
use fabric_client::api::{
    ClientEnv, Decision, DoctorReport, Event, RefusalKind, Reply, Request, RunPlan, SystemPath,
};
use fabric_client::{Client, ClientError, Launched};
use fabric_core::env::{EnvVarName, SetupArg};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_core::session::SessionId;
use fabric_env::materialize::{CacheHealth, MaterializeError, NodeEnv, TreeLock};
use fabric_harness::claude::{ClaudeCode, LeftReason, TrustOutcome};
use fabric_harness::codex::Codex;
use fabric_harness::{Harness, Launch, Running, TurnReport};
use fabric_net::endpoint::DialOrder;
use fabricd::harnesses::{HarnessEnv, acp_adapter, parse_harness};
use fabricd::node::{Node, NodeConfig};
use fabricd::paths::{STATE_DIR_ENV, StateDir};
use fabricd::tool_env::ToolEnv;
use tokio::io::{AsyncBufReadExt, BufReader};

mod render;
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

/// The client of the node that serves `state`.
fn client(state: &StateDir) -> Client {
    Client::new(state.socket()).named(format!("fabric/{}", env!("CARGO_PKG_VERSION")))
}

/// Where `fabric` prints its own lines: standard output, or standard
/// error for a headless run, whose standard output carries only the
/// harness's answer and the line naming the session, for a script to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notes {
    Stdout,
    Stderr,
}

impl Notes {
    fn print<T: fmt::Display>(self, lines: &[T]) -> Result<()> {
        match self {
            Notes::Stdout => {
                let mut out = std::io::stdout().lock();
                lines.iter().try_for_each(|line| writeln!(out, "{line}"))?;
            }
            Notes::Stderr => {
                let mut out = std::io::stderr().lock();
                lines.iter().try_for_each(|line| writeln!(out, "{line}"))?;
            }
        }
        Ok(())
    }
}

/// A refusal, kept typed so `main` can pick the exit code.
#[derive(Debug)]
struct Refused {
    refusal: RefusalKind,
    message: String,
}

impl Refused {
    /// The refusal the node answered with its sentence; any other client error as it is.
    fn from_client(error: ClientError) -> anyhow::Error {
        match error {
            ClientError::Refused(refusal) => Refused {
                refusal: refusal.kind(),
                message: refusal.message().to_owned(),
            }
            .into(),
            other => other.into(),
        }
    }
}

/// The exit code of a command that found no daemon to answer it.
const NO_DAEMON_EXIT: u8 = 80;

/// The exit code for a refusal of `kind` (ADR 0016, Exit codes).
fn refusal_exit(kind: RefusalKind) -> u8 {
    match kind {
        RefusalKind::NotOwner => 81,
        RefusalKind::Frozen => 82,
        RefusalKind::Unreachable => 83,
        RefusalKind::NeedsForce => 84,
        RefusalKind::JobState => 85,
        RefusalKind::Failed => 1,
    }
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
        return ExitCode::from(refusal_exit(refused.refusal));
    }
    if let Some(ClientError::NoDaemon { .. }) = error.downcast_ref::<ClientError>() {
        return ExitCode::from(NO_DAEMON_EXIT);
    }
    ExitCode::FAILURE
}

/// Prints each event of a call as it arrives, keeping the first failure to print.
struct Printer {
    notes: Notes,
    failed: Option<anyhow::Error>,
}

impl Printer {
    fn new(notes: Notes) -> Self {
        Self {
            notes,
            failed: None,
        }
    }

    fn print(&mut self, event: &Event) {
        if let Err(e) = self.notes.print(&render::event(event)) {
            self.failed.get_or_insert(e);
        }
    }

    /// The first failure to print, if any.
    fn finish(self) -> Result<()> {
        self.failed.map_or(Ok(()), Err)
    }
}

/// Sends a request, printing each event as it arrives and then the reply;
/// a refusal becomes an error.
async fn ask(client: &Client, request: &Request, notes: Notes) -> Result<Reply> {
    let mut printer = Printer::new(notes);
    let answered = client.call(request, |event| printer.print(&event)).await;
    printer.finish()?;
    let reply = answered.map_err(Refused::from_client)?;
    notes.print(&render::reply(&reply))?;
    Ok(reply)
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
    let client = client(&state);
    match cli.command {
        Command::Serve(args) => serve(state, home, args).await,
        Command::Node(command) => {
            let request = match command {
                NodeCommand::Id => Request::NodeId,
                NodeCommand::Add { node, addr } => Request::NodeAdd { node, addr },
                NodeCommand::Ls => Request::NodeList,
                NodeCommand::Ping { node } => Request::NodePing { node },
            };
            ask(&client, &request, Notes::Stdout).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { peer, job } => {
            let request = Request::Doctor {
                peer,
                job,
                env: Some(ClientEnv::current()?),
            };
            let reply = ask(&client, &request, Notes::Stdout).await?;
            // The node reports; a row that would stop a move still fails
            // the command, as a preflight does.
            if let Reply::Doctor {
                report: DoctorReport::Compared { rows, .. },
            } = &reply
            {
                let blocking = render::blocking(rows);
                if !blocking.is_empty() {
                    return Err(Refused {
                        refusal: RefusalKind::NeedsForce,
                        message: format!(
                            "the preflight refused the move: {}; `fabric job move` and `fabric job resume` take --force to waive what can be waived",
                            blocking.join(", ")
                        ),
                    }
                    .into());
                }
            }
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
        env: ToolEnv::current(),
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
    let client = client(state);
    let (request, mode) = match command {
        JobCommand::New {
            repo,
            goal,
            harness,
        } => {
            let canonical = std::fs::canonicalize(&repo)
                .with_context(|| format!("repository {}", repo.display()))?;
            let repo = canonical
                .to_str()
                .and_then(|text| SystemPath::parse(text).ok())
                .ok_or_else(|| {
                    anyhow!(
                        "repository {} is not a UTF-8 path, which the client API cannot carry",
                        canonical.display()
                    )
                })?;
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
                env: Some(ClientEnv::current()?),
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
                env: Some(ClientEnv::current()?),
            },
            Mode::new(prompt, headless)?,
        ),
        JobCommand::Move { job, to, force } => (
            Request::JobMove {
                job,
                to,
                force,
                env: Some(ClientEnv::current()?),
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
    let (Reply::JobRun(plan) | Reply::JobResume(plan)) =
        ask(&client, &request, mode.notes()).await?
    else {
        return Ok(ExitCode::SUCCESS);
    };
    launch(state, home, &client, &plan, mode).await
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

    /// Where `fabric`'s own lines go in this mode.
    fn notes(&self) -> Notes {
        match self {
            Mode::Interactive(_) => Notes::Stdout,
            Mode::Headless(_) => Notes::Stderr,
        }
    }
}

/// How long the output of a headless harness that exited is still read,
/// in all, for a process it started that holds its standard output open.
const DRAIN: Duration = Duration::from_secs(2);

/// Reads one line a headless harness printed into a report; true when it named a new session.
type ReadLine = fn(&mut TurnReport, &str) -> bool;

/// Runs the plan's setup steps, launches the harness, registers it, and waits for it.
async fn launch(
    state: &StateDir,
    home: &Path,
    client: &Client,
    plan: &RunPlan,
    mode: Mode,
) -> Result<ExitCode> {
    let notes = mode.notes();
    let read_line: Option<ReadLine> = match (&mode, &plan.harness) {
        (Mode::Headless(_), HarnessKind::ClaudeCode) => Some(ClaudeCode::read_headless_line),
        (Mode::Headless(_), HarnessKind::Codex) => Some(Codex::read_headless_line),
        // The protocol client holds an ACP agent's output.
        _ => None,
    };
    let tree = materialize(state, &plan.root, notes)?;
    let headless = matches!(mode, Mode::Headless(_));
    // Caught from just before the harness starts, so none is lost in
    // between; a signal during the setup steps still ends the command.
    let forward = read_line.map(|_| Forward::new()).transpose()?;
    let mut running = start(home, plan, mode, notes).await?;
    let pid = running.pid();
    let registered = register(client, plan, &running, notes).await;
    drop(tree);
    if let Err(e) = registered {
        kill_group(&running);
        return Err(e);
    }
    if let (Some(read_line), Some(forward)) = (read_line, forward) {
        let turn = Turn {
            client,
            plan,
            notes,
            read_line,
        };
        let (report, status, mut forward) = turn.run(&mut running, forward).await?;
        // The node hears of the exit even when the answer cannot be printed,
        // as after the hang-up of the terminal that would have shown it.
        let printed = print_turn(plan, &report);
        let code = tokio::select! {
            code = exited(client, plan, pid, status, forward.passed > 0) => code?,
            signal = forward.next() => {
                writeln!(
                    std::io::stderr(),
                    "interrupted while the node recorded the end of the turn; `fabric job status {}` shows what it holds",
                    plan.job
                )?;
                return Ok(signal_exit(signal.as_raw()));
            }
        };
        printed?;
        return Ok(code);
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
    let printed = if headless {
        print_turn(
            plan,
            &TurnReport {
                session: running.session().cloned(),
                ..TurnReport::default()
            },
        )
    } else {
        Ok(())
    };
    let code = exited(client, plan, pid, status, false).await?;
    printed?;
    Ok(code)
}

/// Launches the plan's harness in `mode`; held when it is interactive and
/// standard input is a terminal.
async fn start(home: &Path, plan: &RunPlan, mode: Mode, notes: Notes) -> Result<Running> {
    let env = environment();
    let resume = plan.session.clone();
    let harness_env = HarnessEnv::new(home, path_var(), plan.harness_config.clone().into())?;
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
            report_trust(&plan.root, &trust, notes)?;
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
async fn register(client: &Client, plan: &RunPlan, running: &Running, notes: Notes) -> Result<()> {
    let mut printer = Printer::new(notes);
    let registered = client
        .harness_started(
            launched(plan, running.pid()),
            running.session().cloned(),
            |event| printer.print(&event),
        )
        .await;
    printer.finish()?;
    match registered {
        Ok(registered) => notes.print(&[render::registered(&registered)]),
        Err(ClientError::Refused(refusal)) => Err(Refused {
            refusal: refusal.kind(),
            message: format!(
                "the node refused the launch, so the harness was stopped: {}",
                refusal.message()
            ),
        }
        .into()),
        Err(e) => bail!("the node could not record the launch, so the harness was stopped: {e}"),
    }
}

/// The harness `pid` launched from `plan`, as the node registers it.
fn launched(plan: &RunPlan, pid: std::num::NonZeroU32) -> Launched {
    Launched {
        job: plan.job,
        epoch: plan.epoch,
        pid,
        harness: plan.harness.clone(),
    }
}

/// The signals a headless run passes on to its harness, which runs in a
/// process group of its own and would not get them: Ctrl-C, the hang-up
/// of a closed terminal or SSH session, and a termination of `fabric`.
/// Without this, `fabric` would die of them and leave the turn running
/// with nobody to read its answer or record its end.
struct Forward {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
    /// How many signals were passed on.
    passed: u32,
}

impl Forward {
    fn new() -> Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
            passed: 0,
        })
    }

    /// Waits for the next of them and returns it, to send on.
    async fn next(&mut self) -> rustix::process::Signal {
        use rustix::process::Signal;
        tokio::select! {
            Some(()) = self.interrupt.recv() => Signal::INT,
            Some(()) = self.terminate.recv() => Signal::TERM,
            Some(()) = self.hangup.recv() => Signal::HUP,
            else => std::future::pending().await,
        }
    }

    /// Passes `signal` on to the harness's group `pgid`; from the second
    /// signal on, kills the group, so a harness that ignores the first
    /// cannot keep the command from ending.
    fn pass_on(&mut self, pgid: std::num::NonZeroU32, signal: rustix::process::Signal) {
        self.passed += 1;
        if self.passed > 1 {
            send_to_group(pgid, rustix::process::Signal::KILL);
        } else {
            send_to_group(pgid, signal);
        }
    }
}

/// The exit code of a command ended by the signal `number`.
fn signal_exit(number: i32) -> ExitCode {
    ExitCode::from(u8::try_from(128 + number).unwrap_or(u8::MAX))
}

/// A headless turn being read: where to tell the node about a session,
/// and how to read the harness's output.
struct Turn<'a> {
    client: &'a Client,
    plan: &'a RunPlan,
    notes: Notes,
    read_line: ReadLine,
}

impl Turn<'_> {
    /// Reads the turn to its end. The harness's standard error goes to this
    /// command's; its standard output is read line by line into a report.
    /// A session it names that the node does not hold yet goes to a
    /// recorder task at once, so a suspend during the turn captures it,
    /// while this loop keeps reaping the harness, which a suspend waits
    /// for. A signal `fabric` gets is passed on to the harness's group
    /// until the harness is reaped, and one that comes while the node
    /// records the last session gives up waiting for it. Returns the
    /// signals, which the caller keeps watching while it tells the node
    /// the harness exited.
    async fn run(
        &self,
        running: &mut Running,
        mut forward: Forward,
    ) -> Result<(TurnReport, std::process::ExitStatus, Forward)> {
        let pid = running.pid();
        let child = running.child_mut();
        let Some(stdout) = child.stdout.take() else {
            kill_group(running);
            bail!("the headless harness has no standard output, so it was stopped");
        };
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await;
            });
        }
        let (sessions, named) = tokio::sync::mpsc::unbounded_channel();
        let recorder = tokio::spawn(record_sessions(
            self.client.clone(),
            self.plan.clone(),
            pid,
            named,
            self.notes,
        ));
        let mut report = TurnReport {
            session: running.session().cloned(),
            ..TurnReport::default()
        };
        let mut lines = BufReader::new(stdout).split(b'\n');
        let mut status = None;
        let mut drained_by = None;
        loop {
            let read = if let Some(deadline) = drained_by {
                match tokio::time::timeout_at(deadline, lines.next_segment()).await {
                    Ok(read) => read,
                    Err(_) => break,
                }
            } else {
                tokio::select! {
                    read = lines.next_segment() => read,
                    exit = running.child_mut().wait() => {
                        status = Some(exit?);
                        drained_by = Some(tokio::time::Instant::now() + DRAIN);
                        continue;
                    }
                    signal = forward.next() => {
                        forward.pass_on(pid, signal);
                        continue;
                    }
                }
            };
            let line = match read {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "the harness's output could not be read to its end");
                    break;
                }
            };
            if (self.read_line)(&mut report, &String::from_utf8_lossy(&line))
                && let Some(session) = report.session.clone()
            {
                // The receiver lives until this loop ends.
                let _ = sessions.send(session);
            }
        }
        drop(sessions);
        // The harness closed its output and may still run.
        let status = match status {
            Some(status) => status,
            None => loop {
                tokio::select! {
                    exit = running.child_mut().wait() => break exit?,
                    signal = forward.next() => forward.pass_on(pid, signal),
                }
            },
        };
        let mut recorder = recorder;
        tokio::select! {
            ended = &mut recorder => {
                if let Err(e) = ended {
                    tracing::warn!(error = %e, "the session recorder ended early");
                }
            }
            _ = forward.next() => {
                recorder.abort();
                writeln!(
                    std::io::stderr(),
                    "warning: interrupted while the node recorded the session; `fabric job status {}` shows what it holds",
                    self.plan.job
                )?;
            }
        }
        Ok((report, status, forward))
    }
}

/// Sends `signal` to the process group `pgid`, which is gone if it has exited.
fn send_to_group(pgid: std::num::NonZeroU32, signal: rustix::process::Signal) {
    let group = i32::try_from(pgid.get())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group
        && let Err(e) = rustix::process::kill_process_group(group, signal)
        && e != rustix::io::Errno::SRCH
    {
        tracing::warn!(error = %e, ?signal, "the signal could not be passed on to the harness");
    }
}

/// Asks the node to record each session the harness names, in order, for
/// the harness `pid`. A refusal is a warning, because the turn runs on.
async fn record_sessions(
    client: Client,
    plan: RunPlan,
    pid: std::num::NonZeroU32,
    mut named: tokio::sync::mpsc::UnboundedReceiver<SessionId>,
    notes: Notes,
) {
    while let Some(session) = named.recv().await {
        let mut printer = Printer::new(notes);
        let recorded = client
            .harness_session(launched(&plan, pid), session.clone(), |event| {
                printer.print(&event);
            })
            .await;
        let missed = match recorded {
            Ok(recorded) => {
                let _ = notes.print(&[render::session_recorded(&recorded)]);
                continue;
            }
            Err(ClientError::Refused(refusal)) => refusal.message().to_owned(),
            Err(e) => e.to_string(),
        };
        let _ = writeln!(
            std::io::stderr(),
            "warning: the node did not record {} session {session} ({missed}); a suspend of job {} does not carry it",
            plan.harness,
            plan.job
        );
    }
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
/// own, or 128 plus the signal that stopped it, with a line saying why;
/// `passed_on` says fabric passed a signal of its own on to the harness.
async fn exited(
    client: &Client,
    plan: &RunPlan,
    pid: std::num::NonZeroU32,
    status: std::process::ExitStatus,
    passed_on: bool,
) -> Result<ExitCode> {
    let told = client.harness_exited(plan.job, pid, |_| {}).await;
    let missed = match told {
        Ok(()) => None,
        Err(ClientError::Refused(refusal)) => Some(refusal.message().to_owned()),
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
        if passed_on {
            writeln!(
                std::io::stderr(),
                "the harness ended on signal {signal}, after fabric passed a signal on to it"
            )?;
        } else {
            writeln!(
                std::io::stderr(),
                "the harness was stopped by signal {signal}; the node stops it when the job is suspended, moves away or is stolen. Run `fabric job status {}` to see why",
                plan.job
            )?;
        }
        return Ok(signal_exit(signal));
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
fn materialize(state: &StateDir, root: &Path, notes: Notes) -> Result<TreeLock> {
    let lock = TreeLock::acquire(&state.tree_locks(), root).map_err(|e| match e {
        // Another run or resume of this workspace is starting its harness:
        // the same state as a harness that runs, so the same exit code.
        MaterializeError::TreeBusy { .. } => anyhow::Error::new(Refused {
            refusal: RefusalKind::JobState,
            message: e.to_string(),
        }),
        e => e.into(),
    })?;
    let detected = fabric_env::detect::detect(root, &[])?;
    let plan = fabric_env::materialize::plan(&detected, &lock, &NodeEnv::current())?;
    if plan.is_noop() {
        // A declared variable is required even when no step runs, so a
        // warm node refuses the launch a cold node would (ADR 0007).
        plan.ready()?;
        return Ok(lock);
    }
    let mut lines = Vec::new();
    for cache in plan.caches() {
        lines.push(format!(
            "cache {}: {}",
            cache.path,
            health_words(&cache.health)
        ));
    }
    for step in plan.steps() {
        let step = step.step();
        let args: Vec<&str> = step.args.iter().map(SetupArg::as_str).collect();
        lines.push(format!("setup: {} {}", step.program, args.join(" ")));
    }
    notes.print(&lines)?;
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
fn report_trust(root: &Path, outcome: &TrustOutcome, notes: Notes) -> Result<()> {
    match outcome {
        TrustOutcome::AlreadyTrusted { .. } => {}
        TrustOutcome::Written { key } => {
            notes.print(&[format!("trusted {}", key.display())])?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// With every cache whole no step runs, and the declared variable must
    /// still stop the launch.
    #[test]
    fn a_missing_declared_variable_stops_a_launch_with_no_setup_step() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "env = [\"FABRIC_TEST_VARIABLE_NO_ONE_SETS\"]\n",
        )
        .unwrap();
        let state =
            StateDir::resolve(Some(dir.path().join("state").as_os_str()), dir.path()).unwrap();
        let Err(error) = materialize(&state, &root, Notes::Stdout) else {
            panic!("the launch went ahead");
        };
        let error = error.to_string();
        assert!(
            error.contains("FABRIC_TEST_VARIABLE_NO_ONE_SETS"),
            "{error}"
        );
    }
}
