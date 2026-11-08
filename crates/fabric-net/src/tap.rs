//! A capture of the link's plaintext for the end-to-end test.
//!
//! Invariant: without the `tap` feature `record` does nothing, so no node
//! that ships writes a link frame anywhere. With it, a node started with
//! `FABRIC_NET_TAP=<file>` appends to that file, before TLS, the payload of
//! every frame it writes and the decompressed bytes of every chunk it
//! takes, so a test can search everything that crossed the link in the
//! clear. A file that cannot be opened stops nothing: the capture is empty
//! and the test that reads it fails on its own check that it saw a frame.

/// Appends `bytes` to the capture, when one is armed.
#[cfg(feature = "tap")]
pub(crate) fn record(bytes: &[u8]) {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};

    static TAP: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    let tap = TAP.get_or_init(|| {
        let path = std::env::var_os("FABRIC_NET_TAP")?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    });
    if let Some(file) = tap
        && let Ok(mut file) = file.lock()
    {
        // A capture is evidence for a test, never a reason to fail a write.
        let _ = file.write_all(bytes);
    }
}

/// Does nothing: the capture is compiled out.
#[cfg(not(feature = "tap"))]
#[inline]
pub(crate) fn record(bytes: &[u8]) {
    let _ = bytes;
}
