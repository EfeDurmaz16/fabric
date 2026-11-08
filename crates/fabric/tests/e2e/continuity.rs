//! M5 acceptance for planned continuity through the launch path, two real
//! daemons on one host, paired over the node link: a continue of a live
//! attempt cuts it on its node, places the new attempt where it is
//! pinned, and that node starts it from the cut at the fidelity it can
//! offer, with the work package, and reports what it achieved.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use fabric_client::Client;
use fabric_client::api::Inspection;
use fabric_core::id::JobId;

use crate::world::{Env, Fallible, Node, Wire, pair};

/// How long a step of the handoff may take to show.
const WITHIN: Duration = Duration::from_secs(40);

/// Waits until `done` holds of the job's inspection from `client`.
async fn until(
    node: &Node,
    client: &Client,
    job: JobId,
    what: &str,
    done: impl Fn(&Inspection) -> bool,
) -> Fallible<Inspection> {
    let started = Instant::now();
    loop {
        let inspection = client.managed_inspect(job, |_| {}).await?;
        if done(&inspection) {
            return Ok(inspection);
        }
        if started.elapsed() > WITHIN {
            return Err(format!("not {what}: {inspection:?}\n{}", node.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Whether `node` runs anything of `attempt`: its executor's records.
fn runs(node: &Node, attempt: &str) -> usize {
    std::fs::read_dir(node.state().join("runs").join("records").join(attempt))
        .map_or(0, Iterator::count)
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_planned_handoff_records_child_outcomes_and_the_fidelity_achieved() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    for node in [&mut a, &mut b] {
        node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
    }
    a.start(None)?;
    b.start(None)?;
    // The authority `a` dials `b`, so it can probe, launch and pull there.
    pair(&a, &b)?;
    let client = Client::new(a.state().join("fabric.sock"));
    let made = a.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "make the tests pass",
        "--harness",
        "codex",
        "--drain-budget",
        "1000",
        "--operation-key",
        "make-1",
    ])?;
    let job: JobId = made.lines().next().ok_or("nothing printed")?.parse()?;
    let first = until(&a, &client, job, "running", |i| {
        i.job.attempts.last().is_some_and(|x| x.status == "running")
    })
    .await?;
    let from = first.job.attempts[0].attempt;
    let primary = first.job.primary_task.to_string();
    let chosen = first.placements[0].chosen.ok_or("none chosen")?.to_string();
    let here = if chosen == a.id { &a } else { &b };

    // A test that is still running when the continue cuts the attempt.
    let registered = a.ok(&[
        "managed",
        "register",
        &job.to_string(),
        "--parent",
        &primary,
        "--label",
        "slow test",
        "--class",
        "test",
        "--operation-key",
        "register-1",
        "--",
        "sleep",
        "30",
    ])?;
    let test = registered
        .lines()
        .find_map(|l| l.strip_prefix("task ")?.strip_suffix(" (slow test): open"))
        .ok_or_else(|| format!("no test task in {registered}"))?
        .to_owned();
    let started = Instant::now();
    while runs(here, &from.to_string()) < 2 {
        if started.elapsed() > WITHIN {
            return Err(format!("the test was not spawned:\n{}", here.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    a.ok(&[
        "managed",
        "continue",
        &job.to_string(),
        "--operation-key",
        "continue-1",
    ])?;
    let after = until(&a, &client, job, "continued", |i| {
        !i.continuations.is_empty()
    })
    .await?;
    let continuation = &after.continuations[0];
    assert_eq!(continuation.from, from, "{after:?}");
    assert_eq!(
        continuation.achieved, "new_native_session_with_package",
        "{after:?}"
    );
    let digest = continuation.package.as_deref().ok_or("no package digest")?;
    assert_eq!(digest.len(), 64, "the package digest is shown: {after:?}");
    let new = continuation.attempt;
    assert_ne!(new, from);
    let first_attempt = after
        .job
        .attempts
        .iter()
        .find(|x| x.attempt == from)
        .ok_or("the first attempt is gone")?;
    assert_eq!(first_attempt.status, "checkpointed", "{after:?}");
    let test = after
        .job
        .tasks
        .iter()
        .find(|t| t.task.to_string() == test)
        .ok_or("the test task is gone")?;
    let receipt = test.receipt.as_ref().ok_or("the test has no receipt")?;
    assert_eq!(receipt.status, "interrupted", "never passed: {after:?}");
    assert!(receipt.interrupted);
    assert_eq!(
        test.state, "open",
        "the task stays open for the next attempt"
    );
    // The new attempt runs where the handoff's placement chose, and nothing
    // of the first attempt is left on its node.
    let placed = after.placements.last().ok_or("no handoff placement")?;
    assert_eq!(placed.kind, "planned_handoff", "{after:?}");
    let target = placed.chosen.ok_or("the handoff parked")?.to_string();
    let there = if target == a.id { &a } else { &b };
    until(&a, &client, job, "running again", |i| {
        i.job
            .attempts
            .iter()
            .any(|x| x.attempt == new && x.status == "running")
    })
    .await?;
    assert!(
        runs(there, &new.to_string()) >= 1,
        "there:\n{}\nhere:\n{}\n{after:?}",
        there.log(),
        here.log()
    );
    let started = Instant::now();
    while runs(here, &from.to_string()) > 0 {
        if started.elapsed() > WITHIN {
            return Err(format!("the first attempt was left running:\n{}", here.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    a.ok(&[
        "managed",
        "cancel",
        &job.to_string(),
        "--operation-key",
        "cancel-1",
    ])?;
    Ok(())
}

/// A native-session continuation needs the harness's own state restored on
/// the target; the launch path restores the workspace from the cut and
/// offers no native session (docs/managed-jobs.md, "Launch path", as built).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "blocked: the launch does not restore the harness's state, so no native session is offered"]
async fn e2e_a_native_session_continues_on_a_node_with_the_same_layout() -> Fallible {
    Ok(())
}
