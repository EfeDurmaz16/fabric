//! Staging files into a state tree and placing them back.
//!
//! Invariant: every file that enters a state tree passes the denylist, and
//! every file placed on a node is written whole through a rename, only when
//! the bytes already there are equal to, or a prefix of, the incoming bytes
//! (ADR 0006, state outside the root is protected like the root).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fabric_core::path::RelPath;
use tracing::warn;

use crate::denylist;
use crate::error::HarnessError;
use crate::jsontext::{self, TransformError};
use crate::manifest::{Carried, Rewrite};
use crate::redact::{RedactionCounts, Redactor};

/// The redaction rule name under which dropped signed blocks are counted.
pub const SIGNED_BLOCK_DROPPED: &str = "signed-block-dropped";

/// One file an adapter wants to carry.
#[derive(Debug, Clone)]
pub struct Source {
    /// Where the file is on this node.
    pub abs: PathBuf,
    /// Where it goes in the state tree.
    pub rel: RelPath,
    /// What restore will do to it.
    pub rewrite: Rewrite,
}

/// Lists the regular files under `dir`, relative paths in sorted order.
/// A symlink is an error: a harness state tree has none, and following one
/// would carry something outside the tree.
pub fn walk(dir: &Path) -> Result<Vec<PathBuf>, HarnessError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = fs::read_dir(&d).map_err(|e| HarnessError::io("walk", &d, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io("walk", &d, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io("walk", &path, e))?;
            if kind.is_symlink() {
                return Err(HarnessError::SymlinkInState { path });
            }
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let rel = path
                    .strip_prefix(dir)
                    .map(Path::to_path_buf)
                    .unwrap_or(path);
                out.push(rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Joins path components into a `RelPath`, refusing names the tree rules forbid.
pub fn rel_path(parts: &[&str]) -> Result<RelPath, HarnessError> {
    let joined = parts.join("/");
    RelPath::parse(&joined).map_err(|e| HarnessError::Malformed {
        what: "state path",
        path: PathBuf::from(&joined),
        reason: e.to_string(),
    })
}

/// Converts a file system path relative to some root into tree components.
pub fn rel_components(rel: &Path) -> Result<Vec<String>, HarnessError> {
    rel.components()
        .map(|c| {
            c.as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| HarnessError::Malformed {
                    what: "state path",
                    path: rel.to_path_buf(),
                    reason: "not UTF-8".to_owned(),
                })
        })
        .collect()
}

const TEMP_MARK: &str = ".fabric-tmp-";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary sibling of `dest`, unique per process and per call.
fn temp_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    parent.join(format!(
        ".{name}{TEMP_MARK}{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Writes `bytes` to `path` through a temporary sibling and a rename. The
/// file gets the mode of the file it replaces, or 0600 when there is none,
/// and the parent directory is fsynced after the rename so the rename
/// survives a power cut.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| HarnessError::io("create dir", parent, e))?;
    let tmp = temp_path(path);
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::metadata(path) {
            Ok(meta) => fs::set_permissions(&tmp, meta.permissions())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        fs::rename(&tmp, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(HarnessError::io("write", path, e));
    }
    Ok(())
}

/// Copies the bytes at `src` to `dst` without loading large files whole.
fn copy_file(src: &Path, dst: &Path) -> Result<u64, HarnessError> {
    let parent = dst.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| HarnessError::io("create dir", parent, e))?;
    fs::copy(src, dst).map_err(|e| HarnessError::io("copy", src, e))
}

/// Redacts a JSON-text file line by line while copying it. Paths are not
/// mapped at capture; a line that is not JSON (a line cut by a crash) is
/// kept as it is after a plain-text redaction pass, and a line that is not
/// UTF-8 (cut inside a multi-byte character) after the same pass on bytes.
fn stage_json_text(
    src: &Path,
    dst: &Path,
    redactor: &Redactor,
    counts: &mut RedactionCounts,
) -> Result<u64, HarnessError> {
    let parent = dst.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| HarnessError::io("create dir", parent, e))?;
    let input = fs::File::open(src).map_err(|e| HarnessError::io("read", src, e))?;
    let mut out = Vec::new();
    let mut reader = BufReader::new(input);
    let mut line = Vec::new();
    let mut number = 0usize;
    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| HarnessError::io("read", src, e))?;
        if n == 0 {
            break;
        }
        number += 1;
        let (body, newline): (&[u8], &[u8]) = match line.strip_suffix(b"\n") {
            Some(b) => (b, b"\n"),
            None => (&line, b""),
        };
        let Ok(body) = std::str::from_utf8(body) else {
            warn!(path = %src.display(), line = number, "line is not UTF-8; kept as bytes");
            let bytes = redactor
                .apply_bytes(body, counts)
                .unwrap_or_else(|| body.to_vec());
            out.extend_from_slice(&bytes);
            out.extend_from_slice(newline);
            continue;
        };
        if body.trim().is_empty() {
            out.extend_from_slice(&line);
            continue;
        }
        match jsontext::transform(body, redactor) {
            Ok(outcome) => {
                out.extend_from_slice(outcome.text.as_deref().unwrap_or(body).as_bytes());
                out.extend_from_slice(newline);
                for (rule, n) in outcome.redactions {
                    *counts.entry(rule).or_insert(0) += n;
                }
                if outcome.dropped_signed_blocks > 0 {
                    *counts.entry(SIGNED_BLOCK_DROPPED).or_insert(0) +=
                        outcome.dropped_signed_blocks;
                }
            }
            Err(TransformError::NotJson { .. }) => {
                warn!(path = %src.display(), line = number, "line is not JSON; kept as text");
                let text = redactor
                    .apply(body, counts)
                    .unwrap_or_else(|| body.to_owned());
                out.extend_from_slice(text.as_bytes());
                out.extend_from_slice(newline);
            }
            Err(TransformError::SecretInUndroppableSignedBlock) => {
                return Err(HarnessError::SecretInSignedBlock {
                    path: src.to_path_buf(),
                    line: number,
                });
            }
        }
    }
    write_atomic(dst, &out)?;
    Ok(out.len() as u64)
}

/// Redacts a plain-text file line by line while copying it; a line that
/// is not UTF-8 is redacted as bytes and otherwise kept as it is.
fn stage_text(
    src: &Path,
    dst: &Path,
    redactor: &Redactor,
    counts: &mut RedactionCounts,
) -> Result<u64, HarnessError> {
    let input = fs::File::open(src).map_err(|e| HarnessError::io("read", src, e))?;
    let mut reader = BufReader::new(input);
    let mut out = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| HarnessError::io("read", src, e))?;
        if n == 0 {
            break;
        }
        let redacted = match std::str::from_utf8(&line) {
            Ok(text) => redactor.apply(text, counts).map(String::into_bytes),
            Err(_) => redactor.apply_bytes(&line, counts),
        };
        out.extend_from_slice(redacted.as_deref().unwrap_or(&line));
    }
    write_atomic(dst, &out)?;
    Ok(out.len() as u64)
}

/// Stages `sources` under `out`, checking the denylist and redacting JSON
/// text. Returns the carried entries sorted by path and the redaction counts.
pub fn stage(
    sources: &[Source],
    out: &Path,
    counts: &mut RedactionCounts,
) -> Result<Vec<Carried>, HarnessError> {
    let mut carried = Vec::with_capacity(sources.len());
    for source in sources {
        if let Some(rule) = denylist::check(source.rel.as_str()) {
            return Err(HarnessError::Denied {
                path: source.rel.as_str().to_owned(),
                rule,
            });
        }
        let dst = out.join(source.rel.as_str());
        let size = match source.rewrite {
            Rewrite::None => copy_file(&source.abs, &dst)?,
            Rewrite::Text => stage_text(&source.abs, &dst, Redactor::standard(), counts)?,
            Rewrite::JsonText => stage_json_text(&source.abs, &dst, Redactor::standard(), counts)?,
        };
        carried.push(Carried {
            rel_path: source.rel.clone(),
            rewrite: source.rewrite,
            size,
        });
    }
    carried.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    carried.dedup_by(|a, b| a.rel_path == b.rel_path);
    Ok(carried)
}

/// Places `bytes` at `dest` unless a different file is already there.
pub fn place(dest: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    match fs::read(dest) {
        Ok(existing) => {
            if existing.len() > bytes.len() || bytes[..existing.len()] != existing[..] {
                return Err(HarnessError::StateDiverged {
                    path: dest.to_path_buf(),
                });
            }
            if existing.len() == bytes.len() {
                return Ok(());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(HarnessError::io("read", dest, e)),
    }
    write_atomic(dest, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_files_keep_the_mode_they_replace_and_default_to_0600() {
        use std::os::unix::fs::PermissionsExt;

        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.jsonl");
        place(&fresh, b"x\n").unwrap();
        assert_eq!(mode(&fresh), 0o600);
        let private = dir.path().join("private.jsonl");
        fs::write(&private, b"x\n").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        place(&private, b"x\ny\n").unwrap();
        assert_eq!(mode(&private), 0o600);
        let shared = dir.path().join("shared.json");
        fs::write(&shared, b"x\n").unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&shared, b"x\ny\n").unwrap();
        assert_eq!(mode(&shared), 0o644);
    }

    #[test]
    fn a_line_cut_inside_a_two_byte_character_does_not_stop_capture() {
        // `kill -9` cut "café" after the first byte of `é`; Claude then
        // appended a whole line after the cut.
        let dir = tempfile::tempdir().unwrap();
        let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
        let src = dir.path().join("t.jsonl");
        let mut bytes = b"{\"a\":\"ok\"}\n{\"a\":\"caf\xC3".to_vec();
        bytes.extend_from_slice(format!(" {secret}\n{{\"a\":\"{secret}\"}}\n").as_bytes());
        fs::write(&src, bytes).unwrap();
        let out = dir.path().join("out");
        stage(
            &[Source {
                abs: src,
                rel: RelPath::parse("home/t.jsonl").unwrap(),
                rewrite: Rewrite::JsonText,
            }],
            &out,
            &mut RedactionCounts::new(),
        )
        .unwrap();
        let mut expected = b"{\"a\":\"ok\"}\n{\"a\":\"caf\xC3 [REDACTED:github-token]\n".to_vec();
        expected.extend_from_slice(b"{\"a\":\"[REDACTED:github-token]\"}\n");
        assert_eq!(fs::read(out.join("home/t.jsonl")).unwrap(), expected);
    }

    #[test]
    fn stage_counts_a_dropped_signed_block() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
        let src = dir.path().join("t.jsonl");
        fs::write(
            &src,
            format!("{{\"content\":[{{\"type\":\"text\",\"text\":\"a\"}},{{\"thinking\":\"{secret}\",\"signature\":\"s\"}}]}}\n"),
        )
        .unwrap();
        let mut counts = RedactionCounts::new();
        stage(
            &[Source {
                abs: src,
                rel: RelPath::parse("home/t.jsonl").unwrap(),
                rewrite: Rewrite::JsonText,
            }],
            &dir.path().join("out"),
            &mut counts,
        )
        .unwrap();
        assert_eq!(counts.get(SIGNED_BLOCK_DROPPED), Some(&1));
        assert!(!counts.contains_key("github-token"));
    }

    #[test]
    fn place_accepts_equal_and_prefix_and_refuses_divergence() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a/b.jsonl");
        place(&dest, b"one\n").unwrap();
        place(&dest, b"one\n").unwrap();
        place(&dest, b"one\ntwo\n").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"one\ntwo\n");
        assert!(matches!(
            place(&dest, b"one\n"),
            Err(HarnessError::StateDiverged { .. })
        ));
        assert!(matches!(
            place(&dest, b"one\nTWO\n"),
            Err(HarnessError::StateDiverged { .. })
        ));
        assert_eq!(fs::read(&dest).unwrap(), b"one\ntwo\n");
        assert_eq!(fs::read_dir(dir.path().join("a")).unwrap().count(), 1);
    }

    #[test]
    fn stage_refuses_a_denied_path_before_copying() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("src");
        fs::write(&secret, b"x").unwrap();
        let out = dir.path().join("out");
        let sources = vec![Source {
            abs: secret,
            rel: RelPath::parse("home/.claude/.credentials.json").unwrap(),
            rewrite: Rewrite::None,
        }];
        let err = stage(&sources, &out, &mut RedactionCounts::new()).unwrap_err();
        assert!(matches!(err, HarnessError::Denied { .. }), "{err}");
        assert!(!out.exists());
    }

    #[test]
    fn stage_redacts_json_text_and_keeps_a_cut_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("t.jsonl");
        let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
        fs::write(
            &src,
            format!("{{\"cwd\":\"/w\",\"t\":\"{secret}\"}}\n{{\"cwd\":\"/w\",\"cut\":\"{secret}"),
        )
        .unwrap();
        let out = dir.path().join("out");
        let sources = vec![Source {
            abs: src,
            rel: RelPath::parse("home/t.jsonl").unwrap(),
            rewrite: Rewrite::JsonText,
        }];
        let mut counts = RedactionCounts::new();
        let carried = stage(&sources, &out, &mut counts).unwrap();
        assert_eq!(carried.len(), 1);
        let staged = fs::read_to_string(out.join("home/t.jsonl")).unwrap();
        assert!(!staged.contains(secret));
        assert_eq!(staged.matches("[REDACTED:github-token]").count(), 2);
        assert_eq!(counts.get("github-token"), Some(&2));
        assert!(staged.ends_with("\"cut\":\"[REDACTED:github-token]"));
    }
}
