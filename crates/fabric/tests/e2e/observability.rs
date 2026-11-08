//! Row 12 of the 2026-11-08 roadmap, Fabric side (ADR 0031): spans out of
//! two real daemons into one OTLP receiver, `inspect --json` and `audit
//! export` after a scripted run.
//!
//! The authority `a` places a job on `b`, a build task with a marker in
//! its argv is registered, the attempt is cut, and a continue to `a`
//! regrants it at generation 2. Both daemons export to a receiver this
//! test runs, `a` through `--otlp-endpoint`'s fallback variable. The spans
//! of both nodes share the first attempt's trace, the regrant names the
//! generation it supersedes, and no span holds the job's secret value, the
//! goal, the argv marker, an operation key or a home path. The audit
//! export of `a` holds each table's rows and none of those either.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fabric_client::Client;
use fabric_client::api::Inspection;
use fabric_core::id::JobId;
use serde_json::Value;

use crate::world::{Env, Fallible, Node, Wire, finish, pair};

const WITHIN: Duration = Duration::from_secs(40);
const VALUE: &str = "s3cr3t-value-7c1e-only-in-the-daemon-env";
const GOAL: &str = "trace the launch path 4b9d";
const MARK: &str = "argv-marker-e51f";

/// An OTLP/HTTP receiver: the body of every post, answered 200.
fn receiver() -> Fallible<(String, Arc<Mutex<Vec<Value>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}", listener.local_addr()?);
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut bytes = Vec::new();
            let mut buf = [0_u8; 8192];
            let body = loop {
                let Ok(n) = stream.read(&mut buf) else {
                    break None;
                };
                if n == 0 {
                    break None;
                }
                bytes.extend_from_slice(&buf[..n]);
                let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&bytes[..end]).to_string();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + len {
                    break Some(bytes[end + 4..end + 4 + len].to_vec());
                }
            };
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            if let Some(body) = body.and_then(|b| serde_json::from_slice(&b).ok()) {
                if let Ok(mut all) = seen.lock() {
                    all.push(body);
                }
            }
        }
    });
    Ok((url, bodies))
}

async fn until(
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
            return Err(format!("not {what}: {inspection:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn answer(text: &str) -> Fallible<Value> {
    let start = text.find('{').ok_or("no JSON in the answer")?;
    Ok(serde_json::from_str(&text[start..])?)
}

fn attr<'a>(span: &'a Value, key: &str) -> Option<&'a str> {
    span["attributes"].as_array()?.iter().find_map(|a| {
        (a["key"] == key).then(|| {
            a["value"]["stringValue"]
                .as_str()
                .or(a["value"]["intValue"].as_str())
        })?
    })
}

/// Every span received, each with the node that sent it.
fn spans(bodies: &Arc<Mutex<Vec<Value>>>) -> Fallible<Vec<(String, Value)>> {
    let bodies = bodies.lock().map_err(|_| "the receiver panicked")?;
    let mut out = Vec::new();
    for body in bodies.iter() {
        for resource in body["resourceSpans"].as_array().ok_or("no resourceSpans")? {
            let node = resource["resource"]["attributes"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["key"] == "service.instance.id"))
                .and_then(|a| a["value"]["stringValue"].as_str())
                .ok_or("no service.instance.id")?
                .to_owned();
            for scope in resource["scopeSpans"].as_array().ok_or("no scopeSpans")? {
                for span in scope["spans"].as_array().ok_or("no spans")? {
                    out.push((node.clone(), span.clone()));
                }
            }
        }
    }
    Ok(out)
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "the run, the spans and the export read best as one script"
)]
async fn e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let (url, bodies) = receiver()?;
    let mut a = Node::new(&env, "a", Wire::TcpTls)?;
    let mut b = Node::new(&env, "b", Wire::TcpTls)?;
    let home_a = env.root.join("home-a");
    let home_b = env.root.join("home-b");
    for (node, home) in [(&mut a, &home_a), (&mut b, &home_b)] {
        std::fs::create_dir_all(home)?;
        node.daemon_env("HOME", home.clone().into_os_string());
        node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
        node.daemon_env("FABRIC_SECRET_DEPLOY_KEY", OsString::from(VALUE));
        node.daemon_env("OTEL_EXPORTER_OTLP_ENDPOINT", OsString::from(&url));
    }
    a.start(None)?;
    b.start(None)?;
    pair(&b, &a)?;
    let client = Client::new(a.state().join("fabric.sock"));

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
        GOAL,
        "--harness",
        "codex",
        "--secret",
        "DEPLOY_KEY",
        "--drain-budget",
        "1000",
        "--pin",
        &b.id,
        "--operation-key",
        "submit-otel-1",
    ])?)?;
    let job: JobId = accepted["job"].as_str().ok_or("no job")?.parse()?;
    let job_text = job.to_string();
    let first = match until(&client, job, "running", |i| {
        i.job.attempts.last().is_some_and(|x| x.status == "running")
    })
    .await
    {
        Ok(first) => first,
        Err(e) => return Err(format!("{e}\na:\n{}\nb:\n{}", a.log(), b.log()).into()),
    };
    let from = first.job.attempts[0].attempt;
    let primary = first.job.primary_task.to_string();
    a.ok(&[
        "managed",
        "register",
        &job_text,
        "--parent",
        &primary,
        "--label",
        "build",
        "--class",
        "build",
        "--operation-key",
        "register-otel-1",
        "--",
        "sh",
        "-c",
        &format!("echo {MARK} && sleep 30"),
    ])?;
    a.ok(&[
        "managed",
        "checkpoint",
        &job_text,
        "--operation-key",
        "checkpoint-otel-1",
    ])?;
    until(&client, job, "checkpointed", |i| {
        i.job
            .tasks
            .iter()
            .find(|t| t.task == i.job.primary_task)
            .and_then(|t| t.receipt.as_ref())
            .is_some_and(|r| r.status == "checkpointed")
    })
    .await?;
    a.ok(&[
        "managed",
        "continue",
        &job_text,
        "--pin",
        &a.id,
        "--operation-key",
        "continue-otel-1",
    ])?;
    let after = until(&client, job, "continued", |i| {
        i.job
            .attempts
            .iter()
            .any(|x| x.attempt != from && x.status == "running")
    })
    .await?;
    let second = after
        .job
        .attempts
        .iter()
        .find(|x| x.attempt != from)
        .ok_or("no second attempt")?
        .attempt;

    // inspect --json: the documented schema, for the job and for an attempt.
    let whole: Value = serde_json::from_str(&a.ok(&["inspect", &job_text, "--json"])?)?;
    let report = &whole["inspection"];
    assert_eq!(report["schema"], 1, "{whole}");
    let generations = report["generations"].as_array().ok_or("no generations")?;
    assert_eq!(generations.len(), 2, "{whole}");
    assert_eq!(generations[1]["supersedes"], 1, "{whole}");
    assert_eq!(generations[1]["granted_by_kind"], "continue", "{whole}");
    assert_eq!(report["attempts"][0]["node"], b.id.as_str(), "{whole}");
    assert!(
        report["operations"]
            .as_array()
            .is_some_and(|o| o.len() >= 4),
        "{whole}"
    );
    assert!(
        report["evidence"].as_array().is_some_and(|e| !e.is_empty()),
        "{whole}"
    );
    assert!(report["last_report"]["seq"].is_u64(), "{whole}");
    let one: Value = serde_json::from_str(&a.ok(&[
        "inspect",
        &job_text,
        "--attempt",
        &from.to_string(),
        "--json",
    ])?)?;
    let attempts = one["inspection"]["attempts"]
        .as_array()
        .ok_or("no attempts")?;
    assert_eq!(attempts.len(), 1, "{one}");
    assert_eq!(attempts[0]["attempt"], from.to_string().as_str(), "{one}");

    // audit export: JSON lines, the same rows as the --json document.
    let lines = a.ok(&["audit", "export"])?;
    let records: Vec<Value> = lines
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let doc: Value = serde_json::from_str(&a.ok(&["audit", "export", "--json"])?)?;
    assert_eq!(
        doc["records"].as_array(),
        Some(&records),
        "--json wraps the same rows"
    );
    for kind in ["operation", "event", "evidence"] {
        assert!(records.iter().any(|r| r["type"] == kind), "no {kind} row");
    }
    let mut last = (0, 0);
    for record in &records {
        let at = record["at_ms"].as_u64().ok_or("no at_ms")?;
        let rank = ["operation", "event", "evidence", "provider_operation"]
            .iter()
            .position(|k| record["type"] == *k)
            .ok_or("an unknown row")?;
        assert!((at, rank) >= last, "out of order: {record}");
        last = (at, rank);
    }
    let events = records.iter().filter(|r| r["type"] == "event").count();
    assert!(events > 5, "{events} events");
    assert!(records.iter().all(|r| r.get("body").is_none()));
    // A window after the run is empty.
    assert_eq!(a.ok(&["audit", "export", "--since", "2999-01-01"])?, "");

    let mut daemons = Vec::new();
    daemons.push(a.stop()?);
    daemons.push(b.stop()?);
    for daemon in daemons {
        finish(daemon, Duration::from_secs(20))?;
    }

    let spans = spans(&bodies)?;
    let exported = serde_json::to_string(&*bodies.lock().map_err(|_| "poisoned")?)?;
    let audited = lines;
    for (what, text) in [("spans", &exported), ("audit export", &audited)] {
        for needle in [
            VALUE,
            GOAL,
            MARK,
            "submit-otel-1",
            "register-otel-1",
            "continue-otel-1",
            home_a.to_str().ok_or("home")?,
            env.root.to_str().ok_or("root")?,
        ] {
            assert!(!text.contains(needle), "{needle} is in the {what}");
        }
    }
    let nodes: std::collections::BTreeSet<&str> = spans.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(nodes, [a.id.as_str(), b.id.as_str()].into_iter().collect());
    // The first attempt's trace holds spans of both daemons: b's launch,
    // its reports, and a's observations of them.
    let launch = spans
        .iter()
        .find(|(n, s)| *n == b.id && s["name"] == "fabric peer.launch")
        .ok_or("no launch span on b")?;
    let trace = launch.1["traceId"].as_str().ok_or("no trace id")?;
    let in_trace: Vec<&(String, Value)> = spans
        .iter()
        .filter(|(_, s)| s["traceId"] == trace)
        .collect();
    assert!(
        in_trace.iter().any(|(n, _)| *n == a.id),
        "a has no span in b's trace"
    );
    assert!(
        in_trace
            .iter()
            .any(|(n, s)| *n == b.id && s["name"] == "fabric report"),
        "b sent no report span"
    );
    let regrant = spans
        .iter()
        .find(|(_, s)| {
            attr(s, "fabric.attempt") == Some(second.to_string().as_str())
                && attr(s, "fabric.generation.supersedes").is_some()
        })
        .ok_or("no span names the regrant")?;
    assert_eq!(attr(&regrant.1, "fabric.generation.supersedes"), Some("1"));
    for name in [
        "fabric managed.register_task",
        "fabric managed.checkpoint",
        "fabric managed.continue",
    ] {
        assert!(spans.iter().any(|(_, s)| s["name"] == name), "no {name}");
    }
    if let Some(dir) = std::env::var_os("FABRIC_E2E_SPANS_OUT") {
        let rows: Vec<Value> = spans
            .iter()
            .map(|(node, s)| {
                serde_json::json!({
                    "node": if *node == a.id { "a" } else { "b" },
                    "name": s["name"], "traceId": s["traceId"], "spanId": s["spanId"],
                    "parentSpanId": s["parentSpanId"], "result": attr(s, "fabric.result"),
                })
            })
            .collect();
        std::fs::write(
            std::path::Path::new(&dir).join("spans-fake-receiver.json"),
            serde_json::to_string_pretty(&rows)? + "\n",
        )?;
    }
    Ok(())
}
