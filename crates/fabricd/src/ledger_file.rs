//! The ledger on disk: one append-only file per job per node (ADR 0002).
//!
//! Invariant: a record is in memory only after its bytes and, for the first
//! record, the directory entry are flushed through the drive cache, so no
//! record is ever answered to a peer and then forgotten by a crash. Open
//! truncates a torn tail and nothing else; any other damage fails closed
//! with the offset and leaves the file byte for byte as it was, because a
//! dropped middle record may be the `MoveImported` or `Stolen` that fences a
//! live copy. A derived job manifest that remembers more than the ledger
//! holds also fails closed: a shorter ledger never wins over a manifest that
//! saw a later epoch.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use fabric_core::ledger::{JobManifest, Ledger, LedgerStore, Record, RecordError, scan};

use crate::durable;

/// Why a ledger file could not be opened or appended to.
#[derive(Debug, thiserror::Error)]
pub enum LedgerFileError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file holds damage that is not a torn tail; it was not modified.
    #[error("ledger {path} is corrupt at byte {offset}; it was left as it is")]
    Corrupt {
        /// The file.
        path: PathBuf,
        /// Where the damage starts.
        offset: usize,
    },
    /// A record in the file could not be read for another reason.
    #[error("ledger {path}: {source}")]
    Record {
        /// The file.
        path: PathBuf,
        /// What the record codec refused.
        #[source]
        source: RecordError,
    },
    /// An earlier append failed and its bytes could not be cut off again.
    #[error("ledger {0} refused an append whose bytes could not be removed; restart the node")]
    Poisoned(PathBuf),
    /// The derived manifest names records the ledger does not hold.
    #[error(
        "ledger {path} holds {ledger} records but its manifest remembers {manifest}; nothing was changed"
    )]
    BehindManifest {
        /// The ledger file.
        path: PathBuf,
        /// Records in the ledger.
        ledger: u64,
        /// Records the manifest counts.
        manifest: u64,
    },
    /// The derived manifest does not decode.
    #[error("job manifest {path} does not decode: {reason}")]
    BadManifest {
        /// The manifest file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
}

/// One job's ledger file, open for appends.
#[derive(Debug)]
pub struct LedgerFile {
    path: PathBuf,
    file: File,
    len: u64,
    records: Vec<Record>,
    poisoned: bool,
}

impl LedgerFile {
    /// Creates the ledger at `path` holding `first`; the file must not exist.
    ///
    /// The record is written to a sibling, flushed and renamed into place,
    /// so a failed write or a crash leaves no ledger rather than an empty
    /// one that would refuse the job forever after.
    pub fn create(path: &Path, first: &Record) -> Result<Self, LedgerFileError> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(io_err(path, io::Error::from(io::ErrorKind::AlreadyExists)));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_err(path, source)),
        }
        durable::write_atomic(path, first.bytes()).map_err(|source| io_err(path, source))?;
        Self::open(path)
    }

    /// Opens the ledger at `path`, truncating a torn tail and refusing any other damage.
    pub fn open(path: &Path) -> Result<Self, LedgerFileError> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(path)
            .map_err(|source| io_err(path, source))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| io_err(path, source))?;
        let scanned = scan(&bytes).map_err(|source| match source {
            RecordError::Corrupt { offset } => LedgerFileError::Corrupt {
                path: path.to_owned(),
                offset,
            },
            source => LedgerFileError::Record {
                path: path.to_owned(),
                source,
            },
        })?;
        let mut len = bytes.len() as u64;
        if let Some(offset) = scanned.torn_tail {
            tracing::warn!(path = %path.display(), offset, "truncating a torn ledger tail");
            len = offset as u64;
            file.set_len(len).map_err(|source| io_err(path, source))?;
            durable::full_sync(&file).map_err(|source| io_err(path, source))?;
        }
        Ok(Self {
            path: path.to_owned(),
            file,
            len,
            records: scanned.records,
            poisoned: false,
        })
    }
}

fn io_err(path: &Path, source: io::Error) -> LedgerFileError {
    LedgerFileError::Io {
        path: path.to_owned(),
        source,
    }
}

impl LedgerStore for LedgerFile {
    type Error = LedgerFileError;

    fn append(&mut self, record: &Record) -> Result<(), LedgerFileError> {
        if self.poisoned {
            return Err(LedgerFileError::Poisoned(self.path.clone()));
        }
        let written = self
            .file
            .write_all(record.bytes())
            .and_then(|()| durable::full_sync(&self.file));
        if let Err(source) = written {
            // Bytes that reached the file without being acknowledged would
            // sit in the middle of the chain after the next append.
            if self
                .file
                .set_len(self.len)
                .and_then(|()| durable::full_sync(&self.file))
                .is_err()
            {
                self.poisoned = true;
            }
            return Err(io_err(&self.path, source));
        }
        self.len += record.bytes().len() as u64;
        self.records.push(record.clone());
        Ok(())
    }

    fn records(&self) -> Result<Vec<Record>, LedgerFileError> {
        Ok(self.records.clone())
    }
}

/// Writes the derived manifest of `ledger` beside it, after the append it summarizes.
pub fn write_manifest(path: &Path, ledger: &Ledger) -> Result<(), LedgerFileError> {
    let bytes = ledger
        .manifest()
        .encode()
        .map_err(|e| LedgerFileError::BadManifest {
            path: path.to_owned(),
            reason: e.to_string(),
        })?;
    durable::write_atomic(path, &bytes).map_err(|source| io_err(path, source))
}

/// Fails closed when the manifest at `path` remembers more than `ledger` holds.
///
/// A missing manifest, or one that is behind the ledger, is the normal crash
/// window between an append and the manifest write; the caller rewrites it.
pub fn check_manifest(path: &Path, ledger: &Ledger) -> Result<(), LedgerFileError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_err(path, source)),
    };
    let saved = JobManifest::decode(&bytes).map_err(|e| LedgerFileError::BadManifest {
        path: path.to_owned(),
        reason: e.to_string(),
    })?;
    let held = ledger.records().len() as u64;
    let known = ledger.records().iter().any(|r| r.hash() == saved.head);
    if saved.event_count > held || !known {
        return Err(LedgerFileError::BehindManifest {
            path: path.to_owned(),
            ledger: held,
            manifest: saved.event_count,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use fabric_core::harness::HarnessKind;
    use fabric_core::id::{EventId, JobId, NodeId};
    use fabric_core::ledger::{Author, EventKind};

    use super::*;

    const JOB: JobId = JobId::from_bytes([4; 16]);
    const NODE: NodeId = NodeId::from_bytes([5; 32]);

    fn event(n: u64) -> EventId {
        let mut bytes = [0; 16];
        bytes[8..].copy_from_slice(&n.to_be_bytes());
        EventId::from_bytes(bytes)
    }

    /// A ledger at `path` with `Created` and `notes` notes.
    fn write_chain(path: &Path, notes: u64) -> (LedgerFile, Ledger) {
        let created = Ledger::created(
            JOB,
            "goal".into(),
            HarnessKind::ClaudeCode,
            NODE,
            event(0),
            1,
        )
        .unwrap();
        let mut file = LedgerFile::create(path, &created).unwrap();
        let mut ledger = Ledger::open(JOB, &file).unwrap();
        for n in 1..=notes {
            let note = ledger
                .propose(
                    NODE,
                    EventKind::Note {
                        text: format!("note {n}"),
                    },
                    event(n),
                    n,
                )
                .unwrap();
            ledger.commit(&mut file, note, Author::Node(NODE)).unwrap();
        }
        (file, ledger)
    }

    #[test]
    fn a_torn_tail_is_cut_and_every_record_before_it_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger");
        let (_, ledger) = write_chain(&path, 3);
        let whole = fs::read(&path).unwrap();
        let last = ledger.records().last().unwrap().bytes().len();
        fs::write(&path, &whole[..whole.len() - last / 2]).unwrap();

        let reopened = LedgerFile::open(&path).unwrap();
        assert_eq!(reopened.records().unwrap().len(), 3);
        assert_eq!(fs::read(&path).unwrap().len(), whole.len() - last);
        Ledger::open(JOB, &reopened).unwrap();
    }

    #[test]
    fn a_damaged_middle_record_fails_closed_and_the_file_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger");
        let (_, ledger) = write_chain(&path, 3);
        let mut bytes = fs::read(&path).unwrap();
        let second = ledger.records()[0].bytes().len() + 12;
        bytes[second] ^= 0xff;
        fs::write(&path, &bytes).unwrap();

        assert!(matches!(
            LedgerFile::open(&path),
            Err(LedgerFileError::Corrupt { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn a_ledger_shorter_than_its_manifest_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger");
        let manifest = dir.path().join("manifest");
        let (_, ledger) = write_chain(&path, 2);
        write_manifest(&manifest, &ledger).unwrap();
        check_manifest(&manifest, &ledger).unwrap();

        let last = ledger.records().last().unwrap().bytes().len();
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - last]).unwrap();
        let shorter = Ledger::open(JOB, &LedgerFile::open(&path).unwrap()).unwrap();
        assert!(matches!(
            check_manifest(&manifest, &shorter),
            Err(LedgerFileError::BehindManifest {
                ledger: 2,
                manifest: 3,
                ..
            })
        ));
    }

    /// The child side of the kill test: appends notes forever and reports
    /// each acknowledged count to the progress file.
    #[test]
    fn child_appends_until_killed() {
        let Some(dir) = std::env::var_os("FABRICD_LEDGER_CHILD") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let (mut file, mut ledger) = write_chain(&dir.join("ledger"), 0);
        for n in 1.. {
            let note = ledger
                .propose(
                    NODE,
                    EventKind::Note {
                        text: "x".repeat(4096),
                    },
                    event(n),
                    n,
                )
                .unwrap();
            ledger.commit(&mut file, note, Author::Node(NODE)).unwrap();
            fs::write(dir.join("progress"), (n + 1).to_string()).unwrap();
        }
    }

    #[test]
    fn a_writer_killed_mid_append_keeps_every_acknowledged_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ledger_file::tests::child_appends_until_killed"])
            .env("FABRICD_LEDGER_CHILD", dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let progress = dir.path().join("progress");
        let started = Instant::now();
        let acknowledged = loop {
            let count: u64 = fs::read_to_string(&progress)
                .ok()
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            if count >= 20 {
                break count;
            }
            assert!(
                started.elapsed() < Duration::from_mins(1),
                "the child made no progress"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        child.kill().unwrap();
        child.wait().unwrap();

        let file = LedgerFile::open(&dir.path().join("ledger")).unwrap();
        let ledger = Ledger::open(JOB, &file).unwrap();
        assert!(ledger.records().len() as u64 >= acknowledged);
    }
}
