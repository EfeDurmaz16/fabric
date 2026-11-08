//! The OpenCode adapter (PR 5; contract in `docs/harness-adapters.md`,
//! section OpenCode).
//!
//! Invariant: OpenCode keeps every session, message and part of every
//! project in one `SQLite` file, `~/.local/share/opencode/opencode.db`,
//! beside the tables that hold its logins (`account.access_token`,
//! `account.refresh_token`, `control_account.access_token`,
//! `credential.value`, `session_share.secret`; schema read on 1.18.34,
//! 2026-11-08). That file never travels, in whole or in part, and Fabric
//! never opens it. The adapter carries one session as the JSON
//! `opencode export <id>` prints, written to
//! `~/.local/share/opencode/fabric/<id>.export.json` and staged from
//! there byte for byte, refused when a line holds a secret shape. Restore
//! places that file and hands it to `opencode import <file>`, which
//! prints `Imported session: <id>`. On 1.18.34 the import keeps the id
//! (checked on 2026-11-08 against the installed binary with a synthetic
//! export in an empty data directory), so the restore reaches
//! `NativeSession`; an OpenCode that prints another id is recorded in
//! `Restored::session` with `NewNativeSessionWithPackage`, never the
//! higher level (decision of 2026-11-08).
//!
//! The import adds the rows a session lacks and leaves the rows it has
//! (1.18.34: the session row of an earlier import is kept, new messages
//! are added, none is removed). So before it runs, the adapter exports
//! the session this node already holds, if any, and goes on only when
//! its messages are a prefix of the incoming ones; anything else is work
//! done here after the job left, and the restore stops with
//! `StateDiverged`, as for a rollout. An import that fails part way keeps
//! the rows it wrote, so the adapter then deletes the session
//! (`opencode session delete <id>`), which holds nothing the job lacks.
//!
//! A resume runs `opencode run -s <id>` (headless, `--format json`) or
//! `opencode -s <id>` (terminal) with the model and agent of the last
//! user message and the variant of the session row, each one the adapter
//! was configured with winning, after the version guard: the export
//! records the OpenCode that created the session (`info.version`), and
//! the job carries the newest one that continued it (`with_known_writer`);
//! a binary older than either is refused before it starts. `opencode run --format json`
//! prints one JSON object per line, `{"type", "timestamp", "sessionID",
//! ...}`, with the types `step_start`, `step_finish`, `text`, `reasoning`,
//! `tool_use` and `error` (read on 2026-11-08 from the JavaScript the
//! 1.18.34 binary embeds). The adapter's own `export` and `import` run
//! with `HOME` set to its home and `XDG_DATA_HOME` to `<home>/.local/share`,
//! and a launch whose environment moves `XDG_DATA_HOME` elsewhere is
//! refused, so OpenCode reads and writes the store the adapter carries.
//! A running process that holds the database open refuses capture and
//! restore, since OpenCode's own writer rule has not been read from its
//! source.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

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
use crate::version::HarnessVersion;

/// The program name on `PATH`.
pub const PROGRAM: &str = "opencode";

/// The version the layout, the export and the event names were read on.
pub const READ_ON: &str = "1.18.34";

/// OpenCode's data directory under the home, as tree path components.
const DATA_DIR: [&str; 3] = [".local", "share", "opencode"];

/// The directory under the data directory that holds the exports Fabric
/// carries; OpenCode itself never reads it.
pub const EXPORT_DIR: &str = "fabric";

/// The variable that moves OpenCode's data directory.
const DATA_VAR: &str = "XDG_DATA_HOME";

/// How long an `opencode export` or `import` may take.
const HELPER_TIMEOUT: Duration = Duration::from_mins(2);

/// What `opencode import` prints before the id it imported.
const IMPORTED: &str = "Imported session: ";

/// What `opencode export` prints on its error output for an unknown id.
const NOT_FOUND: &str = "Session not found";

/// Settings the adapter passes on every run, each one winning over the
/// one the session records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenCodeSettings {
    /// The model, as `provider/model`.
    pub model: Option<ModelName>,
    /// The agent profile, such as `build`.
    pub agent: Option<ModelName>,
    /// The provider-specific reasoning effort, such as `high`.
    pub variant: Option<ModelName>,
}

/// The OpenCode adapter for one home directory.
#[derive(Debug, Clone)]
pub struct OpenCode {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
    pinned: OpenCodeSettings,
    known_writer: Option<HarnessVersion>,
}

/// True for an id of OpenCode's shape: `ses_` and letters and digits.
/// Anything else is no OpenCode session, and no process starts for it.
fn is_session_id(session: &SessionId) -> bool {
    session
        .as_str()
        .strip_prefix("ses_")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// What one session export says that the adapter acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Exported {
    /// The workspace the session belongs to (`info.directory`).
    directory: Option<String>,
    /// The OpenCode that wrote it (`info.version`).
    version: Option<String>,
    /// The provider and model of the last user message, else of the session.
    model: Option<(String, String)>,
    /// The agent of the last user message, else of the session.
    agent: Option<String>,
    /// The variant of the session's model.
    variant: Option<String>,
    /// Every message, in order.
    messages: Vec<Value>,
}

fn text_of(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// Reads an export, checking that it is the export of `session`.
fn parse_export(bytes: &[u8], session: &SessionId, path: &Path) -> Result<Exported, HarnessError> {
    let bad = |reason: String| HarnessError::Malformed {
        what: Input::SessionExport,
        path: path.to_path_buf(),
        reason,
    };
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
    let info = &doc["info"];
    if info["id"].as_str() != Some(session.as_str()) {
        return Err(bad(format!("it is not the export of session {session}")));
    }
    let messages = match &doc["messages"] {
        Value::Array(messages) => messages.clone(),
        Value::Null => Vec::new(),
        _ => return Err(bad("`messages` is not a list".to_owned())),
    };
    let last_user = messages
        .iter()
        .rev()
        .map(|m| &m["info"])
        .find(|m| m["role"] == "user");
    let model = last_user
        .and_then(|m| {
            Some((
                text_of(&m["model"]["providerID"])?,
                text_of(&m["model"]["modelID"])?,
            ))
        })
        .or_else(|| {
            Some((
                text_of(&info["model"]["providerID"])?,
                text_of(&info["model"]["id"])?,
            ))
        });
    let agent = last_user
        .and_then(|m| text_of(&m["agent"]))
        .or_else(|| text_of(&info["agent"]));
    Ok(Exported {
        directory: text_of(&info["directory"]),
        version: text_of(&info["version"]),
        model,
        agent,
        variant: text_of(&info["model"]["variant"]),
        messages,
    })
}

/// How the session this node holds relates to the incoming one.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    /// This node holds no such session.
    Absent,
    /// It holds every incoming message already.
    Same,
    /// It holds a prefix of the incoming messages.
    Prefix,
    /// It holds a message the incoming session lacks or has otherwise.
    Diverged,
}

fn compare(local: &[Value], incoming: &[Value]) -> Held {
    if local.len() > incoming.len() || local.iter().zip(incoming).any(|(a, b)| a != b) {
        Held::Diverged
    } else if local.len() == incoming.len() {
        Held::Same
    } else {
        Held::Prefix
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

impl OpenCode {
    /// An adapter under `home`, finding `opencode` on the `PATH` value
    /// `path_var`.
    #[must_use]
    pub fn new(home: AbsPath, path_var: OsString) -> Self {
        let binary = process::find_on_path(PROGRAM, &path_var);
        OpenCode {
            home,
            binary,
            path_var,
            pinned: OpenCodeSettings::default(),
            known_writer: None,
        }
    }

    /// The newest OpenCode known to have written the session, which the
    /// job carries because the export names only the OpenCode that created
    /// it (`info.version` is stamped on the session row when it is made,
    /// and `opencode import` keeps whatever version the row carries,
    /// checked on 1.18.34 on 2026-11-08; whether a later turn restamps it
    /// is not shown). A resume refuses an OpenCode older than the newer of
    /// this and the export's version, and a capture records at least this
    /// one, so a node that never ran the session cannot lower it.
    #[must_use]
    pub fn with_known_writer(mut self, version: Option<HarnessVersion>) -> Self {
        self.known_writer = version;
        self
    }

    /// The version a capture records: the installed one, or the known
    /// writer when that is newer or the installed one cannot be read.
    fn captured_version(&self, installed: Option<String>) -> Option<String> {
        let Some(known) = &self.known_writer else {
            return installed;
        };
        match installed.as_deref().map(HarnessVersion::parse) {
            Some(Ok(local)) if local >= *known => installed,
            _ => Some(known.as_str().to_owned()),
        }
    }

    /// Settings passed on every run, each winning over the session's own.
    #[must_use]
    pub fn with_settings(mut self, pinned: OpenCodeSettings) -> Self {
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

    fn data_dir(&self) -> PathBuf {
        DATA_DIR
            .iter()
            .fold(self.home_dir().to_path_buf(), |dir, part| dir.join(part))
    }

    fn database(&self) -> PathBuf {
        self.data_dir().join("opencode.db")
    }

    fn export_file(&self, session: &SessionId) -> PathBuf {
        self.data_dir()
            .join(EXPORT_DIR)
            .join(format!("{session}.export.json"))
    }

    fn export_rel(session: &SessionId) -> String {
        let mut parts = vec![HOME_PREFIX];
        parts.extend(DATA_DIR);
        parts.push(EXPORT_DIR);
        format!("{}/{session}.export.json", parts.join("/"))
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        self.binary
            .clone()
            .ok_or_else(|| HarnessError::NotInstalled {
                program: PROGRAM.to_owned(),
            })
    }

    fn not_found(&self, session: &SessionId) -> HarnessError {
        HarnessError::SessionNotFound {
            kind: HarnessKind::OpenCode,
            session: session.clone(),
            searched: self.database(),
        }
    }

    /// Refuses a launch environment that moves OpenCode's data directory
    /// away from `<home>/.local/share`, where the adapter reads and writes.
    pub fn check_env(&self, env: &[(EnvVarName, String)]) -> Result<(), HarnessError> {
        let expected = self.home_dir().join(".local/share");
        let moved = env.iter().any(|(name, value)| {
            name.as_str() == DATA_VAR && !value.is_empty() && Path::new(value) != expected
        });
        if moved {
            return Err(HarnessError::ConfigMoved {
                program: PROGRAM.to_owned(),
                var: DATA_VAR,
                moves: "the session store",
            });
        }
        Ok(())
    }

    /// The environment of the adapter's own `export` and `import`: this
    /// process's, with `PATH` set to the adapter's and `HOME` and
    /// `XDG_DATA_HOME` naming the adapter's home.
    fn helper_env(&self) -> Vec<(EnvVarName, String)> {
        let mut env: Vec<(EnvVarName, String)> = process::probe_env(&self.path_var)
            .into_iter()
            .filter(|(name, _)| !matches!(name.as_str(), "HOME" | DATA_VAR))
            .collect();
        let data = self.home_dir().join(".local/share");
        for (name, value) in [("HOME", self.home_dir()), (DATA_VAR, data.as_path())] {
            if let (Ok(name), Some(value)) = (EnvVarName::parse(name), value.to_str()) {
                env.push((name, value.to_owned()));
            }
        }
        env
    }

    /// Runs `opencode args...` in `cwd` and returns whether it succeeded,
    /// its output and its error output.
    async fn helper(
        &self,
        args: &[&str],
        cwd: &Path,
    ) -> Result<(bool, Vec<u8>, String), HarnessError> {
        let binary = self.binary()?;
        let env = self.helper_env();
        let run = tokio::process::Command::new(&binary)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(crate::markers::child_env(&env))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(HELPER_TIMEOUT, run)
            .await
            .map_err(|_| HarnessError::ProcessQuery {
                program: PROGRAM,
                reason: format!("`opencode {}` timed out", args.join(" ")),
            })?
            .map_err(|source| HarnessError::Spawn {
                program: PROGRAM.to_owned(),
                source,
            })?;
        Ok((
            output.status.success(),
            output.stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }

    /// The export of `session` this node's OpenCode holds, or `None` when
    /// it holds no such session.
    async fn export(
        &self,
        session: &SessionId,
        cwd: &Path,
    ) -> Result<Option<Vec<u8>>, HarnessError> {
        let (ok, stdout, stderr) = self.helper(&["export", session.as_str()], cwd).await?;
        if ok {
            return Ok(Some(stdout));
        }
        if stderr.contains(NOT_FOUND) {
            return Ok(None);
        }
        Err(HarnessError::ProcessQuery {
            program: PROGRAM,
            reason: format!(
                "`opencode export {session}` failed: {}",
                crate::harness::shown(stderr.trim())
            ),
        })
    }

    /// Hands `file` to `opencode import` and returns the id it printed.
    async fn import(&self, file: &Path, cwd: &Path) -> Result<SessionId, HarnessError> {
        let path = file.to_str().ok_or_else(|| HarnessError::NotUtf8 {
            path: file.to_path_buf(),
        })?;
        let (ok, stdout, stderr) = self.helper(&["import", path], cwd).await?;
        let printed = String::from_utf8_lossy(&stdout);
        let id = printed
            .lines()
            .chain(stderr.lines())
            .find_map(|line| line.trim().strip_prefix(IMPORTED))
            .map(str::trim);
        match id.map(SessionId::parse) {
            Some(Ok(id)) if ok => Ok(id),
            _ => Err(HarnessError::ProcessQuery {
                program: PROGRAM,
                reason: format!(
                    "`opencode import` did not name the session it imported: {}",
                    crate::harness::shown(format!("{printed} {stderr}").trim())
                ),
            }),
        }
    }

    /// Deletes `session` from the store after an import of it failed.
    /// `opencode import` 1.18.34 writes the session row and each message in
    /// turn and stops at the first one it refuses, keeping what it wrote
    /// (checked on 2026-11-08 against the installed binary with a
    /// synthetic export), and a message left without all its parts would
    /// read as diverged on every later restore. The import ran only when
    /// this node held nothing the incoming export lacks (`Held::Absent` or
    /// `Held::Prefix`), so deleting loses nothing the job does not carry,
    /// and the next restore imports into an empty store. A failed delete
    /// is logged; the import's error is the one returned.
    async fn take_back(&self, session: &SessionId, cwd: &Path) {
        match self
            .helper(&["session", "delete", session.as_str()], cwd)
            .await
        {
            Ok((true, _, _)) => {
                info!(%session, "opencode: removed the session a failed import left");
            }
            Ok((false, _, stderr)) if stderr.contains(NOT_FOUND) => {}
            Ok((false, _, stderr)) => tracing::warn!(
                %session,
                error = %crate::harness::shown(stderr.trim()),
                "opencode: could not remove the session a failed import left"
            ),
            Err(e) => tracing::warn!(
                %session,
                error = %e,
                "opencode: could not remove the session a failed import left"
            ),
        }
    }

    /// Refuses while any process holds the database open: OpenCode keeps
    /// it open while it runs, and its writer rule is not read yet.
    fn refuse_open_store(&self) -> Result<(), HarnessError> {
        let db = self.database();
        if !db.exists() {
            return Ok(());
        }
        let pids = process::holding(&[db])?;
        if pids.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            })
        }
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        if !is_session_id(session) {
            return Err(self.not_found(session));
        }
        let cwd = pipeline::fs_path(&root).to_path_buf();
        let store = self.clone();
        pipeline::blocking(move || store.refuse_open_store()).await?;
        let Some(bytes) = self.export(session, &cwd).await? else {
            return Err(self.not_found(session));
        };
        let file = self.export_file(session);
        let exported = parse_export(&bytes, session, &file)?;
        // ADR 0011: the session is carried under its own workspace path.
        if let Some(dir) = &exported.directory
            && dir != root.as_str()
        {
            return Err(HarnessError::RootMismatch {
                from: AbsPath::parse(dir).map_err(|e| HarnessError::Malformed {
                    what: Input::SessionExport,
                    path: file.clone(),
                    reason: e.to_string(),
                })?,
                to: root,
            });
        }
        let version =
            process::installed_version_in(self.binary.clone(), Some(&cwd), &self.path_var, PROGRAM)
                .await;
        let version = self.captured_version(version);
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        pipeline::blocking(move || adapter.capture_sync(&root, &session, &bytes, &out, version))
            .await
    }

    fn capture_sync(
        &self,
        root: &AbsPath,
        session: &SessionId,
        bytes: &[u8],
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let file = self.export_file(session);
        pipeline::refuse_symlinks(self.home_dir(), &file)?;
        if let Some(dir) = file.parent() {
            crate::durable::create_dirs_durable(dir)?;
        }
        crate::durable::write_atomic_with(&file, |w| {
            w.write_all(bytes)
                .map_err(|e| HarnessError::io(IoOp::Write, &file, e))
        })?;
        let sources = [Source {
            abs: file.clone(),
            rel: pipeline::rel_path(&[&Self::export_rel(session)])?,
            carry: Carry::OpaqueRefuseSecrets,
            mutability: Mutability::Replaceable,
        }];
        // The store, not this copy, is what a restore compares, so no
        // departure record is kept for it.
        let capturer = Capturer {
            kind: HarnessKind::OpenCode,
            session,
            root,
            home: &self.home,
            version,
            departure: None,
        };
        let manifest = pipeline::capture(capturer, &sources, RedactionCounts::new(), out)?;
        info!(%session, "opencode: captured the session export");
        Ok(manifest)
    }

    /// Places the staged export. The copy under `fabric/` is Fabric's own,
    /// derived from the store, which the caller has already compared with
    /// the incoming session; one left by an earlier capture or restore is
    /// removed first, so it never reads as a local edit.
    fn restore_sync(
        &self,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let session = manifest.session().clone();
        let file = self.export_file(&session);
        let rel = Self::export_rel(&session);
        pipeline::refuse_symlinks(self.home_dir(), &file)?;
        if file.exists() {
            crate::durable::remove_durable(&file)?;
        }
        pipeline::restore(manifest, state, None, &[], |path, _| {
            Ok((path == rel).then(|| Destination {
                path: file.clone(),
                content: Content::Opaque,
                mutability: Mutability::Replaceable,
            }))
        })
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &HarnessKind::OpenCode, &root, &self.home)?;
        let session = manifest.session().clone();
        if !is_session_id(&session) {
            return Err(self.not_found(&session));
        }
        let store = self.clone();
        pipeline::blocking(move || store.refuse_open_store()).await?;
        let cwd = pipeline::fs_path(&root).to_path_buf();
        // What this node holds is read before anything is placed, so a
        // refusal leaves the node as it was.
        let local = self.export(&session, &cwd).await?;
        let staged = state.join(Self::export_rel(&session));
        let incoming =
            std::fs::read(&staged).map_err(|e| HarnessError::io(IoOp::Read, &staged, e))?;
        let incoming = parse_export(&incoming, &session, &staged)?;
        let held = match local {
            None => Held::Absent,
            Some(local) => compare(
                &parse_export(&local, &session, &self.database())?.messages,
                &incoming.messages,
            ),
        };
        if held == Held::Diverged {
            return Err(HarnessError::StateDiverged {
                path: self.database(),
            });
        }
        let adapter = self.clone();
        let tree = state.to_path_buf();
        let mut restored =
            pipeline::blocking(move || adapter.restore_sync(&tree, manifest)).await?;
        let continues = match held {
            Held::Same => session.clone(),
            _ => match self.import(&self.export_file(&session), &cwd).await {
                Ok(id) => id,
                Err(e) => {
                    self.take_back(&session, &cwd).await;
                    return Err(e);
                }
            },
        };
        if continues != session {
            restored.fidelity = Fidelity::NewNativeSessionWithPackage;
        }
        info!(
            captured = %session,
            continues = %continues,
            fidelity = %restored.fidelity,
            "opencode: restored the session"
        );
        restored.session = continues;
        Ok(restored)
    }

    /// The flags a launch passes, and what it passed and left out, from
    /// what the session records and what the adapter was configured with.
    fn settings(&self, recorded: &Exported, headless: bool) -> (Vec<String>, LaunchSettings) {
        let mut args = Vec::new();
        let mut settings = LaunchSettings::default();
        let recorded_model = recorded
            .model
            .as_ref()
            .map(|(provider, model)| format!("{provider}/{model}"));
        let mut passed = |name, value: &str, origin| {
            settings.passed.push(PassedSetting {
                name,
                value: value.to_owned(),
                origin,
            });
        };
        let mut left_out = Vec::new();
        match pick(self.pinned.model.as_ref(), recorded_model.as_deref()) {
            Some(Ok((model, origin))) => {
                match model.split_once('/') {
                    Some((provider, name)) => {
                        passed(SettingName::Provider, provider, origin);
                        passed(SettingName::Model, name, origin);
                    }
                    None => passed(SettingName::Model, &model, origin),
                }
                args.extend(["-m".to_owned(), model]);
            }
            Some(Err(text)) => left_out.push((SettingName::Model, text, "not a plain model name")),
            None => {}
        }
        match pick(self.pinned.agent.as_ref(), recorded.agent.as_deref()) {
            Some(Ok((agent, origin))) => {
                passed(SettingName::Agent, &agent, origin);
                args.extend(["--agent".to_owned(), agent]);
            }
            Some(Err(text)) => left_out.push((SettingName::Agent, text, "not a plain agent name")),
            None => {}
        }
        match pick(self.pinned.variant.as_ref(), recorded.variant.as_deref()) {
            Some(Ok((variant, _))) if !headless => left_out.push((
                SettingName::Effort,
                variant,
                "the terminal interface takes no --variant",
            )),
            Some(Ok((variant, origin))) => {
                passed(SettingName::Effort, &variant, origin);
                args.extend(["--variant".to_owned(), variant]);
            }
            Some(Err(text)) => {
                left_out.push((SettingName::Effort, text, "not a plain variant name"));
            }
            None => {}
        }
        settings.not_carried = left_out
            .into_iter()
            .map(|(name, value, reason)| NotCarried::new(name, &value, reason))
            .collect();
        (args, settings)
    }

    /// The argument list for `launch`, without the binary.
    fn args_for(launch: &Launch, settings: Vec<String>) -> Vec<String> {
        let mut args = Vec::new();
        if launch.is_headless() {
            args.extend(["run".to_owned(), "--format".to_owned(), "json".to_owned()]);
        }
        if let Some(session) = launch.resume() {
            args.extend(["-s".to_owned(), session.as_str().to_owned()]);
        }
        args.extend(settings);
        match (launch, launch.prompt()) {
            (Launch::Headless { .. }, Some(prompt)) => {
                args.extend(["--".to_owned(), prompt.to_owned()]);
            }
            (Launch::Interactive { .. }, Some(prompt)) => {
                args.push(format!("--prompt={prompt}"));
            }
            (_, None) => {}
        }
        args
    }

    /// Reads one line of what `opencode run --format json` printed into
    /// `report`. Every event names its session (`sessionID`); a `text`
    /// event carries a finished text part, the last one being the turn's
    /// answer; an `error` event carries the failure, its message in
    /// `error.data.message` or else its name. Any other line is left
    /// alone. Returns true when the line named a session other than the
    /// one `report` held.
    pub fn read_headless_line(report: &mut TurnReport, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            return false;
        };
        let named = report.note_session(event["sessionID"].as_str());
        match event["type"].as_str() {
            Some("text") => {
                if let Some(text) = event["part"]["text"].as_str() {
                    report.answer = Some(text.to_owned());
                }
            }
            Some("error") => {
                let error = &event["error"];
                report.failure = Some(
                    error["data"]["message"]
                        .as_str()
                        .or_else(|| error["name"].as_str())
                        .unwrap_or("the turn failed")
                        .to_owned(),
                );
            }
            _ => {}
        }
        named
    }

    /// What the session records, read before a resume, and the version of
    /// the binary that will run it; refused when that binary is older than
    /// the OpenCode the export names. A session this node does not hold
    /// records nothing to carry or to guard; OpenCode then says itself
    /// that it found none.
    async fn read_session(
        &self,
        binary: &Path,
        cwd: &Path,
        env: &[(EnvVarName, String)],
        session: &SessionId,
    ) -> Result<(Exported, Option<HarnessVersion>), HarnessError> {
        let Some(bytes) = self.export(session, cwd).await? else {
            return Ok((Exported::default(), None));
        };
        let recorded = parse_export(&bytes, session, &self.database())?;
        let created = recorded
            .version
            .as_deref()
            .map(HarnessVersion::parse)
            .transpose()
            .map_err(|e| HarnessError::Malformed {
                what: Input::SessionExport,
                path: self.database(),
                reason: e.to_string(),
            })?;
        let Some(writer) = created.into_iter().chain(self.known_writer.clone()).max() else {
            return Ok((recorded, None));
        };
        let runs =
            crate::version::refuse_older(PROGRAM, (binary, cwd, env), None, session, &writer)
                .await?;
        Ok((recorded, Some(runs)))
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
        let cwd = pipeline::fs_path(&root);
        let (recorded, runs) = match launch.resume() {
            Some(session) if !is_session_id(session) => return Err(self.not_found(session)),
            Some(session) => self.read_session(&binary, cwd, env, session).await?,
            None => (Exported::default(), None),
        };
        let (flags, settings) = self.settings(&recorded, launch.is_headless());
        let args = Self::args_for(launch, flags);
        let session = launch.resume().cloned();
        let spawned = process::spawn(&binary, &args, cwd, env, launch.into()).await?;
        info!(?session, pid = %spawned.pid, "opencode: launched, held until released");
        Ok(Running::new(session, spawned)
            .with_settings(settings)
            .with_version(runs))
    }
}

impl Harness for OpenCode {
    fn kind(&self) -> HarnessKind {
        HarnessKind::OpenCode
    }

    /// `NativeSession` when `opencode import` keeps the id; a restore that
    /// gets another id back reports `NewNativeSessionWithPackage`.
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

    /// A restore compares the store itself with the incoming session, so
    /// no departure record is kept and this only checks the manifest's kind.
    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        Box::pin(std::future::ready(pipeline::confirm_departure(
            manifest,
            &HarnessKind::OpenCode,
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

    const SESSION: &str = "ses_0fabric0opencode0session001";

    fn session() -> SessionId {
        SessionId::parse(SESSION).unwrap()
    }

    fn export(messages: &str) -> String {
        format!(
            "{{\"info\":{{\"id\":\"{SESSION}\",\"directory\":\"/w\",\"version\":\"1.18.34\",\"agent\":\"plan\",\"model\":{{\"id\":\"m0\",\"providerID\":\"p0\",\"variant\":\"high\"}}}},\"messages\":[{messages}]}}"
        )
    }

    fn user(id: &str, agent: &str, model: &str) -> String {
        format!(
            "{{\"info\":{{\"id\":\"{id}\",\"role\":\"user\",\"agent\":\"{agent}\",\"model\":{{\"providerID\":\"anthropic\",\"modelID\":\"{model}\"}}}},\"parts\":[]}}"
        )
    }

    #[test]
    fn only_an_opencode_shaped_id_is_a_session() {
        assert!(is_session_id(&session()));
        for bad in ["ses_", "019f233e-bb33", "ses_a.b", "xses_abc"] {
            assert!(!is_session_id(&SessionId::parse(bad).unwrap()), "{bad}");
        }
    }

    #[test]
    fn the_last_user_message_names_the_model_and_the_agent() {
        let text = export(&[user("m1", "build", "one"), user("m2", "plan", "two")].join(","));
        let read = parse_export(text.as_bytes(), &session(), Path::new("x")).unwrap();
        assert_eq!(read.model, Some(("anthropic".into(), "two".into())));
        assert_eq!(read.agent.as_deref(), Some("plan"));
        assert_eq!(read.variant.as_deref(), Some("high"));
        assert_eq!(read.version.as_deref(), Some("1.18.34"));
        let bare = parse_export(export("").as_bytes(), &session(), Path::new("x")).unwrap();
        assert_eq!(bare.model, Some(("p0".into(), "m0".into())));
        let other = SessionId::parse("ses_other").unwrap();
        assert!(matches!(
            parse_export(text.as_bytes(), &other, Path::new("x")),
            Err(HarnessError::Malformed { .. })
        ));
    }

    #[test]
    fn a_held_session_must_be_a_prefix_of_the_incoming_one() {
        let a: Value = serde_json::from_str(&user("m1", "build", "one")).unwrap();
        let b: Value = serde_json::from_str(&user("m2", "build", "one")).unwrap();
        let c: Value = serde_json::from_str(&user("m3", "build", "one")).unwrap();
        let ab = [a.clone(), b];
        assert_eq!(compare(&ab, &ab), Held::Same);
        assert_eq!(compare(&ab[..1], &ab), Held::Prefix);
        assert_eq!(compare(&ab, &ab[..1]), Held::Diverged);
        assert_eq!(compare(&[a, c], &ab), Held::Diverged);
    }

    #[test]
    fn a_resume_passes_the_recorded_settings_and_pinned_ones_win() {
        let adapter = OpenCode::new(AbsPath::parse("/h").unwrap(), OsString::new());
        let text = export(&user("m1", "build", "claude-x"));
        let recorded = parse_export(text.as_bytes(), &session(), Path::new("x")).unwrap();
        let (args, settings) = adapter.settings(&recorded, true);
        assert_eq!(
            args,
            [
                "-m",
                "anthropic/claude-x",
                "--agent",
                "build",
                "--variant",
                "high"
            ]
        );
        assert!(settings.passed.iter().all(|p| p.origin == Origin::Session));
        let pinned = adapter.with_settings(OpenCodeSettings {
            agent: Some(ModelName::parse("plan").unwrap()),
            ..OpenCodeSettings::default()
        });
        let (args, settings) = pinned.settings(&recorded, false);
        assert_eq!(args, ["-m", "anthropic/claude-x", "--agent", "plan"]);
        assert_eq!(settings.not_carried.len(), 1, "{settings:?}");
        assert_eq!(settings.not_carried[0].name, SettingName::Effort);
        let agent = settings
            .passed
            .iter()
            .find(|p| p.name == SettingName::Agent)
            .unwrap();
        assert_eq!(agent.origin, Origin::Pinned);
    }

    #[test]
    fn args_follow_the_cli_surface() {
        let resume = Some(session());
        let headless = Launch::Headless {
            resume: resume.clone(),
            prompt: "-p looks like a flag".into(),
        };
        assert_eq!(
            OpenCode::args_for(&headless, vec!["-m".into(), "p/m".into()]),
            [
                "run",
                "--format",
                "json",
                "-s",
                SESSION,
                "-m",
                "p/m",
                "--",
                "-p looks like a flag"
            ]
        );
        let terminal = Launch::Interactive {
            resume,
            prompt: Some("go".into()),
        };
        assert_eq!(
            OpenCode::args_for(&terminal, Vec::new()),
            ["-s", SESSION, "--prompt=go"]
        );
    }

    #[test]
    fn headless_events_give_the_session_the_answer_and_the_failure() {
        let mut report = TurnReport::default();
        let line = |kind: &str, rest: &str| {
            format!("{{\"type\":\"{kind}\",\"timestamp\":1,\"sessionID\":\"{SESSION}\"{rest}}}")
        };
        assert!(OpenCode::read_headless_line(
            &mut report,
            &line("step_start", ",\"part\":{\"type\":\"step-start\"}")
        ));
        assert!(!OpenCode::read_headless_line(
            &mut report,
            &line("text", ",\"part\":{\"type\":\"text\",\"text\":\"first\"}")
        ));
        OpenCode::read_headless_line(
            &mut report,
            &line("text", ",\"part\":{\"type\":\"text\",\"text\":\"last\"}"),
        );
        OpenCode::read_headless_line(&mut report, "not json");
        assert_eq!(report.session.as_ref(), Some(&session()));
        assert_eq!(report.answer.as_deref(), Some("last"));
        OpenCode::read_headless_line(
            &mut report,
            &line(
                "error",
                ",\"error\":{\"name\":\"APIError\",\"data\":{\"message\":\"rate limited\"}}",
            ),
        );
        assert_eq!(report.failure.as_deref(), Some("rate limited"));
    }

    #[test]
    fn a_launch_that_moves_the_data_directory_is_refused() {
        let adapter = OpenCode::new(AbsPath::parse("/h").unwrap(), OsString::new());
        let var = |value: &str| vec![(EnvVarName::parse(DATA_VAR).unwrap(), value.to_owned())];
        assert!(adapter.check_env(&var("/h/.local/share")).is_ok());
        assert!(adapter.check_env(&var("")).is_ok());
        assert!(matches!(
            adapter.check_env(&var("/elsewhere")),
            Err(HarnessError::ConfigMoved { .. })
        ));
    }
}
