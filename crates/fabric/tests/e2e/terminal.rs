//! Gate finding F5: `fabric job run` of a Codex job in a terminal printed
//! that Codex runs, and Codex exited at once with "Interrupted system
//! call". The harness ran in a process group of its own before that group
//! owned the terminal, so its first change to the terminal's modes stopped
//! it with `SIGTTOU`, and the `SIGCONT` sent with the hand-over interrupted
//! the call it was in.
//!
//! These tests run `fabric job run` under a pseudo-terminal, as a session
//! leader whose controlling terminal it is, with a fake `codex` that sets
//! the terminal's modes and reads from it the moment it starts, and says
//! whether its group owned the terminal at its first instruction and
//! whether it was sent a `SIGCONT`. The session
//! leader is this test binary run again in a role, because making a
//! terminal the controlling one needs `setsid` in the process that then
//! starts `fabric`, and this workspace has no unsafe code for a `pre_exec`.

use std::fs;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::world::{Env, Fallible, Node, Wire, finish, text};

/// The role this test binary plays when run again by a test here.
const ROLE: &str = "FABRIC_E2E_TERMINAL_ROLE";
/// The `fabric` arguments the session leader runs, one per line.
const ARGS: &str = "FABRIC_E2E_TERMINAL_ARGS";
/// The file the session leader writes what it saw to.
const RESULT: &str = "FABRIC_E2E_TERMINAL_RESULT";

/// How long the whole run under the terminal may take.
const RUN_TIMEOUT: Duration = Duration::from_mins(3);

/// Installs the fake full-screen harness (`fixtures/fake_terminal.rs`) as
/// `codex`, first on the `PATH`.
fn install_terminal_codex(env: &Env) -> Fallible {
    let codex = env.root.join("bin").join("codex");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fabric-fake-terminal"), &codex)?;
    // macOS checks a binary the first time it runs, which can take longer
    // than the node takes to record a launch; run it once first.
    let warm = Command::new(&codex).arg("--version").output()?;
    if !warm.status.success() {
        return Err("the fake harness does not answer --version".into());
    }
    Ok(())
}

/// What the fake harness wrote: every line, and the pid, the process
/// group and the terminal's foreground group of its last line.
struct Seen {
    pid: i32,
    group: i32,
    foreground: i32,
    lines: Vec<String>,
}

impl Seen {
    fn read(env: &Env) -> Fallible<Self> {
        let log = fs::read_to_string(env.home.join("fake-terminal"))?;
        let last = log.lines().last().ok_or("the fake harness wrote nothing")?;
        let words: Vec<&str> = last.split_whitespace().collect();
        let ["pid", pid, "group", group, "foreground", foreground] = words[..] else {
            return Err(format!("the fake harness did not finish:\n{log}").into());
        };
        Ok(Self {
            pid: pid.parse()?,
            group: group.parse()?,
            foreground: foreground.parse()?,
            lines: log.lines().map(str::to_owned).collect(),
        })
    }

    /// What it saw and did, without the last line.
    fn steps(&self) -> &[String] {
        &self.lines[..self.lines.len() - 1]
    }
}

/// A started node with a Codex job whose harness is the fake above.
fn codex_job() -> Fallible<(std::sync::Arc<Env>, Node, String)> {
    let env = Env::new()?;
    install_terminal_codex(&env)?;
    let mut node = Node::new(&env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let job = crate::new_job(&env, &node, "codex")?;
    Ok((env, node, job))
}

/// A pseudo-terminal: the master this test holds, and the path of the slave.
fn open_pty() -> Fallible<(OwnedFd, std::ffi::CString)> {
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    grantpt(&master)?;
    unlockpt(&master)?;
    let slave = ptsname(&master, Vec::new())?;
    Ok((master, slave))
}

/// The session leader: makes its standard input, a pseudo-terminal's
/// slave, its controlling terminal, runs `fabric` in its own process
/// group, which is then the foreground group, and records whether that
/// group owns the terminal again once `fabric` has exited.
fn session_leader() -> Fallible {
    rustix::process::setsid()?;
    rustix::process::ioctl_tiocsctty(std::io::stdin())?;
    let args = std::env::var(ARGS)?;
    let status = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .args(args.lines())
        .status()?;
    let foreground = rustix::termios::tcgetpgrp(std::io::stdin())
        .map_or(-1, |group| group.as_raw_nonzero().get());
    fs::write(
        std::env::var(RESULT)?,
        format!(
            "exit {}\ngroup {}\nforeground {foreground}\n",
            status.code().unwrap_or(-1),
            rustix::process::getpgrp().as_raw_nonzero().get(),
        ),
    )?;
    Ok(())
}

/// Runs `fabric args...` on `node` as the leader of a new session whose
/// controlling terminal is a fresh pseudo-terminal, with `input` typed
/// into it, and returns what the leader recorded: `fabric`'s exit code,
/// the leader's process group, and the terminal's foreground group after
/// `fabric` exited.
fn run_in_terminal(env: &Env, node: &Node, args: &[&str], input: &[u8]) -> Fallible<[i32; 3]> {
    let (master, slave_path) = open_pty()?;
    let slave = rustix::fs::open(
        slave_path.as_c_str(),
        rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY,
        rustix::fs::Mode::empty(),
    )?;
    let result = env.root.join("terminal-result");
    let test = "terminal::e2e_a_terminal_harness_owns_the_terminal_before_it_starts";
    let mut leader = env.command(&std::env::current_exe()?.to_string_lossy(), node.state());
    leader
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(ROLE, "leader")
        .env(ARGS, args.join("\n"))
        .env(RESULT, &result)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    let leader = leader.spawn()?;
    rustix::io::write(&master, input)?;
    let mut reader = fs::File::from(master);
    let screen = std::thread::spawn(move || {
        let mut screen = Vec::new();
        let mut buf = [0u8; 4096];
        // The slave's last close ends the read with EOF or EIO.
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            screen.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&screen).into_owned()
    });
    let finished = finish(leader, RUN_TIMEOUT)?;
    let recorded = fs::read_to_string(&result).map_err(|e| {
        format!(
            "the session leader recorded nothing ({e}):\n{}\n--- daemon log ---\n{}",
            text(&finished),
            node.log()
        )
    })?;
    let screen = screen.join().map_err(|_| "the terminal reader panicked")?;
    let mut values = recorded
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(_, value)| value.parse::<i32>()));
    let mut next = || -> Fallible<i32> {
        Ok(values
            .next()
            .ok_or_else(|| format!("short record {recorded:?}; terminal:\n{screen}"))??)
    };
    Ok([next()?, next()?, next()?])
}

/// The harness's group owns the terminal from the harness's first
/// instruction, so setting the terminal's modes and reading from it
/// neither stop it nor interrupt it, and `fabric` owns the terminal again
/// once the harness has exited. The harness still leads a group of its
/// own, apart from `fabric`'s, so a fence kills exactly it.
#[test]
fn e2e_a_terminal_harness_owns_the_terminal_before_it_starts() -> Fallible {
    if std::env::var_os(ROLE).is_some() {
        return session_leader();
    }
    let (env, node, job) = codex_job()?;
    let [exit, leader, foreground] = run_in_terminal(
        &env,
        &node,
        &["job", "run", &job, "--prompt", "one"],
        b"hello\n",
    )?;
    let seen = Seen::read(&env)?;
    assert_eq!(exit, 0, "fabric job run failed: {:?}", seen.lines);
    assert_eq!(
        seen.group, seen.pid,
        "the harness leads a process group of its own"
    );
    assert_ne!(seen.group, leader, "the harness shares fabric's group");
    assert_eq!(
        seen.steps(),
        ["foreground at start true", "set modes", "read hello"],
        "the harness ran before its group owned the terminal, was stopped and continued, or could not use the terminal"
    );
    assert_eq!(seen.foreground, seen.group);
    assert_eq!(
        foreground, leader,
        "the terminal stayed with the harness's group after it exited"
    );
    Ok(())
}

/// With no terminal on standard input, nothing is held or handed over:
/// the harness starts at once in a group of its own, which never becomes
/// a terminal's foreground group, and the run ends.
#[test]
fn e2e_a_harness_without_a_terminal_starts_without_a_hand_over() -> Fallible {
    let (env, node, job) = codex_job()?;
    node.ok(&["job", "run", &job, "--prompt", "one"])?;
    let seen = Seen::read(&env)?;
    assert_eq!(
        seen.group, seen.pid,
        "the harness leads a process group of its own"
    );
    assert_ne!(
        seen.foreground, seen.group,
        "a terminal was handed to the harness"
    );
    assert_eq!(
        seen.steps()[0],
        "foreground at start false",
        "{:?}",
        seen.lines
    );
    assert!(
        seen.steps()[1].starts_with("set modes failed") && seen.steps()[2] == "read nothing",
        "the harness's standard input is not the null device: {:?}",
        seen.lines
    );
    assert!(
        !seen.lines.iter().any(|line| line == "got SIGCONT"),
        "{:?}",
        seen.lines
    );
    Ok(())
}
