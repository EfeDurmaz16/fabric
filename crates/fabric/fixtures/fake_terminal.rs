//! A full-screen harness, as far as the end-to-end test's terminal tests
//! need one: installed on `PATH` as `codex`, it answers `--version` as
//! Codex 0.155.1, and otherwise, as a full-screen agent does when it
//! starts, it sets the terminal's modes and reads from it. It writes to
//! `$HOME/fake-terminal`, one fact per line: whether its process group
//! owned the terminal on its standard input at its first instruction,
//! what setting the modes and reading returned, whether it got a
//! `SIGCONT`, which only a stopped process is sent here, and its pid,
//! group and the foreground group at the end.

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

use rustix::process::{Signal, getpgrp, getpid};
use rustix::termios::{OptionalActions, tcgetattr, tcgetpgrp, tcsetattr};
use tokio::signal::unix::{SignalKind, signal};

/// The terminal's foreground group, or -1 when standard input is not a terminal.
fn foreground() -> i32 {
    tcgetpgrp(std::io::stdin()).map_or(-1, |group| group.as_raw_nonzero().get())
}

fn main() -> ExitCode {
    let at_start = foreground();
    if std::env::args().nth(1).as_deref() == Some("--version") {
        let _ = writeln!(std::io::stdout(), "codex-cli 0.155.1");
        return ExitCode::SUCCESS;
    }
    match run(at_start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "fake terminal harness: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(at_start: i32) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut log = Vec::new();
    let group = getpgrp().as_raw_nonzero().get();
    writeln!(log, "foreground at start {}", at_start == group)?;
    let mut cont =
        runtime.block_on(async { signal(SignalKind::from_raw(Signal::CONT.as_raw())) })?;
    let stdin = std::io::stdin();
    match tcgetattr(&stdin).and_then(|modes| tcsetattr(&stdin, OptionalActions::Now, &modes)) {
        Ok(()) => writeln!(log, "set modes")?,
        Err(e) => writeln!(log, "set modes failed: {e}")?,
    }
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(0) => writeln!(log, "read nothing")?,
        Ok(_) => writeln!(log, "read {}", line.trim_end())?,
        Err(e) => writeln!(log, "read failed: {e}")?,
    }
    let got_cont = runtime.block_on(async {
        tokio::time::timeout(Duration::from_millis(50), cont.recv())
            .await
            .is_ok()
    });
    if got_cont {
        writeln!(log, "got SIGCONT")?;
    }
    writeln!(
        log,
        "pid {} group {group} foreground {}",
        getpid().as_raw_nonzero().get(),
        foreground()
    )?;
    let home = std::env::var_os("HOME").ok_or_else(|| std::io::Error::other("no HOME"))?;
    std::fs::write(std::path::Path::new(&home).join("fake-terminal"), log)
}
