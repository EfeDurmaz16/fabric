//! How Fabric runs git (ADR 0004, How Fabric runs git).
//!
//! Invariant: every git process this crate starts runs with the repository
//! config, the user's global config and the system config unable to inject a
//! command into it, with no prompt and no network, and with a fixed author.
//! Nothing the manifest carries and nothing in the workspace can steer a git
//! command that Fabric runs.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use fabric_core::git::GitNameError;

/// The `-c` settings every git command gets.
pub const CONFIG_FLAGS: &[&str] = &[
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "core.sshCommand=/usr/bin/false",
    "credential.helper=",
    "protocol.allow=never",
    "protocol.file.allow=always",
];

/// Why a git command did not give an answer.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The git binary could not be started.
    #[error("cannot run git: {source}")]
    Spawn {
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// git exited with a failure status.
    #[error("git {command} failed ({code:?}): {stderr}")]
    Failed {
        /// The arguments after `git`.
        command: String,
        /// The exit code, when there was one.
        code: Option<i32>,
        /// What git wrote to stderr.
        stderr: String,
    },
    /// git's output was not UTF-8 where text was expected.
    #[error("git {command} wrote bytes that are not UTF-8")]
    Utf8 {
        /// The arguments after `git`.
        command: String,
    },
    /// git's output did not have the expected shape.
    #[error("cannot parse {what} from git output {text:?}")]
    Parse {
        /// What was being read.
        what: &'static str,
        /// The text that did not parse.
        text: String,
    },
    /// A name in git's output breaks the rule of its type.
    #[error(transparent)]
    Name(#[from] GitNameError),
}

/// The `-c` settings that make git flush every file it writes to the device.
const FSYNC_FLAGS: &[&str] = &["core.fsync=all", "core.fsyncMethod=fsync"];

/// A git runner bound to one repository.
#[derive(Debug, Clone)]
pub struct Git {
    repo: PathBuf,
    date: String,
    full_fsync: bool,
}

impl Git {
    /// Binds to the repository at `repo`; snapshot commits get `created_at_ms` as their date.
    #[must_use]
    pub fn new(repo: &Path, created_at_ms: u64) -> Git {
        Git {
            repo: repo.to_owned(),
            date: format!("{} +0000", created_at_ms / 1000),
            full_fsync: false,
        }
    }

    /// Makes every git command flush the index, refs, packs and config it writes, for a power-loss-safe restore.
    #[must_use]
    pub fn with_full_fsync(mut self) -> Git {
        self.full_fsync = true;
        self
    }

    /// The repository the runner is bound to.
    #[must_use]
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        for flag in CONFIG_FLAGS {
            cmd.arg("-c").arg(flag);
        }
        if self.full_fsync {
            for flag in FSYNC_FLAGS {
                cmd.arg("-c").arg(flag);
            }
        }
        cmd.current_dir(&self.repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", "fabric")
            .env("GIT_AUTHOR_EMAIL", "fabric@localhost")
            .env("GIT_AUTHOR_DATE", &self.date)
            .env("GIT_COMMITTER_NAME", "fabric")
            .env("GIT_COMMITTER_EMAIL", "fabric@localhost")
            .env("GIT_COMMITTER_DATE", &self.date)
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null());
        cmd
    }

    /// Runs git and returns its output whatever the status.
    pub fn output(&self, args: &[&str], env: &[(&str, &OsStr)]) -> Result<Output, GitError> {
        let mut cmd = self.command();
        cmd.args(args);
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.output().map_err(|source| GitError::Spawn { source })
    }

    fn checked(args: &[&str], output: Output) -> Result<Vec<u8>, GitError> {
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(GitError::Failed {
                command: args.join(" "),
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    /// Runs git with extra environment and returns stdout, failing on a nonzero status.
    pub fn run_env(&self, args: &[&str], env: &[(&str, &OsStr)]) -> Result<Vec<u8>, GitError> {
        Self::checked(args, self.output(args, env)?)
    }

    /// Runs git and returns stdout, failing on a nonzero status.
    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
        self.run_env(args, &[])
    }

    /// Runs git and returns stdout as trimmed text.
    pub fn run_text(&self, args: &[&str]) -> Result<String, GitError> {
        self.run_text_env(args, &[])
    }

    /// Runs git with extra environment and returns stdout as trimmed text.
    pub fn run_text_env(&self, args: &[&str], env: &[(&str, &OsStr)]) -> Result<String, GitError> {
        let bytes = self.run_env(args, env)?;
        String::from_utf8(bytes)
            .map(|s| s.trim().to_owned())
            .map_err(|_| GitError::Utf8 {
                command: args.join(" "),
            })
    }

    /// Runs git and reports only whether it succeeded.
    pub fn succeeds(&self, args: &[&str]) -> Result<bool, GitError> {
        Ok(self.output(args, &[])?.status.success())
    }

    /// Runs git with `input` on stdin and extra environment, and returns stdout, failing on a nonzero status.
    pub fn run_stdin_env(
        &self,
        args: &[&str],
        env: &[(&str, &OsStr)],
        input: &[u8],
    ) -> Result<Vec<u8>, GitError> {
        let mut cmd = self.command();
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(|source| GitError::Spawn { source })?;
        if let Some(mut stdin) = child.stdin.take() {
            // A closed pipe means git stopped reading, which the status reports.
            let _ = stdin.write_all(input);
        }
        let output = child
            .wait_with_output()
            .map_err(|source| GitError::Spawn { source })?;
        Self::checked(args, output)
    }

    /// Starts git with stdin and stdout piped, for streaming commands such as `cat-file --batch`.
    pub fn spawn_piped(&self, args: &[&str]) -> Result<Child, GitError> {
        let mut cmd = self.command();
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn().map_err(|source| GitError::Spawn { source })
    }

    /// Resolves a path inside the git directory, absolute, through `rev-parse --git-path`.
    pub fn git_path(&self, name: &str) -> Result<PathBuf, GitError> {
        let text = self.run_text(&["rev-parse", "--git-path", name])?;
        let path = PathBuf::from(text);
        Ok(if path.is_absolute() {
            path
        } else {
            self.repo.join(path)
        })
    }

    /// Splits NUL-terminated output into records.
    pub(crate) fn records(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
        bytes.split(|b| *b == 0).filter(|r| !r.is_empty())
    }

    /// Reads a record as text.
    pub(crate) fn text<'a>(what: &'static str, bytes: &'a [u8]) -> Result<&'a str, GitError> {
        std::str::from_utf8(bytes).map_err(|_| GitError::Parse {
            what,
            text: String::from_utf8_lossy(bytes).into_owned(),
        })
    }
}
