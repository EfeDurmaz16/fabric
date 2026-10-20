//! Durable writes.
//!
//! Invariant: a file written here is never seen half written under its
//! final name. It is written to a temporary sibling, fsynced, given the
//! mode of the file it replaces (0600 when there is none) and renamed into
//! place, and the parent directory is fsynced after the rename, as is the
//! parent of every directory created on the way. A temporary file left by
//! a killed run is recognised by its name and is never state.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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

fn temp_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    parent.join(format!(
        ".{name}{TEMP_MARK}{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
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

fn open_temp(tmp: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)?;
    // `mode` applies only when the file is created; a temporary file left
    // by a killed run keeps its old mode until this resets it.
    file.set_permissions(Permissions::from_mode(0o600))?;
    Ok(file)
}

pub(crate) fn fsync_dir(dir: &Path) -> Result<(), HarnessError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| HarnessError::io(IoOp::Fsync, dir, e))
}

/// Moves `tmp` over `dest`, giving it the mode of the file it replaces, and
/// makes the rename durable.
fn commit(tmp: &Path, dest: &Path) -> Result<(), HarnessError> {
    match fs::metadata(dest) {
        Ok(meta) => {
            fs::set_permissions(tmp, meta.permissions())
                .map_err(|e| HarnessError::io(IoOp::Write, tmp, e))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(HarnessError::io(IoOp::Read, dest, e)),
    }
    fs::rename(tmp, dest).map_err(|e| HarnessError::io(IoOp::Rename, dest, e))?;
    fsync_dir(dest.parent().unwrap_or(Path::new(".")))
}

/// Creates `dir` and every missing ancestor one at a time, fsyncing the
/// parent of each new directory, so a file renamed into it later is not
/// lost with its directory entry on a power cut.
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
        fsync_dir(new.parent().unwrap_or(Path::new(".")))?;
    }
    Ok(())
}

/// Renames `from` over `to` in the same directory and makes the rename durable.
pub(crate) fn rename_durable(from: &Path, to: &Path) -> Result<(), HarnessError> {
    fs::rename(from, to).map_err(|e| HarnessError::io(IoOp::Rename, to, e))?;
    fsync_dir(to.parent().unwrap_or(Path::new(".")))
}

/// Writes a file through a temporary sibling and a rename, letting
/// `produce` write the bytes. The file gets mode 0600, or the mode of the file it
/// replaces, and the parent directory is fsynced after the rename, as is the
/// parent of every directory created on the way.
pub(crate) fn write_atomic_with<F>(path: &Path, produce: F) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
{
    create_dirs_durable(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = temp_path(path);
    let result = (|| {
        let file = open_temp(&tmp).map_err(|e| HarnessError::io(IoOp::Write, &tmp, e))?;
        let mut writer = BufWriter::new(Hashing::new(file));
        produce(&mut writer)?;
        let hashing = writer
            .into_inner()
            .map_err(|e| HarnessError::io(IoOp::Write, &tmp, e.into_error()))?;
        let (file, written) = hashing.finish();
        file.sync_all()
            .map_err(|e| HarnessError::io(IoOp::Write, &tmp, e))?;
        commit(&tmp, path)?;
        Ok(written)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Writes `bytes` to `path` through a temporary sibling and a rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    write_atomic_with(path, |w| {
        w.write_all(bytes)
            .map_err(|e| HarnessError::io(IoOp::Write, path, e))
    })
    .map(|_| ())
}
