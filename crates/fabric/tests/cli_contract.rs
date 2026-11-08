//! The command-line contract (docs/cli-contract.md), held for every command.
//!
//! Each test walks the real command tree (`fabric::contract::paths`), so a
//! command added later is held to the same rules without a line here: its
//! help must have the layout, its examples must parse, an unknown option
//! must be a usage error with a hint, so must a stray word and an unknown
//! option beside `--version`, a failure under `--json` must be one JSON
//! document on standard error with standard output empty, a key the CLI
//! makes must be in that document, and `NO_COLOR` must take every escape
//! sequence out. `fabric help` is a command of the tree like any other. A
//! command whose
//! required arguments this file has no sample for fails until one is
//! added to `sample`.
//!
//! The binary runs with its own `HOME` and `FABRIC_HOME`, where no node
//! listens, so nothing here reaches a node or the user's files.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fabric::contract;
use fabric_core::id::NodeId;

const JOB: &str = "01JABCDEFGHJKMNPQRSTVWXYZ0";

fn node() -> String {
    NodeId::from_bytes([7; 32]).to_string()
}

/// A directory where no node listens, kept for the test's run.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        // Under /tmp so the control socket's path fits a Unix socket.
        let dir = tempfile::Builder::new()
            .prefix("fcc")
            .tempdir_in("/tmp")
            .unwrap();
        for sub in ["home", "state", "repo"] {
            std::fs::create_dir(dir.path().join(sub)).unwrap();
        }
        let private = |path: PathBuf| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        private(dir.path().join("home"));
        private(dir.path().join("state"));
        Self { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    /// `fabric args...` with no node to answer it and a clean color environment.
    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fabric"));
        command
            .args(args)
            .env("HOME", self.path("home"))
            .env("FABRIC_HOME", self.path("state"))
            .env_remove("NO_COLOR")
            .env_remove("FORCE_COLOR")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("CI")
            .env_remove("FABRIC_LOG")
            .env("TERM", "xterm-256color");
        command
    }

    fn run(&self, args: &[String]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn words(text: &[&str]) -> Vec<String> {
    text.iter().map(|word| (*word).to_owned()).collect()
}

fn joined(path: &[String]) -> String {
    std::iter::once("fabric")
        .chain(path.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// The command at `path` in the real tree.
fn find(path: &[String]) -> clap::Command {
    let mut at = contract::command();
    for name in path {
        at = at
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("no command {name} in {}", joined(path)))
            .clone();
    }
    at
}

fn is_leaf(path: &[String]) -> bool {
    find(path)
        .get_subcommands()
        .all(|sub| sub.get_name() == "help")
}

/// A value for an argument or placeholder named `id`; a new name fails
/// here until it gets one, so every command stays runnable by this file.
fn sample(id: &str, sandbox: &Sandbox) -> String {
    match id {
        "job" | "JOB" => JOB.to_owned(),
        "node" | "to" | "on" | "peer" | "NODE" => node(),
        "repo" | "path" | "PATH" => sandbox.path("repo").display().to_string(),
        "goal" | "prompt" | "reason" | "note" | "TEXT" => "demo".to_owned(),
        "addr" | "HOST:PORT" => "127.0.0.1:7777".to_owned(),
        "KEY" => "retry-1".to_owned(),
        "REVISION" => "3".to_owned(),
        other => panic!("tests/cli_contract.rs has no sample for {other:?}; add one to `sample`"),
    }
}

/// `path` with every positional argument given a sample value, so one
/// more word is one too many; `None` for a command that takes any number.
fn filled(path: &[String], sandbox: &Sandbox) -> Option<Vec<String>> {
    let cmd = find(path);
    let mut args = runnable(path, sandbox);
    for arg in cmd.get_positionals() {
        if arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1)
        {
            return None;
        }
        if !arg.is_required_set() {
            args.push(sample(arg.get_id().as_str(), sandbox));
        }
    }
    Some(args)
}

/// `path` with every required argument given a sample value.
fn runnable(path: &[String], sandbox: &Sandbox) -> Vec<String> {
    let cmd = find(path);
    let mut args = path.to_vec();
    for arg in cmd.get_arguments().filter(|arg| arg.is_required_set()) {
        let id = arg.get_id().as_str();
        if let Some(long) = arg.get_long() {
            args.push(format!("--{long}"));
        }
        args.push(sample(id, sandbox));
    }
    args
}

/// The one line of a JSON failure, parsed; panics unless standard error is exactly that.
fn error_document(output: &Output, what: &str) -> serde_json::Value {
    let err = stderr(output);
    assert_eq!(stdout(output), "", "{what}: stdout is not empty");
    assert_eq!(
        err.lines().count(),
        1,
        "{what}: stderr is not one line:\n{err}"
    );
    let doc: serde_json::Value = serde_json::from_str(err.trim_end())
        .unwrap_or_else(|e| panic!("{what}: stderr is not JSON ({e}):\n{err}"));
    for field in ["status", "reason", "message"] {
        assert!(doc[field].is_string(), "{what}: no {field} in {doc}");
    }
    assert!(
        ["error", "action_required"].contains(&doc["status"].as_str().unwrap()),
        "{what}: {doc}"
    );
    doc
}

/// Help: `--help`, `-h` and a bare group print the same layout on stdout and exit 0.
#[test]
fn every_help_has_the_layout_and_prints_on_stdout() {
    let sandbox = Sandbox::new();
    for path in contract::paths() {
        let name = joined(&path);
        let mut asked = path.clone();
        asked.push("--help".to_owned());
        let output = sandbox.run(&asked);
        let help = stdout(&output);
        assert_eq!(output.status.code(), Some(0), "{name} --help");
        assert_eq!(stderr(&output), "", "{name} --help wrote to stderr");
        let mut lines = help.lines();
        let usage = lines.next().unwrap_or_default();
        assert!(
            usage.starts_with(&format!("Usage: {name}")),
            "{name}: the help opens with {usage:?}"
        );
        let summary = lines.next().unwrap_or_default();
        assert!(
            !summary.is_empty() && !summary.starts_with(' ') && !summary.ends_with('.'),
            "{name}: no one-line summary under the usage: {summary:?}"
        );
        assert!(help.contains("\nLearn more: "), "{name}: no Learn more");
        assert!(!help.contains("ADR "), "{name}: help names an internal ADR");
        if path.is_empty() {
            assert!(
                help.contains("\nGlobal Options:\n"),
                "root lists the globals"
            );
            assert!(!help.contains("\nCommands:\n"), "root groups its commands");
            for sub in contract::command().get_subcommands() {
                if sub.get_name() != "help" && !sub.is_hide_set() {
                    assert!(
                        help.contains(&format!("\n  {} ", sub.get_name())),
                        "root help does not list {}",
                        sub.get_name()
                    );
                }
            }
        } else {
            assert!(
                help.contains("\nGlobal Options: See `fabric --help`\n"),
                "{name}: no pointer to the global options"
            );
        }
        if is_leaf(&path) {
            assert!(help.contains("\nExamples:\n"), "{name}: no examples");
        }

        let mut short = path.clone();
        short.push("-h".to_owned());
        assert_eq!(stdout(&sandbox.run(&short)), help, "{name} -h");
        if !path.is_empty() {
            let mut asked = vec!["help".to_owned()];
            asked.extend(path.iter().cloned());
            let output = sandbox.run(&asked);
            assert_eq!(output.status.code(), Some(0), "fabric help {name}");
            assert_eq!(stdout(&output), help, "fabric help {name}");
        }
        // `fabric help` takes the global options as every command does.
        let mut asked = vec!["help".to_owned()];
        asked.extend(path.iter().cloned());
        asked.extend(words(&["--json", "--no-color"]));
        let output = sandbox.run(&asked);
        assert_eq!(output.status.code(), Some(0), "fabric help {name} --json");
        assert_eq!(stdout(&output), help, "fabric help {name} --json");
        if !is_leaf(&path) {
            let bare = sandbox.run(&path);
            assert_eq!(bare.status.code(), Some(0), "bare {name}");
            assert_eq!(stdout(&bare), help, "bare {name}");
            assert_eq!(stderr(&bare), "", "bare {name}");
            // An option does not turn a group without a command into an error.
            for option in ["--json", "--no-color", "--non-interactive"] {
                let mut args = path.clone();
                args.push(option.to_owned());
                let output = sandbox.run(&args);
                assert_eq!(output.status.code(), Some(0), "{name} {option}");
                assert_eq!(stdout(&output), help, "{name} {option}");
                assert_eq!(stderr(&output), "", "{name} {option}");
            }
        }
    }
}

/// Examples: every command has them, and each parses through the real tree
/// once its placeholders are filled in.
#[test]
fn every_example_parses_and_every_leaf_has_one() {
    let sandbox = Sandbox::new();
    let known: BTreeSet<String> = contract::paths()
        .iter()
        .map(|path| path.join(" "))
        .collect();
    for (at, examples) in contract::EXAMPLES {
        assert!(
            known.contains(*at),
            "examples for a command that does not exist: {at:?}"
        );
        for (what, example) in *examples {
            assert!(!what.is_empty(), "{example}: no description");
            let argv: Vec<String> = example
                .split_whitespace()
                .map(
                    |word| match word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
                        Some(placeholder) => sample(placeholder, &sandbox),
                        None => word.to_owned(),
                    },
                )
                .collect();
            assert_eq!(argv[0], "fabric", "{example}");
            if let Err(e) = contract::command().try_get_matches_from(&argv) {
                panic!("the example {example:?} does not parse:\n{e}");
            }
        }
    }
    for path in contract::paths() {
        if is_leaf(&path) {
            let key = path.join(" ");
            assert!(
                contract::EXAMPLES.iter().any(|(at, _)| *at == key),
                "{} has no examples in contract::EXAMPLES",
                joined(&path)
            );
        }
    }
}

/// An unknown option is a usage error, exit 2, with the command's help as the hint.
#[test]
fn an_unknown_option_is_a_usage_error_with_a_hint() {
    let sandbox = Sandbox::new();
    for path in contract::paths() {
        let name = joined(&path);
        let mut args = path.clone();
        args.push("--no-such-option".to_owned());
        let output = sandbox.run(&args);
        let err = stderr(&output);
        assert_eq!(output.status.code(), Some(2), "{name}: {err}");
        assert_eq!(stdout(&output), "", "{name}");
        assert!(
            err.starts_with(&format!(
                "Error: Unknown option --no-such-option for {name}."
            )),
            "{name}: {err}"
        );
        assert!(
            err.contains(&format!("\n    {name} --help\n")),
            "{name}: {err}"
        );

        args.push("--json".to_owned());
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(2), "{name} --json");
        let doc = error_document(&output, &name);
        assert_eq!(doc["reason"], "unknown_option", "{name}: {doc}");
        assert_eq!(
            doc["next"][0]["command"],
            format!("{name} --help"),
            "{name}: {doc}"
        );
    }
}

/// A word no argument takes is a usage error that calls it an argument, not an option.
#[test]
fn a_stray_word_is_a_usage_error_that_names_it() {
    let sandbox = Sandbox::new();
    for path in contract::paths().into_iter().filter(|path| is_leaf(path)) {
        let name = joined(&path);
        let Some(mut args) = filled(&path, &sandbox) else {
            continue;
        };
        args.extend(words(&["stray", "--json"]));
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(2), "{name} stray");
        let doc = error_document(&output, &name);
        assert_eq!(doc["reason"], "unexpected_argument", "{name}: {doc}");
        assert_eq!(
            doc["message"],
            format!("Unexpected argument stray for {name}."),
            "{name}: {doc}"
        );
    }
}

/// `fabric help` reads no state, so no failure of the state reaches it.
fn does_no_work(path: &[String]) -> bool {
    path == ["help"]
}

/// The commands that run a harness; `--json` there is a usage error,
/// because the harness owns standard output.
fn runs_a_harness(path: &[String]) -> bool {
    matches!(path, [group, leaf] if group == "job" && (leaf == "run" || leaf == "resume"))
}

/// A failure under `--json`: standard output empty, standard error one
/// JSON document; in words, standard output empty and an `Error:` line.
/// Two failures every command can meet: a state directory that is not an
/// absolute path, before any work, and no node to answer.
#[test]
fn a_failure_is_one_document_under_json_and_stdout_stays_empty() {
    let sandbox = Sandbox::new();
    for path in contract::paths().into_iter().filter(|path| is_leaf(path)) {
        let name = joined(&path);
        let mut args = runnable(&path, &sandbox);
        let harness = runs_a_harness(&path);
        // A key the CLI makes for a changing command is in its failure, for a retry.
        let mints_a_key = find(&path)
            .get_arguments()
            .any(|arg| arg.get_id() == "operation_key");

        // A relative state directory fails every command before it does anything.
        args.push("--json".to_owned());
        let output = sandbox
            .command(&args)
            .env("FABRIC_HOME", "state")
            .output()
            .unwrap();
        if does_no_work(&path) {
            assert_eq!(output.status.code(), Some(0), "{name}");
            continue;
        }
        let doc = error_document(&output, &format!("{name} (relative FABRIC_HOME)"));
        if harness {
            assert_eq!(output.status.code(), Some(2), "{name}: {doc}");
            assert_eq!(doc["reason"], "invalid_arguments", "{name}: {doc}");
        } else {
            assert_eq!(output.status.code(), Some(1), "{name}: {doc}");
            assert_eq!(doc["reason"], "failed", "{name}: {doc}");
        }

        // `serve` is the node, so it is the one command no node is missing for.
        if path == ["serve"] {
            continue;
        }
        let output = sandbox.run(&args);
        let doc = error_document(&output, &format!("{name} (no node)"));
        if harness {
            assert_eq!(output.status.code(), Some(2), "{name}: {doc}");
        } else {
            assert_eq!(output.status.code(), Some(80), "{name}: {doc}");
            assert_eq!(doc["reason"], "no_daemon", "{name}: {doc}");
            assert_eq!(doc["next"][0]["command"], "fabric serve", "{name}: {doc}");
        }
        assert_eq!(
            doc["data"]["operation_key"].is_string(),
            mints_a_key,
            "{name}: {doc}"
        );

        args.pop();
        let output = sandbox.run(&args);
        let err = stderr(&output);
        assert_eq!(stdout(&output), "", "{name}: stdout is not empty");
        assert_eq!(output.status.code(), Some(80), "{name}: {err}");
        // Context printed before the failure, such as a fresh operation key, stays above it.
        let failure = err.find("Error: ").map_or("", |at| &err[at..]);
        assert!(
            failure.starts_with("Error: No Fabric node is running for this user.\n")
                && (failure.len() == err.len() || err[..err.len() - failure.len()].ends_with('\n')),
            "{name}: {err}"
        );
        assert!(
            err.contains("\n  Start one:\n    fabric serve\n"),
            "{name}: {err}"
        );
    }
}

/// Color: forced color shows, and `NO_COLOR` or `--no-color` takes every
/// escape sequence out of both streams.
#[test]
fn no_color_removes_every_escape_sequence() {
    let sandbox = Sandbox::new();
    let escapes = |output: &Output| output.stdout.contains(&0x1b) || output.stderr.contains(&0x1b);
    for path in contract::paths().into_iter().filter(|path| is_leaf(path)) {
        if path == ["serve"] {
            continue;
        }
        let name = joined(&path);
        let args = runnable(&path, &sandbox);
        let forced = sandbox
            .command(&args)
            .env("FORCE_COLOR", "1")
            .output()
            .unwrap();
        // Only an error is colored, and `fabric help` does not fail.
        if !does_no_work(&path) {
            assert!(escapes(&forced), "{name}: FORCE_COLOR shows no color");
        }
        let off = sandbox
            .command(&args)
            .env("FORCE_COLOR", "1")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(!escapes(&off), "{name}: NO_COLOR left an escape sequence");
        let mut flagged = args.clone();
        flagged.push("--no-color".to_owned());
        let off = sandbox
            .command(&flagged)
            .env("FORCE_COLOR", "1")
            .output()
            .unwrap();
        assert!(!escapes(&off), "{name}: --no-color left an escape sequence");
        let mut help = path.clone();
        help.push("--help".to_owned());
        let help = sandbox
            .command(&help)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(!escapes(&help), "{name}: help kept color under NO_COLOR");
    }
}

/// `--version` at any depth prints the version alone, and `{ "version" }` with `--json`.
#[test]
fn the_version_prints_alone_at_any_depth() {
    let sandbox = Sandbox::new();
    let version = env!("CARGO_PKG_VERSION");
    for path in contract::paths() {
        let name = joined(&path);
        for flag in ["--version", "-v"] {
            let mut args = path.clone();
            args.push(flag.to_owned());
            let output = sandbox.run(&args);
            assert_eq!(output.status.code(), Some(0), "{name} {flag}");
            assert_eq!(stdout(&output), format!("{version}\n"), "{name} {flag}");
            assert_eq!(stderr(&output), "", "{name} {flag}");
        }
        let mut args = path.clone();
        args.extend(words(&["--version", "--json"]));
        let output = sandbox.run(&args);
        let doc: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(doc, serde_json::json!({ "version": version }), "{name}");

        // `--version` stands in for missing input, never for invalid input.
        let mut args = path.clone();
        args.extend(words(&["--version", "--no-such-option"]));
        let output = sandbox.run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{name} --version --no-such-option"
        );
        assert_eq!(stdout(&output), "", "{name} --version --no-such-option");
    }
}

/// docs/reference/cli.md is the help of every command as it is now.
#[test]
fn the_command_reference_is_up_to_date() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/cli.md");
    let made = contract::reference();
    if std::env::var_os("FABRIC_BLESS").is_some() {
        std::fs::write(&file, &made).unwrap();
    }
    let kept = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        kept == made,
        "docs/reference/cli.md is out of date; run FABRIC_BLESS=1 cargo test -p fabric --test cli_contract"
    );
}
