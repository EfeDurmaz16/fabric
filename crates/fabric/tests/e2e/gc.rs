//! `fabric gc` after crashed work (ADR 0030, docs/concepts/residuals.md):
//! a node killed with `SIGKILL` mid capture and mid restore, two jobs
//! stolen from it while it is down, and a third job's harness held
//! running as the live attempt. After the restart gc lists what the two
//! superseded tenures left, with their labels, and nothing of the live
//! one; `--yes` removes exactly what it listed; a second gc finds nothing.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::world::{Fallible, Wire, alive, finish, wait_for};
use crate::{AGENT, new_job, setup};

/// How long a command the test cut short may take to give up.
const GIVE_UP: Duration = Duration::from_mins(3);

/// The report `fabric gc` answers under `--json`, which must succeed.
fn gc(node: &crate::world::Node, args: &[&str]) -> Fallible<Value> {
    let mut args = args.to_vec();
    args.extend(["--json"]);
    let output = node.run(&args)?;
    if !output.status.success() {
        return Err(format!(
            "`fabric {}` failed:\n{}\n--- daemon log ---\n{}",
            args.join(" "),
            crate::world::text(&output),
            node.log()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// One residual as (kind, path, job, epoch, node).
type Listed = (String, Option<PathBuf>, String, Option<u64>, Option<String>);

/// What a report lists.
fn listed(report: &Value) -> Vec<Listed> {
    report["residuals"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            (
                r["kind"].as_str().unwrap_or_default().to_owned(),
                r["path"].as_str().map(PathBuf::from),
                r["label"]["job"].as_str().unwrap_or_default().to_owned(),
                r["label"]["epoch"].as_u64(),
                r["label"]["node"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// Two jobs moved from b to a, a killed mid capture of the first and mid
/// restore of the second, then both stolen by b while a is down; a runs
/// again, and knows it no longer owns them.
fn crashed() -> Fallible<(crate::Pair, String, String)> {
    let mut p = setup(Wire::TcpTls, None, None)?;
    // b dials a too, so a job moves from b to a.
    p.b.ok(&["node", "add", &p.a.id, &p.a.addr()])?;
    // Two jobs begin on b and move to a, so a holds each at epoch 2.
    let mut moved = Vec::new();
    for _ in 0..2 {
        let job = new_job(&p.env, &p.b, AGENT)?;
        p.b.ok(&["job", "run", &job, "--prompt", "one"])?;
        p.b.ok(&["job", "move", &job, "--to", &p.a.id])?;
        moved.push(job);
    }
    let (cut, staged) = (moved[0].clone(), moved[1].clone());

    // A suspend on a is killed with its capture written beside the last one.
    p.a.kill()?;
    p.a.start(Some("mid_harness_capture"))?;
    p.a.ok(&["job", "resume", &cut, "--prompt", "two"])?;
    let suspending = p.a.spawn(&["job", "suspend", &cut])?;
    wait_for(&p.a.signal(), "the capture cut short")?;
    p.a.kill()?;
    finish(suspending, GIVE_UP)?;

    // A restore on a is killed before it renames its tree into place.
    let store_signal = p.a.state().join("store-crash-reached");
    p.a.daemon_env(
        "FABRIC_STORE_CRASH_AT",
        OsString::from("restore.before_rename"),
    );
    p.a.daemon_env("FABRIC_STORE_CRASH_SIGNAL", store_signal.clone().into());
    p.a.start(None)?;
    let resuming = p.a.spawn(&["job", "resume", &staged, "--prompt", "two"])?;
    wait_for(&store_signal, "the restore before its rename")?;
    p.a.kill()?;
    finish(resuming, GIVE_UP)?;
    p.a.clear_daemon_env("FABRIC_STORE_CRASH_AT");
    p.a.clear_daemon_env("FABRIC_STORE_CRASH_SIGNAL");

    // b takes both while a is down; a learns it as it starts.
    for job in &moved {
        p.b.ok(&["job", "steal", job, "--force", "--reason", "gc"])?;
    }
    p.a.start(None)?;
    for job in &moved {
        let lease = p.a.lease(job)?;
        assert_eq!((lease.owner.as_str(), lease.epoch), (p.b.id.as_str(), 3));
    }

    Ok((p, cut, staged))
}

#[test]
fn e2e_gc_after_a_kill_lists_and_removes_only_what_superseded_tenures_left() -> Fallible {
    let (p, cut, staged) = crashed()?;
    // The live attempt: a's own job, its harness held running.
    let live = new_job(&p.env, &p.a, AGENT)?;
    let (running, pid) = p.a.run_lingering(&live)?;

    let found = gc(&p.a, &["gc"])?;
    let residuals = listed(&found);
    let next = residuals
        .iter()
        .find(|(kind, ..)| kind == "harness_state_next")
        .ok_or_else(|| format!("no capture cut short in {found:#}"))?;
    assert_eq!(
        (next.2.as_str(), next.3),
        (cut.as_str(), Some(2)),
        "{found:#}"
    );
    let staging = residuals
        .iter()
        .find(|(kind, ..)| kind == "restore_staging")
        .ok_or_else(|| format!("no unfinished restore in {found:#}"))?;
    assert_eq!(
        (staging.2.as_str(), staging.3, staging.4.as_deref()),
        (staged.as_str(), Some(2), Some(p.a.id.as_str())),
        "the staging label names the job, this node and the tenure: {found:#}"
    );
    assert!(
        residuals.iter().all(|(_, _, job, ..)| job != &live),
        "the live job is listed: {found:#}"
    );
    let pids: Vec<i64> = found["residuals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["pid"].as_i64())
        .collect();
    assert!(
        !pids.contains(&i64::from(pid)),
        "the live harness is listed: {found:#}"
    );
    assert!(
        found["live"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|l| l["holds"] == "tenure" && l["label"]["job"] == live.as_str()),
        "{found:#}"
    );
    let paths: BTreeSet<PathBuf> = residuals.iter().filter_map(|r| r.1.clone()).collect();
    assert!(paths.iter().all(|path| path.exists()), "{found:#}");

    let removed = gc(&p.a, &["gc", "--yes"])?;
    assert_eq!(removed["outcome"], "removed", "{removed:#}");
    assert_eq!(listed(&removed), residuals, "--yes takes the listed set");
    assert!(
        removed["residuals"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|r| r["state"] == "removed"),
        "{removed:#}"
    );
    assert!(paths.iter().all(|path| !path.exists()), "{removed:#}");
    assert!(alive(pid), "gc stopped the live harness");

    let again = gc(&p.a, &["gc"])?;
    assert_eq!(again["residuals"], Value::Array(Vec::new()), "{again:#}");
    drop(running);
    Ok(())
}
