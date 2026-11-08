//! `fabric.harness_state.v1`: a captured harness state tree as one blob.
//!
//! The blob is a POSIX ustar archive of the tree, its `manifest.json`
//! (the harness `StateManifest`) among the files, written so the same tree
//! always gives the same bytes: entries sorted by path bytes, directories
//! before what they hold, mode 0755 for a directory and 0644 for a file,
//! owner 0 and time 0. Only directories and regular files are packed; a
//! tree with anything else is refused, as the capture never writes one.

use std::fs;
use std::io;
use std::path::Path;

const BLOCK: usize = 512;

/// Why a tree could not be packed.
#[derive(Debug)]
pub enum PackError {
    /// The tree could not be read.
    Io(io::Error),
    /// An entry that is neither a directory nor a regular file.
    NotPackable(String),
    /// A path ustar cannot hold (over 255 bytes, or no split at a `/`).
    PathTooLong(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::Io(e) => write!(f, "reading the harness state: {e}"),
            PackError::NotPackable(p) => write!(f, "{p} is neither a directory nor a file"),
            PackError::PathTooLong(p) => write!(f, "{p} is too long for a ustar entry"),
        }
    }
}

impl std::error::Error for PackError {}

/// Packs the tree at `dir`.
pub fn pack(dir: &Path) -> Result<Vec<u8>, PackError> {
    let mut entries = Vec::new();
    collect(dir, "", &mut entries)?;
    entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut out = Vec::new();
    for (rel, is_dir) in entries {
        if is_dir {
            header(&mut out, &format!("{rel}/"), 0, b'5', 0o755)?;
        } else {
            let data = fs::read(dir.join(&rel)).map_err(PackError::Io)?;
            header(&mut out, &rel, data.len() as u64, b'0', 0o644)?;
            out.extend_from_slice(&data);
            out.resize(out.len().next_multiple_of(BLOCK), 0);
        }
    }
    out.resize(out.len() + 2 * BLOCK, 0);
    Ok(out)
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, bool)>) -> Result<(), PackError> {
    for entry in fs::read_dir(dir.join(rel)).map_err(PackError::Io)? {
        let entry = entry.map_err(PackError::Io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let kind = entry.file_type().map_err(PackError::Io)?;
        if kind.is_dir() {
            out.push((child.clone(), true));
            collect(dir, &child, out)?;
        } else if kind.is_file() {
            out.push((child, false));
        } else {
            return Err(PackError::NotPackable(child));
        }
    }
    Ok(())
}

fn octal(field: &mut [u8], value: u64) {
    let text = format!("{value:0width$o}\0", width = field.len() - 1);
    field.copy_from_slice(text.as_bytes());
}

fn header(out: &mut Vec<u8>, path: &str, size: u64, kind: u8, mode: u64) -> Result<(), PackError> {
    let too_long = || PackError::PathTooLong(path.to_owned());
    let (prefix, name) = if path.len() <= 100 {
        ("", path)
    } else {
        let split = path
            .char_indices()
            .filter(|&(i, c)| {
                c == '/' && i <= 155 && path.len() - i - 1 <= 100 && i + 1 < path.len()
            })
            .map(|(i, _)| i)
            .next_back()
            .ok_or_else(too_long)?;
        (&path[..split], &path[split + 1..])
    };
    let mut block = [0u8; BLOCK];
    block[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut block[100..108], mode);
    octal(&mut block[108..116], 0);
    octal(&mut block[116..124], 0);
    octal(&mut block[124..136], size);
    octal(&mut block[136..148], 0);
    block[148..156].fill(b' ');
    block[156] = kind;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    block[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u64 = block.iter().map(|b| u64::from(*b)).sum();
    let text = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(text.as_bytes());
    out.extend_from_slice(&block);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_tree_packs_to_the_same_bytes_and_system_tar_lists_it() {
        let dir = tempfile::tempdir().unwrap();
        let deep = format!("home/{}/{}/x", "d".repeat(90), "e".repeat(40));
        fs::create_dir_all(dir.path().join(&deep)).unwrap();
        fs::write(dir.path().join("manifest.json"), b"{}").unwrap();
        fs::write(dir.path().join(&deep).join("s.jsonl"), b"line\n").unwrap();
        let one = pack(dir.path()).unwrap();
        assert_eq!(one, pack(dir.path()).unwrap());
        assert_eq!(one.len() % BLOCK, 0);
        let archive = dir.path().join("t.tar");
        fs::write(&archive, &one).unwrap();
        let listed = std::process::Command::new("tar")
            .arg("-tf")
            .arg(&archive)
            .output();
        if let Ok(listed) = listed {
            let text = String::from_utf8_lossy(&listed.stdout);
            assert!(
                listed.status.success(),
                "{}",
                String::from_utf8_lossy(&listed.stderr)
            );
            assert!(text.contains("manifest.json"), "{text}");
            assert!(text.contains(&format!("{deep}/s.jsonl")), "{text}");
        }
    }
}
