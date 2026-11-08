//! The Gemini CLI adapter (PR 5, owner B; contract in
//! `docs/harness-adapters.md`, section Gemini CLI).
//!
//! Invariant: Gemini CLI records a chat as
//! `~/.gemini/tmp/<slug>/chats/session-<timestamp>-<first 8 of id>.jsonl`,
//! with a `-<n>` collision form and subagent files under
//! `chats/<full id>/`, and keeps the session's plans, tracker and tasks
//! under `<slug>/<id>/`; the slug is assigned by `~/.gemini/projects.json`
//! and owned by a `.project_root` marker inside the slug directory that
//! holds the workspace path (`chatRecordingService.ts`,
//! `chatRecordingTypes.ts`, `projectRegistry.ts` and `storage.ts` on
//! google-gemini/gemini-cli `main`, read 2026-11-08; v0.67.0). The first
//! line of a chat file is its metadata record and names the full
//! `sessionId`; every later line is appended (`appendRecord`), and the
//! file is rewritten only when Gemini could not read it.
//!
//! This adapter resolves the id to its one chat file by the 8-character
//! prefix and then the full id of the first record, so a second session
//! that shares the prefix is never carried, and carries the chat and its
//! subagent files (append-only) and the `<id>/` side directories
//! (rewritable), byte for byte, refused on a secret shape. It never
//! carries `projects.json`, which names every project of the home, nor
//! `oauth_creds.json`, `google_accounts.json`, `settings.json` or `.env`.
//! A restore places the files under the captured slug and writes the
//! `.project_root` marker when it is missing; Gemini's own registry then
//! finds the slug by its marker (`findExistingSlugForPath`). A restore is
//! refused when this node's registry or a marker gives the workspace or
//! the slug to another project, since Gemini would then read another chat
//! directory and the session would be lost.
//!
//! Gemini CLI records no version in a chat, so it is the writer-version
//! rule's second case: a capture records the installed version, asked in
//! the workspace, or the job's known writer when that is newer
//! (`with_known_writer`), and a resume refuses a `gemini` older than it.
//! Gemini takes no lock on a chat, so capture and restore refuse while any
//! `gemini` runs. A resume continues with `gemini --resume <id>`, headless
//! with `--output-format stream-json --prompt=<text>`, passing the model of
//! the chat's last answer. Gemini CLI is not installed on the owner's Mac:
//! the fake in the tests is built from the documented format alone.
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use fabric_core::version::HarnessVersion;
use serde_json::Value;
use tracing::info;

use crate::departure::Records;
use crate::durable;
use crate::error::{HarnessError, Input, IoOp};
use crate::harness::{
    Fidelity, Harness, Installed, Launch, LaunchSettings, MAX_ANSWER, ModelName, NotCarried,
    Origin, PassedSetting, Restored, Running, SettingName, TurnReport,
};
use crate::manifest::{Content, HOME_PREFIX, Mutability, StateManifest, under_home};
use crate::pipeline::{self, Capturer, Destination};
use crate::process;
use crate::redact::RedactionCounts;
use crate::stage::{Carry, Source};

/// The program name on `PATH`.
pub const PROGRAM: &str = "gemini";

/// The Gemini CLI version whose layout this adapter was written to.
pub const READ_ON: &str = "0.67.0";

/// Gemini's directory under the home.
const GEMINI_DIR: &str = ".gemini";
/// The directory of the per-project slugs under `GEMINI_DIR`.
const TMP_DIR: &str = "tmp";
/// The registry of project slugs under `GEMINI_DIR`.
const REGISTRY: &str = "projects.json";
/// The ownership marker inside a slug directory.
const MARKER: &str = ".project_root";
/// The chats directory inside a slug directory.
const CHATS: &str = "chats";
/// The prefix of a main session's chat file.
const SESSION_FILE_PREFIX: &str = "session-";
/// The characters of the id a main session's file name keeps.
const SHORT_ID_LEN: usize = 8;
/// The departure label of this adapter's rewritable files.
const DEPARTURE_LABEL: &str = "gemini";

/// The Gemini CLI adapter for one home directory.
#[derive(Debug, Clone)]
pub struct GeminiCli {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
    known_writer: Option<HarnessVersion>,
}

/// The files of one session under its slug directory.
struct SessionFiles {
    /// The main chat file, when there is one.
    chat: Option<PathBuf>,
    /// Every file capture takes.
    sources: Vec<Source>,
}

impl GeminiCli {
    /// An adapter under `home`, finding `gemini` on the `PATH` value
    /// `path_var`.
    #[must_use]
    pub fn new(home: AbsPath, path_var: OsString) -> Self {
        let binary = process::find_on_path(PROGRAM, &path_var);
        GeminiCli {
            home,
            binary,
            path_var,
            known_writer: None,
        }
    }

    /// The home directory the adapter reads and writes under.
    #[must_use]
    pub fn home(&self) -> &AbsPath {
        &self.home
    }

    /// The newest Gemini CLI known to have written the session, which the
    /// job carries because a chat records no version: a resume refuses an
    /// older `gemini`, and a capture records at least this version, so a
    /// node that never ran the session cannot lower it.
    #[must_use]
    pub fn with_known_writer(mut self, version: Option<HarnessVersion>) -> Self {
        self.known_writer = version;
        self
    }

    fn home_dir(&self) -> &Path {
        pipeline::fs_path(&self.home)
    }

    fn gemini_dir(&self) -> PathBuf {
        self.home_dir().join(GEMINI_DIR)
    }

    fn slug_dir(&self, slug: &str) -> PathBuf {
        self.gemini_dir().join(TMP_DIR).join(slug)
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        self.binary
            .clone()
            .ok_or_else(|| HarnessError::NotInstalled {
                program: PROGRAM.to_owned(),
            })
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

    /// Refuses while any `gemini` runs: Gemini takes no lock on a chat,
    /// and one running in the workspace appends to it.
    async fn refuse_running(&self) -> Result<(), HarnessError> {
        let pids = process::running_pids(PROGRAM, self.binary.as_deref()).await?;
        if pids.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            })
        }
    }

    /// The slug this node's Gemini gives `root`: the registry's entry,
    /// checked against its marker, else the slug directory whose marker
    /// names `root`; `None` when Gemini never ran there. A registry entry
    /// whose marker names another project is `StateDiverged`, since Gemini
    /// would then drop the entry and pick another slug.
    fn slug_for(&self, root: &AbsPath) -> Result<Option<String>, HarnessError> {
        let registry = self.gemini_dir().join(REGISTRY);
        pipeline::refuse_symlinks(self.home_dir(), &registry)?;
        if let Some(slug) = registered_slug(&registry, root)? {
            let marker = self.slug_dir(&slug).join(MARKER);
            if let Some(owner) = read_marker(self.home_dir(), &marker)?
                && owner != root.as_str()
            {
                return Err(HarnessError::StateDiverged { path: marker });
            }
            return Ok(Some(slug));
        }
        let tmp = self.gemini_dir().join(TMP_DIR);
        pipeline::refuse_symlinks(self.home_dir(), &tmp)?;
        let entries = match std::fs::read_dir(&tmp) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &tmp, e)),
        };
        let mut owned = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &tmp, e))?;
            let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_slug(&slug) {
                continue;
            }
            let marker = entry.path().join(MARKER);
            if read_marker(self.home_dir(), &marker)?.as_deref() == Some(root.as_str()) {
                owned.push(slug);
            }
        }
        owned.sort();
        Ok(owned.into_iter().next())
    }

    /// The main chat file of `session` in `chats`, resolved by its short
    /// id and then the full id of its first record; two such files are
    /// `LineageAmbiguous`.
    fn chat_file(chats: &Path, session: &SessionId) -> Result<Option<PathBuf>, HarnessError> {
        let entries = match std::fs::read_dir(chats) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(HarnessError::io(IoOp::Walk, chats, e)),
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, chats, e))?;
            let path = entry.path();
            let named = entry
                .file_name()
                .to_str()
                .is_some_and(|name| names_chat(name, session));
            if !named {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io(IoOp::Walk, &path, e))?;
            if kind.is_symlink() {
                return Err(HarnessError::SymlinkInState { path });
            }
            if kind.is_file() && first_session_id(&path)?.as_deref() == Some(session.as_str()) {
                found.push(path);
            }
        }
        found.sort();
        if found.len() > 1 {
            return Err(HarnessError::LineageAmbiguous {
                session: session.clone(),
                rollout: session.clone(),
                paths: found,
            });
        }
        Ok(found.pop())
    }

    /// The files of `session` under `slug` on this node: the chat file the
    /// id resolves to, the subagent files under `chats/<id>/` and the side
    /// directory `<id>/`.
    fn session_files(&self, slug: &str, session: &SessionId) -> Result<SessionFiles, HarnessError> {
        let dir = self.slug_dir(slug);
        pipeline::refuse_symlinks(self.home_dir(), &dir)?;
        let chats = dir.join(CHATS);
        let chat = Self::chat_file(&chats, session)?;
        let rel_slug = [GEMINI_DIR, TMP_DIR, slug];
        let mut sources = Vec::new();
        if let Some(chat) = &chat {
            let name = chat
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            sources.push(source(
                &rel_slug,
                &[CHATS, name],
                chat.clone(),
                Mutability::AppendOnly,
            )?);
        }
        let id = session.as_str();
        for (below, mutability) in [
            (vec![CHATS, id], Mutability::AppendOnly),
            (vec![id], Mutability::Replaceable),
        ] {
            let under = below.iter().fold(dir.clone(), |at, part| at.join(part));
            match std::fs::symlink_metadata(&under) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(HarnessError::SymlinkInState { path: under });
                }
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => continue,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(HarnessError::io(IoOp::Read, &under, e)),
            }
            for file in pipeline::walk(&under)? {
                let file_parts = pipeline::rel_components(&file)?;
                let mut parts = below.clone();
                parts.extend(file_parts.iter().map(String::as_str));
                sources.push(source(&rel_slug, &parts, under.join(&file), mutability)?);
            }
        }
        Ok(SessionFiles { chat, sources })
    }

    fn departure(&self, session: &SessionId) -> Records {
        Records::of(self.home_dir(), DEPARTURE_LABEL, session)
    }

    fn capture_sync(
        &self,
        root: &AbsPath,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        let not_found = |searched: PathBuf| HarnessError::SessionNotFound {
            kind: HarnessKind::GeminiCli,
            session: session.clone(),
            searched,
        };
        let Some(slug) = self.slug_for(root)? else {
            return Err(not_found(self.gemini_dir().join(TMP_DIR)));
        };
        let files = self.session_files(&slug, session)?;
        if files.chat.is_none() {
            return Err(not_found(self.slug_dir(&slug).join(CHATS)));
        }
        let records = self.departure(session);
        let capturer = Capturer {
            kind: HarnessKind::GeminiCli,
            session,
            root,
            home: &self.home,
            version,
            departure: Some(&records),
        };
        let manifest = pipeline::capture(capturer, &files.sources, RedactionCounts::new(), out)?;
        info!(%session, %slug, files = manifest.files().len(), "gemini: captured");
        Ok(manifest)
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        self.refuse_running().await?;
        let installed = process::installed_version_in(
            self.binary.clone(),
            Some(pipeline::fs_path(&root)),
            &self.path_var,
            PROGRAM,
        )
        .await;
        let version = self.captured_version(installed);
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        pipeline::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
    }

    fn restore_sync(
        &self,
        root: &AbsPath,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let session = manifest.session().clone();
        let mut slugs: Vec<&str> = manifest
            .files()
            .iter()
            .filter_map(|file| slug_of(file.rel_path.as_str()))
            .collect();
        slugs.sort_unstable();
        slugs.dedup();
        let [slug] = slugs.as_slice() else {
            return Err(HarnessError::Malformed {
                what: Input::Manifest,
                path: state.to_path_buf(),
                reason: format!(
                    "a Gemini capture holds one slug directory, this one {}",
                    slugs.len()
                ),
            });
        };
        let slug = (*slug).to_owned();
        // Gemini on this node must read the same slug for the workspace,
        // or the placed chat is not the one it resumes.
        if let Some(here) = self.slug_for(root)?
            && here != slug
        {
            return Err(HarnessError::StateDiverged {
                path: self.gemini_dir().join(REGISTRY),
            });
        }
        let marker = self.slug_dir(&slug).join(MARKER);
        let has_marker = match read_marker(self.home_dir(), &marker)? {
            Some(owner) if owner == root.as_str() => true,
            Some(_) => return Err(HarnessError::StateDiverged { path: marker }),
            None => false,
        };
        let held = self.session_files(&slug, &session)?.sources;
        let records = self.departure(&session);
        let restored = pipeline::restore(manifest, state, Some(&records), &held, |rel, _| {
            Ok(
                restorable(rel, &slug, &session).map(|(rest, mutability)| Destination {
                    path: self.home_dir().join(rest),
                    content: Content::Opaque,
                    mutability,
                }),
            )
        })?;
        if !has_marker {
            durable::create_dirs_durable(&self.slug_dir(&slug))?;
            durable::write_atomic(&marker, root.as_str().as_bytes())?;
        }
        info!(%session, %slug, files = restored.placed.len(), "gemini: restored");
        Ok(restored)
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &HarnessKind::GeminiCli, &root, &self.home)?;
        self.refuse_running().await?;
        let adapter = self.clone();
        let state = state.to_path_buf();
        pipeline::blocking(move || adapter.restore_sync(&root, &state, manifest)).await
    }

    /// The model of the chat's last answer, read before a resume; `None`
    /// when the session has no chat on this node or no answer names one.
    fn recorded_model(
        &self,
        root: &AbsPath,
        session: &SessionId,
    ) -> Result<Option<String>, HarnessError> {
        let Some(slug) = self.slug_for(root)? else {
            return Ok(None);
        };
        let Some(chat) = Self::chat_file(&self.slug_dir(&slug).join(CHATS), session)? else {
            return Ok(None);
        };
        let file =
            std::fs::File::open(&chat).map_err(|e| HarnessError::io(IoOp::Read, &chat, e))?;
        let mut model = None;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| HarnessError::io(IoOp::Read, &chat, e))?;
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if record["type"] == "gemini"
                && let Some(name) = record["model"].as_str()
            {
                model = Some(name.to_owned());
            }
        }
        Ok(model)
    }

    /// The argument list for `launch` passing `model`, without the binary.
    pub(crate) fn args_for(launch: &Launch, model: Option<&ModelName>) -> Vec<String> {
        let mut args = Vec::new();
        if launch.is_headless() {
            args.extend(["--output-format".to_owned(), "stream-json".to_owned()]);
        }
        if let Some(session) = launch.resume() {
            args.extend(["--resume".to_owned(), session.as_str().to_owned()]);
        }
        if let Some(model) = model {
            args.extend(["--model".to_owned(), model.as_str().to_owned()]);
        }
        // Joined to its flag, so a prompt that starts with `-` is never
        // read as an option.
        match (launch, launch.prompt()) {
            (Launch::Headless { .. }, Some(prompt)) => args.push(format!("--prompt={prompt}")),
            (Launch::Interactive { .. }, Some(prompt)) => {
                args.push(format!("--prompt-interactive={prompt}"));
            }
            _ => {}
        }
        args
    }

    /// Reads one line of what `gemini --output-format stream-json` printed
    /// into `report` (`packages/core/src/output/types.ts`, read
    /// 2026-11-08): `init` names the session, each assistant `message`
    /// carries the answer, in pieces when `delta` is set, and a `result`
    /// whose `status` is `error` carries the failure. Any other line is
    /// left alone. Returns true when the line named a session other than
    /// the one `report` held.
    pub fn read_headless_line(report: &mut TurnReport, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            return false;
        };
        match event["type"].as_str() {
            Some("init") => return report.note_session(event["session_id"].as_str()),
            Some("message") if event["role"] == "assistant" => {
                if let Some(text) = event["content"].as_str() {
                    let answer = report.answer.get_or_insert_with(String::new);
                    if event["delta"] != Value::Bool(true) {
                        answer.clear();
                    }
                    let room = MAX_ANSWER.saturating_sub(answer.len());
                    answer.push_str(&text[..text.floor_char_boundary(room)]);
                }
            }
            Some("result") if event["status"] == "error" => {
                report.failure = Some(
                    event["error"]["message"]
                        .as_str()
                        .unwrap_or("the turn failed")
                        .to_owned(),
                );
            }
            _ => {}
        }
        false
    }

    /// The model the session ran with, as a launch passes it, and what the
    /// launch says about it.
    async fn model_for(
        &self,
        root: &AbsPath,
        session: &SessionId,
        settings: &mut LaunchSettings,
    ) -> Result<Option<ModelName>, HarnessError> {
        let adapter = self.clone();
        let (at, id) = (root.clone(), session.clone());
        let Some(recorded) = pipeline::blocking(move || adapter.recorded_model(&at, &id)).await?
        else {
            return Ok(None);
        };
        match ModelName::parse(&recorded) {
            Ok(name) => {
                settings.passed.push(PassedSetting {
                    name: SettingName::Model,
                    value: name.as_str().to_owned(),
                    origin: Origin::Session,
                });
                Ok(Some(name))
            }
            Err(e) => {
                settings.not_carried.push(NotCarried::new(
                    SettingName::Model,
                    &recorded,
                    format!("not a model name Fabric passes ({e})"),
                ));
                Ok(None)
            }
        }
    }

    async fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let mut settings = LaunchSettings::default();
        let mut runs = None;
        let mut model = None;
        if let Some(session) = launch.resume() {
            if let Some(writer) = &self.known_writer {
                runs = Some(
                    crate::version::refuse_older(
                        PROGRAM,
                        (&binary, pipeline::fs_path(&root), env),
                        None,
                        session,
                        writer,
                    )
                    .await?,
                );
            }
            model = self.model_for(&root, session, &mut settings).await?;
        }
        let args = Self::args_for(launch, model.as_ref());
        let session = launch.resume().cloned();
        let spawned =
            process::spawn(&binary, &args, pipeline::fs_path(&root), env, launch.into()).await?;
        info!(?session, pid = %spawned.pid, "gemini: launched, held until released");
        Ok(Running::new(session, spawned)
            .with_settings(settings)
            .with_version(runs))
    }
}

/// A source under `~/<rel_slug>/<parts>`.
fn source(
    rel_slug: &[&str],
    parts: &[&str],
    abs: PathBuf,
    mutability: Mutability,
) -> Result<Source, HarnessError> {
    let mut all = vec![HOME_PREFIX];
    all.extend_from_slice(rel_slug);
    all.extend_from_slice(parts);
    Ok(Source {
        abs,
        rel: pipeline::rel_path(&all)?,
        carry: Carry::OpaqueRefuseSecrets,
        mutability,
    })
}

/// True for a slug as Gemini's registry writes one: `^[a-z0-9-]+$`.
fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// True when `name` is the file name Gemini gives a main chat of
/// `session`: `session-<timestamp>[-<n>]-<first 8 of id>.jsonl`. Whether
/// it is that session's file is then read from its first record.
fn names_chat(name: &str, session: &SessionId) -> bool {
    let short: String = session.as_str().chars().take(SHORT_ID_LEN).collect();
    name.strip_prefix(SESSION_FILE_PREFIX)
        .and_then(|rest| rest.strip_suffix(".jsonl"))
        .and_then(|rest| rest.strip_suffix(short.as_str()))
        .is_some_and(|rest| rest.len() > 1 && rest.ends_with('-'))
}

/// The `sessionId` of the first record of a chat file, its metadata;
/// `None` when the first line is not such a record.
fn first_session_id(path: &Path) -> Result<Option<String>, HarnessError> {
    let file = std::fs::File::open(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    let mut first = String::new();
    BufReader::new(file)
        .read_line(&mut first)
        .map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    Ok(serde_json::from_str::<Value>(first.trim())
        .ok()
        .and_then(|record| record["sessionId"].as_str().map(str::to_owned)))
}

/// The slug `projects.json` at `registry` gives `root`; `None` when the
/// file or the entry is missing. A registry that does not parse, or a
/// slug outside `^[a-z0-9-]+$`, is refused rather than guessed.
fn registered_slug(registry: &Path, root: &AbsPath) -> Result<Option<String>, HarnessError> {
    let bytes = match std::fs::read(registry) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(HarnessError::io(IoOp::Read, registry, e)),
    };
    let malformed = |reason: String| HarnessError::Malformed {
        what: Input::ConfigJson,
        path: registry.to_path_buf(),
        reason,
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| malformed(e.to_string()))?;
    let Some(slug) = value["projects"].get(root.as_str()) else {
        return Ok(None);
    };
    match slug.as_str() {
        Some(slug) if is_slug(slug) => Ok(Some(slug.to_owned())),
        _ => Err(malformed(format!(
            "the slug of {} is not a slug",
            root.as_str()
        ))),
    }
}

/// The project a slug's `.project_root` marker names, trimmed as Gemini
/// trims it; `None` when there is no marker.
fn read_marker(home: &Path, marker: &Path) -> Result<Option<String>, HarnessError> {
    pipeline::refuse_symlinks(home, marker)?;
    match std::fs::read_to_string(marker) {
        Ok(text) => Ok(Some(text.trim().to_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(HarnessError::io(IoOp::Read, marker, e)),
    }
}

/// The slug a tree path lies under, `home/.gemini/tmp/<slug>/...`.
fn slug_of(rel: &str) -> Option<&str> {
    let mut parts = under_home(rel)?.split('/');
    if parts.next() != Some(GEMINI_DIR) || parts.next() != Some(TMP_DIR) {
        return None;
    }
    parts.next().filter(|slug| is_slug(slug))
}

/// Where a tree path lands under the home and how Gemini treats it, when
/// it is one capture takes for `session` under `slug`: the chat file, a
/// subagent file under `chats/<id>/`, or a file under `<id>/`.
fn restorable<'a>(rel: &'a str, slug: &str, session: &SessionId) -> Option<(&'a str, Mutability)> {
    let rest = under_home(rel)?;
    let parts: Vec<&str> = rest.split('/').collect();
    let [gemini, tmp, at, below @ ..] = parts.as_slice() else {
        return None;
    };
    if *gemini != GEMINI_DIR || *tmp != TMP_DIR || *at != slug {
        return None;
    }
    let id = session.as_str();
    let mutability = match below {
        [chats, name] if *chats == CHATS && names_chat(name, session) => Mutability::AppendOnly,
        [chats, dir, _, ..] if *chats == CHATS && *dir == id => Mutability::AppendOnly,
        [dir, _, ..] if *dir == id => Mutability::Replaceable,
        _ => return None,
    };
    Some((rest, mutability))
}

impl Harness for GeminiCli {
    fn kind(&self) -> HarnessKind {
        HarnessKind::GeminiCli
    }

    /// The chat file and its side directories under the restored slug,
    /// continued by `--resume` under the same id.
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

    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        let records = self.departure(manifest.session());
        let manifest = manifest.clone();
        Box::pin(pipeline::blocking(move || {
            pipeline::confirm_departure(&manifest, &HarnessKind::GeminiCli, Some(&records))
        }))
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

    const ID: &str = "6f1d2c3b-4a5e-4b6c-8d7e-9f0a1b2c3d4e";

    fn id() -> SessionId {
        SessionId::parse(ID).unwrap()
    }

    #[test]
    fn a_chat_file_is_named_by_the_short_id_in_either_form() {
        let session = id();
        for good in [
            "session-2026-11-08T10-00-6f1d2c3b.jsonl",
            "session-2026-11-08T10-00-2-6f1d2c3b.jsonl",
        ] {
            assert!(names_chat(good, &session), "{good}");
        }
        for bad in [
            "session-6f1d2c3b.jsonl",
            "session--6f1d2c3b.jsonl",
            "session-2026-11-08T10-00-6f1d2c3c.jsonl",
            "session-2026-11-08T10-00-6f1d2c3b.json",
            "session-2026-11-08T10-00-x6f1d2c3b.jsonl",
            "6f1d2c3b-4a5e-4b6c-8d7e-9f0a1b2c3d4e.jsonl",
        ] {
            assert!(!names_chat(bad, &session), "{bad}");
        }
    }

    #[test]
    fn restore_accepts_only_the_sessions_own_paths_under_one_slug() {
        let session = id();
        let sub = format!("home/.gemini/tmp/work/chats/{ID}/sub.jsonl");
        let plan = format!("home/.gemini/tmp/work/{ID}/plans/plan.md");
        let ok = [
            (
                "home/.gemini/tmp/work/chats/session-2026-11-08T10-00-6f1d2c3b.jsonl",
                Mutability::AppendOnly,
            ),
            (sub.as_str(), Mutability::AppendOnly),
            (plan.as_str(), Mutability::Replaceable),
        ];
        for (rel, mutability) in ok {
            assert_eq!(
                restorable(rel, "work", &session).map(|(_, m)| m),
                Some(mutability),
                "{rel}"
            );
        }
        let bare = format!("home/.gemini/tmp/work/{ID}");
        for rel in [
            "home/.gemini/projects.json",
            "home/.gemini/oauth_creds.json",
            "home/.gemini/tmp/work/.project_root",
            "home/.gemini/tmp/other/chats/session-2026-11-08T10-00-6f1d2c3b.jsonl",
            "home/.gemini/tmp/work/chats/session-2026-11-08T10-00-0000aaaa.jsonl",
            "home/.gemini/tmp/work/logs/x.json",
            bare.as_str(),
        ] {
            assert!(restorable(rel, "work", &session).is_none(), "{rel}");
        }
        assert_eq!(slug_of("home/.gemini/tmp/work/x"), Some("work"));
        assert_eq!(slug_of("home/.gemini/tmp/Work/x"), None);
    }

    #[test]
    fn a_resume_passes_the_session_the_model_and_a_prompt_joined_to_its_flag() {
        let model = ModelName::parse("gemini-3-pro").unwrap();
        let headless = Launch::Headless {
            resume: Some(id()),
            prompt: "-v what now".to_owned(),
        };
        assert_eq!(
            GeminiCli::args_for(&headless, Some(&model)),
            [
                "--output-format",
                "stream-json",
                "--resume",
                ID,
                "--model",
                "gemini-3-pro",
                "--prompt=-v what now"
            ]
        );
        let interactive = Launch::Interactive {
            resume: None,
            prompt: Some("hi".to_owned()),
        };
        assert_eq!(
            GeminiCli::args_for(&interactive, None),
            ["--prompt-interactive=hi"]
        );
    }

    #[test]
    fn a_stream_names_the_session_gathers_deltas_and_says_a_failure() {
        let mut report = TurnReport::default();
        let init = format!(r#"{{"type":"init","timestamp":"t","session_id":"{ID}","model":"m"}}"#);
        assert!(GeminiCli::read_headless_line(&mut report, &init));
        assert!(!GeminiCli::read_headless_line(&mut report, &init));
        for piece in ["hel", "lo"] {
            let line = format!(
                r#"{{"type":"message","timestamp":"t","role":"assistant","content":"{piece}","delta":true}}"#
            );
            assert!(!GeminiCli::read_headless_line(&mut report, &line));
        }
        let user = r#"{"type":"message","timestamp":"t","role":"user","content":"ignored"}"#;
        GeminiCli::read_headless_line(&mut report, user);
        assert_eq!(report.answer.as_deref(), Some("hello"));
        assert_eq!(report.session.as_ref().map(SessionId::as_str), Some(ID));
        let failed = r#"{"type":"result","timestamp":"t","status":"error","error":{"type":"x","message":"quota"}}"#;
        GeminiCli::read_headless_line(&mut report, failed);
        assert_eq!(report.failure.as_deref(), Some("quota"));
        assert!(!GeminiCli::read_headless_line(&mut report, "not json"));
    }

    #[test]
    fn a_capture_records_the_known_writer_when_it_is_newer() {
        let adapter = GeminiCli::new(AbsPath::parse("/h").unwrap(), OsString::new())
            .with_known_writer(Some(HarnessVersion::parse("0.68.0").unwrap()));
        assert_eq!(
            adapter
                .captured_version(Some("0.67.0".to_owned()))
                .as_deref(),
            Some("0.68.0")
        );
        assert_eq!(
            adapter
                .captured_version(Some("0.69.1".to_owned()))
                .as_deref(),
            Some("0.69.1")
        );
        assert_eq!(adapter.captured_version(None).as_deref(), Some("0.68.0"));
        assert_eq!(adapter.kind(), HarnessKind::GeminiCli);
        assert_eq!(adapter.fidelity(), Fidelity::NativeSession);
    }
}
