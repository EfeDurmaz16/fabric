//! M2-02 (issue #16): a push of an ACP job checks the target for the job's
//! agent before the job changes hands, as a pull does.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::world::{Env, Fallible, Node, Wire, pair, text};

/// The directory of programs only the node at `state` finds, as
/// `world::node_bin` places it.
fn node_bin(state: &Path) -> PathBuf {
    let mut name = state.file_name().unwrap_or_default().to_owned();
    name.push(".bin");
    state.with_file_name(name)
}

/// Puts the fake ACP agent on `node`'s own `PATH` as `program` and gives
/// it a `state_dir` in `node`'s `harness.toml`.
fn agent_only_on(node: &Node, program: &str) -> Fallible {
    std::os::unix::fs::symlink(
        env!("CARGO_BIN_EXE_fabric-fake-agent"),
        node_bin(node.state()).join(program),
    )?;
    Ok(())
}

/// Gives `program` the state directory `.fake-agent` in `node`'s `harness.toml`.
fn state_dir_for(node: &Node, program: &str) -> Fallible {
    let mut config = fs::OpenOptions::new()
        .append(true)
        .open(node.state().join("harness.toml"))?;
    write!(config, "\n[acp.{program}]\nstate_dir = \".fake-agent\"\n")?;
    Ok(())
}

/// Both nodes give the agent a `state_dir`, but only the source finds it
/// on its `PATH`. The push is refused, `--force` does not waive it, and the
/// source stays the owner at epoch 1.
#[test]
fn e2e_a_push_refuses_a_target_without_the_jobs_acp_agent() -> Fallible {
    const ONLY_ON_A: &str = "fabric-agent-on-a";
    let env = Env::new()?;
    let mut a = Node::new(&env, "a", Wire::Quic)?;
    let mut b = Node::new(&env, "b", Wire::Quic)?;
    agent_only_on(&a, ONLY_ON_A)?;
    for node in [&a, &b] {
        state_dir_for(node, ONLY_ON_A)?;
    }
    a.start(None)?;
    b.start(None)?;
    pair(&a, &b)?;
    let job = crate::new_job(&env, &a, &format!("acp:{ONLY_ON_A}"))?;
    let refused = a.run(&["job", "move", &job, "--to", &b.id, "--force"])?;
    let said = text(&refused);
    assert!(!refused.status.success(), "{said}");
    assert!(
        said.contains(&format!("the target's PATH has no {ONLY_ON_A}")),
        "{said}"
    );
    let lease = a.lease(&job)?;
    assert_eq!(lease.owner, a.id, "{said}");
    assert_eq!((lease.epoch, lease.view.as_str()), (1, "owner"), "{said}");
    Ok(())
}
