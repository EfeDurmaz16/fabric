//! Legacy compatibility of M2 (ADR 0020, Standalone jobs and promotion):
//! a state directory the main branch made before managed jobs existed,
//! at commit 176622e, opens under this binary, lists its job with its
//! lease and its ledger, and the job can still be taken and resumed. The
//! ledger's bytes from before are a prefix of the ledger after, so nothing
//! old was rewritten, and its frozen snapshot restores whole.
//!
//! The fixture is frozen under `crates/fabricd/tests/fixtures/legacy-176622e`.
//! `e2e_make_legacy_fixture` made it, and is kept to show how; it runs only
//! when `FABRIC_LEGACY_FIXTURE_OUT` names an empty directory. The node key
//! is left out of the fixture, so the node that opens it is a new node and
//! the job's owner of record is a node it has never seen.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::world::{Env, Fallible, Node, Wire};

/// The frozen state directory, relative to this crate.
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fabricd/tests/fixtures/legacy-176622e"
);

/// Files of a state directory that belong to one run of the daemon, or are
/// the node's private key, and so are not part of the fixture.
const NOT_FROZEN: [&str; 5] = [
    "node.key",
    "daemon.log",
    "daemon.lock",
    "fabric.sock",
    "client-env",
];

/// Makes the fixture with this binary: one job under the fake ACP agent,
/// one turn, work of every kind in its root, then a suspend.
#[test]
#[ignore = "makes the legacy fixture; run by hand on the commit the fixture names"]
fn e2e_make_legacy_fixture() -> Fallible {
    let Some(out) = std::env::var_os("FABRIC_LEGACY_FIXTURE_OUT") else {
        return Ok(());
    };
    let out = PathBuf::from(out);
    let env = Env::new()?;
    let mut node = Node::new(&env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let job = crate::new_job(&env, &node, crate::AGENT)?;
    node.ok(&["job", "run", &job, "--prompt", "one"])?;
    crate::world::edit(&env, &job)?;
    node.ok(&["job", "approve", &job, "--note", "legacy"])?;
    node.ok(&["job", "suspend", &job])?;
    node.kill()?;
    copy_tree(node.state(), &out.join("state"), &NOT_FROZEN)?;
    copy_tree(&env.home.join(".fake-agent"), &out.join("fake-agent"), &[])?;
    fs::write(out.join("job"), format!("{job}\n"))?;
    Ok(())
}

/// The frozen job's id.
fn fixture_job() -> Fallible<String> {
    Ok(fs::read_to_string(Path::new(FIXTURE).join("job"))?
        .trim()
        .to_owned())
}

/// A node whose state directory starts as a copy of the fixture's.
fn legacy_node(env: &std::sync::Arc<Env>) -> Fallible<Node> {
    let node = Node::new(env, "a", Wire::TcpTls)?;
    copy_tree(&Path::new(FIXTURE).join("state"), node.state(), &[])?;
    fs::set_permissions(node.state(), fs::Permissions::from_mode(0o700))?;
    copy_tree(
        &Path::new(FIXTURE).join("fake-agent"),
        &env.home.join(".fake-agent"),
        &[],
    )?;
    Ok(node)
}

#[test]
fn e2e_a_state_directory_from_main_loads_lists_and_resumes() -> Fallible {
    let env = Env::new()?;
    let job = fixture_job()?;
    let mut node = legacy_node(&env)?;
    let ledger = node.state().join("jobs").join(&job).join("ledger");
    let before = fs::read(&ledger)?;
    node.start(None)?;

    let listed = node.ok(&["job", "ls"])?;
    assert!(listed.contains(&job), "{listed}");
    let lease = node.lease(&job)?;
    assert_eq!(lease.epoch, 1, "{lease:?}");
    assert_ne!(
        lease.owner,
        node.ok(&["node", "id"])?.lines().next().unwrap_or_default()
    );
    let log = node.ok(&["job", "log", &job])?;
    for record in ["created", "suspended", "approved"] {
        assert!(log.to_lowercase().contains(record), "{record} in:\n{log}");
    }
    assert_eq!(fs::read(&ledger)?, before, "reading rewrote the ledger");

    // The owner of record is gone, so a steal takes the job. A resume
    // restores the frozen snapshot whole. The fixture's home is not this
    // test's home, so the job lands in the state directory and the fake
    // agent, whose session is keyed to the root path, is not started
    // there: that refusal is standalone behavior, not a loss of old data.
    node.ok(&[
        "job",
        "steal",
        &job,
        "--force",
        "--reason",
        "legacy fixture",
    ])?;
    assert_eq!(node.lease(&job)?.epoch, 2);
    let said = node.fails(&["job", "resume", &job, "--headless", "--prompt", "two"])?;
    assert!(said.contains("restored snapshot"), "{said}");
    assert!(said.contains("the root must be the same path"), "{said}");
    let after = fs::read(&ledger)?;
    assert!(
        after.len() > before.len() && after.starts_with(&before),
        "the ledger from main is not a prefix of the ledger after"
    );
    let root = node.state().join("jobs").join(&job).join("work");
    assert_eq!(
        fs::read_to_string(root.join("README.md"))?,
        "hello\nunstaged edit\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("scratch/plan.md"))?,
        "ignored but precious\n"
    );
    assert_eq!(fs::read_to_string(root.join("staged.txt"))?, "staged\n");
    assert!(!root.join(".env").exists(), "a secret file travelled");
    Ok(())
}

/// Copies `from` into `to`, leaving out the top-level names in `skip`.
fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> Fallible {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip.iter().any(|s| name == *s) {
            continue;
        }
        let status = Command::new("cp")
            .arg("-pR")
            .arg(entry.path())
            .arg(to.join(&name))
            .status()?;
        if !status.success() {
            return Err(format!("cp {} failed", entry.path().display()).into());
        }
    }
    Ok(())
}
