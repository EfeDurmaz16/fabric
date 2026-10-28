//! Durable writes.
//!
//! Invariant: a file written here is never seen half written under its
//! final name, before or after a power loss. It is written to a temporary
//! sibling, flushed to the drive with a barrier, given the mode of the file
//! it replaces (0600 when there is none) and renamed into place, and the
//! parent directory is flushed through the drive's cache after the rename,
//! so the file, its name and the name of every directory created on the
//! way are durable when the write returns. A temporary file is always a
//! new file: it is created exclusively, which never follows a symlink,
//! under a name that holds the pid, a counter and the clock, and a name
//! already taken is skipped for the next one. A temporary file left by a
//! killed run is recognised by its name and is never state.
//!
//! On macOS a plain `fsync` leaves bytes in the drive's write cache, which
//! the drive may write in any order. `F_BARRIERFSYNC` on the temporary
//! makes every write flushed before it, the new directories' names and the
//! file's bytes, reach the medium before any write after it, the rename
//! included; the directory's `F_FULLFSYNC` drains the whole cache, so one
//! full flush makes the write durable (fcntl(2); ADR 0002). Elsewhere every
//! flush is `fsync`.
//!
//! A batch of writes, the files a capture stages, can skip the barrier and
//! the full flush of each (`write_batched_with`): each file and directory
//! is flushed to the drive only, and the batch ends with one atomic write on
//! the same device, the state manifest, whose barrier orders every write of
//! the batch before its rename and whose full flush makes them all durable.
//! This is the method `fabric-store` uses for objects (ADR 0014): a plain
//! `fsync` per file, one full flush at the commit point. A restore places
//! files the user may already hold, so each keeps its barrier, and the
//! directory flushes are shared (`write_ordered_checked`, `Renamed`): when
//! the restore ends each directory it renamed into is flushed once, and one
//! of them per device through the drive's cache.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{HarnessError, IoOp};
use crate::manifest::Digest;

const TEMP_MARK: &str = ".fabric-tmp-";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// True for the name of a temporary file this module writes; a killed run
/// can leave one behind and it is never state.
#[must_use]
pub fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') && name.contains(TEMP_MARK)
}

/// How many taken names `create_temp` skips before it gives up.
const TEMP_ATTEMPTS: u32 = 64;

fn temp_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    parent.join(format!(
        ".{name}{TEMP_MARK}{}-{}-{nanos:09}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Creates a new temporary sibling of `dest`, skipping a name that is
/// already taken by a file, a symlink or anything else.
fn create_temp(dest: &Path) -> Result<(PathBuf, File), HarnessError> {
    let mut last = None;
    for _ in 0..TEMP_ATTEMPTS {
        let tmp = temp_path(dest);
        match open_temp(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some((tmp, e)),
            Err(e) => return Err(HarnessError::io(IoOp::Write, &tmp, e)),
        }
    }
    let (tmp, e) = last.unwrap_or_else(|| {
        (
            temp_path(dest),
            io::Error::from(io::ErrorKind::AlreadyExists),
        )
    });
    Err(HarnessError::io(IoOp::Write, &tmp, e))
}

/// What one write produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Written {
    /// Bytes written.
    pub size: u64,
    /// Digest of those bytes.
    pub digest: Digest,
}

pub(crate) struct Hashing<W> {
    inner: W,
    hasher: blake3::Hasher,
    size: u64,
}

impl<W: Write> Hashing<W> {
    pub(crate) fn new(inner: W) -> Self {
        Hashing {
            inner,
            hasher: blake3::Hasher::new(),
            size: 0,
        }
    }

    pub(crate) fn finish(self) -> (W, Written) {
        let written = Written {
            size: self.size,
            digest: Digest::from_hash(self.hasher.finalize()),
        };
        (self.inner, written)
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Creates `tmp` as a new file with mode 0600. `create_new` is
/// `O_CREAT | O_EXCL`, which fails on an existing name, a symlink
/// included, and so never writes through one.
fn open_temp(tmp: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)?;
    // The umask may have taken bits off; set the mode exactly.
    file.set_permissions(Permissions::from_mode(0o600))?;
    Ok(file)
}

/// How far a flush takes the writes made through a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flush {
    /// To the medium: `F_FULLFSYNC` on macOS, which drains the drive's
    /// whole write cache, so every write it took before is durable too;
    /// `fsync` elsewhere.
    Full,
    /// To the drive, then an I/O barrier, so no write issued after it
    /// reaches the medium before the writes flushed before it, on any file
    /// of the device: `F_BARRIERFSYNC` on macOS, or `F_FULLFSYNC` where the
    /// file system has no barrier; `fsync` elsewhere.
    Barrier,
    /// To the drive only: `fsync`. On macOS the bytes may wait in the
    /// drive's cache, unordered, until a later barrier or full flush on the
    /// device.
    Plain,
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
            Flush::Plain => rustix::fs::fsync(file).map_err(io::Error::from),
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

/// Flushes the directory `dir` as `how` says.
pub(crate) fn flush_dir(dir: &Path, how: Flush) -> Result<(), HarnessError> {
    File::open(dir)
        .and_then(|d| flush(&d, how))
        .map_err(|e| HarnessError::io(IoOp::Fsync, dir, e))
}

/// Flushes the directory `dir` through the drive's cache, so the names
/// created, renamed or removed in it are durable.
pub(crate) fn fsync_dir(dir: &Path) -> Result<(), HarnessError> {
    flush_dir(dir, Flush::Full)
}

/// How an atomic write flushes what it wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Durable at return: a barrier on the temporary before the rename and
    /// a full flush of the parent after it.
    Durable,
    /// Atomic across a power loss, durable once the caller flushes its
    /// `Renamed`: a barrier on the temporary, the parent left to it.
    Ordered,
    /// One write of a batch: every flush plain; ordered and made durable by
    /// the atomic write that ends the batch on the same device.
    Batched,
}

/// Moves `tmp` over `dest`, giving it the mode of the file it replaces, and
/// flushes the parent as `mode` says.
fn commit(tmp: &Path, dest: &Path, mode: Mode) -> Result<(), HarnessError> {
    match fs::metadata(dest) {
        Ok(meta) => {
            fs::set_permissions(tmp, meta.permissions())
                .map_err(|e| HarnessError::io(IoOp::Write, tmp, e))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(HarnessError::io(IoOp::Read, dest, e)),
    }
    fs::rename(tmp, dest).map_err(|e| HarnessError::io(IoOp::Rename, dest, e))?;
    let parent = dest.parent().unwrap_or(Path::new("."));
    match mode {
        Mode::Durable => fsync_dir(parent),
        Mode::Ordered => Ok(()),
        Mode::Batched => flush_dir(parent, Flush::Plain),
    }
}

/// Creates `dir` and every missing ancestor one at a time, flushing the
/// parent of each new directory to the drive, so a file renamed into it
/// later is not lost with its directory entry on a power cut: the barrier
/// before that rename, or the one that ends its batch, orders these names
/// before it, and the full flush after it makes them durable.
fn create_dirs_durable(dir: &Path) -> Result<(), HarnessError> {
    let mut missing = Vec::new();
    let mut at = dir;
    loop {
        match fs::metadata(at) {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                missing.push(at);
                match at.parent() {
                    Some(parent) => at = parent,
                    None => break,
                }
            }
            Err(e) => return Err(HarnessError::io(IoOp::CreateDir, at, e)),
        }
    }
    for new in missing.into_iter().rev() {
        match fs::create_dir(new) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(HarnessError::io(IoOp::CreateDir, new, e)),
        }
        flush_dir(new.parent().unwrap_or(Path::new(".")), Flush::Plain)?;
    }
    Ok(())
}

/// Removes the file at `path`, if any, and makes the removal durable.
pub(crate) fn remove_durable(path: &Path) -> Result<(), HarnessError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(HarnessError::io(IoOp::Remove, path, e)),
    }
    fsync_dir(path.parent().unwrap_or(Path::new(".")))
}

/// Renames `from` over `to` in the same directory and makes the rename durable.
pub(crate) fn rename_durable(from: &Path, to: &Path) -> Result<(), HarnessError> {
    fs::rename(from, to).map_err(|e| HarnessError::io(IoOp::Rename, to, e))?;
    fsync_dir(to.parent().unwrap_or(Path::new(".")))
}

/// Writes a file through a temporary sibling and a rename, letting
/// `produce` write the bytes. The file gets mode 0600, or the mode of the
/// file it replaces; the temporary is flushed with a barrier before the
/// rename and the parent directory through the drive's cache after it, so
/// the file and every directory created on the way are durable at return.
pub(crate) fn write_atomic_with<F>(path: &Path, produce: F) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
{
    write_atomic_checked(path, produce, || Ok(()))
}

/// `write_atomic_with`, running `check` once the temporary file is
/// written and flushed and just before the rename; an error from it leaves
/// `path` as it was and removes the temporary file.
pub(crate) fn write_atomic_checked<F, C>(
    path: &Path,
    produce: F,
    check: C,
) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
    C: FnOnce() -> Result<(), HarnessError>,
{
    write_flushed(path, produce, check, Mode::Durable)
}

/// Writes a file as `write_atomic_with` does, as one write of a batch: the
/// temporary, the new directories and the parent are flushed to the drive
/// only. After a power loss before the batch ends, the file may be missing
/// or torn under its final name; it is ordered and durable once the atomic
/// write that ends the batch, on the same device, returns. A capture ends
/// its batch with the state manifest, which names every staged file by its
/// digest.
pub(crate) fn write_batched_with<F>(path: &Path, produce: F) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
{
    write_flushed(path, produce, || Ok(()), Mode::Batched)
}

fn write_flushed<F, C>(
    path: &Path,
    produce: F,
    check: C,
    mode: Mode,
) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
    C: FnOnce() -> Result<(), HarnessError>,
{
    create_dirs_durable(path.parent().unwrap_or(Path::new(".")))?;
    let (tmp, file) = create_temp(path)?;
    let result = (|| {
        let mut writer = BufWriter::new(Hashing::new(file));
        produce(&mut writer)?;
        let hashing = writer
            .into_inner()
            .map_err(|e| HarnessError::io(IoOp::Write, &tmp, e.into_error()))?;
        let (file, written) = hashing.finish();
        let how = match mode {
            Mode::Durable | Mode::Ordered => Flush::Barrier,
            Mode::Batched => Flush::Plain,
        };
        flush(&file, how).map_err(|e| HarnessError::io(IoOp::Write, &tmp, e))?;
        check()?;
        commit(&tmp, path, mode)?;
        Ok(written)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The directories a run of ordered writes renamed names into, each with
/// its device, to flush once when the run ends: every one but the last of
/// its device with a plain `fsync`, then that last one through the drive's
/// cache. `F_FULLFSYNC` drains the device's whole cache, so it makes
/// durable every write flushed to that device before it (fcntl(2)). A run
/// that renamed into one directory flushes it once, as `write_atomic` does.
#[derive(Debug, Default)]
pub(crate) struct Renamed(Vec<(Option<u64>, PathBuf)>);

impl Renamed {
    /// Notes `dir`; a directory whose device cannot be read is flushed
    /// through the drive's cache on its own.
    fn note(&mut self, dir: &Path) {
        if self.0.iter().any(|(_, seen)| seen == dir) {
            return;
        }
        let device = fs::metadata(dir).ok().map(|meta| meta.dev());
        self.0.push((device, dir.to_path_buf()));
    }

    /// Flushes every directory noted, so every ordered write noted here is
    /// durable when this returns.
    pub(crate) fn flush(self) -> Result<(), HarnessError> {
        let last_of_device = |index: usize| {
            let device = self.0[index].0;
            device.is_none() || self.0[index + 1..].iter().all(|(d, _)| *d != device)
        };
        for (index, (_, dir)) in self.0.iter().enumerate() {
            if !last_of_device(index) {
                flush_dir(dir, Flush::Plain)?;
            }
        }
        for (index, (_, dir)) in self.0.iter().enumerate() {
            if last_of_device(index) {
                fsync_dir(dir)?;
            }
        }
        Ok(())
    }
}

/// `write_atomic_checked` without the directory flush: the file is atomic
/// across a power loss, its barrier ordering its bytes and the directories
/// created on the way before the rename, and it is durable once `renamed`,
/// which notes its directory, is flushed.
pub(crate) fn write_ordered_checked<F, C>(
    path: &Path,
    produce: F,
    check: C,
    renamed: &mut Renamed,
) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
    C: FnOnce() -> Result<(), HarnessError>,
{
    let written = write_flushed(path, produce, check, Mode::Ordered)?;
    renamed.note(path.parent().unwrap_or(Path::new(".")));
    Ok(written)
}

/// Writes `bytes` to `path` through a temporary sibling and a rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    write_atomic_with(path, |w| {
        w.write_all(bytes)
            .map_err(|e| HarnessError::io(IoOp::Write, path, e))
    })
    .map(|_| ())
}

/// The flushes a test asks for: which kind, on which file, in which order.
#[cfg(test)]
pub(crate) mod record {
    use std::cell::RefCell;
    use std::fs::File;
    use std::path::PathBuf;

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

    /// Each flush as its kind and where it went: the directory flushed, or
    /// `tmp:<dir>` for a temporary file of this module in `<dir>`, whose
    /// name holds the clock. Paths are relative to `root`, which may sit
    /// behind a link, as `/var` does on macOS.
    pub(crate) fn shape(log: &[(Flush, PathBuf)], root: &std::path::Path) -> Vec<(Flush, String)> {
        let root = root.canonicalize().unwrap();
        log.iter()
            .map(|(how, path)| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let at = |p: &std::path::Path| {
                    p.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                };
                let place = if super::is_temp_name(name) {
                    format!("tmp:{}", at(path.parent().unwrap()))
                } else {
                    at(path)
                };
                (*how, place)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two directories are new, so their names are flushed to the drive;
    /// the barrier on the temporary orders them and the bytes before the
    /// rename, and the one full flush, of the parent after the rename,
    /// makes all of it durable. Before, each of the four was a full flush.
    #[test]
    fn an_atomic_write_orders_with_a_barrier_and_flushes_the_drive_once() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a/b/dest");
        let (written, log) = record::flushes(|| write_atomic(&dest, b"new"));
        written.unwrap();
        assert_eq!(
            record::shape(&log, dir.path()),
            vec![
                (Flush::Plain, String::new()),
                (Flush::Plain, "a".to_owned()),
                (Flush::Barrier, "tmp:a/b".to_owned()),
                (Flush::Full, "a/b".to_owned()),
            ]
        );
        assert_eq!(fs::read(&dest).unwrap(), b"new");
    }

    #[test]
    fn a_temporary_name_taken_by_a_symlink_is_never_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, b"keep me").unwrap();
        let tmp = dir.path().join(".dest.fabric-tmp-1-1-000000001");
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        let err = open_temp(&tmp).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&victim).unwrap(), b"keep me");

        let dest = dir.path().join("dest");
        write_atomic(&dest, b"new").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(
            !fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&victim).unwrap(), b"keep me");
    }
}
