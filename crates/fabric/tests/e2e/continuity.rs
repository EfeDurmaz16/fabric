//! M5 acceptance for planned continuity through the launch path, two real
//! daemons on one host, paired over the node link: a continue of a live
//! attempt cuts it on its node, places the new attempt where it is
//! pinned, and that node starts it from the cut at the fidelity it can
//! offer, with the work package, and reports what it achieved.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fabric_client::Client;
use fabric_client::api::Inspection;
use fabric_core::id::JobId;

use crate::world::{Env, Fallible, Node, Wire, pair, wait_for};

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
#[expect(
    clippy::too_many_lines,
    reason = "the scenario reads best as one script"
)]
async fn e2e_a_planned_handoff_records_child_outcomes_and_the_fidelity_achieved() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    for node in [&mut a, &mut b] {
        node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
        // A home of its own, as on two hosts: the job root differs, so the
        // continuation takes the package.
        let home = env.root.join(format!("home-{}", node.name));
        std::fs::create_dir_all(&home)?;
        node.daemon_env("HOME", home.into_os_string());
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
    let (here, other) = if chosen == a.id { (&a, &b) } else { (&b, &a) };

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

    // To the other node: its home, and so the job root, is another.
    a.ok(&[
        "managed",
        "continue",
        &job.to_string(),
        "--pin",
        &other.id,
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

/// Where the rollouts of the fake Codex's threads are under `home`.
fn sessions(home: &Path) -> PathBuf {
    home.join(".codex").join("sessions")
}

/// What a continue achieved, and what the target's harness said and was
/// started with, for one layout.
struct Handoff {
    /// The continuation in the inspection read from the authority.
    achieved: String,
    /// Its package digest, when a package was used.
    package: Option<String>,
    /// The continuation as `fabric managed inspect` printed it.
    printed: String,
    /// The target's fake Codex arguments for the new attempt.
    args: Vec<String>,
    /// What the target's harness answered about the first turn.
    recalled: Option<String>,
}

/// The first turn of a Codex job runs on `b` with `GOAL` as its prompt; a
/// continue pinned to `a` cuts it there and is placed on `a`, which is held
/// before the launch while the test removes every rollout from the home
/// `a` runs with, so the only copy of the session `a` can find is the one
/// in the cut. With `same_layout` both daemons share one home, so the job
/// root and the home are the same paths on both; otherwise each has its own.
#[expect(
    clippy::too_many_lines,
    reason = "the scenario reads best as one script"
)]
async fn hand_off(same_layout: bool) -> Fallible<Handoff> {
    const GOAL: &str = "remember the word heliotrope";
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    let (home_a, home_b) = if same_layout {
        (env.home.clone(), env.home.clone())
    } else {
        (env.root.join("home-a"), env.root.join("home-b"))
    };
    for (node, home) in [(&mut a, &home_a), (&mut b, &home_b)] {
        node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
        if !same_layout {
            std::fs::create_dir_all(home)?;
            node.daemon_env("HOME", home.clone().into_os_string());
        }
    }
    a.start(None)?;
    b.start(None)?;
    pair(&a, &b)?;
    let client = Client::new(a.state().join("fabric.sock"));
    let made = a.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        GOAL,
        "--harness",
        "codex",
        "--pin",
        &b.id,
        "--drain-budget",
        "1000",
        "--operation-key",
        "make-1",
    ])?;
    let job: JobId = made.lines().next().ok_or("nothing printed")?.parse()?;
    until(&a, &client, job, "running on b", |i| {
        i.job.attempts.last().is_some_and(|x| x.status == "running")
    })
    .await?;
    // The first turn is in its rollout on `b`, and `b` has read the thread
    // the harness named, so the cut captures the session.
    let started = Instant::now();
    while !std::fs::read_to_string(home_b.join("codex-args"))
        .is_ok_and(|args| args.lines().any(|l| l == GOAL))
    {
        if started.elapsed() > WITHIN {
            return Err(format!("the first turn did not run on b:\n{}", b.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    // `a` is started again held before its next launch: the new attempt's.
    a.kill()?;
    a.start(Some("before_launch:hold"))?;
    a.ok(&[
        "managed",
        "continue",
        &job.to_string(),
        "--pin",
        &a.id,
        "--operation-key",
        "continue-1",
    ])?;
    wait_for(&a.signal(), "the continuation's launch")?;
    for gone in [
        std::fs::remove_dir_all(sessions(&home_a)),
        std::fs::remove_file(home_a.join("codex-args")),
    ] {
        match gone {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    std::fs::remove_file(a.signal())?;

    let after = until(&a, &client, job, "continued", |i| {
        !i.continuations.is_empty()
    })
    .await?;
    let continuation = &after.continuations[0];
    let new = continuation.attempt;
    until(&a, &client, job, "running on a", |i| {
        i.job
            .attempts
            .iter()
            .any(|x| x.attempt == new && x.status == "running")
    })
    .await?;
    // The harness on `a` has written its turn: the first turn and this one
    // in the resumed thread, this one alone in a new one.
    let turns = if same_layout { 2 } else { 1 };
    let started = Instant::now();
    while user_turns(&sessions(&home_a)) != turns {
        if started.elapsed() > WITHIN {
            return Err(format!("the harness did not run its turn on a:\n{}", a.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let printed = a
        .ok(&["managed", "inspect", &job.to_string()])?
        .lines()
        .find(|l| l.starts_with("continuation: "))
        .ok_or("no continuation printed")?
        .to_owned();
    let handoff = Handoff {
        achieved: continuation.achieved.clone(),
        package: continuation.package.clone(),
        printed,
        args: env_args(&home_a)?,
        recalled: std::fs::read_to_string(home_a.join("codex-recalled"))
            .ok()
            .map(|r| r.trim().to_owned()),
    };
    a.ok(&[
        "managed",
        "cancel",
        &job.to_string(),
        "--operation-key",
        "cancel-1",
    ])?;
    Ok(handoff)
}

/// The user turns the rollouts under `dir` hold, in all.
fn user_turns(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                user_turns(&path)
            } else {
                std::fs::read_to_string(&path).map_or(0, |text| {
                    text.lines()
                        .filter(|l| l.contains(r#""role":"user""#))
                        .count()
                })
            }
        })
        .sum()
}

/// The fake Codex's arguments as it wrote them under `home`.
fn env_args(home: &Path) -> Fallible<Vec<String>> {
    Ok(std::fs::read_to_string(home.join("codex-args"))?
        .lines()
        .map(str::to_owned)
        .collect())
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_native_session_continues_on_a_node_with_the_same_layout() -> Fallible {
    let handoff = hand_off(true).await?;
    assert_eq!(handoff.achieved, "native_session");
    assert_eq!(handoff.package, None, "a native session needs no package");
    assert!(
        handoff.printed.ends_with("at fidelity native_session"),
        "{}",
        handoff.printed
    );
    // The new attempt resumed the thread the first turn ran in, and found
    // the first turn in it: the rollout came back only through the cut.
    assert_eq!(
        handoff.args.get(..3),
        Some(
            &[
                "exec".to_owned(),
                "resume".to_owned(),
                Env::CODEX_THREAD.to_owned()
            ][..]
        ),
        "{:?}",
        handoff.args
    );
    assert_eq!(
        handoff.recalled.as_deref(),
        Some("remember the word heliotrope")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_continue_on_a_node_with_another_layout_starts_a_new_session_with_the_package()
-> Fallible {
    let handoff = hand_off(false).await?;
    assert_eq!(handoff.achieved, "new_native_session_with_package");
    let digest = handoff.package.as_deref().ok_or("no package digest")?;
    assert_eq!(digest.len(), 64);
    assert!(
        handoff
            .printed
            .contains("at fidelity new_native_session_with_package with package "),
        "{}",
        handoff.printed
    );
    assert!(
        !handoff.args.iter().any(|a| a == "resume"),
        "a new session resumes nothing: {:?}",
        handoff.args
    );
    assert_eq!(
        handoff.recalled, None,
        "no resume asked about the first turn"
    );
    Ok(())
}
