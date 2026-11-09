//! The CLI contract's two answer rules against a live node, for every
//! command that answers (docs/cli-contract.md, Streams and JSON): in
//! words a change prints the bare id of what it changed or made on
//! standard output and its sentences on standard error, and under
//! `--json` standard output is one document keyed by the noun, with the
//! outcome of a change, and standard error is empty. `tests/cli_contract.rs`
//! holds every other rule with no node; this file is where a reply's
//! shape on success is held, and a leaf command it neither runs nor
//! names in `ELSEWHERE` fails it.

use std::collections::BTreeSet;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use fabric::contract;
use fabric_core::id::JobId;
use serde_json::Value;

use crate::world::{Env, Fallible, Node, Wire, finish, text};
use crate::{AGENT, setup};

/// The leaf commands whose answer another test holds, and where.
const ELSEWHERE: &[(&str, &str)] = &[
    ("help", "tests/cli_contract.rs: help prints on stdout"),
    ("job run", "headless.rs: the harness owns standard output"),
    (
        "job resume",
        "headless.rs: the harness owns standard output",
    ),
    (
        "job export",
        "main.rs: e2e_a_frozen_owner_exports_its_orphan_and_repairs_the_fork",
    ),
    (
        "job repair",
        "main.rs: e2e_a_frozen_owner_exports_its_orphan_and_repairs_the_fork",
    ),
    // A checkpoint and a continue need an attempt a node runs.
    (
        "managed checkpoint",
        "launch.rs: e2e_a_job_submitted_to_a_runs_on_b_and_continues_on_a_with_its_package",
    ),
    (
        "managed continue",
        "launch.rs: e2e_a_job_submitted_to_a_runs_on_b_and_continues_on_a_with_its_package",
    ),
    (
        "facade checkpoint",
        "placement.rs: e2e_a_submit_runs_on_the_eligible_node_without_naming_it",
    ),
    ("facade continue", "ab/new_paths.rs: the facade continue op"),
    (
        "job import",
        "psf.rs: e2e_a_psf_export_never_carries_a_workspace_secret_and_imports_back",
    ),
    (
        "psf inspect",
        "psf.rs: e2e_a_psf_export_never_carries_a_workspace_secret_and_imports_back",
    ),
    // Both read a journal a scripted managed run filled.
    (
        "inspect",
        "observability.rs: e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret",
    ),
    (
        "audit export",
        "observability.rs: e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret",
    ),
    (
        "psf verdicts",
        "sdk/typescript/test/psf.test.ts: the verdict lines every reader prints",
    ),
];

/// The commands this run asked, by command path, such as `job new`.
#[derive(Default)]
struct Asked(BTreeSet<String>);

impl Asked {
    fn note(&mut self, args: &[&str]) {
        let mut at = contract::command();
        let mut path = Vec::new();
        for arg in args {
            let Some(sub) = at.find_subcommand(arg).cloned() else {
                break;
            };
            path.push(*arg);
            at = sub;
        }
        self.0.insert(path.join(" "));
    }

    /// Runs `args` in words on `node`; returns standard output, which must
    /// be one line, the identifier, for a change.
    fn words(&mut self, node: &Node, args: &[&str]) -> Fallible<String> {
        self.note(args);
        let output = node.run(args)?;
        if !output.status.success() {
            return Err(format!("`fabric {}` failed:\n{}", args.join(" "), text(&output)).into());
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    /// `words`, for a change: standard output must be `id` alone.
    fn change(&mut self, node: &Node, args: &[&str], id: &str) -> Fallible {
        let out = self.words(node, args)?;
        assert_eq!(out, format!("{id}\n"), "`fabric {}`", args.join(" "));
        Ok(())
    }

    /// Runs `args` under `--json` on `node`: standard error empty and
    /// standard output one JSON object on one line.
    fn json(&mut self, node: &Node, args: &[&str]) -> Fallible<Value> {
        self.note(args);
        let mut args = args.to_vec();
        // Before a `--`, after which every word is the task's program.
        let at = args
            .iter()
            .position(|arg| *arg == "--")
            .unwrap_or(args.len());
        args.insert(at, "--json");
        let output = node.run(&args)?;
        let what = format!("`fabric {}`", args.join(" "));
        if !output.status.success() {
            return Err(format!("{what} failed:\n{}", text(&output)).into());
        }
        assert_eq!(String::from_utf8_lossy(&output.stderr), "", "{what}");
        let out = String::from_utf8(output.stdout)?;
        assert_eq!(out.lines().count(), 1, "{what}: not one line:\n{out}");
        let doc: Value = serde_json::from_str(&out)?;
        assert!(doc.is_object(), "{what}: {doc}");
        Ok(doc)
    }

    /// `json`, for a change: the document holds `noun` and `outcome`.
    fn changed(
        &mut self,
        node: &Node,
        args: &[&str],
        noun: &str,
        outcome: &str,
    ) -> Fallible<Value> {
        let doc = self.json(node, args)?;
        assert!(doc[noun].is_object(), "`fabric {}`: {doc}", args.join(" "));
        assert_eq!(
            doc["outcome"],
            outcome,
            "`fabric {}`: {doc}",
            args.join(" ")
        );
        Ok(doc)
    }

    /// `json`, for a read: the document holds `key`.
    fn read(&mut self, node: &Node, args: &[&str], key: &str) -> Fallible<Value> {
        let doc = self.json(node, args)?;
        assert!(
            !doc[key].is_null(),
            "`fabric {}`: no {key} in {doc}",
            args.join(" ")
        );
        assert!(
            doc.get("outcome").is_none(),
            "`fabric {}`: {doc}",
            args.join(" ")
        );
        Ok(doc)
    }
}

fn job_of(text: &str) -> Fallible<String> {
    let line = text.trim_end();
    line.parse::<JobId>()
        .map_err(|e| format!("not a bare job id ({e}): {text:?}"))?;
    Ok(line.to_owned())
}

/// Starts `fabric serve` with `extra` on its own state and returns it once
/// it answers, standard output and standard error kept apart.
fn serve(env: &std::sync::Arc<Env>, name: &'static str, extra: &[&str]) -> Fallible<(Node, Child)> {
    let node = Node::new(env, name, Wire::TcpTls)?;
    let addr = node.addr();
    let mut args = vec!["serve", "--listen", &addr, "--wire", "tcp-tls"];
    args.extend(extra);
    let mut child = env
        .command(env.fabric(), node.state())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    while !node.run(&["node", "id"])?.status.success() {
        if child.try_wait()?.is_some() || started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            return Err(format!(
                "{name} did not start:\n{}",
                text(&finish(child, Duration::from_secs(5))?)
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id())?).ok_or("no pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    Ok((node, child))
}

/// `serve` runs until stopped: in words its node and address are context
/// on standard error and standard output stays empty; under `--json` they
/// are the one document.
fn serve_keeps_its_streams(asked: &mut Asked) -> Fallible {
    asked.note(&["serve"]);
    let env = Env::new()?;
    let (_node, child) = serve(&env, "words", &[])?;
    let output = finish(child, Duration::from_secs(30))?;
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "",
        "serve in words"
    );
    assert!(
        err.lines().any(|line| line.starts_with("listening on ")),
        "{err}"
    );
    let (_node, child) = serve(&env, "json", &["--json"])?;
    let output = finish(child, Duration::from_secs(30))?;
    assert_eq!(String::from_utf8_lossy(&output.stderr), "", "serve --json");
    let doc: Value = serde_json::from_slice(&output.stdout)?;
    assert!(
        doc["node"]["node"].is_string() && doc["node"]["listen"].is_string(),
        "{doc}"
    );
    Ok(())
}

/// The M5 commands on the managed job `managed` of `a`: a change prints
/// the job, a read its data, and a facade message the authority's own JSON
/// on one line.
fn m5_commands_keep_their_streams(
    asked: &mut Asked,
    a: &Node,
    managed: &str,
    repo: &str,
) -> Fallible {
    // The M5 operations: a change prints the job, a read its data.
    let note = ["managed", "note", managed, "--question", "q"];
    asked.change(a, &note, managed)?;
    asked.changed(a, &note, "operation", "recorded")?;
    let status = asked.read(a, &["managed", "status", managed], "job")?;
    let parent = status["job"]["primary_task"]
        .as_str()
        .ok_or("no primary task")?
        .to_owned();
    let register = [
        "managed", "register", managed, "--parent", &parent, "--label", "t", "--class", "test",
        "--", "true",
    ];
    asked.change(a, &register, managed)?;
    let mut keyed = register.to_vec();
    keyed.splice(keyed.len() - 2..keyed.len() - 2, ["--operation-key", "r-1"]);
    asked.changed(a, &keyed, "operation", "registered")?;
    assert!(!asked.words(a, &["managed", "inspect", managed])?.is_empty());
    asked.read(a, &["managed", "inspect", managed], "job")?;
    let imported = asked.words(a, &["managed", "import", repo])?;
    assert_eq!(imported.lines().count(), 1, "{imported}");
    asked.changed(a, &["managed", "import", repo], "snapshot", "imported")?;

    // The facade: a mint prints its token and bearer, a facade message the
    // authority's own JSON on one line.
    let mint = ["facade", "mint", "--actor", "streams", "--ttl-ms", "600000"];
    let minted = asked.words(a, &mint)?;
    let bearer = minted
        .lines()
        .find_map(|line| line.strip_prefix("bearer "))
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or("no bearer")?
        .to_owned();
    let token_id = minted
        .lines()
        .find_map(|line| line.strip_prefix("token "))
        .ok_or("no token")?
        .to_owned();
    asked.changed(a, &mint, "token", "minted")?;
    let to = ["--authority", a.id.as_str(), "--token", bearer.as_str()];
    let facade = |op: &'static str, rest: &[&str]| -> Vec<String> {
        ["facade", op]
            .iter()
            .chain(to.iter())
            .chain(rest.iter())
            .map(|word| (*word).to_owned())
            .collect()
    };
    let submit = facade(
        "submit",
        &[
            "--empty",
            "--goal",
            "g",
            "--harness",
            "codex",
            "--operation-key",
            "f-1",
        ],
    );
    let submit: Vec<&str> = submit.iter().map(String::as_str).collect();
    let accepted: Value = serde_json::from_str(asked.words(a, &submit)?.trim_end())?;
    assert_eq!(accepted["type"], "accepted", "{accepted}");
    let facade_job = accepted["job"].as_str().ok_or("no job")?.to_owned();
    assert_eq!(asked.json(a, &submit)?["type"], "accepted");
    for (op, rest) in [
        ("inspect", vec![facade_job.as_str()]),
        ("watch", vec![facade_job.as_str(), "--wait-ms", "0"]),
        (
            "cancel",
            vec![facade_job.as_str(), "--operation-key", "f-2"],
        ),
    ] {
        let args = facade(op, &rest);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let said: Value = serde_json::from_str(asked.words(a, &args)?.trim_end())?;
        assert!(said["type"].is_string(), "{said}");
        assert_eq!(asked.json(a, &args)?["type"], said["type"]);
    }
    // A revoke prints its sentence on stderr; under `--json` a second one is unchanged.
    let revoke = ["facade", "revoke", token_id.as_str()];
    assert_eq!(asked.words(a, &revoke)?, "");
    asked.changed(a, &revoke, "token", "unchanged")?;

    Ok(())
}

/// `gc`: a dry run is a read keyed by the report's own nouns, with no
/// outcome; `--yes` is a change whose outcome is `removed`.
fn gc_keeps_its_streams(asked: &mut Asked, a: &Node) -> Fallible {
    asked.words(a, &["gc"])?;
    let report = asked.read(a, &["gc"], "residuals")?;
    assert_eq!(report["removing"], false, "{report}");
    let removed = asked.json(a, &["gc", "--yes"])?;
    assert_eq!(removed["outcome"], "removed", "{removed}");
    assert!(removed["residuals"].is_array(), "{removed}");
    Ok(())
}

#[test]
fn e2e_every_answer_keeps_its_streams_in_words_and_as_json() -> Fallible {
    let mut asked = Asked::default();
    serve_keeps_its_streams(&mut asked)?;
    let p = setup(Wire::TcpTls, None, None)?;
    let (a, b) = (&p.a, &p.b);
    let repo = p.env.repo.to_str().ok_or("the repository is not UTF-8")?;

    // The node.
    assert_eq!(asked.words(a, &["node", "id"])?, format!("{}\n", a.id));
    assert_eq!(
        asked.read(a, &["node", "id"], "node")?["node"]["node"],
        a.id
    );
    assert!(asked.words(a, &["node", "ls"])?.contains(&b.id));
    asked.read(a, &["node", "ls"], "peers")?;
    assert!(asked.words(a, &["node", "ping", &b.id])?.contains(&b.id));
    asked.read(a, &["node", "ping", &b.id], "node")?;
    asked.change(a, &["node", "add", &b.id, &b.addr()], &b.id)?;
    asked.changed(a, &["node", "add", &b.id, &b.addr()], "node", "added")?;
    assert!(!asked.words(a, &["doctor"])?.is_empty());
    asked.json(a, &["doctor"])?;

    // Standalone jobs.
    let new = [
        "job",
        "new",
        "--repo",
        repo,
        "--goal",
        "louder",
        "--harness",
        AGENT,
    ];
    let one = job_of(&asked.words(a, &new)?)?;
    let made = asked.changed(a, &new, "job", "created")?;
    let two = made["job"]["job"].as_str().ok_or("no job id")?.to_owned();
    assert!(!asked.words(a, &["job", "ls"])?.is_empty());
    asked.read(a, &["job", "ls"], "jobs")?;
    assert!(!asked.words(a, &["job", "status", &one])?.is_empty());
    asked.read(a, &["job", "status", &one], "job")?;
    assert!(!asked.words(a, &["job", "log", &one])?.is_empty());
    asked.read(a, &["job", "log", &one], "records")?;
    for verdict in ["approve", "reject"] {
        asked.change(a, &["job", verdict, &one, "--note", "n"], &one)?;
        asked.changed(a, &["job", verdict, &one, "--note", "n"], "job", "recorded")?;
    }
    asked.change(a, &["job", "suspend", &one], &one)?;
    asked.changed(a, &["job", "suspend", &one], "job", "suspended")?;
    asked.change(a, &["job", "move", &one, "--to", &b.id], &one)?;
    asked.changed(a, &["job", "move", &two, "--to", &b.id], "job", "moved")?;
    asked.change(a, &["job", "steal", &one, "--force", "--reason", "r"], &one)?;
    asked.changed(
        a,
        &["job", "steal", &two, "--force", "--reason", "r"],
        "job",
        "stolen",
    )?;

    // Managed jobs: the same key again is the same job, unchanged.
    let create = [
        "managed",
        "create",
        "--repo",
        repo,
        "--goal",
        "g",
        "--harness",
        "codex",
    ];
    let keyed: Vec<&str> = create
        .iter()
        .copied()
        .chain(["--operation-key", "c-1"])
        .collect();
    let managed = job_of(&asked.words(a, &keyed)?)?;
    let again = asked.changed(a, &keyed, "operation", "unchanged")?;
    assert_eq!(again["operation"]["job"]["job"], managed, "{again}");
    let fresh = asked.changed(a, &create, "operation", "created")?;
    assert!(fresh["operation"]["operation_key"].is_string(), "{fresh}");
    assert!(!asked.words(a, &["managed", "status", &managed])?.is_empty());
    asked.read(a, &["managed", "status", &managed], "job")?;
    assert!(!asked.words(a, &["managed", "ls"])?.is_empty());
    asked.json(a, &["managed", "ls"])?;
    m5_commands_keep_their_streams(&mut asked, a, &managed, repo)?;

    let cancel = ["managed", "cancel", &managed, "--operation-key", "x-1"];
    asked.change(a, &cancel, &managed)?;
    asked.changed(a, &cancel, "operation", "unchanged")?;

    gc_keeps_its_streams(&mut asked, a)?;

    // Every leaf command is held here or names the test that holds it.
    let elsewhere: BTreeSet<&str> = ELSEWHERE.iter().map(|(path, _)| *path).collect();
    for path in contract::paths() {
        let at = contract::command();
        let mut cmd = &at;
        for name in &path {
            cmd = cmd.find_subcommand(name).ok_or("no such command")?;
        }
        if cmd.get_subcommands().next().is_some() {
            continue;
        }
        let key = path.join(" ");
        assert!(
            asked.0.contains(&key) != elsewhere.contains(key.as_str()),
            "fabric {key}: hold its answer in this test or name where in ELSEWHERE, not both"
        );
    }
    Ok(())
}
