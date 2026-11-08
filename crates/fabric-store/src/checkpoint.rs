//! Named stop points for the crash tests.
//!
//! Invariant: outside the crate's own test build and the `faults` build
//! these functions do nothing, so no fault control ships in the library or
//! in a node. In those builds a process stops at the checkpoint named by
//! `FABRIC_STORE_CRASH_AT`, writes the file named by
//! `FABRIC_STORE_CRASH_SIGNAL`, and waits for the test to kill it with
//! `SIGKILL`: the crash tests here, and the node's end-to-end test of
//! `fabric gc`, which stops a restore with its staging directory written.

#[cfg(any(test, feature = "faults"))]
pub(crate) fn reached(name: &str) {
    use std::time::Duration;
    if std::env::var("FABRIC_STORE_CRASH_AT").is_ok_and(|at| at == name) {
        if let Ok(signal) = std::env::var("FABRIC_STORE_CRASH_SIGNAL") {
            let _ = std::fs::write(signal, name);
        }
        loop {
            std::thread::sleep(Duration::from_hours(1));
        }
    }
}

#[cfg(not(any(test, feature = "faults")))]
pub(crate) fn reached(_name: &str) {}
