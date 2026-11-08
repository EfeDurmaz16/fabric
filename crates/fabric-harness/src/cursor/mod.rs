//! The Cursor Agent CLI adapter (PR 5, owner B; contract in
//! `docs/harness-adapters.md`, section Cursor Agent CLI).
//!
//! Invariant to build to: a headless Cursor chat lives in
//! `~/.cursor/chats/<md5 of the workspace path>/<chat id>/store.db`, a
//! `SQLite` store of encrypted, content-addressed blobs whose key sits beside
//! them in the same file (`meta`, hex-encoded JSON with
//! `blobEncryptionKey`; read on 2026.10.01, 2026-11-08), and an ACP session
//! leaves only `~/.cursor/acp-sessions/<id>/meta.json` with `cwd` and
//! `schemaVersion`, no transcript. Fabric's invariant is that a carried
//! file holds no secret-shaped string, and an encrypted blob cannot be
//! scanned, so the store is not carried until Cursor documents its format
//! (decision of 2026-11-08): the adapter's fidelity is `WorkspaceOnly`, a
//! capture takes nothing of the chat and says why, and a resume starts a
//! new chat in the restored workspace with `cursor-agent -p` or
//! `cursor-agent acp`, carrying the model the job pinned. `--api-key` in the
//! launch arguments is refused as a secret. `cli-config.json` never travels.
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
pub const PROGRAM: &str = "cursor-agent";

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

    fn not_built(step: &'static str) -> HarnessError {
        HarnessError::NotBuilt {
            program: PROGRAM.to_owned(),
            step,
        }
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
        let adapter = CursorAgent::new(AbsPath::parse("/h").unwrap(), OsString::new());
        assert_eq!(adapter.kind(), HarnessKind::CursorAgent);
        assert_eq!(adapter.fidelity(), Fidelity::WorkspaceOnly);
        assert_eq!(adapter.home().as_str(), "/h");
        let refused = CursorAgent::not_built("launch").to_string();
        assert!(refused.contains(PROGRAM), "{refused}");
    }
}
