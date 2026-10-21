//! The ACP client against a fake agent process, and the ACP adapter's
//! capture, restore and launch round trip onto a node with the same home
//! and workspace path but none of the agent's state. The fake agent is
//! this test binary, started with `AGENT_ROLE` set, so no fixture binary
//! ships with the crate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::env::EnvVarName;
use fabric_core::harness::ProgramName;
use fabric_core::path::AbsPath;
use fabric_harness::acp::AcpHarness;
use fabric_harness::acp::client::{PermissionPolicy, StopReason};
use fabric_harness::{Harness, HarnessError, Launch, SessionId};
use serde_json::Value;

mod fake_acp_agent;

/// The program name the fake agent is installed under.
const AGENT: &str = "fake-acp-agent";
/// Set in the fake agent's environment; this binary then serves as the agent.
const AGENT_ROLE: &str = "FABRIC_FAKE_ACP_AGENT";
const TIMEOUT: Duration = Duration::from_secs(20);

/// The fake agent's entry point: a no-op unless the adapter started this
/// binary as the agent. The test harness has already printed its header
/// and `test fake_acp_agent ... ` with no newline, so a newline ends that
/// line first; the client skips lines that are not JSON-RPC.
#[test]
#[allow(
    clippy::disallowed_methods,
    reason = "the agent exits as soon as it is done: its client may have closed the pipe the test harness would print its result to"
)]
fn fake_acp_agent() {
    if std::env::var_os(AGENT_ROLE).is_some() {
        let mut out = std::io::stdout();
        out.write_all(b"\n").and_then(|()| out.flush()).unwrap();
        let served = fake_acp_agent::serve();
        std::process::exit(i32::from(served.is_err()));
    }
}

fn abs(path: &Path) -> AbsPath {
    AbsPath::parse(path.to_str().unwrap()).unwrap()
}

/// Puts a symlink to this test binary under `home/agent-bin` as `AGENT`
/// and returns that directory as a `PATH` value.
///
/// A symlink, not a hard link or a copy: macOS can scan a program as it
/// starts by opening its file by path, and kills the program when that
/// open fails. With hard links every test's agent was one file under many
/// short-lived names, and the tests that finished removed theirs while
/// other agents started, so an agent could be killed before it read a line
/// and its launch saw a closed connection. Through a symlink every agent
/// runs from the test binary's own path, which lasts the whole run, and
/// nothing is opened for writing.
fn agent_dir(home: &Path) -> OsString {
    let bin = home.join("agent-bin");
    let agent = bin.join(AGENT);
    if !agent.exists() {
        std::fs::create_dir_all(&bin).unwrap();
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        std::os::unix::fs::symlink(&exe, &agent).unwrap();
    }
    bin.into_os_string()
}

fn program() -> ProgramName {
    ProgramName::parse(AGENT).unwrap()
}

/// The arguments that make the test binary run only `fake_acp_agent`.
fn agent_args() -> Vec<String> {
    [
        "--exact",
        "fake_acp_agent",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn env(state_dir: &Path, extra: &[(&str, &str)]) -> Vec<(EnvVarName, String)> {
    let mut vars = vec![
        (EnvVarName::parse(AGENT_ROLE).unwrap(), "1".to_owned()),
        (
            EnvVarName::parse("FAKE_STATE_DIR").unwrap(),
            state_dir.to_string_lossy().into_owned(),
        ),
    ];
    for (k, v) in extra {
        vars.push((EnvVarName::parse(k).unwrap(), (*v).to_owned()));
    }
    vars
}

fn adapter(home: &Path, policy: PermissionPolicy) -> AcpHarness {
    AcpHarness::new(program(), agent_args(), abs(home), agent_dir(home))
        .with_state_dir(&abs(&home.join(".fake-agent/sessions")))
        .unwrap()
        .with_policy(policy)
}

fn state(home: &Path, session: &SessionId) -> Value {
    let path = home
        .join(".fake-agent/sessions")
        .join(format!("{session}.json"));
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

#[tokio::test]
async fn client_runs_a_turn_and_denies_permission_by_default() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    let adapter = adapter(home.path(), PermissionPolicy::Deny);

    let mut started = tokio::time::timeout(
        TIMEOUT,
        adapter.launch_acp(
            &work,
            None,
            Some("first".to_owned()),
            &env(&state_dir, &[("FAKE_ASK_PERMISSION", "1")]),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let client = started.client.clone();
    let session = started.running.session().cloned().unwrap();

    let update = tokio::time::timeout(TIMEOUT, client.next_update())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(update.session, session);
    assert_eq!(update.update["content"]["text"], "echo: first");
    let first_turn = tokio::time::timeout(TIMEOUT, started.turn.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first_turn, StopReason::EndTurn);

    let reason = tokio::time::timeout(TIMEOUT, client.prompt(&session, "second"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reason, StopReason::EndTurn);
    client.cancel(&session).unwrap();
    drop(client);
    drop(started.client);
    started.running.child_mut().kill().await.ok();

    let recorded = state(home.path(), &session);
    let prompts = recorded["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0]["text"], "first");
    assert_eq!(prompts[0]["permission"]["outcome"]["optionId"], "reject");
    assert_eq!(prompts[1]["permission"]["outcome"]["optionId"], "reject");
    assert_eq!(
        recorded["cwd"].as_str(),
        work.canonicalize().unwrap().to_str()
    );
}

#[tokio::test]
async fn allow_once_policy_picks_the_allow_option() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    let adapter = adapter(home.path(), PermissionPolicy::AllowOnce);
    let started = adapter
        .launch_acp(
            &work,
            None,
            Some("go".to_owned()),
            &env(&state_dir, &[("FAKE_ASK_PERMISSION", "1")]),
        )
        .await
        .unwrap();
    let session = started.running.session().cloned().unwrap();
    tokio::time::timeout(TIMEOUT, started.client.next_update())
        .await
        .unwrap()
        .unwrap();
    let recorded = state(home.path(), &session);
    assert_eq!(
        recorded["prompts"][0]["permission"]["outcome"]["optionId"],
        "allow"
    );
}

#[tokio::test]
async fn the_trait_launch_is_headless_and_owns_the_turn() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    let adapter = adapter(home.path(), PermissionPolicy::Deny);
    let env = env(&state_dir, &[]);
    let err = adapter
        .launch(
            &work,
            Launch::Interactive {
                resume: None,
                prompt: Some("x".to_owned()),
            },
            &env,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::NeedsClient { .. }), "{err}");
    let mut running = adapter
        .launch(
            &work,
            Launch::Headless {
                resume: None,
                prompt: "headless".to_owned(),
            },
            &env,
        )
        .await
        .unwrap();
    let session = running.session().cloned().unwrap();
    // The turn task closes the agent's stdin when the turn ends, so the
    // agent exits on its own and the prompt is on record.
    let status = tokio::time::timeout(TIMEOUT, running.child_mut().wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let recorded = state(home.path(), &session);
    assert_eq!(recorded["prompts"][0]["text"], "headless");
}

#[tokio::test]
async fn a_launched_agent_does_not_inherit_parent_session_markers() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    let env_file = home.path().join("env.txt");
    let env_file_text = env_file.to_string_lossy().into_owned();
    let adapter = adapter(home.path(), PermissionPolicy::Deny);
    let parent = [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_CHILD_SESSION",
        "ANTHROPIC_BASE_URL",
        "CODEX_THREAD_ID",
    ];
    let mut extra: Vec<(&str, &str)> = vec![
        ("FAKE_ENV_FILE", env_file_text.as_str()),
        ("ANTHROPIC_API_KEY", "user-key"),
    ];
    extra.extend(parent.iter().map(|name| {
        let value = if *name == "CLAUDE_CODE_ENTRYPOINT" {
            "claude-desktop"
        } else {
            "1"
        };
        (*name, value)
    }));
    let mut running = adapter
        .launch(
            &work,
            Launch::Headless {
                resume: None,
                prompt: "headless".to_owned(),
            },
            &env(&state_dir, &extra),
        )
        .await
        .unwrap();
    let status = tokio::time::timeout(TIMEOUT, running.child_mut().wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let seen = std::fs::read_to_string(&env_file).unwrap();
    let names: Vec<&str> = seen.lines().collect();
    assert!(names.contains(&"ANTHROPIC_API_KEY"), "{names:?}");
    assert!(names.contains(&AGENT_ROLE), "{names:?}");
    for name in parent {
        assert!(
            !names.contains(&name),
            "{name} belongs to the parent session: {names:?}"
        );
    }
}

async fn round_trip(load: &str, resume: &str) -> Result<(PathBuf, Value), HarnessError> {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("fabric/jobs/j1/work");
    std::fs::create_dir_all(&work).unwrap();
    let env = env(
        &home.path().join(".fake-agent/sessions"),
        &[("FAKE_LOAD", load), ("FAKE_RESUME", resume)],
    );

    let adapter = adapter(home.path(), PermissionPolicy::Deny);
    let mut started = adapter
        .launch_acp(&work, None, Some("start".to_owned()), &env)
        .await?;
    let session = started.running.session().cloned().unwrap();
    tokio::time::timeout(TIMEOUT, started.client.next_update())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(TIMEOUT, started.turn.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(started.client);
    started.running.child_mut().kill().await.ok();

    // Another session of the same agent must not travel with this one.
    std::fs::write(
        home.path()
            .join(".fake-agent/sessions/fake-other-session.json"),
        "{\"prompts\":[{\"text\":\"private\"}]}",
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter.capture(&work, &session, out.path()).await?;
    assert_eq!(manifest.files().len(), 1);
    assert_eq!(
        manifest.files()[0].rel_path.as_str(),
        format!("home/.fake-agent/sessions/{session}.json")
    );

    // The other node: same home and workspace path, none of the state.
    std::fs::rename(
        home.path().join(".fake-agent"),
        home.path().join(".fake-agent-departed"),
    )
    .unwrap();
    let restored = adapter.restore(&work, out.path()).await?;
    assert_eq!(restored.placed.len(), 1);
    assert!(restored.placed[0].starts_with(home.path().join(".fake-agent/sessions")));
    assert!(
        !home
            .path()
            .join(".fake-agent/sessions/fake-other-session.json")
            .exists()
    );

    let mut started = adapter
        .launch_acp(
            &work,
            Some(session.clone()),
            Some("after move".to_owned()),
            &env,
        )
        .await?;
    let client = started.client.clone();
    let mut saw_echo = false;
    for _ in 0..3 {
        let update = tokio::time::timeout(TIMEOUT, client.next_update())
            .await
            .unwrap()
            .unwrap();
        if update.update["content"]["text"] == "echo: after move" {
            saw_echo = true;
            break;
        }
    }
    assert!(saw_echo, "the prompt sent on resume was not echoed");
    tokio::time::timeout(TIMEOUT, started.turn.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(client);
    drop(started.client);
    started.running.child_mut().kill().await.ok();
    let recorded = state(home.path(), &session);
    Ok((work.canonicalize().unwrap(), recorded))
}

#[tokio::test]
async fn captured_session_continues_on_another_node_through_session_load() {
    let (work_b, recorded) = round_trip("1", "0").await.unwrap();
    assert_eq!(recorded["loaded_from"][0].as_str(), work_b.to_str());
    assert_eq!(recorded["prompts"][0]["text"], "start");
    assert_eq!(recorded["prompts"][1]["text"], "after move");
}

#[tokio::test]
async fn resume_capability_is_preferred_and_its_absence_is_an_error() {
    let (work_b, recorded) = round_trip("0", "1").await.unwrap();
    assert_eq!(recorded["loaded_from"][0].as_str(), work_b.to_str());
    let err = round_trip("0", "0").await.unwrap_err();
    assert!(
        matches!(err, HarnessError::LoadNotSupported { .. }),
        "{err}"
    );
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "pid=", "-p", pid])
        .output()
        .is_ok_and(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
}

#[tokio::test]
async fn a_launch_whose_handshake_fails_leaves_no_agent_running() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let pid_file = home.path().join("agent.pid");
    let pid_text = pid_file.to_string_lossy().into_owned();
    let env = env(
        &home.path().join(".fake-agent/sessions"),
        &[
            ("FAKE_LOAD", "0"),
            ("FAKE_RESUME", "0"),
            ("FAKE_LINGER", "1"),
            ("FAKE_PID_FILE", &pid_text),
        ],
    );
    let err = adapter(home.path(), PermissionPolicy::Deny)
        .launch_acp(&work, Some(SessionId::parse("fake-1").unwrap()), None, &env)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::LoadNotSupported { .. }),
        "{err}"
    );
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while alive(&pid) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let leaked = alive(&pid);
    if leaked {
        // Do not leave the lingering agent behind when the test fails.
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid.as_str()])
            .status();
    }
    assert!(!leaked, "agent {pid} was still running");
}

#[tokio::test]
async fn a_secret_in_the_state_directory_refuses_the_capture() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    std::fs::create_dir_all(&state_dir).unwrap();
    let session = SessionId::parse("fake-9").unwrap();
    std::fs::write(
        state_dir.join("fake-9.json"),
        "{\"prompts\":[{\"text\":\"my key is AKIAABCDEFGHIJKLMNOP\"}]}\n",
    )
    .unwrap();
    let adapter = adapter(home.path(), PermissionPolicy::Deny);
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&work, &session, out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::SecretInState { line: 1, .. }),
        "{err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn a_secret_in_the_launch_arguments_refuses_the_capture() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("fake-9.json"), "{}\n").unwrap();
    let mut args = agent_args();
    args.extend(["--api-key".to_owned(), format!("sk-ant-{}", "a".repeat(24))]);
    let adapter = AcpHarness::new(program(), args, abs(home.path()), agent_dir(home.path()))
        .with_state_dir(&abs(&state_dir))
        .unwrap();
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&work, &SessionId::parse("fake-9").unwrap(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::SecretInArguments { .. }),
        "{err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn a_symlinked_state_directory_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(elsewhere.path().join("fake-9.json"), "{}\n").unwrap();
    std::fs::create_dir_all(home.path().join(".fake-agent")).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    std::os::unix::fs::symlink(elsewhere.path(), &state_dir).unwrap();
    let out = tempfile::tempdir().unwrap();
    let err = adapter(home.path(), PermissionPolicy::Deny)
        .capture(&work, &SessionId::parse("fake-9").unwrap(), out.path())
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SymlinkInState { .. }), "{err}");
    assert!(!out.path().join("manifest.json").exists());
}

/// Writes session `fake-9` with `escaped` as the JSON text of a prompt and
/// asserts that capture refuses it at that line.
async fn assert_state_refused(escaped: &str) {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("fake-9.json"),
        format!("{{\n  \"prompts\": [{{\"text\": \"{escaped}\"}}]\n}}\n"),
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let err = adapter(home.path(), PermissionPolicy::Deny)
        .capture(&work, &SessionId::parse("fake-9").unwrap(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::SecretInState { line: 2, .. }),
        "{escaped}: {err}"
    );
    assert!(!out.path().join("manifest.json").exists());
}

#[tokio::test]
async fn an_env_line_after_an_escaped_newline_in_state_is_refused() {
    assert_state_refused(r"NODE_ENV=dev\nSTRIPE_SECRET_KEY=abcd1234efgh").await;
}

#[tokio::test]
async fn a_value_in_escaped_quotes_in_state_is_refused() {
    assert_state_refused(r#"export DB_PASSWORD=\"hunter22xyz\""#).await;
}

#[tokio::test]
async fn only_files_named_for_the_session_are_captured() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let state_dir = home.path().join(".fake-agent/sessions");
    for name in [
        "s1.json",
        "s10.json",
        "xs1y/a.json",
        "s1/turns.jsonl",
        "s1-backup/b.json",
    ] {
        let path = state_dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}\n").unwrap();
    }
    let out = tempfile::tempdir().unwrap();
    let manifest = adapter(home.path(), PermissionPolicy::Deny)
        .capture(&work, &SessionId::parse("s1").unwrap(), out.path())
        .await
        .unwrap();
    let paths: Vec<&str> = manifest
        .files()
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "home/.fake-agent/sessions/s1.json",
            "home/.fake-agent/sessions/s1/turns.jsonl"
        ]
    );
}

#[tokio::test]
async fn capture_without_a_declared_state_dir_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let adapter = AcpHarness::new(
        program(),
        agent_args(),
        abs(home.path()),
        agent_dir(home.path()),
    );
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&work, &SessionId::parse("fake-1").unwrap(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::StateDirNotDeclared { .. }),
        "{err}"
    );
    let err = AcpHarness::new(
        program(),
        agent_args(),
        abs(home.path()),
        agent_dir(home.path()),
    )
    .with_state_dir(&AbsPath::parse("/elsewhere").unwrap())
    .unwrap_err();
    assert!(matches!(err, HarnessError::NotUnderHome { .. }), "{err}");
}

#[test]
fn a_state_dir_that_overlaps_fabric_state_is_refused() {
    let home = tempfile::tempdir().unwrap();
    for dir in [
        home.path().to_path_buf(),
        home.path().join(".fabric"),
        home.path().join(".fabric/harness/departed"),
        home.path().join(".Fabric/x"),
    ] {
        let err = AcpHarness::new(
            program(),
            agent_args(),
            abs(home.path()),
            agent_dir(home.path()),
        )
        .with_state_dir(&abs(&dir))
        .unwrap_err();
        assert!(
            matches!(err, HarnessError::StateDirHoldsFabric { .. }),
            "{}: {err}",
            dir.display()
        );
    }
    AcpHarness::new(
        program(),
        agent_args(),
        abs(home.path()),
        agent_dir(home.path()),
    )
    .with_state_dir(&abs(&home.path().join(".fabric-agent/sessions")))
    .unwrap();
}
