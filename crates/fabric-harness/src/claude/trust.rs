//! Workspace trust in `~/.claude.json`.
//!
//! Invariant: the file is read before anything else, and written only
//! when Claude would not already trust the workspace and no `claude`
//! process runs, because Claude rewrites the file itself and keeps its own
//! backups (ADR 0006, Trust on the target). A write is read, modify, write
//! to a temporary file and rename that keeps the file's mode, and only the
//! one key `projects["<root>"].hasTrustDialogAccepted` changes. A file
//! that is a symlink is read through and never written, because the rename
//! would replace the link. A missing
//! file is not created: it means Claude never ran on this node, so there
//! is no login to trust a workspace for either.
//!
//! Which directories count is Claude's own rule, read from the 2.1.294
//! bundle on 2026-10-20 and seen on the owner's machine the same day (a
//! new folder under the trusted home opened with no trust dialog): Claude
//! walks from the working directory up through its ancestors and stops at
//! the first directory whose `projects` entry has the key set to `true`,
//! but inside a git repository the walk ends at the repository root, so a
//! trusted directory above a checkout does not trust the checkout.
//! `trusted_by` follows that walk and reads only a literal `true`, so it
//! never reports trust that Claude would not find. Claude also checks the
//! main checkout of a git worktree; `trusted_by` does not, which at worst
//! writes a key Claude did not need.
//!
//! Claude spells every key, and its working directory, as the NFC form of
//! the real path (`normalize("NFC")` in the same bundle), while APFS hands
//! back a name in the form it was stored in. The root here is an
//! `AbsPath`, which is NFC by type (ADR 0004), and a workspace whose
//! physical path is not NFC is refused before this module runs, so the
//! keys Fabric reads and writes are the ones Claude looks up.

use std::path::{Path, PathBuf};

use fabric_core::path::AbsPath;
use serde_json::{Map, Value};

use crate::durable;
use crate::error::{HarnessError, Input, IoOp};

/// The key Claude reads to skip the trust dialog.
pub(crate) const TRUST_KEY: &str = "hasTrustDialogAccepted";

/// What `ClaudeCode::preseed_trust` did about a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "LeftToClaude means Claude may stop at its own trust dialog, which the user should be told"]
pub enum TrustOutcome {
    /// Claude already trusts the workspace through `by`, the workspace
    /// root or an ancestor; nothing was written and no process was looked at.
    AlreadyTrusted {
        /// The nearest directory whose entry trusts the workspace.
        by: PathBuf,
    },
    /// The trust key was written for the workspace root.
    Written,
    /// Nothing trusts the workspace and the file was left alone, for
    /// `reason`: an interactive Claude shows its own trust dialog, and a
    /// headless one runs without the workspace's settings. The caller
    /// tells the user; this crate does not.
    LeftToClaude {
        /// Why the key was not written.
        reason: LeftReason,
    },
}

/// Why `ClaudeCode::preseed_trust` left an untrusted workspace to Claude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftReason {
    /// These `claude` processes run, and Claude rewrites the file itself.
    ClaudeRunning {
        /// The running processes.
        pids: LivePids,
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

/// Reads and parses the config file; the top level must be an object.
fn read(config_json: &Path) -> Result<Value, HarnessError> {
    let bytes =
        std::fs::read(config_json).map_err(|e| HarnessError::io(IoOp::Read, config_json, e))?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|e| malformed(config_json, e.to_string()))?;
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

/// The nearest directory whose entry in the config file at `config_json`
/// trusts `root`, read without writing anything.
pub(crate) fn trusted_by(
    config_json: &Path,
    root: &AbsPath,
) -> Result<Option<PathBuf>, HarnessError> {
    let value = read(config_json)?;
    let projects = match value.get("projects") {
        None => return Ok(None),
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
    Ok(inherits_from(Path::new(root.as_str()))
        .into_iter()
        .find(|dir| trusts(dir))
        .map(Path::to_path_buf))
}

/// Why the config file at `config_json` must not be written whoever
/// runs, if anything: a symlink is read through but never written
/// through, as Claude's own writer refuses to.
pub(crate) fn unwritable(config_json: &Path) -> Result<Option<LeftReason>, HarnessError> {
    let meta = std::fs::symlink_metadata(config_json)
        .map_err(|e| HarnessError::io(IoOp::Read, config_json, e))?;
    Ok(meta
        .file_type()
        .is_symlink()
        .then(|| LeftReason::ConfigIsSymlink {
            path: config_json.to_path_buf(),
        }))
}

/// Sets the trust key for `root` in the config file at `config_json`.
/// Returns true when the file changed.
pub(crate) fn preseed_file(config_json: &Path, root: &AbsPath) -> Result<bool, HarnessError> {
    let mut value = read(config_json)?;
    let Some(top) = value.as_object_mut() else {
        return Err(malformed(
            config_json,
            "top level is not an object".to_owned(),
        ));
    };
    let root_key = root.as_str();
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
        .entry(root_key)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(entry) = entry.as_object_mut() else {
        return Err(malformed(
            config_json,
            format!("projects[{root_key:?}] is not an object"),
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
}
