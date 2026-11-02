//! The M6 partition proof (ADR 0028): two `fabricd` processes on this host
//! with separate state roots, a real QUIC link between them through the
//! userspace relay of `relay.rs`, and the relay paused and resumed. This
//! is a two-process partition on one host, not two hosts; no firewall and
//! no system setting is touched (docs/evidence/m6).
//!
//! What runs here on this base: the two daemons pair and talk through the
//! relay, the relay drops their datagrams while paused and forwards again
//! after, and the authority keeps taking managed commands while its runner
//! is unreachable. The three attempt-level scenarios need an attempt that
//! runs on the runner node and heartbeats to the authority, which is the
//! M5 executor; they are written against the harness and marked ignored
//! with that reason until the restack on M5.

use std::time::Duration;

use crate::relay::Relay;
use crate::world::{Env, Fallible, Node, Wire};

/// The authority, the runner behind the relay, and the relay.
pub struct Partition {
    pub authority: Node,
    pub runner: Node,
    pub relay: Relay,
}

impl Partition {
    /// Starts both daemons and pairs them so that the runner dials the
    /// authority through the relay. The specification also has the
    /// authority dial the runner directly for the verifier's fetch; that
    /// path waits for the M5 executor, so here every datagram between the
    /// two crosses the relay.
    pub fn start(env: &std::sync::Arc<Env>) -> Fallible<Partition> {
        let mut authority = Node::new(env, "authority", Wire::Quic)?;
        let mut runner = Node::new(env, "runner", Wire::Quic)?;
        authority.fake_node("v22.23.2", "127")?;
        runner.fake_node("v22.23.2", "127")?;
        authority.start(None)?;
        let relay = Relay::start(env, "authority", authority.addr().parse()?)?;
        runner.start(None)?;
        // The runner dials first, through the relay, so its one session
        // with the authority is the relayed one; the authority learns the
        // runner's address from that session, as `pair` does.
        runner.ok(&["node", "add", &authority.id, &relay.addr.to_string()])?;
        authority.ok(&["node", "add", &runner.id])?;
        Ok(Partition {
            authority,
            runner,
            relay,
        })
    }
}

/// The partition primitive itself, on two real daemons: their session runs
/// through the relay, a pause makes the runner's datagrams to the
/// authority disappear while the authority keeps serving its own managed
/// commands, and a resume lets the two talk again.
#[test]
fn two_daemons_talk_through_the_relay_and_lose_each_other_while_it_is_paused() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start(&env)?;
    let repo = env
        .repo
        .to_str()
        .ok_or("the repository path is not UTF-8")?;
    let new_job = |node: &Node| -> Fallible<String> {
        let made = node.ok(&[
            "job",
            "new",
            "--repo",
            repo,
            "--goal",
            "cross the relay",
            "--harness",
            "codex",
        ])?;
        Ok(made
            .lines()
            .next()
            .ok_or("job new printed nothing")?
            .to_owned())
    };
    // The runner's first move to the authority dials through the relay.
    let first = new_job(&p.runner)?;
    p.runner
        .ok(&["job", "move", &first, "--to", &p.authority.id])?;
    let before = p.relay.wait_until(
        Duration::from_secs(10),
        |c| c.udp_forwarded[0] > 0 && c.udp_forwarded[1] > 0,
        "a session through the relay",
    )?;
    // Paused: the runner's datagrams to the authority are dropped, a move
    // cannot complete, and the authority still takes a managed command of
    // its own.
    p.relay.pause()?;
    let second = new_job(&p.runner)?;
    let refused = p
        .runner
        .fails(&["job", "move", &second, "--to", &p.authority.id])?;
    assert!(!refused.is_empty());
    let work = env.work("m6-partition");
    std::fs::create_dir_all(&work)?;
    let created = p.authority.ok(&[
        "managed",
        "create",
        "--repo",
        work.to_str().ok_or("the work path is not UTF-8")?,
        "--goal",
        "prove the partition",
        "--harness",
        "codex",
    ])?;
    assert!(!created.trim().is_empty(), "{created}");
    let paused = p.relay.wait_until(
        Duration::from_secs(10),
        |c| c.dropped() > 0,
        "dropped datagrams while paused",
    )?;
    assert_eq!(
        paused.udp_forwarded, before.udp_forwarded,
        "a datagram crossed the paused relay"
    );
    // Resumed: the move the partition stopped completes on a fresh dial.
    p.relay.resume()?;
    p.runner
        .ok(&["job", "move", &second, "--to", &p.authority.id])?;
    let after = p.relay.wait_until(
        Duration::from_secs(10),
        |c| c.udp_forwarded[0] > paused.udp_forwarded[0],
        "forwarding after resume",
    )?;
    eprintln!(
        "relay counters: {after:?} (forwarded {}, dropped {})",
        after.forwarded(),
        after.dropped()
    );
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}

const BLOCKED: &str = "needs an attempt that runs on the runner node and heartbeats to the authority: the M5 executor, landed at the restack (docs/evidence/m6)";

/// Acknowledged checkpoint; relay paused; past `UNAVAILABLE_AFTER_MS` the
/// authority recovers; relay resumed: the old runner's later writes are
/// refused and recorded, the authority's checkpoint head is unchanged,
/// the new attempt starts from the acknowledged snapshot.
#[test]
#[ignore = "needs the M5 executor on the runner node; see docs/evidence/m6"]
fn relay_paused_runner_alive() -> Fallible {
    Err(BLOCKED.into())
}

/// Acknowledged checkpoint; `SIGSTOP` to the runner's daemon and the
/// attempt's group; the authority recovers; `SIGCONT`: the woken runner's
/// first heartbeat is answered superseded and its group stopped.
#[test]
#[ignore = "needs the M5 executor on the runner node; see docs/evidence/m6"]
fn old_runner_sigstop_then_sigcont() -> Fallible {
    Err(BLOCKED.into())
}

/// Acknowledged checkpoint; `SIGKILL` to the runner's daemon; the authority
/// recovers; the runner's daemon starts again in the same state root,
/// reads its marker, is told superseded and stops the orphan group.
#[test]
#[ignore = "needs the M5 executor on the runner node; see docs/evidence/m6"]
fn old_runner_sigkill_then_restart() -> Fallible {
    Err(BLOCKED.into())
}
