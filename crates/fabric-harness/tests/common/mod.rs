//! A fake `claude` for the Claude adapter's tests. It reports the version
//! the session registry was read on, stays alive when its first argument
//! is `hold` (so the running-process check sees it), and otherwise writes
//! its working directory, arguments and environment to `$FAKE_LOG`.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const SCRIPT: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
  hold) sleep 60; exit 0 ;;
esac
{ echo "cwd=$(pwd -P)"; for a in "$@"; do echo "arg=$a"; done; echo "home=$HOME"; echo "leak=${CARGO_MANIFEST_DIR:-unset}"; } > "$FAKE_LOG"
"#;

/// Writes the fake under `dir/bin` once; returns the script and a `PATH`
/// value that finds it.
pub fn fake_claude(dir: &Path) -> (PathBuf, OsString) {
    let bin = dir.join("bin");
    let script = bin.join("claude");
    if !script.exists() {
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(&script, SCRIPT).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (script, bin.into_os_string())
}
