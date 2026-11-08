//! The OpenCode adapter (PR 5, owner A; contract in
//! `docs/harness-adapters.md`, section OpenCode).
//!
//! Invariant to build to: OpenCode keeps every session, message and part of
//! every project in one `SQLite` file, `~/.local/share/opencode/opencode.db`,
//! beside the tables that hold its logins (`account.access_token`,
//! `account.refresh_token`, `control_account.access_token`,
//! `credential.value`, `session_share.secret`; schema read on 1.18.34,
//! 2026-11-08). The file never travels, in whole or in part. The adapter
//! carries one session as the JSON `opencode export <id>` writes, refused
//! when a secret shape is found in it, and hands it back with
//! `opencode import <file>`, which prints `Imported session: <id>`. When
//! that id is the captured one the restore reached `NativeSession`; when
//! OpenCode minted another, the restore reports it in `Restored::session`
//! and `NewNativeSessionWithPackage`, and never claims the higher level
//! (decision of 2026-11-08). A resume runs `opencode run -s <id>` with the
//! `model`, `agent` and `variant` the session row records, each one the
//! adapter was configured with winning. `auth.json` and the database never
//! travel; a running `opencode` that holds the database refuses capture
//! and restore until the writer rule is read from its source.
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
pub const PROGRAM: &str = "opencode";

/// The OpenCode adapter for one home directory.
#[derive(Debug, Clone)]
pub struct OpenCode {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
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
        let adapter = OpenCode::new(AbsPath::parse("/h").unwrap(), OsString::new());
        assert_eq!(adapter.kind(), HarnessKind::OpenCode);
        assert_eq!(adapter.fidelity(), Fidelity::NativeSession);
        assert_eq!(adapter.home().as_str(), "/h");
        let refused = OpenCode::not_built("launch").to_string();
        assert!(refused.contains(PROGRAM), "{refused}");
    }
}
