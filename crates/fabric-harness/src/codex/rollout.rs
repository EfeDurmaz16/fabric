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

use crate::error::{HarnessError, Input, IoOp};
use crate::jsontext;
use crate::redact::Redactor;
use crate::session::SessionId;

/// What a rollout file name says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutName {
    /// The `YYYY-MM-DDTHH-MM-SS` text from the name; sorts in time order.
    pub timestamp: String,
    /// The stable thread id.
    pub thread_id: SessionId,
    /// The rollout id; equals the thread id except after a revert.
    pub rollout_id: SessionId,
    /// True when the file is `.jsonl.zst`.
    pub compressed: bool,
}

/// One rollout file as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutFile {
    /// The file, plain or compressed.
    pub path: PathBuf,
    /// What its name says.
    pub name: RolloutName,
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
pub fn parse_name(name: &str) -> Option<RolloutName> {
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
    Some(RolloutName {
        timestamp: timestamp.to_owned(),
        thread_id: SessionId::parse(thread).ok()?,
        rollout_id: SessionId::parse(rollout).ok()?,
        compressed,
    })
}

/// Lists every rollout file under `root`, walking all subdirectories,
/// newest first. When a plain and a compressed file share a directory and
/// a name, the plain one is listed, as Codex does.
pub fn list(root: &Path) -> Result<Vec<RolloutFile>, HarnessError> {
    let mut found: Vec<RolloutFile> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &dir, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io(IoOp::Walk, &path, e))?;
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(name) = parse_name(file_name) else {
                continue;
            };
            if name.compressed {
                let plain = path.with_file_name(file_name.trim_end_matches(".zst"));
                if plain.is_file() {
                    continue;
                }
            }
            found.push(RolloutFile { path, name });
        }
    }
    found.sort_by(|a, b| {
        b.name
            .timestamp
            .cmp(&a.name.timestamp)
            .then_with(|| b.name.rollout_id.cmp(&a.name.rollout_id))
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
    pub history_base: Option<SessionId>,
}

fn open_lines(file: &RolloutFile) -> Result<Box<dyn BufRead>, HarnessError> {
    let handle =
        std::fs::File::open(&file.path).map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?;
    if file.name.compressed {
        let decoder = zstd::stream::read::Decoder::new(handle)
            .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?;
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
        .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?;
    let malformed = |reason: String| HarnessError::Malformed {
        what: Input::RolloutLine,
        path: file.path.clone(),
        reason,
    };
    let parsed: FirstLine =
        serde_json::from_str(line.trim_end()).map_err(|e| malformed(e.to_string()))?;
    if parsed.kind != "session_meta" {
        return Err(malformed(format!(
            "first line is {:?}, not session_meta",
            parsed.kind
        )));
    }
    let history_base = match parsed.payload.history_base {
        Some(base) => Some(
            SessionId::parse(&base.thread_id)
                .map_err(|e| malformed(format!("history_base.thread_id: {e}")))?,
        ),
        None => None,
    };
    Ok(Meta {
        cwd: parsed.payload.cwd,
        history_base,
    })
}

/// Refuses a rollout, plain or compressed, that holds a secret-shaped
/// string, raw or inside a decoded JSON string.
pub(crate) fn refuse_secrets(file: &RolloutFile) -> Result<(), HarnessError> {
    let redactor = Redactor::standard()?;
    let mut lines = open_lines(file)?;
    let mut line = Vec::new();
    let mut number = 0usize;
    loop {
        line.clear();
        if lines
            .read_until(b'\n', &mut line)
            .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?
            == 0
        {
            return Ok(());
        }
        number += 1;
        if jsontext::holds_secret(&line, redactor) {
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
        let name = parse_name(
            "rollout-2026-07-02T17-32-21-019f233e-bb33-7342-85fa-f2d33b14368a.jsonl.zst",
        )
        .unwrap();
        assert_eq!(name.timestamp, "2026-07-02T17-32-21");
        assert_eq!(
            name.thread_id.as_str(),
            "019f233e-bb33-7342-85fa-f2d33b14368a"
        );
        assert_eq!(name.rollout_id, name.thread_id);
        assert!(name.compressed);
        let name = parse_name(
            "rollout-2026-10-19T10-00-00-019f233e-bb33-7342-85fa-f2d33b14368a_019f233e-bb33-7342-85fa-f2d33b14368b.jsonl",
        )
        .unwrap();
        assert_eq!(
            name.thread_id.as_str(),
            "019f233e-bb33-7342-85fa-f2d33b14368a"
        );
        assert_eq!(
            name.rollout_id.as_str(),
            "019f233e-bb33-7342-85fa-f2d33b14368b"
        );
        assert!(!name.compressed);
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
        std::fs::write(dir.path().join(format!("{b}.jsonl.zst")), "").unwrap();
        let listed = list(dir.path()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name.timestamp, "2026-10-19T11-00-00");
        assert!(listed[0].name.compressed);
        assert!(!listed[1].name.compressed);
        assert!(listed[1].path.ends_with(format!("{a}.jsonl")));
    }
}
