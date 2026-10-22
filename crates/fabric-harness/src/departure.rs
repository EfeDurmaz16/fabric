//! What this node held when a job's harness state last left it.
//!
//! Invariant: restore replaces a file that the harness rewrites in place
//! (`Mutability::Replaceable`) with an incoming copy that does not extend
//! it only when the file on disk still has the digest this node recorded
//! for a capture that is known to have left. A record is written in two
//! steps. Capture writes it as pending, keyed by the digest of the
//! manifest it produced, and restore ignores a pending record. The core
//! confirms it once the snapshot that holds the capture is published, and
//! only then does it become the record restore reads. A capture whose
//! publish failed, or a process killed between capture and publish, so
//! never moves the record forward, and a file edited here after the last
//! published capture is never overwritten. Writing a pending record and
//! confirming one run under one `flock` per session, so a confirm compares
//! and renames the same pending record and never promotes a later capture
//! that replaced it in between. Records live under
//! `~/.fabric/harness/departed/` and never travel.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::durable;
use crate::error::{HarnessError, Input, IoOp};
use crate::manifest::Digest;
use crate::session::SessionId;
use crate::stage;

/// The directory under the home where Fabric keeps its own harness state,
/// which no adapter may capture or restore into.
pub(crate) const FABRIC_DIR: &str = ".fabric";

/// The record format this crate writes.
const FORMAT: u16 = 1;

/// The digests of a session's rewritable files on this node at one capture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Departure {
    format: u16,
    /// The digest of the manifest of the capture this record belongs to;
    /// `None` only for the empty record of a node that never captured.
    capture: Option<Digest>,
    /// Absolute path on this node, and the digest of its bytes at capture.
    files: BTreeMap<String, Digest>,
}

/// Where the records of one session of one harness live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Records {
    /// The record restore reads: the last capture known to have left.
    active: PathBuf,
    /// The record of the last capture, until the core confirms it.
    pending: PathBuf,
    /// The file whose `flock` serializes writing the pending record with
    /// confirming it.
    lock: PathBuf,
}

impl Records {
    /// The records of `session` for the harness `label` under `home`.
    pub(crate) fn of(home: &Path, label: &str, session: &SessionId) -> Self {
        let dir = home.join(FABRIC_DIR).join("harness/departed").join(label);
        Records {
            active: dir.join(format!("{session}.json")),
            pending: dir.join(format!("{session}.pending.json")),
            lock: dir.join(format!("{session}.lock")),
        }
    }

    /// Holds the session's record lock until the file is dropped. Another
    /// process that captures or confirms the same session waits for it;
    /// the lock goes with the process that held it, however it ends.
    fn hold(&self) -> Result<File, HarnessError> {
        let dir = self.lock.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(IoOp::CreateDir, dir, e))?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock)
            .map_err(|e| HarnessError::io(IoOp::Write, &self.lock, e))?;
        file.lock()
            .map_err(|e| HarnessError::io(IoOp::Lock, &self.lock, e))?;
        Ok(file)
    }

    /// Records the rewritable files of the capture whose manifest has
    /// digest `capture`, as pending; it replaces any earlier pending record.
    /// `read` holds each file and the digest of the bytes staging read from
    /// it. The file is never read again here: an edit made after staging
    /// must not be recorded as the bytes that left.
    pub(crate) fn write_pending(
        &self,
        read: &[(PathBuf, Digest)],
        capture: Digest,
    ) -> Result<(), HarnessError> {
        let mut files = BTreeMap::new();
        for (abs, digest) in read {
            files.insert(utf8(abs)?.to_owned(), *digest);
        }
        let record = Departure {
            format: FORMAT,
            capture: Some(capture),
            files,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(|e| HarnessError::Malformed {
            what: Input::DepartureRecord,
            path: self.pending.clone(),
            reason: e.to_string(),
        })?;
        let _held = self.hold()?;
        durable::write_atomic(&self.pending, &bytes)
    }

    /// Makes the pending record of the capture whose manifest has digest
    /// `capture` the one restore reads. Confirming the capture that is
    /// already active again changes nothing. A capture that is neither
    /// pending nor active was not made here or was replaced by a later
    /// one, and is refused, leaving both records as they were.
    pub(crate) fn confirm(&self, session: &SessionId, capture: Digest) -> Result<(), HarnessError> {
        let _held = self.hold()?;
        if read(&self.pending)?.is_some_and(|p| p.capture == Some(capture)) {
            return durable::rename_durable(&self.pending, &self.active);
        }
        if read(&self.active)?.is_some_and(|a| a.capture == Some(capture)) {
            return Ok(());
        }
        Err(HarnessError::DepartureNotPending {
            session: session.clone(),
        })
    }

    /// The record restore reads; an empty one when no capture of the
    /// session has been confirmed on this node.
    pub(crate) fn active(&self) -> Result<Departure, HarnessError> {
        Ok(read(&self.active)?.unwrap_or_default())
    }
}

fn utf8(path: &Path) -> Result<&str, HarnessError> {
    path.to_str().ok_or_else(|| HarnessError::NotUtf8 {
        path: path.to_path_buf(),
    })
}

/// Reads the record at `path`; `None` when there is none.
fn read(path: &Path) -> Result<Option<Departure>, HarnessError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(HarnessError::io(IoOp::Read, path, e)),
    };
    let malformed = |reason: String| HarnessError::Malformed {
        what: Input::DepartureRecord,
        path: path.to_path_buf(),
        reason,
    };
    let record: Departure = serde_json::from_slice(&bytes).map_err(|e| malformed(e.to_string()))?;
    if record.format != FORMAT {
        return Err(malformed(format!(
            "format {} is not {FORMAT}",
            record.format
        )));
    }
    Ok(Some(record))
}

impl Departure {
    /// True when `dest` holds exactly the bytes this record has for it.
    pub(crate) fn unchanged(&self, dest: &Path) -> Result<bool, HarnessError> {
        let Some(recorded) = self.files.get(utf8(dest)?) else {
            return Ok(false);
        };
        Ok(stage::digest_file(dest)?.digest == *recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Mutability;
    use crate::redact::RedactionCounts;
    use crate::stage::{Carry, Source};
    use fabric_core::path::RelPath;

    /// Stages `sources` under `dir/out` and records the capture `capture`
    /// as pending from what staging read.
    fn capture(records: &Records, dir: &Path, sources: &[Source], capture: Digest) {
        let staged = stage::stage(sources, &dir.join("out"), &mut RedactionCounts::new()).unwrap();
        records.write_pending(&staged.replaceable, capture).unwrap();
    }

    fn digest(text: &str) -> Digest {
        Digest::from_hash(blake3::hash(text.as_bytes()))
    }

    /// A rewritable memory file and an append-only transcript under `dir`.
    fn sources(dir: &Path) -> (PathBuf, Vec<Source>) {
        let memory = dir.join("MEMORY.md");
        let transcript = dir.join("t.jsonl");
        std::fs::write(&memory, "# memory\n").unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();
        let sources = vec![
            Source {
                abs: memory.clone(),
                rel: RelPath::parse("home/MEMORY.md").unwrap(),
                carry: Carry::Text,
                mutability: Mutability::Replaceable,
            },
            Source {
                abs: transcript,
                rel: RelPath::parse("home/t.jsonl").unwrap(),
                carry: Carry::JsonLines,
                mutability: Mutability::AppendOnly,
            },
        ];
        (memory, sources)
    }

    #[test]
    fn a_pending_record_counts_only_once_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, sources) = sources(dir.path());
        let session = SessionId::parse("s1").unwrap();
        let records = Records::of(dir.path(), "claude", &session);
        capture(&records, dir.path(), &sources, digest("m1"));
        assert!(!records.active().unwrap().unchanged(&memory).unwrap());

        records.confirm(&session, digest("m1")).unwrap();
        let active = records.active().unwrap();
        assert!(active.unchanged(&memory).unwrap());
        assert!(
            !active.unchanged(&dir.path().join("t.jsonl")).unwrap(),
            "only rewritable files are recorded"
        );
        records.confirm(&session, digest("m1")).unwrap();
        std::fs::write(&memory, "# memory\nedited here\n").unwrap();
        assert!(!active.unchanged(&memory).unwrap());
    }

    #[test]
    fn a_capture_replaced_by_a_later_one_cannot_be_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, sources) = sources(dir.path());
        let session = SessionId::parse("s1").unwrap();
        let records = Records::of(dir.path(), "claude", &session);
        capture(&records, dir.path(), &sources, digest("m1"));
        records.confirm(&session, digest("m1")).unwrap();
        std::fs::write(&memory, "# memory\nsecond\n").unwrap();
        capture(&records, dir.path(), &sources, digest("m2"));
        std::fs::write(&memory, "# memory\nthird\n").unwrap();
        capture(&records, dir.path(), &sources, digest("m3"));
        assert!(matches!(
            records.confirm(&session, digest("m2")),
            Err(HarnessError::DepartureNotPending { .. })
        ));
        std::fs::write(&memory, "# memory\n").unwrap();
        assert!(
            records.active().unwrap().unchanged(&memory).unwrap(),
            "the refused confirm left the active record alone"
        );
    }

    /// A confirm compares the pending record with its capture and then
    /// renames it. A capture of the same session by another process in
    /// between would have the rename promote that later capture, which no
    /// snapshot published yet, so the two take turns under the record lock.
    #[test]
    fn writing_and_confirming_a_pending_record_take_turns() {
        let dir = tempfile::tempdir().unwrap();
        let (_, sources) = sources(dir.path());
        let session = SessionId::parse("s1").unwrap();
        let records = Records::of(dir.path(), "claude", &session);
        capture(&records, dir.path(), &sources, digest("m1"));
        let pending = || read(&records.pending).unwrap().map(|p| p.capture);
        let wait = || std::thread::sleep(std::time::Duration::from_millis(300));

        // A capture holds the lock: a confirm of m1 waits, then finds m2.
        let capturing = records.hold().unwrap();
        let confirm = {
            let (records, session) = (records.clone(), session.clone());
            std::thread::spawn(move || records.confirm(&session, digest("m1")))
        };
        wait();
        let m2 = Departure {
            format: FORMAT,
            capture: Some(digest("m2")),
            files: BTreeMap::new(),
        };
        durable::write_atomic(&records.pending, &serde_json::to_vec(&m2).unwrap()).unwrap();
        drop(capturing);
        assert!(
            matches!(
                confirm.join().unwrap(),
                Err(HarnessError::DepartureNotPending { .. })
            ),
            "a confirm promoted a capture that replaced its own"
        );

        // A confirm holds the lock: a capture waits until it is done.
        let staged = stage::stage(
            &sources,
            &dir.path().join("out3"),
            &mut RedactionCounts::new(),
        )
        .unwrap();
        let confirming = records.hold().unwrap();
        let later = {
            let records = records.clone();
            std::thread::spawn(move || records.write_pending(&staged.replaceable, digest("m3")))
        };
        wait();
        assert_eq!(
            pending(),
            Some(Some(digest("m2"))),
            "a capture replaced the pending record under a confirm"
        );
        drop(confirming);
        later.join().unwrap().unwrap();
        assert_eq!(pending(), Some(Some(digest("m3"))));
    }

    #[test]
    fn the_record_holds_the_bytes_staging_read_not_a_later_edit() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, sources) = sources(dir.path());
        let session = SessionId::parse("s1").unwrap();
        let records = Records::of(dir.path(), "claude", &session);
        let staged = stage::stage(
            &sources,
            &dir.path().join("out"),
            &mut RedactionCounts::new(),
        )
        .unwrap();
        // An edit lands after staging read the file and before the record.
        std::fs::write(
            &memory,
            "# memory
edited after staging
",
        )
        .unwrap();
        records
            .write_pending(&staged.replaceable, digest("m1"))
            .unwrap();
        records.confirm(&session, digest("m1")).unwrap();
        let active = records.active().unwrap();
        assert!(
            !active.unchanged(&memory).unwrap(),
            "the edit never left this node, so restore must not replace it"
        );
        std::fs::write(&memory, "# memory\n").unwrap();
        assert!(active.unchanged(&memory).unwrap());
    }
}
