//! `fabric job run --headless` and `fabric job resume --headless`: one
//! turn without a terminal, the answer and the session printed on
//! standard output and nothing else there, the run recorded as an
//! interactive one is, and the session found by the next capture. The
//! harnesses are fakes that record their arguments and environment and
//! keep their session files where Claude Code and Codex keep theirs.

use std::fs;
use std::io::{BufRead, Read};
use std::path::Path;
use std::time::Duration;

use crate::world::{Env, Fallible, Node, Wire, finish, text, wait_for};

/// A parent-session marker the CLI runs under, which no harness may inherit (ADR 0015).
const MARKER: (&str, &str) = ("CODEX_THREAD_ID", "a-parent-thread");

/// Runs `fabric args...` on `node` as if typed inside a Codex session, and
/// returns its standard output, which a headless run keeps for the answer
/// and the session line, and everything it printed.
fn run_marked(node: &Node, args: &[&str]) -> Fallible<(String, String)> {
    let child = node.spawn_with(args, &[(MARKER.0, Path::new(MARKER.1))])?;
    let output = finish(child, Duration::from_mins(3))?;
    let all = text(&output);
    if !output.status.success() {
        return Err(format!("`fabric {}` failed:\n{all}\n{}", args.join(" "), node.log()).into());
    }
    Ok((String::from_utf8(output.stdout)?, all))
}

/// A started node with a job under `harness`.
fn job_under(env: &std::sync::Arc<Env>, harness: &str) -> Fallible<(Node, String)> {
    let mut node = Node::new(env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let job = crate::new_job(env, &node, harness)?;
    Ok((node, job))
}

/// The harness ran with the user's environment, without the marker.
fn assert_env(env: &Env, name: &str) -> Fallible {
    let names = env.fake_env_names(name)?;
    assert!(names.iter().any(|n| n == "HOME"), "{names:?}");
    assert!(!names.iter().any(|n| n == MARKER.0), "{names:?}");
    Ok(())
}

/// A headless Codex run starts `codex exec`, records the thread Codex
/// names in its first event while the turn still runs, prints the answer
/// and the thread, and a suspend captures the rollout; a headless resume
/// continues that thread with `codex exec resume`, passing the model,
/// effort and sandbox the thread ran with, and says so, and says that it
/// left the approval policy to `codex exec` (gate finding F10a).
#[test]
fn e2e_a_headless_codex_turn_records_its_thread_for_the_next_capture() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let thread = Env::CODEX_THREAD;
    let root = env.work(&job);
    let trust = format!("projects.\"{}\".trust_level=trusted", root.display());

    let (out, ran) = run_marked(
        &node,
        &["job", "run", &job, "--headless", "--prompt", "add a test"],
    )?;
    assert_eq!(
        out,
        format!("codex did: add a test\ncodex session {thread}\n"),
        "standard output holds only the answer and the session:\n{ran}"
    );
    assert!(
        ran.contains(&format!("recorded codex session {thread}")),
        "{ran}"
    );
    assert_eq!(
        fs::read_to_string(env.home.join("codex-saw-session"))?,
        "yes\n",
        "the thread was not recorded while the turn ran:\n{ran}"
    );
    assert_eq!(
        env.fake_args("codex")?,
        ["exec", "--json", "-c", &trust, "--", "add a test"]
    );
    assert_env(&env, "codex")?;

    let suspended = node.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured codex session {thread} (1 files)")),
        "{suspended}"
    );

    let (out, resumed) = run_marked(
        &node,
        &["job", "resume", &job, "--headless", "--prompt", "next"],
    )?;
    assert_eq!(
        out,
        format!("codex did: next\ncodex session {thread}\n"),
        "{resumed}"
    );
    assert_eq!(
        env.fake_args("codex")?,
        [
            "exec",
            "resume",
            thread,
            "--json",
            "-c",
            &trust,
            "-c",
            "model=\"gpt-6-sol\"",
            "-c",
            "model_reasoning_effort=\"xhigh\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "--",
            "next"
        ]
    );
    assert!(
        resumed.contains(&format!(
            "continuing codex session {thread} with model gpt-6-sol, effort xhigh, sandbox workspace-write (recorded in the session)\napproval on-request is not carried (codex exec sets it: never, or this node's own under auto review); codex uses its own setting\n"
        )),
        "{resumed}"
    );
    assert_eq!(
        resumed.matches("continuing codex session").count(),
        1,
        "the node names no continuing line of its own:\n{resumed}"
    );
    let log = node.ok(&["job", "log", &job])?;
    assert_eq!(
        log.matches("Resumed").count(),
        2,
        "both headless turns are recorded as runs:\n{log}"
    );
    Ok(())
}

/// Codex records its version only in the `session_meta` of a rollout it
/// creates, never on a resume, so the newest Codex that continued a thread
/// travels with the job instead: a thread made by 0.155.1 on one node and
/// continued by 0.160.0 on the other is refused when 0.155.1 would
/// continue it again, and that Codex never starts, even after a suspend
/// on its own node captures the thread again (gate finding F10b).
#[test]
fn e2e_an_older_codex_is_refused_a_thread_a_newer_one_continued() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    p.env.install_fake_codex()?;
    fs::write(p.b.state().join("codex-version"), "0.160.0\n")?;
    let job = crate::new_job(&p.env, &p.a, "codex")?;
    let thread = Env::CODEX_THREAD;
    p.a.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    p.a.ok(&["job", "suspend", &job])?;
    // The doctor row refuses differing Codex versions; the move waives it.
    p.a.ok(&["job", "move", &job, "--to", &p.b.id, "--force"])?;
    p.b.ok(&["job", "resume", &job, "--headless", "--prompt", "two"])?;
    let rollout = p.env.home.join(format!(
        ".codex/sessions/2026/10/21/rollout-2026-10-21T10-00-00-{thread}.jsonl"
    ));
    let continued = fs::read_to_string(&rollout)?;
    assert!(
        continued.contains("\"cli_version\":\"0.155.1\"")
            && !continued.contains("0.160.0")
            && continued.contains("\"text\":\"two\""),
        "the rollout names only the codex that created it:\n{continued}"
    );

    fs::write(p.env.home.join("codex-args"), "")?;
    let back = p.a.run(&[
        "job",
        "resume",
        &job,
        "--force",
        "--headless",
        "--prompt",
        "three",
    ])?;
    let said = text(&back);
    assert!(!back.status.success(), "{said}");
    assert!(
        said.contains(&format!(
            "codex 0.155.1 is older than codex 0.160.0, which wrote session {thread}"
        )),
        "{said}"
    );
    assert_eq!(
        fs::read_to_string(&rollout)?,
        continued,
        "the older codex wrote"
    );
    let args = fs::read_to_string(p.env.home.join("codex-args"))?;
    assert!(!args.contains("exec"), "the older codex started: {args}");

    // A capture on the node that was refused records the newer version,
    // not the older one installed there, so the snapshot carries it to the
    // next node; and running again there does not get past the guard.
    p.a.ok(&["job", "suspend", &job])?;
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
        p.a.state()
            .join("jobs")
            .join(&job)
            .join("harness-state/manifest.json"),
    )?)?;
    assert_eq!(
        manifest["harness_version"], "0.160.0",
        "the capture lowered the known writer"
    );
    let again =
        p.a.run(&["job", "run", &job, "--headless", "--prompt", "four"])?;
    let said = text(&again);
    assert!(!again.status.success(), "{said}");
    assert!(said.contains("older than codex 0.160.0"), "{said}");
    assert_eq!(
        fs::read_to_string(&rollout)?,
        continued,
        "the older codex wrote"
    );
    Ok(())
}

/// A pull that restores a Codex thread names it as soon as the
/// workspace is written (`session_to_continue`), and the CLI starts its
/// resume guard's `codex --version` then, while the node still places the
/// thread, instead of after the answer (item 15 of
/// docs/evidence/perf-ab/README.md). The node's `lsof` of that placement
/// waits up to five seconds for the guard's `--version`, run in the
/// workspace, to start, and logs whether it did.
#[test]
fn e2e_a_pull_asks_the_guards_version_while_the_node_places_the_session() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    p.env.install_fake_codex()?;
    let job = crate::new_job(&p.env, &p.a, "codex")?;
    p.a.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    p.a.ok(&["job", "suspend", &job])?;
    let home = &p.env.home;
    let (asked, log) = (home.join("guard-asked"), home.join("lsof-log"));
    // The guard's probe is the one run in the workspace.
    let codex = fs::read_to_string(p.env.bin().join("codex"))?.replacen(
        "if [ \"$1\" = --version ]; then\n",
        &format!(
            "if [ \"$1\" = --version ]; then\n[ \"$(basename \"$(pwd -P)\")\" != work ] || touch '{}'\n",
            asked.display()
        ),
        1,
    );
    p.env.install("codex", &codex)?;
    p.env.install(
        "lsof",
        &format!(
            "#!/bin/sh\n\
             i=0\n\
             while [ ! -f '{asked}' ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i + 1)); done\n\
             if [ -f '{asked}' ]; then echo together >> '{log}'; else echo alone >> '{log}'; fi\n\
             for real in /usr/sbin/lsof /usr/bin/lsof /sbin/lsof /bin/lsof; do\n\
               [ ! -x \"$real\" ] || exec \"$real\" \"$@\"\n\
             done\n\
             exit 1\n",
            asked = asked.display(),
            log = log.display()
        ),
    )?;
    p.b.ok(&[
        "job",
        "resume",
        &job,
        "--on",
        &p.a.id,
        "--force",
        "--headless",
        "--prompt",
        "two",
    ])?;
    let logged = fs::read_to_string(&log)?;
    assert!(logged.starts_with("together"), "{logged}");
    Ok(())
}

/// A version manager pins a newer Codex in the workspace, and it runs the
/// thread there, while the node's own version check, run from the
/// daemon, finds the global one. That newer Codex writes no version into
/// the rollout, so the node remembers the one the launch started: once
/// the pin is gone, the older global Codex is refused the thread, and a
/// suspend records the newer version for the next node (gate finding
/// F10b).
#[test]
fn e2e_a_codex_the_workspace_pinned_is_remembered_after_it_ran() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let thread = Env::CODEX_THREAD;
    let pin = env.work(&job).join(".tool-versions");
    node.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    fs::write(&pin, "codex 0.160.0\n")?;
    node.ok(&["job", "run", &job, "--headless", "--prompt", "two"])?;
    let rollout = env.home.join(format!(
        ".codex/sessions/2026/10/21/rollout-2026-10-21T10-00-00-{thread}.jsonl"
    ));
    let continued = fs::read_to_string(&rollout)?;
    assert!(
        continued.contains("\"cli_version\":\"0.155.1\"")
            && !continued.contains("0.160.0")
            && continued.contains("\"text\":\"two\""),
        "the rollout names only the codex that created it:\n{continued}"
    );

    fs::remove_file(&pin)?;
    fs::write(env.home.join("codex-args"), "")?;
    let again = node.run(&["job", "run", &job, "--headless", "--prompt", "three"])?;
    let said = text(&again);
    assert!(!again.status.success(), "{said}");
    assert!(
        said.contains(&format!(
            "codex 0.155.1 is older than codex 0.160.0, which wrote session {thread}"
        )),
        "{said}"
    );
    assert_eq!(
        fs::read_to_string(&rollout)?,
        continued,
        "the older codex wrote"
    );
    let args = fs::read_to_string(env.home.join("codex-args"))?;
    assert!(!args.contains("exec"), "the older codex started: {args}");

    node.ok(&["job", "suspend", &job])?;
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
        node.state()
            .join("jobs")
            .join(&job)
            .join("harness-state/manifest.json"),
    )?)?;
    assert_eq!(
        manifest["harness_version"], "0.160.0",
        "the capture recorded the daemon's codex, not the one that ran"
    );
    Ok(())
}

/// The move preflight compares the target's Codex with the newest one
/// known to have run the thread, not only with the source's: a workspace
/// pin ran the thread with 0.160.0 on the source, whose daemon finds
/// 0.155.1, so a target on 0.158.0, newer than the source's, is named as
/// older than 0.160.0, which its resume guard would refuse (gate finding
/// F10b). The known writer is the owner's, so the same holds for
/// `fabric doctor --peer --job` on the owner, for a pull typed on the
/// target, which holds no session record of the job, and for the doctor
/// on that node, which judges the owner as the target.
#[test]
fn e2e_the_preflight_names_the_codex_a_pinned_workspace_ran_when_the_target_is_older() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    p.env.install_fake_codex()?;
    fs::write(p.b.state().join("codex-version"), "0.158.0\n")?;
    let job = crate::new_job(&p.env, &p.a, "codex")?;
    p.a.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    fs::write(p.env.work(&job).join(".tool-versions"), "codex 0.160.0\n")?;
    p.a.ok(&["job", "run", &job, "--headless", "--prompt", "two"])?;
    let older = "the target's codex is older than codex 0.160.0";
    let refused = p.a.run(&["job", "move", &job, "--to", &p.b.id])?;
    let said = text(&refused);
    assert!(!refused.status.success(), "{said}");
    assert!(said.contains(older), "{said}");

    let doctor = text(&p.a.run(&["doctor", "--peer", &p.b.id, "--job", &job])?);
    assert!(doctor.contains(older), "{doctor}");

    let pulled = p.b.run(&["job", "resume", &job, "--on", &p.a.id])?;
    let said = text(&pulled);
    assert!(!pulled.status.success(), "{said}");
    assert!(said.contains(older), "{said}");

    let doctor = text(&p.b.run(&["doctor", "--peer", &p.a.id, "--job", &job])?);
    assert!(doctor.contains(older), "{doctor}");
    Ok(())
}

/// A headless Claude run starts `claude -p --output-format json` with a
/// session id it chose, prints the answer and the session, and a suspend
/// captures the transcript; a headless resume continues it with
/// `--resume` and the model the session ran with.
#[test]
fn e2e_a_headless_claude_turn_prints_its_answer_and_is_captured() -> Fallible {
    let env = Env::new()?;
    env.install_fake_claude()?;
    // An empty session registry answers that no claude writes the session,
    // so a claude running on the test machine does not block the capture.
    fs::create_dir_all(env.home.join(".claude/sessions"))?;
    let (node, job) = job_under(&env, "claude")?;

    let (out, ran) = run_marked(
        &node,
        &["job", "run", &job, "--headless", "--prompt", "one"],
    )?;
    let args = env.fake_args("claude")?;
    let [p, format, json, flag, session, dashes, prompt] = &args[..] else {
        return Err(format!("unexpected arguments {args:?}").into());
    };
    assert_eq!(
        [p, format, json, flag, dashes, prompt],
        ["-p", "--output-format", "json", "--session-id", "--", "one"]
    );
    assert_eq!(
        out,
        format!("claude did: one\nclaude-code session {session}\n"),
        "standard output holds only the answer and the session:\n{ran}"
    );
    assert_env(&env, "claude")?;

    let suspended = node.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured claude-code session {session}")),
        "{suspended}"
    );

    let (out, resumed) = run_marked(
        &node,
        &["job", "resume", &job, "--headless", "--prompt", "two"],
    )?;
    assert_eq!(
        out,
        format!("claude did: two\nclaude-code session {session}\n"),
        "{resumed}"
    );
    assert_eq!(
        env.fake_args("claude")?,
        [
            "-p",
            "--output-format",
            "json",
            "--resume",
            session.as_str(),
            "--model",
            "claude-opus-5-5",
            "--",
            "two"
        ]
    );
    Ok(())
}

/// Ctrl-C on a headless run reaches the harness, which runs in a process
/// group of its own: the turn ends, `fabric` reads it to the end, tells the
/// node the harness exited, and exits with the harness's code.
#[test]
fn e2e_ctrl_c_on_a_headless_run_reaches_the_harness() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let running = node.spawn_with(
        &["job", "run", &job, "--headless", "--prompt", "a long turn"],
        &[("FAKE_CODEX_WAIT", Path::new("1"))],
    )?;
    wait_for(&env.home.join("codex-waiting"), "the fake codex's turn")?;
    let fabric =
        rustix::process::Pid::from_raw(i32::try_from(running.id())?).ok_or("fabric has no pid")?;
    rustix::process::kill_process(fabric, rustix::process::Signal::INT)?;
    let output = finish(running, Duration::from_mins(3))?;
    assert_eq!(output.status.code(), Some(130), "{}", text(&output));
    assert_eq!(
        fs::read_to_string(env.home.join("codex-signal"))?,
        "interrupted\n"
    );
    let status = node.ok(&["job", "status", &job])?;
    assert!(
        !status.contains("harness: running"),
        "the node still records the harness:\n{status}"
    );
    Ok(())
}

/// A signal while the resume guard asks the harness for its version,
/// which can take as long as the probe's timeout, ends the command as one
/// during the earlier setup steps does: the harness never starts, so the
/// turn the user stopped is neither begun nor cut, and the session is
/// untouched.
#[test]
fn e2e_a_signal_before_the_harness_starts_ends_a_headless_run() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    node.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    let rollout = env.home.join(format!(
        ".codex/sessions/2026/10/21/rollout-2026-10-21T10-00-00-{}.jsonl",
        Env::CODEX_THREAD
    ));
    let before = fs::read_to_string(&rollout)?;
    fs::write(env.home.join("codex-args"), "")?;
    let running = node.spawn_with(
        &["job", "run", &job, "--headless", "--prompt", "two"],
        &[("FAKE_CODEX_PROBE_WAIT", Path::new("1"))],
    )?;
    wait_for(&env.home.join("codex-probing"), "the version check")?;
    let fabric =
        rustix::process::Pid::from_raw(i32::try_from(running.id())?).ok_or("fabric has no pid")?;
    rustix::process::kill_process(fabric, rustix::process::Signal::TERM)?;
    let output = finish(running, Duration::from_mins(3))?;
    let said = text(&output);
    assert_eq!(output.status.code(), Some(143), "{said}");
    assert!(
        said.contains("interrupted before the harness started") && !said.contains("continuing"),
        "the harness started:\n{said}"
    );
    let args = fs::read_to_string(env.home.join("codex-args"))?;
    assert!(!args.contains("exec"), "the harness started: {args}");
    assert_eq!(fs::read_to_string(&rollout)?, before, "the session changed");
    Ok(())
}

/// The answer cannot be printed when standard output is gone, as after
/// the hang-up of the terminal that would have shown it; the node still
/// hears that the harness exited and drops its record.
#[test]
fn e2e_a_headless_run_whose_output_is_gone_still_records_the_exit() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let mut running = node.spawn(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    drop(running.stdout.take());
    let output = finish(running, Duration::from_mins(3))?;
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("Broken pipe"),
        "the answer was printed somewhere:\n{}",
        text(&output)
    );
    let record = node.state().join("jobs").join(&job).join("harness");
    assert!(
        !record.exists(),
        "the node was not told the harness exited:\n{}",
        text(&output)
    );
    Ok(())
}

/// A script that reads a headless run's output and acts once the turn is
/// printed, before `fabric` exits, must find the node done with the
/// harness: the node is told the harness exited before any line of the
/// turn is printed (Codex review, 2026-10-22).
#[test]
fn e2e_a_headless_run_tells_the_node_before_it_prints_the_turn() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let record = node.state().join("jobs").join(&job).join("harness");
    let mut running = node.spawn(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    // Standard error is read from the start, so a full pipe never stops
    // `fabric` before it prints the turn.
    let mut stderr = running
        .stderr
        .take()
        .ok_or("the run has no standard error")?;
    let errors = std::thread::spawn(move || {
        let mut errors = String::new();
        let _ = stderr.read_to_string(&mut errors);
        errors
    });
    let stdout = running
        .stdout
        .take()
        .ok_or("the run has no standard output")?;
    let mut lines = std::io::BufReader::new(stdout);
    let mut first = String::new();
    lines.read_line(&mut first)?;
    let recorded = record.exists();
    let mut rest = String::new();
    lines.read_to_string(&mut rest)?;
    let output = finish(running, Duration::from_mins(3))?;
    let errors = errors
        .join()
        .map_err(|_| "the reader of standard error panicked")?;
    assert_eq!(first, "codex did: one\n", "{errors}");
    assert!(
        !recorded,
        "the turn was printed while the node still recorded the harness:\n{errors}"
    );
    assert_eq!(
        rest,
        format!("codex session {}\n", Env::CODEX_THREAD),
        "{errors}"
    );
    assert!(output.status.success(), "{errors}");
    Ok(())
}

/// What a headless harness writes to standard error reaches the user
/// whole, even when a process it started writes it after the turn ended:
/// `fabric` copies it to its end before it exits (Codex review, 2026-10-22).
#[test]
fn e2e_a_headless_run_forwards_standard_error_to_its_end() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let running = node.spawn_with(
        &["job", "run", &job, "--headless", "--prompt", "one"],
        &[("FAKE_CODEX_LATE_STDERR", Path::new("1"))],
    )?;
    let output = finish(running, Duration::from_mins(3))?;
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("codex: late diagnostics"),
        "the harness's last diagnostics were dropped:\n{}",
        text(&output)
    );
    Ok(())
}

/// Ctrl-C on a headless ACP run reaches the agent too, which runs in a
/// process group of its own: here the agent stays alive after its turn,
/// the signal ends it, and `fabric` tells the node before it exits, so no
/// agent runs on that nobody watches (Codex review, 2026-10-22).
#[test]
fn e2e_ctrl_c_on_a_headless_acp_run_reaches_the_agent() -> Fallible {
    let env = Env::new()?;
    let (node, job) = job_under(&env, crate::AGENT)?;
    let pid_file = node.state().join("agent.pid");
    let running = node.spawn_with(
        &["job", "run", &job, "--headless", "--prompt", "two"],
        &[
            ("FAKE_LINGER", Path::new("1")),
            ("FAKE_PID_FILE", &pid_file),
        ],
    )?;
    let started = std::time::Instant::now();
    while !node
        .ok(&["job", "status", &job])?
        .contains("harness: running as pid")
    {
        if started.elapsed() > Duration::from_mins(1) {
            return Err("the agent never registered".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The agent is released only once the node recorded it.
    let agent = crate::world::read_pid(&pid_file)?;
    // The agent writes to the run's standard error too, so the run's end is
    // its exit, not the end of its output, which an agent left running
    // would keep open.
    let mut running = running;
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(running.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = drain(running.stderr.take().map(|pipe| Box::new(pipe) as _));
    let fabric =
        rustix::process::Pid::from_raw(i32::try_from(running.id())?).ok_or("fabric has no pid")?;
    rustix::process::kill_process(fabric, rustix::process::Signal::INT)?;
    let started = std::time::Instant::now();
    let exited = loop {
        if let Some(status) = running.try_wait()? {
            break Some(status);
        }
        if started.elapsed() > Duration::from_mins(1) {
            let _ = running.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let left = crate::world::alive(agent);
    if left && let Some(agent) = rustix::process::Pid::from_raw(agent) {
        let _ = rustix::process::kill_process_group(agent, rustix::process::Signal::KILL);
    }
    let printed = format!(
        "{}{}",
        stdout
            .join()
            .map_err(|_| "the reader of standard output panicked")?,
        stderr
            .join()
            .map_err(|_| "the reader of standard error panicked")?
    );
    let exited = exited.ok_or_else(|| format!("fabric did not exit:\n{printed}"))?;
    assert!(!left, "the agent ran on after fabric exited:\n{printed}");
    assert_eq!(exited.code(), Some(130), "{printed}");
    let status = node.ok(&["job", "status", &job])?;
    assert!(
        !status.contains("harness: running"),
        "the node still records the agent:\n{status}"
    );
    Ok(())
}

/// A headless run needs the prompt it starts its turn with.
#[test]
fn e2e_a_headless_run_needs_a_prompt() -> Fallible {
    let env = Env::new()?;
    let node = Node::new(&env, "a", Wire::TcpTls)?;
    let refused = node.run(&["job", "run", "01JAAAAAAAAAAAAAAAAAAAAAAA", "--headless"])?;
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    Ok(())
}
