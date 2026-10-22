//! Writes a program a test then runs, so that running it cannot fail with
//! `ETXTBSY` ("Text file busy") on Linux.
//!
//! Linux refuses to run a file while any process holds it open for writing.
//! A file this process wrote itself was open here for a moment, and a child
//! that another test thread forked in that moment inherited the descriptor
//! and kept it until it ran its own program, so a run of the file right
//! after the write could fail. Writing a temporary file and renaming it
//! does not help: the inherited descriptor follows the file. Here a short
//! `sh` writes the file instead, so this process never opens it for
//! writing, and that `sh` has exited, closing its own descriptor, before
//! the write returns.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `contents` to `path` with mode `0755`.
pub fn write_executable(path: &Path, contents: &[u8]) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    // The pipe closes at the end of the statement, so `cat` sees the end.
    writer.stdin.take().unwrap().write_all(contents).unwrap();
    let status = writer.wait().unwrap();
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
}
