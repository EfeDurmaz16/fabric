//! The pi adapter (PR 5, owner A; contract in `docs/harness-adapters.md`,
//! section pi).
//!
//! Invariant to build to: pi keeps one session in one JSONL file,
//! `~/.pi/agent/sessions/--<workspace path with / as ->--/<timestamp>_<uuid>.jsonl`,
//! a v3 tree whose header `{"type":"session","id","cwd","timestamp","version"}`
//! names the workspace (format of 0.80.3, `docs/session-format.md` of the
//! installed package, read 2026-11-08). The file is carried byte for byte,
//! as Codex's rollout is, and refused when a line holds a secret shape; an
//! Anthropic `thinkingSignature` is a signed block and is never edited. A
//! resume runs `pi --session <path>` with the provider and model of the last
//! `model_change` entry and the level of the last `thinking_level_change`,
//! each one the adapter was configured with winning. `auth.json`,
//! `settings.json` and `trust.json` never travel. The reached fidelity is
//! `NativeSession`: the same file under the same path key.
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
pub const PROGRAM: &str = "pi";

/// The pi adapter for one home directory.
#[derive(Debug, Clone)]
pub struct Pi {
    home: AbsPath,
    binary: Option<PathBuf>,
    path_var: OsString,
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
        let adapter = Pi::new(AbsPath::parse("/h").unwrap(), OsString::new());
        assert_eq!(adapter.kind(), HarnessKind::Pi);
        assert_eq!(adapter.fidelity(), Fidelity::NativeSession);
        assert_eq!(adapter.home().as_str(), "/h");
        let refused = Pi::not_built("launch").to_string();
        assert!(refused.contains(PROGRAM), "{refused}");
    }
}
