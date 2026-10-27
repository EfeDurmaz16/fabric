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
//! waits for the harness to exit. Every harness is held before its exec
//! until the daemon has recorded it, so a CLI killed before then, even by
//! `SIGKILL`, leaves nothing of it running (issue #17), and an interactive
//! one launched from a terminal until its group owns the terminal too
//! (`terminal`).
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
use fabric_client::{Client, ClientError, Launched, Started};
use fabric_core::env::{EnvVarName, SetupArg};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{EnvSpecId, JobId, NodeId};
use fabric_core::session::SessionId;
use fabric_env::materialize::{CacheHealth, MaterializeError, NodeEnv, TreeLock};
use fabric_harness::acp::{TurnEvent, TurnFeed};
use fabric_harness::claude::{ClaudeCode, LeftReason, TrustOutcome};
use fabric_harness::codex::Codex;
use fabric_harness::{Harness, Launch, LaunchSettings, Origin, PassedSetting, Running, TurnReport};
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
    /// Ping a paired node over its session, dialing it when none is open.
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
        /// Run setup steps this node has not approved for the job, as the workspace now is, without asking.
        #[arg(long)]
        yes: bool,
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
        /// Run setup steps this node has not approved for the job, as the workspace now is, without asking.
        #[arg(long)]
        yes: bool,
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
            Notes::Stdout => write_lines(&mut std::io::stdout().lock(), lines)?,
            Notes::Stderr => write_lines(&mut std::io::stderr().lock(), lines)?,
        }
        Ok(())
    }
}

/// Writes each of `lines` on a line of its own, with its control
/// characters escaped (`fabric_client::sentences::printable`).
///
/// Every line `fabric` prints that holds text it did not make goes through
/// here: the lines of each event and reply, its notes and warnings, and
/// its error. A goal, a steal's reason, a peer's facts, a path from a
/// snapshot and an ACP harness's arguments are in the words of whoever
/// wrote them, a paired node or an agent in the workspace among them, and
/// an ANSI or OSC sequence there would otherwise act on the user's
/// terminal, and a line break would forge a line. Only a headless turn's
/// answer, which is the harness's own output, prints as it came.
fn write_lines<T: fmt::Display>(out: &mut impl Write, lines: &[T]) -> std::io::Result<()> {
    for line in lines {
        let line = line.to_string();
        writeln!(out, "{}", fabric_client::sentences::printable(&line))?;
    }
    Ok(())
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
    if let Reply::JobStatus(status) = &reply
        && let Some(error) = render::first_unreadable(status)
    {
        return Err(Refused {
            refusal: RefusalKind::Failed,
            message: error.to_owned(),
        }
        .into());
    }
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
            let _ = Notes::Stderr.print(&[format!("error: {e:#}")]);
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
            // The node reports; a row or a path that would stop a move
            // still fails the command, as a preflight does.
            if let Reply::Doctor {
                report: DoctorReport::Compared { rows, carry, .. },
            } = &reply
                && let Some(message) = render::refused_move(rows, carry.as_ref())
            {
                return Err(Refused {
                    refusal: RefusalKind::NeedsForce,
                    message,
                }
                .into());
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
    let (request, mode, yes) = job_request(command)?;
    let (Reply::JobRun(plan) | Reply::JobResume(plan)) =
        ask(&client, &request, mode.notes()).await?
    else {
        return Ok(ExitCode::SUCCESS);
    };
    let approval = Approval {
        record: state.job(plan.job).env_spec_ran(),
        yes,
        ask: &ask_on_terminal,
    };
    launch(state, home, &client, &plan, mode, &approval).await
}

/// The request a job command sends, how a harness it launches runs, and
/// whether `--yes` lets setup steps this node has not approved run without asking.
fn job_request(command: JobCommand) -> Result<(Request, Mode, bool)> {
    let env = || ClientEnv::current().map(Some);
    Ok(match command {
        JobCommand::New {
            repo,
            goal,
            harness,
        } => (
            Request::JobNew {
                repo: repository(&repo)?,
                goal,
                harness,
            },
            Mode::Interactive(None),
            false,
        ),
        JobCommand::Run {
            job,
            prompt,
            headless,
            yes,
        } => (
            Request::JobRun { job, env: env()? },
            Mode::new(prompt, headless)?,
            yes,
        ),
        JobCommand::Suspend { job } => (
            Request::JobSuspend { job, env: env()? },
            Mode::Interactive(None),
            false,
        ),
        JobCommand::Resume {
            job,
            on,
            harness,
            prompt,
            headless,
            force,
            yes,
        } => (
            Request::JobResume {
                job,
                on,
                harness,
                force,
                env: env()?,
            },
            Mode::new(prompt, headless)?,
            yes,
        ),
        JobCommand::Move { job, to, force } => (
            Request::JobMove {
                job,
                to,
                force,
                env: env()?,
            },
            Mode::Interactive(None),
            false,
        ),
        JobCommand::Steal { job, force, reason } => (
            Request::JobSteal { job, reason, force },
            Mode::Interactive(None),
            false,
        ),
        JobCommand::Status { job } => (Request::JobStatus { job }, Mode::Interactive(None), false),
        JobCommand::Ls => (Request::JobList, Mode::Interactive(None), false),
        JobCommand::Log { job } => (Request::JobLog { job }, Mode::Interactive(None), false),
        JobCommand::Approve { job, note } => (
            Request::JobDecide {
                job,
                decision: Decision::Approve,
                note,
            },
            Mode::Interactive(None),
            false,
        ),
        JobCommand::Reject { job, note } => (
            Request::JobDecide {
                job,
                decision: Decision::Reject,
                note,
            },
            Mode::Interactive(None),
            false,
        ),
    })
}

/// The repository `fabric job new` names, as the absolute path the client API carries.
fn repository(repo: &Path) -> Result<SystemPath> {
    let canonical =
        std::fs::canonicalize(repo).with_context(|| format!("repository {}", repo.display()))?;
    canonical
        .to_str()
        .and_then(|text| SystemPath::parse(text).ok())
        .ok_or_else(|| {
            anyhow!(
                "repository {} is not a UTF-8 path, which the client API cannot carry",
                canonical.display()
            )
        })
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
    approval: &Approval<'_>,
) -> Result<ExitCode> {
    let notes = mode.notes();
    let read_line: Option<ReadLine> = match (&mode, &plan.harness) {
        (Mode::Headless(_), HarnessKind::ClaudeCode) => Some(ClaudeCode::read_headless_line),
        (Mode::Headless(_), HarnessKind::Codex) => Some(Codex::read_headless_line),
        // The protocol client holds an ACP agent's output.
        _ => None,
    };
    let tree = prepare(state, home, client, plan, notes, approval).await?;
    let headless = matches!(mode, Mode::Headless(_));
    let env = environment();
    // Caught from before the harness starts: until the node records it, the
    // harness runs in a group of its own that nothing else supervises, so
    // a signal that ends this command must stop it first. A signal during
    // the setup steps still ends the command.
    let mut interrupts = Interrupts::new()?;
    let (adapter, how) = adapter_for(home, plan, mode, notes, &env).await?;
    // The resume guard runs inside the launch, before the harness exists,
    // and can wait as long as a `--version` run; a signal then ends the
    // command with nothing started. An adapter whose launch is dropped
    // before it returns kills what it started, and a signal already caught
    // wins over a launch that would finish in the same poll.
    let mut running = tokio::select! {
        biased;
        signal = interrupts.next() => return Ok(interrupted_before_start(signal)),
        running = adapter.launch(&plan.root, how, &env) => running?,
    };
    let pid = running.pid();
    let said = settings_lines(&plan.harness, plan.session.as_ref(), running.settings());
    let registered = match notes.print(&said) {
        Ok(()) => tokio::select! {
            registered = register(client, plan, &running, notes) => registered,
            signal = interrupts.next() => {
                kill_group(&running);
                return interrupted(signal, plan);
            }
        },
        Err(e) => Err(e),
    };
    drop(tree);
    if let Err(e) = registered {
        kill_group(&running);
        return Err(e);
    }
    // The node records the harness, which has not run yet: it gets the
    // terminal first when it is wired to it, then starts.
    let handed = if running.takes_terminal() {
        match terminal::hand_over(pid) {
            Ok(handed) => handed,
            Err(e) => {
                let error = anyhow!(
                    "the terminal could not be given to the harness ({e}), so it was stopped before it ran"
                );
                return Err(abandon(client, plan, &mut running, &mut interrupts, error).await);
            }
        }
    } else {
        None
    };
    if let Err(e) = running.release() {
        return Err(abandon(client, plan, &mut running, &mut interrupts, e.into()).await);
    }
    if let Some(feed) = running.take_turn() {
        let turn = AgentTurn {
            client,
            plan,
            notes,
            headless,
        };
        return turn.finish(&mut running, feed, &mut interrupts).await;
    }
    if let Some(read_line) = read_line {
        let turn = Turn {
            client,
            plan,
            notes,
            read_line,
        };
        let (report, status) = match turn.run(&mut running, &mut interrupts).await? {
            TurnEnd::Reaped { report, status } => (report, status),
            TurnEnd::Interrupted(signal) => return Ok(signal_exit(signal.as_raw())),
        };
        // The node hears of the exit before anything prints the turn, so a
        // script that reads the session line finds the harness no longer
        // recorded, and hears of it even when the answer cannot be printed,
        // as after the hang-up of the terminal that would have shown it.
        let code = exited(client, plan, pid, status, &mut interrupts).await?;
        print_turn(plan, &report)?;
        return Ok(code);
    }
    let status = if headless {
        // An agent run headless gets no signal of a terminal either, so
        // they are passed on to it until it is reaped and the node told.
        reap(&mut running, &mut interrupts).await?
    } else {
        tokio::select! {
            status = running.child_mut().wait() => status?,
            // The node supervises the harness now, so a signal ends this
            // command as it did before any was caught, and the harness runs on.
            signal = interrupts.next() => return Ok(signal_exit(signal.as_raw())),
        }
    };
    drop(handed);
    let code = exited(client, plan, pid, status, &mut interrupts).await?;
    if headless {
        print_turn(
            plan,
            &TurnReport {
                session: running.session().cloned(),
                ..TurnReport::default()
            },
        )?;
    }
    Ok(code)
}

/// Stops a recorded harness that cannot go on, reaps it and tells the node
/// it exited, so the node drops its record at once; returns `error`. A
/// signal stops the wait for the node, which finds the group gone at its
/// next look.
async fn abandon(
    client: &Client,
    plan: &RunPlan,
    running: &mut Running,
    interrupts: &mut Interrupts,
    error: anyhow::Error,
) -> anyhow::Error {
    kill_group(running);
    if let Err(e) = running.child_mut().wait().await {
        tracing::warn!(error = %e, "the stopped harness could not be reaped");
    }
    tokio::select! {
        told = client.harness_exited(plan.job, running.pid(), |_| {}) => {
            if let Err(e) = told {
                tracing::warn!(error = %e, "the node did not record that the stopped harness exited");
            }
        }
        _ = interrupts.next() => {}
    }
    error
}

/// The adapter for the plan's harness and how it launches it in `mode`.
async fn adapter_for(
    home: &Path,
    plan: &RunPlan,
    mode: Mode,
    notes: Notes,
    env: &[(EnvVarName, String)],
) -> Result<(Box<dyn Harness>, Launch)> {
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
            let trust = claude.preseed_trust(&plan.root, env).await?;
            report_trust(&plan.root, &trust, notes)?;
            (Box::new(claude), launch)
        }
        HarnessKind::Codex => {
            let codex = Codex::new(harness_env.home.clone(), harness_env.path_var.clone())
                .with_known_writer(plan.known_writer.clone());
            (Box::new(codex), launch)
        }
        HarnessKind::Acp { program, args } => {
            let agent = acp_adapter(program, args, &harness_env)?;
            let launch = Launch::Headless {
                resume: launch.resume().cloned(),
                prompt: launch.prompt().unwrap_or("Continue.").to_owned(),
            };
            (Box::new(agent), launch)
        }
    })
}

/// What the CLI says once the harness has started: the session it
/// continues and the settings the launch carried, and one line for each
/// setting the session records that the launch left out, with why, so a
/// job never runs on another model or sandbox without the user being told
/// (gate finding F10).
fn settings_lines(
    harness: &HarnessKind,
    session: Option<&SessionId>,
    settings: &LaunchSettings,
) -> Vec<String> {
    let mut lines = Vec::new();
    match (session, settings_words(&settings.passed)) {
        (Some(session), Some(with)) => {
            lines.push(format!(
                "continuing {harness} session {session} with {with}"
            ));
        }
        (Some(session), None) => lines.push(format!("continuing {harness} session {session}")),
        (None, Some(with)) => lines.push(format!("{harness} runs with {with}")),
        (None, None) => {}
    }
    for left in &settings.not_carried {
        lines.push(format!(
            "{} {} is not carried ({}); {harness} uses its own setting",
            left.name, left.value, left.reason
        ));
    }
    lines
}

/// `model gpt-6-sol, effort xhigh (recorded in the session)`, with the
/// origin once when every setting shares it and after each one otherwise.
fn settings_words(passed: &[PassedSetting]) -> Option<String> {
    let first = passed.first()?;
    let shared = passed.iter().all(|p| p.origin == first.origin);
    let items: Vec<String> = passed
        .iter()
        .map(|p| {
            if shared {
                format!("{} {}", p.name, p.value)
            } else {
                format!("{} {} ({})", p.name, p.value, origin_words(p.origin))
            }
        })
        .collect();
    let words = items.join(", ");
    Some(if shared {
        format!("{words} ({})", origin_words(first.origin))
    } else {
        words
    })
}

fn origin_words(origin: Origin) -> &'static str {
    match origin {
        Origin::Session => "recorded in the session",
        Origin::Pinned => "pinned",
    }
}

/// Tells the node the harness started, so it records it and appends
/// `Resumed`, and prints what it answered. The caller kills the harness's
/// group on an error.
async fn register(client: &Client, plan: &RunPlan, running: &Running, notes: Notes) -> Result<()> {
    let mut printer = Printer::new(notes);
    let started = Started {
        session: running.session().cloned(),
        planned: plan.session.clone(),
        version: running.version().cloned(),
    };
    let registered = client
        .harness_started(launched(plan, running.pid()), started, |event| {
            printer.print(&event);
        })
        .await;
    if let Err(e) = printer.finish() {
        tracing::warn!(error = %e, "the node's events of the launch could not be printed");
    }
    match registered {
        // The harness runs and is recorded, so nothing may end the
        // supervision now: output that cannot be written, as when the
        // reader of a pipe went away, is reported and the wait goes on.
        Ok(registered) => {
            if let Err(e) = notes.print(&[render::registered(&registered)]) {
                tracing::warn!(error = %e, "the node's answer to the launch could not be printed");
            }
            Ok(())
        }
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

/// The signals that end this command: Ctrl-C, the hang-up of a closed
/// terminal or SSH session, and a termination.
///
/// The harness runs in a process group of its own and does not get them.
/// Until the node records it, one of them stops the harness and ends the
/// command. After that, a headless run passes them on to the harness until
/// it is reaped, so the turn ends and its end is recorded; an interactive
/// one ends the command and leaves the harness to the node.
struct Interrupts {
    interrupt: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    /// How many were passed on to the harness.
    passed: u32,
}

impl Interrupts {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            hangup: signal(SignalKind::hangup())?,
            terminate: signal(SignalKind::terminate())?,
            passed: 0,
        })
    }

    /// The next one to arrive.
    async fn next(&mut self) -> rustix::process::Signal {
        use rustix::process::Signal;
        tokio::select! {
            Some(()) = self.interrupt.recv() => Signal::INT,
            Some(()) = self.hangup.recv() => Signal::HUP,
            Some(()) = self.terminate.recv() => Signal::TERM,
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

/// Ends a launch that `signal` cut before the node recorded its harness,
/// which the caller stopped if it had started.
fn interrupted(signal: rustix::process::Signal, plan: &RunPlan) -> Result<ExitCode> {
    let number = signal.as_raw();
    writeln!(
        std::io::stderr(),
        "interrupted by signal {number} before the node recorded the harness, so the harness was stopped; `fabric job status {}` shows what the node holds",
        plan.job
    )?;
    Ok(signal_exit(number))
}

/// Ends a launch that `signal` cut before the harness started, as while
/// the resume guard asks the harness its version: nothing was started, or
/// what the launch had begun was stopped when it was dropped, before any
/// turn. The terminal may be gone, as after a hang-up; the exit code still
/// says what ended the command.
fn interrupted_before_start(signal: rustix::process::Signal) -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "interrupted before the harness started; no turn ran"
    );
    signal_exit(signal.as_raw())
}

/// The exit code of a command ended by the signal numbered `number`.
fn signal_exit(number: i32) -> ExitCode {
    ExitCode::from(u8::try_from(128 + number).unwrap_or(u8::MAX))
}

/// How the reading of a headless turn ended.
#[derive(Debug)]
enum TurnEnd {
    /// The harness was reaped, and the node answered for every session it named.
    Reaped {
        report: TurnReport,
        status: std::process::ExitStatus,
    },
    /// The harness was reaped, and this signal came while the node
    /// recorded a session it named; the command ends with it.
    Interrupted(rustix::process::Signal),
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
    /// command's, copied to its end before this returns; its standard
    /// output is read line by line into a report.
    /// A session it names that the node does not hold yet goes to a
    /// recorder task at once, so a suspend during the turn captures it,
    /// while this loop keeps reaping the harness, which a suspend waits
    /// for. A signal `fabric` gets is passed on to the harness's group
    /// until the harness is reaped, and one that comes while the node
    /// records the last session gives up waiting for it and ends the turn
    /// with it, so the command exits as interrupted.
    async fn run(&self, running: &mut Running, interrupts: &mut Interrupts) -> Result<TurnEnd> {
        let pid = running.pid();
        let child = running.child_mut();
        let Some(stdout) = child.stdout.take() else {
            kill_group(running);
            bail!("the headless harness has no standard output, so it was stopped");
        };
        let copied = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await;
            })
        });
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
                    signal = interrupts.next() => {
                        interrupts.pass_on(pid, signal);
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
            None => reap(running, interrupts).await?,
        };
        // The harness's diagnostics are copied to their end before the
        // command can exit, which would drop the copy with what it had not
        // written yet; a process it started that holds its standard error
        // open is waited for as long as one that holds its output.
        let drained_by = drained_by.unwrap_or_else(|| tokio::time::Instant::now() + DRAIN);
        if let Some(mut copied) = copied
            && tokio::time::timeout_at(drained_by, &mut copied)
                .await
                .is_err()
        {
            copied.abort();
        }
        if let Some(signal) = recorded(recorder, self.plan, interrupts).await? {
            return Ok(TurnEnd::Interrupted(signal));
        }
        Ok(TurnEnd::Reaped { report, status })
    }
}

/// Waits for the recorder of the sessions a harness named; a signal stops
/// the wait, says so, and is returned, so the command ends with it.
async fn recorded(
    mut recorder: tokio::task::JoinHandle<()>,
    plan: &RunPlan,
    interrupts: &mut Interrupts,
) -> Result<Option<rustix::process::Signal>> {
    tokio::select! {
        ended = &mut recorder => {
            if let Err(e) = ended {
                tracing::warn!(error = %e, "the session recorder ended early");
            }
            Ok(None)
        }
        signal = interrupts.next() => {
            recorder.abort();
            writeln!(
                std::io::stderr(),
                "interrupted by signal {} while the node recorded the session; `fabric job status {}` shows what it holds",
                signal.as_raw(),
                plan.job
            )?;
            Ok(Some(signal))
        }
    }
}

/// The turn of an ACP agent being read: its standard output is the
/// protocol, so the adapter's driver says what the turn did on the
/// handle's feed (`Running::take_turn`) instead.
struct AgentTurn<'a> {
    client: &'a Client,
    plan: &'a RunPlan,
    notes: Notes,
    /// True for `--headless`: a signal is passed on to the agent until it
    /// is reaped. Otherwise a signal ends the command and leaves the agent
    /// to the node, as for an interactive harness.
    headless: bool,
}

impl AgentTurn<'_> {
    /// Reads the turn and reaps the agent (`run`), tells the node it
    /// exited, prints the turn, and returns the command's exit code.
    async fn finish(
        &self,
        running: &mut Running,
        feed: TurnFeed,
        interrupts: &mut Interrupts,
    ) -> Result<ExitCode> {
        let (report, status) = match self.run(running, feed, interrupts).await? {
            TurnEnd::Reaped { report, status } => (report, status),
            TurnEnd::Interrupted(signal) => return Ok(signal_exit(signal.as_raw())),
        };
        let code = exited(self.client, self.plan, running.pid(), status, interrupts).await?;
        // The agent serves the protocol and exits 0 when its stdin
        // closes, whatever became of the turn, so the turn says whether
        // the run failed, and so does an answer that could not be printed,
        // as after its reader went away. An agent stopped by a signal
        // keeps the signal's code.
        let printed = print_turn(self.plan, &report);
        if let Err(e) = &printed {
            tracing::warn!(error = %e, "the agent's turn could not be printed");
        }
        Ok(
            if status.success() && (report.failure.is_some() || printed.is_err()) {
                ExitCode::FAILURE
            } else {
                code
            },
        )
    }

    /// Reads the feed and reaps the agent, both to their end. A new
    /// session the agent opens once it runs goes to a recorder task at
    /// once, so a suspend during the turn captures it; the feed's end
    /// gives the answer and any failure. The feed is read for at most
    /// `DRAIN` after the agent exited, as a headless harness's output is.
    async fn run(
        &self,
        running: &mut Running,
        mut feed: TurnFeed,
        interrupts: &mut Interrupts,
    ) -> Result<TurnEnd> {
        let pid = running.pid();
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
        let mut status = None;
        let mut reading = true;
        let mut drained_by = None;
        while reading || status.is_none() {
            tokio::select! {
                event = feed.next(), if reading => match event {
                    Some(TurnEvent::Session(session)) => {
                        if report.session.as_ref() != Some(&session) {
                            report.session = Some(session.clone());
                            // The receiver lives until this loop ends.
                            let _ = sessions.send(session);
                        }
                    }
                    Some(TurnEvent::Ended(ended)) => {
                        report.answer = ended.answer;
                        report.failure = ended.failure;
                    }
                    None => reading = false,
                },
                exit = running.child_mut().wait(), if status.is_none() => {
                    status = Some(exit?);
                    drained_by = Some(tokio::time::Instant::now() + DRAIN);
                }
                () = sleep_until(drained_by), if reading && drained_by.is_some() => {
                    tracing::warn!("the agent's turn did not end within {DRAIN:?} of its exit");
                    reading = false;
                }
                signal = interrupts.next() => {
                    if self.headless {
                        interrupts.pass_on(pid, signal);
                    } else {
                        recorder.abort();
                        return Ok(TurnEnd::Interrupted(signal));
                    }
                }
            }
        }
        drop(sessions);
        // The loop ends only once the agent was reaped.
        let status = match status {
            Some(status) => status,
            None => reap(running, interrupts).await?,
        };
        if let Some(signal) = recorded(recorder, self.plan, interrupts).await? {
            return Ok(TurnEnd::Interrupted(signal));
        }
        Ok(TurnEnd::Reaped { report, status })
    }
}

/// Sleeps until `deadline`, or for ever when there is none.
async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Waits for a headless harness to exit, passing each signal this command
/// gets on to its group, as `Interrupts::pass_on` does.
async fn reap(
    running: &mut Running,
    interrupts: &mut Interrupts,
) -> std::io::Result<std::process::ExitStatus> {
    let pid = running.pid();
    loop {
        tokio::select! {
            exit = running.child_mut().wait() => return exit,
            signal = interrupts.next() => interrupts.pass_on(pid, signal),
        }
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
        let _ = Notes::Stderr.print(&[format!(
            "warning: the node did not record {} session {session} ({missed}); job {} keeps the session the node holds, or, for a new Codex thread, the one its next capture finds in the workspace",
            plan.harness, plan.job
        )]);
    }
}

/// Prints what a headless turn reported: the answer on standard output,
/// as the harness wrote it, then a line naming the session; a failure, a
/// missing answer and a missing session on standard error.
fn print_turn(plan: &RunPlan, report: &TurnReport) -> Result<()> {
    let harness = &plan.harness;
    if let Some(failure) = &report.failure {
        Notes::Stderr.print(&[format!("the turn failed: {failure}")])?;
    }
    if let Some(answer) = &report.answer {
        writeln!(std::io::stdout(), "{answer}")?;
    } else if report.failure.is_none() {
        Notes::Stderr.print(&[format!("warning: {harness} printed no answer")])?;
    }
    match (&report.session, &plan.session) {
        (Some(session), Some(planned)) if session != planned => Notes::Stderr.print(&[format!(
            "warning: {harness} continued in session {session}, not in {planned}"
        )])?,
        (None, _) => Notes::Stderr.print(&[format!("warning: {harness} named no session")])?,
        _ => {}
    }
    if let Some(session) = &report.session {
        Notes::Stdout.print(&[format!("{harness} session {session}")])?;
    }
    Ok(())
}

/// Takes the tree lock and runs the setup steps, once the plan is known
/// to hold and the user approved them, and returns the lock.
async fn prepare(
    state: &StateDir,
    home: &Path,
    client: &Client,
    plan: &RunPlan,
    notes: Notes,
    approval: &Approval<'_>,
) -> Result<TreeLock> {
    // The node captures and restores the harness's state under its home;
    // a harness launched under another would keep its session elsewhere.
    if plan.home != home {
        bail!(
            "the node runs with home {} and this command with home {}, so the harness would keep its session where the node never captures it; run the command with HOME={}",
            plan.home.display(),
            home.display(),
            plan.home.display()
        );
    }
    let tree = lock_tree(state, &plan.root)?;
    // Another launch may have registered its harness and let the lock go
    // since the plan was made; the node says so before any setup step
    // writes to the tree.
    let mut printer = Printer::new(notes);
    let checked = client
        .launch_check(plan, |event| printer.print(&event))
        .await;
    printer.finish()?;
    checked.map_err(Refused::from_client)?;
    materialize(&tree, &plan.root, notes, approval)?;
    Ok(tree)
}

/// Tells the node the harness exited and picks the exit code: the harness's
/// own, or 128 plus the signal that stopped it, with a line saying why,
/// which names a signal `interrupts` passed on to the harness.
///
/// The node answers once it holds the job, which a suspend or a move of it
/// can keep for minutes, so a signal ends the wait as it ends the command
/// at any other point; the node finds the group gone at its next look.
async fn exited(
    client: &Client,
    plan: &RunPlan,
    pid: std::num::NonZeroU32,
    status: std::process::ExitStatus,
    interrupts: &mut Interrupts,
) -> Result<ExitCode> {
    let told = tokio::select! {
        told = client.harness_exited(plan.job, pid, |_| {}) => told,
        signal = interrupts.next() => {
            let number = signal.as_raw();
            writeln!(
                std::io::stderr(),
                "interrupted by signal {number} while telling the node the harness exited; `fabric job status {}` shows what it holds",
                plan.job
            )?;
            return Ok(signal_exit(number));
        }
    };
    let missed = match told {
        Ok(()) => None,
        Err(ClientError::Refused(refusal)) => Some(refusal.message().to_owned()),
        Err(e) => Some(e.to_string()),
    };
    if let Some(reason) = missed {
        Notes::Stderr.print(&[format!(
            "warning: the node did not record that the harness exited ({reason}); `fabric job status {}` shows what it holds",
            plan.job
        )])?;
    }
    if let Some(signal) = status.signal() {
        if interrupts.passed > 0 {
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

/// Takes the lock over the tree at `root` that setup steps and the launch run under.
///
/// The caller holds the lock until the node has answered the harness's
/// registration, so a second `run` or `resume` of the same workspace
/// refuses with the lock's path from the setup steps until the first
/// harness is recorded, and the node's own check refuses it after that.
fn lock_tree(state: &StateDir, root: &Path) -> Result<TreeLock> {
    TreeLock::acquire(&state.tree_locks(), root).map_err(|e| match e {
        // Another run or resume of this workspace is starting its harness:
        // the same state as a harness that runs, so the same exit code.
        MaterializeError::TreeBusy { .. } => anyhow::Error::new(Refused {
            refusal: RefusalKind::JobState,
            message: e.to_string(),
        }),
        e => e.into(),
    })
}

/// Runs the setup steps a restored workspace needs on this node (ADR 0007),
/// printing each, under `lock`, once `approval` lets them run.
fn materialize(lock: &TreeLock, root: &Path, notes: Notes, approval: &Approval<'_>) -> Result<()> {
    let detected = fabric_env::detect::detect(root, &[])?;
    let plan = fabric_env::materialize::plan(&detected, lock, &NodeEnv::current())?;
    if plan.is_noop() {
        // A declared variable is required even when no step runs, so a
        // warm node refuses the launch a cold node would (ADR 0007).
        plan.ready()?;
        return Ok(());
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
    // Programs and variables are checked first, so the user is never asked
    // about steps that cannot run here.
    let ready = plan.ready()?;
    let inputs = detected.inputs_digest().to_hex();
    approval.approve(detected.spec().id()?, &inputs)?;
    ready.execute(lock)?;
    Ok(())
}

/// Whether the setup steps of an environment spec may run (ADR 0007).
///
/// A restored workspace brings its spec, and with it the programs its
/// steps run, from wherever it was last edited, an agent's edit of
/// `.fabric/env.toml` included. It brings the files those programs read
/// too: a lockfile, `package.json`'s scripts or `.pnpmfile.cjs` name code
/// an install runs, and change without changing the spec's id. So before
/// this node runs the steps of a spec, or of the same spec over other
/// install inputs, than it last ran for the job, the user approves them,
/// by answering at the terminal or with `--yes`. A spec this node only
/// captured still asks: the agent may have written it during the run.
/// The node removes the record when it restores a snapshot another node
/// captured, since the steps also run files no digest covers, such as a
/// script a declared step names, and the agent may have edited them there.
struct Approval<'a> {
    /// Where the spec id and install inputs this node last ran for the job are kept.
    record: PathBuf,
    /// `--yes`: run a new spec's steps without asking.
    yes: bool,
    /// Asks the user about the steps printed above; false when no one can answer.
    ask: &'a dyn Fn() -> Result<bool>,
}

impl Approval<'_> {
    /// Lets the steps of `spec` run over the install inputs whose digest is
    /// `inputs`, in hex, or refuses, and records the pair once approved so
    /// the next launch of the same does not ask again.
    fn approve(&self, spec: EnvSpecId, inputs: &str) -> Result<()> {
        let id = spec.to_string();
        let approved = format!("{id} {inputs}");
        let ran = std::fs::read_to_string(&self.record).ok();
        if ran.as_deref().map(str::trim) == Some(approved.as_str()) {
            return Ok(());
        }
        if !self.yes && !(self.ask)()? {
            return Err(Refused {
                refusal: RefusalKind::NeedsForce,
                message: format!(
                    "the setup steps above come from environment spec {id}, which this node has not run for the job over these lockfiles and install configuration, or not since the workspace came from another node, and were not approved; read them and what changed, then run the command again with --yes"
                ),
            }
            .into());
        }
        // The record sits in the job's directory, where a harness running
        // as this user can plant a link at a temporary's name; the durable
        // write never writes through one.
        fabricd::durable::write_atomic(&self.record, format!("{approved}\n").as_bytes())
            .with_context(|| format!("recording the approved spec in {}", self.record.display()))?;
        Ok(())
    }
}

/// Asks at the terminal whether to run the setup steps printed above; with
/// no terminal on standard input, the answer is no.
fn ask_on_terminal() -> Result<bool> {
    let stdin = std::io::stdin();
    if !rustix::termios::isatty(&stdin) {
        return Ok(false);
    }
    let mut stderr = std::io::stderr();
    write!(
        stderr,
        "this node has not approved these setup steps for the job as its workspace now is; run them? [y/N] "
    )?;
    stderr.flush()?;
    let mut answer = String::new();
    stdin.read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

/// A cache's state before the setup steps run, in words.
fn health_words(health: &CacheHealth) -> &'static str {
    match health {
        CacheHealth::Healthy => "present and complete",
        CacheHealth::Missing => "missing",
        CacheHealth::NotADirectory => "not a directory",
        CacheHealth::Empty => "empty",
        CacheHealth::Incomplete => "left unfinished by an earlier install",
        CacheHealth::Stale => "installed from other lockfiles than the workspace has now",
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
        TrustOutcome::Inherited { by, reason } => Notes::Stderr.print(&[format!(
            "warning: Fabric did not trust {} ({}); Claude trusts it through {}, so it skips its trust dialog and ignores the workspace's project settings",
            root.display(),
            left_reason(reason),
            by.display()
        )])?,
        TrustOutcome::LeftToClaude { reason } => Notes::Stderr.print(&[format!(
            "warning: Fabric did not trust {} ({}); Claude asks in its own trust dialog",
            root.display(),
            left_reason(reason)
        )])?,
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
    use fabric_harness::{NotCarried, SettingName};

    use super::*;

    /// The signal tests send a signal to this whole process, which every
    /// handler installed in it gets, so they run one at a time.
    static SIGNALS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A private directory with a short path under `/tmp`, where a control
    /// socket's path fits.
    fn socket_dir() -> tempfile::TempDir {
        let dir = tempfile::Builder::new()
            .prefix("fsig")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(
            dir.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        dir
    }

    /// A node at `socket` that takes every request and never answers one.
    fn silent_node(socket: &Path) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        })
    }

    /// A plan for a Codex job rooted in `dir`, with no session to continue.
    fn codex_plan(dir: &Path) -> RunPlan {
        RunPlan {
            job: "01M5EBB2BWY97N3YEKD6CDN2EM".parse().unwrap(),
            epoch: fabric_core::id::Epoch::FIRST,
            head: fabric_core::id::RecordHash::of(b"a head"),
            root: dir.join("work"),
            harness: HarnessKind::Codex,
            session: None,
            known_writer: None,
            goal: "goal".to_owned(),
            harness_config: fabric_client::api::HarnessConfig::default(),
            home: dir.to_owned(),
        }
    }

    /// Launches a fake `codex` headless for `plan` and releases it; under
    /// `exec` it runs `script`. It is written by a short `sh`, so this
    /// process never holds it open for writing while another test forks.
    async fn headless_codex(plan: &RunPlan, script: &str) -> Running {
        let bin = plan.home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&plan.root).unwrap();
        let fake = format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex-cli 0.155.1'; exit 0; fi\n{script}"
        );
        let mut writer = std::process::Command::new("/bin/sh")
            .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
            .arg(bin.join("codex"))
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(fake.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
        let harness_env = HarnessEnv::new(
            &plan.home,
            bin.into_os_string(),
            plan.harness_config.clone().into(),
        )
        .unwrap();
        let env = [(
            EnvVarName::parse("PATH").unwrap(),
            "/usr/bin:/bin".to_owned(),
        )];
        let launch = Launch::Headless {
            resume: None,
            prompt: "a turn".to_owned(),
        };
        let mut running = Codex::new(harness_env.home, harness_env.path_var)
            .launch(&plan.root, launch, &env)
            .await
            .unwrap();
        running.release().unwrap();
        running
    }

    /// A headless turn names its session, which goes to the node, and the
    /// harness exits. A signal while the node has not answered yet, as
    /// while a suspend holds the job, must end the command with that signal:
    /// the user asked it to stop, and a turn that went on to print and exit
    /// 0 would hide that (Codex review, 2026-10-22).
    #[tokio::test]
    async fn a_signal_while_the_node_records_the_session_ends_the_turn_with_it() {
        use std::time::Duration;

        let _one = SIGNALS.lock().await;
        let dir = socket_dir();
        let socket = dir.path().join("fabric.sock");
        let _node = silent_node(&socket);
        let plan = codex_plan(dir.path());
        let gone = dir.path().join("gone");
        let mut running = headless_codex(
            &plan,
            &format!(
                "echo '{{\"type\":\"thread.started\",\"thread_id\":\"019f233e-bb33-7342-85fa-f2d33b14368a\"}}'\ntouch '{}'\n",
                gone.display()
            ),
        )
        .await;
        let mut interrupts = Interrupts::new().unwrap();
        tokio::spawn(async move {
            while !gone.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // The harness exits right after; the turn is then waiting on the node.
            tokio::time::sleep(Duration::from_millis(300)).await;
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::HUP)
                .unwrap();
        });
        let client = Client::new(&socket);
        let turn = Turn {
            client: &client,
            plan: &plan,
            notes: Notes::Stderr,
            read_line: Codex::read_headless_line,
        };
        let ended = tokio::time::timeout(
            Duration::from_secs(10),
            turn.run(&mut running, &mut interrupts),
        )
        .await
        .expect("the signal did not end the wait for the node")
        .unwrap();
        assert!(
            matches!(ended, TurnEnd::Interrupted(signal) if signal == rustix::process::Signal::HUP),
            "{ended:?}"
        );
    }

    /// An approval for a launch that must not get as far as asking.
    fn unasked(dir: &Path) -> Approval<'static> {
        Approval {
            record: dir.join("env-spec-ran"),
            yes: false,
            ask: &|| panic!("the launch asked to run setup steps"),
        }
    }

    /// A workspace whose `.fabric/env.toml` declares one step, `mkdir`
    /// with `args`, and no cache, so the step runs at every launch.
    fn workspace_with_step(dir: &Path, args: &str) -> PathBuf {
        let root = dir.join("work");
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            format!("[[setup]]\nprogram = \"mkdir\"\nargs = [{args}]\n"),
        )
        .unwrap();
        root
    }

    /// A restored workspace can bring setup steps this node has never
    /// run, written into `.fabric/env.toml` on another node, by an agent
    /// for one. They must not run before the user approves them; with no
    /// one at a terminal to ask, the launch stops before any step runs.
    #[test]
    fn a_spec_this_node_has_not_run_does_not_run_unapproved() {
        let dir = tempfile::tempdir().unwrap();
        let root = workspace_with_step(dir.path(), "\"-p\", \"ran\"");
        let state =
            StateDir::resolve(Some(dir.path().join("state").as_os_str()), dir.path()).unwrap();
        let lock = lock_tree(&state, &root).unwrap();
        let no = || Ok(false);
        let approval = Approval {
            record: dir.path().join("env-spec-ran"),
            yes: false,
            ask: &no,
        };
        let Err(error) = materialize(&lock, &root, Notes::Stdout, &approval) else {
            panic!("the steps ran unapproved");
        };
        assert!(
            matches!(
                error.downcast_ref::<Refused>(),
                Some(Refused {
                    refusal: RefusalKind::NeedsForce,
                    ..
                })
            ),
            "{error:#}"
        );
        assert!(!root.join("ran").exists(), "a step ran");
    }

    /// An approval holds for the spec it was given for: the same steps run
    /// again without asking, and steps that changed since ask again.
    #[test]
    fn an_approval_holds_for_its_spec_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = workspace_with_step(dir.path(), "\"-p\", \"ran\"");
        let state =
            StateDir::resolve(Some(dir.path().join("state").as_os_str()), dir.path()).unwrap();
        let lock = lock_tree(&state, &root).unwrap();
        let record = dir.path().join("env-spec-ran");
        let yes = || Ok(true);
        let approval = |ask| Approval {
            record: record.clone(),
            yes: false,
            ask,
        };
        materialize(&lock, &root, Notes::Stdout, &approval(&yes)).unwrap();
        assert!(root.join("ran").is_dir());
        std::fs::remove_dir(root.join("ran")).unwrap();
        materialize(&lock, &root, Notes::Stdout, &unasked(dir.path())).unwrap();
        assert!(root.join("ran").is_dir());
        std::fs::write(
            root.join(".fabric/env.toml"),
            "[[setup]]\nprogram = \"mkdir\"\nargs = [\"-p\", \"other\"]\n",
        )
        .unwrap();
        let no = || Ok(false);
        assert!(materialize(&lock, &root, Notes::Stdout, &approval(&no)).is_err());
        assert!(
            !root.join("other").exists(),
            "a changed step ran unapproved"
        );
    }

    /// The record of an approval sits in the job's directory, where a
    /// harness running as this user can plant a link at the name a
    /// temporary takes. Recording the approval must not write through the
    /// link to the file it names, nor rename the link into the record's
    /// place (Codex review of #8, issue #23).
    #[test]
    fn an_approval_is_not_written_through_a_link_at_a_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        // The name the record was once written through, and the one the
        // durable write takes now.
        for temp in [
            "env-spec-ran.tmp".to_owned(),
            format!("env-spec-ran.{}.tmp", std::process::id()),
        ] {
            std::os::unix::fs::symlink(&victim, dir.path().join(temp)).unwrap();
        }
        let record = dir.path().join("env-spec-ran");
        let approval = Approval {
            record: record.clone(),
            yes: true,
            ask: &|| panic!("an approval with --yes asked"),
        };
        let spec = EnvSpecId::from_bytes([7; 32]);
        approval.approve(spec, "inputs").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert!(
            !std::fs::symlink_metadata(&record)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link took the record's place"
        );
        assert_eq!(
            std::fs::read_to_string(&record).unwrap(),
            format!("{spec} inputs\n")
        );
    }

    /// An approval holds for the install inputs it was given with too. A
    /// lockfile or a manager's configuration names code the install runs,
    /// and can change while the spec, which holds only the commands, keeps
    /// its id; once one changed and the step that reads it is due, the
    /// launch asks again.
    #[test]
    fn a_changed_install_input_asks_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(root.join("go.mod"), "module example.com/m\n").unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "[[cache]]\npath = \"out\"\nclass = \"build\"\n\n[[setup]]\nprogram = \"sh\"\nargs = [\"-c\", \"echo ran >> ran.log; mkdir -p out; echo x > out/f\"]\n",
        )
        .unwrap();
        let state =
            StateDir::resolve(Some(dir.path().join("state").as_os_str()), dir.path()).unwrap();
        let lock = lock_tree(&state, &root).unwrap();
        let approval = |ask| Approval {
            record: dir.path().join("env-spec-ran"),
            yes: false,
            ask,
        };
        let yes = || Ok(true);
        materialize(&lock, &root, Notes::Stdout, &approval(&yes)).unwrap();
        materialize(&lock, &root, Notes::Stdout, &unasked(dir.path())).unwrap();
        std::fs::write(
            root.join("go.mod"),
            "module example.com/m\n\nrequire example.com/x v1.0.0\n",
        )
        .unwrap();
        let no = || Ok(false);
        assert!(materialize(&lock, &root, Notes::Stdout, &approval(&no)).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("ran.log")).unwrap(),
            "ran\n",
            "the step ran again unapproved"
        );
    }

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
        let lock = lock_tree(&state, &root).unwrap();
        let Err(error) = materialize(&lock, &root, Notes::Stdout, &unasked(dir.path())) else {
            panic!("the launch went ahead");
        };
        let error = error.to_string();
        assert!(
            error.contains("FABRIC_TEST_VARIABLE_NO_ONE_SETS"),
            "{error}"
        );
    }

    fn passed(name: SettingName, value: &str, origin: Origin) -> PassedSetting {
        PassedSetting {
            name,
            value: value.to_owned(),
            origin,
        }
    }

    /// The continuing line names the carried settings and where they came
    /// from, and each setting left out gets a line saying why. A resume
    /// that carries nothing still says which session it continues, since
    /// the node no longer prints a continuing line of its own.
    #[test]
    fn the_report_names_what_was_carried_and_what_was_not() {
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let settings = LaunchSettings {
            passed: vec![
                passed(SettingName::Model, "gpt-6-sol", Origin::Session),
                passed(SettingName::Effort, "xhigh", Origin::Session),
            ],
            not_carried: vec![NotCarried {
                name: SettingName::Sandbox,
                value: "danger-full-access".to_owned(),
                reason: "Fabric never passes full access, ADR 0006".to_owned(),
            }],
        };
        assert_eq!(
            settings_lines(&HarnessKind::Codex, Some(&session), &settings),
            [
                "continuing codex session 019f233e-bb33-7342-85fa-f2d33b14368a with model gpt-6-sol, effort xhigh (recorded in the session)",
                "sandbox danger-full-access is not carried (Fabric never passes full access, ADR 0006); codex uses its own setting",
            ]
        );
        let mixed = LaunchSettings {
            passed: vec![
                passed(SettingName::Model, "o3", Origin::Pinned),
                passed(SettingName::Effort, "xhigh", Origin::Session),
            ],
            not_carried: Vec::new(),
        };
        assert_eq!(
            settings_lines(&HarnessKind::Codex, None, &mixed),
            ["codex runs with model o3 (pinned), effort xhigh (recorded in the session)"]
        );
        assert!(settings_lines(&HarnessKind::Codex, None, &LaunchSettings::default()).is_empty());
        let nothing_carried = LaunchSettings {
            passed: Vec::new(),
            not_carried: vec![NotCarried {
                name: SettingName::Model,
                value: "claude-fable-5 or claude-opus-4-8".to_owned(),
                reason: "the session has transcripts under several project keys".to_owned(),
            }],
        };
        assert_eq!(
            settings_lines(&HarnessKind::ClaudeCode, Some(&session), &nothing_carried),
            [
                "continuing claude-code session 019f233e-bb33-7342-85fa-f2d33b14368a",
                "model claude-fable-5 or claude-opus-4-8 is not carried (the session has transcripts under several project keys); claude-code uses its own setting",
            ]
        );
    }

    /// The node captures and restores harness state under its own home.
    /// A CLI whose `HOME` resolves elsewhere, with the same `FABRIC_HOME`,
    /// would launch the harness against its home, and the session would
    /// sit where no suspend looks; the launch is refused before setup.
    #[tokio::test]
    async fn a_launch_under_another_home_than_the_nodes_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("mine");
        let state = StateDir::resolve(Some(dir.path().join("state").as_os_str()), &mine).unwrap();
        let plan = RunPlan {
            job: "01M5EBB2BWY97N3YEKD6CDN2EM".parse().unwrap(),
            epoch: fabric_core::id::Epoch::FIRST,
            head: fabric_core::id::RecordHash::of(b"a head"),
            root: dir.path().join("work"),
            harness: HarnessKind::Codex,
            session: None,
            known_writer: None,
            goal: "goal".to_owned(),
            harness_config: fabric_client::api::HarnessConfig::default(),
            home: dir.path().join("the-nodes"),
        };
        let Err(error) = launch(
            &state,
            &mine,
            &client(&state),
            &plan,
            Mode::Interactive(None),
            &unasked(dir.path()),
        )
        .await
        else {
            panic!("the launch went ahead");
        };
        let error = format!("{error:#}");
        assert!(
            error.contains("the-nodes") && error.contains("mine"),
            "{error}"
        );
    }

    /// A plan can be overtaken between the node's answer and the tree lock:
    /// another launch can register its harness and let the lock go. The
    /// launch must ask the node again under the lock, before any setup step
    /// writes to the tree. With no node to ask, it stops there, before the
    /// declared variable it lacks is even looked at.
    #[tokio::test]
    async fn a_launch_asks_the_node_again_before_any_setup_step() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = dir.path().join("work");
        std::fs::create_dir_all(root.join(".fabric")).unwrap();
        std::fs::write(
            root.join(".fabric/env.toml"),
            "env = [\"FABRIC_TEST_VARIABLE_NO_ONE_SETS\"]\n",
        )
        .unwrap();
        let state_dir = dir.path().join("state");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = StateDir::resolve(Some(state_dir.as_os_str()), &home).unwrap();
        let plan = RunPlan {
            job: "01M5EBB2BWY97N3YEKD6CDN2EM".parse().unwrap(),
            epoch: fabric_core::id::Epoch::FIRST,
            head: fabric_core::id::RecordHash::of(b"a head"),
            root,
            harness: HarnessKind::Codex,
            session: None,
            known_writer: None,
            goal: "goal".to_owned(),
            harness_config: fabric_client::api::HarnessConfig::default(),
            home: home.clone(),
        };
        let Err(error) = launch(
            &state,
            &home,
            &client(&state),
            &plan,
            Mode::Interactive(None),
            &unasked(dir.path()),
        )
        .await
        else {
            panic!("the launch went ahead");
        };
        assert!(
            matches!(
                error.downcast_ref::<ClientError>(),
                Some(ClientError::NoDaemon { .. })
            ),
            "{error:#}"
        );
    }

    /// Once the harness exits, the command tells the node, which can wait
    /// on the job through a suspend or a move for minutes, or not answer
    /// at all. A signal must still end the command then: the handlers the
    /// launch installed replaced the default action for the whole process,
    /// so nothing else would.
    #[tokio::test]
    async fn a_signal_ends_the_wait_for_the_node_after_the_harness_exited() {
        use std::os::unix::process::ExitStatusExt;
        use std::time::Duration;

        let _one = SIGNALS.lock().await;
        let dir = socket_dir();
        let socket = dir.path().join("fabric.sock");
        let _node = silent_node(&socket);
        let mut interrupts = Interrupts::new().unwrap();
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::HUP)
                .unwrap();
        });
        let plan = codex_plan(dir.path());
        let pid = std::num::NonZeroU32::new(std::process::id()).unwrap();
        let status = std::process::ExitStatus::from_raw(0);
        let client = Client::new(&socket);
        let told = exited(&client, &plan, pid, status, &mut interrupts);
        let code = tokio::time::timeout(Duration::from_secs(10), told)
            .await
            .expect("the signal did not end the wait")
            .unwrap();
        assert_eq!(code, signal_exit(rustix::process::Signal::HUP.as_raw()));
    }
}
