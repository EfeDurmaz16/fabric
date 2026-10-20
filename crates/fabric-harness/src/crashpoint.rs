//! Named crash points for the crash tests.
//!
//! Invariant: a crash point does nothing unless the test-only
//! `crashpoints` feature is built and `FABRIC_CRASHPOINT` names it, so no
//! release build can be stopped this way.

/// Kills the process with `SIGKILL` at a named step when
/// `FABRIC_CRASHPOINT` names it, as `<step>:<index>`, so no destructor,
/// flush or handler runs (ADR 0009). Built only with the `crashpoints`
/// feature, which the crash tests turn on, so they can stop a capture or
/// restore at an exact step instead of after a guessed delay. Should
/// `kill` fail, the process aborts, which the tests read as a failure.
#[cfg(feature = "crashpoints")]
pub(crate) fn crashpoint(step: &str, index: usize) {
    if std::env::var("FABRIC_CRASHPOINT").is_ok_and(|named| named == format!("{step}:{index}")) {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", &std::process::id().to_string()])
            .status();
        std::process::abort();
    }
}

#[cfg(not(feature = "crashpoints"))]
pub(crate) fn crashpoint(_step: &str, _index: usize) {}
