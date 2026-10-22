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
///
/// The temporary is always a new file this call made: anything already at
/// its name, a file left by a process that had this pid or a link another
/// process planted, is removed and never written through. A link there
/// would otherwise have this write overwrite the file it names, and for a
/// ledger the link itself renamed into place would take every later append.
/// Callers write one path from one task at a time, under the job's lock.
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
        let mut file = create_new(&temp)?;
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

/// Creates `path` as a new file, after removing whatever is at the name.
///
/// `O_EXCL` fails on any entry at the name, a link included, and never
/// follows one, so the file opened is the one created here.
fn create_new(path: &Path) -> io::Result<File> {
    let open = || {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    };
    match open() {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(path)?;
            open()
        }
        opened => opened,
    }
}

/// Creates `dir` and its parents, then flushes the parent so the new name is durable.
pub(crate) fn create_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    match dir.parent() {
        Some(parent) => sync_dir(parent),
        None => Ok(()),
    }
}

/// Creates `dir`, mode 0700, and its parents, then flushes the parent so
/// the new name is durable; a `dir` that already exists keeps its mode.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| io::Error::other("a private directory needs a parent"))?;
    fs::create_dir_all(parent)?;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => return Ok(()),
        Err(e) => return Err(e),
    }
    sync_dir(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The temporary's name is predictable, and a process that can write
    /// in the state directory, such as a harness, can put a link there. The
    /// write must not go through it to the file it names.
    #[test]
    fn a_link_at_the_temporary_name_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        let path = dir.path().join("ledger");
        let temp = dir
            .path()
            .join(format!("ledger.{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &temp).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::symlink_metadata(&temp).is_err(), "the link was left");
    }

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
