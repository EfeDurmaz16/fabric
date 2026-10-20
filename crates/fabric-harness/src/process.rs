//! Finding, probing and starting harness binaries.
//!
//! Invariant: a harness starts in its own process group with exactly the
//! environment the caller passed, never through a shell, so the supervisor
//! can fence the whole group (ADR 0003) and no variable of the calling
//! process leaks into it. A question about running processes fails closed:
//! when `pgrep` or `ps` cannot answer, the caller is told so, never "none".

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::ffi::OsStr;
use std::num::NonZeroU32;
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
pub(crate) fn find_on_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs `binary args...` and returns trimmed stdout, within `PROBE_TIMEOUT`.
pub(crate) async fn probe(binary: &Path, args: &[&str]) -> Result<String, HarnessError> {
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
pub(crate) fn version_token(output: &str) -> Option<&str> {
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

/// How a spawned process's stdio is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wiring {
    /// The user's terminal: stdin, stdout and stderr are inherited.
    Terminal,
    /// Headless: no stdin, stdout and stderr captured.
    Piped,
    /// A protocol client owns stdin and stdout; stderr is inherited.
    Rpc,
}

impl From<Io> for Wiring {
    fn from(io: Io) -> Self {
        match io {
            Io::Terminal => Wiring::Terminal,
            Io::Piped => Wiring::Piped,
        }
    }
}

/// Starts `binary args...` in `cwd`, in a new process group, with exactly
/// `env`, and the stdio `wiring` asks for. Returns the child and its pid.
pub(crate) fn spawn(
    binary: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(EnvVarName, String)],
    wiring: Wiring,
) -> Result<(Child, NonZeroU32), HarnessError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .process_group(0)
        .kill_on_drop(false);
    match wiring {
        Wiring::Terminal => {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }
        Wiring::Piped => {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }
        Wiring::Rpc => {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
        }
    }
    let spawn_error = |source| HarnessError::Spawn {
        program: binary.display().to_string(),
        source,
    };
    let child = command.spawn().map_err(spawn_error)?;
    let pid = child
        .id()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| spawn_error(std::io::Error::other("the process had no pid after spawn")))?;
    Ok((child, pid))
}

/// Unix milliseconds now; informational.
#[must_use]
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

async fn pgrep(args: &[&OsStr]) -> Result<Vec<u32>, HarnessError> {
    let output = Command::new("pgrep")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|source| HarnessError::Spawn {
            program: "pgrep".to_owned(),
            source,
        })?;
    // pgrep exits 0 with matches, 1 with none, 2 on a bad invocation and 3
    // on an internal error; the last two must not read as "none".
    match output.status.code() {
        Some(0 | 1) => Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()),
        code => Err(HarnessError::Spawn {
            program: "pgrep".to_owned(),
            source: std::io::Error::other(format!("pgrep exited with {code:?}")),
        }),
    }
}

/// Process ids of running processes named exactly `name`, plus those whose
/// command line names `binary` (an npm-installed harness runs as `node`
/// with the script path on its command line).
pub(crate) async fn running_pids(
    name: &str,
    binary: Option<&Path>,
) -> Result<Vec<u32>, HarnessError> {
    let mut pids = pgrep(&[OsStr::new("-x"), OsStr::new(name)]).await?;
    if let Some(binary) = binary {
        let physical = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
        pids.extend(pgrep(&[OsStr::new("-f"), physical.as_os_str()]).await?);
    }
    let own = std::process::id();
    pids.retain(|pid| *pid != own);
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

/// True when a process with this id exists, asked of `ps`.
pub(crate) async fn pid_alive(pid: u32) -> Result<bool, HarnessError> {
    let output = Command::new("ps")
        .arg("-o")
        .arg("pid=")
        .arg("-p")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|source| HarnessError::Spawn {
            program: "ps".to_owned(),
            source,
        })?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.trim().parse::<u32>() == Ok(pid)))
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

    #[tokio::test]
    async fn pid_alive_sees_this_process_and_not_a_dead_one() {
        assert!(pid_alive(std::process::id()).await.unwrap());
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid).await.unwrap());
    }
}
