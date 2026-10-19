//! Staging files into a state tree and placing them back.
//!
//! Invariant: every file that enters a state tree passes the denylist, and
//! every file placed on a node is written whole through a rename, only when
//! the bytes already there are equal to, or a prefix of, the incoming bytes
//! (ADR 0006, state outside the root is protected like the root).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use fabric_core::path::RelPath;
use tracing::warn;

use crate::denylist;
use crate::error::HarnessError;
use crate::jsontext::{self, TransformError};
use crate::manifest::{Carried, Rewrite};
use crate::rewrite::{PathMap, RedactionCounts, Redactor};

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
/// Symlinks are skipped with a warning; a harness state tree has none.
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
                warn!(path = %path.display(), "skipping symlink in harness state");
            } else if kind.is_dir() {
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

/// Writes `bytes` to `path` through a temporary file and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| HarnessError::io("create dir", parent, e))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = parent.join(format!(".{name}.fabric-tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
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
/// kept as it is after a plain-text redaction pass.
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
    let mut line = String::new();
    let mut number = 0usize;
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| HarnessError::io("read", src, e))?;
        if n == 0 {
            break;
        }
        number += 1;
        let (body, newline) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line.as_str(), ""),
        };
        if body.trim().is_empty() {
            out.extend_from_slice(line.as_bytes());
            continue;
        }
        match jsontext::transform(body, &PathMap::default(), Some(redactor)) {
            Ok(outcome) => {
                out.extend_from_slice(outcome.text.as_deref().unwrap_or(body).as_bytes());
                out.extend_from_slice(newline.as_bytes());
                for (rule, n) in outcome.redactions {
                    *counts.entry(rule).or_insert(0) += n;
                }
            }
            Err(TransformError::NotJson { .. }) => {
                warn!(path = %src.display(), line = number, "line is not JSON; kept as text");
                let text = redactor
                    .apply(body, counts)
                    .unwrap_or_else(|| body.to_owned());
                out.extend_from_slice(text.as_bytes());
                out.extend_from_slice(newline.as_bytes());
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

/// Applies `map` to a staged JSON-text file and returns the bytes to place.
pub fn rewrite_json_text(path: &Path, map: &PathMap) -> Result<Vec<u8>, HarnessError> {
    let bytes = fs::read(path).map_err(|e| HarnessError::io("read", path, e))?;
    if map.is_empty() {
        return Ok(bytes);
    }
    let text = String::from_utf8(bytes).map_err(|e| HarnessError::Malformed {
        what: "json text",
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let (body, newline) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        if body.trim().is_empty() {
            out.push_str(line);
            continue;
        }
        match jsontext::transform(body, map, None) {
            Ok(outcome) => {
                out.push_str(outcome.text.as_deref().unwrap_or(body));
                out.push_str(newline);
            }
            Err(TransformError::NotJson { .. }) => {
                // A whole-file JSON document has newlines inside values of
                // no line; fall back to transforming the whole text once.
                if index == 0 && text.contains('\n') {
                    if let Ok(outcome) = jsontext::transform(&text, map, None) {
                        return Ok(outcome.text.unwrap_or(text).into_bytes());
                    }
                    warn!(path = %path.display(), "file is not JSON; placed without rewrite");
                    return Ok(text.into_bytes());
                }
                warn!(path = %path.display(), line = index + 1, "line is not JSON; mapped as text");
                out.push_str(&map.apply(line).unwrap_or_else(|| line.to_owned()));
            }
            Err(TransformError::SecretInUndroppableSignedBlock) => {
                out.push_str(line);
            }
        }
    }
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn rewrite_json_text_handles_lines_and_whole_documents() {
        let dir = tempfile::tempdir().unwrap();
        let map = PathMap::new([("/old".to_owned(), "/new".to_owned())]);
        let lines = dir.path().join("l.jsonl");
        fs::write(&lines, "{\"cwd\":\"/old\"}\n{\"cwd\":\"/old/x\"}\n").unwrap();
        assert_eq!(
            rewrite_json_text(&lines, &map).unwrap(),
            b"{\"cwd\":\"/new\"}\n{\"cwd\":\"/new/x\"}\n"
        );
        let doc = dir.path().join("d.json");
        fs::write(&doc, "{\n  \"path\": \"/old/y\"\n}\n").unwrap();
        assert_eq!(
            rewrite_json_text(&doc, &map).unwrap(),
            b"{\n  \"path\": \"/new/y\"\n}\n"
        );
    }
}
