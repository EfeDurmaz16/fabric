//! pi: a job under `--harness pi` is suspended on one node and resumed on
//! the other with a fake `pi` that keeps the agent's real on-disk layout
//! (`crates/fabric-harness/tests/fixtures/pi/`), and the refusals of
//! docs/harness-adapters.md hold. The fake is installed from here through
//! `Env::install`, so `world.rs` stays untouched.

use std::fs;
use std::path::{Path, PathBuf};

use crate::world::{Env, Fallible, Wire};

/// The fake `pi`, shared with the adapter's own roundtrip test.
const FAKE: &str = include_str!("../../../fabric-harness/tests/fixtures/pi/fake-pi.sh");

/// The session the fake mints for the first run in a workspace.
const SESSION: &str = "0199fab0-0000-7000-8000-000000000001";

/// The key the fake's `auth.json` holds, which must never travel.
const AUTH_KEY: &str = "sk-ant-FAKEpiAUTH0123456789abcdef";

/// pi's session file for the job's workspace in the shared home.
fn session_file(env: &Env, job: &str) -> Fallible<PathBuf> {
    let work = env.work(job);
    let key = work
        .to_str()
        .ok_or("the workspace path is not UTF-8")?
        .trim_start_matches('/')
        .replace('/', "-");
    Ok(env
        .home
        .join(".pi/agent/sessions")
        .join(format!("--{key}--"))
        .join(format!("2026-11-08T10-00-00-000Z_{SESSION}.jsonl")))
}

/// Every byte under `dir`, as text.
fn everything_under(dir: &Path) -> Fallible<String> {
    let mut all = String::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            all.push_str(&everything_under(&path)?);
        } else {
            all.push_str(&String::from_utf8_lossy(&fs::read(&path)?));
        }
    }
    Ok(all)
}

/// The session file travels byte for byte and pi continues it on the
/// target by its path: a question about the first turn is answered there
/// from the restored file, the provider, model and thinking level the
/// session ran with are passed again and said, the file only grew, and
/// `auth.json` never left.
#[test]
fn e2e_a_pi_session_moves_and_resumes_as_the_same_file() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    p.env.install("pi", FAKE)?;
    let job = crate::new_job(&p.env, &p.a, "pi")?;
    let ran = p.a.ok(&[
        "job",
        "run",
        &job,
        "--headless",
        "--prompt",
        "the first turn: name the greeting file",
    ])?;
    assert!(ran.contains(&format!("pi session {SESSION}")), "{ran}");
    let suspended = p.a.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured pi session {SESSION} (1 files)")),
        "{suspended}"
    );
    let snapshot = everything_under(&p.a.state().join("jobs").join(&job).join("harness-state"))?;
    assert!(!snapshot.contains(AUTH_KEY), "auth.json travelled");
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;

    let file = session_file(&p.env, &job)?;
    let captured = fs::read(&file)?;
    fs::remove_file(&file)?;
    let resumed = p.b.ok(&[
        "job",
        "resume",
        &job,
        "--headless",
        "--prompt",
        "what did the first turn ask?",
    ])?;
    assert!(
        resumed.contains("the first turn said: the first turn: name the greeting file"),
        "{resumed}"
    );
    assert!(
        resumed.contains(&format!(
            "continuing pi session {SESSION} with model provider anthropic, model claude-fake-5, effort medium (recorded in the session)"
        )),
        "{resumed}"
    );
    let args = p.env.fake_args("pi")?;
    assert_eq!(
        args,
        [
            "-p",
            "--mode",
            "json",
            "--session",
            file.to_str().ok_or("not UTF-8")?,
            "--provider",
            "anthropic",
            "--model",
            "claude-fake-5",
            "--thinking",
            "medium",
            "what did the first turn ask?"
        ]
    );
    assert!(
        fs::read(&file)?.starts_with(&captured),
        "the resumed session is not the captured file grown"
    );
    let log = p.b.log();
    assert!(log.contains("pi: restored the session file"), "{log}");
    Ok(())
}

/// A session whose transcript holds a secret shape is not captured: the
/// suspend says why, and nothing of the session reaches the snapshot.
#[test]
fn e2e_a_pi_session_that_holds_a_secret_is_refused() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    p.env.install("pi", FAKE)?;
    let job = crate::new_job(&p.env, &p.a, "pi")?;
    p.a.ok(&[
        "job",
        "run",
        &job,
        "--headless",
        "--prompt",
        "use sk-ant-FAKEleakedKEY0123456789abcd",
    ])?;
    let refused = p.a.fails(&["job", "suspend", &job])?;
    assert!(refused.contains("holds a secret"), "{refused}");
    let state = p.a.state().join("jobs").join(&job).join("harness-state");
    assert!(
        !state.exists() || !everything_under(&state)?.contains("sk-ant-FAKEleaked"),
        "the secret reached the snapshot"
    );
    Ok(())
}
