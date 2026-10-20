//! The ACP client against a fake agent process, and the ACP adapter's
//! capture, restore and launch round trip onto a node with the same home
//! and workspace path but none of the agent's state.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::env::EnvVarName;
use fabric_core::harness::ProgramName;
use fabric_harness::acp::AcpHarness;
use fabric_harness::acp::client::{PermissionPolicy, StopReason};
use fabric_harness::{Harness, HarnessError, Io, Launch, SessionId};
use serde_json::Value;

const AGENT: &str = env!("CARGO_BIN_EXE_fake-acp-agent");
const TIMEOUT: Duration = Duration::from_secs(20);

fn agent_dir() -> OsString {
    Path::new(AGENT)
        .parent()
        .map(|p| p.as_os_str().to_owned())
        .unwrap_or_default()
}

fn program() -> ProgramName {
    let name = Path::new(AGENT)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("fake-acp-agent");
    ProgramName::parse(name).unwrap()
}

fn env(state_dir: &Path, extra: &[(&str, &str)]) -> Vec<(EnvVarName, String)> {
    let mut vars = vec![
        (
            EnvVarName::parse("FAKE_STATE_DIR").unwrap(),
            state_dir.to_string_lossy().into_owned(),
        ),
        (
            EnvVarName::parse("PATH").unwrap(),
            agent_dir().to_string_lossy().into_owned(),
        ),
    ];
    for (k, v) in extra {
        vars.push((EnvVarName::parse(k).unwrap(), (*v).to_owned()));
    }
    vars
}

fn adapter(home: &Path, policy: PermissionPolicy) -> AcpHarness {
    AcpHarness::new(program(), vec![], home.to_path_buf())
        .with_state_dir(home.join(".fake-agent/sessions"))
        .unwrap()
        .with_policy(policy)
        .with_path_var(agent_dir())
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
            Launch::New {
                prompt: "first".to_owned(),
            },
            &env(&state_dir, &[("FAKE_ASK_PERMISSION", "1")]),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let client = started.client.clone();
    let session = started.running.session().cloned().unwrap();
    assert_eq!(
        client.capabilities().unwrap().name.as_deref(),
        Some("fake-acp-agent")
    );

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
            Launch::New {
                prompt: "go".to_owned(),
            },
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
            Launch::New {
                prompt: "x".to_owned(),
            },
            Io::Terminal,
            &env,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::NeedsClient { .. }), "{err}");
    let mut running = adapter
        .launch(
            &work,
            Launch::New {
                prompt: "headless".to_owned(),
            },
            Io::Piped,
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
        .launch_acp(
            &work,
            Launch::New {
                prompt: "start".to_owned(),
            },
            &env,
        )
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
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(
        manifest.files[0].rel_path.as_str(),
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
            Launch::Resume {
                session: session.clone(),
                prompt: Some("after move".to_owned()),
            },
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
async fn capture_without_a_declared_state_dir_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let adapter =
        AcpHarness::new(program(), vec![], home.path().to_path_buf()).with_path_var(agent_dir());
    let out = tempfile::tempdir().unwrap();
    let err = adapter
        .capture(&work, &SessionId::parse("fake-1").unwrap(), out.path())
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::StateDirNotDeclared { .. }),
        "{err}"
    );
    let err = AcpHarness::new(program(), vec![], home.path().to_path_buf())
        .with_state_dir(PathBuf::from("/elsewhere"))
        .unwrap_err();
    assert!(matches!(err, HarnessError::NotUnderHome { .. }), "{err}");
}
