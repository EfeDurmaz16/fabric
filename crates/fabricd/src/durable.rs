//! Durable writes for the files a node keeps beside its store.
//!
//! Invariant: a file this module reports written survives a power loss
//! with all of its bytes, and so does its name. Every write ends with a full
//! flush of the file and of the directory that names it, because on macOS a
//! plain `fsync` leaves bytes in the drive's write cache (ADR 0002, Durable
//! before visible). A replaced file is written beside its final name and
//! renamed over it, so a reader sees the old bytes or the new ones.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Flushes a file's data and metadata to the device, through the drive cache on macOS.
pub(crate) fn full_sync(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        rustix::fs::fcntl_fullfsync(file).map_err(io::Error::from)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_all()
    }
}

/// Flushes a directory so that the names created or renamed in it are durable.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    full_sync(&File::open(dir)?)
}

/// Replaces `path` with `bytes`: a temporary sibling, a full flush, a rename, a directory flush.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("a file to replace needs a parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("a file to replace needs a name"))?;
    let mut temp_name = name.to_os_string();
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = dir.join(temp_name);
    let written = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        full_sync(&file)?;
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written?;
    sync_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_file_holds_the_new_bytes_and_no_temporary_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("marker")]);
    }
}
