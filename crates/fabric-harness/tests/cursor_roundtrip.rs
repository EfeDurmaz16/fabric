//! Cursor Agent CLI adapter: a synthetic home laid out as Cursor Agent CLI
//! 2026.10.01 keeps it (`tests/fixtures/cursor/`), with a chat whose
//! `store.db` holds an encryption key beside its blobs. The capture carries
//! nothing of the chat and says why, a restore reaches `workspace_only`
//! and places nothing, a resume on the node that holds the chat continues
//! it with `--resume`, and one on a node without it starts a new chat in
//! the workspace. A state tree that lists a file and a `cursor-agent`
//! that holds the chat open are refused.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use fabric_harness::cursor::CursorAgent;
use fabric_harness::manifest::StateManifest;
use fabric_harness::{Fidelity, Harness, HarnessError, Launch, SettingName};

mod executable;
use executable::write_executable;

const ID: &str = "3c9e1f2a-7b4d-4e8f-9a1b-2c3d4e5f6a7b";

struct Fixture {
    home: tempfile::TempDir,
    work: PathBuf,
    bin: OsString,
    chat: PathBuf,
}

impl Fixture {
    fn home(&self) -> PathBuf {
        self.home.path().canonicalize().unwrap()
    }

    fn adapter(&self) -> CursorAgent {
        CursorAgent::new(abs(&self.home()), self.bin.clone())
    }

    fn state(&self) -> PathBuf {
        let out = self.home().join("state");
        std::fs::create_dir_all(&out).unwrap();
        out
    }
}

fn abs(path: &Path) -> AbsPath {
    AbsPath::parse(path.to_str().unwrap()).unwrap()
}

fn session() -> SessionId {
    SessionId::parse(ID).unwrap()
}

/// The md5 of `text` in hex, as Cursor names a workspace's chat directory,
/// from the system's own tool, so no fixture stores a path's hash.
fn md5_hex(text: &str) -> String {
    let out = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "printf %s \"$1\" | { md5sum 2>/dev/null || md5 -q; } | cut -c1-32",
            "sh",
            text,
        ])
        .output()
        .unwrap();
    let hex = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    assert_eq!(hex.len(), 32, "{hex:?}");
    hex
}

/// Copies `from` into `to`, writing the workspace path and its md5 where
/// the fixture holds their placeholders.
fn copy_fixture(from: &Path, to: &Path, root: &str, hash: &str) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry
            .file_name()
            .to_str()
            .unwrap()
            .replace("@WORKSPACE_MD5@", hash);
        let dest = to.join(name);
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
            copy_fixture(&entry.path(), &dest, root, hash);
        } else {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            std::fs::write(dest, text.replace("@ROOT@", root)).unwrap();
        }
    }
}

/// A home holding the fixture, a workspace `<home>/fabric/jobs/j1/work`
/// and a fake `cursor-agent` that answers `--version` and writes its
/// arguments and directory to `$FAKE_LOG`.
fn build() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let work = home_path.join("fabric/jobs/j1/work");
    std::fs::create_dir_all(&work).unwrap();
    let root = work.to_str().unwrap();
    let hash = md5_hex(root);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cursor/home");
    copy_fixture(&fixture, &home_path, root, &hash);
    let bin = home_path.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_executable(
        &bin.join("cursor-agent"),
        b"#!/bin/sh\ncase \"$1\" in\n  --version) echo 2026.10.01-e373342; exit 0 ;;\n  linger) exec 3<\"$2\"; : > \"$2.held\"; while :; do sleep 0.1; done ;;\nesac\n{ echo \"cwd=$(pwd -P)\"; for a in \"$@\"; do echo \"arg=$a\"; done; } > \"$FAKE_LOG\"\n",
    );
    let chat = home_path.join(".cursor/chats").join(hash).join(ID);
    Fixture {
        home,
        work,
        bin: bin.into_os_string(),
        chat,
    }
}

fn fake_log(f: &Fixture) -> (PathBuf, Vec<(fabric_core::env::EnvVarName, String)>) {
    let log = f.home().join("cursor.log");
    let env = vec![(
        fabric_core::env::EnvVarName::parse("FAKE_LOG").unwrap(),
        log.to_str().unwrap().to_owned(),
    )];
    (log, env)
}

/// Decision of 2026-11-08: the store is encrypted with its key beside it,
/// so no scan can prove it holds no secret, and nothing of it travels.
#[tokio::test]
async fn a_capture_carries_nothing_of_the_chat_and_says_why() {
    let f = build();
    let out = f.state();
    let manifest = f
        .adapter()
        .capture(&f.work, &session(), &out)
        .await
        .unwrap();
    assert!(manifest.files().is_empty(), "{:?}", manifest.files());
    assert_eq!(manifest.session(), &session());
    assert_eq!(manifest.harness_version(), Some("2026.10.01-e373342"));
    let why: Vec<(&str, u32)> = manifest
        .redactions()
        .iter()
        .map(|r| (r.rule(), r.count()))
        .collect();
    assert_eq!(why, [("cursor-store-not-carried", 1)]);
    let mut tree = Vec::new();
    for entry in std::fs::read_dir(&out).unwrap() {
        tree.push(entry.unwrap().file_name().into_string().unwrap());
    }
    assert_eq!(tree, ["manifest.json"], "only the manifest is written");
}

#[tokio::test]
async fn a_restore_reaches_workspace_only_and_places_nothing() {
    let f = build();
    let out = f.state();
    let adapter = f.adapter();
    let manifest = adapter.capture(&f.work, &session(), &out).await.unwrap();
    adapter.confirm_departure(&manifest).await.unwrap();
    std::fs::remove_dir_all(f.home().join(".cursor")).unwrap();
    let restored = adapter.restore(&f.work, &out).await.unwrap();
    assert_eq!(restored.fidelity, Fidelity::WorkspaceOnly);
    assert_eq!(restored.session, session());
    assert!(restored.placed.is_empty());
    assert!(!f.home().join(".cursor").exists(), "a restore wrote state");
}

/// A tree that lists a file was not made by this adapter; it is refused
/// before anything is written.
#[tokio::test]
async fn a_state_tree_that_lists_a_file_is_refused() {
    let f = build();
    let out = f.state();
    f.adapter()
        .capture(&f.work, &session(), &out)
        .await
        .unwrap();
    let path = out.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let rel = format!(
        "home/.cursor/chats/{}/{ID}/meta.json",
        f.chat
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
    );
    manifest["files"] = serde_json::json!([{
        "rel_path": rel,
        "content": "Opaque",
        "mutability": "Replaceable",
        "size": 0,
        "blake3": blake3::hash(b"").to_hex().to_string(),
    }]);
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    StateManifest::read(&out).unwrap();
    std::fs::remove_dir_all(f.home().join(".cursor")).unwrap();
    let err = f.adapter().restore(&f.work, &out).await.unwrap_err();
    assert!(matches!(err, HarnessError::UnexpectedFile { .. }), "{err}");
    assert!(!f.home().join(".cursor").exists());
}

#[tokio::test]
async fn a_cursor_agent_that_holds_the_chat_open_refuses_the_capture() {
    let f = build();
    let mut linger = std::process::Command::new("/bin/sh")
        .arg(Path::new(&f.bin).join("cursor-agent"))
        .arg("linger")
        .arg(f.chat.join("store.db"))
        .spawn()
        .unwrap();
    // The shell says once it holds the store open.
    let held = f.chat.join("store.db.held");
    for _ in 0..200 {
        if held.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(held.exists(), "the fake never opened the store");
    let err = f.adapter().capture(&f.work, &session(), &f.state()).await;
    linger.kill().unwrap();
    let _ = linger.wait();
    assert!(
        matches!(err, Err(HarnessError::HarnessRunning { .. })),
        "{err:?}"
    );
}

/// On the node that wrote the chat, a resume continues it.
#[tokio::test]
async fn a_resume_where_the_chat_lives_continues_it() {
    let f = build();
    let (log, env) = fake_log(&f);
    let launch = Launch::Headless {
        resume: Some(session()),
        prompt: "-n and then?".to_owned(),
    };
    let mut running = f.adapter().launch(&f.work, launch, &env).await.unwrap();
    assert_eq!(running.session(), Some(&session()));
    assert!(running.settings().not_carried.is_empty());
    running.release().unwrap();
    assert!(running.child_mut().wait().await.unwrap().success());
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        format!(
            "cwd={}\narg=--print\narg=--output-format\narg=stream-json\narg=--trust\narg=--resume\narg={ID}\narg=--\narg=-n and then?\n",
            f.work.display()
        )
    );
}

/// On a node the chat never reached, a resume starts a new chat in the
/// restored workspace and says the conversation stayed behind.
#[tokio::test]
async fn a_resume_without_the_chat_starts_a_new_one_and_says_so() {
    let f = build();
    std::fs::remove_dir_all(&f.chat).unwrap();
    let (log, env) = fake_log(&f);
    let launch = Launch::Interactive {
        resume: Some(session()),
        prompt: None,
    };
    let mut running = f.adapter().launch(&f.work, launch, &env).await.unwrap();
    assert_eq!(running.session(), None, "a new chat has no id yet");
    let left = &running.settings().not_carried;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].name, SettingName::Chat);
    assert_eq!(left[0].value, ID);
    assert!(
        left[0].reason.contains("workspace_only"),
        "{}",
        left[0].reason
    );
    running.release().unwrap();
    assert!(running.child_mut().wait().await.unwrap().success());
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        format!("cwd={}\narg=--trust\n", f.work.display())
    );
}
