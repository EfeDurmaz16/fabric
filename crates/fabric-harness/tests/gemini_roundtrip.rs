//! Gemini CLI adapter: a synthetic home laid out as Gemini CLI 0.67.0 keeps
//! it (`tests/fixtures/gemini/`), with a second session that shares the
//! chat's 8-character prefix, a subagent file and side directories. The
//! capture takes the session's own files and nothing else, a restore onto
//! a node with the same home and workspace path but no Gemini state places
//! them and the slug's marker, and a fake `gemini` is resumed there with
//! the model of the chat. A registry that gives the workspace another
//! slug, a secret in the chat, an unknown or ambiguous id, a running
//! `gemini` and a `gemini` older than the known writer are refused.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use fabric_core::version::HarnessVersion;
use fabric_harness::gemini::GeminiCli;
use fabric_harness::{Fidelity, Harness, HarnessError, Launch, Origin, SettingName};

mod executable;
use executable::write_executable;

const ID: &str = "6f1d2c3b-4a5e-4b6c-8d7e-9f0a1b2c3d4e";
const CHAT: &str = "session-2026-11-08T10-00-6f1d2c3b.jsonl";
const PLACEHOLDER: &str = "@ROOT@";

struct Fixture {
    home: tempfile::TempDir,
    work: PathBuf,
    bin: OsString,
}

impl Fixture {
    fn home(&self) -> PathBuf {
        self.home.path().canonicalize().unwrap()
    }

    fn gemini(&self) -> PathBuf {
        self.home().join(".gemini")
    }

    fn slug(&self) -> PathBuf {
        self.gemini().join("tmp/work")
    }

    fn adapter(&self) -> GeminiCli {
        GeminiCli::new(abs(&self.home()), self.bin.clone())
    }

    fn set_version(&self, version: &str) {
        std::fs::write(Path::new(&self.bin).join("version"), format!("{version}\n")).unwrap();
    }
}

fn abs(path: &Path) -> AbsPath {
    AbsPath::parse(path.to_str().unwrap()).unwrap()
}

fn session() -> SessionId {
    SessionId::parse(ID).unwrap()
}

/// Copies `from` into `to`, writing `root` where a file holds the placeholder.
fn copy_fixture(from: &Path, to: &Path, root: &str) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
            copy_fixture(&entry.path(), &dest, root);
        } else {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            std::fs::write(dest, text.replace(PLACEHOLDER, root)).unwrap();
        }
    }
}

/// A home holding the fixture, a workspace `<home>/fabric/jobs/j1/work`
/// and a fake `gemini` that answers `--version` from the `version` file
/// beside it, with shell builtins alone since a probe runs with only its
/// `PATH`, and writes its arguments and directory to `$FAKE_LOG`.
fn build() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let work = home_path.join("fabric/jobs/j1/work");
    std::fs::create_dir_all(&work).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gemini/home");
    copy_fixture(&fixture, &home_path, work.to_str().unwrap());
    let bin = home_path.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_executable(
        &bin.join("gemini"),
        b"#!/bin/sh\ncase \"$1\" in\n  --version) read -r v < \"${0%/*}/version\"; echo \"$v\"; exit 0 ;;\n  linger) while :; do sleep 0.1; done ;;\nesac\n{ echo \"cwd=$(pwd -P)\"; for a in \"$@\"; do echo \"arg=$a\"; done; } > \"$FAKE_LOG\"\n",
    );
    let fixture = Fixture {
        home,
        work,
        bin: bin.into_os_string(),
    };
    fixture.set_version("0.67.0");
    fixture
}

/// The tree paths a manifest carries, sorted.
fn carried(state: &Path) -> Vec<String> {
    let manifest = fabric_harness::manifest::StateManifest::read(state).unwrap();
    manifest
        .files()
        .iter()
        .map(|file| file.rel_path.as_str().to_owned())
        .collect()
}

#[tokio::test]
async fn a_capture_takes_the_sessions_own_files_and_nothing_else() {
    let f = build();
    let out = f.home().join("state");
    std::fs::create_dir_all(&out).unwrap();
    let manifest = f
        .adapter()
        .capture(&f.work, &session(), &out)
        .await
        .unwrap();
    assert_eq!(manifest.harness_version(), Some("0.67.0"));
    assert_eq!(
        carried(&out),
        [
            format!("home/.gemini/tmp/work/{ID}/plans/plan.md"),
            format!("home/.gemini/tmp/work/{ID}/tracker/tasks.json"),
            format!("home/.gemini/tmp/work/chats/{ID}/9a8b7c6d-1e2f-4a3b-8c4d-5e6f7a8b9c0d.jsonl"),
            format!("home/.gemini/tmp/work/chats/{CHAT}"),
        ]
    );
    let staged = out.join(format!("home/.gemini/tmp/work/chats/{CHAT}"));
    assert_eq!(
        std::fs::read(staged).unwrap(),
        std::fs::read(f.slug().join("chats").join(CHAT)).unwrap(),
        "the chat travels byte for byte"
    );
}

/// A node with the same home and workspace path and no Gemini state: the
/// files land where they were, the marker names the workspace, so Gemini's
/// registry finds the slug, and nothing of the login travels.
#[tokio::test]
async fn a_restore_places_the_session_and_the_marker_on_a_node_without_gemini_state() {
    let f = build();
    let out = f.home().join("state");
    std::fs::create_dir_all(&out).unwrap();
    let adapter = f.adapter();
    adapter.capture(&f.work, &session(), &out).await.unwrap();
    adapter
        .confirm_departure(&fabric_harness::manifest::StateManifest::read(&out).unwrap())
        .await
        .unwrap();
    let chat = std::fs::read(f.slug().join("chats").join(CHAT)).unwrap();
    std::fs::remove_dir_all(f.gemini().join("tmp")).unwrap();
    std::fs::remove_file(f.gemini().join("projects.json")).unwrap();

    let restored = adapter.restore(&f.work, &out).await.unwrap();
    assert_eq!(restored.session, session());
    assert_eq!(restored.fidelity, Fidelity::NativeSession);
    assert_eq!(restored.placed.len(), 4);
    assert_eq!(
        std::fs::read(f.slug().join("chats").join(CHAT)).unwrap(),
        chat
    );
    assert_eq!(
        std::fs::read_to_string(f.slug().join(".project_root")).unwrap(),
        f.work.to_str().unwrap()
    );
    assert!(
        !f.slug()
            .join("chats/session-2026-11-07T09-00-6f1d2c3b.jsonl")
            .exists()
    );
    assert!(!f.gemini().join("projects.json").exists());

    // Again over what is there: equal files are found in place.
    let again = adapter.restore(&f.work, &out).await.unwrap();
    assert_eq!(again.placed.len(), 4);
}

#[tokio::test]
async fn a_restore_is_refused_when_this_node_gives_the_workspace_another_slug() {
    let f = build();
    let out = f.home().join("state");
    std::fs::create_dir_all(&out).unwrap();
    let adapter = f.adapter();
    adapter.capture(&f.work, &session(), &out).await.unwrap();
    std::fs::remove_dir_all(f.gemini().join("tmp")).unwrap();
    std::fs::write(
        f.gemini().join("projects.json"),
        format!("{{\"projects\":{{\"{}\":\"work-1\"}}}}", f.work.display()),
    )
    .unwrap();
    let err = adapter.restore(&f.work, &out).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::StateDiverged { ref path } if path.ends_with("projects.json")),
        "{err}"
    );
    assert!(!f.gemini().join("tmp").exists(), "nothing was written");

    // A slug directory whose marker names another project is refused too.
    std::fs::remove_file(f.gemini().join("projects.json")).unwrap();
    std::fs::create_dir_all(f.slug()).unwrap();
    std::fs::write(f.slug().join(".project_root"), "/elsewhere/work").unwrap();
    let err = adapter.restore(&f.work, &out).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::StateDiverged { ref path } if path.ends_with(".project_root")),
        "{err}"
    );
    assert!(!f.slug().join("chats").exists(), "nothing was written");
}

#[tokio::test]
async fn a_secret_in_the_chat_refuses_the_capture() {
    let f = build();
    let chat = f.slug().join("chats").join(CHAT);
    let mut text = std::fs::read_to_string(&chat).unwrap();
    text.push_str("{\"id\":\"m3\",\"timestamp\":\"t\",\"type\":\"user\",\"content\":[{\"text\":\"use sk-ant-api03-FAKEFAKEFAKEFAKEFAKEFAKEFAKE0123\"}]}\n");
    std::fs::write(&chat, text).unwrap();
    let out = f.home().join("state");
    std::fs::create_dir_all(&out).unwrap();
    let err = f
        .adapter()
        .capture(&f.work, &session(), &out)
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SecretInState { .. }), "{err}");
}

#[tokio::test]
async fn an_unknown_or_ambiguous_id_is_refused() {
    let f = build();
    let out = f.home().join("state");
    std::fs::create_dir_all(&out).unwrap();
    // Shares the short id with the decoy and the session, and is neither.
    let other = SessionId::parse("6f1d2c3b-ffff-4fff-8fff-ffffffffffff").unwrap();
    let err = f
        .adapter()
        .capture(&f.work, &other, &out)
        .await
        .unwrap_err();
    assert!(matches!(err, HarnessError::SessionNotFound { .. }), "{err}");

    let chats = f.slug().join("chats");
    std::fs::copy(
        chats.join(CHAT),
        chats.join("session-2026-11-08T10-00-1-6f1d2c3b.jsonl"),
    )
    .unwrap();
    let err = f
        .adapter()
        .capture(&f.work, &session(), &out)
        .await
        .unwrap_err();
    assert!(
        matches!(err, HarnessError::LineageAmbiguous { .. }),
        "{err}"
    );
}

#[tokio::test]
async fn a_running_gemini_refuses_the_capture() {
    let f = build();
    // Through `sh`, so the process is named `sh` and only its command line
    // names this test's `gemini`: no other test's capture sees it.
    let mut linger = std::process::Command::new("/bin/sh")
        .arg(Path::new(&f.bin).join("gemini"))
        .arg("linger")
        .spawn()
        .unwrap();
    let out = f.home().join("state");
    std::fs::create_dir_all(&out).unwrap();
    let err = f.adapter().capture(&f.work, &session(), &out).await;
    linger.kill().unwrap();
    let _ = linger.wait();
    assert!(
        matches!(err, Err(HarnessError::HarnessRunning { .. })),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_headless_resume_continues_the_id_with_the_chats_model() {
    let f = build();
    let log = f.home().join("gemini.log");
    let env = vec![(
        fabric_core::env::EnvVarName::parse("FAKE_LOG").unwrap(),
        log.to_str().unwrap().to_owned(),
    )];
    let adapter = f
        .adapter()
        .with_known_writer(Some(HarnessVersion::parse("0.66.2").unwrap()));
    let launch = Launch::Headless {
        resume: Some(session()),
        prompt: "what did we rename?".to_owned(),
    };
    let mut running = adapter.launch(&f.work, launch, &env).await.unwrap();
    assert_eq!(running.session(), Some(&session()));
    assert_eq!(
        running.version().map(HarnessVersion::as_str),
        Some("0.67.0")
    );
    let passed = &running.settings().passed;
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert_eq!(passed[0].name, SettingName::Model);
    assert_eq!(passed[0].value, "gemini-3-pro");
    assert_eq!(passed[0].origin, Origin::Session);
    running.release().unwrap();
    assert!(running.child_mut().wait().await.unwrap().success());
    let logged = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        logged,
        format!(
            "cwd={}\narg=--output-format\narg=stream-json\narg=--resume\narg={ID}\narg=--model\narg=gemini-3-pro\narg=--prompt=what did we rename?\n",
            f.work.display()
        )
    );
}

/// A chat records no version, so the job carries the newest writer, and a
/// `gemini` older than it never starts on the session.
#[tokio::test]
async fn a_gemini_older_than_the_known_writer_is_refused_before_it_starts() {
    let f = build();
    f.set_version("0.66.0");
    let log = f.home().join("gemini.log");
    let env = vec![(
        fabric_core::env::EnvVarName::parse("FAKE_LOG").unwrap(),
        log.to_str().unwrap().to_owned(),
    )];
    let adapter = f
        .adapter()
        .with_known_writer(Some(HarnessVersion::parse("0.67.0").unwrap()));
    let launch = Launch::Headless {
        resume: Some(session()),
        prompt: "next".to_owned(),
    };
    let err = adapter.launch(&f.work, launch, &env).await.unwrap_err();
    assert!(
        matches!(err, HarnessError::HarnessOlderThanSession { ref local, ref recorded, .. } if local == "0.66.0" && recorded == "0.67.0"),
        "{err}"
    );
    assert!(!log.exists(), "the older gemini started");
}
