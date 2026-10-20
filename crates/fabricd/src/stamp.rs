//! A fingerprint of a workspace tree's metadata, so a node can tell whether
//! anything under a root changed since a snapshot was taken or restored.
//!
//! Invariant: an entry added, removed or renamed under the root, or a file
//! or link written, between two measurements changes the stamp, as far as
//! the paths and each file's size, mode, inode, modification and change
//! times show. A change that keeps all of them, which needs a deliberate
//! reset of the change time, is missed. A stamp that differs although
//! nothing that matters changed only costs a capture. A capture writes into
//! the repository too, so its writes are left out: `.git/objects`, where an
//! object added never changes what the refs, the index or the tree hold,
//! and `.git/refs/fabric`, the refs that keep snapshot commits alive. A
//! directory counts by its path alone, since what its own times record is
//! the entries, which count themselves.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The fingerprint of every entry under a root at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeStamp([u8; 32]);

impl TreeStamp {
    /// Measures the tree at `root`, or `None` when any part of it cannot be read.
    ///
    /// The walk runs on a blocking thread; a large tree takes about a second.
    pub async fn measure(root: &Path) -> Option<Self> {
        let root = root.to_owned();
        tokio::task::spawn_blocking(move || walk(&root).ok())
            .await
            .ok()
            .flatten()
    }
}

/// The directories a capture writes into, relative to the root.
const CAPTURE_WRITES: [&str; 2] = [".git/objects", ".git/refs/fabric"];

fn walk(root: &Path) -> io::Result<TreeStamp> {
    let mut hasher = blake3::Hasher::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        let mut entries = std::fs::read_dir(root.join(&rel))?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        for name in entries {
            let rel = rel.join(name);
            if CAPTURE_WRITES
                .iter()
                .any(|skipped| rel == Path::new(skipped))
            {
                continue;
            }
            let meta = std::fs::symlink_metadata(root.join(&rel))?;
            let path = rel.as_os_str().as_bytes();
            hasher.update(&(path.len() as u64).to_le_bytes());
            hasher.update(path);
            if meta.is_dir() {
                hasher.update(b"d");
                pending.push(rel);
                continue;
            }
            hasher.update(b"f");
            for value in [
                u64::from(meta.mode()),
                meta.ino(),
                meta.size(),
                meta.mtime().cast_unsigned(),
                meta.mtime_nsec().cast_unsigned(),
                meta.ctime().cast_unsigned(),
                meta.ctime_nsec().cast_unsigned(),
            ] {
                hasher.update(&value.to_le_bytes());
            }
        }
    }
    Ok(TreeStamp(*hasher.finalize().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_edit_changes_the_stamp_and_what_a_capture_writes_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        let before = TreeStamp::measure(root).await.unwrap();
        assert_eq!(TreeStamp::measure(root).await, Some(before));
        std::fs::write(root.join(".git/objects/ab/cdef"), "object").unwrap();
        std::fs::create_dir_all(root.join(".git/refs/fabric/job/snap")).unwrap();
        std::fs::write(root.join(".git/refs/fabric/job/snap/s1"), "oid\n").unwrap();
        assert_eq!(TreeStamp::measure(root).await, Some(before));
        std::fs::write(root.join("README.md"), "hello, edited\n").unwrap();
        assert_ne!(TreeStamp::measure(root).await, Some(before));
    }
}
