//! Finding, probing and starting harness binaries.
//!
//! Invariant: a harness starts in its own process group with the
//! environment the caller passed minus the parent-session markers of
//! `markers`, never through a shell, so the supervisor can fence the whole
//! group (ADR 0003), no variable of the calling process leaks into it, and
//! no Claude Code or Codex session this process runs under claims it as a
//! child. A version probe runs with this process's environment minus the
//! same markers, so even that one short call is no child of a host
//! session, and with `PATH` set to the value the binary was found on. A question about running processes fails closed:
//! when `pgrep` or `ps` cannot answer, the caller is told so, never "none".
//! `ps`, `pgrep` and `kill` are run from their system paths, never looked
//! up on this process's `PATH`, so a planted `ps` cannot report a live
//! writer as gone; only a build with the `counted-tools` feature, which
//! the end-to-end time budget makes, takes them from the directory
//! `FABRIC_SYSTEM_TOOLS` names. A question about running processes includes the
//! processes this one runs under: BSD `pgrep` on macOS leaves out its own
//! ancestors unless given `-a` (pgrep(1)), and when Fabric runs from a
//! Claude Code session the host `claude` is one of them. procps `pgrep` on
//! Linux includes ancestors already, and there `-a` changes the output, so
//! the flag is passed on macOS only.
//!
//! Every harness starts held, behind a trampoline: `/bin/sh` stops itself
//! with `SIGSTOP` and only then execs the harness, whose path and
//! arguments reach it as positional parameters, never inside the script
//! text. `spawn` returns once the trampoline has stopped, as `waitid`
//! reports it, so nothing of the harness has run yet. The caller records
//! the pid with the node, gives the group the terminal when the harness is
//! wired to it, and only then continues the group (`Running::release`).
//! The `SIGCONT` reaches the shell before its exec, so the harness itself
//! never sees one.
//!
//! Two rules need the hold. First, a harness the node has not recorded
//! never runs, since no suspend, move or fence could stop it (issue #17).
//! A caller killed before it records the harness, even by `SIGKILL`,
//! leaves a stopped shell whose new parent, `launchd` or `init`, is in
//! another session. Its group is then orphaned, and the kernel sends an
//! orphaned group that has a stopped member `SIGHUP` and then `SIGCONT`,
//! so the shell ends on the `SIGHUP` before its exec. That needs `SIGHUP`
//! at its default action in the shell: a caller that ignores it passes
//! the ignore on through the exec, so the CLI installs its own handler,
//! which an exec resets, before it launches. A reaper in the caller's own
//! session leaves the group not orphaned and the shell stopped for good,
//! which still never runs the harness. Second, a harness wired to the
//! user's terminal never runs before its process group owns that
//! terminal. A process in a group the terminal does not have in the
//! foreground is stopped by `SIGTTOU` when it sets the terminal's modes
//! and by `SIGTTIN` when it reads it, and the `SIGCONT` that lets it go on
//! interrupts the call it was in: Codex 0.155.1 exited with "Interrupted
//! system call" this way (gate finding F5). A process cannot be given the
//! terminal between its fork and its exec without unsafe code, so the
//! caller gives it to the held group instead.

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
use fabric_core::version::version_token;
use tokio::process::{Child, Command};

use crate::error::HarnessError;
use crate::harness::{Installed, Launch};
use crate::markers;

/// How long a version probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The shell that holds a harness before its exec.
const TRAMPOLINE: &str = "/bin/sh";
/// What the trampoline runs: stop, and once continued, become the harness.
/// The harness's path and arguments are its positional parameters. bash,
/// which is `/bin/sh` on macOS, imports exported functions from the
/// environment, and a function named `kill` would run in place of the
/// builtin; `unset` is a special builtin no function can shadow.
const TRAMPOLINE_SCRIPT: &str = "unset -f kill && kill -STOP $$ && exec \"$@\"";
/// The trampoline's `$0`, which `ps -o args` shows while it waits.
const TRAMPOLINE_NAME: &str = "fabric-hold";
/// How long the trampoline may take to stop itself.
const HOLD_TIMEOUT: Duration = Duration::from_secs(10);
/// How often `spawn` asks whether the trampoline has stopped.
const HOLD_POLL: Duration = Duration::from_millis(2);

/// Where macOS and Linux keep `ps`.
const PS: &[&str] = &["/bin/ps", "/usr/bin/ps"];
/// Where macOS and Linux keep `pgrep`.
const PGREP: &[&str] = &["/usr/bin/pgrep", "/bin/pgrep"];
/// Where macOS and Linux keep `kill`.
const KILL: &[&str] = &["/bin/kill", "/usr/bin/kill"];
/// Where macOS and Linux keep `lsof`.
pub(crate) const LSOF: &[&str] = &["/usr/sbin/lsof", "/usr/bin/lsof", "/sbin/lsof", "/bin/lsof"];
/// The `pgrep` flag that keeps the caller's ancestors in the answer.
#[cfg(target_os = "macos")]
const PGREP_WITH_ANCESTORS: &[&str] = &["-a"];
/// procps `pgrep` keeps ancestors without a flag.
#[cfg(not(target_os = "macos"))]
const PGREP_WITH_ANCESTORS: &[&str] = &[];

/// The first of `candidates` that is an executable file.
///
/// With the `counted-tools` feature, `program` in the directory
/// `FABRIC_SYSTEM_TOOLS` names comes first when it is there, so the
/// end-to-end time budget can count each start (issue #26).
pub(crate) fn system_tool(
    program: &'static str,
    candidates: &'static [&'static str],
) -> Result<&'static Path, HarnessError> {
    #[cfg(feature = "counted-tools")]
    if let Some(counted) = counted_tool(program) {
        return Ok(counted);
    }
    candidates
        .iter()
        .map(|candidate| Path::new(*candidate))
        .find(|path| is_executable(path))
        .ok_or_else(|| HarnessError::ProcessQuery {
            program,
            reason: format!("not found at {}", candidates.join(" or ")),
        })
}

/// `program` in the directory `FABRIC_SYSTEM_TOOLS` names, read once,
/// when it is an executable file there.
#[cfg(feature = "counted-tools")]
fn counted_tool(program: &str) -> Option<&'static Path> {
    static TOOLS: std::sync::OnceLock<Vec<(&'static str, PathBuf)>> = std::sync::OnceLock::new();
    TOOLS
        .get_or_init(|| {
            let Some(dir) = std::env::var_os("FABRIC_SYSTEM_TOOLS") else {
                return Vec::new();
            };
            ["kill", "lsof", "pgrep", "ps"]
                .into_iter()
                .map(|name| (name, Path::new(&dir).join(name)))
                .collect()
        })
        .iter()
        .find(|(name, path)| *name == program && is_executable(path))
        .map(|(_, path)| path.as_path())
}

/// Looks `name` up on the given `PATH` value, as a shell would, except
/// that a relative or empty entry is skipped: it names a directory
/// relative to whatever the current directory is, which is Fabric's own
/// here and the workspace once the harness starts, so the file found and
/// the file launched could differ. The path returned is always absolute.
#[must_use]
pub(crate) fn find_on_path(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// This process's environment, as far as its names and values are text
/// a launch env can hold, with `PATH` set to `path_var`: the binary was
/// found on that value, and a shim such as `#!/usr/bin/env node` must find
/// its interpreter there too, as it will at launch.
fn probe_env(path_var: &OsStr) -> Vec<(EnvVarName, String)> {
    let mut env: Vec<(EnvVarName, String)> = std::env::vars_os()
        .filter_map(|(name, value)| {
            let name = EnvVarName::parse(name.to_str()?).ok()?;
            Some((name, value.into_string().ok()?))
        })
        .filter(|(name, _)| name.as_str() != "PATH")
        .collect();
    if let (Ok(name), Some(value)) = (EnvVarName::parse("PATH"), path_var.to_str()) {
        env.push((name, value.to_owned()));
    }
    env
}

/// Runs `binary args...` with `env` minus its parent-session markers, in
/// `cwd` when one is given and else in this process's directory, and
/// returns trimmed stdout, within `PROBE_TIMEOUT`. A run that exits with
/// a failure is an error, whatever it printed: a wrapper that prints a
/// version and then fails to start is not a working install, and its
/// version must not be trusted.
async fn probe(
    binary: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(EnvVarName, String)],
) -> Result<String, HarnessError> {
    let program = binary.display().to_string();
    let mut command = Command::new(binary);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let run = command
        .args(args)
        .env_clear()
        .envs(markers::child_env(env))
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
    if !output.status.success() {
        return Err(HarnessError::Spawn {
            program,
            source: std::io::Error::other(format!(
                "{} exited with {}",
                args.join(" "),
                output.status
            )),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Asks `binary`, found on the `PATH` value `path_var`, for its version;
/// `None` when there is no binary. A binary that answers with no version
/// in its output is still installed, with `version` set to `None`.
pub(crate) async fn detect(
    binary: Option<PathBuf>,
    path_var: &OsStr,
) -> Result<Option<Installed>, HarnessError> {
    detect_in(binary, None, path_var).await
}

/// `detect`, run in `cwd` when one is given.
async fn detect_in(
    binary: Option<PathBuf>,
    cwd: Option<&Path>,
    path_var: &OsStr,
) -> Result<Option<Installed>, HarnessError> {
    let Some(path) = binary else {
        return Ok(None);
    };
    let raw = probe(&path, &["--version"], cwd, &probe_env(path_var)).await?;
    Ok(Some(Installed {
        version: version_token(&raw).map(str::to_owned),
        raw,
        path,
    }))
}

/// What `binary --version` prints when run in `cwd` with `env` minus its
/// parent-session markers, as a launch in `cwd` with `env` would run it;
/// for the resume guard (`version::refuse_older`), which must ask the
/// binary it is about to start. The directory counts: a version manager's
/// shim (mise, asdf, Volta) picks the tool version by the directory it
/// runs in, so a probe from anywhere else could ask another version than
/// the one `spawn` starts in the workspace.
pub(crate) async fn reported_version(
    binary: &Path,
    cwd: &Path,
    env: &[(EnvVarName, String)],
) -> Result<String, HarnessError> {
    probe(binary, &["--version"], Some(cwd), env).await
}

/// The version `binary` reports, or `None` when there is no binary or the
/// probe fails; a failed probe is logged, never fatal.
pub(crate) async fn installed_version(
    binary: Option<PathBuf>,
    path_var: &OsStr,
    program: &str,
) -> Option<String> {
    installed_version_in(binary, None, path_var, program).await
}

/// `installed_version`, run in `cwd` when one is given: a capture asks in
/// the workspace, where a version manager's shim picks the same Codex a
/// launch there runs (`reported_version`), not the one it picks for the
/// daemon's directory.
pub(crate) async fn installed_version_in(
    binary: Option<PathBuf>,
    cwd: Option<&Path>,
    path_var: &OsStr,
    program: &str,
) -> Option<String> {
    match detect_in(binary, cwd, path_var).await {
        Ok(installed) => installed.and_then(|i| i.version),
        Err(e) => {
            tracing::warn!(program, error = %e, "version probe failed");
            None
        }
    }
}

/// How a spawned process's stdio is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wiring {
    /// The user's terminal: stdin, stdout and stderr are inherited, and
    /// the caller gives the held group the terminal before it releases it.
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

/// A process `spawn` started, in a process group it leads: the
/// trampoline, stopped before it execs the harness.
pub(crate) struct Spawned {
    /// The child.
    pub(crate) child: Child,
    /// Its pid, which is also its process group id.
    pub(crate) pid: NonZeroU32,
    /// How its stdio is wired.
    pub(crate) wiring: Wiring,
}

/// Starts `binary args...` held in `cwd`, in a new process group, with
/// `env` minus its parent-session markers, and the stdio `wiring` asks
/// for.
///
/// The trampoline runs in the harness's place, and this returns once it
/// has stopped, so nothing of the harness has run yet; `Running::release`
/// lets it start. A trampoline that exits instead, or does not stop within
/// `HOLD_TIMEOUT`, fails the spawn and its group is killed.
pub(crate) async fn spawn(
    binary: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(EnvVarName, String)],
    wiring: Wiring,
) -> Result<Spawned, HarnessError> {
    let mut command = Command::new(TRAMPOLINE);
    command
        .arg("-c")
        .arg(TRAMPOLINE_SCRIPT)
        .arg(TRAMPOLINE_NAME)
        .arg(binary)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(markers::child_env(env))
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
    let program = format!("{TRAMPOLINE} holding {}", binary.display());
    let spawn_error = |source| HarnessError::Spawn {
        program: program.clone(),
        source,
    };
    let child = command.spawn().map_err(spawn_error)?;
    let pid = child
        .id()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| spawn_error(std::io::Error::other("the process had no pid after spawn")))?;
    let guard = GroupGuard::new(pid);
    wait_stopped(pid).await.map_err(spawn_error)?;
    guard.disarm();
    Ok(Spawned { child, pid, wiring })
}

/// Waits until the trampoline `pid` has stopped itself. It leaves the
/// stop, and an exit, waitable (`WNOWAIT`): tokio reaps the child later
/// and never asks for stops.
async fn wait_stopped(pid: NonZeroU32) -> std::io::Result<()> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let raw = i32::try_from(pid.get()).map_err(std::io::Error::other)?;
    let child = Pid::from_raw(raw).ok_or_else(|| std::io::Error::other("pid 0"))?;
    let options = WaitIdOptions::STOPPED
        | WaitIdOptions::EXITED
        | WaitIdOptions::NOWAIT
        | WaitIdOptions::NOHANG;
    let deadline = tokio::time::Instant::now() + HOLD_TIMEOUT;
    loop {
        // The status holds raw pointers and is not `Send`, so it is read
        // to plain values before the next await.
        let ended = match waitid(WaitId::Pid(child), options)? {
            Some(status) if status.stopped() => return Ok(()),
            Some(status) => Some((status.exit_status(), status.terminating_signal())),
            None => None,
        };
        if let Some((code, signal)) = ended {
            return Err(std::io::Error::other(format!(
                "the hold ended before the harness started (exit status {code:?}, signal {signal:?})"
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("the hold did not stop within {HOLD_TIMEOUT:?}"),
            ));
        }
        tokio::time::sleep(HOLD_POLL).await;
    }
}

/// Continues the process group `pgid`, as `Running::release` does for a held harness.
pub(crate) fn continue_group(pgid: NonZeroU32) -> Result<(), HarnessError> {
    use rustix::process::{Pid, Signal, kill_process_group};
    let group = i32::try_from(pgid.get())
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| HarnessError::ProcessQuery {
            program: "kill",
            reason: format!("{pgid} is not a process group id"),
        })?;
    kill_process_group(group, Signal::CONT).map_err(|e| HarnessError::ProcessQuery {
        program: "kill",
        reason: format!("SIGCONT to group {pgid}: {e}"),
    })
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
        .args(PGREP_WITH_ANCESTORS)
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

/// `text` as an extended regular expression that matches only itself:
/// `pgrep` reads its operand as a pattern, and a path may hold `[`, `(`
/// or `.`.
fn ere_literal(text: &OsStr) -> std::ffi::OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let mut out = Vec::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if b"\\.[]()*+?{}|^$".contains(&byte) {
            out.push(b'\\');
        }
        out.push(byte);
    }
    std::ffi::OsString::from_vec(out)
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
        let pattern = ere_literal(physical.as_os_str());
        pids.extend(pgrep(&[OsStr::new("-f"), &pattern]).await?);
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

/// Process ids, other than this one, that hold any of `files` open,
/// asked of `lsof`. It lists them and exits 0 when every file is open, or
/// 1 when some is not, and then says nothing on its error output; anything
/// on that output, as for a file that vanished, or any other exit is an
/// error, never "none". It runs from a blocking thread, under a lock the
/// caller holds, so it is synchronous.
pub(crate) fn holding(files: &[PathBuf]) -> Result<Vec<u32>, HarnessError> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let output = std::process::Command::new(system_tool("lsof", LSOF)?)
        .arg("-t")
        .arg("--")
        .args(files)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| HarnessError::Spawn {
            program: "lsof".to_owned(),
            source,
        })?;
    let own = std::process::id();
    let mut pids: Vec<u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid| *pid != own)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    match output.status.code() {
        Some(0 | 1) if output.stderr.trim_ascii().is_empty() => Ok(pids),
        code => Err(HarnessError::ProcessQuery {
            program: "lsof",
            reason: format!(
                "exited with {code:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `contents` to `path` with mode `0755` through a short `sh`,
    /// so this process never holds the file open for writing. Linux
    /// refuses to run a file open for writing (`ETXTBSY`), and a child
    /// another test forked while this process held it open would keep it
    /// open until that child ran its own program. A temporary file renamed
    /// into place would not help: the descriptor follows the file.
    fn write_executable(path: &Path, contents: &[u8]) {
        use std::io::Write;
        let mut writer = std::process::Command::new("/bin/sh")
            .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        // The pipe closes at the end of the statement, so `cat` sees the end.
        writer.stdin.take().unwrap().write_all(contents).unwrap();
        let status = writer.wait().unwrap();
        assert!(
            status.success(),
            "writing {} failed: {status}",
            path.display()
        );
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

    #[test]
    fn find_on_path_skips_relative_and_empty_entries() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The test runs from the crate directory; a relative entry that
        // reaches the tool from there must not count.
        let cwd = std::env::current_dir().unwrap();
        let up: PathBuf = cwd.components().skip(1).map(|_| "..").collect();
        let relative = up.join(dir.path().strip_prefix("/").unwrap());
        assert!(
            relative.join("tool").exists(),
            "the relative entry reaches the tool"
        );
        let var = std::env::join_paths([relative.as_path(), Path::new("")]).unwrap();
        assert_eq!(find_on_path("tool", &var), None);
        let var = std::env::join_paths([relative.as_path(), dir.path()]).unwrap();
        assert_eq!(find_on_path("tool", &var), Some(tool));
    }

    /// The role a run of `running_pids_sees_a_claude_this_process_runs_under` plays.
    const ANCESTOR_ROLE: &str = "FABRIC_TEST_ANCESTOR_ROLE";

    #[tokio::test]
    async fn running_pids_sees_a_claude_this_process_runs_under() {
        let test = "process::tests::running_pids_sees_a_claude_this_process_runs_under";
        let run = |binary: &Path, role: &str| {
            let status = std::process::Command::new(binary)
                .args(["--exact", test, "--test-threads=1"])
                .env(ANCESTOR_ROLE, role)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "the {role} run failed");
        };
        match std::env::var(ANCESTOR_ROLE).ok().as_deref() {
            None => {
                let dir = tempfile::tempdir().unwrap();
                let claude = dir.path().join("claude");
                write_executable(
                    &claude,
                    &std::fs::read(std::env::current_exe().unwrap()).unwrap(),
                );
                run(&claude, "parent");
            }
            Some("parent") => run(&std::env::current_exe().unwrap(), "child"),
            Some(_) => {
                let parent = std::os::unix::process::parent_id();
                let pids = running_pids("claude", None).await.unwrap();
                assert!(
                    pids.contains(&parent),
                    "the claude this process runs under ({parent}) is missing from {pids:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_version_probe_does_not_pass_parent_session_markers() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("tool");
        write_executable(&binary, b"#!/bin/sh\n/usr/bin/env\n");
        let env: Vec<(EnvVarName, String)> = [("CLAUDECODE", "1"), ("FABRIC_PROBE_KEPT", "1")]
            .into_iter()
            .map(|(name, value)| (EnvVarName::parse(name).unwrap(), value.to_owned()))
            .collect();
        let seen = probe(&binary, &[], None, &env).await.unwrap();
        assert!(seen.contains("FABRIC_PROBE_KEPT=1"), "{seen}");
        assert!(!seen.contains("CLAUDECODE"), "{seen}");
    }

    #[tokio::test]
    async fn a_version_probe_that_exits_with_a_failure_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("tool");
        write_executable(&binary, b"#!/bin/sh\necho 'tool 1.2.3'\nexit 1\n");
        let err = detect(Some(binary.clone()), OsStr::new("/usr/bin:/bin"))
            .await
            .unwrap_err();
        assert!(matches!(err, HarnessError::Spawn { .. }), "{err}");
        assert_eq!(
            installed_version(Some(binary), OsStr::new("/usr/bin:/bin"), "tool").await,
            None
        );
    }

    #[tokio::test]
    async fn a_version_probe_finds_an_interpreter_on_the_configured_path() {
        let dir = tempfile::tempdir().unwrap();
        let interp = dir.path().join("fabric-test-interp");
        write_executable(&interp, b"#!/bin/sh\necho 'tool 4.5.6'\n");
        let binary = dir.path().join("tool");
        write_executable(&binary, b"#!/usr/bin/env fabric-test-interp\n");
        let path_var =
            std::env::join_paths([dir.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
        let installed = detect(Some(binary), &path_var).await.unwrap().unwrap();
        assert_eq!(installed.version.as_deref(), Some("4.5.6"));
    }

    #[test]
    fn ere_literal_escapes_every_metacharacter() {
        assert_eq!(
            ere_literal(OsStr::new("/a.b/[x](y)*+?{1}|^$\\")),
            OsStr::new("/a\\.b/\\[x\\]\\(y\\)\\*\\+\\?\\{1\\}\\|\\^\\$\\\\")
        );
    }

    #[tokio::test]
    async fn running_pids_finds_a_script_whose_path_holds_regex_characters() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("lib[1](x)");
        std::fs::create_dir_all(&bin).unwrap();
        let script = std::fs::canonicalize(&bin).unwrap().join("claude.js");
        std::fs::write(&script, "").unwrap();
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30; :")
            .arg(&script)
            .spawn()
            .unwrap();
        let pid = child.id();
        let pids = running_pids("fabric-no-such-name", Some(&script)).await;
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(pids.unwrap().contains(&pid), "the writer was missed");
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
