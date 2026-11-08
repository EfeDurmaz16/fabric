//! The command line, held to the same contract as `fabric`
//! (docs/cli-contract.md): standard output carries only the answer, `--json`
//! makes it one document and a failure one JSON line on standard error,
//! every failure has a stable reason and a class of exit code, and every
//! help has the same layout. `tests/cli_contract.rs` walks the command tree
//! against these rules.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{
    Arg, ArgAction, ColorChoice, Command, CommandFactory, FromArgMatches, Parser, Subcommand,
};
use serde_json::{Map, Value, json};

use crate::sync::{Home, PullRequest, PushRequest, SyncReport, pull, push};
use crate::target::{Target, TreeName};
use crate::{NAME, SyncError};

/// Where the commands are described; each help ends with its anchor.
pub const REFERENCE: &str =
    "https://github.com/EfeDurmaz16/fabric/blob/main/docs/guides/sync-alone.md";

const USAGE_EXIT: u8 = 2;
const UNREACHABLE_EXIT: u8 = 83;
const REFUSED_EXIT: u8 = 84;

/// The options every command takes: id, long name, short name and summary.
const GLOBALS: &[(&str, &str, Option<char>, &str)] = &[
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

/// The examples each command's help shows, by command path. A placeholder
/// is `<UPPER_CASE>`; the contract test fills each one in and parses every
/// example through the real command tree.
pub const EXAMPLES: &[(&str, &[(&str, &str)])] = &[
    (
        "push",
        &[
            (
                "Push this tree to a directory on another disk",
                "fabric-sync push /Volumes/<DISK>/sync",
            ),
            (
                "Push a tree to a machine over SSH",
                "fabric-sync push <HOST>:sync --tree <PATH>",
            ),
        ],
    ),
    (
        "pull",
        &[
            (
                "Pull a tree pushed as app into a new directory",
                "fabric-sync pull <HOST>:sync --name app --tree <PATH>",
            ),
            (
                "Pull over a tree with content, keeping the old one beside it",
                "fabric-sync pull <HOST>:sync --tree <PATH> --replace",
            ),
        ],
    ),
    (
        "serve",
        &[(
            "What ssh runs on the other machine",
            "fabric-sync serve <PATH>",
        )],
    ),
    (
        "help",
        &[("Print the help of a command", "fabric-sync help push")],
    ),
];

/// Push and pull a working tree with its uncommitted work and its caches
#[derive(Debug, Parser)]
#[command(name = NAME)]
pub struct Cli {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// Capture a tree and push it to a target
    Push {
        /// Where to: a directory, `host:path` or `ssh://host[:port]/path`
        #[arg(value_name = "TARGET", value_parser = target)]
        target: Target,
        /// The tree to push; the top level when it is a repository
        #[arg(long, value_name = "PATH", default_value = ".")]
        tree: PathBuf,
        /// The tree's name on the target [default: the tree's directory name]
        #[arg(long, value_name = "NAME", value_parser = name)]
        name: Option<TreeName>,
        /// Move the target's head even when this tree has not pulled it
        #[arg(long)]
        force: bool,
    },
    /// Pull a tree from a target onto a directory
    Pull {
        /// Where from: a directory, `host:path` or `ssh://host[:port]/path`
        #[arg(value_name = "TARGET", value_parser = target)]
        target: Target,
        /// The directory to write; absent or empty unless `--replace`
        #[arg(long, value_name = "PATH", default_value = ".")]
        tree: PathBuf,
        /// The tree's name on the target [default: the tree's directory name]
        #[arg(long, value_name = "NAME", value_parser = name)]
        name: Option<TreeName>,
        /// Replace a tree with content; the old one is kept beside it
        #[arg(long)]
        replace: bool,
    },
    /// Answer a sync on stdin and stdout; ssh runs this on the target
    Serve {
        /// The target directory there; a leading ~/ is the home there
        #[arg(value_name = "PATH")]
        path: String,
    },
}

fn target(text: &str) -> Result<Target, String> {
    Target::parse(text).map_err(|e| e.to_string())
}

fn name(text: &str) -> Result<TreeName, String> {
    TreeName::parse(text).map_err(|e| e.to_string())
}

/// The full command tree with the contract's help layout, examples and
/// global options on every command.
#[must_use]
pub fn command() -> Command {
    decorate(
        Cli::command()
            .disable_version_flag(true)
            .help_expected(true),
        &[],
    )
}

/// Every command path, the root as the empty path, depth first.
#[must_use]
pub fn paths() -> Vec<Vec<String>> {
    fn walk(cmd: &Command, at: &[String], out: &mut Vec<Vec<String>>) {
        out.push(at.to_vec());
        for sub in cmd.get_subcommands() {
            let mut below = at.to_vec();
            below.push(sub.get_name().to_owned());
            walk(sub, &below, out);
        }
    }
    let mut out = Vec::new();
    walk(&command(), &[], &mut out);
    out
}

fn decorate(cmd: Command, path: &[&str]) -> Command {
    let root = path.is_empty();
    let mut cmd = cmd
        .mut_args(|arg| arg.display_order(0))
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .help_template(if root {
            "{usage-heading} {usage}\n{about}\n\nCommands:\n{subcommands}\n\nGlobal Options:\n{options}\n{after-help}"
        } else {
            "{usage-heading} {usage}\n{about}\n\n{all-args}{after-help}"
        })
        .after_help(after_help(path, root));
    cmd = cmd.arg(
        Arg::new("help")
            .short('h')
            .long("help")
            .help("Print help")
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
            .hide(!root);
        if let Some(short) = short {
            arg = arg.short(short);
        }
        cmd = cmd.arg(arg);
    }
    if root {
        cmd = cmd.subcommand(
            Command::new("help")
                .about("Print the help of a command")
                .arg(
                    Arg::new("command")
                        .num_args(0..)
                        .value_name("COMMAND")
                        .help("The command, such as `push`"),
                ),
        );
    }
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for sub in names {
        let mut below: Vec<&str> = path.to_vec();
        below.push(&sub);
        cmd = cmd.mut_subcommand(&sub, |s| decorate(s, &below));
    }
    cmd
}

fn after_help(path: &[&str], root: bool) -> String {
    let key = path.join(" ");
    let mut text = String::new();
    if !root {
        let _ = writeln!(text, "Global Options: See `{NAME} --help`");
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
        format!("#{NAME}-{}", path.join("-"))
    };
    let _ = write!(text, "Learn more: {REFERENCE}{anchor}");
    text
}

/// How one run prints, from the options, the environment and the terminals.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the guide resolves each mode independently of the others"
)]
struct Modes {
    json: bool,
    /// A prompt may appear: none of `--json`, `--non-interactive` and a
    /// nonempty `CI`, and standard input and standard error are terminals.
    interactive: bool,
    rich: bool,
    color: bool,
    help_color: bool,
    stdout_tty: bool,
}

fn env_set(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn color(no_color: bool, detected: bool) -> bool {
    if no_color || env_set("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    match env_set("FORCE_COLOR") {
        Some(v) if v == "0" => false,
        Some(_) => true,
        None => detected,
    }
}

impl Modes {
    fn resolve(seen: &Seen) -> Modes {
        let (json, no_color) = (seen.json, seen.no_color);
        let dumb = env_set("TERM").is_some_and(|t| t == "dumb");
        let ci = env_set("CI").is_some_and(|ci| !ci.is_empty());
        let stderr_tty = std::io::stderr().is_terminal();
        let stdout_tty = std::io::stdout().is_terminal();
        Modes {
            json,
            interactive: !json
                && !seen.non_interactive
                && !ci
                && std::io::stdin().is_terminal()
                && stderr_tty,
            rich: stderr_tty && !dumb,
            color: color(no_color, stderr_tty && !dumb),
            help_color: color(no_color, stdout_tty && !dumb),
            stdout_tty,
        }
    }
}

/// The global flags a lenient parse saw, and the command path it reached.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one global flag, read on its own"
)]
struct Seen {
    json: bool,
    no_color: bool,
    non_interactive: bool,
    version: bool,
    reached: String,
}

fn lenient(cmd: &Command, argv: &[OsString]) -> Seen {
    let parsed = cmd.clone().ignore_errors(true).try_get_matches_from(argv);
    let mut path = vec![NAME.to_owned()];
    let mut seen = Seen::default();
    if let Ok(matches) = &parsed {
        let mut at = Some(matches);
        while let Some(m) = at {
            let flag = |id: &str| m.try_get_one::<bool>(id).ok().flatten() == Some(&true);
            seen.json |= flag("json");
            seen.no_color |= flag("no_color");
            seen.non_interactive |= flag("non_interactive");
            seen.version |= flag("version");
            at = m.subcommand().map(|(name, sub)| {
                path.push(name.to_owned());
                sub
            });
        }
    }
    for arg in argv.iter().skip(1).take_while(|arg| *arg != "--") {
        seen.json |= arg == "--json";
        seen.no_color |= arg == "--no-color";
        seen.non_interactive |= arg == "--non-interactive";
    }
    seen.reached = path.join(" ");
    seen
}

/// Runs the command line and returns its exit code.
#[must_use]
pub fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let mut cmd = command();
    let seen = lenient(&cmd, &argv);
    let modes = Modes::resolve(&seen);
    let (no_color, version, reached) = (seen.no_color, seen.version, seen.reached);
    if no_color || env_set("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        cmd = cmd.color(ColorChoice::Never);
    }
    let matches = match cmd.clone().try_get_matches_from(&argv) {
        Ok(matches) => matches,
        Err(e)
            if version
                && matches!(
                    e.kind(),
                    ErrorKind::MissingRequiredArgument
                        | ErrorKind::MissingSubcommand
                        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                ) =>
        {
            return print_version(modes);
        }
        Err(e) => return clap_exit(&cmd, &e, modes, &reached),
    };
    if version {
        return print_version(modes);
    }
    if let Some(("help", asked)) = matches.subcommand() {
        let path: Vec<String> = asked
            .get_many::<String>("command")
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        return help_of(&cmd, &path, modes);
    }
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => return clap_exit(&cmd, &e, modes, &reached),
    };
    let lossy: Vec<String> = argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    match run(cli, modes) {
        Ok(code) => code,
        Err(e) => {
            let failure = failure(&e, &lossy);
            failure.print(modes);
            ExitCode::from(failure.exit)
        }
    }
}

fn print_version(modes: Modes) -> ExitCode {
    let version = env!("CARGO_PKG_VERSION");
    let text = if modes.json {
        json!({ "version": version }).to_string()
    } else {
        version.to_owned()
    };
    let _ = writeln!(std::io::stdout().lock(), "{text}");
    ExitCode::SUCCESS
}

fn help_of(cmd: &Command, path: &[String], modes: Modes) -> ExitCode {
    let mut argv: Vec<OsString> = vec![NAME.into()];
    argv.extend(path.iter().map(OsString::from));
    argv.push("--help".into());
    let reached = std::iter::once(NAME)
        .chain(path.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    match cmd.clone().try_get_matches_from(argv) {
        Err(e) => clap_exit(cmd, &e, modes, &reached),
        Ok(_) => ExitCode::SUCCESS,
    }
}

fn clap_exit(cmd: &Command, error: &clap::Error, modes: Modes, reached: &str) -> ExitCode {
    match error.kind() {
        ErrorKind::MissingSubcommand => {
            let group: Vec<String> = usage_path(error, reached)
                .split_whitespace()
                .skip(1)
                .map(str::to_owned)
                .collect();
            help_of(cmd, &group, modes)
        }
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let help = error.render();
            let text = if modes.help_color {
                help.ansi().to_string()
            } else {
                help.to_string()
            };
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
            ExitCode::SUCCESS
        }
        ErrorKind::DisplayVersion => print_version(modes),
        _ => {
            usage_failure(error, reached).print(modes);
            ExitCode::from(USAGE_EXIT)
        }
    }
}

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

fn usage_failure(error: &clap::Error, reached: &str) -> Failure {
    let path = usage_path(error, reached);
    let invalid = context_text(error, ContextKind::InvalidArg);
    let (reason, headline, action_required) = match error.kind() {
        ErrorKind::UnknownArgument => match invalid.as_deref() {
            Some(word) if !word.starts_with('-') => (
                "unexpected_argument",
                format!("Unexpected argument {word} for {path}."),
                false,
            ),
            word => (
                "unknown_option",
                format!("Unknown option {} for {path}.", word.unwrap_or("?")),
                false,
            ),
        },
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
        ErrorKind::MissingRequiredArgument => (
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
                .trim_start_matches("error: ")
                .trim_end_matches('.')
                .to_owned();
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
        next: Some(Next::new(
            "See its usage",
            help.split_whitespace().map(str::to_owned).collect(),
            None,
        )),
        data: suggestion.map(|suggestion| json!({ "suggestion": suggestion })),
        retryable: None,
    }
}

/// A recovery command.
struct Next {
    label: String,
    argv: Vec<String>,
    when: Option<String>,
}

impl Next {
    fn new(label: &str, argv: Vec<String>, when: Option<&str>) -> Next {
        Next {
            label: label.to_owned(),
            argv,
            when: when.map(str::to_owned),
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

fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,<>~".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// Text with its control characters escaped, for any text that came from a path or the other side.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A failure in the one shape every command uses.
struct Failure {
    reason: String,
    exit: u8,
    action_required: bool,
    headline: String,
    rows: Vec<(String, String)>,
    next: Option<Next>,
    data: Option<Value>,
    retryable: Option<bool>,
}

impl Failure {
    fn human(&self, modes: Modes) -> String {
        let prefix = match (modes.rich, modes.color) {
            (true, true) => "\x1b[31m\u{2717}\x1b[0m",
            (true, false) => "\u{2717}",
            (false, true) => "\x1b[31mError:\x1b[0m",
            (false, false) => "Error:",
        };
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

    fn document(&self) -> Value {
        let mut doc = Map::new();
        let status = if self.action_required {
            "action_required"
        } else {
            "error"
        };
        doc.insert("status".into(), status.into());
        doc.insert("reason".into(), self.reason.as_str().into());
        doc.insert("message".into(), printable(&self.headline).into());
        if let Some(next) = &self.next {
            let mut entry = Map::new();
            entry.insert("command".into(), next.command().into());
            entry.insert("argv".into(), next.argv.clone().into());
            if let Some(when) = &next.when {
                entry.insert("when".into(), when.as_str().into());
            }
            doc.insert("next".into(), Value::Array(vec![Value::Object(entry)]));
        }
        if let Some(data) = &self.data {
            doc.insert("data".into(), data.clone());
        }
        if let Some(retryable) = self.retryable {
            doc.insert("retryable".into(), retryable.into());
        }
        if self.action_required && self.next.as_ref().is_some_and(|n| n.when.is_some()) {
            doc.insert("userActionRequired".into(), true.into());
        }
        Value::Object(doc)
    }

    fn print(&self, modes: Modes) {
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

/// The same command again with `option` added.
fn again_with(argv: &[String], option: &str) -> Vec<String> {
    let mut argv = argv.to_vec();
    if let Some(first) = argv.first_mut() {
        NAME.clone_into(first);
    }
    if !argv.iter().any(|a| a == option) {
        argv.push(option.to_owned());
    }
    argv.retain(|a| a != "--json");
    argv
}

fn sentence(text: &str) -> String {
    let mut text = text.to_owned();
    if let Some(first) = text.get(..1) {
        let upper = first.to_ascii_uppercase();
        text.replace_range(..1, &upper);
    }
    if !text.ends_with('.') {
        text.push('.');
    }
    text
}

fn failure(error: &SyncError, argv: &[String]) -> Failure {
    let mut failure = Failure {
        reason: "failed".to_owned(),
        exit: 1,
        action_required: false,
        headline: sentence(&error.to_string()),
        rows: Vec::new(),
        next: None,
        data: None,
        retryable: None,
    };
    let refused = |failure: &mut Failure, reason: &str, option: &str, when: &str| {
        reason.clone_into(&mut failure.reason);
        failure.exit = REFUSED_EXIT;
        failure.action_required = true;
        failure.retryable = Some(false);
        failure.next = Some(Next::new(
            &format!("Run it with {option}"),
            again_with(argv, option),
            Some(when),
        ));
    };
    match error {
        SyncError::TargetMoved { head, .. } => {
            failure
                .rows
                .push(("Target head".to_owned(), head.to_string()));
            failure.data = Some(json!({ "head": head.to_string() }));
            refused(
                &mut failure,
                "target_moved",
                "--force",
                "only to drop what the target holds; pull it first to keep it",
            );
        }
        SyncError::OtherTree { job, .. } => {
            failure.data = Some(json!({ "job": job.to_string() }));
            refused(
                &mut failure,
                "other_tree",
                "--force",
                "only to replace the other tree; give --name to keep both",
            );
        }
        SyncError::NotEmpty { path } => {
            failure
                .rows
                .push(("Tree".to_owned(), path.display().to_string()));
            refused(
                &mut failure,
                "target_not_empty",
                "--replace",
                "when the tree may be replaced; it is kept beside the new one",
            );
        }
        SyncError::NothingThere { .. } => "unknown_tree".clone_into(&mut failure.reason),
        SyncError::BadTree { .. } => "bad_tree".clone_into(&mut failure.reason),
        SyncError::NoHome { .. } => "no_home".clone_into(&mut failure.reason),
        SyncError::Unreachable { .. } => {
            "unreachable".clone_into(&mut failure.reason);
            failure.exit = UNREACHABLE_EXIT;
            failure.retryable = Some(true);
        }
        SyncError::Protocol { .. } => "protocol".clone_into(&mut failure.reason),
        SyncError::Refused { .. } => "refused".clone_into(&mut failure.reason),
        SyncError::Capture(fabric_store::capture::CaptureError::GitBusy { .. }) => {
            "workspace_busy".clone_into(&mut failure.reason);
            failure.retryable = Some(true);
        }
        SyncError::Store(fabric_store::store::StoreError::Locked { .. }) => {
            "store_busy".clone_into(&mut failure.reason);
            failure.retryable = Some(true);
        }
        _ => {}
    }
    failure
}

/// The answer of a push or pull as the JSON document carries it.
fn document(report: &SyncReport) -> Value {
    let c = report.counters;
    json!({
        "sync": {
            "name": report.name.as_str(),
            "snapshot": report.snapshot.to_string(),
            "files": report.files,
            "snapshot_bytes": report.snapshot_bytes,
            "snapshots": c.snapshots,
            "objects_needed": c.objects_needed,
            "objects_sent": c.objects_sent,
            "bytes_sent": c.bytes_sent,
            "hit_ratio": c.hit_ratio(),
            "left_behind": report.left_behind,
            "tracked_secrets": report.tracked_secrets,
            "warnings": report.warnings,
        },
        "outcome": report.outcome.as_str(),
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a size shown to a person, to one decimal"
)]
fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / f64::from(1u32 << 20))
}

fn notes(report: &SyncReport, target: &Target) -> String {
    let c = report.counters;
    let verb = report.outcome.as_str();
    let way = if report.outcome == crate::Outcome::Pushed {
        "to"
    } else {
        "from"
    };
    let mut out = format!(
        "{verb} {} {way} {}: {} files, sent {} of {} ({} of {} objects, {:.1}% already there)\n",
        report.name,
        printable(&target.to_string()),
        report.files,
        mib(c.bytes_sent),
        mib(report.snapshot_bytes),
        c.objects_sent,
        c.objects_needed,
        c.hit_ratio() * 100.0
    );
    if !report.left_behind.is_empty() {
        let _ = writeln!(
            out,
            "left behind: {}",
            printable(&report.left_behind.join(", "))
        );
    }
    if !report.tracked_secrets.is_empty() {
        let _ = writeln!(
            out,
            "warning: secret-shaped paths travel in git history: {}",
            printable(&report.tracked_secrets.join(", "))
        );
    }
    for warning in &report.warnings {
        let _ = writeln!(out, "warning: {}", printable(warning));
    }
    out
}

fn answer(report: &SyncReport, target: &Target, modes: Modes) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    if modes.json {
        let doc = document(report);
        let text = if modes.stdout_tty {
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        } else {
            doc.to_string()
        };
        let _ = writeln!(stdout, "{text}");
    } else {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(notes(report, target).as_bytes());
        let _ = writeln!(stdout, "{}", report.snapshot);
    }
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

fn run(cli: Cli, modes: Modes) -> Result<ExitCode, SyncError> {
    match cli.command {
        Sub::Push {
            target,
            tree,
            name,
            force,
        } => {
            let report = push(
                &Home::from_env()?,
                &PushRequest {
                    tree,
                    target: target.clone(),
                    name,
                    force,
                    interactive: modes.interactive,
                },
            )?;
            Ok(answer(&report, &target, modes))
        }
        Sub::Pull {
            target,
            tree,
            name,
            replace,
        } => {
            let report = pull(
                &Home::from_env()?,
                &PullRequest {
                    tree,
                    target: target.clone(),
                    name,
                    replace,
                    interactive: modes.interactive,
                },
            )?;
            Ok(answer(&report, &target, modes))
        }
        Sub::Serve { path } => {
            if modes.json {
                let failure = Failure {
                    reason: "invalid_arguments".to_owned(),
                    exit: USAGE_EXIT,
                    action_required: false,
                    headline: format!(
                        "Can't run {NAME} serve with --json: standard output carries the sync protocol."
                    ),
                    rows: Vec::new(),
                    next: Some(Next::new(
                        "See its usage",
                        vec![NAME.to_owned(), "serve".to_owned(), "--help".to_owned()],
                        None,
                    )),
                    data: None,
                    retryable: None,
                };
                failure.print(modes);
                return Ok(ExitCode::from(USAGE_EXIT));
            }
            crate::wire::serve(&path, std::io::stdin().lock(), std::io::stdout().lock())?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
