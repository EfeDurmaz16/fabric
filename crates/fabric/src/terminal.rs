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
//! handed over, and nothing is taken back.

use std::io::IsTerminal;
use std::num::NonZeroU32;

use nix::sys::signal::{SigSet, SigmaskHow, Signal};
use rustix::process::Pid;

/// The terminal handed to a harness's process group; dropping it gives
/// the terminal back to this process's group.
#[derive(Debug)]
pub(crate) struct Handed {
    own: Pid,
}

/// Makes the process group `group` the foreground group of the terminal
/// on standard input, so the harness it holds can read and set it.
///
/// Returns `None`, and hands nothing over, when standard input is not a
/// terminal, and when the hand-over fails, which it logs: the harness
/// then runs as it would have without one. Called from a background job,
/// this process is stopped until the shell brings it to the foreground,
/// as any job that takes the terminal is.
pub(crate) fn hand_over(group: NonZeroU32) -> Option<Handed> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return None;
    }
    let group = Pid::from_raw(i32::try_from(group.get()).ok()?)?;
    if let Err(e) = rustix::termios::tcsetpgrp(&stdin, group) {
        tracing::warn!(error = %e, "the harness could not take the terminal");
        return None;
    }
    Some(Handed {
        own: rustix::process::getpgrp(),
    })
}

impl Drop for Handed {
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
        if let Err(e) = rustix::termios::tcsetpgrp(std::io::stdin(), self.own) {
            tracing::warn!(error = %e, "the terminal could not be taken back from the harness's group");
        }
        if let Err(e) = before.thread_set_mask() {
            tracing::warn!(error = %e, "the signal mask could not be restored");
        }
    }
}
