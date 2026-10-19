//! Finding, probing and starting harness binaries.
//!
//! Invariant: a harness starts in its own process group with exactly the
//! environment the caller passed, never through a shell, so the supervisor
//! can fence the whole group (ADR 0003) and no variable of the calling
//! process leaks into it.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fabric_core::env::EnvVarName;
use tokio::process::{Child, Command};

use crate::error::HarnessError;
use crate::harness::Io;

/// How long a version probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Looks `name` up on the given `PATH` value, as a shell would.
#[must_use]
pub fn find_on_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs `binary args...` and returns trimmed stdout, within `PROBE_TIMEOUT`.
pub async fn probe(binary: &Path, args: &[&str]) -> Result<String, HarnessError> {
    let program = binary.display().to_string();
    let run = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(PROBE_TIMEOUT, run)
        .await
        .map_err(|_| HarnessError::Spawn {
            program: program.clone(),
            source: std::io::Error::new(std::io::ErrorKind::TimedOut, "version probe timed out"),
        })?
        .map_err(|source| HarnessError::Spawn {
            program: program.clone(),
            source,
        })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Returns the first token of `output` that looks like `digits.digits...`.
#[must_use]
pub fn version_token(output: &str) -> Option<&str> {
    output.split_whitespace().find(|token| {
        let mut parts = token.split('.');
        let first = parts.next().unwrap_or("");
        !first.is_empty()
            && first.bytes().all(|b| b.is_ascii_digit())
            && parts
                .next()
                .is_some_and(|second| second.bytes().next().is_some_and(|b| b.is_ascii_digit()))
    })
}

/// Starts `binary args...` in `cwd`, in a new process group, with exactly
/// `env`, and the stdio `io` asks for.
pub fn spawn(
    binary: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(EnvVarName, String)],
    io: Io,
) -> Result<Child, HarnessError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .process_group(0)
        .kill_on_drop(false);
    match io {
        Io::Terminal => {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }
        Io::Piped => {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }
        Io::Rpc => {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
        }
    }
    command.spawn().map_err(|source| HarnessError::Spawn {
        program: binary.display().to_string(),
        source,
    })
}

/// Unix milliseconds now; informational.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Process ids of running processes whose name is exactly `name`, via `pgrep -x`.
pub async fn running_pids(name: &str) -> Result<Vec<u32>, HarnessError> {
    let output = Command::new("pgrep")
        .arg("-x")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|source| HarnessError::Spawn {
            program: "pgrep".to_owned(),
            source,
        })?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect())
}

/// The variables a harness needs to find its login and its terminal, taken
/// from this process: `HOME`, `PATH`, `TERM`, `SHELL`, `LANG`, `TMPDIR`,
/// `USER`. The caller adds the names the job's `EnvSpec` lists.
#[must_use]
pub fn baseline_env() -> Vec<(EnvVarName, String)> {
    ["HOME", "PATH", "TERM", "SHELL", "LANG", "TMPDIR", "USER"]
        .iter()
        .filter_map(|name| {
            let value = std::env::var(name).ok()?;
            let name = EnvVarName::parse(name).ok()?;
            Some((name, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_token_finds_the_number() {
        assert_eq!(version_token("2.1.294 (Claude Code)"), Some("2.1.294"));
        assert_eq!(version_token("codex-cli 0.155.1"), Some("0.155.1"));
        assert_eq!(version_token("fake 1.2"), Some("1.2"));
        assert_eq!(version_token("no version here"), None);
        assert_eq!(version_token(""), None);
    }

    #[test]
    fn find_on_path_needs_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("tool");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        let var = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_on_path("tool", Some(&var)), None);
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_on_path("tool", Some(&var)), Some(plain));
        assert_eq!(find_on_path("tool", None), None);
    }
}
