//! M5 acceptance for the job facade (ADR 0027, docs/facade-protocol.md)
//! through real daemons: a job submitted on one node is observed from
//! another node of the same owner over the node link; a watch's reader is
//! killed and resumes from its cursor with every event delivered once; a
//! token out of scope is refused before the authority is asked; and a
//! socket-only operation on a managed job is refused as `managed_job`,
//! the explicit answer a legacy client gets.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::world::{Env, Fallible, Node, Wire, finish, pair};

/// The `token <id>` and `bearer <hex>` lines `fabric facade mint` prints.
fn minted(text: &str) -> Fallible<(String, String)> {
    let mut id = None;
    let mut bearer = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("token ") {
            id = Some(rest.trim().to_owned());
        }
        if let Some(rest) = line.strip_prefix("bearer ") {
            bearer = rest.split_whitespace().next().map(str::to_owned);
        }
    }
    Ok((id.ok_or("no token line")?, bearer.ok_or("no bearer line")?))
}

/// The JSON a facade command printed: the authority's answer.
fn answer(text: &str) -> Fallible<Value> {
    let start = text
        .find('{')
        .ok_or_else(|| format!("no JSON in:\n{text}"))?;
    serde_json::from_str(&text[start..]).map_err(|e| format!("{e} in:\n{text}").into())
}

/// The job id `fabric managed create` printed: its first line.
fn created_job(text: &str) -> Fallible<String> {
    text.lines()
        .find(|line| line.parse::<fabric_core::id::JobId>().is_ok())
        .map(str::to_owned)
        .ok_or_else(|| format!("no job id in:\n{text}").into())
}

fn seqs(events: &Value) -> Vec<u64> {
    events["events"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|e| e["seq"].as_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Two daemons, the job's authority `a` and the other node `b`, paired, with
/// a token minted on `a` and a managed job made there.
fn world(env: &std::sync::Arc<Env>) -> Fallible<(Node, Node, String, String, String)> {
    let mut a = Node::new(env, "a", Wire::TcpTls)?;
    let mut b = Node::new(env, "b", Wire::TcpTls)?;
    a.start(None)?;
    b.start(None)?;
    pair(&b, &a)?;
    let job = created_job(&a.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "watch me from elsewhere",
        "--harness",
        "codex",
        "--operation-key",
        "create-1",
    ])?)?;
    let (token_id, bearer) =
        minted(&a.ok(&["facade", "mint", "--actor", "e2e", "--ttl-ms", "600000"])?)?;
    Ok((a, b, job, token_id, bearer))
}

#[test]
fn e2e_the_same_job_is_observed_after_a_reconnect_from_another_node() -> Fallible {
    let env = Env::new()?;
    let (a, b, job, token_id, bearer) = world(&env)?;
    let authority = a.id.clone();
    let watch = |after: Option<&str>, wait: &str| -> Fallible<Value> {
        let mut args = vec![
            "facade",
            "watch",
            "--authority",
            &authority,
            "--token",
            &bearer,
            &job,
            "--wait-ms",
            wait,
        ];
        if let Some(after) = after {
            args.extend(["--after", after]);
        }
        answer(&b.ok(&args)?)
    };

    // From the other node, the job's whole history so far, in order.
    let first = watch(None, "0")?;
    assert_eq!(first["type"], "events", "{first}");
    assert_eq!(first["lagged"], false);
    let seen = seqs(&first);
    assert!(!seen.is_empty(), "{first}");
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert_eq!(first["events"][0]["event"]["kind"], "command_accepted");
    let cursor = first["next"].as_str().ok_or("next")?.to_owned();

    // An inspection from the other node is the authority's object.
    let inspection = answer(&b.ok(&[
        "facade",
        "inspect",
        "--authority",
        &authority,
        "--token",
        &bearer,
        &job,
    ])?)?;
    assert_eq!(inspection["type"], "inspection");
    assert_eq!(inspection["job"]["job"], job);
    assert_eq!(inspection["job"]["tasks"][0]["class"], "harness");
    assert_eq!(inspection["job"]["goal"], "watch me from elsewhere");

    // A reader killed mid-wait loses nothing: the next read resumes from its
    // cursor and gets exactly the events the authority recorded since.
    let mut waiting = b.spawn(&[
        "facade",
        "watch",
        "--authority",
        &authority,
        "--token",
        &bearer,
        &job,
        "--after",
        &cursor,
        "--wait-ms",
        "20000",
    ])?;
    std::thread::sleep(Duration::from_millis(300));
    waiting.kill()?;
    let _ = finish(waiting, Duration::from_secs(5));
    a.ok(&[
        "managed",
        "note",
        &job,
        "--question",
        "still here?",
        "--operation-key",
        "note-1",
    ])?;
    let resumed = watch(Some(&cursor), "0")?;
    let later = seqs(&resumed);
    assert!(!later.is_empty(), "{resumed}");
    assert!(
        later.iter().all(|seq| *seq > seen[seen.len() - 1]),
        "nothing twice: {seen:?} then {later:?}"
    );
    assert!(
        resumed["events"]
            .as_array()
            .ok_or("events")?
            .iter()
            .any(|e| e["event"]["kind"] == "note_recorded"),
        "{resumed}"
    );
    let next = resumed["next"].as_str().ok_or("next")?.to_owned();

    // A watch that waits returns within its wait when nothing happens.
    let started = Instant::now();
    let quiet = watch(Some(&next), "500")?;
    assert!(seqs(&quiet).is_empty(), "{quiet}");
    assert_eq!(quiet["next"], next);
    assert!(started.elapsed() < Duration::from_secs(20));

    // The token revoked on the authority: the next message from the other
    // node is refused, and the CLI says so without a stack trace.
    a.ok(&["facade", "revoke", &token_id])?;
    let refused = b.fails(&[
        "facade",
        "inspect",
        "--authority",
        &authority,
        "--token",
        &bearer,
        &job,
    ])?;
    assert!(refused.contains("unauthorized/revoked"), "{refused}");
    Ok(())
}

#[test]
fn e2e_a_token_out_of_scope_is_refused_before_the_authority_is_asked() -> Fallible {
    let env = Env::new()?;
    let (a, b, job, _token_id, _bearer) = world(&env)?;
    let other = "01K8W3A00000000000000000J2";
    let (_, scoped) = minted(&a.ok(&[
        "facade", "mint", "--actor", "narrow", "--job", &job, "--ttl-ms", "600000",
    ])?)?;
    let refused = b.fails(&[
        "facade",
        "inspect",
        "--authority",
        &a.id,
        "--token",
        &scoped,
        other,
    ])?;
    assert!(refused.contains("scope/job"), "{refused}");
    let own = answer(&b.ok(&[
        "facade",
        "inspect",
        "--authority",
        &a.id,
        "--token",
        &scoped,
        &job,
    ])?)?;
    assert_eq!(own["type"], "inspection");
    Ok(())
}

#[test]
fn e2e_a_socket_only_op_on_a_managed_job_is_refused_as_managed_job() -> Fallible {
    let env = Env::new()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    a.start(None)?;
    let job = created_job(&a.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "a managed job",
        "--harness",
        "codex",
        "--operation-key",
        "create-1",
    ])?)?;
    for args in [
        vec!["job", "status", job.as_str()],
        vec!["job", "suspend", job.as_str()],
        vec!["job", "log", job.as_str()],
    ] {
        let refused = a.fails(&args)?;
        assert!(
            refused.contains("managed"),
            "`fabric {}` did not name the managed job:\n{refused}",
            args.join(" ")
        );
    }
    // The managed read surface shows it, with the placement decision slot.
    let inspected = a.ok(&["managed", "inspect", &job])?;
    assert!(inspected.contains(&job), "{inspected}");
    Ok(())
}
