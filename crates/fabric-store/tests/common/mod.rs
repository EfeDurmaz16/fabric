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
use std::process::{Command, Stdio};

use fabric_core::id::ChunkId;

/// Writes `contents` to `path` with mode `0755` through `sh`, so this
/// process never holds an executable open for writing, which Linux would
/// refuse to run (`ETXTBSY`) in a child forked meanwhile.
pub fn write_executable(path: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()?;
    std::io::Write::write_all(
        &mut writer.stdin.take().ok_or("no pipe to the writer")?,
        contents.as_bytes(),
    )?;
    if !writer.wait()?.success() {
        return Err(format!("writing {} failed", path.display()).into());
    }
    Ok(())
}

/// How the logged `git` of `run_with_logged_git` behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggedGit {
    /// It runs the machine's git.
    Plain,
    /// It fails an `update-ref --stdin` whose input holds `symref-update`,
    /// as git before 2.46 does, and runs the machine's git otherwise.
    WithoutSymrefUpdate,
}

/// Runs the test `child` of this test binary in a new process whose `PATH`
/// starts with a `git` that appends the arguments of each call, one line
/// per call, to the file `FABRIC_GIT_LOG` names, and then runs the first
/// other `git` on the `PATH`. The child finds a scratch directory in
/// `FABRIC_GIT_COUNT_DIR` (`logged_git_dir`) and reads the calls with
/// `logged_git_calls`; it does nothing when that variable is unset, as in
/// an ordinary test run.
pub fn run_with_logged_git(child: &str, kind: LoggedGit) -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin)?;
    let path = std::env::var_os("PATH").ok_or("no PATH")?;
    let real = std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .ok_or("no git on the PATH")?;
    let refuse = match kind {
        LoggedGit::Plain => "",
        LoggedGit::WithoutSymrefUpdate => {
            r#"case " $* " in
*" update-ref "*" --stdin "*)
  input=$(cat)
  case $input in
  *symref-update*) echo "fatal: unknown command: symref-update" >&2; exit 128 ;;
  esac
  printf '%s\n' "$input" | exec "$real" "$@" ;;
esac
"#
        }
    };
    write_executable(
        &bin.join("git"),
        &format!(
            "#!/bin/sh\nreal='{}'\nprintf '%s\\n' \"$*\" >> \"$FABRIC_GIT_LOG\"\n{refuse}exec \"$real\" \"$@\"\n",
            real.display()
        ),
    )?;
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path)))?;
    let work = dir.path().join("work");
    std::fs::create_dir(&work)?;
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", child, "--nocapture"])
        .env("FABRIC_GIT_COUNT_DIR", &work)
        .env("FABRIC_GIT_LOG", dir.path().join("git.log"))
        .env("PATH", path)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() || !stdout.contains("1 passed") {
        return Err(format!("{child} failed:\n{stdout}\n{stderr}").into());
    }
    Ok(())
}

/// In a child of `run_with_logged_git`, its scratch directory; `None` in an ordinary run.
pub fn logged_git_dir() -> Option<PathBuf> {
    std::env::var_os("FABRIC_GIT_COUNT_DIR").map(PathBuf::from)
}

/// In a child of `run_with_logged_git`, the arguments of every git call so far, one entry per call.
pub fn logged_git_calls() -> Vec<String> {
    std::env::var_os("FABRIC_GIT_LOG")
        .and_then(|log| std::fs::read_to_string(log).ok())
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The calls of `calls` from index `from` on, each without the `-c`
/// settings in front of its git command, so it starts with the command.
pub fn git_commands(calls: &[String], from: usize) -> Vec<String> {
    calls[from..]
        .iter()
        .map(|call| {
            let mut words = call.split(' ');
            let mut kept: Vec<&str> = Vec::new();
            while let Some(word) = words.next() {
                if word == "-c" && kept.is_empty() {
                    words.next();
                } else {
                    kept.push(word);
                }
            }
            kept.join(" ")
        })
        .collect()
}

/// Returns where the store rooted at `store_root` keeps the object `id`: `objects/<first two hex digits>/<hex>`.
pub fn object_path(store_root: &Path, id: ChunkId) -> PathBuf {
    let hex = id.to_string();
    store_root.join("objects").join(&hex[..2]).join(hex)
}

/// Returns `len` bytes from a xorshift generator seeded with `seed`, the same on every run.
pub fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[3]
        })
        .collect()
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
