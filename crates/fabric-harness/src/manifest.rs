//! The state manifest that travels with a captured harness state tree.
//!
//! Invariant: a manifest lists every file of the tree except itself by a
//! relative path under `home/` or `workspace/`, in strict order, and says
//! which of them hold path-bearing JSON text. The core stores the tree
//! without reading it (ADR 0006); the adapter that captured it is the only
//! reader, and it checks the kind before it trusts a field.

use std::path::Path;

use fabric_core::harness::HarnessKind;
use fabric_core::path::{AbsPath, RelPath};
use serde::{Deserialize, Serialize};

use crate::error::HarnessError;
use crate::session::SessionId;

/// The file name of the manifest inside a captured state tree.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The manifest format this crate writes.
pub const MANIFEST_FORMAT: u16 = 1;

/// The prefix of tree paths that restore under the user's home directory.
pub const HOME_PREFIX: &str = "home";

/// The prefix of tree paths that restore under the workspace root.
pub const WORKSPACE_PREFIX: &str = "workspace";

/// What one capture produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateManifest {
    /// The manifest format version.
    pub format: u16,
    /// The harness that captured the state.
    pub kind: HarnessKind,
    /// The session the state belongs to.
    pub session: SessionId,
    /// The physical workspace path at capture.
    pub root: AbsPath,
    /// The home directory at capture.
    pub home: AbsPath,
    /// The harness version at capture, when the binary was found.
    pub harness_version: Option<String>,
    /// Unix milliseconds at capture; informational.
    pub captured_at_ms: u64,
    /// Every file in the tree except this manifest; sorted by path.
    pub files: Vec<Carried>,
    /// How many matches each redaction rule removed, over the whole tree.
    pub redactions: Vec<Redaction>,
}

/// One file of the state tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carried {
    /// The path inside the tree: `home/...` or `workspace/...`.
    pub rel_path: RelPath,
    /// What restore does to the bytes.
    pub rewrite: Rewrite,
    /// Byte length of the staged copy.
    pub size: u64,
}

/// What restore does to a file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rewrite {
    /// Bytes are placed as they are.
    None,
    /// Plain text: secret shapes were redacted line by line at capture;
    /// placed as captured.
    Text,
    /// JSON text, one document or one per line: strings outside signed
    /// blocks have the old root, home and project key mapped to the new.
    JsonText,
}

/// How many matches one redaction rule removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redaction {
    /// The rule name.
    pub rule: String,
    /// The match count.
    pub count: u32,
}

impl StateManifest {
    /// Checks the manifest's own rules.
    pub fn validate(&self, path: &Path) -> Result<(), HarnessError> {
        let malformed = |reason: String| HarnessError::Malformed {
            what: "manifest",
            path: path.to_path_buf(),
            reason,
        };
        if self.format != MANIFEST_FORMAT {
            return Err(malformed(format!(
                "format {} is not {MANIFEST_FORMAT}",
                self.format
            )));
        }
        let mut previous: Option<&RelPath> = None;
        for (index, file) in self.files.iter().enumerate() {
            let text = file.rel_path.as_str();
            let under_prefix = text
                .strip_prefix(HOME_PREFIX)
                .or_else(|| text.strip_prefix(WORKSPACE_PREFIX))
                .is_some_and(|rest| rest.starts_with('/'));
            if !under_prefix {
                return Err(malformed(format!(
                    "file {index} ({text}) is not under {HOME_PREFIX}/ or {WORKSPACE_PREFIX}/"
                )));
            }
            if previous.is_some_and(|p| p >= &file.rel_path) {
                return Err(malformed(format!(
                    "file {index} ({text}) is not strictly after the one before it"
                )));
            }
            previous = Some(&file.rel_path);
        }
        Ok(())
    }

    /// Writes the manifest as `MANIFEST_FILE` under `out`.
    pub fn write(&self, out: &Path) -> Result<(), HarnessError> {
        let path = out.join(MANIFEST_FILE);
        self.validate(&path)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| HarnessError::Malformed {
            what: "manifest",
            path: path.clone(),
            reason: e.to_string(),
        })?;
        crate::files::write_atomic(&path, &bytes)
    }

    /// Reads and checks the manifest under `state`.
    pub fn read(state: &Path) -> Result<Self, HarnessError> {
        let path = state.join(MANIFEST_FILE);
        let bytes = std::fs::read(&path).map_err(|e| HarnessError::io("read", &path, e))?;
        let manifest: StateManifest =
            serde_json::from_slice(&bytes).map_err(|e| HarnessError::Malformed {
                what: "manifest",
                path: path.clone(),
                reason: e.to_string(),
            })?;
        manifest.validate(&path)?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(files: Vec<Carried>) -> StateManifest {
        StateManifest {
            format: MANIFEST_FORMAT,
            kind: HarnessKind::ClaudeCode,
            session: SessionId::parse("s1").unwrap(),
            root: AbsPath::parse("/w").unwrap(),
            home: AbsPath::parse("/h").unwrap(),
            harness_version: None,
            captured_at_ms: 0,
            files,
            redactions: vec![],
        }
    }

    fn carried(path: &str) -> Carried {
        Carried {
            rel_path: RelPath::parse(path).unwrap(),
            rewrite: Rewrite::None,
            size: 0,
        }
    }

    #[test]
    fn files_must_be_sorted_and_under_a_prefix() {
        let path = Path::new("m");
        assert!(
            manifest(vec![carried("home/a"), carried("workspace/b")])
                .validate(path)
                .is_ok()
        );
        assert!(
            manifest(vec![carried("workspace/b"), carried("home/a")])
                .validate(path)
                .is_err()
        );
        assert!(
            manifest(vec![carried("home/a"), carried("home/a")])
                .validate(path)
                .is_err()
        );
        assert!(
            manifest(vec![carried("homework/a")])
                .validate(path)
                .is_err()
        );
        assert!(manifest(vec![carried("other/a")]).validate(path).is_err());
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(vec![carried("home/.claude/projects/-w/s1.jsonl")]);
        m.write(dir.path()).unwrap();
        assert_eq!(StateManifest::read(dir.path()).unwrap(), m);
    }
}
