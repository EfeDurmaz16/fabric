//! `fabric job run --headless` and `fabric job resume --headless`: one
//! turn without a terminal, the answer and the session printed, the run
//! recorded as an interactive one is, and the session found by the next
//! capture. The harnesses are fakes that record their arguments and
//! environment and keep their session files where Claude Code and Codex
//! keep theirs.

use std::fs;
use std::path::Path;

use crate::world::{Env, Fallible, Node, Wire};

/// A parent-session marker the CLI runs under, which no harness may inherit (ADR 0015).
const MARKER: (&str, &str) = ("CODEX_THREAD_ID", "a-parent-thread");

/// Runs `fabric args...` on `node` as if typed inside a Codex session.
fn run_marked(node: &Node, args: &[&str]) -> Fallible<String> {
    node.ok_with(args, &[(MARKER.0, Path::new(MARKER.1))])
}

/// A started node with a job under `harness`.
fn job_under(env: &std::sync::Arc<Env>, harness: &str) -> Fallible<(Node, String)> {
    let mut node = Node::new(env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let job = crate::new_job(env, &node, harness)?;
    Ok((node, job))
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// The harness ran with the user's environment, without the marker.
fn assert_env(env: &Env, name: &str) -> Fallible {
    let names = env.fake_env_names(name)?;
    assert!(names.iter().any(|n| n == "HOME"), "{names:?}");
    assert!(!names.iter().any(|n| n == MARKER.0), "{names:?}");
    Ok(())
}

/// A headless Codex run starts `codex exec`, records the thread Codex
/// names in its first event while it runs, prints the answer and the
/// thread, and a suspend captures the rollout; a headless resume continues
/// that thread with `codex exec resume`.
#[test]
fn e2e_a_headless_codex_turn_records_its_thread_for_the_next_capture() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (node, job) = job_under(&env, "codex")?;
    let thread = Env::CODEX_THREAD;
    let root = env.work(&job);
    let trust = format!("projects.\"{}\".trust_level=trusted", root.display());

    let ran = run_marked(
        &node,
        &["job", "run", &job, "--headless", "--prompt", "add a test"],
    )?;
    let printed = lines(&ran);
    assert!(printed.contains(&"codex did: add a test"), "{ran}");
    assert!(
        printed.contains(&format!("codex session {thread}").as_str()),
        "{ran}"
    );
    assert!(
        printed.contains(&format!("recorded codex session {thread}").as_str()),
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

    let resumed = run_marked(
        &node,
        &["job", "resume", &job, "--headless", "--prompt", "next"],
    )?;
    assert!(lines(&resumed).contains(&"codex did: next"), "{resumed}");
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

    let ran = run_marked(
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
    let printed = lines(&ran);
    assert!(printed.contains(&"claude did: one"), "{ran}");
    assert!(
        printed.contains(&format!("claude-code session {session}").as_str()),
        "{ran}"
    );
    assert_env(&env, "claude")?;

    let suspended = node.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured claude-code session {session}")),
        "{suspended}"
    );

    let resumed = run_marked(
        &node,
        &["job", "resume", &job, "--headless", "--prompt", "two"],
    )?;
    assert!(lines(&resumed).contains(&"claude did: two"), "{resumed}");
    let args = env.fake_args("claude")?;
    assert_eq!(
        args,
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

/// A headless run needs the prompt it starts its turn with.
#[test]
fn e2e_a_headless_run_needs_a_prompt() -> Fallible {
    let env = Env::new()?;
    let node = Node::new(&env, "a", Wire::TcpTls)?;
    let refused = node.run(&["job", "run", "01JAAAAAAAAAAAAAAAAAAAAAAA", "--headless"])?;
    assert_eq!(
        refused.status.code(),
        Some(2),
        "{}",
        crate::world::text(&refused)
    );
    Ok(())
}
