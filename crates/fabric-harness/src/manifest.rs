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

use crate::error::{HarnessError, Input, IoOp};
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

/// What one capture produced. Its fields are private: a manifest is
/// built by `StateManifest::new` or read from a file, and both check the
/// rules of this module, so an invalid one cannot be passed around.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Unchecked")]
pub struct StateManifest {
    format: u16,
    kind: HarnessKind,
    session: SessionId,
    root: AbsPath,
    home: AbsPath,
    harness_version: Option<String>,
    captured_at_ms: u64,
    files: Vec<Carried>,
    redactions: Vec<Redaction>,
}

/// A manifest as read, before its rules are checked.
#[derive(Deserialize)]
struct Unchecked {
    format: u16,
    kind: HarnessKind,
    session: SessionId,
    root: AbsPath,
    home: AbsPath,
    harness_version: Option<String>,
    captured_at_ms: u64,
    files: Vec<Carried>,
    redactions: Vec<Redaction>,
}

impl TryFrom<Unchecked> for StateManifest {
    type Error = ManifestError;

    fn try_from(raw: Unchecked) -> Result<Self, ManifestError> {
        let manifest = StateManifest {
            format: raw.format,
            kind: raw.kind,
            session: raw.session,
            root: raw.root,
            home: raw.home,
            harness_version: raw.harness_version,
            captured_at_ms: raw.captured_at_ms,
            files: raw.files,
            redactions: raw.redactions,
        };
        manifest.check()?;
        Ok(manifest)
    }
}

/// Why a manifest breaks the rules of this module.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// The format is not the one this crate writes.
    #[error("format {found} is not {MANIFEST_FORMAT}")]
    Format {
        /// The format found.
        found: u16,
    },
    /// A file is not under `home/`.
    #[error("file {index} ({path}) is not under {HOME_PREFIX}/")]
    NotUnderHome {
        /// Its position in the list.
        index: usize,
        /// Its tree path.
        path: String,
    },
    /// A file is a temporary file of this crate.
    #[error("file {index} ({path}) is a temporary file of this crate")]
    TemporaryFile {
        /// Its position in the list.
        index: usize,
        /// Its tree path.
        path: String,
    },
    /// A file is not strictly after the one before it.
    #[error("file {index} ({path}) is not strictly after the one before it")]
    NotSorted {
        /// Its position in the list.
        index: usize,
        /// Its tree path.
        path: String,
    },
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

/// Text that is not a 64-character hex digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a 64-character hex BLAKE3 digest")]
pub struct DigestError;

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
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let hash = blake3::Hash::from_hex(text).map_err(|_| DigestError)?;
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
    rule: RuleName,
    count: u32,
}

impl Redaction {
    /// The rule name, such as `github-token` or `signed-block-dropped`.
    #[must_use]
    pub fn rule(&self) -> &str {
        self.rule.0.as_str()
    }

    /// The match count.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// A redaction rule name: lower-case ASCII letters, digits and `-`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
struct RuleName(String);

impl TryFrom<String> for RuleName {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(RuleName(text))
        } else {
            Err(format!("{text:?} is not a redaction rule name"))
        }
    }
}

impl StateManifest {
    /// Builds the manifest of one capture, sorting `files` by path and
    /// checking the rules of this module.
    #[cfg(any(test, feature = "acp", feature = "claude", feature = "codex"))]
    pub(crate) fn new(
        kind: HarnessKind,
        session: SessionId,
        root: AbsPath,
        home: AbsPath,
        harness_version: Option<String>,
        mut files: Vec<Carried>,
        counts: &crate::redact::RedactionCounts,
    ) -> Result<Self, ManifestError> {
        files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let redactions = counts
            .iter()
            .map(|(rule, count)| Redaction {
                rule: RuleName((*rule).to_owned()),
                count: *count,
            })
            .collect();
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind,
            session,
            root,
            home,
            harness_version,
            captured_at_ms: crate::process::now_ms(),
            files,
            redactions,
        };
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), ManifestError> {
        if self.format != MANIFEST_FORMAT {
            return Err(ManifestError::Format { found: self.format });
        }
        let mut previous: Option<&RelPath> = None;
        for (index, file) in self.files.iter().enumerate() {
            let path = file.rel_path.as_str().to_owned();
            if under_home(&path).is_none() {
                return Err(ManifestError::NotUnderHome { index, path });
            }
            if path.split('/').any(crate::durable::is_temp_name) {
                return Err(ManifestError::TemporaryFile { index, path });
            }
            if previous.is_some_and(|p| p >= &file.rel_path) {
                return Err(ManifestError::NotSorted { index, path });
            }
            previous = Some(&file.rel_path);
        }
        Ok(())
    }

    /// The harness that captured the state.
    #[must_use]
    pub fn kind(&self) -> &HarnessKind {
        &self.kind
    }

    /// The session the state belongs to.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// The physical workspace path at capture.
    #[must_use]
    pub fn root(&self) -> &AbsPath {
        &self.root
    }

    /// The home directory at capture.
    #[must_use]
    pub fn home(&self) -> &AbsPath {
        &self.home
    }

    /// The harness version at capture: the one installed on the capturing
    /// node, when the binary was found (for Codex, asked in the workspace,
    /// as a launch there would run it), or for Codex the known writer when
    /// that is newer (`Codex::with_known_writer`): the one the session
    /// arrived with, raised to the version each resume on the node started
    /// with, so it is at least the newest Codex that ran the session under
    /// Fabric.
    #[must_use]
    pub fn harness_version(&self) -> Option<&str> {
        self.harness_version.as_deref()
    }

    /// Unix milliseconds at capture; informational.
    #[must_use]
    pub fn captured_at_ms(&self) -> u64 {
        self.captured_at_ms
    }

    /// Every file in the tree except this manifest, sorted by path.
    #[must_use]
    pub fn files(&self) -> &[Carried] {
        &self.files
    }

    /// How many matches each redaction rule removed, over the whole tree.
    #[must_use]
    pub fn redactions(&self) -> &[Redaction] {
        &self.redactions
    }

    /// The `BLAKE3` digest of the manifest's JSON form, which names one
    /// capture: equal manifests, read back or kept in memory, share it.
    #[cfg(any(feature = "acp", feature = "claude", feature = "codex"))]
    pub(crate) fn digest(&self) -> Result<Digest, HarnessError> {
        let bytes = serde_json::to_vec(self).map_err(|e| HarnessError::Malformed {
            what: Input::Manifest,
            path: std::path::PathBuf::from(MANIFEST_FILE),
            reason: e.to_string(),
        })?;
        Ok(Digest::from_hash(blake3::hash(&bytes)))
    }

    /// Writes the manifest as `MANIFEST_FILE` under `out`.
    pub fn write(&self, out: &Path) -> Result<(), HarnessError> {
        let path = out.join(MANIFEST_FILE);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| HarnessError::Malformed {
            what: Input::Manifest,
            path: path.clone(),
            reason: e.to_string(),
        })?;
        crate::durable::write_atomic(&path, &bytes)
    }

    /// Reads the manifest under `state`, checking its rules.
    pub fn read(state: &Path) -> Result<Self, HarnessError> {
        let path = state.join(MANIFEST_FILE);
        let bytes = std::fs::read(&path).map_err(|e| HarnessError::io(IoOp::Read, &path, e))?;
        serde_json::from_slice(&bytes).map_err(|e| HarnessError::Malformed {
            what: Input::Manifest,
            path,
            reason: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(files: Vec<Carried>) -> Result<StateManifest, ManifestError> {
        let mut counts = crate::redact::RedactionCounts::new();
        counts.insert("github-token", 2);
        StateManifest::new(
            HarnessKind::ClaudeCode,
            SessionId::parse("s1").unwrap(),
            AbsPath::parse("/w").unwrap(),
            AbsPath::parse("/h").unwrap(),
            None,
            files,
            &counts,
        )
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
        let ok = manifest(vec![carried("home/b"), carried("home/a")]).unwrap();
        let paths: Vec<&str> = ok.files().iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["home/a", "home/b"], "new sorts the files");
        assert!(matches!(
            manifest(vec![carried("home/a"), carried("home/a")]),
            Err(ManifestError::NotSorted { index: 1, .. })
        ));
        for outside in ["homework/a", "workspace/a"] {
            assert!(matches!(
                manifest(vec![carried(outside)]),
                Err(ManifestError::NotUnderHome { .. })
            ));
        }
        assert!(matches!(
            manifest(vec![carried("home/.a.fabric-tmp-1-2")]),
            Err(ManifestError::TemporaryFile { .. })
        ));
    }

    #[test]
    fn round_trips_through_the_file_and_digests_are_hex() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(vec![carried("home/.claude/projects/-w/s1.jsonl")]).unwrap();
        m.write(dir.path()).unwrap();
        assert_eq!(StateManifest::read(dir.path()).unwrap(), m);
        let text = std::fs::read_to_string(dir.path().join(MANIFEST_FILE)).unwrap();
        assert!(text.contains(&blake3::hash(b"").to_hex().to_string()));
        assert!(serde_json::from_str::<Digest>("\"zz\"").is_err());
    }

    #[test]
    fn a_manifest_file_that_breaks_the_rules_is_refused_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(vec![carried("home/a"), carried("home/b")]).unwrap();
        let mut value = serde_json::to_value(&m).unwrap();
        value["files"].as_array_mut().unwrap().reverse();
        std::fs::write(dir.path().join(MANIFEST_FILE), value.to_string()).unwrap();
        assert!(matches!(
            StateManifest::read(dir.path()),
            Err(HarnessError::Malformed { .. })
        ));
        let mut value = serde_json::to_value(&m).unwrap();
        value["redactions"][0]["rule"] = "Not A Rule".into();
        std::fs::write(dir.path().join(MANIFEST_FILE), value.to_string()).unwrap();
        assert!(StateManifest::read(dir.path()).is_err());
    }
}
