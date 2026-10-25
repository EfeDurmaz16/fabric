//! How Fabric runs git (ADR 0004, How Fabric runs git).
//!
//! Invariant: every git process this crate starts is a `git` found in an
//! absolute `PATH` directory outside the repository it runs in, or the git
//! the macOS `/usr/bin/git` stub runs when that stub is the one found, so
//! neither a relative entry such as `.` nor an entry inside the workspace
//! can make capture or restore run a `git` the workspace holds. It runs with no prompt, no
//! network, no hooks, no fsmonitor and a fixed author, with the user's
//! global and system config unread, with no `GIT_*` variable inherited from
//! whoever started Fabric, and in the C locale so its messages can be
//! matched. Nothing the manifest carries can steer a git command: a restored
//! repository's config holds only the allowlisted keys.
//!
//! The source repository's own config is the user's and is honored there.
//! Its filters run during `git status` on capture, as they do for every
//! other git command the user runs. A path under a clean filter that config
//! sets up (git-crypt's) is refused before `git add -A`, because restore
//! writes blobs with no smudge step and the target would hold the filtered
//! bytes.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;

use fabric_core::git::GitNameError;

/// The `-c` settings every git command gets.
const CONFIG_FLAGS: &[&str] = &[
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

/// The first executable `git` on the `PATH` value `path_var` that lies
/// outside `repo`.
///
/// A relative or empty entry names a directory relative to the current
/// one, which is the repository for every command here, and an entry under
/// the repository, such as a `node_modules/.bin` a shell hook added, is
/// the workspace's too; both are skipped, as is a candidate that resolves
/// into the repository through a link. The setup steps look their
/// programs up the same way (`fabric-env`, `find_on_path`).
///
/// On macOS `/usr/bin/git` is a stub that asks `xcrun` for the developer
/// tools' git on every call, which costs more than the git command it
/// runs. When the one found is the stub, or a link that resolves to it,
/// the git it would run is run directly, provided that one is outside
/// `repo` too (`shim_target`).
fn program(path_var: Option<&OsStr>, repo: &Path) -> Option<PathBuf> {
    let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_owned());
    let inside =
        |path: &Path| std::fs::canonicalize(path).is_ok_and(|real| real.starts_with(&repo));
    let found = std::env::split_paths(path_var?)
        .filter(|dir| dir.is_absolute() && !dir.starts_with(&repo) && !inside(dir))
        .map(|dir| dir.join("git"))
        .find(|candidate| !inside(candidate) && executable(candidate))?;
    if cfg!(target_os = "macos")
        && std::fs::canonicalize(&found).is_ok_and(|real| real == Path::new(XCRUN_SHIM))
        && let Some(real) = shim_target()
        && !real.starts_with(&repo)
        && !inside(real)
    {
        return Some(real.to_owned());
    }
    Some(found)
}

/// True when `path` is a regular file someone may execute.
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The stub macOS installs at `/usr/bin/git`.
const XCRUN_SHIM: &str = "/usr/bin/git";

/// The git the macOS stub at `/usr/bin/git` runs, asked of `xcrun` once
/// per process; `None` when `xcrun` names none, and the stub then runs.
///
/// `xcrun` runs with this process's environment, which every git command
/// here inherits apart from the `GIT_*` variables, so it picks the
/// developer directory the stub would pick. The stub also sets `SDKROOT`,
/// `CPATH`, `LIBRARY_PATH` and `MANPATH` for the git it runs; nothing git
/// does here reads them, as hooks are off.
fn shim_target() -> Option<&'static Path> {
    static TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();
    TARGET
        .get_or_init(|| {
            let out = Command::new("/usr/bin/xcrun")
                .args(["--find", "git"])
                .current_dir("/")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            let text = String::from_utf8(out.stdout).ok()?;
            let path = PathBuf::from(text.trim_end_matches('\n'));
            (out.status.success() && path.is_absolute() && executable(&path)).then_some(path)
        })
        .as_deref()
}

/// The `-c` settings that make git flush every file it writes to the device.
const FSYNC_FLAGS: &[&str] = &["core.fsync=all", "core.fsyncMethod=fsync"];

/// A git runner bound to one repository.
#[derive(Debug, Clone)]
pub(crate) struct Git {
    repo: PathBuf,
    date: Option<String>,
    full_fsync: bool,
}

impl Git {
    /// Binds to the repository at `repo`.
    #[must_use]
    pub fn new(repo: &Path) -> Git {
        Git {
            repo: repo.to_owned(),
            date: None,
            full_fsync: false,
        }
    }

    /// Gives every commit and reflog entry git writes the date `unix_ms`; without it git reads the clock.
    #[must_use]
    pub fn with_date(mut self, unix_ms: u64) -> Git {
        self.date = Some(format!("{} +0000", unix_ms / 1000));
        self
    }

    /// Makes every git command flush the index, refs, packs and config it writes, for a power-loss-safe restore.
    #[must_use]
    pub fn with_full_fsync(mut self) -> Git {
        self.full_fsync = true;
        self
    }

    fn command(&self) -> Result<Command, GitError> {
        let program =
            program(std::env::var_os("PATH").as_deref(), &self.repo).ok_or_else(|| {
                GitError::Spawn {
                    source: io::Error::new(
                        io::ErrorKind::NotFound,
                        "no git in an absolute PATH directory outside the repository",
                    ),
                }
            })?;
        let mut cmd = Command::new(program);
        for flag in CONFIG_FLAGS {
            cmd.arg("-c").arg(flag);
        }
        if self.full_fsync {
            for flag in FSYNC_FLAGS {
                cmd.arg("-c").arg(flag);
            }
        }
        for (key, _) in std::env::vars_os() {
            if key.as_encoded_bytes().starts_with(b"GIT_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(&self.repo)
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", "fabric")
            .env("GIT_AUTHOR_EMAIL", "fabric@localhost")
            .env("GIT_COMMITTER_NAME", "fabric")
            .env("GIT_COMMITTER_EMAIL", "fabric@localhost")
            .stdin(Stdio::null());
        if let Some(date) = &self.date {
            cmd.env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        Ok(cmd)
    }

    /// Runs git and returns its output whatever the status.
    pub fn output(&self, args: &[&str], env: &[(&str, &OsStr)]) -> Result<Output, GitError> {
        let mut cmd = self.command()?;
        cmd.args(args);
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.output().map_err(|source| GitError::Spawn { source })
    }

    pub(crate) fn checked(args: &[&str], output: Output) -> Result<Vec<u8>, GitError> {
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
        let mut cmd = self.command()?;
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(|source| GitError::Spawn { source })?;
        let stdin = child.stdin.take();
        // Input is fed from its own thread while stdout drains here, so git
        // never blocks on a full stdout pipe while this side blocks on stdin.
        let output = thread::scope(|scope| {
            if let Some(mut stdin) = stdin {
                scope.spawn(move || {
                    // A closed pipe means git stopped reading, which the status reports.
                    let _ = stdin.write_all(input);
                });
            }
            child.wait_with_output()
        })
        .map_err(|source| GitError::Spawn { source })?;
        Self::checked(args, output)
    }

    /// Starts git with stdin and stdout piped, for streaming commands such as `cat-file --batch`.
    pub fn spawn_piped(&self, args: &[&str]) -> Result<Child, GitError> {
        let mut cmd = self.command()?;
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// The child half: git must not see what the parent put in the environment.
    #[test]
    fn inherited_git_variables_child() {
        let Ok(dir) = std::env::var("FABRIC_GIT_ENV_CHILD") else {
            return;
        };
        let git = Git::new(Path::new(&dir));
        let out = git
            .output(&["config", "--get", "fabric.injected"], &[])
            .unwrap();
        assert!(
            !out.status.success(),
            "git saw {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(
            git.run_text(&["rev-parse", "--is-inside-work-tree"])
                .unwrap(),
            "true"
        );
    }

    #[test]
    fn inherited_git_variables_do_not_reach_git() {
        let dir = tempfile::tempdir().unwrap();
        Git::new(dir.path()).run(&["init", "-q"]).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "git::tests::inherited_git_variables_child"])
            .env("FABRIC_GIT_ENV_CHILD", dir.path())
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "fabric.injected")
            .env("GIT_CONFIG_VALUE_0", "yes")
            .env("GIT_DIR", "/nonexistent")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// The child half: git must run from outside the repository whatever `PATH` says.
    #[test]
    fn a_git_in_the_workspace_child() {
        let Ok(dir) = std::env::var("FABRIC_GIT_PATH_CHILD") else {
            return;
        };
        let git = Git::new(Path::new(&dir));
        assert_eq!(
            git.run_text(&["rev-parse", "--is-inside-work-tree"])
                .unwrap(),
            "true"
        );
    }

    /// A daemon whose `PATH` holds `.`, or a directory inside a workspace,
    /// must not run a `git` the workspace holds when it captures or
    /// restores there: that file is the agent's to write.
    #[test]
    fn a_git_in_the_workspace_is_never_run() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        Git::new(repo).run(&["init", "-q"]).unwrap();
        let real = program(std::env::var_os("PATH").as_deref(), repo).unwrap();
        let planted = "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\nexit 1\n";
        fs::create_dir(repo.join("bin")).unwrap();
        for at in [repo.join("git"), repo.join("bin/git")] {
            fs::write(&at, planted).unwrap();
            fs::set_permissions(&at, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path =
            std::env::join_paths([Path::new("."), &repo.join("bin"), real.parent().unwrap()])
                .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "git::tests::a_git_in_the_workspace_child"])
            .env("FABRIC_GIT_PATH_CHILD", repo)
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!repo.join("ran").exists(), "the workspace's git ran");
        assert!(!repo.join("bin/ran").exists(), "the workspace's git ran");
    }

    /// The macOS stub at `/usr/bin/git` asks `xcrun` for the developer
    /// tools' git on every call, which cost a suspend about 6 ms per git
    /// command (issue #26); the git `xcrun` names runs instead, also when
    /// the `PATH` finds the stub through a link, as in the end-to-end
    /// fixture's `bin` (review of PR #29).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_git_behind_the_macos_stub_runs_in_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let linked = dir.path().join("bin");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir(&linked).unwrap();
        std::os::unix::fs::symlink(XCRUN_SHIM, linked.join("git")).unwrap();
        let named = Command::new("/usr/bin/xcrun")
            .args(["--find", "git"])
            .output()
            .unwrap();
        // Without developer tools only the stub can say so.
        let expected = if named.status.success() {
            PathBuf::from(String::from_utf8(named.stdout).unwrap().trim_end())
        } else {
            PathBuf::from(XCRUN_SHIM)
        };
        for path in [
            std::ffi::OsString::from("/usr/bin:/bin"),
            std::env::join_paths([linked.as_path(), Path::new("/usr/bin")]).unwrap(),
        ] {
            assert_eq!(
                program(Some(&path), &repo),
                Some(expected.clone()),
                "{path:?}"
            );
        }
    }

    #[test]
    fn input_and_output_larger_than_a_pipe_do_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let name = "p".repeat(200);
        fs::write(dir.path().join(&name), "x").unwrap();
        let git = Git::new(dir.path());
        git.run(&["init", "-q"]).unwrap();
        let lines = 20_000;
        let input = format!("{name}\n").repeat(lines);

        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let outcome =
                git.run_stdin_env(&["hash-object", "--stdin-paths"], &[], input.as_bytes());
            let _ = done.send(outcome);
        });
        let out = finished
            .recv_timeout(Duration::from_mins(1))
            .expect("git hung on a full pipe")
            .unwrap();
        assert_eq!(
            out.split(|b| *b == b'\n').filter(|l| !l.is_empty()).count(),
            lines
        );
    }
}
