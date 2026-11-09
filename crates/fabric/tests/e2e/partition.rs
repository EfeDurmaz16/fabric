//! The M6 scenarios (ADR 0028), run for real: two `fabricd` processes on
//! this host with separate state roots, a real QUIC link between them
//! through the userspace relay of `relay.rs`, and a managed job the
//! authority places on the runner node and launches there through the M5
//! launch path. The runner's harness is the fake Codex held with
//! `FAKE_CODEX_HOLD` (one process, no child: `FAKE_CODEX_WAIT`'s escaped
//! child would block every cut, ADR 0023), its heartbeats flow, and a
//! checkpoint is cut on the runner, fetched and acknowledged by the
//! authority before the loss. This is a two-process partition on one
//! host, not two hosts; no firewall and no system setting is touched
//! (docs/evidence/m6).
//!
//! The job needs the secret `M6_SITE`. Only the runner holds it at the
//! first placement, so the M5 placement chooses the runner; the test then
//! provisions it on the authority, so a failover, which never chooses the
//! lost node, has the authority to go to.

use std::time::{Duration, Instant};

use std::ffi::OsString;
use std::sync::Arc;

use fabric_client::Client;
use fabric_client::api::Inspection;
use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::launch::{Report, ReportAnswer};
use fabric_core::managed::{
    AttemptId, ExitOutcome, Generation, GrantToken, HEARTBEAT_INTERVAL_MS, Observation,
    UNAVAILABLE_AFTER_MS,
};
use fabric_net::identity::NodeIdentity;
use fabric_net::quic::QuicTransport;
use fabric_net::rpc::Session;
use fabric_net::tls::Trust;
use fabric_net::transport::Transport;
use fabric_net::wire::{Fenced, RecordBytes, RecordPage, VersionText};
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
        Partition::start_with(env, None)
    }

    /// `start` with the authority's daemon armed with `fault`. Both
    /// daemons hold their harness with `FAKE_CODEX_HOLD`, and only the
    /// runner holds the job's secret.
    pub fn start_with(env: &std::sync::Arc<Env>, fault: Option<&str>) -> Fallible<Partition> {
        let mut authority = Node::new(env, "authority", Wire::Quic)?;
        let mut runner = Node::new(env, "runner", Wire::Quic)?;
        authority.fake_node("v22.23.2", "127")?;
        runner.fake_node("v22.23.2", "127")?;
        for node in [&mut authority, &mut runner] {
            node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
        }
        authority.start(fault)?;
        let relay = Relay::start(env, "authority", authority.addr().parse()?)?;
        runner.start(None)?;
        // After its first start, which narrows the state root.
        provision(&runner)?;
        // The runner dials first, through the relay, so its one session
        // with the authority is the relayed one; the authority learns the
        // runner's address from that session, as `pair` does.
        runner.ok(&["node", "add", &authority.id, &relay.addr.to_string()])?;
        authority.ok(&["node", "add", &runner.id])?;
        // The runner's session through the relay is the one the authority
        // probes, launches and fetches over; a ping opens it.
        runner.ok(&["node", "ping", &authority.id])?;
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

/// The secret the scenarios' job needs, by name.
const SITE: &str = "M6_SITE";

/// Writes the job's secret into `node`'s secret directory, as a user does
/// by hand: read live by the node's resolver, mode 0600.
fn provision(node: &Node) -> Fallible {
    use std::os::unix::fs::PermissionsExt;
    let dir = node.state().join("secrets");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(SITE);
    std::fs::write(&path, b"site\n")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// How long a scenario waits for one step the authority takes by itself:
/// `UNAVAILABLE_AFTER_MS` of silence, as long again of dwell, a tick, a
/// failover probe bound and a launch, with room.
const WITHIN: Duration = Duration::from_mins(1);

/// A job the authority placed on the runner node, running there.
struct Placed {
    client: Client,
    token: String,
    job: JobId,
    first: AttemptId,
}

impl Placed {
    fn job_text(&self) -> String {
        self.job.to_string()
    }
}

/// Creates the job on the authority (`recovery`: `block` or
/// `restart_from_acknowledged`) and waits until its first attempt runs on
/// the runner node.
async fn place_on_runner(p: &Partition, key: &str, recovery: &str) -> Fallible<Placed> {
    let client = Client::new(p.authority.state().join("fabric.sock"));
    let token = bearer(
        &p.authority
            .ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?,
    )?;
    let made = p.authority.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "survive the partition",
        "--harness",
        "codex",
        "--secret",
        SITE,
        "--recovery",
        recovery,
        "--drain-budget",
        "1000",
        "--operation-key",
        key,
    ])?;
    let job: JobId = made.lines().next().ok_or("nothing printed")?.parse()?;
    let running = until(
        p,
        &client,
        job,
        "the first attempt running on the runner",
        |i| i.job.attempts.len() == 1 && i.job.attempts[0].status == "running",
    )
    .await?;
    let chosen = running.placements.first().and_then(|d| d.chosen);
    assert_eq!(
        chosen.map(|n| n.to_string()),
        Some(p.runner.id.clone()),
        "placed on the runner: {running:?}"
    );
    Ok(Placed {
        client,
        token,
        job,
        first: running.job.attempts[0].attempt,
    })
}

/// Cuts the running attempt and waits until the authority fetched,
/// verified and acknowledged the cut.
async fn acknowledged(p: &Partition, placed: &Placed) -> Fallible<Vec<Value>> {
    p.authority.ok(&[
        "managed",
        "checkpoint",
        &placed.job_text(),
        "--operation-key",
        "m6-cut",
    ])?;
    until_event(p, placed, "checkpoint_acknowledged").await
}

/// Polls the authority's inspection of `job` until `check` holds.
async fn until(
    p: &Partition,
    client: &Client,
    job: JobId,
    what: &str,
    check: impl Fn(&Inspection) -> bool,
) -> Fallible<Inspection> {
    let started = Instant::now();
    loop {
        let inspection = client.managed_inspect(job, |_| {}).await?;
        if check(&inspection) {
            return Ok(inspection);
        }
        if started.elapsed() > WITHIN {
            return Err(format!(
                "{what} not seen within {WITHIN:?}: {inspection:?}\nauthority:\n{}\nrunner:\n{}",
                p.authority.log(),
                p.runner.log()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Polls the job's events on the authority until one of `kind` is there;
/// every event then.
async fn until_event(p: &Partition, placed: &Placed, kind: &str) -> Fallible<Vec<Value>> {
    let started = Instant::now();
    loop {
        let kinds = event_kinds(
            &p.authority,
            &p.authority.id,
            &placed.token,
            &placed.job_text(),
        )?;
        if kinds.iter().any(|e| e["kind"] == kind) {
            return Ok(kinds);
        }
        if started.elapsed() > WITHIN {
            return Err(format!(
                "no {kind} within {WITHIN:?}: {kinds:?}\nauthority:\n{}\nrunner:\n{}",
                p.authority.log(),
                p.runner.log()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits until the authority superseded the first attempt and its
/// failover runs on the authority's own node.
async fn recovered_onto_the_authority(p: &Partition, placed: &Placed) -> Fallible<Inspection> {
    let after = until(p, &placed.client, placed.job, "the failover running", |i| {
        i.job.attempts.len() == 2
            && i.job.attempts[0].status == "superseded"
            && i.job.attempts[1].status == "running"
    })
    .await?;
    let failover = after
        .placements
        .iter()
        .find(|d| d.kind == "failover")
        .ok_or_else(|| format!("no failover placement: {after:?}"))?;
    assert_eq!(
        failover.chosen.map(|n| n.to_string()),
        Some(p.authority.id.clone()),
        "{after:?}"
    );
    let lost = failover
        .candidates
        .iter()
        .find(|c| c.node.to_string() == p.runner.id)
        .ok_or_else(|| format!("the runner was not a candidate: {after:?}"))?;
    let verdict = serde_json::to_value(&lost.verdict)?;
    assert!(
        verdict.to_string().contains("excluded"),
        "the lost node was not excluded: {verdict}"
    );
    assert_eq!(
        after.job.generation,
        Generation::new(2).ok_or("generation")?
    );
    Ok(after)
}

/// Heartbeats hold presence while the link is up: past
/// `UNAVAILABLE_AFTER_MS` (and two heartbeats more) after the first
/// attempt was seen running at `since`, it is still `running` and the
/// authority recorded no unavailability. Silence is counted from the last
/// heartbeat, never from the grant (review finding on PR #39).
async fn heartbeats_hold_presence(p: &Partition, placed: &Placed, since: Instant) -> Fallible {
    let past = since
        + Duration::from_millis(UNAVAILABLE_AFTER_MS)
        + Duration::from_millis(2 * HEARTBEAT_INTERVAL_MS);
    if let Some(left) = past.checked_duration_since(Instant::now()) {
        tokio::time::sleep(left).await;
    }
    let held = placed.client.managed_inspect(placed.job, |_| {}).await?;
    assert_eq!(held.job.attempts.len(), 1, "{held:?}");
    assert_eq!(held.job.attempts[0].status, "running", "{held:?}");
    let kinds = event_kinds(
        &p.authority,
        &p.authority.id,
        &placed.token,
        &placed.job_text(),
    )?;
    assert!(
        !kinds.iter().any(|e| e["kind"] == "attempt_unavailable"),
        "unavailable while the link was up: {kinds:?}"
    );
    Ok(())
}

/// The number of the job's attempts that may still run.
fn live(i: &Inspection) -> usize {
    i.job
        .attempts
        .iter()
        .filter(|a| matches!(a.status.as_str(), "granted" | "running" | "unknown"))
        .count()
}

/// Waits until the runner's log says it stopped the attempt on a verdict
/// that ended its grant.
fn runner_obeyed(p: &Partition) -> Fallible {
    let started = Instant::now();
    while !p
        .runner
        .log()
        .contains("the grant ended; stopping the attempt")
    {
        if started.elapsed() > WITHIN {
            return Err(format!("the runner never obeyed:\n{}", p.runner.log()).into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Trusts the authority alone.
#[derive(Debug)]
struct Only(NodeId);

impl Trust for Only {
    fn is_paired(&self, node: NodeId) -> bool {
        node == self.0
    }
}

/// The covered writes the old runner holds, sent under its own node key
/// over QUIC through the relay with the old grant: its attempt's exit (a
/// journal operation) and a ledger append. The runner itself sends
/// neither once its heartbeat is answered with the verdict that ended its
/// grant; these are the writes a runner that has not heard yet would send,
/// on the same link and under the same identity.
async fn old_runner_writes(
    p: &Partition,
    job: JobId,
    attempt: AttemptId,
) -> Fallible<(ReportAnswer, Fenced<fabric_net::wire::Appended>)> {
    let identity = NodeIdentity::load(&p.runner.state().join("node.key"))?;
    let authority: NodeId = p.authority.id.parse()?;
    let transport = QuicTransport::bind(
        &identity,
        Arc::new(Only(authority)),
        ([127, 0, 0, 1], 0).into(),
    )?;
    let connection = transport.connect(authority, p.relay.addr).await?;
    let deadline = Duration::from_secs(10);
    let session = Session::connect(
        Arc::from(connection),
        identity.node_id(),
        VersionText::new("e2e")?,
        deadline,
    )
    .await?;
    let token = GrantToken {
        job,
        attempt,
        generation: Generation::FIRST,
    };
    let report = session
        .report(
            Report {
                token,
                observation: Observation::Exited {
                    attempt,
                    generation: Generation::FIRST,
                    outcome: ExitOutcome::Succeeded,
                },
            },
            deadline,
        )
        .await?;
    let append = session
        .managed_append(
            token,
            RecordPage::new(RecordBytes::new(vec![7; 8])?),
            deadline,
        )
        .await?;
    Ok((report, append))
}

/// The refusals the old runner's writes got, and their records.
async fn assert_refused_and_recorded(p: &Partition, placed: &Placed) -> Fallible {
    let (report, append) = old_runner_writes(p, placed.job, placed.first).await?;
    assert!(
        matches!(
            report,
            ReportAnswer::Refused {
                reason: fabric_core::managed::launch::ReportRefusal::Superseded { .. }
            }
        ),
        "the old runner's exit was taken: {report:?}"
    );
    assert!(
        matches!(append, Fenced::GrantRefused { .. }),
        "the old runner's append was taken: {append:?}"
    );
    let kinds = until_event(p, placed, "covered_write_refused").await?;
    let refused: Vec<_> = kinds
        .iter()
        .filter(|e| e["kind"] == "covered_write_refused")
        .collect();
    assert!(
        refused
            .iter()
            .all(|e| e["attempt"] == placed.first.to_string().as_str()),
        "{refused:?}"
    );
    assert!(!refused.is_empty());
    Ok(())
}

/// The process group of the runner's harness for `attempt`, from the
/// executor's launch record on the runner's node.
fn harness_group(runner: &Node, attempt: AttemptId) -> Fallible<i32> {
    let dir = runner
        .state()
        .join("runs")
        .join("records")
        .join(attempt.to_string());
    for entry in std::fs::read_dir(&dir)? {
        let record: Value = serde_json::from_slice(&std::fs::read(entry?.path())?)?;
        if let Some(pid) = record["pid"].as_u64() {
            return Ok(i32::try_from(pid)?);
        }
    }
    Err(format!("no launched task under {}", dir.display()).into())
}

/// Sends `signal` to process group `group` and to `pid`.
fn signal(pid: u32, group: i32, signal: rustix::process::Signal) -> Fallible {
    let daemon = rustix::process::Pid::from_raw(i32::try_from(pid)?).ok_or("no pid")?;
    let group = rustix::process::Pid::from_raw(group).ok_or("no group")?;
    rustix::process::kill_process_group(group, signal)?;
    rustix::process::kill_process(daemon, signal)?;
    Ok(())
}

/// Whether a process group still has a member.
fn group_alive(group: i32) -> bool {
    rustix::process::Pid::from_raw(group)
        .is_some_and(|g| rustix::process::test_kill_process_group(g).is_ok())
}

/// Scenario (1), the relay paused with the runner alive: the authority
/// marks the attempt unavailable and, under `restart_from_acknowledged`,
/// supersedes it and places the failover on its own node, never on the
/// runner. Resumed, the old runner is told its grant ended and stops; its
/// covered writes under the old grant are refused and recorded. Exactly
/// one attempt may run.
#[tokio::test(flavor = "multi_thread")]
async fn relay_paused_runner_alive() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start(&env)?;
    let placed = place_on_runner(&p, "m6-relay", "restart_from_acknowledged").await?;
    let running = Instant::now();
    acknowledged(&p, &placed).await?;
    provision(&p.authority)?;
    let group = harness_group(&p.runner, placed.first)?;
    heartbeats_hold_presence(&p, &placed, running).await?;
    p.relay.pause()?;
    let paused = Instant::now();
    let after = recovered_onto_the_authority(&p, &placed).await?;
    let took = paused.elapsed();
    eprintln!("recovered {took:?} after the pause");
    // The silence that made it unknown, then as long again, both counted
    // from the last heartbeat before the pause, at most one interval
    // earlier: never counted from the grant.
    let floor = Duration::from_millis(2 * UNAVAILABLE_AFTER_MS - HEARTBEAT_INTERVAL_MS);
    assert!(
        took >= floor,
        "recovered {took:?} after the pause, under {floor:?}"
    );
    let kinds = event_kinds(
        &p.authority,
        &p.authority.id,
        &placed.token,
        &placed.job_text(),
    )?;
    for kind in [
        "attempt_unavailable",
        "grant_superseded",
        "checkpoint_acknowledged",
    ] {
        assert!(kinds.iter().any(|e| e["kind"] == kind), "{kind}: {kinds:?}");
    }
    assert!(group_alive(group), "the old harness died with the pause");
    let counters = p.relay.counters()?;
    p.relay.resume()?;
    runner_obeyed(&p)?;
    let started = Instant::now();
    while group_alive(group) {
        assert!(started.elapsed() < WITHIN, "the old harness still runs");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_refused_and_recorded(&p, &placed).await?;
    let end = placed.client.managed_inspect(placed.job, |_| {}).await?;
    assert_eq!(live(&end), 1, "{end:?}");
    assert_eq!(end.job.attempts[1].attempt, after.job.attempts[1].attempt);
    eprintln!("relay while paused: {counters:?}");
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}

/// Scenario (2), the old runner frozen: `SIGSTOP` to the runner's daemon
/// and its attempt's group; the authority supersedes and regrants on its
/// own node; `SIGCONT`: the woken runner's heartbeat is answered with the
/// verdict that ended its grant, it stops the group, and its writes under
/// the old grant are refused. Exactly one attempt may run.
#[tokio::test(flavor = "multi_thread")]
async fn old_runner_sigstop_then_sigcont() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start(&env)?;
    let placed = place_on_runner(&p, "m6-stop", "restart_from_acknowledged").await?;
    acknowledged(&p, &placed).await?;
    provision(&p.authority)?;
    let group = harness_group(&p.runner, placed.first)?;
    let daemon = p.runner.daemon_pid().ok_or("the runner does not run")?;
    signal(daemon, group, rustix::process::Signal::STOP)?;
    recovered_onto_the_authority(&p, &placed).await?;
    signal(daemon, group, rustix::process::Signal::CONT)?;
    runner_obeyed(&p)?;
    let started = Instant::now();
    while group_alive(group) {
        assert!(started.elapsed() < WITHIN, "the woken harness still runs");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_refused_and_recorded(&p, &placed).await?;
    let end = placed.client.managed_inspect(placed.job, |_| {}).await?;
    assert_eq!(live(&end), 1, "{end:?}");
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}

/// The runner's local marker of the grant it took for `attempt`.
fn marker(runner: &Node, attempt: AttemptId) -> Fallible<Value> {
    let path = runner
        .state()
        .join("runs")
        .join("taken")
        .join(format!("{attempt}.json"));
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Scenario (3), the runner killed: `SIGKILL` to the runner's daemon; the
/// authority supersedes and regrants on its own node; the runner's daemon
/// starts again in the same state root, stops what its previous run left,
/// presents the grant it had taken and is answered superseded. It resumes
/// nothing, and its marker and the authority's journal agree.
#[tokio::test(flavor = "multi_thread")]
async fn old_runner_sigkill_then_restart() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start(&env)?;
    let placed = place_on_runner(&p, "m6-kill", "restart_from_acknowledged").await?;
    acknowledged(&p, &placed).await?;
    provision(&p.authority)?;
    let group = harness_group(&p.runner, placed.first)?;
    p.runner.kill()?;
    assert!(
        group_alive(group),
        "the orphan harness died with its daemon"
    );
    recovered_onto_the_authority(&p, &placed).await?;
    p.runner.start(None)?;
    let started = Instant::now();
    let ended = loop {
        let m = marker(&p.runner, placed.first)?;
        if !m["ended"].is_null() {
            break m;
        }
        assert!(
            started.elapsed() < WITHIN,
            "the grant was never presented:\n{}",
            p.runner.log()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(
        ended["ended"]["GrantEnded"]["verdict"], "superseded",
        "{ended}"
    );
    assert!(!group_alive(group), "the orphan harness still runs");
    let end = placed.client.managed_inspect(placed.job, |_| {}).await?;
    assert_eq!(end.job.attempts[0].attempt, placed.first);
    assert_eq!(end.job.attempts[0].status, "superseded", "{end:?}");
    assert_eq!(live(&end), 1, "{end:?}");
    let resumed = p
        .runner
        .state()
        .join("runs")
        .join("records")
        .join(placed.first.to_string());
    assert!(
        !resumed.exists(),
        "the runner resumed the superseded attempt"
    );
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}

/// Scenario (4), the `block` policy with a held harness: the relay is
/// paused, the attempt becomes `unknown`, and through the window in which
/// `restart_from_acknowledged` would have acted nothing moves; the state
/// is visible. An operator recover then supersedes it and places the
/// failover on the authority's node; resumed, the old runner stops.
#[tokio::test(flavor = "multi_thread")]
async fn block_policy_waits_until_an_operator_recovers() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start(&env)?;
    let placed = place_on_runner(&p, "m6-block", "block").await?;
    let running = Instant::now();
    acknowledged(&p, &placed).await?;
    provision(&p.authority)?;
    heartbeats_hold_presence(&p, &placed, running).await?;
    p.relay.pause()?;
    let paused = Instant::now();
    let unknown = until(&p, &placed.client, placed.job, "the attempt unknown", |i| {
        i.job.attempts[0].status == "unknown"
    })
    .await?;
    let floor = Duration::from_millis(UNAVAILABLE_AFTER_MS - HEARTBEAT_INTERVAL_MS);
    let took = paused.elapsed();
    assert!(
        took >= floor,
        "unknown {took:?} after the pause, under {floor:?}"
    );
    // A negative: past UNAVAILABLE_AFTER_MS of dwell and a tick, the
    // window in which a restart policy acts, under block nothing moved.
    tokio::time::sleep(Duration::from_secs(14)).await;
    let waited = placed.client.managed_inspect(placed.job, |_| {}).await?;
    for state in [&unknown, &waited] {
        assert_eq!(state.job.attempts.len(), 1, "a second grant: {state:?}");
        assert_eq!(state.job.attempts[0].status, "unknown", "{state:?}");
    }
    let kinds = event_kinds(
        &p.authority,
        &p.authority.id,
        &placed.token,
        &placed.job_text(),
    )?;
    for never in ["grant_superseded", "recovery_blocked"] {
        assert!(
            !kinds.iter().any(|e| e["kind"] == never),
            "{never}: {kinds:?}"
        );
    }
    p.authority.ok(&[
        "managed",
        "recover",
        &placed.job_text(),
        "--reason",
        "the operator moved it",
        "--operation-key",
        "m6-operator",
    ])?;
    recovered_onto_the_authority(&p, &placed).await?;
    p.relay.resume()?;
    runner_obeyed(&p)?;
    let end = placed.client.managed_inspect(placed.job, |_| {}).await?;
    assert_eq!(live(&end), 1, "{end:?}");
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}

/// Scenario (5), captured but not acknowledged: the authority's verifier
/// is held before its fetch (`before_acknowledge:hold`), so the cut is
/// captured and never acknowledged. The relay is paused: under
/// `restart_from_acknowledged` the restart is refused with its reason,
/// recorded, and the job shows it. An operator recover is refused the
/// same way; an operator supersede takes the grant and grants nothing.
#[tokio::test(flavor = "multi_thread")]
async fn captured_but_not_acknowledged_is_never_restarted_and_says_why() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut p = Partition::start_with(&env, Some("before_acknowledge:hold"))?;
    let placed = place_on_runner(&p, "m6-captured", "restart_from_acknowledged").await?;
    p.authority.ok(&[
        "managed",
        "checkpoint",
        &placed.job_text(),
        "--operation-key",
        "m6-cut",
    ])?;
    until_event(&p, &placed, "checkpoint_captured").await?;
    let started = Instant::now();
    while !p.authority.signal().exists() {
        assert!(
            started.elapsed() < WITHIN,
            "the verifier never reached its hold"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    provision(&p.authority)?;
    p.relay.pause()?;
    let kinds = until_event(&p, &placed, "recovery_blocked").await?;
    let blocked = kinds
        .iter()
        .find(|e| e["kind"] == "recovery_blocked")
        .ok_or("no recovery_blocked")?;
    assert_eq!(blocked["reason"], "no_acknowledged_checkpoint", "{blocked}");
    assert!(!kinds.iter().any(|e| e["kind"] == "checkpoint_acknowledged"));
    let shown = placed.client.managed_inspect(placed.job, |_| {}).await?;
    assert_eq!(shown.job.attempts.len(), 1, "{shown:?}");
    assert_eq!(shown.job.attempts[0].status, "unknown", "{shown:?}");
    let refused = p.authority.fails(&[
        "managed",
        "recover",
        &placed.job_text(),
        "--operation-key",
        "m6-recover",
    ])?;
    assert!(refused.contains("no acknowledged checkpoint"), "{refused}");
    p.authority.ok(&[
        "managed",
        "supersede",
        &placed.job_text(),
        "--reason",
        "nothing to restart from",
        "--operation-key",
        "m6-supersede",
    ])?;
    let end = until(&p, &placed.client, placed.job, "the grant taken", |i| {
        i.job.attempts[0].status == "superseded"
    })
    .await?;
    assert_eq!(live(&end), 0, "{end:?}");
    assert_eq!(end.job.attempts.len(), 1, "nothing granted: {end:?}");
    p.relay.resume()?;
    runner_obeyed(&p)?;
    p.runner.kill()?;
    p.authority.kill()?;
    Ok(())
}
