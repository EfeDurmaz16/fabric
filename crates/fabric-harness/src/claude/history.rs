//! Which file-history backups may travel.
//!
//! Invariant: a backup under `file-history/<sid>/` is carried only when the
//! session's transcripts name the file it is a copy of, and that file is
//! not on the credential denylist. Claude names a backup by a hash of the
//! path (`<hex>@v<n>`), so the denylist never sees the original name; the
//! `file-history-snapshot` records of the transcript map each backup back to
//! the path it tracks. A backup of `.env`, a key file or any other denied
//! name stays behind, and so does a backup no record names, because nothing
//! says what it holds. Each one left behind is counted in the redaction
//! report.
//!
//! Record shape, read from the owner's transcripts on 2026-10-20 (key
//! names only): `{"type":"file-history-snapshot","snapshot":
//! {"trackedFileBackups":{"<path>":{"backupFileName":"<hex>@v<n>"|null,
//! ...}}}}`, where the path is absolute or relative to the workspace.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::Deserialize;
use tracing::warn;

use crate::denylist;
use crate::error::HarnessError;
use crate::redact::RedactionCounts;

/// The count name for a backup left behind because its original is denied.
pub(crate) const DENIED_BACKUP: &str = "file-backup-of-denied-file";

/// The count name for a backup left behind because no record names it.
pub(crate) const UNNAMED_BACKUP: &str = "file-backup-not-in-transcript";

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
    snapshot: Option<Snapshot>,
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(rename = "trackedFileBackups", default)]
    tracked: BTreeMap<String, Tracked>,
}

#[derive(Deserialize)]
struct Tracked {
    #[serde(rename = "backupFileName")]
    backup: Option<String>,
}

/// Whether a backup's original may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Every path that maps to it passes the denylist.
    Allowed,
    /// Some path that maps to it is denied.
    Denied,
}

/// The backups the transcripts name, each with whether its original may
/// travel.
#[derive(Debug, Default)]
pub(crate) struct Backups {
    origins: BTreeMap<String, Origin>,
}

impl Backups {
    /// Reads the `file-history-snapshot` records of every transcript.
    /// A line that does not parse is skipped: a cut last line names no
    /// backup, and a backup it would have named is then left behind.
    pub(crate) fn read(transcripts: &[&Path]) -> Result<Self, HarnessError> {
        let mut backups = Backups::default();
        for path in transcripts {
            let file = File::open(path).map_err(|e| HarnessError::io("read", path, e))?;
            let mut reader = BufReader::new(file);
            let mut line = Vec::new();
            loop {
                line.clear();
                let n = reader
                    .read_until(b'\n', &mut line)
                    .map_err(|e| HarnessError::io("read", path, e))?;
                if n == 0 {
                    break;
                }
                if !contains(&line, b"file-history-snapshot") {
                    continue;
                }
                match serde_json::from_slice::<Line>(&line) {
                    Ok(record) => backups.add(record),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "claude: snapshot record skipped");
                    }
                }
            }
        }
        Ok(backups)
    }

    fn add(&mut self, record: Line) {
        if record.kind != "file-history-snapshot" {
            return;
        }
        let Some(snapshot) = record.snapshot else {
            return;
        };
        for (original, tracked) in snapshot.tracked {
            let Some(backup) = tracked.backup else {
                continue;
            };
            let origin = if denylist::check(&original).is_some() {
                Origin::Denied
            } else {
                Origin::Allowed
            };
            let entry = self.origins.entry(backup).or_insert(origin);
            if origin == Origin::Denied {
                *entry = Origin::Denied;
            }
        }
    }

    /// True when the backup named `name` may travel; counts it under the
    /// reason when it may not.
    pub(crate) fn admit(&self, name: &str, counts: &mut RedactionCounts) -> bool {
        let reason = match self.origins.get(name) {
            Some(Origin::Allowed) => return true,
            Some(Origin::Denied) => DENIED_BACKUP,
            None => UNNAMED_BACKUP,
        };
        *counts.entry(reason).or_insert(0) += 1;
        false
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &str, backup: &str) -> String {
        format!(
            "{{\"type\":\"file-history-snapshot\",\"messageId\":\"m\",\"snapshot\":{{\"messageId\":\"m\",\"trackedFileBackups\":{{\"{path}\":{{\"backupFileName\":\"{backup}\",\"version\":1,\"backupTime\":\"t\"}}}},\"timestamp\":\"t\"}}}}\n"
        )
    }

    #[test]
    fn only_backups_of_allowed_named_files_are_admitted() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("t.jsonl");
        let mut text = String::new();
        text.push_str(&record("src/lib.rs", "aaa@v1"));
        text.push_str(&record("/w/.env", "bbb@v1"));
        text.push_str(&record("config/server.pem", "ccc@v2"));
        text.push_str(&record("/w/.env.example", "ddd@v1"));
        text.push_str("{\"type\":\"file-history-snapshot\",\"snapshot\":{\"trackedFi");
        std::fs::write(&transcript, text).unwrap();
        let backups = Backups::read(&[transcript.as_path()]).unwrap();
        let mut counts = RedactionCounts::new();
        assert!(backups.admit("aaa@v1", &mut counts));
        assert!(backups.admit("ddd@v1", &mut counts));
        assert!(!backups.admit("bbb@v1", &mut counts));
        assert!(!backups.admit("ccc@v2", &mut counts));
        assert!(!backups.admit("eee@v1", &mut counts));
        assert_eq!(counts.get(DENIED_BACKUP), Some(&2));
        assert_eq!(counts.get(UNNAMED_BACKUP), Some(&1));
    }

    #[test]
    fn a_backup_named_for_a_denied_path_once_is_never_admitted() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("t.jsonl");
        let text = format!(
            "{}{}",
            record("/w/.env", "aaa@v1"),
            record("a.txt", "aaa@v1")
        );
        std::fs::write(&transcript, text).unwrap();
        let backups = Backups::read(&[transcript.as_path()]).unwrap();
        assert!(!backups.admit("aaa@v1", &mut RedactionCounts::new()));
    }
}
