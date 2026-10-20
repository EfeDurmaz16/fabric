//! The state manifest that travels with a captured harness state tree.
//!
//! Invariant: a manifest lists every file of the tree except itself by a
//! relative path under `home/`, in strict order, with the byte length and
//! `BLAKE3` digest of the staged copy and the content kind that decides how
//! the bytes were redacted at capture and how restore recognises a node's
//! own redacted copy. The core stores
//! the tree without reading it (ADR 0006); the adapter that captured it is
//! the only reader, and it checks the kind and every digest before it
//! places a byte.

use std::fmt;
use std::path::Path;

use fabric_core::harness::HarnessKind;
use fabric_core::path::{AbsPath, RelPath};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::HarnessError;
use crate::session::SessionId;

/// The file name of the manifest inside a captured state tree.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The manifest format this crate writes.
pub const MANIFEST_FORMAT: u16 = 1;

/// The prefix of tree paths that restore under the user's home directory.
pub const HOME_PREFIX: &str = "home";

/// The part of a tree path below `HOME_PREFIX`, which restore joins to
/// the home directory; `None` for a path outside it.
#[must_use]
pub fn under_home(rel: &str) -> Option<&str> {
    rel.strip_prefix(HOME_PREFIX)?.strip_prefix('/')
}

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
    /// The path inside the tree, under `home/`.
    pub rel_path: RelPath,
    /// What the bytes are.
    pub content: Content,
    /// How the harness changes the file, which decides what restore may do
    /// to a copy already on disk.
    pub mutability: Mutability,
    /// Byte length of the staged copy.
    pub size: u64,
    /// `BLAKE3` digest of the staged copy; restore refuses a file that does not match.
    pub blake3: Digest,
}

/// What a carried file's bytes are, which decides how capture redacts them
/// and how restore compares them with a file already on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Content {
    /// Bytes carried as they are; the adapter scanned them for secrets at capture.
    Opaque,
    /// Plain text: secret shapes are redacted line by line at capture.
    Text,
    /// One JSON value per line: strings outside signed blocks are redacted
    /// at capture, and a signed block that holds a secret is dropped.
    JsonLines,
    /// One JSON document: treated like `JsonLines`, as a whole.
    JsonDocument,
}

/// How a harness changes one of its files after writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mutability {
    /// Only ever grows at the end, such as a transcript or a rollout.
    AppendOnly,
    /// Never changes once written, such as a tool result or a file backup.
    Immutable,
    /// Rewritten in place, such as memory, a task file or sidecar metadata.
    Replaceable,
}

/// A `BLAKE3` digest, written as 64 hex characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Wraps a finished hash.
    #[must_use]
    pub fn from_hash(hash: blake3::Hash) -> Self {
        Digest(*hash.as_bytes())
    }

    /// Parses the 64-character hex form.
    pub fn parse(text: &str) -> Result<Self, String> {
        let hash = blake3::Hash::from_hex(text).map_err(|e| e.to_string())?;
        Ok(Self::from_hash(hash))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&blake3::Hash::from_bytes(self.0).to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({self})")
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
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
            if under_home(text).is_none() {
                return Err(malformed(format!(
                    "file {index} ({text}) is not under {HOME_PREFIX}/"
                )));
            }
            if text.split('/').any(crate::files::is_temp_name) {
                return Err(malformed(format!(
                    "file {index} ({text}) is a temporary file of this crate"
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
            content: Content::Opaque,
            mutability: Mutability::AppendOnly,
            size: 0,
            blake3: Digest::from_hash(blake3::hash(b"")),
        }
    }

    #[test]
    fn files_must_be_sorted_and_under_home() {
        let path = Path::new("m");
        assert!(
            manifest(vec![carried("home/a"), carried("home/b")])
                .validate(path)
                .is_ok()
        );
        assert!(
            manifest(vec![carried("home/b"), carried("home/a")])
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
        assert!(
            manifest(vec![carried("workspace/a")])
                .validate(path)
                .is_err()
        );
        assert!(
            manifest(vec![carried("home/.a.fabric-tmp-1-2")])
                .validate(path)
                .is_err()
        );
    }

    #[test]
    fn round_trips_through_the_file_and_digests_are_hex() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(vec![carried("home/.claude/projects/-w/s1.jsonl")]);
        m.write(dir.path()).unwrap();
        assert_eq!(StateManifest::read(dir.path()).unwrap(), m);
        let text = std::fs::read_to_string(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(text.contains(&blake3::hash(b"").to_hex().to_string()));
        assert!(serde_json::from_str::<Digest>("\"zz\"").is_err());
    }
}
