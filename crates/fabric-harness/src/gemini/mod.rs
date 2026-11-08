//! The Gemini CLI adapter (PR 5, owner B; contract in
//! `docs/harness-adapters.md`, section Gemini CLI).
//!
//! Invariant to build to: Gemini CLI records a chat as
//! `~/.gemini/tmp/<slug>/chats/session-<timestamp>-<first 8 of id>.jsonl`,
//! with a `-<n>` collision form and subagent files `<full id>.jsonl` under
//! `chats/<parent id>/`, and keeps the session's plans, tracker and tasks
//! under `<slug>/<id>/`; the slug is assigned by `~/.gemini/projects.json`
//! and owned by a `.project_root` marker inside the slug directory
//! (`chatRecordingService.ts`, `projectRegistry.ts` and `storage.ts` on
//! google-gemini/gemini-cli `main`, read 2026-11-08; v0.67.0). The generic
//! ACP adapter would take the `<id>/` directories and miss the chat file,
//! a partial capture reported as a session, so this adapter resolves the id
//! to its file, carries the chat and the side directories byte for byte,
//! refused on a secret shape, and restores the registry entry and the
//! marker with them. A resume continues over ACP with `session/load`
//! (`gemini --acp`), or headless with `gemini --resume <id>`; the model
//! comes from the chat's own records. `oauth_creds.json`, `settings.json`
//! and `.env` never travel. Gemini CLI is not installed on the owner's
//! Mac: the fake is built from the documented format alone.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;

use crate::error::HarnessError;
use crate::harness::{Fidelity, Harness, Installed, Launch, Restored, Running};
use crate::manifest::StateManifest;
use crate::process;

/// The program name on `PATH`.
pub const PROGRAM: &str = "gemini";

/// The Gemini CLI adapter for one home directory.
#[derive(Debug, Clone)]
pub struct GeminiCli {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
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
        }
    }

    /// The home directory the adapter reads and writes under.
    #[must_use]
    pub fn home(&self) -> &AbsPath {
        &self.home
    }

    fn not_built(step: &'static str) -> HarnessError {
        HarnessError::NotBuilt {
            program: PROGRAM.to_owned(),
            step,
        }
    }
}

impl Harness for GeminiCli {
    fn kind(&self) -> HarnessKind {
        HarnessKind::GeminiCli
    }

    /// The chat file and its side directories under the restored slug,
    /// continued by `session/load` or `--resume`.
    fn fidelity(&self) -> Fidelity {
        Fidelity::NativeSession
    }

    fn detect(&self) -> BoxFuture<'_, Result<Option<Installed>, HarnessError>> {
        Box::pin(process::detect(self.binary.clone(), &self.path_var))
    }

    fn capture<'a>(
        &'a self,
        _workspace: &'a Path,
        _session: &'a SessionId,
        _out: &'a Path,
    ) -> BoxFuture<'a, Result<StateManifest, HarnessError>> {
        Box::pin(async { Err(Self::not_built("capture")) })
    }

    fn confirm_departure<'a>(
        &'a self,
        _manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        Box::pin(async { Err(Self::not_built("confirm a departure")) })
    }

    fn restore<'a>(
        &'a self,
        _workspace: &'a Path,
        _state: &'a Path,
    ) -> BoxFuture<'a, Result<Restored, HarnessError>> {
        Box::pin(async { Err(Self::not_built("restore")) })
    }

    fn launch<'a>(
        &'a self,
        _workspace: &'a Path,
        _launch: Launch,
        _env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(async { Err(Self::not_built("launch")) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scaffold_names_its_kind_and_refuses_the_steps_not_built() {
        let adapter = GeminiCli::new(AbsPath::parse("/h").unwrap(), OsString::new());
        assert_eq!(adapter.kind(), HarnessKind::GeminiCli);
        assert_eq!(adapter.fidelity(), Fidelity::NativeSession);
        assert_eq!(adapter.home().as_str(), "/h");
        let refused = GeminiCli::not_built("launch").to_string();
        assert!(refused.contains(PROGRAM), "{refused}");
    }
}
