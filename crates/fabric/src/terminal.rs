//! The terminal `fabric job run` or `fabric job resume` was typed in, and
//! the harness's turn at it.
//!
//! Invariant: an interactive harness's process group is the terminal's
//! foreground group from before the harness runs until it exits, and this
//! process's group is the foreground group again before this process
//! prints anything after it. The harness starts held (`Running::is_held`),
//! so handing the terminal over before `Running::release` is enough for
//! the first half. For the second, this process is in a background group
//! when it takes the terminal back, and `tcsetpgrp` from a background
//! group stops the caller with `SIGTTOU` unless the caller blocks or
//! ignores it (POSIX `tcsetpgrp`), so the calling thread blocks `SIGTTOU`
//! for that one call. With no terminal on standard input nothing is
//! handed over, and nothing is taken back. A hand-over that fails is an
//! error, never a harness started without the terminal: started in a
//! background group, its first read of the terminal would stop it.

use std::io::IsTerminal;
use std::num::NonZeroU32;
use std::os::fd::AsFd;

use nix::sys::signal::{SigSet, SigmaskHow, Signal};
use rustix::process::Pid;

/// The terminal handed to a harness's process group; dropping it gives
/// the terminal back to this process's group.
#[derive(Debug)]
pub(crate) struct Handed<T: AsFd = std::io::Stdin> {
    terminal: T,
    own: Pid,
}

/// Makes the process group `group` the foreground group of the terminal
/// on standard input, so the harness it holds can read and set it.
///
/// Returns `None`, and hands nothing over, when standard input is not a
/// terminal. A hand-over that fails is an error, so the caller stops the
/// harness it holds instead of letting it start without the terminal.
/// Called from a background job, this process is stopped until the shell
/// brings it to the foreground, as any job that takes the terminal is.
pub(crate) fn hand_over(group: NonZeroU32) -> std::io::Result<Option<Handed>> {
    hand_over_on(std::io::stdin(), group)
}

/// `hand_over` for the terminal `terminal`.
fn hand_over_on<T: AsFd + IsTerminal>(
    terminal: T,
    group: NonZeroU32,
) -> std::io::Result<Option<Handed<T>>> {
    if !terminal.is_terminal() {
        return Ok(None);
    }
    let group = i32::try_from(group.get())
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| std::io::Error::other(format!("{group} is not a process group id")))?;
    rustix::termios::tcsetpgrp(&terminal, group)?;
    Ok(Some(Handed {
        terminal,
        own: rustix::process::getpgrp(),
    }))
}

impl<T: AsFd> Drop for Handed<T> {
    fn drop(&mut self) {
        let mut ttou = SigSet::empty();
        ttou.add(Signal::SIGTTOU);
        let before = match ttou.thread_swap_mask(SigmaskHow::SIG_BLOCK) {
            Ok(before) => before,
            Err(e) => {
                tracing::warn!(error = %e, "SIGTTOU could not be blocked; the terminal stays with the harness's group");
                return;
            }
        };
        if let Err(e) = rustix::termios::tcsetpgrp(&self.terminal, self.own) {
            tracing::warn!(error = %e, "the terminal could not be taken back from the harness's group");
        }
        if let Err(e) = before.thread_set_mask() {
            tracing::warn!(error = %e, "the signal mask could not be restored");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal this process does not control cannot be handed over, as
    /// one a hang-up took away cannot: `tcsetpgrp` fails, and the hand-over
    /// says so instead of returning as if there were no terminal, which
    /// would start the held harness in a background group, where its first
    /// read of the terminal stops it (Codex review, 2026-10-22).
    #[test]
    fn a_hand_over_that_fails_is_an_error() {
        use rustix::fs::{Mode, OFlags};
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let terminal = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY,
            Mode::empty(),
        )
        .unwrap();
        assert!(terminal.is_terminal());
        let own = rustix::process::getpgrp().as_raw_nonzero().get();
        let own = NonZeroU32::new(own.unsigned_abs()).unwrap();
        let handed = hand_over_on(terminal, own);
        assert!(handed.is_err(), "{handed:?}");
    }
}
