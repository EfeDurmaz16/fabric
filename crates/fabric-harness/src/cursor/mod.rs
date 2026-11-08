//! The Cursor Agent CLI adapter (PR 5, owner B; contract in
//! `docs/harness-adapters.md`, section Cursor Agent CLI).
//!
//! Invariant: a headless or interactive Cursor chat lives in
//! `~/.cursor/chats/<md5 of the workspace path>/<chat id>/`, whose
//! `store.db` is a `SQLite` store of encrypted, content-addressed blobs with
//! the key beside them in the same file (`meta`, hex-encoded JSON with
//! `blobEncryptionKey`; read on 2026.10.01-e373342, 2026-11-08), and an ACP
//! session leaves only `~/.cursor/acp-sessions/<id>/meta.json`, with no
//! transcript at all. Fabric's invariant is that a carried file holds no
//! secret-shaped string, an encrypted blob cannot be scanned for one, and
//! decrypting it would mean parsing a private format with its own key. So
//! nothing of the chat travels until Cursor documents its format (decision
//! of 2026-11-08, fail closed): the adapter's fidelity is `WorkspaceOnly`,
//! a capture takes no file and records why (`cursor-store-not-carried`),
//! and a restore places nothing and reports `WorkspaceOnly`. A resume on
//! the node that holds the chat continues it with `--resume <id>`; on any
//! other node it starts a new chat in the restored workspace and says the
//! conversation stayed behind. `cli-config.json`, the login and the chat
//! directories never travel. Since nothing of the session travels, no
//! version guard applies: the manifest records the installed version, and
//! a chat continued with `--resume` is only ever the one this node's own
//! `cursor-agent` wrote. A capture refuses while a process holds the
//! chat's store open, since the session then still runs on this node, and
//! while a `cursor-agent` works in the workspace, which covers a chat the
//! job does not name, as one started here after a move.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use serde_json::Value;
use tracing::info;

use crate::error::HarnessError;
use crate::harness::{
    Fidelity, Harness, Installed, Launch, LaunchSettings, NotCarried, Restored, Running,
    SettingName, TurnReport,
};
use crate::manifest::StateManifest;
use crate::pipeline::{self, Capturer};
use crate::process;
use crate::redact::RedactionCounts;

/// The program name on `PATH`.
pub const PROGRAM: &str = "cursor-agent";

/// The Cursor Agent CLI version whose layout this adapter was read on.
pub const READ_ON: &str = "2026.10.01-e373342";

/// The reason a capture records for the chat it leaves behind.
pub const NOT_CARRIED: &str = "cursor-store-not-carried";

/// Why a resume on another node starts a new chat, in the words a launch
/// reports.
const NEW_CHAT: &str = "the Cursor chat store is encrypted and stays on the node that wrote it; workspace_only: a new chat starts in the restored workspace";

/// The Cursor Agent CLI adapter for one home directory.
#[derive(Debug, Clone)]
pub struct CursorAgent {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
}

impl CursorAgent {
    /// An adapter under `home`, finding `cursor-agent` on the `PATH` value
    /// `path_var`.
    #[must_use]
    pub fn new(home: AbsPath, path_var: OsString) -> Self {
        let binary = process::find_on_path(PROGRAM, &path_var);
        CursorAgent {
            home,
            binary,
            path_var,
        }
    }

    /// The home directory the adapter reads and writes under.
    #[must_use]
    pub fn home(&self) -> &AbsPath {
        &self.home
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        self.binary
            .clone()
            .ok_or_else(|| HarnessError::NotInstalled {
                program: PROGRAM.to_owned(),
            })
    }

    /// The chat directories of `session` on this node,
    /// `~/.cursor/chats/<hash>/<id>/`. The hash is not recomputed, since a
    /// chat id names one chat.
    fn chat_dirs(&self, session: &SessionId) -> Vec<PathBuf> {
        let chats = pipeline::fs_path(&self.home).join(".cursor").join("chats");
        let Ok(entries) = std::fs::read_dir(chats) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path().join(session.as_str()))
            .filter(|dir| dir.is_dir())
            .collect()
    }

    /// Refuses while a process holds the chat's store open: a
    /// `cursor-agent` still runs the session, so the job would leave while
    /// its conversation goes on here. Judged by the open store, not by the
    /// program's name, because Cursor's editor runs `cursor-agent` workers
    /// of its own that never touch this chat.
    fn refuse_open_chat(&self, session: &SessionId) -> Result<(), HarnessError> {
        let files: Vec<PathBuf> = self
            .chat_dirs(session)
            .iter()
            .flat_map(|dir| ["store.db", "store.db-wal"].map(|name| dir.join(name)))
            .filter(|file| file.is_file())
            .collect();
        if files.is_empty() {
            return Ok(());
        }
        let pids = process::holding(&files)?;
        if pids.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            })
        }
    }

    /// Refuses while a `cursor-agent` runs with its working directory in
    /// the workspace `root`, whichever chat it runs: one the job's chat
    /// store does not show, as a chat started here after a move, still
    /// edits the workspace the capture takes. `pids` are the running
    /// processes of the program; one working elsewhere, as the editor's
    /// own workers do, is left alone.
    fn refuse_in_workspace(root: &AbsPath, pids: &[u32]) -> Result<(), HarnessError> {
        let root = pipeline::fs_path(root);
        let inside: Vec<u32> = process::working_dirs(pids)?
            .into_iter()
            .filter(|(_, dir)| {
                let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
                dir.starts_with(root)
            })
            .map(|(pid, _)| pid)
            .collect();
        if inside.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids: inside,
            })
        }
    }

    /// True when this node holds the chat `session` as its own: a chat
    /// directory with a `store.db`.
    fn holds_chat(&self, session: &SessionId) -> bool {
        self.chat_dirs(session)
            .iter()
            .any(|dir| dir.join("store.db").is_file())
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let version = process::installed_version_in(
            self.binary.clone(),
            Some(pipeline::fs_path(&root)),
            &self.path_var,
            PROGRAM,
        )
        .await;
        let running = process::running_pids(PROGRAM, self.binary.as_deref()).await?;
        let home = self.home.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        let adapter = self.clone();
        pipeline::blocking(move || {
            adapter.refuse_open_chat(&session)?;
            Self::refuse_in_workspace(&root, &running)?;
            let mut counts = RedactionCounts::new();
            counts.insert(NOT_CARRIED, 1);
            let capturer = Capturer {
                kind: HarnessKind::CursorAgent,
                session: &session,
                root: &root,
                home: &home,
                version,
                departure: None,
            };
            let manifest = pipeline::capture(capturer, &[], counts, &out)?;
            info!(%session, "cursor: captured the session id; the encrypted chat store stays here");
            Ok(manifest)
        })
        .await
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &HarnessKind::CursorAgent, &root, &self.home)?;
        let state = state.to_path_buf();
        let mut restored = pipeline::blocking(move || {
            // The adapter captures no file, so every one listed is refused.
            pipeline::restore(manifest, &state, None, &[], |_, _| Ok(None))
        })
        .await?;
        restored.fidelity = Fidelity::WorkspaceOnly;
        info!(session = %restored.session, fidelity = %restored.fidelity, "cursor: restored");
        Ok(restored)
    }

    /// The argument list for `launch`, without the binary, continuing
    /// `resume` when this node holds that chat.
    pub(crate) fn args_for(launch: &Launch, resume: Option<&SessionId>) -> Vec<String> {
        let mut args = Vec::new();
        if launch.is_headless() {
            args.extend(["--print", "--output-format", "stream-json"].map(str::to_owned));
        }
        // The workspace is the job's own, which the user asked to run in.
        args.push("--trust".to_owned());
        if let Some(session) = resume {
            args.extend(["--resume".to_owned(), session.as_str().to_owned()]);
        }
        if let Some(prompt) = launch.prompt() {
            args.extend(["--".to_owned(), prompt.to_owned()]);
        }
        args
    }

    /// Reads one line of what `cursor-agent --print --output-format
    /// stream-json` printed into `report` (Cursor's output format
    /// reference, read 2026-11-08): every event names its `session_id`,
    /// an `assistant` event carries a whole message between tool calls,
    /// and the `result` event carries the turn's full text, or its failure
    /// when `is_error` is set or its `subtype` is not `success`. Returns
    /// true when the line named a session other than the one `report` held.
    pub fn read_headless_line(report: &mut TurnReport, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            return false;
        };
        let named = report.note_session(event["session_id"].as_str());
        match event["type"].as_str() {
            Some("assistant") => {
                let text: String = event["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect();
                if !text.is_empty() {
                    report.answer = Some(text);
                }
            }
            Some("result") => {
                let failed =
                    event["is_error"] == Value::Bool(true) || event["subtype"] != "success";
                let text = event["result"].as_str().map(str::to_owned);
                if failed {
                    report.failure = Some(text.unwrap_or_else(|| "the turn failed".to_owned()));
                } else if text.is_some() {
                    report.answer = text;
                }
            }
            _ => {}
        }
        named
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
        let resume = match launch.resume() {
            Some(session) if self.holds_chat(session) => Some(session.clone()),
            Some(session) => {
                settings.not_carried.push(NotCarried::new(
                    SettingName::Chat,
                    session.as_str(),
                    NEW_CHAT,
                ));
                None
            }
            None => None,
        };
        let args = Self::args_for(launch, resume.as_ref());
        let spawned =
            process::spawn(&binary, &args, pipeline::fs_path(&root), env, launch.into()).await?;
        info!(?resume, pid = %spawned.pid, "cursor: launched, held until released");
        Ok(Running::new(resume, spawned).with_settings(settings))
    }
}

impl Harness for CursorAgent {
    fn kind(&self) -> HarnessKind {
        HarnessKind::CursorAgent
    }

    /// The encrypted chat store is not carried (decision of 2026-11-08); the
    /// workspace is, and a new chat starts in it.
    fn fidelity(&self) -> Fidelity {
        Fidelity::WorkspaceOnly
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
        let manifest = manifest.clone();
        Box::pin(pipeline::blocking(move || {
            pipeline::confirm_departure(&manifest, &HarnessKind::CursorAgent, None)
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

    const ID: &str = "3c9e1f2a-7b4d-4e8f-9a1b-2c3d4e5f6a7b";

    #[test]
    fn the_adapter_declares_workspace_only() {
        let adapter = CursorAgent::new(AbsPath::parse("/h").unwrap(), OsString::new());
        assert_eq!(adapter.kind(), HarnessKind::CursorAgent);
        assert_eq!(adapter.fidelity(), Fidelity::WorkspaceOnly);
        assert_eq!(adapter.home().as_str(), "/h");
    }

    #[test]
    fn a_headless_launch_prints_a_stream_and_ends_its_options_before_the_prompt() {
        let session = SessionId::parse(ID).unwrap();
        let headless = Launch::Headless {
            resume: Some(session.clone()),
            prompt: "--api-key x".to_owned(),
        };
        assert_eq!(
            CursorAgent::args_for(&headless, Some(&session)),
            [
                "--print",
                "--output-format",
                "stream-json",
                "--trust",
                "--resume",
                ID,
                "--",
                "--api-key x"
            ]
        );
        assert_eq!(
            CursorAgent::args_for(&headless, None)[3..],
            ["--trust", "--", "--api-key x"]
        );
    }

    #[test]
    fn a_stream_names_the_chat_and_takes_the_results_text() {
        let mut report = TurnReport::default();
        let init = format!(r#"{{"type":"system","subtype":"init","session_id":"{ID}"}}"#);
        assert!(CursorAgent::read_headless_line(&mut report, &init));
        let said = format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"part"}}]}},"session_id":"{ID}"}}"#
        );
        assert!(!CursorAgent::read_headless_line(&mut report, &said));
        assert_eq!(report.answer.as_deref(), Some("part"));
        let done = format!(
            r#"{{"type":"result","subtype":"success","is_error":false,"result":"the whole answer","session_id":"{ID}"}}"#
        );
        assert!(!CursorAgent::read_headless_line(&mut report, &done));
        assert_eq!(report.answer.as_deref(), Some("the whole answer"));
        assert_eq!(report.failure, None);
        let failed = format!(
            r#"{{"type":"result","subtype":"error","is_error":true,"result":"usage limit","session_id":"{ID}"}}"#
        );
        CursorAgent::read_headless_line(&mut report, &failed);
        assert_eq!(report.failure.as_deref(), Some("usage limit"));
        assert!(!CursorAgent::read_headless_line(&mut report, "not json"));
    }
}
