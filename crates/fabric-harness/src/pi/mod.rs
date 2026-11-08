//! The pi adapter (PR 5; contract in `docs/harness-adapters.md`, section
//! pi).
//!
//! Invariant: pi keeps one session in one JSONL file,
//! `~/.pi/agent/sessions/--<workspace path with / as ->--/<timestamp>_<uuid>.jsonl`,
//! a v3 tree whose header `{"type":"session","id","cwd","timestamp","version"}`
//! names the workspace (format of 0.80.3, `docs/session-format.md` and
//! `getDefaultSessionDirPath` of the installed package, read 2026-11-08:
//! the leading `/` dropped, then every `/`, `\` and `:` as `-`). The file is
//! carried byte for byte, as Codex's rollout is, and refused when a line
//! holds a secret shape; an Anthropic `thinkingSignature` is a signed block
//! and is never edited. It only grows, so restore accepts a destination
//! that is missing, equal, or a prefix of the incoming bytes, and nothing
//! else. The id is matched on the whole `_<uuid>.jsonl` suffix, never a
//! partial id, and the header must name this session and this workspace:
//! the key is lossy (`/a-b` and `/a/b` share one), so a file under the key
//! whose `cwd` is another path is refused. A resume runs
//! `pi --session <path>` with the provider and model of the last
//! `model_change` entry or assistant message and the level of the last
//! `thinking_level_change`, each one the adapter was configured with
//! winning. `auth.json`, `settings.json` and `trust.json` never travel. The
//! reached fidelity is `NativeSession`: the same file under the same path
//! key, continued by its own id. pi takes no lock and records no version
//! in the session, so capture and restore refuse while any process runs
//! the `pi` the adapter would start, and the manifest carries the
//! installed version only.
use std::ffi::OsString;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use serde_json::Value;
use tracing::info;

use crate::error::{HarnessError, Input, IoOp};
use crate::harness::{
    Fidelity, Harness, Installed, Launch, LaunchSettings, ModelName, NotCarried, Origin,
    PassedSetting, Restored, Running, SettingName, TurnReport,
};
use crate::manifest::{Content, HOME_PREFIX, Mutability, StateManifest};
use crate::pipeline::{self, Capturer, Destination};
use crate::process;
use crate::redact::RedactionCounts;
use crate::stage::{Carry, Source};

/// The program name on `PATH`.
pub const PROGRAM: &str = "pi";

/// The version the layout and the flags were read on.
pub const READ_ON: &str = "0.80.3";

/// pi's sessions directory under the home, as tree path components.
const SESSIONS: [&str; 3] = [".pi", "agent", "sessions"];

/// The variables that move pi's agent or session directory.
const MOVING_VARS: [&str; 2] = ["PI_CODING_AGENT_DIR", "PI_CODING_AGENT_SESSION_DIR"];

/// Settings the adapter passes on every run, each one winning over the
/// one the session records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PiSettings {
    /// The provider, such as `anthropic`.
    pub provider: Option<ModelName>,
    /// The model id, such as `claude-sonnet-4-5`.
    pub model: Option<ModelName>,
    /// The thinking level: `off`, `minimal`, `low`, `medium`, `high` or `xhigh`.
    pub thinking: Option<ModelName>,
}

/// The pi adapter for one home directory.
#[derive(Debug, Clone)]
pub struct Pi {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
    pinned: PiSettings,
}

/// pi's directory name for a workspace path.
fn key_of(root: &AbsPath) -> String {
    let path = root.as_str();
    let path = path.strip_prefix('/').unwrap_or(path);
    let key: String = path
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{key}--")
}

/// True when `name` is a session file of `session`: `<timestamp>_<id>.jsonl`.
fn names_session(name: &str, session: &SessionId) -> bool {
    name.strip_suffix(".jsonl")
        .and_then(|stem| stem.strip_suffix(session.as_str()))
        .is_some_and(|rest| rest.ends_with('_') && rest.len() > 1)
}

/// What a session file records that a resume carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Recorded {
    provider: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
}

impl Recorded {
    /// Reads one entry: a `model_change`, an assistant message (the model
    /// that answered), or a `thinking_level_change`.
    fn note(&mut self, entry: &Value) {
        let text = |v: &Value| v.as_str().map(str::to_owned);
        match entry["type"].as_str() {
            Some("model_change") => {
                self.provider = text(&entry["provider"]).or(self.provider.take());
                self.model = text(&entry["modelId"]).or(self.model.take());
            }
            Some("message") if entry["message"]["role"] == "assistant" => {
                let message = &entry["message"];
                if let (Some(provider), Some(model)) =
                    (text(&message["provider"]), text(&message["model"]))
                {
                    self.provider = Some(provider);
                    self.model = Some(model);
                }
            }
            Some("thinking_level_change") => {
                self.thinking = text(&entry["thinkingLevel"]).or(self.thinking.take());
            }
            _ => {}
        }
    }
}

/// Picks a setting: the pinned one, else the recorded one when it is a
/// plain name; `Err` holds a recorded value that is not.
fn pick(
    pinned: Option<&ModelName>,
    recorded: Option<&str>,
) -> Option<Result<(String, Origin), String>> {
    match (pinned, recorded) {
        (Some(pinned), _) => Some(Ok((pinned.as_str().to_owned(), Origin::Pinned))),
        (None, Some(text)) => Some(
            ModelName::parse(text)
                .map(|name| (name.as_str().to_owned(), Origin::Session))
                .map_err(|_| text.to_owned()),
        ),
        (None, None) => None,
    }
}

impl Pi {
    /// An adapter under `home`, finding `pi` on the `PATH` value
    /// `path_var`.
    #[must_use]
    pub fn new(home: AbsPath, path_var: OsString) -> Self {
        let binary = process::find_on_path(PROGRAM, &path_var);
        Pi {
            home,
            binary,
            path_var,
            pinned: PiSettings::default(),
        }
    }

    /// Settings passed on every run, each winning over the session's own.
    #[must_use]
    pub fn with_settings(mut self, pinned: PiSettings) -> Self {
        self.pinned = pinned;
        self
    }

    /// The home directory the adapter reads and writes under.
    #[must_use]
    pub fn home(&self) -> &AbsPath {
        &self.home
    }

    fn home_dir(&self) -> &Path {
        pipeline::fs_path(&self.home)
    }

    fn sessions_dir(&self) -> PathBuf {
        SESSIONS
            .iter()
            .fold(self.home_dir().to_path_buf(), |dir, part| dir.join(part))
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        self.binary
            .clone()
            .ok_or_else(|| HarnessError::NotInstalled {
                program: PROGRAM.to_owned(),
            })
    }

    /// Refuses a launch environment that moves pi's agent or session
    /// directory away from `<home>/.pi/agent`, where the adapter reads.
    pub fn check_env(&self, env: &[(EnvVarName, String)]) -> Result<(), HarnessError> {
        let default_agent_dir = self.home_dir().join(".pi/agent");
        let moved = env.iter().find_map(|(name, value)| {
            let var = MOVING_VARS.iter().find(|var| **var == name.as_str())?;
            let is_default = *var == MOVING_VARS[0] && Path::new(value) == default_agent_dir;
            (!value.is_empty() && !is_default).then_some(*var)
        });
        match moved {
            Some(var) => Err(HarnessError::ConfigMoved {
                program: PROGRAM.to_owned(),
                var,
                moves: "the session files",
            }),
            None => Ok(()),
        }
    }

    /// Refuses while a process runs the `pi` this adapter would start: pi
    /// takes no lock, so a running pi may be appending to the session.
    async fn refuse_running(&self) -> Result<(), HarnessError> {
        let Some(binary) = &self.binary else {
            return Ok(());
        };
        let pids = process::running_binary(binary).await?;
        if pids.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            })
        }
    }

    /// The one file of `session` under the workspace's key, its header
    /// checked: refused when there is none, when two files share the id,
    /// and when the header names another session or workspace.
    fn session_file(&self, root: &AbsPath, session: &SessionId) -> Result<PathBuf, HarnessError> {
        let dir = self.sessions_dir().join(key_of(root));
        pipeline::refuse_symlinks(self.home_dir(), &dir)?;
        let mut found = Vec::new();
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &dir, e))?;
                    let name = entry.file_name();
                    if name
                        .to_str()
                        .is_some_and(|name| names_session(name, session))
                    {
                        found.push(entry.path());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &dir, e)),
        }
        found.sort();
        let file = match found.len() {
            0 => {
                return Err(HarnessError::SessionNotFound {
                    kind: HarnessKind::Pi,
                    session: session.clone(),
                    searched: dir,
                });
            }
            1 => found.remove(0),
            _ => {
                return Err(HarnessError::LineageAmbiguous {
                    session: session.clone(),
                    rollout: session.clone(),
                    paths: found,
                });
            }
        };
        if std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(HarnessError::SymlinkInState { path: file });
        }
        Self::check_header(&file, root, session)?;
        Ok(file)
    }

    fn check_header(file: &Path, root: &AbsPath, session: &SessionId) -> Result<(), HarnessError> {
        let bad = |reason: String| HarnessError::Malformed {
            what: Input::TranscriptLine,
            path: file.to_path_buf(),
            reason,
        };
        let opened =
            std::fs::File::open(file).map_err(|e| HarnessError::io(IoOp::Read, file, e))?;
        let mut first = String::new();
        std::io::BufReader::new(opened)
            .read_line(&mut first)
            .map_err(|e| HarnessError::io(IoOp::Read, file, e))?;
        let header: Value =
            serde_json::from_str(first.trim()).map_err(|e| bad(format!("header: {e}")))?;
        if header["type"] != "session" || header["id"].as_str() != Some(session.as_str()) {
            return Err(bad(format!(
                "the first line is not the header of session {session}"
            )));
        }
        match header["cwd"].as_str() {
            Some(cwd) if cwd == root.as_str() => Ok(()),
            Some(cwd) => Err(HarnessError::RootMismatch {
                from: AbsPath::parse(cwd).map_err(|e| bad(e.to_string()))?,
                to: root.clone(),
            }),
            None => Err(bad("the header names no cwd".to_owned())),
        }
    }

    /// The capture source of one session file.
    fn source_of(&self, file: &Path) -> Result<Source, HarnessError> {
        let rel = file
            .strip_prefix(self.home_dir())
            .map_err(|_| HarnessError::NotUnderHome {
                path: file.to_path_buf(),
                home: self.home_dir().to_path_buf(),
            })?;
        let parts = pipeline::rel_components(rel)?;
        let mut all = vec![HOME_PREFIX];
        all.extend(parts.iter().map(String::as_str));
        Ok(Source {
            abs: file.to_path_buf(),
            rel: pipeline::rel_path(&all)?,
            carry: Carry::OpaqueRefuseSecrets,
            mutability: Mutability::AppendOnly,
        })
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        self.refuse_running().await?;
        let version = process::installed_version_in(
            self.binary.clone(),
            Some(pipeline::fs_path(&root)),
            &self.path_var,
            PROGRAM,
        )
        .await;
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        pipeline::blocking(move || {
            let file = adapter.session_file(&root, &session)?;
            let sources = [adapter.source_of(&file)?];
            // The file only grows, so no departure record is kept.
            let capturer = Capturer {
                kind: HarnessKind::Pi,
                session: &session,
                root: &root,
                home: &adapter.home,
                version,
                departure: None,
            };
            let manifest = pipeline::capture(capturer, &sources, RedactionCounts::new(), &out)?;
            info!(%session, "pi: captured the session file");
            Ok(manifest)
        })
        .await
    }

    /// True when the tree path `rel` is a file of `session` directly under
    /// the workspace's key, the only place capture takes one from.
    fn restorable(rel: &str, key: &str, session: &SessionId) -> bool {
        let mut parts = rel.split('/');
        let expected = std::iter::once(HOME_PREFIX)
            .chain(SESSIONS)
            .chain(std::iter::once(key));
        for want in expected {
            if parts.next() != Some(want) {
                return false;
            }
        }
        matches!((parts.next(), parts.next()), (Some(name), None) if names_session(name, session))
    }

    fn restore_sync(
        &self,
        root: &AbsPath,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let session = manifest.session().clone();
        let key = key_of(root);
        let held = match self.session_file(root, &session) {
            Ok(file) => vec![self.source_of(&file)?],
            Err(HarnessError::SessionNotFound { .. }) => Vec::new(),
            Err(e) => return Err(e),
        };
        let restored = pipeline::restore(manifest, state, None, &held, |rel, _| {
            Ok(Self::restorable(rel, &key, &session).then(|| Destination {
                path: self.home_dir().join(&rel[HOME_PREFIX.len() + 1..]),
                content: Content::Opaque,
                mutability: Mutability::AppendOnly,
            }))
        })?;
        info!(%session, files = restored.placed.len(), "pi: restored the session file");
        Ok(restored)
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &HarnessKind::Pi, &root, &self.home)?;
        self.refuse_running().await?;
        let adapter = self.clone();
        let state = state.to_path_buf();
        pipeline::blocking(move || adapter.restore_sync(&root, &state, manifest)).await
    }

    /// What the session file records, read before a resume.
    fn read_recorded(file: &Path) -> Result<Recorded, HarnessError> {
        let opened =
            std::fs::File::open(file).map_err(|e| HarnessError::io(IoOp::Read, file, e))?;
        let mut recorded = Recorded::default();
        for line in std::io::BufReader::new(opened).lines() {
            let line = line.map_err(|e| HarnessError::io(IoOp::Read, file, e))?;
            if let Ok(entry) = serde_json::from_str::<Value>(&line) {
                recorded.note(&entry);
            }
        }
        Ok(recorded)
    }

    /// The flags a launch passes, and what it passed and left out.
    fn settings(&self, recorded: &Recorded) -> (Vec<String>, LaunchSettings) {
        let mut args = Vec::new();
        let mut settings = LaunchSettings::default();
        let rows = [
            (
                SettingName::Provider,
                "--provider",
                self.pinned.provider.as_ref(),
                recorded.provider.as_deref(),
            ),
            (
                SettingName::Model,
                "--model",
                self.pinned.model.as_ref(),
                recorded.model.as_deref(),
            ),
            (
                SettingName::Effort,
                "--thinking",
                self.pinned.thinking.as_ref(),
                recorded.thinking.as_deref(),
            ),
        ];
        for (name, flag, pinned, recorded) in rows {
            match pick(pinned, recorded) {
                Some(Ok((value, origin))) => {
                    args.extend([flag.to_owned(), value.clone()]);
                    settings.passed.push(PassedSetting {
                        name,
                        value,
                        origin,
                    });
                }
                Some(Err(text)) => settings.not_carried.push(NotCarried::new(
                    name,
                    &text,
                    "not a plain name pi takes on its command line",
                )),
                None => {}
            }
        }
        (args, settings)
    }

    /// The argument list for `launch`, without the binary. pi reads a
    /// word that starts with `-` as a flag and one that starts with `@` as
    /// a file to attach, and takes no `--`, so such a prompt is passed
    /// with one space before it.
    fn args_for(launch: &Launch, file: Option<&Path>, settings: Vec<String>) -> Vec<String> {
        let mut args = Vec::new();
        if launch.is_headless() {
            args.extend(["-p".to_owned(), "--mode".to_owned(), "json".to_owned()]);
        }
        if let Some(file) = file {
            args.extend(["--session".to_owned(), file.display().to_string()]);
        }
        args.extend(settings);
        if let Some(prompt) = launch.prompt() {
            if prompt.starts_with(['-', '@']) {
                args.push(format!(" {prompt}"));
            } else {
                args.push(prompt.to_owned());
            }
        }
        args
    }

    /// Reads one line of what `pi -p --mode json` printed into `report`
    /// (`docs/json.md` of 0.80.3): the first line is the session header,
    /// whose `id` names the session; each `message_end` of an assistant
    /// message carries an answer, its text blocks joined, the last one
    /// being the turn's, and its failure when `stopReason` is `error`.
    /// Returns true when the line named a session other than the one
    /// `report` held.
    pub fn read_headless_line(report: &mut TurnReport, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            return false;
        };
        match event["type"].as_str() {
            Some("session") => return report.note_session(event["id"].as_str()),
            Some("message_end") if event["message"]["role"] == "assistant" => {
                let message = &event["message"];
                let text: Vec<&str> = message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .collect();
                if !text.is_empty() {
                    report.answer = Some(text.concat());
                }
                if message["stopReason"] == "error" {
                    report.failure = Some(
                        message["errorMessage"]
                            .as_str()
                            .unwrap_or("the turn failed")
                            .to_owned(),
                    );
                }
            }
            _ => {}
        }
        false
    }

    async fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        self.check_env(env)?;
        let root = pipeline::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let (file, recorded) = match launch.resume() {
            Some(session) => {
                let adapter = self.clone();
                let (root, session) = (root.clone(), session.clone());
                pipeline::blocking(move || {
                    let file = adapter.session_file(&root, &session)?;
                    let recorded = Self::read_recorded(&file)?;
                    Ok((Some(file), recorded))
                })
                .await?
            }
            None => (None, Recorded::default()),
        };
        let (flags, settings) = self.settings(&recorded);
        let args = Self::args_for(launch, file.as_deref(), flags);
        let session = launch.resume().cloned();
        let spawned =
            process::spawn(&binary, &args, pipeline::fs_path(&root), env, launch.into()).await?;
        info!(?session, pid = %spawned.pid, "pi: launched, held until released");
        Ok(Running::new(session, spawned).with_settings(settings))
    }
}

impl Harness for Pi {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Pi
    }

    /// The same JSONL file under the same path key.
    fn fidelity(&self) -> Fidelity {
        Fidelity::NativeSession
    }

    fn detect(&self) -> BoxFuture<'_, Result<Option<Installed>, HarnessError>> {
        Box::pin(process::detect(self.binary.clone(), &self.path_var))
    }

    fn capture<'a>(
        &'a self,
        workspace: &'a Path,
        session: &'a SessionId,
        out: &'a Path,
    ) -> BoxFuture<'a, Result<StateManifest, HarnessError>> {
        Box::pin(self.capture_impl(workspace, session, out))
    }

    /// The file only grows, so pi keeps no departure record and this only
    /// checks that the manifest is a pi one.
    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        Box::pin(std::future::ready(pipeline::confirm_departure(
            manifest,
            &HarnessKind::Pi,
            None,
        )))
    }

    fn restore<'a>(
        &'a self,
        workspace: &'a Path,
        state: &'a Path,
    ) -> BoxFuture<'a, Result<Restored, HarnessError>> {
        Box::pin(self.restore_impl(workspace, state))
    }

    fn launch<'a>(
        &'a self,
        workspace: &'a Path,
        launch: Launch,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(async move { self.launch_impl(workspace, &launch, env).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "0199fab0-0000-7000-8000-0000000000aa";

    fn session() -> SessionId {
        SessionId::parse(SESSION).unwrap()
    }

    #[test]
    fn the_key_is_the_path_as_pi_writes_it() {
        let root = AbsPath::parse("/home/u/my proj:x").unwrap();
        assert_eq!(key_of(&root), "--home-u-my proj-x--");
    }

    #[test]
    fn only_the_whole_id_names_a_file() {
        let s = session();
        assert!(names_session(
            &format!("2026-11-08T10-00-00-000Z_{SESSION}.jsonl"),
            &s
        ));
        for bad in [
            format!("{SESSION}.jsonl"),
            format!("x_{SESSION}.json"),
            format!("x_{SESSION}a.jsonl"),
            format!("x_a{SESSION}.jsonl"),
        ] {
            assert!(!names_session(&bad, &s), "{bad}");
        }
        let key = "--w--";
        let good = format!("home/.pi/agent/sessions/{key}/t_{SESSION}.jsonl");
        assert!(Pi::restorable(&good, key, &s));
        let deeper = format!("home/.pi/agent/sessions/{key}/sub/t_{SESSION}.jsonl");
        assert!(!Pi::restorable(&deeper, key, &s));
        assert!(!Pi::restorable(&good.replace(key, "--v--"), key, &s));
    }

    #[test]
    fn the_last_change_and_the_last_answer_name_the_settings() {
        let mut recorded = Recorded::default();
        for line in [
            r#"{"type":"model_change","provider":"openai","modelId":"gpt-x"}"#,
            r#"{"type":"thinking_level_change","thinkingLevel":"low"}"#,
            r#"{"type":"message","message":{"role":"assistant","provider":"anthropic","model":"claude-y"}}"#,
            r#"{"type":"message","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"thinking_level_change","thinkingLevel":"high"}"#,
        ] {
            recorded.note(&serde_json::from_str(line).unwrap());
        }
        assert_eq!(recorded.provider.as_deref(), Some("anthropic"));
        assert_eq!(recorded.model.as_deref(), Some("claude-y"));
        assert_eq!(recorded.thinking.as_deref(), Some("high"));
        let adapter =
            Pi::new(AbsPath::parse("/h").unwrap(), OsString::new()).with_settings(PiSettings {
                thinking: Some(ModelName::parse("off").unwrap()),
                ..PiSettings::default()
            });
        let (args, settings) = adapter.settings(&recorded);
        assert_eq!(
            args,
            [
                "--provider",
                "anthropic",
                "--model",
                "claude-y",
                "--thinking",
                "off"
            ]
        );
        assert_eq!(settings.passed[2].origin, Origin::Pinned);
    }

    #[test]
    fn a_prompt_pi_would_read_as_a_flag_or_a_file_is_kept_a_prompt() {
        for (prompt, passed) in [("-x", " -x"), ("@a.md", " @a.md"), ("go", "go")] {
            let launch = Launch::Headless {
                resume: None,
                prompt: prompt.to_owned(),
            };
            assert_eq!(
                Pi::args_for(&launch, None, Vec::new()),
                ["-p", "--mode", "json", passed]
            );
        }
        let terminal = Launch::Interactive {
            resume: Some(session()),
            prompt: None,
        };
        assert_eq!(
            Pi::args_for(&terminal, Some(Path::new("/h/s.jsonl")), Vec::new()),
            ["--session", "/h/s.jsonl"]
        );
    }

    #[test]
    fn headless_lines_give_the_session_the_answer_and_the_failure() {
        let mut report = TurnReport::default();
        assert!(Pi::read_headless_line(
            &mut report,
            &format!(r#"{{"type":"session","version":3,"id":"{SESSION}","cwd":"/w"}}"#)
        ));
        Pi::read_headless_line(
            &mut report,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"t"},{"type":"text","text":"a"},{"type":"text","text":"b"}],"stopReason":"stop"}}"#,
        );
        Pi::read_headless_line(
            &mut report,
            r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"no"}]}}"#,
        );
        assert_eq!(report.session, Some(session()));
        assert_eq!(report.answer.as_deref(), Some("ab"));
        Pi::read_headless_line(
            &mut report,
            r#"{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"overloaded"}}"#,
        );
        assert_eq!(report.failure.as_deref(), Some("overloaded"));
    }

    #[test]
    fn a_launch_that_moves_the_session_directory_is_refused() {
        let adapter = Pi::new(AbsPath::parse("/h").unwrap(), OsString::new());
        let var =
            |name: &str, value: &str| vec![(EnvVarName::parse(name).unwrap(), value.to_owned())];
        assert!(
            adapter
                .check_env(&var("PI_CODING_AGENT_DIR", "/h/.pi/agent"))
                .is_ok()
        );
        assert!(adapter.check_env(&var("PI_CODING_AGENT_DIR", "")).is_ok());
        for (name, value) in [
            ("PI_CODING_AGENT_DIR", "/elsewhere"),
            ("PI_CODING_AGENT_SESSION_DIR", "/h/.pi/agent/sessions"),
        ] {
            assert!(
                matches!(
                    adapter.check_env(&var(name, value)),
                    Err(HarnessError::ConfigMoved { .. })
                ),
                "{name}={value}"
            );
        }
    }

    #[test]
    fn a_default_agent_dir_does_not_hide_a_moved_session_dir() {
        let adapter = Pi::new(AbsPath::parse("/h").unwrap(), OsString::new());
        let env = vec![
            (
                EnvVarName::parse("PI_CODING_AGENT_DIR").unwrap(),
                "/h/.pi/agent".to_owned(),
            ),
            (
                EnvVarName::parse("PI_CODING_AGENT_SESSION_DIR").unwrap(),
                "/elsewhere".to_owned(),
            ),
        ];
        assert!(matches!(
            adapter.check_env(&env),
            Err(HarnessError::ConfigMoved {
                var: "PI_CODING_AGENT_SESSION_DIR",
                ..
            })
        ));
    }
}
