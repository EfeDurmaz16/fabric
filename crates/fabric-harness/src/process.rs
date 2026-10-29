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
//! Every harness starts held, behind a trampoline: a shell that waits and
//! only then execs the harness, whose path and arguments reach it as
//! positional parameters, never inside the script text. `spawn` returns as
//! soon as the trampoline has a pid, while the shell still starts, so the
//! caller records the pid with the node meanwhile, gives the group the
//! terminal when the harness is wired to it, and then releases it
//! (`Running::release`), which may come before the shell is ready.
//!
//! How it waits depends on its standard input. A harness that owns no pipe
//! there, headless or speaking a protocol on its stdio, gets a trampoline
//! that reads one line from a pipe the caller holds: the release writes the
//! line, whenever the shell gets to read it. A harness wired to the user's
//! terminal keeps the terminal as its standard input, so its trampoline
//! stops itself with `SIGSTOP`, a watcher thread sees the stop, and the
//! release continues the group once the stop has settled (`STOP_SETTLE`:
//! on macOS a `SIGCONT` sent the moment the stop is seen can be lost). The
//! `SIGCONT` reaches the shell before its exec, so the harness itself never
//! sees one.
//!
//! Two rules need the hold. First, a harness the node has not recorded
//! never runs, since no suspend, move or fence could stop it (issue #17).
//! A caller killed before it records the harness, even by `SIGKILL`,
//! closes the pipe a line trampoline reads, which then exits without its
//! exec. A stopped trampoline is left with a new parent, `launchd` or
//! `init`, in another session; its group is then orphaned, and the kernel
//! sends an orphaned group that has a stopped member `SIGHUP` and then
//! `SIGCONT`, so the shell ends on the `SIGHUP` before its exec. That
//! needs `SIGHUP` at its default action in the shell: a caller that
//! ignores it passes the ignore on through the exec, so the CLI installs
//! its own handler, which an exec resets, before it launches. A caller
//! killed before the trampoline has stopped, or a reaper in the caller's
//! own session, leaves the shell stopped for good, which still never runs
//! the harness. Second, a harness wired to the user's terminal never runs
//! before its process group owns that terminal. A process in a group the
//! terminal does not have in the foreground is stopped by `SIGTTOU` when it
//! sets the terminal's modes and by `SIGTTIN` when it reads it, and the
//! `SIGCONT` that lets it go on interrupts the call it was in: Codex
//! 0.155.1 exited with "Interrupted system call" this way (gate finding
//! F5). A process cannot be given the terminal between its fork and its
//! exec without unsafe code, so the caller gives it to the held group
//! instead.

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
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fabric_core::env::EnvVarName;
use fabric_core::version::version_token;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};

use crate::error::HarnessError;
use crate::harness::{Installed, Launch};
use crate::markers;

/// How long a version probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The shells that can hold a harness before its exec, in order: dash,
/// where the system has it, starts in 1.9 ms on the bench Mac where
/// `/bin/sh`, a shim there that starts bash, takes 3.4 ms (median of 40
/// starts of `-c exit`, 2026-10-28; Debian's `/bin/sh` is dash already).
/// Both run the trampoline's script alike. Every macOS and Linux node has
/// `/bin/sh`.
const TRAMPOLINES: [&str; 2] = ["/bin/dash", "/bin/sh"];
/// What a trampoline held by a stop runs: stop, and once continued, become
/// the harness. The harness's path and arguments are its positional
/// parameters. bash, which is `/bin/sh` on macOS, imports exported
/// functions from the environment, and a function named `kill` would run
/// in place of the builtin; `unset` is a special builtin no function can
/// shadow.
const STOP_SCRIPT: &str = "unset -f kill && kill -STOP $$ && exec \"$@\"";
/// What a trampoline held by a line runs: read one line from its standard
/// input, a pipe the caller holds, and only then become the harness. At
/// the pipe's end without a line, as when the caller died, `read` fails
/// and the shell exits without its exec. `read` is no special builtin, so
/// a function of that name is removed first, as for `kill`.
const LINE_SCRIPT: &str = "unset -f read && read -r go && exec \"$@\"";
/// `LINE_SCRIPT` for a harness that reads nothing: it gets `/dev/null`, as
/// a headless harness always has.
const LINE_SCRIPT_NO_INPUT: &str = "unset -f read && read -r go && exec \"$@\" </dev/null";
/// The trampoline's `$0`, which `ps -o args` shows while it waits.
const TRAMPOLINE_NAME: &str = "fabric-hold";
/// How long a trampoline held by a stop may take to stop itself.
const HOLD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the watcher waits before it asks again, should a wait not
/// name the trampoline.
const HOLD_POLL: Duration = Duration::from_millis(2);
/// How long after it sees a stop the watcher lets the stop finish before
/// a release may continue the group. XNU wakes a parent waiting for the
/// stop inside `stop()` before it suspends the task (`bsd/kern/kern_sig.c`),
/// and a `SIGCONT` sent in between resumes a task not yet suspended and is
/// lost: the shell then stays suspended inside `kill` for good. Sent at
/// once, 16 of 960 early releases hung that way here. Linux marks the task
/// stopped before it tells the parent, and a `SIGCONT` wakes a task so
/// marked even before it has been scheduled out, so it needs none.
#[cfg(target_os = "macos")]
const STOP_SETTLE: Duration = Duration::from_millis(10);
#[cfg(not(target_os = "macos"))]
const STOP_SETTLE: Duration = Duration::ZERO;

/// How a harness is held before its exec: the shell, and how long one held
/// by a stop may take to stop itself. Every launch uses
/// `Trampoline::standard`; a test can hold a harness behind a slower or
/// failing script.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Trampoline {
    /// The shell.
    pub(crate) shell: &'static str,
    /// A script in place of the one the wiring picks; for tests.
    pub(crate) script: Option<&'static str>,
    /// How long a stop may take.
    pub(crate) timeout: Duration,
}

impl Trampoline {
    /// The trampoline every launch uses, in the first of `TRAMPOLINES`
    /// this system has, looked up once.
    pub(crate) fn standard() -> Trampoline {
        static SHELL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        let shell = SHELL.get_or_init(|| {
            TRAMPOLINES
                .into_iter()
                .find(|shell| is_executable(Path::new(shell)))
                .unwrap_or(TRAMPOLINES[TRAMPOLINES.len() - 1])
        });
        Trampoline {
            shell,
            script: None,
            timeout: HOLD_TIMEOUT,
        }
    }

    /// What the shell runs for a harness wired as `wiring`: a harness that
    /// owns no pipe on its standard input waits for a line on one, and one
    /// wired to the terminal, whose standard input is the user's, stops.
    fn script(&self, wiring: Wiring) -> &'static str {
        self.script.unwrap_or(match wiring {
            Wiring::Terminal => STOP_SCRIPT,
            Wiring::Piped => LINE_SCRIPT_NO_INPUT,
            Wiring::Rpc => LINE_SCRIPT,
        })
    }
}

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
/// trampoline, which waits before it execs the harness.
pub(crate) struct Spawned {
    /// The child.
    pub(crate) child: Child,
    /// Its pid, which is also its process group id.
    pub(crate) pid: NonZeroU32,
    /// How its stdio is wired.
    pub(crate) wiring: Wiring,
    /// Its hold, which `Running::release` ends.
    pub(crate) hold: Hold,
    /// For `Wiring::Rpc`: fires when the hold is released, and the protocol
    /// client that owns the harness's standard input then writes the line
    /// the trampoline waits for, before anything else
    /// (`AcpClient::attach_held`).
    pub(crate) go: Option<oneshot::Receiver<()>>,
}

/// Starts `binary args...` held in `cwd`, in a new process group, with
/// `env` minus its parent-session markers, and the stdio `wiring` asks
/// for.
///
/// A trampoline runs in the harness's place, and this returns as soon as
/// it has a pid, while the shell still starts, so the caller registers it
/// meanwhile. Nothing of the harness runs before `Running::release`, which
/// may come before the shell is ready: a headless or protocol harness's
/// trampoline waits for a line on a pipe the caller holds, which the
/// release writes, and a terminal harness's stops itself and is continued
/// once the stop has been seen and has settled. A trampoline that exits
/// first, or one held by a stop that does not stop within `HOLD_TIMEOUT`,
/// has its group killed and fails the release; the harness never runs. A
/// line needs no deadline: nothing runs before the shell reads it, however
/// long the shell takes to start.
#[expect(
    clippy::unused_async,
    reason = "the launches await it; it no longer waits for the trampoline"
)]
pub(crate) async fn spawn(
    binary: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(EnvVarName, String)],
    wiring: Wiring,
) -> Result<Spawned, HarnessError> {
    spawn_held(Trampoline::standard(), binary, args, cwd, env, wiring)
}

/// `spawn`, held behind `trampoline`. It must run inside a Tokio runtime,
/// which runs the deadline of a hold by a stop.
pub(crate) fn spawn_held(
    trampoline: Trampoline,
    binary: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(EnvVarName, String)],
    wiring: Wiring,
) -> Result<Spawned, HarnessError> {
    let program = format!("{} holding {}", trampoline.shell, binary.display());
    let spawn_error = |source| HarnessError::Spawn {
        program: program.clone(),
        source,
    };
    let mut command = Command::new(trampoline.shell);
    command
        .arg("-c")
        .arg(trampoline.script(wiring))
        .arg(TRAMPOLINE_NAME)
        .arg(binary)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(markers::child_env(env))
        .process_group(0)
        .kill_on_drop(false);
    let mut line = None;
    match wiring {
        Wiring::Terminal => {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }
        Wiring::Piped => {
            let (read, write) = std::io::pipe().map_err(spawn_error)?;
            line = Some(write);
            command
                .stdin(Stdio::from(read))
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
    let child = command.spawn().map_err(spawn_error)?;
    // The command keeps the pipe's read end: closed here, so the release's
    // write finds no reader once the trampoline is gone, and fails.
    drop(command);
    let pid = child
        .id()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| spawn_error(std::io::Error::other("the process had no pid after spawn")))?;
    let guard = GroupGuard::new(pid);
    let (hold, go) = match wiring {
        Wiring::Terminal => {
            let stop =
                StopHold::watch(pid, program.clone(), trampoline.timeout).map_err(spawn_error)?;
            (Hold::new(pid, program, HoldKind::Stop(stop)), None)
        }
        Wiring::Piped => (Hold::new(pid, program, HoldKind::Line(line.take())), None),
        Wiring::Rpc => {
            let (go, released) = oneshot::channel();
            (
                Hold::new(pid, program, HoldKind::Go(Some(go))),
                Some(released),
            )
        }
    };
    guard.disarm();
    Ok(Spawned {
        child,
        pid,
        wiring,
        hold,
        go,
    })
}

/// The hold of a trampoline `spawn` started, which `Running::release`
/// ends: a line written to the pipe the trampoline reads, or a
/// `SIGCONT` once it has stopped.
#[derive(Debug)]
pub(crate) struct Hold {
    pid: NonZeroU32,
    /// The shell and the harness, for errors.
    program: String,
    kind: HoldKind,
    /// Released already.
    released: bool,
    /// Why the release failed, once it has.
    failed: Option<String>,
    /// Opened once the harness may run, for a driver that must not speak
    /// to it before (a hold by a stop opens it from its watcher instead).
    gate: Option<oneshot::Sender<()>>,
}

#[derive(Debug)]
enum HoldKind {
    /// Headless: the write end of the trampoline's standard input, until
    /// the release writes its line and closes it.
    Line(Option<std::io::PipeWriter>),
    /// A protocol harness: tells the client that owns its standard input
    /// to write the line first.
    Go(Option<oneshot::Sender<()>>),
    /// Wired to the terminal: the trampoline stops itself, and a watcher
    /// continues it.
    Stop(StopHold),
}

impl Hold {
    fn new(pid: NonZeroU32, program: String, kind: HoldKind) -> Self {
        Hold {
            pid,
            program,
            kind,
            released: false,
            failed: None,
            gate: None,
        }
    }

    /// Releases the harness. A line hold writes the line at once: the
    /// trampoline reads it whenever its shell gets there, and runs the
    /// harness then. A stop hold continues the group once the stop has
    /// been seen and has settled, at once when it has. Releasing twice is
    /// releasing once. Fails when the trampoline is gone, ended before its
    /// stop, did not stop in time, or could not be continued; its group is
    /// then killed and the harness never ran.
    pub(crate) fn release(&mut self) -> Result<(), HarnessError> {
        if let HoldKind::Stop(stop) = &self.kind {
            return stop.release();
        }
        if let Some(reason) = &self.failed {
            return Err(self.error(reason.clone()));
        }
        if self.released {
            return Ok(());
        }
        let outcome = match &mut self.kind {
            HoldKind::Line(pipe) => match pipe.take() {
                // One byte into a fresh pipe never waits; the pipe closes
                // after it.
                Some(mut pipe) => std::io::Write::write_all(&mut pipe, b"\n")
                    .map_err(|e| format!("the trampoline did not take its line: {e}")),
                None => Ok(()),
            },
            HoldKind::Go(go) => {
                // A client that is gone closed the trampoline's standard
                // input, which then exits without its exec.
                if let Some(go) = go.take() {
                    let _ = go.send(());
                }
                Ok(())
            }
            HoldKind::Stop(_) => Ok(()),
        };
        match outcome {
            Ok(()) => {
                self.released = true;
                if let Some(gate) = self.gate.take() {
                    // A driver that is gone has nothing left to say.
                    let _ = gate.send(());
                }
                Ok(())
            }
            Err(reason) => {
                kill_held_group(self.pid);
                self.gate = None;
                self.failed = Some(reason.clone());
                Err(self.error(reason))
            }
        }
    }

    /// Opens `gate` once the harness may run: at its release, or for a
    /// hold by a stop once the group is continued. A hold that fails
    /// closes it unopened.
    #[cfg(feature = "acp")]
    pub(crate) fn open_on_release(&mut self, gate: oneshot::Sender<()>) {
        if let HoldKind::Stop(stop) = &self.kind {
            stop.open_on_continue(gate);
        } else if self.released {
            let _ = gate.send(());
        } else if self.failed.is_none() {
            self.gate = Some(gate);
        }
    }

    /// Waits until a hold by a stop has stopped or failed; at once for any
    /// other.
    #[cfg(test)]
    pub(crate) async fn settled(&self) {
        if let HoldKind::Stop(stop) = &self.kind {
            stop.settled().await;
        }
    }

    fn error(&self, reason: String) -> HarnessError {
        HarnessError::Spawn {
            program: self.program.clone(),
            source: std::io::Error::other(reason),
        }
    }
}

/// The stack of a stop hold's watcher thread, which only waits, signals
/// and logs.
const WATCHER_STACK: usize = 128 * 1024;

/// A trampoline that stops itself, shared by its `Hold`, a watcher thread
/// and a deadline task.
///
/// The watcher blocks in `waitid` until the trampoline stops or ends, then
/// lets a stop settle (`STOP_SETTLE`) before it says so. The caller's
/// release may come before or after that: whichever comes second continues
/// the group, under one lock, so the group is continued exactly once and
/// only once both have happened. A trampoline that ends first, or that has
/// done neither by the deadline, fails the hold: its group is killed, a
/// release before that is dropped and one after it is an error, and a
/// driver's gate is closed unopened.
#[derive(Debug, Clone)]
pub(crate) struct StopHold {
    shared: Arc<StopShared>,
}

#[derive(Debug)]
struct StopShared {
    /// The trampoline's pid, which is also its group's id.
    pid: NonZeroU32,
    /// The shell and the harness, for errors.
    program: String,
    state: Mutex<StopState>,
    /// True once the trampoline has stopped or the hold failed.
    settled: watch::Sender<bool>,
}

#[derive(Debug, Default)]
struct StopState {
    phase: Phase,
    /// The caller has released the harness.
    released: bool,
    /// Opened when the group is continued.
    gate: Option<oneshot::Sender<()>>,
}

/// Where a stop hold is.
#[derive(Debug, Default)]
enum Phase {
    /// The trampoline has not stopped yet, or its stop is settling.
    #[default]
    Starting,
    /// It has stopped and waits for the release.
    Stopped,
    /// Its group was continued: the harness runs.
    Continued,
    /// It ended before its stop, or did not stop in time, and its group
    /// was killed; why.
    Failed(String),
}

impl StopHold {
    /// Starts watching the trampoline `pid`: a thread waits for its stop
    /// or its end, and a task fails the hold when neither came within
    /// `timeout`.
    fn watch(pid: NonZeroU32, program: String, timeout: Duration) -> std::io::Result<Self> {
        let (settled, mut done) = watch::channel(false);
        let shared = Arc::new(StopShared {
            pid,
            program,
            state: Mutex::new(StopState::default()),
            settled,
        });
        let watcher = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(TRAMPOLINE_NAME.to_owned())
            .stack_size(WATCHER_STACK)
            .spawn(move || watcher.wait())?;
        let deadline = Arc::clone(&shared);
        tokio::spawn(async move {
            if tokio::time::timeout(timeout, done.wait_for(|settled| *settled))
                .await
                .is_err()
            {
                deadline.fail(format!("the hold did not stop within {timeout:?}"));
            }
        });
        Ok(StopHold { shared })
    }

    /// Continues the group at once when the trampoline has stopped, or as
    /// soon as it does.
    fn release(&self) -> Result<(), HarnessError> {
        let shared = &self.shared;
        let mut state = shared.lock();
        state.released = true;
        match &state.phase {
            Phase::Starting | Phase::Continued => Ok(()),
            Phase::Stopped => shared.resume(&mut state),
            Phase::Failed(reason) => Err(HarnessError::Spawn {
                program: shared.program.clone(),
                source: std::io::Error::other(reason.clone()),
            }),
        }
    }

    /// Opens `gate` once the group is continued: at once when it already
    /// is, never when the hold failed.
    #[cfg(feature = "acp")]
    fn open_on_continue(&self, gate: oneshot::Sender<()>) {
        let mut state = self.shared.lock();
        match state.phase {
            Phase::Continued => {
                // A driver that is gone has nothing left to say.
                let _ = gate.send(());
            }
            Phase::Failed(_) => {}
            Phase::Starting | Phase::Stopped => state.gate = Some(gate),
        }
    }

    /// Waits until the trampoline has stopped or the hold failed.
    #[cfg(test)]
    async fn settled(&self) {
        let mut settled = self.shared.settled.subscribe();
        let _ = settled.wait_for(|settled| *settled).await;
    }
}

impl StopShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, StopState> {
        // Every change under the lock is a single assignment, so a panic
        // elsewhere leaves the state whole.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The watcher thread: blocks until the trampoline stops or ends. It
    /// leaves the stop, and an end, waitable (`WNOWAIT`): tokio reaps the
    /// child later and never asks for stops.
    fn wait(&self) {
        use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
        let Some(child) = i32::try_from(self.pid.get()).ok().and_then(Pid::from_raw) else {
            return self.fail(format!("{} is not a pid", self.pid));
        };
        let options = WaitIdOptions::STOPPED | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT;
        loop {
            match waitid(WaitId::Pid(child), options) {
                Ok(Some(status)) if status.stopped() => {
                    std::thread::sleep(STOP_SETTLE);
                    return self.stopped();
                }
                Ok(Some(status)) => {
                    return self.fail(format!(
                        "the hold ended before the harness started (exit status {:?}, signal {:?})",
                        status.exit_status(),
                        status.terminating_signal()
                    ));
                }
                // A wait without `WNOHANG` names a child; should one not,
                // ask again a little later rather than spin.
                Ok(None) => std::thread::sleep(HOLD_POLL),
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return self.fail(format!("waiting for the hold failed: {e}")),
            }
        }
    }

    /// The trampoline stopped: continue it when it was released already.
    fn stopped(&self) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Starting) {
            if state.released {
                if let Err(e) = self.resume(&mut state) {
                    tracing::warn!(pid = %self.pid, error = %e, "the released harness could not be continued, so its group was killed");
                }
            } else {
                state.phase = Phase::Stopped;
            }
        }
        drop(state);
        self.settled.send_replace(true);
    }

    /// The hold failed for `reason`, unless it had stopped already: kill
    /// the group and close the gate.
    fn fail(&self, reason: String) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Starting) {
            kill_held_group(self.pid);
            if state.released {
                tracing::warn!(pid = %self.pid, %reason, "the hold of a released harness failed, so it never ran");
            }
            state.phase = Phase::Failed(reason);
            state.gate = None;
        }
        drop(state);
        self.settled.send_replace(true);
    }

    /// Continues the stopped group and opens the gate; on a failure, kills
    /// the group and fails the hold.
    fn resume(&self, state: &mut StopState) -> Result<(), HarnessError> {
        match continue_group(self.pid) {
            Ok(()) => {
                state.phase = Phase::Continued;
                if let Some(gate) = state.gate.take() {
                    // A driver that is gone has nothing left to say.
                    let _ = gate.send(());
                }
                Ok(())
            }
            Err(e) => {
                kill_held_group(self.pid);
                state.phase = Phase::Failed(e.to_string());
                state.gate = None;
                Err(e)
            }
        }
    }
}

/// Sends `SIGKILL` to the group of a held harness; a group already gone
/// is no error.
pub(crate) fn kill_held_group(pgid: NonZeroU32) {
    #[cfg(test)]
    killed::note(pgid);
    let group = i32::try_from(pgid.get())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group
        && let Err(e) = rustix::process::kill_process_group(group, rustix::process::Signal::KILL)
        && e != rustix::io::Errno::SRCH
    {
        tracing::warn!(%pgid, error = %e, "could not kill the group of a held harness");
    }
}

/// The groups a test saw `kill_held_group` signal on its thread.
#[cfg(test)]
pub(crate) mod killed {
    use std::cell::RefCell;
    use std::num::NonZeroU32;

    thread_local! {
        static LOG: RefCell<Option<Vec<NonZeroU32>>> = const { RefCell::new(None) };
    }

    /// Runs `body` and returns what it returned with the groups it killed
    /// on this thread.
    pub(crate) fn during<T>(body: impl FnOnce() -> T) -> (T, Vec<NonZeroU32>) {
        LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));
        let out = body();
        let log = LOG.with(|log| log.borrow_mut().take()).unwrap_or_default();
        (out, log)
    }

    pub(super) fn note(pgid: NonZeroU32) {
        LOG.with(|log| {
            if let Some(log) = log.borrow_mut().as_mut() {
                log.push(pgid);
            }
        });
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
                crate::fake_tool::write_unrun(
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
        crate::fake_tool::write(&binary, b"#!/bin/sh\n/usr/bin/env\n");
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
        crate::fake_tool::write(&binary, b"#!/bin/sh\necho 'tool 1.2.3'\nexit 1\n");
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
        crate::fake_tool::write(&interp, b"#!/bin/sh\necho 'tool 4.5.6'\n");
        let binary = dir.path().join("tool");
        crate::fake_tool::write(&binary, b"#!/usr/bin/env fabric-test-interp\n");
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

    /// The hold, through the launch handle that ends it, which only a
    /// build with an adapter has.
    #[cfg(any(feature = "acp", feature = "claude", feature = "codex"))]
    mod hold {
        use super::*;

        /// How long a slow trampoline takes before it is ready, as on a busy
        /// machine, through a `sleep` in its own group.
        const HOLD_SLEEP: Duration = Duration::from_secs(2);

        /// A trampoline held by a stop that takes `HOLD_SLEEP` to stop.
        const SLOW_STOP: Trampoline = Trampoline {
            shell: "/bin/sh",
            script: Some("sleep 2 && kill -STOP $$ && exec \"$@\""),
            timeout: Duration::from_secs(20),
        };

        /// A trampoline held by a line that takes `HOLD_SLEEP` to read it.
        const SLOW_LINE: Trampoline = Trampoline {
            shell: "/bin/sh",
            script: Some("sleep 2 && read -r go && exec \"$@\" </dev/null"),
            timeout: Duration::from_secs(20),
        };

        /// A trampoline that exits before it is ready.
        const ENDS: Trampoline = Trampoline {
            shell: "/bin/sh",
            script: Some("exit 3"),
            timeout: Duration::from_secs(20),
        };

        /// How long a test waits for a held process to end.
        const ENDS_WITHIN: Duration = Duration::from_secs(20);

        /// Spawns, held behind `trampoline` and wired as `wiring`, a harness
        /// that writes the file `ran` in `dir`.
        fn spawn_marker(trampoline: Trampoline, wiring: Wiring, dir: &Path) -> (PathBuf, Spawned) {
            let ran = dir.join("ran");
            let args = [
                "-c".to_owned(),
                "echo ran > \"$0\"".to_owned(),
                ran.to_string_lossy().into_owned(),
            ];
            let spawned =
                spawn_held(trampoline, Path::new("/bin/sh"), &args, dir, &[], wiring).unwrap();
            (ran, spawned)
        }

        /// `spawn_marker` in a fresh directory, as a launch's handle.
        fn hold_marker(
            trampoline: Trampoline,
            wiring: Wiring,
        ) -> (tempfile::TempDir, PathBuf, crate::harness::Running) {
            let dir = tempfile::tempdir().unwrap();
            let (ran, spawned) = spawn_marker(trampoline, wiring, dir.path());
            (dir, ran, crate::harness::Running::new(None, spawned))
        }

        /// The exit status of `running`, within `ENDS_WITHIN`.
        async fn ends(running: &mut crate::harness::Running) -> std::process::ExitStatus {
            tokio::time::timeout(ENDS_WITHIN, running.child_mut().wait())
                .await
                .expect("the held process did not end")
                .unwrap()
        }

        /// True once no process of the group `pgid` is left, within `ENDS_WITHIN`.
        async fn group_ends(pgid: NonZeroU32) -> bool {
            let group = rustix::process::Pid::from_raw(i32::try_from(pgid.get()).unwrap()).unwrap();
            let deadline = tokio::time::Instant::now() + ENDS_WITHIN;
            while tokio::time::Instant::now() < deadline {
                if rustix::process::test_kill_process_group(group) == Err(rustix::io::Errno::SRCH) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            false
        }

        /// Item 1 of the A/B lab: `spawn` returns as soon as the trampoline
        /// has a pid, so the caller registers it while the shell starts, and a
        /// release that comes before the trampoline is ready is kept: the
        /// harness runs once it is, never before. A headless harness's
        /// trampoline waits for a line, a terminal harness's stops.
        #[tokio::test]
        async fn a_release_before_the_trampoline_is_ready_runs_the_harness_once_it_is() {
            for (trampoline, wiring) in [(SLOW_LINE, Wiring::Piped), (SLOW_STOP, Wiring::Terminal)]
            {
                let started = std::time::Instant::now();
                let (_dir, ran, mut running) = hold_marker(trampoline, wiring);
                assert!(
                    started.elapsed() < HOLD_SLEEP / 2,
                    "{wiring:?}: spawn waited {:?} for the trampoline",
                    started.elapsed()
                );
                assert!(running.is_held());
                running.release().unwrap();
                assert!(!running.is_held());
                assert!(
                    !ran.exists(),
                    "{wiring:?}: the harness ran before its trampoline was ready"
                );
                let status = ends(&mut running).await;
                assert!(status.success(), "{wiring:?}: {status}");
                assert!(ran.exists(), "{wiring:?}: the released harness never ran");
            }
        }

        /// Item 4 of the A/B lab: the trampoline runs in dash where the
        /// system has it, which starts in 1.9 ms here where `/bin/sh`, on
        /// macOS a shim that starts bash, takes 3.4 ms.
        #[tokio::test]
        async fn the_trampoline_runs_in_dash_where_the_system_has_it() {
            if !Path::new("/bin/dash").exists() {
                return;
            }
            let (_dir, _ran, mut running) = hold_marker(Trampoline::standard(), Wiring::Piped);
            let ps = std::process::Command::new("/bin/ps")
                .args(["-o", "comm=", "-p", &running.pid().to_string()])
                .output()
                .unwrap();
            let comm = String::from_utf8_lossy(&ps.stdout).trim().to_owned();
            kill_held_group(running.pid());
            ends(&mut running).await;
            assert!(comm.ends_with("dash"), "the trampoline ran in {comm:?}");
        }

        /// A release once the trampoline is ready lets the harness run, and
        /// nothing runs before it.
        #[tokio::test]
        async fn a_release_after_the_trampoline_is_ready_runs_the_harness() {
            for wiring in [Wiring::Piped, Wiring::Terminal] {
                let (_dir, ran, mut running) = hold_marker(Trampoline::standard(), wiring);
                running.hold_settled().await;
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert!(
                    !ran.exists(),
                    "{wiring:?}: the harness ran before it was released"
                );
                running.release().unwrap();
                let status = ends(&mut running).await;
                assert!(status.success(), "{wiring:?}: {status}");
                assert!(ran.exists(), "{wiring:?}");
            }
        }

        /// A caller that dies before it releases the harness closes the pipe
        /// the trampoline reads, which then exits without its exec: no
        /// `SIGHUP` of an orphaned group is needed, and no stopped shell is
        /// left behind.
        #[tokio::test]
        async fn a_line_hold_whose_caller_is_gone_exits_without_running_the_harness() {
            let dir = tempfile::tempdir().unwrap();
            let (ran, mut spawned) =
                spawn_marker(Trampoline::standard(), Wiring::Piped, dir.path());
            let HoldKind::Line(pipe) = &mut spawned.hold.kind else {
                panic!("a headless trampoline is held by a line");
            };
            drop(pipe.take());
            let status = tokio::time::timeout(ENDS_WITHIN, spawned.child.wait())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(status.code(), Some(1), "{status}");
            assert!(!ran.exists(), "the harness ran with no release");
        }

        /// A trampoline that ends before it is ready never runs the harness,
        /// and a release once its end is known fails, whichever hold it has.
        #[tokio::test]
        async fn a_hold_that_ends_before_it_is_ready_never_runs_the_harness() {
            for wiring in [Wiring::Piped, Wiring::Terminal] {
                // Released at once: the end may not be known yet, and the
                // release is then kept, or written to a pipe nobody reads.
                let (_dir, ran, mut early) = hold_marker(ENDS, wiring);
                let _ = early.release();
                let status = ends(&mut early).await;
                assert!(!status.success(), "{wiring:?}: {status}");
                assert!(
                    !ran.exists(),
                    "{wiring:?}: the harness ran after its hold ended"
                );

                let (_dir, ran, mut late) = hold_marker(ENDS, wiring);
                late.hold_settled().await;
                let status = ends(&mut late).await;
                assert!(!status.success(), "{wiring:?}: {status}");
                let err = late.release().unwrap_err();
                assert!(
                    matches!(err, HarnessError::Spawn { .. }),
                    "{wiring:?}: {err}"
                );
                assert!(late.is_held(), "{wiring:?}: a failed release still holds");
                assert!(!ran.exists(), "{wiring:?}");
            }
        }

        /// A trampoline that does not stop within its deadline has its whole
        /// group killed, the `sleep` it started included, and its harness
        /// never runs, though it was released.
        #[tokio::test]
        async fn a_late_stop_kills_the_group() {
            let late = Trampoline {
                shell: "/bin/sh",
                script: Some("sleep 30; kill -STOP $$ && exec \"$@\""),
                timeout: Duration::from_millis(300),
            };
            let (_dir, ran, mut running) = hold_marker(late, Wiring::Terminal);
            let pid = running.pid();
            running.release().unwrap();
            let status = ends(&mut running).await;
            assert!(!status.success(), "{status}");
            assert!(
                group_ends(pid).await,
                "the late hold's group was left running"
            );
            assert!(!ran.exists());
            let err = running.release().unwrap_err();
            assert!(err.to_string().contains("did not stop within"), "{err}");
        }

        /// A handle dropped before its release kills the group even while the
        /// trampoline is not ready yet, so the harness never runs.
        #[tokio::test]
        async fn dropping_a_handle_before_the_trampoline_is_ready_kills_the_group() {
            for (trampoline, wiring) in [(SLOW_LINE, Wiring::Piped), (SLOW_STOP, Wiring::Terminal)]
            {
                let (_dir, ran, running) = hold_marker(trampoline, wiring);
                let pid = running.pid();
                drop(running);
                assert!(
                    group_ends(pid).await,
                    "{wiring:?}: the dropped hold's group was left running"
                );
                tokio::time::sleep(HOLD_SLEEP + Duration::from_millis(500)).await;
                assert!(!ran.exists(), "{wiring:?}");
            }
        }

        /// A handle whose trampoline its caller has reaped already, as the
        /// CLI does when it abandons a launch, signals no group when it is
        /// dropped: the trampoline led the group alone, since nothing of the
        /// harness ran, so the group is gone with it, and its id may name
        /// another process's group by then.
        #[tokio::test]
        async fn dropping_a_handle_whose_trampoline_was_reaped_signals_no_group() {
            for (trampoline, wiring) in [(SLOW_LINE, Wiring::Piped), (SLOW_STOP, Wiring::Terminal)]
            {
                let (_dir, ran, mut running) = hold_marker(trampoline, wiring);
                let pid = running.pid();
                let leader =
                    rustix::process::Pid::from_raw(i32::try_from(pid.get()).unwrap()).unwrap();
                rustix::process::kill_process(leader, rustix::process::Signal::KILL).unwrap();
                ends(&mut running).await;
                let ((), killed) = killed::during(|| drop(running));
                assert_eq!(
                    killed,
                    Vec::<NonZeroU32>::new(),
                    "{wiring:?}: a reaped trampoline's group id was signalled"
                );
                assert!(!ran.exists(), "{wiring:?}");
            }
        }

        /// XNU wakes a parent waiting for a stop before the stopping task is
        /// suspended, and a `SIGCONT` sent in between is lost: the trampoline
        /// then hangs inside `kill` for good. Released at once, 16 of 960
        /// trampolines hung that way before the stop was let settle. Many
        /// early releases at once, as busy tests make them, must all run.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn early_releases_of_stopping_trampolines_never_hang() {
            let dir = tempfile::tempdir().unwrap();
            let mut tasks = Vec::new();
            for task in 0..8 {
                let dir = dir.path().join(format!("t{task}"));
                std::fs::create_dir_all(&dir).unwrap();
                tasks.push(tokio::spawn(async move {
                    for round in 0..25 {
                        let at = dir.join(format!("r{round}"));
                        std::fs::create_dir_all(&at).unwrap();
                        let (ran, spawned) =
                            spawn_marker(Trampoline::standard(), Wiring::Terminal, &at);
                        let mut running = crate::harness::Running::new(None, spawned);
                        running.release().unwrap();
                        let waited = tokio::time::timeout(
                            Duration::from_secs(5),
                            running.child_mut().wait(),
                        )
                        .await;
                        let Ok(status) = waited else {
                            // Not left holding the test's output open.
                            kill_held_group(running.pid());
                            panic!("an early release hung: {running:?}");
                        };
                        let status = status.unwrap();
                        assert!(status.success(), "{status}");
                        assert!(ran.exists());
                    }
                }));
            }
            for task in tasks {
                task.await.unwrap();
            }
        }

        /// A driver that speaks to the harness over its pipes is let go only
        /// once the harness may run: at the release for a line hold, which
        /// writes the line before anything of the driver's, and once the
        /// group is continued for a stop hold, not at a release that came
        /// before the stop.
        #[cfg(feature = "acp")]
        #[tokio::test]
        async fn the_driver_gate_opens_once_the_harness_may_run() {
            let (_dir, _ran, running) = hold_marker(SLOW_STOP, Wiring::Terminal);
            let (gate, mut opened) = tokio::sync::oneshot::channel();
            let (_events, feed) = crate::harness::TurnFeed::channel();
            let mut running = running.with_driver(gate, feed);
            running.release().unwrap();
            assert_eq!(
                opened.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty),
                "the gate opened before the trampoline stopped"
            );
            tokio::time::timeout(ENDS_WITHIN, &mut opened)
                .await
                .unwrap()
                .unwrap();
            assert!(ends(&mut running).await.success());

            let (_dir, _ran, running) = hold_marker(SLOW_LINE, Wiring::Rpc);
            let (gate, mut opened) = tokio::sync::oneshot::channel();
            let (_events, feed) = crate::harness::TurnFeed::channel();
            let mut running = running.with_driver(gate, feed);
            assert_eq!(
                opened.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty),
                "the gate opened before the release"
            );
            running.release().unwrap();
            assert_eq!(opened.try_recv(), Ok(()));
            // No client writes the line here; the group is not left behind.
            kill_held_group(running.pid());
            ends(&mut running).await;
        }
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
