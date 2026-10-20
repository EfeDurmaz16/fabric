//! Finding, probing and starting harness binaries.
//!
//! Invariant: a harness starts in its own process group with exactly the
//! environment the caller passed, never through a shell, so the supervisor
//! can fence the whole group (ADR 0003) and no variable of the calling
//! process leaks into it. A question about running processes fails closed:
//! when `pgrep` or `ps` cannot answer, the caller is told so, never "none".
//! `ps`, `pgrep` and `kill` are run from their system paths, never looked
//! up on this process's `PATH`, so a planted `ps` cannot report a live
//! writer as gone.

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
use crate::harness::{Installed, Launch};

/// How long a version probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Where macOS and Linux keep `ps`.
const PS: &[&str] = &["/bin/ps", "/usr/bin/ps"];
/// Where macOS and Linux keep `pgrep`.
const PGREP: &[&str] = &["/usr/bin/pgrep", "/bin/pgrep"];
/// Where macOS and Linux keep `kill`.
const KILL: &[&str] = &["/bin/kill", "/usr/bin/kill"];

/// The first of `candidates` that is an executable file.
fn system_tool(
    program: &'static str,
    candidates: &'static [&'static str],
) -> Result<&'static Path, HarnessError> {
    candidates
        .iter()
        .map(|candidate| Path::new(*candidate))
        .find(|path| is_executable(path))
        .ok_or_else(|| HarnessError::ProcessQuery {
            program,
            reason: format!("not found at {}", candidates.join(" or ")),
        })
}

/// Looks `name` up on the given `PATH` value, as a shell would.
#[must_use]
pub(crate) fn find_on_path(name: &str, path_var: &OsStr) -> Option<PathBuf> {
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

/// Asks `binary` for its version; `None` when there is no binary. A
/// binary that answers with no version in its output is still installed,
/// with `version` set to `None`.
pub(crate) async fn detect(binary: Option<PathBuf>) -> Result<Option<Installed>, HarnessError> {
    let Some(path) = binary else {
        return Ok(None);
    };
    let raw = probe(&path, &["--version"]).await?;
    Ok(Some(Installed {
        version: version_token(&raw).map(str::to_owned),
        raw,
        path,
    }))
}

/// The version `binary` reports, or `None` when there is no binary or the
/// probe fails; a failed probe is logged, never fatal.
pub(crate) async fn installed_version(binary: Option<PathBuf>, program: &str) -> Option<String> {
    match detect(binary).await {
        Ok(installed) => installed.and_then(|i| i.version),
        Err(e) => {
            tracing::warn!(program, error = %e, "version probe failed");
            None
        }
    }
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

impl From<&Launch> for Wiring {
    fn from(launch: &Launch) -> Self {
        match launch {
            Launch::Interactive { .. } => Wiring::Terminal,
            Launch::Headless { .. } => Wiring::Piped,
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

/// Sends `SIGKILL` to the process group `pgid` through `kill(1)`; the
/// crate forbids unsafe code, so it does not call `killpg` itself.
pub(crate) fn kill_group(pgid: NonZeroU32) -> Result<(), HarnessError> {
    let status = std::process::Command::new(system_tool("kill", KILL)?)
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{pgid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| HarnessError::Spawn {
            program: "kill".to_owned(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(HarnessError::ProcessQuery {
            program: "kill",
            reason: format!("kill -KILL -{pgid} exited with {status}"),
        })
    }
}

/// Kills the process group of a child this crate spawned when dropped,
/// unless it was disarmed first. Tokio reaps the dropped child.
pub(crate) struct GroupGuard {
    pgid: Option<NonZeroU32>,
}

impl GroupGuard {
    /// Arms the guard for the group `pgid`.
    pub(crate) fn new(pgid: NonZeroU32) -> Self {
        GroupGuard { pgid: Some(pgid) }
    }

    /// Leaves the group running.
    pub(crate) fn disarm(mut self) {
        self.pgid = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid
            && let Err(e) = kill_group(pgid)
        {
            tracing::warn!(%pgid, error = %e, "could not kill the process group of a failed launch");
        }
    }
}

/// Unix milliseconds now; informational.
#[must_use]
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

async fn pgrep(args: &[&OsStr]) -> Result<Vec<u32>, HarnessError> {
    let output = Command::new(system_tool("pgrep", PGREP)?)
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
        code => Err(HarnessError::ProcessQuery {
            program: "pgrep",
            reason: format!("exited with {code:?}"),
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

/// True when a process with this id exists, asked of `ps`. `ps` exits 0
/// and prints the pid for a live process, and exits 1 with no output for
/// none; any other answer is an error, never "dead".
pub(crate) async fn pid_alive(pid: u32) -> Result<bool, HarnessError> {
    let output = Command::new(system_tool("ps", PS)?)
        .arg("-o")
        .arg("pid=")
        .arg("-p")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|source| HarnessError::Spawn {
            program: "ps".to_owned(),
            source,
        })?;
    let listed = String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.trim().parse::<u32>() == Ok(pid));
    let silent = output.stdout.trim_ascii().is_empty() && output.stderr.trim_ascii().is_empty();
    match output.status.code() {
        Some(0) if listed => Ok(true),
        Some(1) if silent => Ok(false),
        code => Err(HarnessError::ProcessQuery {
            program: "ps",
            reason: format!(
                "ps -p {pid} exited with {code:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }),
    }
}

/// True when the dotted version `version` is at least `floor`, compared
/// part by part as numbers; a part that is not a number fails the check.
#[must_use]
pub(crate) fn version_at_least(version: &str, floor: &[u32]) -> bool {
    let parts: Option<Vec<u32>> = version.split('.').map(|p| p.parse().ok()).collect();
    parts.is_some_and(|parts| parts.as_slice() >= floor)
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
    fn version_at_least_compares_numbers_not_text() {
        assert!(version_at_least("2.1.294", &[2, 1, 294]));
        assert!(version_at_least("2.1.300", &[2, 1, 294]));
        assert!(version_at_least("2.10.0", &[2, 9, 999]));
        assert!(!version_at_least("2.1.199", &[2, 1, 294]));
        assert!(!version_at_least("2.1", &[2, 1, 294]));
        assert!(!version_at_least("2.1.x", &[2, 1, 294]));
    }

    #[test]
    fn find_on_path_needs_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("tool");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        let var = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_on_path("tool", &var), None);
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_on_path("tool", &var), Some(plain));
        assert_eq!(find_on_path("tool", OsStr::new("")), None);
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
