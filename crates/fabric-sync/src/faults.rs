//! Named fault points for the crash tests.
//!
//! Without the `faults` feature `hit` does nothing. With it, a process
//! started with `<NAME>_FAULT=<point>` kills itself with `SIGKILL` the
//! first time it reaches that point: no destructor runs and no buffer is
//! flushed. A value that names no point fails `check`, so a test never runs
//! with a fault it believes armed and is not.

/// Every point, in the order a pull reaches them.
#[cfg(feature = "faults")]
const POINTS: &[&str] = &[
    // Some objects of a snapshot are copied and the snapshot is not published.
    "copy.mid_objects",
    // The snapshot is copied and nothing of the new tree is written.
    "pull.before_restore",
    // The new tree is restored beside the old one and nothing is renamed.
    "pull.before_swap",
    // The old tree is renamed aside and the new one is not in place.
    "pull.mid_swap",
];

/// Refuses a fault the build does not know.
#[cfg_attr(
    not(feature = "faults"),
    expect(clippy::unnecessary_wraps, reason = "only the faults build can refuse")
)]
pub(crate) fn check() -> Result<(), String> {
    #[cfg(feature = "faults")]
    if let Some(armed) = armed()
        && !POINTS.contains(&armed.as_str())
    {
        return Err(format!("unknown fault point {armed}"));
    }
    Ok(())
}

#[cfg(feature = "faults")]
fn armed() -> Option<String> {
    std::env::var(crate::env_var("FAULT")).ok()
}

/// Kills the process when `point` is the armed fault.
#[cfg_attr(
    not(feature = "faults"),
    expect(
        unused_variables,
        reason = "the point names nothing without the feature"
    )
)]
pub(crate) fn hit(point: &'static str) {
    #[cfg(feature = "faults")]
    if armed().as_deref() == Some(point) {
        let _ =
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL);
        std::process::abort();
    }
}
