//! `fabric job run --headless` and `fabric job resume --headless`: one
//! turn without a terminal, the answer and the session printed on
//! standard output and nothing else there, the run recorded as an
//! interactive one is, and the session found by the next capture. The
//! harnesses are fakes that record their arguments and environment and
//! keep their session files where Claude Code and Codex keep theirs.

use std::fs;
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
/// continues that thread with `codex exec resume`.
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
            "exec", "resume", thread, "--json", "-c", &trust, "--", "next"
        ]
    );
    let log = node.ok(&["job", "log", &job])?;
    assert_eq!(
        log.matches("Resumed").count(),
        2,
        "both headless turns are recorded as runs:\n{log}"
    );
    Ok(())
}

/// A headless Claude run starts `claude -p --output-format json` with a
/// session id it chose, prints the answer and the session, and a suspend
/// captures the transcript; a headless resume continues it with `--resume`.
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

/// A headless run needs the prompt it starts its turn with.
#[test]
fn e2e_a_headless_run_needs_a_prompt() -> Fallible {
    let env = Env::new()?;
    let node = Node::new(&env, "a", Wire::TcpTls)?;
    let refused = node.run(&["job", "run", "01JAAAAAAAAAAAAAAAAAAAAAAA", "--headless"])?;
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    Ok(())
}
