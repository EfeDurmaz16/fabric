//! M5 acceptance for secrets through the launch path (ADR 0025): a job's
//! secret reaches the processes of its attempt by name and is held in no
//! journal, snapshot, work package, receipt or log on the node.

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use fabric_client::Client;
use fabric_core::id::JobId;

use crate::world::{Env, Fallible, Node, Wire};

/// A value no fixture or default holds, so a match is the secret itself.
const VALUE: &str = "s3cr3t-value-7f1c9a2e-only-in-the-daemon-env";

/// Every file under `dir` whose bytes hold `needle`.
fn holding(dir: &Path, needle: &[u8], out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            holding(&path, needle, out);
        } else if let Ok(bytes) = std::fs::read(&path)
            && bytes.windows(needle.len()).any(|w| w == needle)
        {
            out.push(path.display().to_string());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "the scenario reads best as one script"
)]
async fn e2e_a_secret_value_appears_in_no_journal_snapshot_package_receipt_or_log() -> Fallible {
    let env = Env::new()?;
    env.install_fake_codex()?;
    let mut node = Node::new(&env, "a", Wire::TcpTls)?;
    node.daemon_env("FAKE_CODEX_HOLD", OsString::from("1"));
    node.daemon_env("FABRIC_SECRET_DEPLOY_KEY", OsString::from(VALUE));
    node.start(None)?;
    let client = Client::new(node.state().join("fabric.sock"));
    let made = node.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "deploy it",
        "--harness",
        "codex",
        "--secret",
        "DEPLOY_KEY",
        "--drain-budget",
        "1000",
        "--operation-key",
        "make-1",
    ])?;
    let job: JobId = made.lines().next().ok_or("nothing printed")?.parse()?;
    let primary = made
        .lines()
        .find_map(|l| l.strip_prefix("task ")?.strip_suffix(" (primary): open"))
        .ok_or_else(|| format!("no primary task in {made}"))?
        .to_owned();
    let wait = |what: &'static str, done: fn(&fabric_client::api::Inspection) -> bool| {
        let client = &client;
        let node = &node;
        async move {
            let started = Instant::now();
            loop {
                let inspection = client.managed_inspect(job, |_| {}).await?;
                if done(&inspection) {
                    return Ok::<_, Box<dyn std::error::Error>>(inspection);
                }
                if started.elapsed() > Duration::from_secs(40) {
                    return Err(format!("not {what}: {inspection:?}\n{}", node.log()).into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    wait("running", |i| {
        i.job.attempts.last().is_some_and(|a| a.status == "running")
    })
    .await?;
    // A test that passes only when the secret reached it by its name.
    node.ok(&[
        "managed",
        "register",
        &job.to_string(),
        "--parent",
        &primary,
        "--label",
        "has the key",
        "--class",
        "test",
        "--operation-key",
        "register-1",
        "--",
        "sh",
        "-c",
        "test -n \"$DEPLOY_KEY\" && sleep 30",
    ])?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    node.ok(&[
        "managed",
        "checkpoint",
        &job.to_string(),
        "--operation-key",
        "cp-1",
    ])?;
    wait("cut", |i| {
        i.job.tasks.iter().any(|t| {
            t.receipt
                .as_ref()
                .is_some_and(|r| r.status == "checkpointed")
        })
    })
    .await?;
    node.ok(&[
        "managed",
        "continue",
        &job.to_string(),
        "--operation-key",
        "continue-1",
    ])?;
    let continued = wait("continued", |i| !i.continuations.is_empty()).await?;
    assert!(
        continued.continuations[0].package.is_some(),
        "{continued:?}"
    );
    // The test was running with the key when the cut interrupted it.
    let test = continued
        .job
        .tasks
        .iter()
        .find(|t| t.label.as_deref() == Some("has the key"))
        .ok_or("no test task")?;
    assert_eq!(
        test.receipt.as_ref().map(|r| r.status.as_str()),
        Some("interrupted"),
        "the test saw the key and ran until the cut: {continued:?}"
    );
    let inspection = client.managed_inspect(job, |_| {}).await?;
    node.ok(&[
        "managed",
        "cancel",
        &job.to_string(),
        "--operation-key",
        "cancel-1",
    ])?;

    let mut found = Vec::new();
    holding(&node.state().join("managed"), VALUE.as_bytes(), &mut found);
    holding(&node.state().join("store"), VALUE.as_bytes(), &mut found);
    holding(&node.state().join("runs"), VALUE.as_bytes(), &mut found);
    holding(
        &env.home.join("fabric").join("jobs"),
        VALUE.as_bytes(),
        &mut found,
    );
    assert!(found.is_empty(), "the secret value is held in {found:?}");
    assert!(
        !node.log().contains(VALUE),
        "the daemon's log names the value"
    );
    assert!(
        !format!("{inspection:?}").contains(VALUE),
        "no receipt or inspection holds the value"
    );
    Ok(())
}
