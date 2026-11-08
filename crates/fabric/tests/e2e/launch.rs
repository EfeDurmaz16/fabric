//! M5 acceptance for the launch path across two real daemons on one host,
//! each with its own state root, paired over a TCP and TLS node link, each
//! capturing the link's plaintext (`FABRIC_NET_TAP`). A job submitted to
//! the authority `a` through its facade is pinned to `b` and starts there:
//! its harness turn runs, a build task runs its program and a test task
//! runs with the job's secret. A checkpoint cuts the harness's turn and
//! the attempt goes on. A continue pinned to `a` cuts the attempt on `b`
//! with each kind's outcome in the cut (the harness checkpointed, the
//! build and the test interrupted, never passed), and `a` starts the next
//! attempt from that cut with the work package, records the fidelity it
//! achieved, and has the workspace the build wrote. The secret's value is
//! then searched for in both state roots, both logs, both captures and the
//! shared home, and is in none of them.

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use fabric_client::Client;
use fabric_client::api::Inspection;
use fabric_core::id::JobId;
use serde_json::Value;

use crate::world::{Env, Fallible, Node, Wire, pair};

/// How long a step may take to show.
const WITHIN: Duration = Duration::from_secs(40);

/// A value no fixture or default holds, so a match is the secret itself.
const VALUE: &str = "s3cr3t-value-41d8e0b7-only-in-the-daemon-env";

/// What the build writes in the workspace, so its run is seen there.
const BUILT: &str = "built-by-the-build-task-5c2e";

/// Waits until `done` holds of the job's inspection.
async fn until(
    nodes: [&Node; 2],
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
            return Err(format!(
                "not {what}: {inspection:?}\na:\n{}\nb:\n{}",
                nodes[0].log(),
                nodes[1].log()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The executor's launch records `node` holds for `attempt`, by class.
fn records(node: &Node, attempt: &str) -> Vec<String> {
    let dir = node.state().join("runs").join("records").join(attempt);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut classes: Vec<String> = entries
        .flatten()
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter_map(|r| r["class"].as_str().map(str::to_owned))
        .collect();
    classes.sort();
    classes
}

/// Waits until `node` runs `attempt`'s harness, build and test.
async fn runs_all(node: &Node, attempt: &str) -> Fallible {
    let started = Instant::now();
    loop {
        if records(node, attempt) == ["Build", "Harness", "Test"] {
            return Ok(());
        }
        if started.elapsed() > WITHIN {
            return Err(format!(
                "{attempt} runs {:?} on {}:\n{}",
                records(node, attempt),
                node.name,
                node.log()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every file under `dir` whose bytes hold `needle`.
fn holding(dir: &Path, needle: &[u8], out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && !path.is_symlink() {
            holding(&path, needle, out);
        } else if let Ok(bytes) = std::fs::read(&path)
            && bytes.windows(needle.len()).any(|w| w == needle)
        {
            out.push(path.display().to_string());
        }
    }
}

/// Whether `bytes` hold `needle`.
fn holds(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}

/// The task labelled `label`'s last receipt status and whether it was interrupted.
fn receipt(inspection: &Inspection, label: &str) -> Option<(String, bool)> {
    let task = inspection
        .job
        .tasks
        .iter()
        .find(|t| t.label.as_deref() == Some(label))?;
    let receipt = task.receipt.as_ref()?;
    Some((receipt.status.clone(), receipt.interrupted))
}

/// The JSON a facade command printed.
fn answer(text: &str) -> Fallible<Value> {
    let start = text.find('{').ok_or("no JSON in the answer")?;
    Ok(serde_json::from_str(&text[start..])?)
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "the launch, the cut and the carry read best as one script"
)]
async fn e2e_a_job_submitted_to_a_runs_on_b_and_continues_on_a_with_its_package() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let tap_a = env.root.join("tap-a");
    let tap_b = env.root.join("tap-b");
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    let home_a = env.root.join("home-a");
    let home_b = env.root.join("home-b");
    for (node, tap, home) in [(&mut a, &tap_a, &home_a), (&mut b, &tap_b, &home_b)] {
        // A home of its own, so each daemon's job roots are its own as on
        // two hosts; its CLI commands keep the shared one.
        std::fs::create_dir_all(home)?;
        node.daemon_env("HOME", home.clone().into_os_string());
        // One harness process that holds its turn open, so the strong
        // handoff is not blocked by an escaped child.
        node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
        node.daemon_env("FABRIC_SECRET_DEPLOY_KEY", OsString::from(VALUE));
        node.daemon_env("FABRIC_NET_TAP", tap.clone().into_os_string());
    }
    a.start(None)?;
    b.start(None)?;
    pair(&b, &a)?;
    let client = Client::new(a.state().join("fabric.sock"));

    // (1) Submitted to `a`'s facade from `b`, pinned to `b`: `a` places it
    // and launches it on `b` over the link.
    let minted = a.ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?;
    let token = minted
        .lines()
        .find_map(|l| l.strip_prefix("bearer "))
        .and_then(|r| r.split_whitespace().next())
        .ok_or("no bearer")?
        .to_owned();
    let accepted = answer(&b.ok(&[
        "facade",
        "submit",
        "--authority",
        &a.id,
        "--token",
        &token,
        "--empty",
        "--goal",
        "ship the launch path",
        "--harness",
        "codex",
        "--secret",
        "DEPLOY_KEY",
        "--drain-budget",
        "1000",
        "--pin",
        &b.id,
        "--key",
        "submit-1",
    ])?)?;
    assert_eq!(accepted["type"], "accepted", "{accepted}");
    let job: JobId = accepted["job"].as_str().ok_or("no job")?.parse()?;
    let first = until([&a, &b], &client, job, "running", |i| {
        i.job.attempts.last().is_some_and(|x| x.status == "running")
    })
    .await?;
    assert_eq!(
        first.placements[0].chosen.map(|n| n.to_string()),
        Some(b.id.clone()),
        "{first:?}"
    );
    let from = first.job.attempts[0].attempt;
    let from_text = from.to_string();
    let primary = first.job.primary_task.to_string();
    let job_text = job.to_string();
    assert_eq!(records(&a, &from_text), Vec::<String>::new());

    // A build that writes the workspace and keeps going, and a test that
    // runs only with the job's secret in its environment.
    for (label, class, script, key) in [
        (
            "build",
            "build",
            format!("echo {BUILT} > built.txt && sleep 30"),
            "register-build",
        ),
        (
            "test",
            "test",
            "test -n \"$DEPLOY_KEY\" && sleep 30".to_owned(),
            "register-test",
        ),
    ] {
        a.ok(&[
            "managed",
            "register",
            &job_text,
            "--parent",
            &primary,
            "--label",
            label,
            "--class",
            class,
            "--operation-key",
            key,
            "--",
            "sh",
            "-c",
            &script,
        ])?;
    }
    runs_all(&b, &from_text).await?;
    let built = home_b
        .join("fabric")
        .join("jobs")
        .join(job.to_string())
        .join("work")
        .join("built.txt");
    let started = Instant::now();
    while !std::fs::read_to_string(&built).is_ok_and(|t| t.trim() == BUILT) {
        if started.elapsed() > WITHIN {
            return Err(format!("the build did not write {}", built.display()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The harness's turn is captured and the attempt goes on.
    a.ok(&[
        "managed",
        "checkpoint",
        &job_text,
        "--operation-key",
        "checkpoint-1",
    ])?;
    let cut = until([&a, &b], &client, job, "checkpointed", |i| {
        i.job
            .tasks
            .iter()
            .find(|t| t.task == i.job.primary_task)
            .and_then(|t| t.receipt.as_ref())
            .is_some_and(|r| r.status == "checkpointed")
    })
    .await?;
    let last = cut.job.attempts.last().ok_or("no attempt")?;
    assert_eq!((last.attempt, last.status.as_str()), (from, "running"));

    // (2) and (3): the planned handoff to `a`, named by the continue's pin.
    a.ok(&[
        "managed",
        "continue",
        &job_text,
        "--pin",
        &a.id,
        "--operation-key",
        "continue-1",
    ])?;
    let after = until([&a, &b], &client, job, "continued", |i| {
        !i.continuations.is_empty()
            && i.job
                .attempts
                .iter()
                .any(|x| x.attempt != from && x.status == "running")
    })
    .await?;
    let placed = after.placements.last().ok_or("no handoff placement")?;
    assert_eq!(placed.kind, "planned_handoff", "{after:?}");
    assert_eq!(
        placed.chosen.map(|n| n.to_string()),
        Some(a.id.clone()),
        "{after:?}"
    );
    let continuation = &after.continuations[0];
    assert_eq!(continuation.from, from, "{after:?}");
    assert_eq!(continuation.achieved, "new_native_session_with_package");
    let digest = continuation.package.as_deref().ok_or("no package")?;
    assert_eq!(digest.len(), 64, "{after:?}");
    let checkpointed = after
        .job
        .attempts
        .iter()
        .find(|x| x.attempt == from)
        .ok_or("the first attempt is gone")?;
    assert_eq!(checkpointed.status, "checkpointed", "{after:?}");
    // Each kind's outcome in the cut: the harness's turn checkpointed, the
    // build and the test interrupted and never passed.
    let harness = after
        .job
        .tasks
        .iter()
        .find(|t| t.task == after.job.primary_task)
        .and_then(|t| t.receipt.as_ref())
        .map(|r| r.status.clone());
    assert_eq!(harness.as_deref(), Some("checkpointed"), "{after:?}");
    assert_eq!(
        receipt(&after, "build"),
        Some(("interrupted".to_owned(), true)),
        "{after:?}"
    );
    assert_eq!(
        receipt(&after, "test"),
        Some(("interrupted".to_owned(), true)),
        "the test saw the key and ran until the cut: {after:?}"
    );
    let new = continuation.attempt.to_string();
    runs_all(&a, &new).await?;
    let started = Instant::now();
    while !records(&b, &from_text).is_empty() {
        if started.elapsed() > WITHIN {
            return Err(format!("the first attempt was left on b:\n{}", b.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        records(&b, &new).is_empty(),
        "nothing of the new attempt on b"
    );
    // The workspace the build wrote on `b` came to `a` from the cut.
    let carried = home_a
        .join("fabric")
        .join("jobs")
        .join(job.to_string())
        .join("work")
        .join("built.txt");
    assert_eq!(
        std::fs::read_to_string(&carried)?.trim(),
        BUILT,
        "the cut's workspace is on a"
    );

    a.ok(&[
        "managed",
        "cancel",
        &job_text,
        "--operation-key",
        "cancel-1",
    ])?;
    let cancelled = until([&a, &b], &client, job, "cancelled", |i| {
        i.job.state == "cancelled"
    })
    .await?;
    a.stop()?;
    b.stop()?;

    // The captures saw the link: the launch's secret name and goal, and
    // the workspace chunk `a` pulled from `b`, in the clear.
    let tap_a = std::fs::read(&tap_a)?;
    let tap_b = std::fs::read(&tap_b)?;
    assert!(holds(&tap_a, b"DEPLOY_KEY"), "a's capture saw no launch");
    assert!(holds(&tap_a, b"ship the launch path"));
    assert!(holds(&tap_a, BUILT.as_bytes()), "a's capture saw no chunk");
    assert!(!tap_b.is_empty(), "b's capture saw nothing");
    // The value is nowhere: no state root, journal, store, package, run
    // record, checkpoint, log, capture or file under the shared home.
    // The fake harness writes its own environment to `codex-env` in its
    // home: the job's process holding its secret is the job's, and it shows
    // the harness got it by name on both nodes. Nothing else may hold it.
    let given = format!("DEPLOY_KEY={VALUE}");
    for home in [&home_a, &home_b] {
        let seen = std::fs::read_to_string(home.join("codex-env"))?;
        assert!(seen.lines().any(|l| l == given), "{}", home.display());
    }
    let mut found = Vec::new();
    holding(&env.root, VALUE.as_bytes(), &mut found);
    holding(&env.home, VALUE.as_bytes(), &mut found);
    found.retain(|p| !p.ends_with("/codex-env"));
    assert!(found.is_empty(), "the secret value is held in {found:?}");
    assert!(!format!("{cancelled:?}").contains(VALUE));
    eprintln!(
        "counters: captured bytes a={} b={}",
        tap_a.len(),
        tap_b.len()
    );
    Ok(())
}

/// The per-kind policy's other side: a harness that left a child outside
/// its group blocks a strong handoff. The continue to `a` is answered with
/// `handoff_blocked` from `b`, nothing is placed or granted on `a`, and the
/// attempt goes on running on `b`.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_handoff_an_escaped_child_blocks_leaves_the_attempt_running_on_b() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    for node in [&mut a, &mut b] {
        // Its loop forks a `sleep` per tick: an escaped child.
        node.daemon_env("FAKE_CODEX_WAIT", OsString::from("1"));
    }
    a.start(None)?;
    b.start(None)?;
    // The authority `a` dials `b`, so it can probe and launch there.
    pair(&a, &b)?;
    let client = Client::new(a.state().join("fabric.sock"));
    let made = a.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "hold a child",
        "--harness",
        "codex",
        "--drain-budget",
        "1000",
        "--pin",
        &b.id,
        "--operation-key",
        "make-1",
    ])?;
    let job: JobId = made.lines().next().ok_or("nothing printed")?.parse()?;
    let first = until([&a, &b], &client, job, "running", |i| {
        i.job.attempts.last().is_some_and(|x| x.status == "running")
    })
    .await?;
    let from = first.job.attempts[0].attempt;
    let job_text = job.to_string();
    a.ok(&[
        "managed",
        "continue",
        &job_text,
        "--pin",
        &a.id,
        "--operation-key",
        "continue-1",
    ])?;
    let minted = a.ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?;
    let token = minted
        .lines()
        .find_map(|l| l.strip_prefix("bearer "))
        .and_then(|r| r.split_whitespace().next())
        .ok_or("no bearer")?
        .to_owned();
    let started = Instant::now();
    loop {
        let text = b.ok(&[
            "facade",
            "watch",
            "--authority",
            &a.id,
            "--token",
            &token,
            &job_text,
            "--wait-ms",
            "0",
        ])?;
        if text.contains("\"handoff_blocked\"") {
            break;
        }
        if started.elapsed() > WITHIN {
            return Err(format!("no handoff_blocked: {text}\nb:\n{}", b.log()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let after = client.managed_inspect(job, |_| {}).await?;
    assert!(after.continuations.is_empty(), "{after:?}");
    assert_eq!(after.job.attempts.len(), 1, "nothing granted: {after:?}");
    let last = &after.job.attempts[0];
    assert_eq!((last.attempt, last.status.as_str()), (from, "running"));
    assert!(
        after.placements.iter().all(|p| p.kind != "planned_handoff"),
        "nothing placed: {after:?}"
    );
    assert_eq!(records(&a, &from.to_string()), Vec::<String>::new());
    assert_eq!(records(&b, &from.to_string()), ["Harness"]);
    a.ok(&[
        "managed",
        "cancel",
        &job_text,
        "--operation-key",
        "cancel-1",
    ])?;
    until([&a, &b], &client, job, "cancelled", |i| {
        i.job.state == "cancelled"
    })
    .await?;
    Ok(())
}
