//! Durable writes for the files a node keeps beside its store, and for
//! the one the CLI keeps there: the approval of a job's setup steps.
//!
//! Invariant: a file `write_atomic` reports written survives a power loss
//! with all of its bytes, and so does its name. A file `replace_ordered`
//! reports written holds after a power loss either all of its new bytes or
//! what it held before, and is durable once any later full flush on its
//! device returns. Either way a replaced file is written beside its final
//! name and renamed over it, so a reader sees the old bytes or the new
//! ones (ADR 0002, Durable before visible).
//!
//! On macOS a plain `fsync` leaves bytes in the drive's write cache, and
//! the drive may write them in any order, so the temporary file gets
//! `F_BARRIERFSYNC` before the rename: its bytes reach the drive, and no
//! write issued after the barrier, the rename included, reaches the medium
//! before them. `write_atomic` then flushes the directory with
//! `F_FULLFSYNC`, which drains the drive's whole cache, the temporary's
//! bytes with it, so the file and its name are durable at return with one
//! full flush. Elsewhere every flush is `fsync`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// How far a flush takes the writes made through a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flush {
    /// To the medium: `F_FULLFSYNC` on macOS, which drains the drive's
    /// whole write cache, so every write it took before is durable too;
    /// `fsync` elsewhere.
    Full,
    /// To the drive, then an I/O barrier, so no write issued after it
    /// reaches the medium before the writes flushed before it, on any
    /// file of the device: `F_BARRIERFSYNC` on macOS, or `F_FULLFSYNC`
    /// where the file system has no barrier; `fsync` elsewhere.
    Barrier,
}

/// Flushes `file` as `how` says.
pub(crate) fn flush(file: &File, how: Flush) -> io::Result<()> {
    #[cfg(test)]
    record::note(file, how);
    #[cfg(target_vendor = "apple")]
    {
        match how {
            Flush::Full => rustix::fs::fcntl_fullfsync(file).map_err(io::Error::from),
            Flush::Barrier => barrier(file),
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = how;
        file.sync_all()
    }
}

/// `F_BARRIERFSYNC`, retried when a signal interrupts it. A file system
/// that has no barrier, which fcntl(2) says of all but HFS and APFS, gets
/// `F_FULLFSYNC`, which orders as well.
#[cfg(target_vendor = "apple")]
fn barrier(file: &File) -> io::Result<()> {
    use nix::errno::Errno;
    use nix::fcntl::{FcntlArg, fcntl};
    loop {
        match fcntl(file, FcntlArg::F_BARRIERFSYNC) {
            Ok(_) => return Ok(()),
            Err(Errno::EINTR) => {}
            Err(Errno::ENOTSUP | Errno::EOPNOTSUPP | Errno::ENOTTY | Errno::EINVAL) => {
                return rustix::fs::fcntl_fullfsync(file).map_err(io::Error::from);
            }
            Err(e) => return Err(io::Error::from(e)),
        }
    }
}

/// Flushes a file's data and metadata to the device, through the drive cache on macOS.
pub(crate) fn full_sync(file: &File) -> io::Result<()> {
    flush(file, Flush::Full)
}

/// Flushes a directory so that the names created or renamed in it are durable.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    full_sync(&File::open(dir)?)
}

/// Replaces `path` with `bytes`, durable at return: a temporary sibling
/// with a barrier, a rename, a full flush of the directory.
///
/// The temporary is always a new file this call made: anything already at
/// its name, a file left by a process that had this pid or a link another
/// process planted, is removed and never written through. A link there
/// would otherwise have this write overwrite the file it names, and for a
/// ledger the link itself renamed into place would take every later append.
/// Within the node, callers write one path from one task at a time, under
/// the job's lock; another process, such as the CLI recording an approval,
/// writes through a temporary of its own, since the name holds its pid.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = replace_ordered(path, bytes)?;
    sync_dir(dir)
}

/// Replaces `path` with `bytes` as `write_atomic` does, without the full
/// flush: after a power loss the name holds the old bytes or all of the
/// new ones, and the new ones are durable once a later full flush on the
/// same device returns. Returns the directory that names the file.
///
/// For a file that may fall behind what it summarizes and is written
/// again when found behind, such as a job's derived manifest after the
/// ledger append it follows, which keeps its own full flush.
pub(crate) fn replace_ordered<'a>(path: &'a Path, bytes: &[u8]) -> io::Result<&'a Path> {
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
        flush(&file, Flush::Barrier)?;
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written?;
    Ok(dir)
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

/// The flushes a test asks for, as ADR 0002 has them checked: which kind,
/// on which file, in which order.
#[cfg(test)]
pub(crate) mod record {
    use std::cell::RefCell;
    use std::fs::File;
    use std::path::{Path, PathBuf};

    use super::Flush;

    thread_local! {
        static LOG: RefCell<Option<Vec<(Flush, PathBuf)>>> = const { RefCell::new(None) };
    }

    /// Runs `body` and returns what it returned with the flushes it asked
    /// for on this thread, each with the path its descriptor had then.
    pub(crate) fn flushes<T>(body: impl FnOnce() -> T) -> (T, Vec<(Flush, PathBuf)>) {
        LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));
        let out = body();
        let log = LOG.with(|log| log.borrow_mut().take()).unwrap_or_default();
        (out, log)
    }

    pub(super) fn note(file: &File, how: Flush) {
        LOG.with(|log| {
            if let Some(log) = log.borrow_mut().as_mut() {
                log.push((how, path_of(file)));
            }
        });
    }

    /// Where the file open as `file` is named now.
    fn path_of(file: &File) -> PathBuf {
        #[cfg(target_vendor = "apple")]
        {
            let mut path = PathBuf::new();
            nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_GETPATH(&mut path)).unwrap();
            path
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            use std::os::fd::AsRawFd;
            std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap()
        }
    }

    /// `path` as a descriptor's path names it: the temporary directory of
    /// a test may sit behind a link, as `/var` does on macOS.
    pub(crate) fn real(path: &Path) -> PathBuf {
        let dir = path.parent().unwrap().canonicalize().unwrap();
        dir.join(path.file_name().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::record::{flushes, real};
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

    /// ADR 0002: the bytes are ordered before the rename by a barrier on
    /// the temporary, while it still has its temporary name, and the one
    /// full flush, of the directory, comes after the rename and makes both
    /// durable: one full flush where flushing both made two.
    #[test]
    fn an_atomic_write_orders_with_a_barrier_and_flushes_the_drive_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker");
        write_atomic(&path, b"one").unwrap();
        let (written, log) = flushes(|| write_atomic(&path, b"two"));
        written.unwrap();
        let temp = real(
            &dir.path()
                .join(format!("marker.{}.tmp", std::process::id())),
        );
        let parent = dir.path().canonicalize().unwrap();
        assert_eq!(log, vec![(Flush::Barrier, temp), (Flush::Full, parent)]);
        assert_eq!(fs::read(&path).unwrap(), b"two");
    }

    /// The ordered write keeps the barrier before the rename and leaves the
    /// full flush to a later write on the device.
    #[test]
    fn an_ordered_replace_has_the_barrier_and_no_full_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest");
        let (written, log) = flushes(|| replace_ordered(&path, b"m"));
        assert_eq!(written.unwrap(), dir.path());
        let temp = real(
            &dir.path()
                .join(format!("manifest.{}.tmp", std::process::id())),
        );
        assert_eq!(log, vec![(Flush::Barrier, temp)]);
        assert_eq!(fs::read(&path).unwrap(), b"m");
    }
}
