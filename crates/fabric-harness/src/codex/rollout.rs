//! Codex rollout files: names, lookup and lineage.
//!
//! Invariant: a rollout is found by its file name alone,
//! `rollout-<YYYY-MM-DDTHH-MM-SS>-<thread_id>[_<rollout_id>].jsonl[.zst]`,
//! anywhere under `sessions/` or `archived_sessions/`, which is how Codex
//! itself finds it when its `SQLite` row is missing (`codex-rs/rollout/src/
//! list.rs`, `visit_rollout_filenames`, at `e4568a3`, 2026-10-19). A
//! paginated thread owns only its suffix; the prefix lives in the rollout
//! named by `session_meta.history_base.thread_id`, which is a rollout id,
//! and must travel byte for byte because the reference is a byte offset.
//! Because the bytes travel verbatim, a rollout that holds a secret-shaped
//! string is refused rather than carried.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::HarnessError;
use crate::rewrite::Redactor;

/// One rollout file as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutFile {
    /// The file, plain or compressed.
    pub path: PathBuf,
    /// The `YYYY-MM-DDTHH-MM-SS` text from the name; sorts in time order.
    pub timestamp: String,
    /// The stable thread id.
    pub thread_id: String,
    /// The rollout id; equals the thread id except after a revert.
    pub rollout_id: String,
    /// True when the file is `.jsonl.zst`.
    pub compressed: bool,
}

fn is_uuid_like(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Parses a rollout file name; `None` for any other name.
#[must_use]
pub fn parse_name(name: &str) -> Option<(String, String, String, bool)> {
    let (core, compressed) = match name.strip_suffix(".jsonl.zst") {
        Some(core) => (core, true),
        None => (name.strip_suffix(".jsonl")?, false),
    };
    let core = core.strip_prefix("rollout-")?;
    if core.len() < 21 || core.as_bytes()[19] != b'-' {
        return None;
    }
    let timestamp = &core[..19];
    let ts_ok = timestamp.char_indices().all(|(i, c)| match i {
        10 => c == 'T',
        4 | 7 | 13 | 16 => c == '-',
        _ => c.is_ascii_digit(),
    });
    if !ts_ok {
        return None;
    }
    let ids = &core[20..];
    let (thread, rollout) = ids.split_once('_').unwrap_or((ids, ids));
    if !is_uuid_like(thread) || !is_uuid_like(rollout) {
        return None;
    }
    Some((
        timestamp.to_owned(),
        thread.to_owned(),
        rollout.to_owned(),
        compressed,
    ))
}

/// Lists every rollout file under `root`, walking all subdirectories. When
/// a plain and a compressed file share a name, the plain one is listed, as
/// Codex does.
pub fn list(root: &Path) -> Result<Vec<RolloutFile>, HarnessError> {
    let mut found: Vec<RolloutFile> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(HarnessError::io("walk", &dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io("walk", &dir, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io("walk", &path, e))?;
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some((timestamp, thread_id, rollout_id, compressed)) = parse_name(name) else {
                continue;
            };
            if compressed {
                let plain = path.with_file_name(name.trim_end_matches(".zst"));
                if plain.is_file() {
                    continue;
                }
            }
            found.push(RolloutFile {
                path,
                timestamp,
                thread_id,
                rollout_id,
                compressed,
            });
        }
    }
    found.sort_by(|a, b| {
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| b.rollout_id.cmp(&a.rollout_id))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(found)
}

#[derive(Deserialize)]
struct FirstLine {
    #[serde(rename = "type")]
    kind: String,
    payload: Payload,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    history_base: Option<HistoryBase>,
}

#[derive(Deserialize)]
struct HistoryBase {
    thread_id: String,
}

/// What the first line of a rollout says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meta {
    /// `session_meta.cwd`.
    pub cwd: Option<String>,
    /// The rollout id of the prefix file, for a paginated suffix.
    pub history_base: Option<String>,
}

fn open_lines(file: &RolloutFile) -> Result<Box<dyn BufRead>, HarnessError> {
    let handle =
        std::fs::File::open(&file.path).map_err(|e| HarnessError::io("read", &file.path, e))?;
    if file.compressed {
        let decoder = zstd::stream::read::Decoder::new(handle)
            .map_err(|e| HarnessError::io("read", &file.path, e))?;
        Ok(Box::new(BufReader::new(decoder)))
    } else {
        Ok(Box::new(BufReader::new(handle)))
    }
}

/// Reads and parses the first line of a plain or compressed rollout.
pub fn read_meta(file: &RolloutFile) -> Result<Meta, HarnessError> {
    let mut line = String::new();
    open_lines(file)?
        .read_line(&mut line)
        .map_err(|e| HarnessError::io("read", &file.path, e))?;
    let parsed: FirstLine =
        serde_json::from_str(line.trim_end()).map_err(|e| HarnessError::Malformed {
            what: "rollout line",
            path: file.path.clone(),
            reason: e.to_string(),
        })?;
    if parsed.kind != "session_meta" {
        return Err(HarnessError::Malformed {
            what: "rollout line",
            path: file.path.clone(),
            reason: format!("first line is {:?}, not session_meta", parsed.kind),
        });
    }
    Ok(Meta {
        cwd: parsed.payload.cwd,
        history_base: parsed.payload.history_base.map(|h| h.thread_id),
    })
}

/// Refuses a rollout, plain or compressed, that holds a secret-shaped string.
pub(crate) fn refuse_secrets(file: &RolloutFile) -> Result<(), HarnessError> {
    let mut lines = open_lines(file)?;
    let mut line = Vec::new();
    let mut number = 0usize;
    loop {
        line.clear();
        if lines
            .read_until(b'\n', &mut line)
            .map_err(|e| HarnessError::io("read", &file.path, e))?
            == 0
        {
            return Ok(());
        }
        number += 1;
        if Redactor::standard().matches_bytes(&line) {
            return Err(HarnessError::SecretInState {
                path: file.path.clone(),
                line: number,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_observed_names() {
        let (ts, thread, rollout, compressed) = parse_name(
            "rollout-2026-07-02T17-32-21-019f233e-bb33-7342-85fa-f2d33b14368a.jsonl.zst",
        )
        .unwrap();
        assert_eq!(ts, "2026-07-02T17-32-21");
        assert_eq!(thread, "019f233e-bb33-7342-85fa-f2d33b14368a");
        assert_eq!(rollout, thread);
        assert!(compressed);
        let (_, thread, rollout, compressed) = parse_name(
            "rollout-2026-10-19T10-00-00-019f233e-bb33-7342-85fa-f2d33b14368a_019f233e-bb33-7342-85fa-f2d33b14368b.jsonl",
        )
        .unwrap();
        assert_eq!(thread, "019f233e-bb33-7342-85fa-f2d33b14368a");
        assert_eq!(rollout, "019f233e-bb33-7342-85fa-f2d33b14368b");
        assert!(!compressed);
        assert!(parse_name("rollout-2026-10-19T10-00-00-not-a-uuid.jsonl").is_none());
        assert!(parse_name("notes.jsonl").is_none());
        assert!(
            parse_name("rollout-2026-10-19-019f233e-bb33-7342-85fa-f2d33b14368a.jsonl").is_none()
        );
    }

    #[test]
    fn plain_wins_over_its_compressed_sibling_and_newest_sorts_first() {
        let dir = tempfile::tempdir().unwrap();
        let a = "rollout-2026-10-19T10-00-00-019f233e-bb33-7342-85fa-f2d33b14368a";
        let b = "rollout-2026-10-19T11-00-00-019f233e-bb33-7342-85fa-f2d33b14368b";
        std::fs::create_dir_all(dir.path().join("2026/10/19")).unwrap();
        std::fs::write(dir.path().join(format!("2026/10/19/{a}.jsonl")), "").unwrap();
        std::fs::write(dir.path().join(format!("2026/10/19/{a}.jsonl.zst")), "").unwrap();
        std::fs::write(
            dir.path()
                .join(format!("fabric/x/{b}.jsonl.zst").replace("fabric/x/", "")),
            "",
        )
        .unwrap();
        let listed = list(dir.path()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].timestamp, "2026-10-19T11-00-00");
        assert!(listed[0].compressed);
        assert!(!listed[1].compressed);
        assert!(listed[1].path.ends_with(format!("{a}.jsonl")));
    }
}
