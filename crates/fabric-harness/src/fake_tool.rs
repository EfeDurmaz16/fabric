//! Fake executables for this crate's tests, each run once when it is written.
//!
//! macOS checks a new executable the first time it runs, and that check
//! waits on the system's assessment: about 0.2 s here, and 13.8 s was
//! measured while six test binaries of this crate started at once. A test whose first run of a fake is a version probe can then
//! outlast `PROBE_TIMEOUT` and fail as "version probe timed out" with
//! nothing wrong in the code under test; a resume test that loses its
//! version that way may go on to find the machine's own `codex` running.
//! [`write`] pays the check when the fake is made, by running it once with
//! `FABRIC_TOOL_WARM` set. A fake whose body has effects puts
//! [`WARM_GUARD`] first, so that run ends before the body does anything.
//! The product's timeouts are unchanged.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// The first line of a fake's body that ends the run at install before the
/// body does anything.
pub(crate) const WARM_GUARD: &str = "[ -z \"$FABRIC_TOOL_WARM\" ] || exit 0\n";

/// Writes `contents` to `path`, makes it executable and runs it once.
///
/// A `/bin/sh` child writes the file, not this process: a file this process
/// held open for writing while another test's thread forked would be open
/// in that child too, and running it would fail with `ETXTBSY`. The run at
/// install ignores the fake's exit status and output, since a fake may fail
/// on purpose or name an interpreter that only its test's `PATH` finds; the
/// kernel has run the file either way, and that run is what pays the check.
pub(crate) fn write(path: &Path, contents: &[u8]) {
    write_unrun(path, contents);
    let ran = Command::new(path)
        .env("FABRIC_TOOL_WARM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    assert!(ran.is_ok(), "{} did not run once: {ran:?}", path.display());
}

/// Writes `contents` to `path` and makes it executable, as [`write`] does,
/// without the run: for a copy of a test binary, which run with no
/// arguments would run every test, this one too.
pub(crate) fn write_unrun(path: &Path, contents: &[u8]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake has run once when `write` returns, so a test's first run of
    /// it does not wait on macOS's check of a new executable, and the guard
    /// kept that run from the body.
    #[test]
    fn a_written_fake_has_run_once_and_the_guard_kept_its_body_from_running() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let tool = dir.path().join("tool");
        let script = format!(
            "#!/bin/sh\necho \"${{FABRIC_TOOL_WARM:-test}}\" >> '{0}'\n{WARM_GUARD}echo body >> '{0}'\n",
            log.display()
        );
        write(&tool, script.as_bytes());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap_or_default(),
            "1\n",
            "the fake did not run at install, or its body ran then"
        );
        assert!(Command::new(&tool).status().unwrap().success());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "1\ntest\nbody\n");
    }
}
