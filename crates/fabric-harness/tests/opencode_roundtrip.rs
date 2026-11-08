//! OpenCode adapter: a synthetic session in a fake `opencode`'s store is
//! captured as the export OpenCode prints, never as its database, restored
//! onto a node with the same home and workspace path whose store lacks it,
//! through `opencode import`, and continued there by the same id; the
//! refusals of docs/harness-adapters.md are proved. The fake and the rows
//! it starts from are `tests/fixtures/opencode/`.

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
use fabric_harness::opencode::OpenCode;
use fabric_harness::{Fidelity, Harness, HarnessError, Launch, Origin, SettingName};

mod executable;
use executable::write_executable;

const SESSION: &str = "ses_0fabric0opencode0session001";
const FAKE: &str = include_str!("fixtures/opencode/fake-opencode.sh");
const INFO: &str = include_str!("fixtures/opencode/session.info.json");
const MESSAGES: &str = include_str!("fixtures/opencode/session.messages.jsonl");
/// The fake login token the fake keeps in its database.
const DB_TOKEN: &str = "sk-ant-FAKEdatabaseTOKEN0123456789";

struct Node {
    _dir: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
    path: OsString,
}

impl Node {
    /// A home with a workspace and the fake on `PATH`, holding the
    /// fixture session written by `version`.
    fn new(version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let work = home.join("fabric/jobs/j1/work");
        std::fs::create_dir_all(&work).unwrap();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("opencode"), FAKE.as_bytes());
        let path = OsString::from(format!("{}:/usr/bin:/bin", bin.display()));
        let node = Node {
            _dir: dir,
            home,
            work,
            path,
        };
        node.seed(version, MESSAGES);
        node
    }

    fn fill(&self, text: &str, version: &str) -> String {
        let work = self.work.to_str().unwrap();
        text.replace("@SESSION@", SESSION)
            .replace("@WORK_REL@", work.trim_start_matches('/'))
            .replace("@WORK@", work)
            .replace("@VERSION@", version)
    }

    fn rows(&self) -> PathBuf {
        self.home.join(".local/share/opencode/fake-rows")
    }

    fn seed(&self, version: &str, messages: &str) {
        std::fs::create_dir_all(self.rows()).unwrap();
        std::fs::write(
            self.rows().join(format!("{SESSION}.info")),
            self.fill(INFO, version),
        )
        .unwrap();
        std::fs::write(
            self.rows().join(format!("{SESSION}.msgs")),
            self.fill(messages, version),
        )
        .unwrap();
    }

    /// Forgets the session, as a second machine never had it.
    fn forget(&self) {
        std::fs::remove_dir_all(self.home.join(".local/share/opencode")).unwrap();
    }

    fn adapter(&self) -> OpenCode {
        OpenCode::new(abs(&self.home), self.path.clone())
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

    fn messages(&self, id: &str) -> String {
        std::fs::read_to_string(self.rows().join(format!("{id}.msgs"))).unwrap_or_default()
    }
}

fn abs(path: &Path) -> AbsPath {
    AbsPath::parse(path.to_str().unwrap()).unwrap()
}

fn session() -> SessionId {
    SessionId::parse(SESSION).unwrap()
}

/// Every byte under `dir`, as text.
fn everything_under(dir: &Path) -> String {
    let mut all = String::new();
    for entry in walk(dir) {
        all.push_str(&String::from_utf8_lossy(&std::fs::read(entry).unwrap()));
    }
    all
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Lets a headless run go and returns what it printed.
async fn turn_output(mut running: fabric_harness::Running) -> String {
    running.release().unwrap();
    let child = running.child_mut();
    let mut stdout = child.stdout.take().unwrap();
    let mut said = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout, &mut said)
        .await
        .unwrap();
    assert!(child.wait().await.unwrap().success());
    said
}

#[tokio::test]
async fn a_session_moves_as_its_export_and_continues_by_the_same_id() {
    let node = Node::new("1.18.34");
    let out = node.home.join("state");
    let adapter = node.adapter();
    let manifest = adapter.capture(&node.work, &session(), &out).await.unwrap();
    let paths: Vec<&str> = manifest
        .files()
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    assert_eq!(
        paths,
        [format!(
            "home/.local/share/opencode/fabric/{SESSION}.export.json"
        )]
    );
    assert_eq!(manifest.harness_version(), Some("1.18.34"));
    let carried = everything_under(&out);
    assert!(carried.contains("the first turn: name the greeting file"));
    assert!(!carried.contains(DB_TOKEN), "the database travelled");
    assert!(
        !carried.contains("sk-ant-FAKEauthJSON"),
        "auth.json travelled"
    );
    adapter.confirm_departure(&manifest).await.unwrap();

    let before = node.messages(SESSION);
    node.forget();
    let restored = adapter.restore(&node.work, &out).await.unwrap();
    assert_eq!(restored.session, session());
    assert_eq!(restored.fidelity, Fidelity::NativeSession);
    assert_eq!(node.messages(SESSION), before, "the import lost a message");

    let running = adapter
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
    assert_eq!(
        running
            .version()
            .map(fabric_core::version::HarnessVersion::as_str),
        Some("1.18.34")
    );
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
            (SettingName::Agent, "build", Origin::Session),
            (SettingName::Effort, "high", Origin::Session),
        ]
    );
    let said = turn_output(running).await;
    let mut report = fabric_harness::TurnReport::default();
    for line in said.lines() {
        OpenCode::read_headless_line(&mut report, line);
    }
    assert_eq!(report.session, Some(session()));
    assert_eq!(
        report.answer.as_deref(),
        Some("the first turn said: the first turn: name the greeting file")
    );
    let args = std::fs::read_to_string(node.home.join("opencode-args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
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
}

#[tokio::test]
async fn an_import_that_mints_a_new_id_is_not_called_native() {
    let node = Node::new("1.18.34");
    let out = node.home.join("state");
    let adapter = node.adapter();
    let manifest = adapter.capture(&node.work, &session(), &out).await.unwrap();
    adapter.confirm_departure(&manifest).await.unwrap();
    node.forget();
    std::fs::create_dir_all(node.home.join(".local/share/opencode")).unwrap();
    std::fs::write(node.home.join(".local/share/opencode/fake-new-id"), "").unwrap();
    let restored = adapter.restore(&node.work, &out).await.unwrap();
    let minted = SessionId::parse("ses_0fabric0minted0by0import1").unwrap();
    assert_eq!(restored.session, minted);
    assert_eq!(restored.fidelity, Fidelity::NewNativeSessionWithPackage);
    assert!(
        node.messages(minted.as_str())
            .contains("name the greeting file")
    );
}

#[tokio::test]
async fn a_node_that_holds_a_prefix_gets_the_rest_and_one_with_more_is_refused() {
    let node = Node::new("1.18.34");
    let out = node.home.join("state");
    let adapter = node.adapter();
    let manifest = adapter.capture(&node.work, &session(), &out).await.unwrap();
    adapter.confirm_departure(&manifest).await.unwrap();

    // The node holds the first message only: the import adds the second.
    let first = MESSAGES.lines().next().unwrap();
    node.seed("1.18.34", &format!("{first}\n"));
    let restored = adapter.restore(&node.work, &out).await.unwrap();
    assert_eq!(restored.fidelity, Fidelity::NativeSession);
    assert_eq!(node.messages(SESSION).lines().count(), 2);

    // A turn taken here after the job left is not overwritten or merged.
    let extra = MESSAGES.replace(
        "msg_0fabric0asst000000000001",
        "msg_0fabric0asst000000000009",
    );
    node.seed(
        "1.18.34",
        &format!("{MESSAGES}{}", extra.lines().nth(1).unwrap()),
    );
    let held = node.messages(SESSION);
    let err = adapter.restore(&node.work, &out).await.unwrap_err();
    assert!(matches!(err, HarnessError::StateDiverged { .. }), "{err}");
    assert_eq!(node.messages(SESSION), held);
}

#[tokio::test]
async fn a_session_that_holds_a_secret_is_refused_and_nothing_is_written() {
    let node = Node::new("1.18.34");
    let leaked = MESSAGES.replace(
        "name the greeting file",
        "use sk-ant-FAKEleakedKEY0123456789abcd",
    );
    node.seed("1.18.34", &leaked);
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
async fn an_unknown_or_foreign_session_is_not_captured() {
    let node = Node::new("1.18.34");
    let out = node.home.join("state");
    let adapter = node.adapter();
    for id in [
        "ses_0fabric0opencode0missing01",
        "019f233e-bb33-7342-85fa-f2d33b14368a",
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
    // A session of another workspace is not carried under this one.
    let other = node.home.join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    let err = adapter.capture(&other, &session(), &out).await.unwrap_err();
    assert!(matches!(err, HarnessError::RootMismatch { .. }), "{err}");
}

#[tokio::test]
async fn an_older_opencode_is_refused_a_session_a_newer_one_wrote() {
    let node = Node::new("1.19.0");
    let err = node
        .adapter()
        .launch(
            &node.work,
            Launch::Headless {
                resume: Some(session()),
                prompt: "next".to_owned(),
            },
            &node.env(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessOlderThanSession { ref local, ref recorded, .. }
            if local == "1.18.34" && recorded == "1.19.0"),
        "{err}"
    );
    assert!(
        !node.home.join("opencode-args").exists(),
        "the older opencode ran"
    );
}

/// A capture refused for a secret leaves Fabric's own copy of the export
/// in the home; a later restore onto that node does not read it as work
/// done there, since the store is what it compares.
#[tokio::test]
async fn a_stale_copy_of_the_export_does_not_block_a_restore() {
    let node = Node::new("1.18.34");
    let out = node.home.join("state");
    let adapter = node.adapter();
    adapter.capture(&node.work, &session(), &out).await.unwrap();
    let copy = node.home.join(format!(
        ".local/share/opencode/fabric/{SESSION}.export.json"
    ));
    std::fs::write(&copy, "an older capture").unwrap();
    node.forget();
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, "an older capture").unwrap();
    let restored = adapter.restore(&node.work, &out).await.unwrap();
    assert_eq!(restored.fidelity, Fidelity::NativeSession);
    assert!(node.messages(SESSION).contains("name the greeting file"));
}

/// `opencode import` 1.18.34 writes the session row and every message
/// before the one it refuses, then exits 1. A restore it fails takes that
/// partial session back out, so the node holds what it held before.
#[tokio::test]
async fn an_import_that_fails_part_way_leaves_no_partial_session() {
    let node = Node::new("1.18.34");
    let unimportable = MESSAGES.replace("\"parentID\":\"msg_0fabric0user000000000001\",", "");
    node.seed("1.18.34", &unimportable);
    let out = node.home.join("state");
    let adapter = node.adapter();
    let manifest = adapter.capture(&node.work, &session(), &out).await.unwrap();
    adapter.confirm_departure(&manifest).await.unwrap();
    node.forget();
    let err = adapter.restore(&node.work, &out).await.unwrap_err();
    assert!(matches!(err, HarnessError::ProcessQuery { .. }), "{err}");
    assert!(
        !node.rows().join(format!("{SESSION}.info")).exists(),
        "a partial session was left in the store"
    );
    assert_eq!(node.messages(SESSION), "");
}
