//! OpenCode: a job under `--harness opencode` is suspended on one node and
//! resumed on the other with a fake `opencode` that keeps the agent's real
//! on-disk layout (`crates/fabric-harness/tests/fixtures/opencode/`), and
//! the refusals of docs/harness-adapters.md hold. The fake is installed
//! from here through `Env::install`, so `world.rs` stays untouched.

use std::fs;
use std::path::Path;

use crate::world::{Env, Fallible, Wire};

/// The fake `opencode`, shared with the adapter's own roundtrip test.
const FAKE: &str = include_str!("../../../fabric-harness/tests/fixtures/opencode/fake-opencode.sh");

/// The session the fake mints for a first run.
const SESSION: &str = "ses_0fabric0opencode0session001";

/// The login token the fake keeps in its database, which must never travel.
const DB_TOKEN: &str = "sk-ant-FAKEdatabaseTOKEN0123456789";

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

/// Forgets every OpenCode session in the shared home, as the second
/// machine never had them; the database and the login stay.
fn forget_sessions(env: &Env) -> Fallible {
    let data = env.home.join(".local/share/opencode");
    for dir in ["fake-rows", "fabric"] {
        if data.join(dir).exists() {
            fs::remove_dir_all(data.join(dir))?;
        }
    }
    Ok(())
}

/// A job on `a` whose first headless turn ran, suspended there and moved
/// to `b`, with the session gone from the shared home.
fn moved_job(p: &crate::Pair) -> Fallible<String> {
    p.env.install("opencode", FAKE)?;
    let job = crate::new_job(&p.env, &p.a, "opencode")?;
    let ran = p.a.ok(&[
        "job",
        "run",
        &job,
        "--headless",
        "--prompt",
        "the first turn: name the greeting file",
    ])?;
    assert!(
        ran.contains(&format!("opencode session {SESSION}")),
        "{ran}"
    );
    let suspended = p.a.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured opencode session {SESSION} (1 files)")),
        "{suspended}"
    );
    let snapshot = everything_under(&p.a.state().join("jobs").join(&job).join("harness-state"))?;
    assert!(snapshot.contains("name the greeting file"), "{snapshot}");
    assert!(!snapshot.contains(DB_TOKEN), "the database travelled");
    assert!(
        !snapshot.contains("sk-ant-FAKEauthJSON"),
        "auth.json travelled"
    );
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    forget_sessions(&p.env)?;
    Ok(job)
}

/// The session travels as OpenCode's own export, never its database, and
/// the target imports it under the same id: a question about the first
/// turn is answered there from the imported transcript, the model the
/// session ran with is passed again, and the node records the session it
/// continues, which the next capture takes.
#[test]
fn e2e_an_opencode_job_moves_and_resumes_without_its_database() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    let job = moved_job(&p)?;
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
        resumed.contains(&format!("opencode session {SESSION}")),
        "{resumed}"
    );
    let args = p.env.fake_args("opencode")?;
    assert_eq!(
        args,
        [
            "run",
            "--format",
            "json",
            "-s",
            SESSION,
            "-m",
            "anthropic/claude-fake-5",
            "--agent",
            "build",
            "--variant",
            "high",
            "--",
            "what did the first turn ask?"
        ]
    );
    let log = p.b.log();
    assert!(
        log.contains("opencode: restored the session") && log.contains("fidelity=native_session"),
        "{log}"
    );
    let again = p.b.ok(&["job", "suspend", &job])?;
    assert!(
        again.contains(&format!("captured opencode session {SESSION} (1 files)")),
        "{again}"
    );
    assert!(
        !everything_under(&p.b.state().join("jobs").join(&job).join("harness-state"))?
            .contains(DB_TOKEN)
    );
    Ok(())
}

/// An OpenCode whose import mints a new id continues the conversation in
/// that session: the node says so with `new_native_session_with_package`,
/// never `native_session`, records the new id, and the resume and the next
/// capture take it.
#[test]
fn e2e_an_opencode_import_that_mints_an_id_is_recorded_as_a_new_session() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    let job = moved_job(&p)?;
    fs::write(p.env.home.join(".local/share/opencode/fake-new-id"), "")?;
    let minted = "ses_0fabric0minted0by0import1";
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
        resumed.contains(&format!("opencode session {minted}")),
        "{resumed}"
    );
    let log = p.b.log();
    assert!(
        log.contains(&format!("continues={minted}"))
            && log.contains("fidelity=new_native_session_with_package"),
        "{log}"
    );
    assert!(!log.contains("fidelity=native_session"), "{log}");
    let again = p.b.ok(&["job", "suspend", &job])?;
    assert!(
        again.contains(&format!("captured opencode session {minted} (1 files)")),
        "{again}"
    );
    Ok(())
}

/// A session whose transcript holds a secret shape is not captured: the
/// suspend says why, and nothing of the session reaches the snapshot.
#[test]
fn e2e_an_opencode_session_that_holds_a_secret_is_refused() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    p.env.install("opencode", FAKE)?;
    let job = crate::new_job(&p.env, &p.a, "opencode")?;
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
