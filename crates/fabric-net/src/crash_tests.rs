//! Crash tests. A child re-executes this test binary, stops at a named
//! checkpoint inside a file write, and is killed with `SIGKILL`; the parent
//! then asserts what the node key and the peer book promise: no file or a
//! whole file, and a restart that works.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fabric_core::id::NodeId;

use crate::identity::NodeIdentity;
use crate::peers::{PeerBook, Peers};

const A: NodeId = NodeId::from_bytes([1; 32]);
const B: NodeId = NodeId::from_bytes([2; 32]);
const C: NodeId = NodeId::from_bytes([3; 32]);

/// The child side: runs the write named by the environment until the checkpoint stops it.
#[test]
fn crash_child() {
    let Ok(mode) = std::env::var("FABRIC_NET_CRASH_MODE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("FABRIC_NET_CRASH_DIR").unwrap());
    match mode.as_str() {
        "identity" => {
            NodeIdentity::load_or_generate(&dir.join("node.key")).unwrap();
        }
        "peers" => {
            let peers = Peers::open(dir.join("peers")).unwrap();
            peers.update(|book| book.pair(B, None)).unwrap();
        }
        other => panic!("unknown mode {other}"),
    }
    panic!("child did not reach its checkpoint");
}

/// Runs the child until it reaches `checkpoint`, then kills it with SIGKILL.
fn run_child_and_kill(dir: &Path, mode: &str, checkpoint: &str) {
    let signal = dir.join(format!("signal-{checkpoint}"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_tests::crash_child", "--nocapture"])
        .env("FABRIC_NET_CRASH_MODE", mode)
        .env("FABRIC_NET_CRASH_DIR", dir)
        .env("FABRIC_NET_CRASH_AT", checkpoint)
        .env("FABRIC_NET_CRASH_SIGNAL", &signal)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if signal.exists() {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "{checkpoint}: child exited with {status} before the checkpoint\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            started.elapsed() < Duration::from_mins(2),
            "{checkpoint}: child did not reach the checkpoint in time"
        );
        thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "{checkpoint}: child was not killed");
}

#[test]
fn a_key_write_killed_at_any_checkpoint_leaves_no_key_or_a_whole_key() {
    for checkpoint in ["identity.after_tmp", "identity.after_link"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("node.key");
        run_child_and_kill(temp.path(), "identity", checkpoint);

        let linked = checkpoint == "identity.after_link";
        assert_eq!(path.exists(), linked, "{checkpoint}");
        let before = linked.then(|| NodeIdentity::load(&path).unwrap().node_id());
        let restarted = NodeIdentity::load_or_generate(&path)
            .unwrap_or_else(|err| panic!("{checkpoint}: the first restart failed: {err}"));
        if let Some(id) = before {
            assert_eq!(restarted.node_id(), id, "{checkpoint}: the key changed");
        }
        assert_eq!(
            NodeIdentity::load(&path).unwrap().node_id(),
            restarted.node_id(),
            "{checkpoint}"
        );
    }
}

#[test]
fn a_book_write_killed_at_any_checkpoint_leaves_the_old_or_the_new_book() {
    for checkpoint in ["peers.after_tmp", "peers.after_rename"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("peers");
        Peers::open(&path)
            .unwrap()
            .update(|book| book.pair(A, None))
            .unwrap();
        run_child_and_kill(temp.path(), "peers", checkpoint);

        let renamed = checkpoint == "peers.after_rename";
        let book = PeerBook::load(&path).unwrap();
        assert!(book.is_paired(A), "{checkpoint}: the old pairing is kept");
        assert_eq!(book.is_paired(B), renamed, "{checkpoint}");

        let peers = Peers::open(&path).unwrap();
        peers.update(|book| book.pair(C, None)).unwrap();
        let book = PeerBook::load(&path).unwrap();
        assert!(book.is_paired(A) && book.is_paired(C), "{checkpoint}");
        assert_eq!(book.is_paired(B), renamed, "{checkpoint}");
    }
}
