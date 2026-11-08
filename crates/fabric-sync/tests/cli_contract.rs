//! The command-line contract (docs/cli-contract.md), held for every
//! command of `fabric-sync`, as `crates/fabric/tests/cli_contract.rs` holds
//! it for `fabric`. Each test walks the real command tree
//! (`fabric_sync::cli::paths`), so a command added later is held to the same
//! rules without a line here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::process::{Command, Output};

use fabric_sync::NAME;
use fabric_sync::cli;

const BIN: &str = env!("CARGO_BIN_EXE_fabric-sync");

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("home")).unwrap();
        Sandbox { dir }
    }

    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("HOME", self.dir.path().join("home"))
            .env("FABRIC_SYNC_HOME", self.dir.path().join("home/sync"))
            .env_remove("NO_COLOR")
            .env_remove("FORCE_COLOR")
            .env_remove("CI")
            .env("TERM", "xterm-256color");
        command
    }

    fn run(&self, args: &[String]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn words(text: &[&str]) -> Vec<String> {
    text.iter().map(|w| (*w).to_owned()).collect()
}

fn joined(path: &[String]) -> String {
    std::iter::once(NAME)
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

fn find(path: &[String]) -> clap::Command {
    let mut at = cli::command();
    for name in path {
        at = at.find_subcommand(name).unwrap().clone();
    }
    at
}

fn is_leaf(path: &[String]) -> bool {
    !path.is_empty()
        && find(path)
            .get_subcommands()
            .all(|sub| sub.get_name() == "help")
}

/// A value for a placeholder; a new name fails here until it gets one.
fn sample(id: &str, sandbox: &Sandbox) -> String {
    match id {
        "TARGET" => sandbox.dir.path().join("no-target").display().to_string(),
        "PATH" => sandbox.dir.path().join("no-tree").display().to_string(),
        "HOST" => "box".to_owned(),
        "DISK" => "Backup".to_owned(),
        "COMMAND" => "push".to_owned(),
        other => panic!("no sample for {other}"),
    }
}

/// The path with every required argument filled in.
fn runnable(path: &[String], sandbox: &Sandbox) -> Vec<String> {
    let mut args = path.to_vec();
    for arg in find(path).get_arguments() {
        if arg.is_positional() && arg.is_required_set() {
            let name = arg
                .get_value_names()
                .and_then(|names| names.first())
                .map_or_else(|| arg.get_id().to_string(), ToString::to_string);
            args.push(sample(&name, sandbox));
        }
    }
    args
}

fn error_document(output: &Output, what: &str) -> serde_json::Value {
    assert_eq!(stdout(output), "", "{what}: stdout is not empty");
    let err = stderr(output);
    assert_eq!(
        err.lines().count(),
        1,
        "{what}: stderr is not one line: {err}"
    );
    let doc: serde_json::Value = serde_json::from_str(&err).unwrap();
    for field in ["status", "reason", "message"] {
        assert!(doc[field].is_string(), "{what}: no {field} in {doc}");
    }
    doc
}

#[test]
fn every_help_has_the_layout_and_prints_on_stdout() {
    let sandbox = Sandbox::new();
    for path in cli::paths() {
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
            "{name}: {usage:?}"
        );
        let summary = lines.next().unwrap_or_default();
        assert!(
            !summary.is_empty() && !summary.starts_with(' ') && !summary.ends_with('.'),
            "{name}: no one-line summary: {summary:?}"
        );
        assert!(help.contains("\nLearn more: "), "{name}: no Learn more");
        assert!(!help.contains("ADR "), "{name}: help names an internal ADR");
        if path.is_empty() {
            assert!(
                help.contains("\nGlobal Options:\n"),
                "root lists the globals"
            );
        } else {
            assert!(
                help.contains(&format!("\nGlobal Options: See `{NAME} --help`\n")),
                "{name}: no pointer to the global options"
            );
            let anchor = format!("#{NAME}-{}", path.join("-"));
            assert!(help.contains(&anchor), "{name}: no anchor {anchor}");
        }
        if is_leaf(&path) {
            assert!(help.contains("\nExamples:\n"), "{name}: no examples");
        }
        let mut short = path.clone();
        short.push("-h".to_owned());
        assert_eq!(stdout(&sandbox.run(&short)), help, "{name} -h");
        let mut via_help = words(&["help"]);
        via_help.extend(path.iter().cloned());
        assert_eq!(stdout(&sandbox.run(&via_help)), help, "{NAME} help {name}");
        asked.push("--json".to_owned());
        assert_eq!(stdout(&sandbox.run(&asked)), help, "{name} --help --json");
    }
}

#[test]
fn every_example_parses_and_every_leaf_has_one() {
    let sandbox = Sandbox::new();
    for path in cli::paths().into_iter().filter(|p| is_leaf(p)) {
        let key = path.join(" ");
        let (_, examples) = cli::EXAMPLES
            .iter()
            .find(|(at, _)| *at == key)
            .unwrap_or_else(|| panic!("{key} has no examples"));
        for (what, example) in *examples {
            assert!(!what.is_empty(), "{example}: no description");
            let argv: Vec<String> = example
                .split_whitespace()
                .map(|word| {
                    let mut out = word.to_owned();
                    while let (Some(start), Some(end)) = (out.find('<'), out.find('>')) {
                        let filled = sample(&out[start + 1..end], &sandbox);
                        out.replace_range(start..=end, &filled);
                    }
                    out
                })
                .collect();
            assert_eq!(argv[0], NAME, "{example}");
            if let Err(e) = cli::command().try_get_matches_from(&argv) {
                assert!(
                    e.kind() == clap::error::ErrorKind::DisplayHelp,
                    "{example} does not parse: {e}"
                );
            }
        }
    }
}

#[test]
fn an_unknown_option_is_a_usage_error_with_a_hint() {
    let sandbox = Sandbox::new();
    for path in cli::paths() {
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

#[test]
fn a_stray_word_is_a_usage_error_that_names_it() {
    let sandbox = Sandbox::new();
    for path in cli::paths()
        .into_iter()
        .filter(|p| is_leaf(p) && p[0] != "help")
    {
        let name = joined(&path);
        let mut args = runnable(&path, &sandbox);
        args.extend(words(&["stray", "--json"]));
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(2), "{name} stray");
        let doc = error_document(&output, &name);
        assert_eq!(doc["reason"], "unexpected_argument", "{name}: {doc}");
        assert_eq!(
            doc["message"],
            format!("Unexpected argument stray for {name}.")
        );
    }
}

/// A failure every syncing command can meet before any work: a state
/// directory that is not an absolute path. `serve` owns stdout, so `--json`
/// there is a usage error.
#[test]
fn a_failure_is_one_document_under_json_and_stdout_stays_empty() {
    let sandbox = Sandbox::new();
    for path in cli::paths()
        .into_iter()
        .filter(|p| is_leaf(p) && p[0] != "help")
    {
        let name = joined(&path);
        let mut args = runnable(&path, &sandbox);
        args.push("--json".to_owned());
        let output = sandbox
            .command(&args)
            .env("FABRIC_SYNC_HOME", "relative/home")
            .output()
            .unwrap();
        let doc = error_document(&output, &name);
        if path[0] == "serve" {
            assert_eq!(output.status.code(), Some(2), "{name}: {doc}");
            assert_eq!(doc["reason"], "invalid_arguments", "{name}: {doc}");
        } else {
            assert_eq!(output.status.code(), Some(1), "{name}: {doc}");
            assert_eq!(doc["reason"], "no_home", "{name}: {doc}");
        }
        args.pop();
        let output = sandbox
            .command(&args)
            .env("FABRIC_SYNC_HOME", "relative/home")
            .output()
            .unwrap();
        if path[0] != "serve" {
            assert_eq!(stdout(&output), "", "{name}: stdout is not empty");
            assert!(
                stderr(&output).starts_with("Error: "),
                "{name}: {}",
                stderr(&output)
            );
        }
    }
}

#[test]
fn no_color_removes_every_escape_sequence() {
    let sandbox = Sandbox::new();
    let escapes = |output: &Output| output.stderr.contains(&0x1b) || output.stdout.contains(&0x1b);
    for path in cli::paths() {
        let name = joined(&path);
        let mut args = path.clone();
        args.push("--no-such-option".to_owned());
        let forced = sandbox
            .command(&args)
            .env("FORCE_COLOR", "1")
            .output()
            .unwrap();
        assert!(escapes(&forced), "{name}: FORCE_COLOR shows no color");
        let off = sandbox
            .command(&args)
            .env("FORCE_COLOR", "1")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(!escapes(&off), "{name}: NO_COLOR left an escape sequence");
        args.push("--no-color".to_owned());
        let off = sandbox
            .command(&args)
            .env("FORCE_COLOR", "1")
            .output()
            .unwrap();
        assert!(!escapes(&off), "{name}: --no-color left an escape sequence");
    }
}

#[test]
fn version_prints_alone_and_as_json() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&words(&["--version"]));
    assert_eq!(stdout(&output), format!("{}\n", env!("CARGO_PKG_VERSION")));
    let output = sandbox.run(&words(&["push", "--version", "--json"]));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stdout(&output),
        format!("{{\"version\":\"{}\"}}\n", env!("CARGO_PKG_VERSION"))
    );
}
