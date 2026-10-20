//! What this node held when a job's harness state last left it.
//!
//! Invariant: restore replaces a file that the harness rewrites in place
//! (`Mutability::Replaceable`) with an incoming copy that does not extend
//! it only when the file on disk still has the digest this node recorded
//! the last time it captured that file. The bytes such a replace drops
//! therefore already left with the job, and a file edited here after the
//! job left is never overwritten. The record lives under
//! `~/.fabric/harness/departed/`, never travels, and is written before
//! the manifest, so a capture whose manifest exists always has its record.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::HarnessError;
use crate::files::{self, Source};
use crate::manifest::{Digest, Mutability};
use crate::session::SessionId;

/// The record format this crate writes.
const FORMAT: u16 = 1;

/// The digests of a session's rewritable files on this node at capture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Departure {
    format: u16,
    /// Absolute path on this node, and the digest of its bytes at capture.
    files: BTreeMap<String, Digest>,
}

/// Where the record of `session` for the harness `label` lives under `home`.
pub(crate) fn record_path(home: &Path, label: &str, session: &SessionId) -> PathBuf {
    home.join(".fabric/harness/departed")
        .join(label)
        .join(format!("{session}.json"))
}

fn utf8(path: &Path) -> Result<&str, HarnessError> {
    path.to_str().ok_or_else(|| HarnessError::Malformed {
        what: "departure path",
        path: path.to_path_buf(),
        reason: "not UTF-8".to_owned(),
    })
}

impl Departure {
    /// Digests the local bytes of every source that is `Replaceable`.
    pub(crate) fn of(sources: &[Source]) -> Result<Self, HarnessError> {
        let mut files = BTreeMap::new();
        for source in sources {
            if source.mutability == Mutability::Replaceable {
                let digest = files::digest_file(&source.abs)?.digest;
                files.insert(utf8(&source.abs)?.to_owned(), digest);
            }
        }
        Ok(Departure {
            format: FORMAT,
            files,
        })
    }

    /// Writes the record at `path` through a temporary file and a rename.
    pub(crate) fn write(&self, path: &Path) -> Result<(), HarnessError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| HarnessError::Malformed {
            what: "departure record",
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        files::write_atomic(path, &bytes)
    }

    /// Reads the record at `path`; an empty record when there is none,
    /// because this node never captured the session.
    pub(crate) fn read(path: &Path) -> Result<Self, HarnessError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Departure::default()),
            Err(e) => return Err(HarnessError::io("read", path, e)),
        };
        let record: Departure =
            serde_json::from_slice(&bytes).map_err(|e| HarnessError::Malformed {
                what: "departure record",
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
        if record.format != FORMAT {
            return Err(HarnessError::Malformed {
                what: "departure record",
                path: path.to_path_buf(),
                reason: format!("format {} is not {FORMAT}", record.format),
            });
        }
        Ok(record)
    }

    /// True when `dest` holds exactly the bytes this node recorded for it
    /// at its last capture.
    pub(crate) fn unchanged(&self, dest: &Path) -> Result<bool, HarnessError> {
        let Some(recorded) = self.files.get(utf8(dest)?) else {
            return Ok(false);
        };
        Ok(files::digest_file(dest)?.digest == *recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Carry;
    use fabric_core::path::RelPath;

    #[test]
    fn a_file_edited_after_capture_is_not_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let memory = dir.path().join("MEMORY.md");
        let transcript = dir.path().join("t.jsonl");
        std::fs::write(&memory, "# memory\n").unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();
        let sources = [
            Source {
                abs: memory.clone(),
                rel: RelPath::parse("home/MEMORY.md").unwrap(),
                carry: Carry::Text,
                mutability: Mutability::Replaceable,
            },
            Source {
                abs: transcript.clone(),
                rel: RelPath::parse("home/t.jsonl").unwrap(),
                carry: Carry::JsonLines,
                mutability: Mutability::AppendOnly,
            },
        ];
        let path = dir.path().join("departed/claude/s1.json");
        assert_eq!(Departure::read(&path).unwrap(), Departure::default());
        Departure::of(&sources).unwrap().write(&path).unwrap();
        let record = Departure::read(&path).unwrap();
        assert!(record.unchanged(&memory).unwrap());
        assert!(
            !record.unchanged(&transcript).unwrap(),
            "only rewritable files are recorded"
        );
        std::fs::write(&memory, "# memory\nedited here\n").unwrap();
        assert!(!record.unchanged(&memory).unwrap());
    }
}
