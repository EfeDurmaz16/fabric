//! pi adapter: a synthetic v3 session file laid out as pi 0.80.3 keeps it
//! is captured byte for byte, restored onto a node with the same home and
//! workspace path that lacks it, under the same path key, and continued
//! there by a fake `pi` through `--session <path>`; the refusals of
//! docs/harness-adapters.md are proved. The fake and the session it starts
//! from are `tests/fixtures/pi/`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::env::EnvVarName;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use fabric_harness::pi::Pi;
use fabric_harness::{Fidelity, Harness, HarnessError, Launch, Origin, SettingName, TurnReport};

mod executable;
use executable::write_executable;

const SESSION: &str = "0199fab0-0000-7000-8000-0000000000aa";
const FILE_NAME: &str = "2026-11-08T10-00-00-000Z_0199fab0-0000-7000-8000-0000000000aa.jsonl";
const FAKE: &str = include_str!("fixtures/pi/fake-pi.sh");
const LINES: &str = include_str!("fixtures/pi/session.jsonl");

struct Node {
    _dir: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
    path: OsString,
}

impl Node {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let work = home.join("fabric/jobs/j1/work");
        std::fs::create_dir_all(&work).unwrap();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("pi"), FAKE.as_bytes());
        let path = OsString::from(format!("{}:/usr/bin:/bin", bin.display()));
        let node = Node {
            _dir: dir,
            home,
            work,
            path,
        };
        node.write(FILE_NAME, &node.fill(LINES));
        std::fs::write(
            node.home.join(".pi/agent/auth.json"),
            "{\"k\":\"sk-ant-FAKEpiAUTH0123456789abcdef\"}",
        )
        .unwrap();
        node
    }

    fn fill(&self, text: &str) -> String {
        text.replace("@SESSION@", SESSION)
            .replace("@WORK@", self.work.to_str().unwrap())
    }

    /// pi's directory for the workspace: its path without the leading
    /// slash, every `/` as `-`, between `--` and `--`.
    fn sessions(&self) -> PathBuf {
        let key = self
            .work
            .to_str()
            .unwrap()
            .trim_start_matches('/')
            .replace('/', "-");
        self.home
            .join(".pi/agent/sessions")
            .join(format!("--{key}--"))
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::create_dir_all(self.sessions()).unwrap();
        std::fs::write(self.sessions().join(name), text).unwrap();
    }

    fn file(&self) -> PathBuf {
        self.sessions().join(FILE_NAME)
    }

    fn adapter(&self) -> Pi {
        Pi::new(abs(&self.home), self.path.clone())
    }

    fn env(&self) -> Vec<(EnvVarName, String)> {
        [
            ("HOME", self.home.to_str().unwrap().to_owned()),
            ("PATH", self.path.to_str().unwrap().to_owned()),
        ]
        .into_iter()
        .map(|(n, v)| (EnvVarName::parse(n).unwrap(), v))
        .collect()
    }
}

fn abs(path: &Path) -> AbsPath {
    AbsPath::parse(path.to_str().unwrap()).unwrap()
}

fn session() -> SessionId {
    SessionId::parse(SESSION).unwrap()
}

#[tokio::test]
async fn a_session_moves_as_the_same_file_and_continues_from_it() {
    let node = Node::new();
    let out = node.home.join("state");
    let adapter = node.adapter();
    let bytes = std::fs::read(node.file()).unwrap();
    let manifest = adapter.capture(&node.work, &session(), &out).await.unwrap();
    let key = node.sessions();
    let key = key.file_name().unwrap().to_str().unwrap();
    let paths: Vec<&str> = manifest
        .files()
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    assert_eq!(
        paths,
        [format!("home/.pi/agent/sessions/{key}/{FILE_NAME}")]
    );
    assert_eq!(manifest.harness_version(), Some("0.80.3"));
    assert!(!out.join("home/.pi/agent/auth.json").exists());
    adapter.confirm_departure(&manifest).await.unwrap();

    std::fs::remove_dir_all(node.home.join(".pi")).unwrap();
    let restored = adapter.restore(&node.work, &out).await.unwrap();
    assert_eq!(restored.session, session());
    assert_eq!(restored.fidelity, Fidelity::NativeSession);
    assert_eq!(
        std::fs::read(node.file()).unwrap(),
        bytes,
        "the bytes changed"
    );
    // Restoring the same state again finds the file equal and writes nothing.
    adapter.restore(&node.work, &out).await.unwrap();

    let mut running = adapter
        .launch(
            &node.work,
            Launch::Headless {
                resume: Some(session()),
                prompt: "what did the first turn ask?".to_owned(),
            },
            &node.env(),
        )
        .await
        .unwrap();
    assert_eq!(running.session(), Some(&session()));
    let passed: Vec<(SettingName, &str, Origin)> = running
        .settings()
        .passed
        .iter()
        .map(|p| (p.name, p.value.as_str(), p.origin))
        .collect();
    assert_eq!(
        passed,
        [
            (SettingName::Provider, "anthropic", Origin::Session),
            (SettingName::Model, "claude-fake-5", Origin::Session),
            (SettingName::Effort, "high", Origin::Session),
        ]
    );
    running.release().unwrap();
    let child = running.child_mut();
    let mut stdout = child.stdout.take().unwrap();
    let mut said = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout, &mut said)
        .await
        .unwrap();
    assert!(child.wait().await.unwrap().success());
    let mut report = TurnReport::default();
    for line in said.lines() {
        Pi::read_headless_line(&mut report, line);
    }
    assert_eq!(report.session, Some(session()));
    assert_eq!(
        report.answer.as_deref(),
        Some("the first turn said: the first turn: name the greeting file")
    );
    let args = std::fs::read_to_string(node.home.join("pi-args")).unwrap();
    let file = node.file();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "-p",
            "--mode",
            "json",
            "--session",
            file.to_str().unwrap(),
            "--provider",
            "anthropic",
            "--model",
            "claude-fake-5",
            "--thinking",
            "high",
            "what did the first turn ask?"
        ]
    );
    let grown = std::fs::read(node.file()).unwrap();
    assert!(
        grown.starts_with(&bytes),
        "the resume did not append to the same file"
    );
}

#[tokio::test]
async fn a_node_whose_copy_grew_after_the_job_left_is_refused() {
    let node = Node::new();
    let out = node.home.join("state");
    let adapter = node.adapter();
    adapter.capture(&node.work, &session(), &out).await.unwrap();
    let grown = format!(
        "{}{{\"type\":\"label\",\"id\":\"a0000009\",\"parentId\":\"a0000005\",\"timestamp\":\"t\",\"targetId\":\"a0000003\",\"label\":\"here\"}}\n",
        node.fill(LINES)
    );
    node.write(FILE_NAME, &grown);
    let err = adapter.restore(&node.work, &out).await.unwrap_err();
    assert!(matches!(err, HarnessError::StateDiverged { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(node.file()).unwrap(), grown);
}

#[tokio::test]
async fn a_session_that_holds_a_secret_is_refused() {
    let node = Node::new();
    node.write(
        FILE_NAME,
        &node.fill(LINES).replace(
            "name the greeting file",
            "use sk-ant-FAKEleakedKEY0123456789abcd",
        ),
    );
    let out = node.home.join("state");
    let err = node
        .adapter()
        .capture(&node.work, &session(), &out)
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SecretInState { .. }), "{err}");
    assert!(!out.join("manifest.json").exists());
}

#[tokio::test]
async fn only_the_whole_id_under_this_workspace_names_the_session() {
    let node = Node::new();
    let out = node.home.join("state");
    let adapter = node.adapter();
    for id in [
        "0199fab0",
        "0199fab0-0000-7000-8000-0000000000a",
        "0199fab0-0000-7000-8000-0000000000bb",
    ] {
        let err = adapter
            .capture(&node.work, &SessionId::parse(id).unwrap(), &out)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HarnessError::SessionNotFound { .. }),
            "{id}: {err}"
        );
    }
    // A second file with the same id under the same key is not guessed between.
    node.write(
        &FILE_NAME.replace("T10-00-00", "T11-00-00"),
        &node.fill(LINES),
    );
    let err = adapter
        .capture(&node.work, &session(), &out)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::LineageAmbiguous { .. }),
        "{err}"
    );
}

#[tokio::test]
async fn a_file_of_another_workspace_under_the_same_key_is_not_carried() {
    let node = Node::new();
    // `/a-b` and `/a/b` share pi's key; the header names the real one.
    let other = node.work.to_str().unwrap().replace("jobs/j1", "jobs-j1");
    node.write(
        FILE_NAME,
        &LINES
            .replace("@SESSION@", SESSION)
            .replace("@WORK@", &other),
    );
    let err = node
        .adapter()
        .capture(&node.work, &session(), &node.home.join("state"))
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::RootMismatch { .. }), "{err}");
}

#[tokio::test]
async fn a_headless_run_names_the_session_pi_made() {
    let node = Node::new();
    let mut running = node
        .adapter()
        .launch(
            &node.work,
            Launch::Headless {
                resume: None,
                prompt: "-starts with a dash".to_owned(),
            },
            &node.env(),
        )
        .await
        .unwrap();
    running.release().unwrap();
    let child = running.child_mut();
    let mut stdout = child.stdout.take().unwrap();
    let mut said = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout, &mut said)
        .await
        .unwrap();
    assert!(child.wait().await.unwrap().success());
    let mut report = TurnReport::default();
    let mut named = false;
    for line in said.lines() {
        named |= Pi::read_headless_line(&mut report, line);
    }
    assert!(named, "{said}");
    assert!(report.session.is_some());
    assert_eq!(
        report.answer.as_deref(),
        Some("pi did:  -starts with a dash")
    );
}
