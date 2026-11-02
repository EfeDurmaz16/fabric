//! Named stop points for the crash tests.
//!
//! Invariant: outside the crate's own test build these functions do
//! nothing, so no fault control ships in the library. In the test build a
//! child process stops at the point named by `FABRIC_JOURNAL_CRASH_AT`,
//! writes the file named by `FABRIC_JOURNAL_CRASH_SIGNAL`, and waits for
//! the parent to kill it with `SIGKILL`.

#[cfg(test)]
pub(crate) fn reached(name: &str) {
    use std::time::Duration;
    if std::env::var("FABRIC_JOURNAL_CRASH_AT").is_ok_and(|at| at == name) {
        if let Ok(signal) = std::env::var("FABRIC_JOURNAL_CRASH_SIGNAL") {
            let _ = std::fs::write(signal, name);
        }
        loop {
            std::thread::sleep(Duration::from_hours(1));
        }
    }
}

#[cfg(not(test))]
pub(crate) fn reached(_name: &str) {}
