//! M2-01: a headless ACP run through the CLI prints the agent's answer, as
//! a Claude or Codex run does, and a turn the agent fails ends the run with
//! a failure exit even when the agent itself exits 0.

use std::path::Path;

use crate::world::{Env, Fallible, Node, Wire, finish, text};

/// How long a headless run may take here.
const RUN: std::time::Duration = std::time::Duration::from_mins(2);

/// A node with one job under the fake ACP agent.
fn acp_job(env: &std::sync::Arc<Env>) -> Fallible<(Node, String)> {
    let mut node = Node::new(env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let job = crate::new_job(env, &node, crate::AGENT)?;
    Ok((node, job))
}

/// The agent answers `echo: <prompt>` in an `agent_message_chunk`; the
/// CLI prints it on standard output.
#[test]
fn e2e_a_headless_acp_run_prints_the_answer() -> Fallible {
    let env = Env::new()?;
    let (node, job) = acp_job(&env)?;
    let said = node.ok(&["job", "run", &job, "--headless", "--prompt", "hello"])?;
    assert!(said.contains("echo: hello"), "{said}");
    Ok(())
}

/// The agent answers the prompt with a JSON-RPC error and then exits 0
/// when its stdin closes; the run fails and says why.
#[test]
fn e2e_a_failed_acp_turn_fails_the_run() -> Fallible {
    let env = Env::new()?;
    let (node, job) = acp_job(&env)?;
    let running = node.spawn_with(
        &["job", "run", &job, "--headless", "--prompt", "hello"],
        &[("FAKE_PROMPT_ERROR", Path::new("1"))],
    )?;
    let output = finish(running, RUN)?;
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("fake turn failed"), "{said}");
    Ok(())
}
