//! Fake executables for this crate's tests, each run once when it is written.
//!
//! A fake a test writes shares two things with every other test running at
//! the same time, in this process and in the other test binaries of a
//! workspace run, and neither goes through the environment a request sends:
//!
//! - macOS checks a new executable the first time it runs, and that check
//!   waits on the system's assessment, which every process on the machine
//!   queues on: about 0.2 s alone, and 13.8 s was measured while six test
//!   binaries started at once. A fake whose first run is a node's probe can
//!   then outlast `COMMAND_TIMEOUT`, and the probe's fact is left out as a
//!   tool that did not answer, with nothing wrong in the code under test.
//! - The file descriptors of this process: a fake this process held open
//!   for writing while another test's thread forked would be open in that
//!   child too, and Linux refuses to run it (`ETXTBSY`).
//!
//! [`write`] has a `/bin/sh` child write the fake, so this process never
//! holds it open, and pays the check by running it once with
//! `FABRIC_TOOL_WARM` set. A fake whose body has effects puts
//! [`WARM_GUARD`] first, so that run ends before the body does anything.
//! The product's timeouts are unchanged. `fabric-harness` keeps its own
//! copy for its tests.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// The first line of a fake's body that ends the run at install before the
/// body does anything.
pub(crate) const WARM_GUARD: &str = "[ -z \"$FABRIC_TOOL_WARM\" ] || exit 0\n";

/// Writes `contents` to `path`, makes it executable and runs it once.
///
/// The run at install ignores the fake's exit status and output, since a
/// fake may fail on purpose; the kernel has run the file either way, and
/// that run is what pays the check.
pub(crate) fn write(path: &Path, contents: &str) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    // The pipe closes at the end of the statement, so `cat` sees the end.
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
    let status = writer.wait().unwrap();
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
    let ran = Command::new(path)
        .env("FABRIC_TOOL_WARM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    assert!(ran.is_ok(), "{} did not run once: {ran:?}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{NodeFacts, Probes, Tool};
    use crate::tool_env::ToolEnv;

    /// The first run of a fake a test writes is the one at install, not the
    /// node's probe of it: the probe is bounded by `COMMAND_TIMEOUT`, and a
    /// first run that waited on the system's check of a new executable left
    /// `rustc` unmeasured in a workspace run, so a capture test read a
    /// `Rust` carry key as unmeasured. The guard keeps the body from the
    /// install run.
    #[tokio::test]
    async fn a_fakes_first_run_is_its_install_and_never_a_nodes_probe() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let script = format!(
            "#!/bin/sh\necho \"${{FABRIC_TOOL_WARM:-probe}}\" >> '{0}'\n{WARM_GUARD}echo rustc 1.0.0-fake\n",
            log.display()
        );
        write(&bin.join("rustc"), &script);
        let env = ToolEnv::from_vars(|name| (name == "PATH").then(|| bin.clone().into()));
        let home = dir.path().join("home");
        let facts = NodeFacts::measure(&home, &env, Probes::NONE.with(Tool::Rustc)).await;
        assert_eq!(
            facts.get(crate::facts::name::RUSTC),
            Some("rustc 1.0.0-fake")
        );
        let runs = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            runs.lines().next(),
            Some("1"),
            "a node's probe was the fake's first run: {runs:?}"
        );
        assert!(runs.lines().skip(1).all(|run| run == "probe"), "{runs:?}");
    }
}
