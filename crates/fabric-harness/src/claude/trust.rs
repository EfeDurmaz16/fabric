//! Workspace trust in Claude's config file, `~/.claude.json`, or
//! `~/.claude/.config.json` when that exists (`global_config`).
//!
//! Invariant: the file is read before anything else, and written only
//! when the key Claude reads for the workspace is not already `true` and
//! no `claude` process runs, because Claude rewrites the file itself and
//! keeps its own backups (ADR 0006, Trust on the target). A write is read,
//! modify, write to a temporary file and rename that keeps the file's
//! mode, done under an exclusive lock in `~/.fabric` so two Fabric
//! processes never lose each other's key, and only the one key
//! `projects["<key>"].hasTrustDialogAccepted` changes. A file that is a symlink is read through and never written,
//! because the rename would replace the link. A missing file is not
//! created and is not an error: Claude writes its first-run state there,
//! whose shape Fabric does not own, and a node logged in with
//! `ANTHROPIC_API_KEY` alone may never have run Claude. The outcome is
//! then left to Claude, which creates the file and asks.
//!
//! Claude 2.1.294 has two trust checks, read from its bundle on
//! 2026-10-20 and reproduced with a temporary home the same day. The
//! dialog check walks from the working directory up through its
//! ancestors to the first directory whose `projects` entry has the key
//! set, ending at the repository root inside a git repository; a trusted
//! ancestor therefore skips the dialog (seen on the owner's machine: a
//! new folder under the trusted home opened with no dialog). The project
//! settings check (`gg`) has no walk: it reads one key, the main checkout
//! of the git repository that holds the working directory, or the
//! working directory itself outside a repository, and it must be a
//! literal `true`. Only that check turns on the workspace's
//! `permissions.allow` rules, its `.mcp.json` servers, its additional
//! directories and its project hooks; without it Claude prints that it
//! ignores them. `project_key` computes that key as Claude does,
//! following a linked worktree's `.git` file to its main checkout, and
//! `read` reports `Exact` only for a literal `true` there, so Fabric
//! never reports as trusted a workspace whose project settings Claude
//! would drop. A directory the dialog walk reaches is reported apart, as
//! `Ancestor`.
//!
//! Claude spells every key, and its working directory, as the NFC form of
//! the real path (`normalize("NFC")` in the same bundle), while APFS hands
//! back a name in the form it was stored in. The root here is an
//! `AbsPath`, which is NFC by type (ADR 0004): a workspace whose physical
//! path is not NFC is refused before this module runs, and so is a main
//! checkout whose path a worktree's files spell in another form, so the
//! keys Fabric reads and writes are the ones Claude looks up.

use std::path::{Component, Path, PathBuf};

use fabric_core::path::AbsPath;
use serde_json::{Map, Value};

use crate::durable;
use crate::error::{HarnessError, Input, IoOp};

/// The field Claude reads to skip its trust dialog and to apply project
/// settings.
pub(crate) const TRUST_KEY: &str = "hasTrustDialogAccepted";

/// What `ClaudeCode::preseed_trust` did about a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "Inherited and LeftToClaude mean Claude drops the workspace's project settings, which the user should be told"]
pub enum TrustOutcome {
    /// The key Claude reads for the workspace was already `true`, so it
    /// skips its dialog and applies the project settings; nothing was
    /// written and no process was looked at.
    AlreadyTrusted {
        /// The key: the main checkout of the repository, or the workspace.
        key: PathBuf,
    },
    /// The trust key was written for `key`.
    Written {
        /// The key: the main checkout of the repository, or the workspace.
        key: PathBuf,
    },
    /// The key was not written, for `reason`, but `by`, a directory
    /// Claude's dialog walk reaches, is trusted: Claude skips its dialog
    /// and runs, and ignores the workspace's permission rules, `.mcp.json`
    /// servers, additional directories and project hooks. The caller
    /// tells the user; this crate does not.
    Inherited {
        /// The nearest trusted directory the dialog walk reaches.
        by: PathBuf,
        /// Why the key was not written.
        reason: LeftReason,
    },
    /// Nothing trusts the workspace and the file was left alone, for
    /// `reason`: an interactive Claude shows its own trust dialog, and a
    /// headless one runs without the workspace's settings. The caller
    /// tells the user; this crate does not.
    LeftToClaude {
        /// Why the key was not written.
        reason: LeftReason,
    },
}

/// Why `ClaudeCode::preseed_trust` did not write the trust key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftReason {
    /// These `claude` processes run, and Claude rewrites the file itself.
    ClaudeRunning {
        /// The running processes.
        pids: LivePids,
    },
    /// There is no config file yet, because Claude never ran on this node
    /// under this config directory.
    NoConfig {
        /// The file that is missing.
        path: PathBuf,
    },
    /// The config file is a symlink, as a dotfile manager makes it, and a
    /// rename over it would turn the link into a plain file.
    ConfigIsSymlink {
        /// The link.
        path: PathBuf,
    },
}

/// Process ids of running processes; never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePids {
    first: u32,
    rest: Vec<u32>,
}

impl LivePids {
    /// The ids in `pids`, or `None` when there are none.
    pub(crate) fn of(pids: Vec<u32>) -> Option<Self> {
        let mut pids = pids.into_iter();
        let first = pids.next()?;
        Some(LivePids {
            first,
            rest: pids.collect(),
        })
    }

    /// Every id, in the order they were found.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(self.first).chain(self.rest.iter().copied())
    }
}

fn malformed(config_json: &Path, reason: String) -> HarnessError {
    HarnessError::Malformed {
        what: Input::ConfigJson,
        path: config_json.to_path_buf(),
        reason,
    }
}

/// The config file Claude reads and writes: the legacy
/// `<config_dir>/.config.json` when it exists, otherwise `default`.
pub(crate) fn global_config(config_dir: &Path, default: PathBuf) -> PathBuf {
    let legacy = config_dir.join(".config.json");
    if legacy.exists() { legacy } else { default }
}

/// Reads and parses the config file; the top level must be an object.
fn read_value(config_json: &Path) -> Result<Value, HarnessError> {
    let bytes =
        std::fs::read(config_json).map_err(|e| HarnessError::io(IoOp::Read, config_json, e))?;
    parse(config_json, &bytes)
}

/// Parses the bytes of the config file; the top level must be an object.
fn parse(config_json: &Path, bytes: &[u8]) -> Result<Value, HarnessError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| malformed(config_json, e.to_string()))?;
    if !value.is_object() {
        return Err(malformed(
            config_json,
            "top level is not an object".to_owned(),
        ));
    }
    Ok(value)
}

/// True when `dir` holds a `.git` directory or file, as Claude tests for
/// the repository root.
fn is_git_root(dir: &Path) -> bool {
    std::fs::metadata(dir.join(".git")).is_ok_and(|m| m.is_dir() || m.is_file())
}

/// `path` with `.` and `..` resolved by text, as Node's `path.resolve`
/// does for an absolute path.
pub(super) fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The text of the regular file at `path`; none for a missing file, a
/// symlink or anything else, which Claude does not follow here either.
fn read_plain(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// The main checkout of the linked worktree whose root is `git_root`,
/// found as Claude 2.1.294 finds it: `.git` is a file whose `gitdir:`
/// names a directory under `<common>/worktrees/`, whose `commondir` names
/// the common directory, and whose `gitdir` file points back at this
/// `.git`. The main checkout is the parent of a common directory named
/// `.git`, or the common directory itself for a bare repository. None for
/// a main checkout, a submodule, or anything else that does not hold
/// together; Claude then uses `git_root` itself.
fn main_checkout(git_root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_root.join(".git")).ok()?;
    let named = text.trim().strip_prefix("gitdir:")?.trim();
    let gitdir = lexical(&git_root.join(named));
    let common = lexical(&gitdir.join(read_plain(&gitdir.join("commondir"))?.trim()));
    if gitdir.parent()? != common.join("worktrees") {
        return None;
    }
    let back = read_plain(&gitdir.join("gitdir"))?;
    let points_here = std::fs::canonicalize(gitdir.join(back.trim())).ok()?
        == std::fs::canonicalize(git_root).ok()?.join(".git");
    if !points_here {
        return None;
    }
    if common.file_name()? == ".git" {
        common.parent().map(Path::to_path_buf)
    } else {
        (!is_git_root(&common)).then_some(common)
    }
}

/// The key Claude reads to decide whether the project settings of the
/// workspace at `root` apply: the main checkout of the git repository
/// that holds `root`, or `root` outside a repository. A key that is not
/// NFC is refused, as a workspace path is, because Claude would look up
/// its NFC form.
pub(crate) fn project_key(root: &AbsPath) -> Result<AbsPath, HarnessError> {
    let Some(git_root) = Path::new(root.as_str())
        .ancestors()
        .find(|dir| is_git_root(dir))
    else {
        return Ok(root.clone());
    };
    let key = main_checkout(git_root).unwrap_or_else(|| git_root.to_path_buf());
    let text = key
        .to_str()
        .ok_or_else(|| HarnessError::NotUtf8 { path: key.clone() })?;
    AbsPath::parse(text).map_err(|e| HarnessError::Malformed {
        what: Input::WorkspacePath,
        path: key.clone(),
        reason: e.to_string(),
    })
}

/// What the config file says about the trust of a workspace.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// The project settings key is a literal `true`.
    Exact,
    /// The key is not, but this directory the dialog walk reaches is.
    Ancestor(PathBuf),
    /// Nothing trusts the workspace, or there is no config file.
    Nothing,
}

/// The directories whose trust Claude lets `root` inherit, nearest first:
/// `root` and its ancestors, up to and including the git repository root
/// when `root` is inside one.
fn inherits_from(root: &Path) -> Vec<&Path> {
    let mut dirs = Vec::new();
    for dir in root.ancestors() {
        dirs.push(dir);
        if is_git_root(dir) {
            break;
        }
    }
    dirs
}

/// What the config file at `config_json` says about the workspace at
/// `root` whose project settings key is `key`, read without writing
/// anything.
pub(crate) fn read(
    config_json: &Path,
    root: &AbsPath,
    key: &AbsPath,
) -> Result<Found, HarnessError> {
    let bytes = match std::fs::read(config_json) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Found::Nothing),
        Err(e) => return Err(HarnessError::io(IoOp::Read, config_json, e)),
    };
    let value = parse(config_json, &bytes)?;
    let projects = match value.get("projects") {
        None => return Ok(Found::Nothing),
        Some(Value::Object(projects)) => projects,
        Some(_) => {
            return Err(malformed(
                config_json,
                "projects is not an object".to_owned(),
            ));
        }
    };
    let trusts = |dir: &Path| {
        dir.to_str()
            .and_then(|key| projects.get(key))
            .and_then(|entry| entry.get(TRUST_KEY))
            == Some(&Value::Bool(true))
    };
    if trusts(Path::new(key.as_str())) {
        return Ok(Found::Exact);
    }
    Ok(inherits_from(Path::new(root.as_str()))
        .into_iter()
        .find(|dir| trusts(dir))
        .map_or(Found::Nothing, |dir| Found::Ancestor(dir.to_path_buf())))
}

/// Why the config file at `config_json` must not be written whoever
/// runs, if anything: a missing file is not created, and a symlink is
/// read through but never written through, as Claude's own writer
/// refuses to.
pub(crate) fn unwritable(config_json: &Path) -> Result<Option<LeftReason>, HarnessError> {
    let meta = match std::fs::symlink_metadata(config_json) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(LeftReason::NoConfig {
                path: config_json.to_path_buf(),
            }));
        }
        Err(e) => return Err(HarnessError::io(IoOp::Read, config_json, e)),
    };
    Ok(meta
        .file_type()
        .is_symlink()
        .then(|| LeftReason::ConfigIsSymlink {
            path: config_json.to_path_buf(),
        }))
}

/// `preseed_file` under an exclusive lock on the file `lock`, so two
/// Fabric processes pre-seeding trust for different workspaces never both
/// read the same config and lose one key to the other's rename.
pub(crate) fn preseed_locked(
    config_json: &Path,
    key: &AbsPath,
    lock: &Path,
) -> Result<bool, HarnessError> {
    if let Some(parent) = lock.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| HarnessError::io(IoOp::CreateDir, parent, e))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)
        .map_err(|e| HarnessError::io(IoOp::Lock, lock, e))?;
    file.lock()
        .map_err(|e| HarnessError::io(IoOp::Lock, lock, e))?;
    let changed = preseed_file(config_json, key);
    drop(file);
    changed
}

/// Sets the trust key `key` in the config file at `config_json`.
/// Returns true when the file changed.
pub(crate) fn preseed_file(config_json: &Path, key: &AbsPath) -> Result<bool, HarnessError> {
    let mut value = read_value(config_json)?;
    let Some(top) = value.as_object_mut() else {
        return Err(malformed(
            config_json,
            "top level is not an object".to_owned(),
        ));
    };
    let key_text = key.as_str();
    let projects = top
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else {
        return Err(malformed(
            config_json,
            "projects is not an object".to_owned(),
        ));
    };
    let entry = projects
        .entry(key_text)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(entry) = entry.as_object_mut() else {
        return Err(malformed(
            config_json,
            format!("projects[{key_text:?}] is not an object"),
        ));
    };
    if entry.get(TRUST_KEY) == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    entry.insert(TRUST_KEY.to_owned(), Value::Bool(true));
    let mut out =
        serde_json::to_vec_pretty(&value).map_err(|e| malformed(config_json, e.to_string()))?;
    out.push(b'\n');
    durable::write_atomic(config_json, &out)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_only_the_trust_key_keeps_the_mode_and_is_idempotent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        std::fs::write(
            &config,
            r#"{"oauthAccount":{"emailAddress":"x"},"projects":{"/other":{"allowedTools":[]}}}"#,
        )
        .unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(preseed_file(&config, &AbsPath::parse("/w").unwrap()).unwrap());
        assert_eq!(
            std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600,
            "the login file must stay private"
        );
        let value: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(value["projects"]["/w"][TRUST_KEY], Value::Bool(true));
        assert_eq!(
            value["projects"]["/other"]["allowedTools"],
            serde_json::json!([])
        );
        assert_eq!(value["oauthAccount"]["emailAddress"], "x");
        assert!(!preseed_file(&config, &AbsPath::parse("/w").unwrap()).unwrap());
    }

    #[test]
    fn concurrent_preseeds_keep_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        std::fs::write(&config, r#"{"projects":{}}"#).unwrap();
        let lock = dir.path().join(".fabric/harness/locks/claude-config.lock");
        let threads: Vec<_> = (0..16)
            .map(|n| {
                let (config, lock) = (config.clone(), lock.clone());
                std::thread::spawn(move || {
                    let key = AbsPath::parse(&format!("/w{n}")).unwrap();
                    preseed_locked(&config, &key, &lock).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let value: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        for n in 0..16 {
            assert_eq!(
                value["projects"][format!("/w{n}")][TRUST_KEY],
                Value::Bool(true),
                "/w{n} was lost"
            );
        }
    }
}
