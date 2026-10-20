//! A git runner for the integration tests.
//!
//! It runs git the way the crate's own runner does: the user's global and
//! system config unread, no inherited `GIT_` variable, the C locale, and a
//! fixed author and date, so commits made in a test have the same ids on
//! every machine. Local file transport is allowed for submodules.

#![allow(
    dead_code,
    reason = "each test binary uses a different part of this module"
)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use fabric_core::id::ChunkId;

/// Returns where the store rooted at `store_root` keeps the object `id`: `objects/<first two hex digits>/<hex>`.
pub fn object_path(store_root: &Path, id: ChunkId) -> PathBuf {
    let hex = id.to_string();
    store_root.join("objects").join(&hex[..2]).join(hex)
}

/// A git runner bound to one repository.
pub struct TestGit {
    repo: PathBuf,
    date: String,
}

impl TestGit {
    /// Binds to `repo`; commits get `unix_ms` as their date.
    pub fn new(repo: &Path, unix_ms: u64) -> TestGit {
        TestGit {
            repo: repo.to_owned(),
            date: format!("{} +0000", unix_ms / 1000),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "core.hooksPath=/dev/null",
        ]);
        cmd.args(args);
        for (key, _) in std::env::vars_os() {
            if key.as_encoded_bytes().starts_with(b"GIT_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(&self.repo)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "fabric test")
            .env("GIT_AUTHOR_EMAIL", "test@localhost")
            .env("GIT_AUTHOR_DATE", &self.date)
            .env("GIT_COMMITTER_NAME", "fabric test")
            .env("GIT_COMMITTER_EMAIL", "test@localhost")
            .env("GIT_COMMITTER_DATE", &self.date);
        cmd
    }

    /// Runs git and returns stdout, failing with git's stderr on a nonzero status.
    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
        let out = self.command(args).output()?;
        if !out.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into());
        }
        Ok(out.stdout)
    }

    /// Runs git and returns stdout as trimmed text.
    pub fn run_text(&self, args: &[&str]) -> Result<String, Box<dyn Error>> {
        Ok(String::from_utf8(self.run(args)?)?.trim().to_owned())
    }

    /// Runs git and reports only whether it succeeded.
    pub fn succeeds(&self, args: &[&str]) -> Result<bool, Box<dyn Error>> {
        Ok(self.command(args).output()?.status.success())
    }

    /// Resolves a path inside the git directory.
    pub fn git_path(&self, name: &str) -> Result<PathBuf, Box<dyn Error>> {
        Ok(self
            .repo
            .join(self.run_text(&["rev-parse", "--git-path", name])?))
    }
}
