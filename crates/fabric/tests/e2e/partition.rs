//! The M6 partition proof (ADR 0028): two `fabricd` processes on this host
//! with separate state roots, a real QUIC link between them through the
//! userspace relay of `relay.rs`, and the relay paused and resumed. This
//! is a two-process partition on one host, not two hosts; no firewall and
//! no system setting is touched (docs/evidence/m6).
//!
//! What runs here on the M5 base: the two daemons pair and talk through
//! the relay, the relay drops their datagrams while paused and forwards
//! again after, the authority keeps taking managed commands while its
//! runner is unreachable, and scenario (4): a job the authority placed on
//! the runner node waits under `block` with its state visible. The three
//! scenarios that need an attempt that runs on the runner node and
//! heartbeats to the authority are written against the harness and
//! marked ignored: M5 places and grants, and no node launches a grant
//! (docs/evidence/m5, "Blocked"; docs/evidence/m6).

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::placement::{answer, bearer};
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
    p.relay.wait_until(
        Duration::from_secs(10),
        |c| c.udp_forwarded[0] > 0 && c.udp_forwarded[1] > 0,
        "a session through the relay",
    )?;
    // Paused: the runner's datagrams to the authority are dropped, a move
    // cannot complete, and the authority still takes a managed command of
    // its own. The baseline is taken after the pause, once the counters
    // stand still, so a datagram forwarded between the last reading and
    // the flag is not mistaken for one that crossed the paused relay.
    p.relay.pause()?;
    let before = p.relay.settled(Duration::from_secs(10))?;
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
        |c| c.dropped() > before.dropped(),
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

/// The facade inspection of `job`, read from `from` through `authority`.
fn inspect(from: &Node, authority: &str, token: &str, job: &str) -> Fallible<Value> {
    let inspection = answer(&from.ok(&[
        "facade",
        "inspect",
        "--authority",
        authority,
        "--token",
        token,
        job,
    ])?)?;
    assert_eq!(inspection["type"], "inspection", "{inspection}");
    Ok(inspection)
}

/// The kinds of every event of `job`, read from `from` through `authority`.
fn event_kinds(from: &Node, authority: &str, token: &str, job: &str) -> Fallible<Vec<Value>> {
    let events = answer(&from.ok(&[
        "facade",
        "watch",
        "--authority",
        authority,
        "--token",
        token,
        job,
        "--wait-ms",
        "0",
    ])?)?;
    assert_eq!(events["type"], "events", "{events}");
    Ok(events["events"]
        .as_array()
        .ok_or("no events")?
        .iter()
        .map(|e| e["event"].clone())
        .collect())
}

/// Polls `check` on the authority's own inspection until it holds.
fn until_inspection(
    p: &Partition,
    token: &str,
    job: &str,
    within: Duration,
    what: &str,
    check: impl Fn(&Value) -> bool,
) -> Fallible<Value> {
    let started = Instant::now();
    loop {
        let inspection = inspect(&p.authority, &p.authority.id, token, job)?;
        if check(&inspection) {
            return Ok(inspection);
        }
        if started.elapsed() > within {
            return Err(format!(
                "{what} not seen within {within:?}: {inspection}\n{}",
                p.authority.log()
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Scenario (4), the `block` policy, on two real daemons through the
/// relay: a job submitted from the runner node and pinned to it is placed
/// there by the authority (M5) and its attempt granted at generation 1.
/// The relay is paused. Past `UNAVAILABLE_AFTER_MS` without a heartbeat
/// the authority records `attempt_unavailable` and the attempt is
/// `unknown`; under `block` it then waits: no supersede, no second grant,
/// no automatic recovery, however long the silence. After the resume the
/// same state is read from the runner node through the facade. The
/// runner node runs nothing for the grant (no node launches a grant on
/// this base, docs/evidence/m6), so its silence is that of a node that
/// holds a grant and does not answer for it.
#[test]
fn block_policy_waits_with_the_state_visible() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start(&env)?;
    let authority = p.authority.id.clone();
    let runner = p.runner.id.clone();
    let token = bearer(
        &p.authority
            .ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?,
    )?;
    let accepted = answer(&p.runner.ok(&[
        "facade",
        "submit",
        "--authority",
        &authority,
        "--token",
        &token,
        "--empty",
        "--goal",
        "wait for an operator",
        "--harness",
        "codex",
        "--key",
        "m6-block",
        "--pin",
        &runner,
    ])?)?;
    assert_eq!(accepted["type"], "accepted", "{accepted}");
    let job = accepted["job"].as_str().ok_or("no job")?.to_owned();
    let placed = until_inspection(
        &p,
        &token,
        &job,
        Duration::from_secs(20),
        "a grant on the runner node",
        |i| i["job"]["attempts"][0]["status"] == "granted",
    )?;
    assert_eq!(
        placed["placements"][0]["chosen"],
        runner.as_str(),
        "{placed}"
    );
    let attempt = placed["job"]["attempts"][0]["attempt"]
        .as_str()
        .ok_or("no attempt")?
        .to_owned();

    p.relay.pause()?;
    let paused_at = Instant::now();
    let unknown = until_inspection(
        &p,
        &token,
        &job,
        Duration::from_secs(40),
        "the attempt unknown",
        |i| i["job"]["attempts"][0]["status"] == "unknown",
    )?;
    eprintln!(
        "unknown {:?} after the pause; the silence counts from the grant, since nothing on the runner node heartbeats",
        paused_at.elapsed()
    );
    // Past the time a restart policy would have acted (the attempt
    // unknown for UNAVAILABLE_AFTER_MS, then a tick), under block nothing
    // more is written.
    std::thread::sleep(Duration::from_secs(14));
    let waited = inspect(&p.authority, &authority, &token, &job)?;
    for state in [&unknown, &waited] {
        let attempts = state["job"]["attempts"].as_array().ok_or("no attempts")?;
        assert_eq!(attempts.len(), 1, "a second grant under block: {state}");
        assert_eq!(attempts[0]["attempt"], attempt.as_str());
        assert_eq!(attempts[0]["status"], "unknown", "{state}");
        assert_eq!(attempts[0]["generation"], "1", "{state}");
        assert_eq!(state["job"]["state"], "active", "{state}");
    }
    let kinds = event_kinds(&p.authority, &authority, &token, &job)?;
    let unavailable: Vec<_> = kinds
        .iter()
        .filter(|e| e["kind"] == "attempt_unavailable")
        .collect();
    assert_eq!(unavailable.len(), 1, "{kinds:?}");
    assert_eq!(unavailable[0]["attempt"], attempt.as_str());
    assert!(unavailable[0]["since_ms"].is_string(), "{kinds:?}");
    for never in ["grant_superseded", "recovery_blocked", "attempt_reachable"] {
        assert!(
            !kinds.iter().any(|e| e["kind"] == never),
            "{never} under block: {kinds:?}"
        );
    }
    assert_eq!(
        kinds
            .iter()
            .filter(|e| e["kind"] == "attempt_granted")
            .count(),
        1,
        "{kinds:?}"
    );
    let counters = p.relay.counters()?;

    // Resumed: the runner node reads the same waiting state through the
    // facade; nothing it says moves the job, since nothing on it heartbeats.
    p.relay.resume()?;
    let from_runner = inspect(&p.runner, &authority, &token, &job)?;
    assert_eq!(
        from_runner["job"]["attempts"][0]["status"], "unknown",
        "{from_runner}"
    );
    assert_eq!(
        from_runner["job"]["attempts"].as_array().map(Vec::len),
        Some(1),
        "{from_runner}"
    );
    let seen = event_kinds(&p.runner, &authority, &token, &job)?;
    assert!(seen.iter().any(|e| e["kind"] == "attempt_unavailable"));
    eprintln!(
        "block: job {job}, attempt {attempt} unknown, {} events, relay while paused {counters:?}",
        seen.len()
    );
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}

const BLOCKED: &str = "needs an attempt that runs on the runner node and heartbeats to the authority: M5 places and grants, and no node launches a grant (docs/evidence/m5 and m6, Blocked)";

/// Acknowledged checkpoint; relay paused; past `UNAVAILABLE_AFTER_MS` the
/// authority recovers; relay resumed: the old runner's later writes are
/// refused and recorded, the authority's checkpoint head is unchanged,
/// the new attempt starts from the acknowledged snapshot.
#[test]
#[ignore = "no node launches a granted attempt on the M5 base; see docs/evidence/m6"]
fn relay_paused_runner_alive() -> Fallible {
    Err(BLOCKED.into())
}

/// Acknowledged checkpoint; `SIGSTOP` to the runner's daemon and the
/// attempt's group; the authority recovers; `SIGCONT`: the woken runner's
/// first heartbeat is answered superseded and its group stopped.
#[test]
#[ignore = "no node launches a granted attempt on the M5 base; see docs/evidence/m6"]
fn old_runner_sigstop_then_sigcont() -> Fallible {
    Err(BLOCKED.into())
}

/// Acknowledged checkpoint; `SIGKILL` to the runner's daemon; the authority
/// recovers; the runner's daemon starts again in the same state root,
/// reads its marker, is told superseded and stops the orphan group.
#[test]
#[ignore = "no node launches a granted attempt on the M5 base; see docs/evidence/m6"]
fn old_runner_sigkill_then_restart() -> Fallible {
    Err(BLOCKED.into())
}
