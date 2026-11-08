//! The command-line contract, in one place (docs/cli-contract.md).
//!
//! Every rule a script or an agent can depend on is enforced here rather
//! than in each command: the global options and how they resolve into the
//! run's modes, the help layout, the usage errors, the one shape every
//! failure takes in words and in JSON, the exit code of each failure class,
//! and where an answer goes. The commands only say what they answer.
//!
//! The rules come from the Vercel CLI design guidelines
//! (<https://vercel.com/design/cli>); docs/cli-contract.md traces each one,
//! and `tests/cli_contract.rs` walks every command against them.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, ArgAction, ArgMatches, ColorChoice, Command, CommandFactory};
use fabric_client::ClientError;
use fabric_client::api::RefusalKind;
use serde_json::{Map, Value, json};

use crate::{Cli, Refused};

/// Where the command reference lives; each command's help ends with its anchor.
pub(crate) const REFERENCE: &str =
    "https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md";

/// The exit code of a usage error or of missing input.
pub(crate) const USAGE_EXIT: u8 = 2;

/// The exit code of a command that found no node to answer it, the socket
/// missing or left behind by a node that is gone (ADR 0016, Exit codes).
pub(crate) const NO_DAEMON_EXIT: u8 = 80;

/// The options every command takes at any depth: id, long name, short name and summary.
const GLOBALS: &[(&str, &str, Option<char>, &str)] = &[
    (
        "debug",
        "debug",
        Some('d'),
        "Print diagnostic detail on stderr",
    ),
    (
        "json",
        "json",
        None,
        "Print the result as one JSON document on stdout, and a failure as one JSON line on stderr",
    ),
    ("no_color", "no-color", None, "Disable color"),
    (
        "non_interactive",
        "non-interactive",
        None,
        "Never prompt; fail with the input or approval needed instead",
    ),
    ("version", "version", Some('v'), "Print the version"),
];

/// How the root help groups the commands: in the order a user meets them.
const GROUPS: &[(&str, &[&str])] = &[
    ("Set up a node", &["serve", "node", "doctor"]),
    ("Work on jobs", &["job"]),
    ("Managed jobs", &["managed"]),
];

/// The examples each command's help shows, by command path. A placeholder
/// is `<UPPER_CASE>`; `tests/cli_contract.rs` fills each one in and parses
/// every example through the real command tree.
pub const EXAMPLES: &[(&str, &[(&str, &str)])] = &[
    (
        "",
        &[
            ("Run a node on this machine", "fabric serve"),
            (
                "Make a job from a repository",
                "fabric job new --repo <PATH> --goal <TEXT>",
            ),
            (
                "Move a job to a paired node",
                "fabric job move <JOB> --to <NODE>",
            ),
        ],
    ),
    (
        "serve",
        &[(
            "Run the node, listening on one port",
            "fabric serve --listen 0.0.0.0:7777",
        )],
    ),
    (
        "doctor",
        &[
            ("Print this node's facts", "fabric doctor"),
            (
                "Check a move before making it",
                "fabric doctor --peer <NODE> --job <JOB>",
            ),
        ],
    ),
    (
        "node id",
        &[("Read this node's id in a script", "fabric node id")],
    ),
    (
        "node add",
        &[
            (
                "Pair a node this one can dial",
                "fabric node add <NODE> <HOST:PORT>",
            ),
            (
                "Pair a node behind a NAT that dials in",
                "fabric node add <NODE>",
            ),
        ],
    ),
    (
        "node ls",
        &[("List the paired nodes as JSON", "fabric node ls --json")],
    ),
    (
        "node ping",
        &[("Ping a paired node", "fabric node ping <NODE>")],
    ),
    (
        "job new",
        &[
            (
                "Make a job and keep its id",
                "fabric job new --repo <PATH> --goal <TEXT>",
            ),
            (
                "Make a job for Codex",
                "fabric job new --repo <PATH> --goal <TEXT> --harness codex",
            ),
        ],
    ),
    (
        "job run",
        &[
            ("Run the job in this terminal", "fabric job run <JOB>"),
            (
                "Run one turn in a script",
                "fabric job run <JOB> --prompt <TEXT> --headless",
            ),
        ],
    ),
    (
        "job suspend",
        &[("Suspend the job here", "fabric job suspend <JOB>")],
    ),
    (
        "job resume",
        &[
            ("Resume a job here", "fabric job resume <JOB>"),
            (
                "Pull a job from the node that holds it",
                "fabric job resume <JOB> --on <NODE>",
            ),
        ],
    ),
    (
        "job move",
        &[
            (
                "Move the job to a paired node",
                "fabric job move <JOB> --to <NODE>",
            ),
            (
                "Move it past the refusals a human waived",
                "fabric job move <JOB> --to <NODE> --force",
            ),
        ],
    ),
    (
        "job steal",
        &[
            ("See what a steal would do", "fabric job steal <JOB>"),
            (
                "Take the job when its owner is gone for good",
                "fabric job steal <JOB> --force --reason <TEXT>",
            ),
        ],
    ),
    (
        "job status",
        &[("Read the job's state", "fabric job status <JOB>")],
    ),
    (
        "job ls",
        &[("List the jobs as JSON", "fabric job ls --json")],
    ),
    (
        "job log",
        &[("Read the job's ledger", "fabric job log <JOB>")],
    ),
    (
        "job approve",
        &[(
            "Approve the work with a note",
            "fabric job approve <JOB> --note <TEXT>",
        )],
    ),
    (
        "job reject",
        &[(
            "Reject the work with a note",
            "fabric job reject <JOB> --note <TEXT>",
        )],
    ),
    (
        "job repair",
        &[("Settle a forked job", "fabric job repair <JOB>")],
    ),
    (
        "job export",
        &[
            (
                "Keep the workspace at the root as a snapshot",
                "fabric job export <JOB>",
            ),
            (
                "Keep an orphan workspace",
                "fabric job export <JOB> --path <PATH>",
            ),
        ],
    ),
    (
        "managed create",
        &[(
            "Make a managed job under a key a retry can reuse",
            "fabric managed create --repo <PATH> --goal <TEXT> --operation-key <KEY>",
        )],
    ),
    (
        "managed cancel",
        &[(
            "Cancel a managed job at a known revision",
            "fabric managed cancel <JOB> --expected-revision <REVISION>",
        )],
    ),
    (
        "managed status",
        &[("Read a managed job", "fabric managed status <JOB>")],
    ),
    (
        "managed ls",
        &[("List the managed jobs", "fabric managed ls")],
    ),
];

/// The full command tree with the contract's help layout, examples and
/// global options on every command.
#[must_use]
pub fn command() -> Command {
    let root = Cli::command()
        .disable_version_flag(true)
        .help_expected(true);
    decorate(root, &[])
}

/// Every command path below the root, depth first in help order, the
/// root as the empty path; `help` is clap's and is left out.
#[must_use]
pub fn paths() -> Vec<Vec<String>> {
    fn walk(cmd: &Command, at: &[String], out: &mut Vec<Vec<String>>) {
        out.push(at.to_vec());
        for sub in cmd.get_subcommands().filter(|sub| sub.get_name() != "help") {
            let mut below = at.to_vec();
            below.push(sub.get_name().to_owned());
            walk(sub, &below, out);
        }
    }
    let mut out = Vec::new();
    walk(&command(), &[], &mut out);
    out
}

/// The help `fabric <path> --help` prints when standard output is not a terminal.
#[must_use]
pub fn help(path: &[String]) -> String {
    let argv = std::iter::once("fabric".to_owned())
        .chain(path.iter().cloned())
        .chain(std::iter::once("--help".to_owned()));
    match command().try_get_matches_from(argv) {
        Err(e) if e.kind() == ErrorKind::DisplayHelp => e.render().to_string(),
        _ => String::new(),
    }
}

/// docs/reference/cli.md: the help of every command, as `--help` prints it.
#[must_use]
pub fn reference() -> String {
    let mut text = String::from(
        "# Command reference\n\nGenerated from `fabric <command> --help`; do not edit. The contract every command keeps is in [../cli-contract.md](../cli-contract.md). Regenerate with `FABRIC_BLESS=1 cargo test -p fabric --test cli_contract`.\n",
    );
    for path in paths() {
        let title = std::iter::once("fabric")
            .chain(path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(
            text,
            "\n## {title}\n\n```text\n{}\n```\n",
            help(&path).trim_end()
        );
    }
    text
}

/// Applies the help layout to `cmd`, at `path` below the root, and to every command under it.
fn decorate(cmd: Command, path: &[&str]) -> Command {
    let root = path.is_empty();
    let mut cmd = cmd
        // Options sort by long name: one display order for all of them.
        .mut_args(|arg| arg.display_order(0))
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .help_template(if root {
            root_template()
        } else {
            "{usage-heading} {usage}\n{about}\n\n{all-args}{after-help}".to_owned()
        })
        .after_help(after_help(path, root));
    cmd = cmd.arg(
        Arg::new("help")
            .short('h')
            .long("help")
            .help("Print help")
            // `-h` and `--help` print the same text.
            .action(ArgAction::HelpShort)
            .help_heading("Global Options")
            .display_order(0)
            .hide(!root),
    );
    for &(id, long, short, help) in GLOBALS {
        let mut arg = Arg::new(id)
            .long(long)
            .help(help)
            .action(ArgAction::SetTrue)
            .help_heading("Global Options")
            .display_order(0)
            // Shown in the root help; every other help points there.
            .hide(!root);
        if let Some(short) = short {
            arg = arg.short(short);
        }
        cmd = cmd.arg(arg);
    }
    if root {
        // `fabric help <path>` prints exactly what `fabric <path> --help` does.
        cmd = cmd.subcommand(
            Command::new("help")
                .about("Print the help of a command")
                .arg(
                    Arg::new("command")
                        .num_args(0..)
                        .value_name("COMMAND")
                        .help("The command, such as `job move`"),
                )
                .disable_help_flag(true),
        );
    }
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .filter(|name| name != "help")
        .collect();
    for name in names {
        let mut below: Vec<&str> = path.to_vec();
        below.push(&name);
        let below: Vec<&str> = below.clone();
        cmd = cmd.mut_subcommand(&name, |sub| decorate(sub, &below));
    }
    cmd
}

/// The root help: usage and summary, the commands grouped by workflow, the global options.
fn root_template() -> String {
    let cli = Cli::command();
    let width = GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|name| name.len())
        .max()
        .unwrap_or(0);
    let mut groups = String::new();
    for (heading, names) in GROUPS {
        let _ = writeln!(groups, "{heading}:");
        for name in *names {
            let about = cli
                .find_subcommand(name)
                .and_then(Command::get_about)
                .map(ToString::to_string)
                .unwrap_or_default();
            let _ = writeln!(groups, "  {name:width$}  {about}");
        }
        groups.push('\n');
    }
    format!(
        "{{usage-heading}} {{usage}}\n{{about}}\n\n{groups}Global Options:\n{{options}}\n{{after-help}}"
    )
}

/// What follows the options in a command's help: the pointer to the
/// global options, the examples, and where to learn more.
fn after_help(path: &[&str], root: bool) -> String {
    let key = path.join(" ");
    let mut text = String::new();
    if !root {
        text.push_str("Global Options: See `fabric --help`\n");
    }
    if let Some((_, examples)) = EXAMPLES.iter().find(|(at, _)| *at == key) {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("Examples:\n");
        for (n, (what, example)) in examples.iter().enumerate() {
            if n > 0 {
                text.push('\n');
            }
            let _ = write!(text, "  {what}\n    {example}\n");
        }
    }
    if !text.is_empty() {
        text.push('\n');
    }
    let anchor = if root {
        String::new()
    } else {
        format!("#fabric-{}", path.join("-"))
    };
    let _ = write!(text, "Learn more: {REFERENCE}{anchor}");
    text
}

/// The global options as one run resolved them.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one independent option, as the user gave it"
)]
pub(crate) struct Globals {
    pub(crate) debug: bool,
    pub(crate) json: bool,
    pub(crate) no_color: bool,
    pub(crate) non_interactive: bool,
    pub(crate) version: bool,
}

impl Globals {
    /// The options set at any depth of `matches`.
    fn from_matches(matches: &ArgMatches) -> Self {
        let mut globals = Self::default();
        let mut at = Some(matches);
        while let Some(matches) = at {
            let flag = |id: &str| matches.try_get_one::<bool>(id).ok().flatten() == Some(&true);
            globals.debug |= flag("debug");
            globals.json |= flag("json");
            globals.no_color |= flag("no_color");
            globals.non_interactive |= flag("non_interactive");
            globals.version |= flag("version");
            at = matches.subcommand().map(|(_, sub)| sub);
        }
        globals
    }

    /// The options a parse that stops at the first error has seen, so a
    /// usage error still answers in the format and color the run asked for.
    /// Also returns the command path that parse reached, such as `fabric job status`.
    fn lenient(cmd: &Command, argv: &[OsString]) -> (Self, String) {
        let parsed = cmd.clone().ignore_errors(true).try_get_matches_from(argv);
        let mut path = vec!["fabric".to_owned()];
        if let Ok(matches) = &parsed {
            let mut at = matches;
            while let Some((name, sub)) = at.subcommand() {
                path.push(name.to_owned());
                at = sub;
            }
        }
        let mut globals = parsed
            .map(|matches| Self::from_matches(&matches))
            .unwrap_or_default();
        // A parse that stops at an unknown option never reaches the options
        // after it; for the format of the error alone, `--json` anywhere
        // before `--` counts.
        let before_end = argv.iter().skip(1).take_while(|arg| *arg != "--");
        for arg in before_end {
            globals.json |= arg == "--json";
            globals.no_color |= arg == "--no-color";
        }
        (globals, path.join(" "))
    }
}

/// How one run prints and asks, resolved once from the options, the
/// environment and the terminals (docs/cli-contract.md, Modes).
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the guide resolves each mode independently of the others"
)]
pub(crate) struct Modes {
    /// `--json`: one document on stdout, a failure as one line on stderr.
    pub(crate) json: bool,
    /// May prompt: not `--json`, not `--non-interactive`, not in CI, and
    /// standard input and standard error are terminals.
    pub(crate) interactive: bool,
    /// Standard error is a terminal that renders glyphs.
    pub(crate) rich: bool,
    /// Standard error may carry color.
    pub(crate) color: bool,
    /// Standard output is a terminal, so a JSON document is indented.
    pub(crate) stdout_tty: bool,
    /// `--debug`.
    pub(crate) debug: bool,
}

impl Modes {
    fn resolve(globals: Globals, env: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let stderr_tty = std::io::stderr().is_terminal();
        let dumb = env("TERM").is_some_and(|term| term == "dumb");
        let ci = env("CI").is_some_and(|ci| !ci.is_empty());
        Self {
            json: globals.json,
            interactive: !globals.json
                && !globals.non_interactive
                && !ci
                && std::io::stdin().is_terminal()
                && stderr_tty,
            rich: stderr_tty && !dumb,
            color: color(globals.no_color, env, stderr_tty && !dumb),
            stdout_tty: std::io::stdout().is_terminal(),
            debug: globals.debug,
        }
    }
}

/// Whether a stream may carry color: `--no-color`, then a nonempty
/// `NO_COLOR`, then `FORCE_COLOR` (`0` is off), then whether the stream is
/// a terminal that renders it.
fn color(no_color: bool, env: &dyn Fn(&str) -> Option<OsString>, detected: bool) -> bool {
    if no_color || env("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    match env("FORCE_COLOR") {
        Some(value) if value == "0" => false,
        Some(_) => true,
        None => detected,
    }
}

fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// One run: its modes and what it was asked, for the failure that may end it.
pub(crate) struct Run {
    pub(crate) modes: Modes,
    /// The arguments, lossy, for a recovery command that repeats them.
    argv: Vec<String>,
    /// The ids of the options the command that runs takes.
    takes: Vec<String>,
}

/// What `main` does with the command line: run a command, or exit at once.
pub(crate) enum Parsed {
    Run(Box<Cli>, Run),
    Exit(std::process::ExitCode),
}

/// Parses `argv` under the contract: help and version print and exit 0, a
/// usage error prints in the run's format and exits 2.
pub(crate) fn parse(argv: &[OsString]) -> Parsed {
    use clap::FromArgMatches;
    let mut cmd = command();
    let (early, reached) = Globals::lenient(&cmd, argv);
    let modes = Modes::resolve(early, &process_env);
    if early.no_color || process_env("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        cmd = cmd.color(ColorChoice::Never);
    }
    let lossy: Vec<String> = argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let failed =
        |error: clap::Error| -> Parsed { Parsed::Exit(clap_exit(&error, modes, &reached)) };
    let matches = match cmd.clone().try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(e) if early.version && !matches!(e.kind(), ErrorKind::DisplayHelp) => {
            return Parsed::Exit(version(modes));
        }
        Err(e) => return failed(e),
    };
    let globals = Globals::from_matches(&matches);
    if globals.version {
        return Parsed::Exit(version(modes));
    }
    if let Some(("help", asked)) = matches.subcommand() {
        let path: Vec<String> = asked
            .get_many::<String>("command")
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        return Parsed::Exit(help_of(&path, modes));
    }
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => return failed(e),
    };
    let mut leaf = &cmd;
    let mut at = &matches;
    while let Some((name, sub)) = at.subcommand() {
        let Some(next) = leaf.find_subcommand(name) else {
            break;
        };
        leaf = next;
        at = sub;
    }
    let takes = leaf
        .get_arguments()
        .map(|arg| arg.get_id().to_string())
        .collect();
    Parsed::Run(
        Box::new(cli),
        Run {
            modes: Modes::resolve(globals, &process_env),
            argv: lossy,
            takes,
        },
    )
}

/// `fabric help <path>`: the help of the command at `path`, or a usage
/// error naming the first word that is not a command.
fn help_of(path: &[String], modes: Modes) -> std::process::ExitCode {
    let mut argv: Vec<OsString> = vec!["fabric".into()];
    argv.extend(path.iter().map(OsString::from));
    argv.push("--help".into());
    let mut cmd = command();
    if !modes.stdout_tty {
        cmd = cmd.color(ColorChoice::Never);
    }
    let reached = std::iter::once("fabric")
        .chain(path.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    match cmd.try_get_matches_from(argv) {
        Err(e) => clap_exit(&e, modes, &reached),
        Ok(_) => std::process::ExitCode::SUCCESS,
    }
}

/// `--version`: the version alone, or `{ "version" }` with `--json`.
fn version(modes: Modes) -> std::process::ExitCode {
    let version = env!("CARGO_PKG_VERSION");
    let text = if modes.json {
        json!({ "version": version }).to_string()
    } else {
        version.to_owned()
    };
    let _ = writeln!(std::io::stdout().lock(), "{text}");
    std::process::ExitCode::SUCCESS
}

/// Help on stdout with exit 0; any other parse failure as a usage error.
fn clap_exit(error: &clap::Error, modes: Modes, reached: &str) -> std::process::ExitCode {
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let help = error.render();
            let text = if modes.stdout_tty {
                help.ansi().to_string()
            } else {
                help.to_string()
            };
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
            std::process::ExitCode::SUCCESS
        }
        ErrorKind::DisplayVersion => version(modes),
        _ => {
            usage_failure(error, reached).print(modes);
            std::process::ExitCode::from(USAGE_EXIT)
        }
    }
}

/// The command path a clap error's usage line names, such as `fabric
/// job status`, else `reached`, the path the parse got to.
fn usage_path(error: &clap::Error, reached: &str) -> String {
    let Some(ContextValue::StyledStr(usage)) = error.get(ContextKind::Usage) else {
        return reached.to_owned();
    };
    let usage = usage.to_string();
    let words: Vec<&str> = usage
        .trim_start_matches("Usage:")
        .split_whitespace()
        .take_while(|word| !word.starts_with(['<', '[', '-']))
        .collect();
    if words.is_empty() {
        reached.to_owned()
    } else {
        words.join(" ")
    }
}

fn context_text(error: &clap::Error, kind: ContextKind) -> Option<String> {
    match error.get(kind)? {
        ContextValue::String(text) => Some(text.clone()),
        ContextValue::Strings(texts) => texts.first().cloned(),
        ContextValue::StyledStr(text) => Some(text.to_string()),
        ContextValue::StyledStrs(texts) => texts.first().map(ToString::to_string),
        _ => None,
    }
}

/// A parse failure as a usage error, with the one suggestion clap has and
/// the help command to read.
pub(crate) fn usage_failure(error: &clap::Error, reached: &str) -> Failure {
    let path = usage_path(error, reached);
    let invalid = context_text(error, ContextKind::InvalidArg);
    let (reason, headline, action_required) = match error.kind() {
        ErrorKind::UnknownArgument => (
            "unknown_option",
            format!(
                "Unknown option {} for {path}.",
                invalid.as_deref().unwrap_or("?")
            ),
            false,
        ),
        ErrorKind::InvalidSubcommand => (
            "unknown_command",
            format!(
                "Unknown command {} for {path}.",
                context_text(error, ContextKind::InvalidSubcommand)
                    .as_deref()
                    .unwrap_or("?")
            ),
            false,
        ),
        ErrorKind::MissingRequiredArgument | ErrorKind::MissingSubcommand => (
            "missing_argument",
            format!(
                "Can't run {path} without {}.",
                invalid.as_deref().unwrap_or("a required argument")
            ),
            true,
        ),
        _ => {
            let rendered = error.render().to_string();
            let first = rendered
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches("error: ");
            (
                "invalid_arguments",
                format!("Can't run {path}: {first}."),
                false,
            )
        }
    };
    let suggestion = context_text(error, ContextKind::SuggestedSubcommand)
        .or_else(|| context_text(error, ContextKind::SuggestedArg));
    let mut rows = Vec::new();
    if let Some(suggestion) = &suggestion {
        rows.push(("Did you mean".to_owned(), suggestion.clone()));
    }
    let help = format!("{path} --help");
    Failure {
        reason: reason.to_owned(),
        exit: USAGE_EXIT,
        action_required,
        headline,
        rows,
        next: Some(Next::text("See its usage", &help, None)),
        data: suggestion.map(|suggestion| json!({ "suggestion": suggestion })),
        retryable: None,
        user_action_required: false,
    }
}

/// A recovery command: what it is for, the command, and when to use it.
#[derive(Debug, Clone)]
pub(crate) struct Next {
    label: String,
    argv: Vec<String>,
    when: Option<String>,
}

impl Next {
    /// A command given as text whose words split on spaces, placeholders included.
    pub(crate) fn text(label: &str, command: &str, when: Option<String>) -> Self {
        Self {
            label: label.to_owned(),
            argv: command.split_whitespace().map(str::to_owned).collect(),
            when,
        }
    }

    fn command(&self) -> String {
        self.argv
            .iter()
            .map(|word| shell_word(word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `word` quoted for a POSIX shell when it needs to be; a placeholder stays as it is.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,<>".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// A failure in the one shape every command uses (docs/cli-contract.md, Errors).
#[derive(Debug)]
pub(crate) struct Failure {
    /// The stable `snake_case` reason, from the node's registry or the CLI's own.
    pub(crate) reason: String,
    pub(crate) exit: u8,
    /// Blocked before any effect, and input or approval would let it run.
    pub(crate) action_required: bool,
    /// What failed and the known cause.
    pub(crate) headline: String,
    /// Context and identifiers, as label and value.
    pub(crate) rows: Vec<(String, String)>,
    pub(crate) next: Option<Next>,
    /// Resolved context for a program.
    pub(crate) data: Option<Value>,
    pub(crate) retryable: Option<bool>,
    pub(crate) user_action_required: bool,
}

impl Failure {
    /// The human error, on standard error:
    ///
    /// ```text
    /// Error: <headline>
    ///   <Label>         <value>
    ///
    ///   <Recovery>:
    ///     <command>
    /// ```
    fn human(&self, modes: Modes) -> String {
        let prefix = match (modes.rich, modes.color) {
            (true, true) => "\x1b[31m\u{2717}\x1b[0m".to_owned(),
            (true, false) => "\u{2717}".to_owned(),
            (false, true) => "\x1b[31mError:\x1b[0m".to_owned(),
            (false, false) => "Error:".to_owned(),
        };
        let printable = |text: &str| fabric_client::sentences::printable(text).into_owned();
        let mut out = format!("{prefix} {}\n", printable(&self.headline));
        for (label, value) in &self.rows {
            let _ = writeln!(out, "  {label:<16}{}", printable(value));
        }
        if let Some(next) = &self.next {
            let _ = write!(
                out,
                "\n  {}:\n    {}\n",
                next.label,
                printable(&next.command())
            );
        }
        out
    }

    /// The error document: one line, with the fields that apply.
    fn document(&self) -> Value {
        let mut doc = Map::new();
        doc.insert(
            "status".to_owned(),
            Value::from(if self.action_required {
                "action_required"
            } else {
                "error"
            }),
        );
        doc.insert("reason".to_owned(), Value::from(self.reason.as_str()));
        doc.insert(
            "message".to_owned(),
            Value::from(fabric_client::sentences::printable(&self.headline).into_owned()),
        );
        if let Some(next) = &self.next {
            let mut entry = Map::new();
            entry.insert("command".to_owned(), Value::from(next.command()));
            entry.insert(
                "argv".to_owned(),
                Value::from(next.argv.iter().map(String::as_str).collect::<Vec<_>>()),
            );
            if let Some(when) = &next.when {
                entry.insert("when".to_owned(), Value::from(when.as_str()));
            }
            doc.insert("next".to_owned(), Value::Array(vec![Value::Object(entry)]));
        }
        if let Some(data) = &self.data {
            doc.insert("data".to_owned(), data.clone());
        }
        if let Some(retryable) = self.retryable {
            doc.insert("retryable".to_owned(), Value::from(retryable));
        }
        if self.user_action_required {
            doc.insert("userActionRequired".to_owned(), Value::from(true));
        }
        Value::Object(doc)
    }

    /// Prints the failure on standard error, in the run's format.
    pub(crate) fn print(&self, modes: Modes) {
        let text = if modes.json {
            format!("{}\n", self.document())
        } else {
            self.human(modes)
        };
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    }
}

/// The commands a node's sentence names in backticks, `fabric ...` ones only.
fn named_commands(message: &str) -> Vec<String> {
    message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|quoted| quoted.starts_with("fabric "))
        .map(str::to_owned)
        .collect()
}

impl Run {
    /// Where a command's own lines go in this run: standard error, or nowhere under `--json`.
    pub(crate) fn notes(&self) -> crate::Notes {
        if self.modes.json {
            crate::Notes::Silent
        } else {
            crate::Notes::Stderr
        }
    }

    /// The same command again with `option` added.
    fn again_with(&self, option: &str) -> Vec<String> {
        let mut argv = self.argv.clone();
        if let Some(first) = argv.first_mut() {
            "fabric".clone_into(first);
        }
        if !argv.iter().any(|arg| arg == option) {
            argv.push(option.to_owned());
        }
        argv
    }

    /// The same command again with `option` set to `value`.
    fn again_with_value(&self, option: &str, value: &str) -> Vec<String> {
        let mut argv = self.again_with(option);
        argv.push(value.to_owned());
        argv
    }

    /// The failure an error that ended the command is.
    pub(crate) fn failure(&self, error: &anyhow::Error) -> Failure {
        if let Some(keyed) = error.downcast_ref::<Keyed>() {
            return self.keyed(keyed);
        }
        if let Some(failure) = error.downcast_ref::<Usage>() {
            return Failure {
                reason: "invalid_arguments".to_owned(),
                exit: USAGE_EXIT,
                action_required: false,
                headline: failure.0.clone(),
                rows: Vec::new(),
                next: Some(Next::text(
                    "See its usage",
                    &format!("{} --help", self.path()),
                    None,
                )),
                data: None,
                retryable: None,
                user_action_required: false,
            };
        }
        if let Some(refused) = error.downcast_ref::<Refused>() {
            return self.refused(refused);
        }
        if let Some(client) = error.downcast_ref::<ClientError>() {
            return client_failure(client);
        }
        Failure {
            reason: "failed".to_owned(),
            exit: 1,
            action_required: false,
            headline: format!("{error:#}"),
            rows: Vec::new(),
            next: None,
            data: None,
            retryable: None,
            user_action_required: false,
        }
    }

    /// The command path of this run, such as `fabric job move`.
    fn path(&self) -> String {
        let mut path = vec!["fabric".to_owned()];
        let cmd = command();
        let mut at = &cmd;
        for arg in self.argv.iter().skip(1) {
            match at.find_subcommand(arg) {
                Some(sub) => {
                    path.push(arg.clone());
                    at = sub;
                }
                None if arg.starts_with('-') => {}
                None => break,
            }
        }
        path.join(" ")
    }

    /// A failure under an operation key the CLI made: every such failure
    /// carries the key, and one whose outcome is unknown offers the same
    /// command under it, which the node answers without running it twice.
    fn keyed(&self, keyed: &Keyed) -> Failure {
        let mut failure = self.failure(&keyed.error);
        let key = Value::from(keyed.key.as_str());
        failure.data = Some(match failure.data.take() {
            Some(Value::Object(mut data)) => {
                data.insert("operation_key".to_owned(), key);
                Value::Object(data)
            }
            None => json!({ "operation_key": key }),
            Some(other) => json!({ "operation_key": key, "detail": other }),
        });
        failure
            .rows
            .push(("Operation key".to_owned(), keyed.key.clone()));
        if failure.reason == "unknown_outcome" {
            failure.next = Some(Next {
                label: "Run it again under the same key".to_owned(),
                argv: self.again_with_value("--operation-key", &keyed.key),
                when: Some(
                    "Safe to repeat: the node answers a key it has seen with its first answer"
                        .to_owned(),
                ),
            });
        }
        failure
    }

    fn refused(&self, refused: &Refused) -> Failure {
        let named = named_commands(&refused.message);
        let needs = refused.refusal == RefusalKind::NeedsForce;
        let next = if refused.code == "confirmation_required" {
            Some(Next {
                label: "Approve and run again".to_owned(),
                argv: self.again_with("--yes"),
                when: Some(
                    "Only after a human approves: runs the setup steps this node has not run for the job".to_owned(),
                ),
            })
        } else if let Some(command) = named.first() {
            Some(Next::text("To continue, run", command, None))
        } else if needs && self.takes.iter().any(|id| id == "force") {
            Some(Next {
                label: "Waive the refusal and run again".to_owned(),
                argv: self.again_with("--force"),
                when: Some(
                    "Only after a human approves: waives the refusals that can be waived"
                        .to_owned(),
                ),
            })
        } else {
            None
        };
        Failure {
            reason: refused.code.clone(),
            exit: crate::refusal_exit(refused.refusal),
            action_required: needs,
            headline: refused.message.clone(),
            rows: Vec::new(),
            next,
            data: refused.data.clone(),
            retryable: Some(false),
            user_action_required: needs,
        }
    }
}

/// A client error that is not a refusal, as a failure.
fn client_failure(error: &ClientError) -> Failure {
    let plain = |reason: &str, headline: String| Failure {
        reason: reason.to_owned(),
        exit: 1,
        action_required: false,
        headline,
        rows: Vec::new(),
        next: None,
        data: None,
        retryable: None,
        user_action_required: false,
    };
    match error {
        ClientError::NoDaemon { path, source } => Failure {
            reason: "no_daemon".to_owned(),
            exit: NO_DAEMON_EXIT,
            action_required: false,
            headline: "No Fabric node is running for this user.".to_owned(),
            rows: vec![
                ("Socket".to_owned(), path.display().to_string()),
                ("Cause".to_owned(), source.to_string()),
            ],
            next: Some(Next::text("Start one", "fabric serve", None)),
            data: Some(json!({ "socket": path.display().to_string() })),
            retryable: Some(true),
            user_action_required: false,
        },
        ClientError::Untrusted { path, reason } => {
            let mut failure = plain(
                "untrusted_state",
                format!(
                    "Can't trust the node's answers. {} {reason}.",
                    path.display()
                ),
            );
            if reason.contains("mode") {
                failure.next = Some(Next {
                    label: "Make it private".to_owned(),
                    argv: vec![
                        "chmod".to_owned(),
                        "700".to_owned(),
                        path.display().to_string(),
                    ],
                    when: None,
                });
            }
            failure.data = Some(json!({ "path": path.display().to_string() }));
            failure
        }
        ClientError::Incompatible { .. }
        | ClientError::Unsupported { .. }
        | ClientError::UnsupportedField { .. }
        | ClientError::ClosedBeforeWelcome { .. } => plain("incompatible_node", error.to_string()),
        ClientError::ClosedBeforeAnswer { .. } => {
            let mut failure = plain("unknown_outcome", error.to_string());
            failure.retryable = Some(false);
            failure
        }
        ClientError::Refused(refusal) => {
            let refused = Refused::from_refusal(refusal);
            Failure {
                reason: refused.code.clone(),
                exit: crate::refusal_exit(refused.refusal),
                action_required: false,
                headline: refused.message.clone(),
                rows: Vec::new(),
                next: named_commands(&refused.message)
                    .first()
                    .map(|command| Next::text("To continue, run", command, None)),
                data: refused.data,
                retryable: Some(false),
                user_action_required: false,
            }
        }
        _ => plain("node_error", error.to_string()),
    }
}

/// An error of a command that ran under an operation key the CLI made, so
/// its failure names the key a retry must reuse (`Run::keyed`).
#[derive(Debug)]
pub(crate) struct Keyed {
    pub(crate) key: String,
    pub(crate) error: anyhow::Error,
}

impl std::fmt::Display for Keyed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.error)
    }
}

impl std::error::Error for Keyed {}

/// A usage error a command finds after parsing, such as `--json` where the
/// harness owns standard output; exits 2.
#[derive(Debug)]
pub(crate) struct Usage(pub(crate) String);

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Usage {}

/// Writes `doc` as the run's one JSON document: indented on a terminal, one line otherwise.
pub(crate) fn print_document(modes: Modes, doc: &Value) -> anyhow::Result<()> {
    let text = if modes.stdout_tty {
        serde_json::to_string_pretty(doc)?
    } else {
        serde_json::to_string(doc)?
    };
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{text}")?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    /// Color resolves highest first: the option, `NO_COLOR`, `FORCE_COLOR`, the terminal.
    #[test]
    fn color_follows_the_precedence_of_the_guide() {
        assert!(!color(true, &env_of(&[("FORCE_COLOR", "1")]), true));
        assert!(!color(
            false,
            &env_of(&[("NO_COLOR", "1"), ("FORCE_COLOR", "1")]),
            true
        ));
        assert!(color(false, &env_of(&[("NO_COLOR", "")]), true));
        assert!(color(false, &env_of(&[("FORCE_COLOR", "1")]), false));
        assert!(!color(false, &env_of(&[("FORCE_COLOR", "0")]), true));
        assert!(!color(false, &env_of(&[]), false));
    }

    /// Each sentence's backticked `fabric` commands, in order; others are not commands to run.
    #[test]
    fn a_sentence_names_its_recovery_commands() {
        assert_eq!(
            named_commands(
                "job x is not known on this node; run `fabric job resume x --on <node>` or `git status`"
            ),
            vec!["fabric job resume x --on <node>"]
        );
        assert!(named_commands("no command here").is_empty());
    }

    /// A lost answer to a command under a key the CLI made names the key
    /// and offers the same command under it, so the retry cannot make a
    /// second job; any other failure under such a key carries it too.
    #[test]
    fn a_failure_under_a_made_key_names_the_key_and_the_safe_retry() {
        let run = Run {
            modes: Modes::resolve(Globals::default(), &env_of(&[])),
            argv: [
                "fabric",
                "managed",
                "create",
                "--repo",
                "r",
                "--goal",
                "make it louder",
            ]
            .map(str::to_owned)
            .to_vec(),
            takes: Vec::new(),
        };
        let lost = anyhow::Error::from(Keyed {
            key: "01JABCDEFGHJKMNPQRSTVWXYZ0".to_owned(),
            error: ClientError::ClosedBeforeAnswer {
                op: fabric_client::api::Op::ManagedCreate,
            }
            .into(),
        });
        let doc = run.failure(&lost).document();
        assert_eq!(doc["reason"], "unknown_outcome");
        assert_eq!(doc["retryable"], false);
        assert_eq!(doc["data"]["operation_key"], "01JABCDEFGHJKMNPQRSTVWXYZ0");
        assert_eq!(
            doc["next"][0]["command"],
            "fabric managed create --repo r --goal 'make it louder' --operation-key 01JABCDEFGHJKMNPQRSTVWXYZ0"
        );

        let refused = anyhow::Error::from(Keyed {
            key: "k1".to_owned(),
            error: Usage("bad".to_owned()).into(),
        });
        let doc = run.failure(&refused).document();
        assert_eq!(doc["reason"], "invalid_arguments");
        assert_eq!(doc["data"]["operation_key"], "k1");
        assert_eq!(doc["next"][0]["command"], "fabric managed create --help");
    }

    #[test]
    fn a_word_is_quoted_only_when_a_shell_needs_it() {
        assert_eq!(shell_word("--force"), "--force");
        assert_eq!(shell_word("<JOB>"), "<JOB>");
        assert_eq!(shell_word("make it louder"), "'make it louder'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
    }
}
