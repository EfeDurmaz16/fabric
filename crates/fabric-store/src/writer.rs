//! A file writer that cannot leave the directory it was opened on.
//!
//! Invariant: every path is resolved component by component from a directory
//! handle with `O_NOFOLLOW`, never from a string joined to the root, so a
//! symlink that already exists in the tree, whether from an earlier entry or
//! an earlier snapshot, is never followed, and a name that already exists is
//! a collision, never an overwrite. `RelPath` has already refused `..` and
//! `.git`; this module defends against what a path string cannot say.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use fabric_core::path::RelPath;
use fabric_core::snapshot::Mode as EntryMode;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, RawMode, Timespec, Timestamps};
use rustix::io::Errno;

/// Why a write into the tree was refused.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path, relative to the tree root.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// A component on the way to the path is a symlink or not a directory.
    #[error("{path}: a component is a symlink or not a directory")]
    ThroughSymlink {
        /// The path, relative to the tree root.
        path: PathBuf,
    },
    /// The name already exists; on a case-folding volume this is also how two names that fold together show up.
    #[error("{path}: already exists")]
    Collision {
        /// The path, relative to the tree root.
        path: PathBuf,
    },
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn mode(entry: EntryMode) -> Mode {
    Mode::from_raw_mode(RawMode::from(entry.bits()))
}

fn map(errno: Errno, path: &Path) -> WriteError {
    match errno {
        Errno::LOOP | Errno::NOTDIR => WriteError::ThroughSymlink {
            path: path.to_owned(),
        },
        Errno::EXIST => WriteError::Collision {
            path: path.to_owned(),
        },
        other => WriteError::Io {
            path: path.to_owned(),
            source: io::Error::from(other),
        },
    }
}

/// A writer bound to one root directory.
#[derive(Debug)]
pub struct TreeWriter {
    root: OwnedFd,
}

impl TreeWriter {
    /// Opens `root`, which must be a directory and not a symlink to one.
    pub fn open(root: &Path) -> Result<TreeWriter, WriteError> {
        let fd = fs::openat(fs::CWD, root, DIR_FLAGS, Mode::empty()).map_err(|e| map(e, root))?;
        Ok(TreeWriter { root: fd })
    }

    fn child_dir(
        parent: &OwnedFd,
        name: &str,
        so_far: &Path,
        create: bool,
    ) -> Result<OwnedFd, WriteError> {
        match fs::openat(parent, name, DIR_FLAGS, Mode::empty()) {
            Ok(fd) => Ok(fd),
            Err(Errno::NOENT) if create => {
                match fs::mkdirat(parent, name, Mode::from_raw_mode(0o755)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(map(e, so_far)),
                }
                fs::openat(parent, name, DIR_FLAGS, Mode::empty()).map_err(|e| map(e, so_far))
            }
            Err(e) => Err(map(e, so_far)),
        }
    }

    /// Walks to the parent of `rel`, creating missing directories when asked,
    /// and returns the parent handle with the final name.
    fn parent_of<'a>(
        &self,
        rel: &'a RelPath,
        create: bool,
    ) -> Result<(OwnedFd, &'a str), WriteError> {
        let path = Path::new(rel.as_str());
        let mut dir = self.root.try_clone().map_err(|e| WriteError::Io {
            path: path.to_owned(),
            source: e,
        })?;
        let mut components: Vec<&str> = rel.as_str().split('/').collect();
        let name = components.pop().unwrap_or_default();
        let mut so_far = PathBuf::new();
        for component in components {
            so_far.push(component);
            dir = Self::child_dir(&dir, component, &so_far, create)?;
        }
        Ok((dir, name))
    }

    /// Creates a directory with `bits`; an existing directory at the path is fine.
    pub fn mkdir(&self, rel: &RelPath, bits: EntryMode) -> Result<(), WriteError> {
        let (parent, name) = self.parent_of(rel, true)?;
        let path = Path::new(rel.as_str());
        match fs::mkdirat(&parent, name, mode(bits)) {
            Ok(()) => {}
            Err(Errno::EXIST) => {
                let stat = fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|e| map(e, path))?;
                if !FileType::from_raw_mode(stat.st_mode).is_dir() {
                    return Err(WriteError::Collision {
                        path: path.to_owned(),
                    });
                }
            }
            Err(e) => return Err(map(e, path)),
        }
        // mkdir applies the umask; the recorded bits are what the source had.
        let dir = fs::openat(&parent, name, DIR_FLAGS, Mode::empty()).map_err(|e| map(e, path))?;
        fs::fchmod(&dir, mode(bits)).map_err(|e| map(e, path))?;
        Ok(())
    }

    /// Creates a new file with `bits` and returns it open for writing.
    pub fn create_file(&self, rel: &RelPath, bits: EntryMode) -> Result<File, WriteError> {
        let (parent, name) = self.parent_of(rel, true)?;
        let path = Path::new(rel.as_str());
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = fs::openat(&parent, name, flags, mode(bits)).map_err(|e| map(e, path))?;
        fs::fchmod(&fd, mode(bits)).map_err(|e| map(e, path))?;
        Ok(File::from(fd))
    }

    /// Creates a symlink with the target written verbatim.
    pub fn symlink(&self, rel: &RelPath, target: &str) -> Result<(), WriteError> {
        let (parent, name) = self.parent_of(rel, true)?;
        fs::symlinkat(target, &parent, name).map_err(|e| map(e, Path::new(rel.as_str())))
    }

    /// Sets the modification time of a file, directory or symlink itself.
    pub fn set_mtime(&self, rel: &RelPath, mtime_ns: u64) -> Result<(), WriteError> {
        let (parent, name) = self.parent_of(rel, false)?;
        let stamp = Timespec {
            tv_sec: i64::try_from(mtime_ns / 1_000_000_000).unwrap_or(i64::MAX),
            tv_nsec: (mtime_ns % 1_000_000_000).try_into().unwrap_or(0),
        };
        let times = Timestamps {
            last_access: stamp,
            last_modification: stamp,
        };
        fs::utimensat(&parent, name, &times, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| map(e, Path::new(rel.as_str())))
    }

    /// Reads the target of the symlink at `rel` without following it.
    pub fn read_link(&self, rel: &RelPath) -> Result<String, WriteError> {
        let (parent, name) = self.parent_of(rel, false)?;
        let path = Path::new(rel.as_str());
        let target = fs::readlinkat(&parent, name, Vec::new()).map_err(|e| map(e, path))?;
        String::from_utf8(target.into_bytes()).map_err(|e| WriteError::Io {
            path: path.to_owned(),
            source: io::Error::new(io::ErrorKind::InvalidData, e),
        })
    }

    /// Opens an existing regular file for reading without following a link at its name.
    pub fn open_file(&self, rel: &RelPath) -> Result<File, WriteError> {
        let (parent, name) = self.parent_of(rel, false)?;
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = fs::openat(&parent, name, flags, Mode::empty())
            .map_err(|e| map(e, Path::new(rel.as_str())))?;
        Ok(File::from(fd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn rel(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    #[test]
    fn never_writes_through_a_symlink() {
        let outside = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let writer = TreeWriter::open(dir.path()).unwrap();
        writer
            .symlink(&rel("link"), outside.path().to_str().unwrap())
            .unwrap();
        let err = writer
            .create_file(
                &rel("link/escaped"),
                fabric_core::snapshot::Mode::from_masked(0o644),
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::ThroughSymlink { .. }), "{err}");
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        let err = writer
            .mkdir(
                &rel("link/sub"),
                fabric_core::snapshot::Mode::from_masked(0o755),
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::ThroughSymlink { .. }), "{err}");
    }

    #[test]
    fn an_existing_name_is_a_collision_not_an_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let writer = TreeWriter::open(dir.path()).unwrap();
        writer
            .create_file(&rel("a"), fabric_core::snapshot::Mode::from_masked(0o644))
            .unwrap()
            .write_all(b"first")
            .unwrap();
        let err = writer
            .create_file(&rel("a"), fabric_core::snapshot::Mode::from_masked(0o644))
            .unwrap_err();
        assert!(matches!(err, WriteError::Collision { .. }), "{err}");
        let err = writer.symlink(&rel("a"), "x").unwrap_err();
        assert!(matches!(err, WriteError::Collision { .. }), "{err}");
        let err = writer
            .mkdir(&rel("a"), fabric_core::snapshot::Mode::from_masked(0o755))
            .unwrap_err();
        assert!(matches!(err, WriteError::Collision { .. }), "{err}");
        assert_eq!(std::fs::read(dir.path().join("a")).unwrap(), b"first");
        writer
            .mkdir(&rel("d"), fabric_core::snapshot::Mode::from_masked(0o700))
            .unwrap();
        writer
            .mkdir(&rel("d"), fabric_core::snapshot::Mode::from_masked(0o700))
            .unwrap();
    }

    #[test]
    fn modes_and_mtimes_are_exact() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let writer = TreeWriter::open(dir.path()).unwrap();
        writer
            .create_file(
                &rel("x/y/exec"),
                fabric_core::snapshot::Mode::from_masked(0o764),
            )
            .unwrap();
        writer
            .set_mtime(&rel("x/y/exec"), 1_700_000_000_123_456_789)
            .unwrap();
        writer.symlink(&rel("x/l"), "y/exec").unwrap();
        writer
            .set_mtime(&rel("x/l"), 1_600_000_000_000_000_000)
            .unwrap();
        let meta = std::fs::symlink_metadata(dir.path().join("x/y/exec")).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o764);
        assert_eq!(meta.mtime(), 1_700_000_000);
        assert_eq!(meta.mtime_nsec(), 123_456_789);
        let link = std::fs::symlink_metadata(dir.path().join("x/l")).unwrap();
        assert!(link.file_type().is_symlink());
        assert_eq!(link.mtime(), 1_600_000_000);
    }
}
