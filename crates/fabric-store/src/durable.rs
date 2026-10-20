//! Durable file writes.
//!
//! Invariant: a file that reaches its final name through this module holds
//! all of its bytes, and once its parent directory has been flushed the name
//! and the bytes survive a power loss. The cost is split the way git's
//! `core.fsyncMethod=batch` splits it: each file gets a plain `fsync`, which
//! on macOS pushes its pages to the drive without flushing the drive's write
//! cache, and the directory flush at the commit point issues `F_FULLFSYNC`,
//! which flushes the whole device cache once for every file written before
//! it (ADR 0014, Publish). A full flush per object measured 4 ms per file on
//! APFS, which is minutes for a `node_modules`; one per shard directory is
//! at most 256 per publish. A shard is flushed when an object was linked
//! into it in this process, and also when publish names an object that has
//! no catalog row yet, because only a committed row proves that an earlier
//! process flushed the shard after linking the file. Publish re-reads such
//! an object first, since a power loss can keep its name and lose its bytes.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

/// Flushes a directory so that renames and links inside it are durable.
pub(crate) fn fsync_dir(path: &Path) -> io::Result<()> {
    let dir = File::open(path)?;
    full_fsync(&dir)
}

/// Flushes a file's data and metadata to the device, through the drive cache on macOS.
pub(crate) fn full_fsync(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        rustix::fs::fcntl_fullfsync(file).map_err(io::Error::from)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_all()
    }
}

/// Opens `path` for reading when it is a regular file, and returns `None` for anything else.
///
/// The open does not follow a symlink in the last component and does not
/// wait on a FIFO, and the type is read from the open descriptor, so what
/// the caller reads is the file that was checked.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<File>> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(Errno::LOOP) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(file.metadata()?.is_file().then_some(file))
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Returns a path in `dir` that no other call in this process returns.
///
/// The name carries the process id and a counter, so a crashed earlier
/// process and a parallel test cannot collide with it.
pub(crate) fn temp_path(dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{}-{n}-{nanos}.tmp", std::process::id()))
}

/// A scratch file name that is removed, if it exists, when the guard drops.
#[derive(Debug)]
pub(crate) struct TempFile(PathBuf);

impl TempFile {
    /// Picks a fresh name in `dir`; nothing is created yet.
    pub(crate) fn in_dir(dir: &Path) -> TempFile {
        TempFile(temp_path(dir))
    }

    /// The file's path.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // A missing file is the normal case on an early error.
        let _ = fs::remove_file(&self.0);
    }
}

/// What `publish_noclobber` found at the final name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Published {
    /// The temp file became the final file.
    New,
    /// A file already held the final name; the temp file was removed.
    Existed,
}

/// Writes `bytes` to a temp file in `tmp_dir`, flushes it, and links it to
/// `final_path` without replacing an existing file.
///
/// `hard_link` fails with `AlreadyExists` when the name is taken, so two
/// writers of the same object cannot replace each other's bytes, and the
/// temp file is removed either way.
pub(crate) fn publish_noclobber(
    tmp_dir: &Path,
    final_path: &Path,
    bytes: &[u8],
) -> io::Result<Published> {
    let tmp = temp_path(tmp_dir);
    let mut file = File::create(&tmp)?;
    let outcome = (|| {
        file.write_all(bytes)?;
        // Plain fsync(2): std's sync_all is F_FULLFSYNC on macOS, which is the
        // per-file drive flush the module doc rules out.
        rustix::fs::fsync(&file)?;
        drop(file);
        match fs::hard_link(&tmp, final_path) {
            Ok(()) => Ok(Published::New),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(Published::Existed),
            Err(e) => Err(e),
        }
    })();
    // The temp name is ours whatever happened; a leftover only wastes space.
    let _ = fs::remove_file(&tmp);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_publish_of_a_name_keeps_the_first_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        fs::create_dir(&tmp).unwrap();
        let final_path = dir.path().join("obj");
        assert_eq!(
            publish_noclobber(&tmp, &final_path, b"first").unwrap(),
            Published::New
        );
        assert_eq!(
            publish_noclobber(&tmp, &final_path, b"second").unwrap(),
            Published::Existed
        );
        assert_eq!(fs::read(&final_path).unwrap(), b"first");
        assert_eq!(fs::read_dir(&tmp).unwrap().count(), 0);
    }
}
