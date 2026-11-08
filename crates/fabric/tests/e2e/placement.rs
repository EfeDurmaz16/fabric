//! M5 acceptance for intent-first placement (ADR 0026) through two real
//! daemons on one host, each with its own state root, paired over the
//! node link: a job submitted from the other node names no machine, and
//! the authority places it from facts both nodes measure now. Covered:
//! two eligible nodes and the recorded decision, no eligible node parks
//! the submit, a secret only one node can resolve, a pin, and a create
//! accepted before a crash that is placed once after the restart.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::world::{Env, Fallible, Node, Wire, pair};

/// How long a placement may take to show: one probe round is bound by 5 s.
const PLACED_WITHIN: Duration = Duration::from_secs(20);

/// The `bearer <hex>` line `fabric facade mint` prints.
fn bearer(text: &str) -> Fallible<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("bearer "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| format!("no bearer in:\n{text}").into())
}

/// The JSON a facade command printed.
fn answer(text: &str) -> Fallible<Value> {
    let start = text.find('{').ok_or("no JSON in the answer")?;
    Ok(serde_json::from_str(&text[start..])?)
}

/// The authority `a` and the other node `b`, paired, `b` started with
/// `b_env` in its daemon's environment, and a bearer minted on `a`.
fn world(
    env: &std::sync::Arc<Env>,
    b_env: &[(&'static str, &str)],
) -> Fallible<(Node, Node, String)> {
    env.install_fake_codex()?;
    let mut a = Node::new(env, "a", Wire::TcpTls)?;
    let mut b = Node::new(env, "b", Wire::TcpTls)?;
    for (name, value) in b_env {
        b.daemon_env(name, OsString::from(value));
    }
    a.start(None)?;
    b.start(None)?;
    pair(&b, &a)?;
    let token = bearer(&a.ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?)?;
    Ok((a, b, token))
}

/// Submits from `from` to the authority `a` with `policy` flags; the job id.
fn submit(from: &Node, a: &Node, token: &str, key: &str, policy: &[&str]) -> Fallible<String> {
    let mut args = vec![
        "facade",
        "submit",
        "--authority",
        &a.id,
        "--token",
        token,
        "--empty",
        "--goal",
        "place me where I can run",
        "--harness",
        "codex",
        "--operation-key",
        key,
    ];
    args.extend_from_slice(policy);
    let accepted = answer(&from.ok(&args)?)?;
    assert_eq!(accepted["type"], "accepted", "{accepted}");
    Ok(accepted["job"].as_str().ok_or("no job")?.to_owned())
}

/// The inspection from `from` once the job holds a placement.
fn placed(from: &Node, a: &Node, token: &str, job: &str) -> Fallible<Value> {
    let started = Instant::now();
    loop {
        let inspection = answer(&from.ok(&[
            "facade",
            "inspect",
            "--authority",
            &a.id,
            "--token",
            token,
            job,
        ])?)?;
        assert_eq!(inspection["type"], "inspection", "{inspection}");
        if inspection["placements"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
        {
            return Ok(inspection);
        }
        if started.elapsed() > PLACED_WITHIN {
            return Err(format!(
                "no placement within {PLACED_WITHIN:?}: {inspection}\n{}",
                a.log()
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The candidate for `node` in a placement.
fn candidate<'a>(placement: &'a Value, node: &str) -> Fallible<&'a Value> {
    placement["candidates"]
        .as_array()
        .ok_or("no candidates")?
        .iter()
        .find(|c| c["node"] == node)
        .ok_or_else(|| format!("{node} is not a candidate: {placement}").into())
}

/// The names of `candidate`'s exclusion reasons; empty when it was eligible.
fn exclusions(candidate: &Value) -> Vec<String> {
    candidate["verdict"]["excluded"]["reasons"]
        .as_array()
        .map(|reasons| {
            reasons
                .iter()
                .map(|r| match r {
                    Value::String(name) => name.clone(),
                    Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn eligible(candidate: &Value) -> bool {
    candidate["verdict"].get("eligible").is_some()
}

#[test]
fn e2e_a_submit_names_no_machine_and_the_decision_is_recorded() -> Fallible {
    let env = Env::new()?;
    let (a, b, token) = world(&env, &[("FABRIC_SECRET_DEPLOY_KEY", "only-b-has-it")])?;

    // Two eligible nodes: both measured now, both eligible, the first in
    // rank order is the one granted, and the grant names it.
    let job = submit(&b, &a, &token, "submit-any", &[])?;
    let inspection = placed(&b, &a, &token, &job)?;
    let placement = &inspection["placements"][0];
    assert_eq!(placement["kind"], "initial", "{placement}");
    assert_eq!(inspection["parked"], false, "{inspection}");
    assert!(eligible(candidate(placement, &a.id)?), "{placement}");
    assert!(eligible(candidate(placement, &b.id)?), "{placement}");
    let chosen = placement["chosen"].as_str().ok_or("none chosen")?;
    assert_eq!(
        placement["candidates"][0]["node"], chosen,
        "the first ranked wins: {placement}"
    );
    let attempts = inspection["job"]["attempts"].as_array().ok_or("attempts")?;
    assert_eq!(attempts.len(), 1, "{inspection}");
    // Granted, and launched on the chosen node at once since the launch
    // path: the attempt half is e2e_a_submit_runs_on_the_eligible_node_without_naming_it.
    assert!(
        ["granted", "running", "succeeded", "failed"]
            .contains(&attempts[0]["status"].as_str().unwrap_or_default()),
        "{inspection}"
    );
    let events = answer(&b.ok(&[
        "facade",
        "watch",
        "--authority",
        &a.id,
        "--token",
        &token,
        &job,
        "--wait-ms",
        "0",
    ])?)?;
    let granted: Vec<&Value> = events["events"]
        .as_array()
        .ok_or("no events")?
        .iter()
        .filter(|e| e["event"]["kind"] == "attempt_granted")
        .collect();
    assert_eq!(granted.len(), 1, "{events}");
    assert_eq!(
        granted[0]["event"]["node"], chosen,
        "the grant names the chosen node: {events}"
    );
    assert_eq!(
        granted[0]["event"]["attempt"], attempts[0]["attempt"],
        "{events}"
    );

    // A secret only `b` resolves: `a` is excluded naming it, `b` is chosen.
    let job = submit(&b, &a, &token, "submit-secret", &["--secret", "DEPLOY_KEY"])?;
    let inspection = placed(&b, &a, &token, &job)?;
    let placement = &inspection["placements"][0];
    assert_eq!(placement["chosen"], b.id.as_str(), "{placement}");
    assert_eq!(
        exclusions(candidate(placement, &a.id)?),
        ["secret"],
        "{placement}"
    );
    assert!(eligible(candidate(placement, &b.id)?), "{placement}");
    assert!(
        !inspection.to_string().contains("only-b-has-it"),
        "a secret value never leaves the node that holds it"
    );

    // No eligible node: accepted, parked, both exclusions recorded, no attempt.
    let job = submit(
        &b,
        &a,
        &token,
        "submit-parked",
        &["--secret", "NOBODY_HAS_THIS"],
    )?;
    let inspection = placed(&b, &a, &token, &job)?;
    let placement = &inspection["placements"][0];
    assert_eq!(placement["chosen"], Value::Null, "{placement}");
    assert_eq!(inspection["parked"], true, "{inspection}");
    assert_eq!(inspection["job"]["state"], "active", "{inspection}");
    assert_eq!(
        inspection["job"]["attempts"],
        Value::Array(vec![]),
        "{inspection}"
    );
    assert_eq!(
        exclusions(candidate(placement, &a.id)?),
        ["secret"],
        "{placement}"
    );
    assert_eq!(
        exclusions(candidate(placement, &b.id)?),
        ["secret"],
        "{placement}"
    );

    // A pin to each node in turn: the pinned one is chosen, the other is
    // excluded naming the pin, whatever the ranking would have said.
    for (pinned, other, key) in [(&a, &b, "submit-pin-a"), (&b, &a, "submit-pin-b")] {
        let job = submit(&b, &a, &token, key, &["--pin", &pinned.id])?;
        let inspection = placed(&b, &a, &token, &job)?;
        let placement = &inspection["placements"][0];
        assert_eq!(placement["chosen"], pinned.id.as_str(), "{placement}");
        assert_eq!(
            exclusions(candidate(placement, &other.id)?),
            ["pin"],
            "{placement}"
        );
    }
    Ok(())
}

/// The inspection from `from` once the job's newest attempt is `status`.
fn attempt_is(from: &Node, a: &Node, token: &str, job: &str, status: &str) -> Fallible<Value> {
    let started = Instant::now();
    loop {
        let inspection = answer(&from.ok(&[
            "facade",
            "inspect",
            "--authority",
            &a.id,
            "--token",
            token,
            job,
        ])?)?;
        let now = inspection["job"]["attempts"]
            .as_array()
            .and_then(|a| a.last())
            .map(|a| a["status"].clone());
        if now.as_ref().and_then(Value::as_str) == Some(status) {
            return Ok(inspection);
        }
        if started.elapsed() > PLACED_WITHIN {
            return Err(format!(
                "the attempt is not {status} within {PLACED_WITHIN:?}: {inspection}\n{}",
                a.log()
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The launch path's half of M5 placement: a job submitted from `b`
/// names no machine, the authority `a` places it, and the attempt starts
/// on the chosen node: `running` in the inspection, its harness process
/// recorded by that node's executor and alive, and nothing of it on the
/// other node.
#[test]
fn e2e_a_submit_runs_on_the_eligible_node_without_naming_it() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    // The fake harness keeps its turn open, as a real one works for a while.
    a.daemon_env("FAKE_CODEX_WAIT", OsString::from("1"));
    b.daemon_env("FAKE_CODEX_WAIT", OsString::from("1"));
    a.start(None)?;
    b.start(None)?;
    pair(&b, &a)?;
    let token = bearer(&a.ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?)?;
    let job = submit(&b, &a, &token, "submit-run", &[])?;
    let inspection = attempt_is(&b, &a, &token, &job, "running")?;
    let chosen = inspection["placements"][0]["chosen"]
        .as_str()
        .ok_or("none chosen")?
        .to_owned();
    let attempt = inspection["job"]["attempts"][0]["attempt"]
        .as_str()
        .ok_or("no attempt")?
        .to_owned();
    let (runs_on, other) = if chosen == a.id { (&a, &b) } else { (&b, &a) };
    let records = runs_on.state().join("runs").join("records").join(&attempt);
    let record = std::fs::read_dir(&records)
        .map_err(|e| format!("{}: {e}\n{}", records.display(), runs_on.log()))?
        .flatten()
        .next()
        .ok_or("no launch record on the chosen node")?;
    let record: Value = serde_json::from_slice(&std::fs::read(record.path())?)?;
    assert_eq!(record["class"], "Harness", "{record}");
    let pid = record["pid"].as_u64().ok_or("the record names no pid")?;
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()?
        .success();
    assert!(alive, "the harness runs on the chosen node");
    assert!(
        !other
            .state()
            .join("runs")
            .join("records")
            .join(&attempt)
            .exists(),
        "nothing of the attempt runs on the other node"
    );
    // Cancelled from the other node: the live attempt is stopped.
    let cancelled = answer(&b.ok(&[
        "facade",
        "cancel",
        "--authority",
        &a.id,
        "--token",
        &token,
        &job,
        "--operation-key",
        "cancel-run",
    ])?)?;
    assert_eq!(cancelled["type"], "accepted", "{cancelled}");
    attempt_is(&b, &a, &token, &job, "cancelled")?;
    Ok(())
}

#[test]
fn e2e_a_create_accepted_before_a_crash_is_placed_once_after_the_restart() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    a.start(Some("before_placement"))?;
    b.start(None)?;
    pair(&b, &a)?;
    let token = bearer(&a.ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?)?;
    let job = submit(&b, &a, &token, "submit-crash", &[])?;
    let started = Instant::now();
    while !a.signal().exists() {
        if started.elapsed() > PLACED_WITHIN {
            return Err(format!("the fault point was not reached:\n{}", a.log()).into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Killed after the commit and before any decision: the submit was
    // answered `accepted`, and nothing was placed.
    a.kill()?;
    a.start(None)?;
    let inspection = placed(&b, &a, &token, &job)?;
    assert_eq!(
        inspection["placements"].as_array().map(Vec::len),
        Some(1),
        "placed once: {inspection}"
    );
    assert_eq!(
        inspection["job"]["attempts"].as_array().map(Vec::len),
        Some(1)
    );
    // A later commit does not place it again.
    a.ok(&[
        "managed",
        "note",
        &job,
        "--question",
        "still one?",
        "--operation-key",
        "note-1",
    ])?;
    std::thread::sleep(Duration::from_millis(300));
    let again = placed(&b, &a, &token, &job)?;
    assert_eq!(
        again["placements"].as_array().map(Vec::len),
        Some(1),
        "{again}"
    );
    Ok(())
}
