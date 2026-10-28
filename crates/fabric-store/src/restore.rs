//! Workspace restore (ADR 0004, Restore; as built in ADR 0014, Restore).
//!
//! Invariant: restore writes into a staging directory beside the target and
//! renames it into place only after every byte is written and, when asked,
//! verified. Nothing is written through a symlink or outside the staging
//! directory, an existing target with content is never touched, and a crash
//! leaves either no target or a complete one: a staging directory that
//! outlives its process is the sign of a restore that did not finish. A
//! staging directory is removed only when it holds the owner file restore
//! writes into it first; a directory of that name that restore did not
//! make is refused, never deleted.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;

use fabric_core::git::{GitConfigKey, GitConfigValue, GitOid};
use fabric_core::id::{ChunkId, JobId, SnapshotId};
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::{
    CacheEntry, CarryState, Entry, EntryKind, GitHead, GitState, HarnessState, Mode, PathMeta,
    SnapshotManifest,
};

use crate::bundle_cache::{self, BundleCache};
use crate::capture::{stash_prefix, symlink_escapes};
use crate::checkpoint;
use crate::durable::{self, TempFile};
use crate::git::{Git, GitError};
use crate::objects::ObjectError;
use crate::store::{Store, StoreError};
use crate::writer::{TreeWriter, WriteError};

/// The suffix of the staging directory a restore writes into.
pub const STAGING_SUFFIX: &str = ".fabric-restoring";

/// The file a restore writes first into its staging directory, which proves the directory is its own.
const STAGING_OWNER: &str = "fabric-staging";

/// What the owner file holds.
const STAGING_MAGIC: &[u8] = b"fabric restore staging, v1\n";

/// The directory inside the staging directory that is renamed onto the target.
const STAGING_TREE: &str = "tree";

/// How much of the written tree is re-read against the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    /// Trust the writes.
    None,
    /// Re-read about this many files, spread over the list.
    Sample(usize),
    /// Re-read every file.
    All,
}

/// What a crash the restore must survive looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// A killed process: the kernel finishes the writes it was given.
    ProcessCrash,
    /// A power loss: every file restore writes, every directory of the staging tree, and the index, refs, packs and config git writes, reach the drive before the rename, and the rename's directory flush empties the drive cache.
    PowerLoss,
}

/// What to restore and where.
pub struct RestoreRequest<'a> {
    /// The snapshot to materialize.
    pub snapshot: SnapshotId,
    /// The directory to create; absent or empty.
    pub target: &'a Path,
    /// Where the harness state goes; absent or empty.
    pub harness_state: Option<&'a Path>,
    /// How much to re-read.
    pub verify: Verify,
    /// Which crash to survive.
    pub durability: Durability,
    /// Decides per cache row whether its bytes are written (ADR 0012); a `NotCarried` row never is.
    pub carry: &'a dyn Fn(&CacheEntry) -> bool,
}

/// Files and bytes written for one class of path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    /// Regular files and symlinks.
    pub files: u64,
    /// The bytes of the regular files among them.
    pub bytes: u64,
}

impl Written {
    fn add(&mut self, bytes: u64) {
        self.files += 1;
        self.bytes += bytes;
    }
}

impl std::ops::Add for Written {
    type Output = Written;

    fn add(self, other: Written) -> Written {
        Written {
            files: self.files + other.files,
            bytes: self.bytes + other.bytes,
        }
    }
}

/// What restore wrote, by class of path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Working-tree files the index holds: tracked files, staged or not.
    pub tracked: Written,
    /// Working-tree files the index does not hold.
    pub untracked: Written,
    /// Ignored files from `files`: precious files and the caches the target writes.
    pub ignored: Written,
    /// Harness state files.
    pub harness: Written,
    /// Files and links re-read and found equal to the store or to the working-tree commit.
    pub verified: u64,
    /// Cache rows whose bytes were not written.
    pub skipped_caches: Vec<RelPath>,
    /// Submodule paths left uninitialized.
    pub submodules: Vec<RelPath>,
    /// Symlinks written whose target is absolute or climbs above the root.
    pub escaping_symlinks: Vec<RelPath>,
    /// Bundles of the chain whose objects the repository got: from the
    /// node's bundle cache or unpacked.
    pub bundles_fetched: u32,
    /// The bundles among them this restore unpacked with git; the rest came
    /// from the node's bundle cache.
    pub bundles_unpacked: u32,
}

impl RestoreReport {
    /// Every file and byte written, whatever its class.
    #[must_use]
    pub fn total(&self) -> Written {
        self.tracked + self.untracked + self.ignored + self.harness
    }
}

/// What a target path holds, as far as a restore is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreState {
    /// Nothing, or an empty directory.
    Absent,
    /// A staging directory exists: a restore started and did not finish.
    HalfRestored {
        /// The staging directory.
        staging: PathBuf,
    },
    /// A directory with content and no staging directory beside it.
    Present,
}

/// Why restore stopped.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The store, or an object in it, refused.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The tree writer refused a path.
    #[error(transparent)]
    Write(#[from] WriteError),
    /// The target is not an absolute path with a parent directory.
    #[error("{path} is not an absolute path with a parent")]
    BadTarget {
        /// The path given.
        path: PathBuf,
    },
    /// A directory with the staging name exists that no restore made; it is left alone.
    #[error("{path} exists and was not made by a restore; move it away and retry")]
    ForeignStaging {
        /// The directory.
        path: PathBuf,
    },
    /// The target and the harness state directory lie one inside the other.
    #[error("{harness_state} and {target} lie one inside the other")]
    NestedDirectories {
        /// The target, resolved.
        target: PathBuf,
        /// The harness state directory, resolved.
        harness_state: PathBuf,
    },
    /// The target exists and holds something.
    #[error("{path} exists and is not empty")]
    TargetNotEmpty {
        /// The path given.
        path: PathBuf,
    },
    /// A path inside the working-tree commit breaks a manifest path rule.
    #[error("tree path {path:?}: {source}")]
    BadTreePath {
        /// The path as git listed it.
        path: String,
        /// Which rule it breaks.
        #[source]
        source: PathError,
    },
    /// `git ls-tree` or `cat-file` output did not have the expected shape.
    #[error("cannot parse git tree output {text:?}")]
    BadTreeOutput {
        /// The text.
        text: String,
    },
    /// A written file does not match the store.
    #[error("{path}: written bytes differ from the store")]
    VerifyFailed {
        /// The path, relative to its root.
        path: RelPath,
    },
    /// A file's chunks did not add up to its recorded size.
    #[error("{path}: chunks hold {written} bytes, the entry says {size}")]
    SizeMismatch {
        /// The path.
        path: RelPath,
        /// Bytes the chunks held.
        written: u64,
        /// Bytes the entry records.
        size: u64,
    },
    /// A file of the working-tree commit has a recorded mode whose owner
    /// execute bit is not the one its tree entry records (`100755` or
    /// `100644`). Capture makes the two agree, so only a manifest no
    /// capture wrote holds this; written, the file would differ from the
    /// index on the target.
    #[error("{path}: the recorded mode {mode:?} and the tree disagree on the execute bit")]
    ModeDisagreesWithTree {
        /// The path.
        path: RelPath,
        /// The mode the manifest records.
        mode: Mode,
    },
    /// A symlink in the working-tree commit has a target longer than any filesystem allows.
    #[error("{path}: symlink target is {size} bytes")]
    SymlinkTooLong {
        /// The link.
        path: RelPath,
        /// The blob's size.
        size: u64,
    },
    /// A bundle in the chain names a base snapshot the store does not have.
    #[error("bundle chain of {snapshot} needs {base}, which is not in the store")]
    BundleChainBroken {
        /// The snapshot being restored.
        snapshot: SnapshotId,
        /// The missing base.
        base: SnapshotId,
    },
}

/// An object error always arrives as `StoreError::Object`, so a caller matches one shape.
impl From<ObjectError> for RestoreError {
    fn from(error: ObjectError) -> Self {
        RestoreError::Store(StoreError::Object(error))
    }
}

fn io_err(path: &Path, source: io::Error) -> RestoreError {
    RestoreError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Returns the staging directory for `target`.
fn staging_path(target: &Path) -> Result<PathBuf, RestoreError> {
    let bad = || RestoreError::BadTarget {
        path: target.to_owned(),
    };
    if !target.is_absolute() {
        return Err(bad());
    }
    let parent = target.parent().ok_or_else(bad)?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(bad)?;
    Ok(parent.join(format!("{name}{STAGING_SUFFIX}")))
}

/// Reads at most one byte more than the owner file holds, or `None` when `staging` has no owner file.
fn owner_bytes(staging: &Path) -> Result<Option<Vec<u8>>, RestoreError> {
    use rustix::fs::{Mode as RawBits, OFlags};
    let path = staging.join(STAGING_OWNER);
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = match rustix::fs::open(&path, flags, RawBits::empty()) {
        Ok(fd) => fs::File::from(fd),
        // Absent, or a link in its place, which is not the owner file.
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) => {
            return Ok(None);
        }
        Err(e) => return Err(io_err(&path, e.into())),
    };
    // Nor is a FIFO or a directory.
    if !file.metadata().map_err(|e| io_err(&path, e))?.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(STAGING_MAGIC.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(&path, e))?;
    Ok(Some(bytes))
}

/// What a directory at the staging path is, as far as restore can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Staging {
    /// Not a directory a restore made: no owner file, and not empty.
    Foreign,
    /// A restore's: empty, as a kill right after its creation leaves it, or
    /// holding the owner file, whole or cut short while it was written.
    Ours {
        /// True while it holds the tree, which is renamed onto the target last.
        tree: bool,
    },
}

/// Tells a staging directory a restore made from any other directory of that name.
fn staging_kind(staging: &Path) -> Result<Staging, RestoreError> {
    match fs::symlink_metadata(staging) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(Staging::Foreign),
        Err(e) => return Err(io_err(staging, e)),
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(staging).map_err(|e| io_err(staging, e))? {
        names.push(entry.map_err(|e| io_err(staging, e))?.file_name());
    }
    let owned = names.is_empty()
        || owner_bytes(staging)?.is_some_and(|bytes| STAGING_MAGIC.starts_with(&bytes));
    Ok(if owned {
        Staging::Ours {
            tree: names.iter().any(|name| name == STAGING_TREE),
        }
    } else {
        Staging::Foreign
    })
}

/// Says whether `target` is absent, half restored or present.
///
/// A staging directory of a restore that holds no tree, because the tree
/// was already renamed onto the target or never made, does not count; the
/// target says what is there.
pub fn restore_state(target: &Path) -> Result<RestoreState, RestoreError> {
    if let Ok(staging) = staging_path(target)
        && fs::symlink_metadata(&staging).is_ok()
        && staging_kind(&staging)? != (Staging::Ours { tree: false })
    {
        return Ok(RestoreState::HalfRestored { staging });
    }
    match fs::read_dir(target) {
        Ok(mut entries) => Ok(if entries.next().is_some() {
            RestoreState::Present
        } else {
            RestoreState::Absent
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(RestoreState::Absent),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => Ok(RestoreState::Present),
        Err(e) => Err(io_err(target, e)),
    }
}

/// Refuses a target that exists with content; an empty directory is fine.
fn check_target_empty(target: &Path) -> Result<(), RestoreError> {
    match fs::read_dir(target) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(RestoreError::TargetNotEmpty {
                    path: target.to_owned(),
                });
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => Err(RestoreError::TargetNotEmpty {
            path: target.to_owned(),
        }),
        Err(e) => Err(io_err(target, e)),
    }
}

/// Gives the owner write and search permission on `dir` and every directory below it.
///
/// A restore that was killed after it applied the recorded modes leaves
/// read-only directories with files in them, which no unlink can empty.
/// Links are never followed.
pub(crate) fn open_up(dir: &Path) -> Result<(), RestoreError> {
    use rustix::fs::{Mode as RawBits, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = rustix::fs::open(dir, flags, RawBits::empty()).map_err(|e| io_err(dir, e.into()))?;
    let stat = rustix::fs::fstat(&fd).map_err(|e| io_err(dir, e.into()))?;
    let bits = RawBits::from_raw_mode(stat.st_mode) | RawBits::RWXU;
    rustix::fs::fchmod(&fd, bits).map_err(|e| io_err(dir, e.into()))?;
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        if entry
            .file_type()
            .map_err(|e| io_err(&entry.path(), e))?
            .is_dir()
        {
            open_up(&entry.path())?;
        }
    }
    Ok(())
}

/// Returns `path` with its deepest existing ancestor resolved, so two spellings of one place compare equal.
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut ancestor = path;
    loop {
        if let Ok(real) = fs::canonicalize(ancestor) {
            return rest.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (ancestor.parent(), ancestor.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                ancestor = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

/// Refuses a harness state directory inside the target, or a target inside it.
///
/// Either rename would then land in a directory the other one fills, and
/// every rerun would stop at a non-empty target.
fn refuse_nested(target: &Path, harness_state: &Path) -> Result<(), RestoreError> {
    let place = |path: &Path| match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => path.to_owned(),
    };
    let (target, harness_state) = (place(target), place(harness_state));
    if target.starts_with(&harness_state) || harness_state.starts_with(&target) {
        return Err(RestoreError::NestedDirectories {
            target,
            harness_state,
        });
    }
    Ok(())
}

/// Makes a fresh staging directory for `target`, removing a stale one a restore made, and returns the tree inside it.
///
/// The owner file is written, and under `PowerLoss` flushed, before the
/// tree exists, so a staging directory that holds anything restore wrote
/// also holds the proof that restore made it. A directory of the staging
/// name without that proof is `ForeignStaging`.
fn fresh_staging(target: &Path, durability: Durability) -> Result<PathBuf, RestoreError> {
    let staging = staging_path(target)?;
    match fs::symlink_metadata(&staging) {
        Ok(_) => {
            if staging_kind(&staging)? == Staging::Foreign {
                return Err(RestoreError::ForeignStaging { path: staging });
            }
            open_up(&staging)?;
            fs::remove_dir_all(&staging).map_err(|e| io_err(&staging, e))?;
            tracing::info!(path = %staging.display(), "removed a half-finished restore");
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&staging, e)),
    }
    fs::create_dir(&staging).map_err(|e| io_err(&staging, e))?;
    let owner = staging.join(STAGING_OWNER);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&owner)
        .map_err(|e| io_err(&owner, e))?;
    file.write_all(STAGING_MAGIC)
        .map_err(|e| io_err(&owner, e))?;
    if durability == Durability::PowerLoss {
        rustix::fs::fsync(&file).map_err(|e| io_err(&owner, e.into()))?;
        rustix::fs::fsync(fs::File::open(&staging).map_err(|e| io_err(&staging, e))?)
            .map_err(|e| io_err(&staging, e.into()))?;
    }
    let tree = staging.join(STAGING_TREE);
    fs::create_dir(&tree).map_err(|e| io_err(&tree, e))?;
    Ok(tree)
}

/// Renames the staging tree onto the target, flushes the parent, then removes the emptied staging directory.
fn move_into_place(tree: &Path, target: &Path) -> Result<(), RestoreError> {
    fs::rename(tree, target).map_err(|e| io_err(target, e))?;
    if let Some(parent) = target.parent() {
        durable::fsync_dir(parent).map_err(|e| io_err(parent, e))?;
    }
    // The tree is in place, so the restore has succeeded; a staging
    // directory a kill leaves here holds the owner file or nothing, and
    // `restore_state` and the next `fresh_staging` both know it for what it is.
    if let Some(staging) = tree.parent() {
        let removed =
            fs::remove_file(staging.join(STAGING_OWNER)).and_then(|()| fs::remove_dir(staging));
        if let Err(e) = removed {
            tracing::warn!(path = %staging.display(), error = %e, "could not remove a finished staging directory");
        }
    }
    Ok(())
}

/// Gives the staging tree `writer` writes the recorded mode and mtime of
/// its root, flushes it under `PowerLoss`, renames it onto `target`, and
/// settles the root's exact mode and mtime there (M2-05).
///
/// Nothing writes into the root after `finish_root`, so its mtime stays.
/// A root with no recorded meta, from a format version 2 manifest, keeps
/// the mode and mtime the staging directory got, as before version 3.
fn put_in_place(
    writer: &TreeWriter,
    (tree, target): (&Path, &Path),
    root: Option<PathMeta>,
    durability: Durability,
    before_rename: Option<&str>,
) -> Result<(), RestoreError> {
    if let Some(meta) = root {
        writer.finish_root(meta)?;
    }
    if durability == Durability::PowerLoss {
        sync_tree(tree, false)?;
    }
    if let Some(name) = before_rename {
        checkpoint::reached(name);
    }
    move_into_place(tree, target)?;
    if let Some(meta) = root {
        writer.settle_root(meta, durability == Durability::PowerLoss)?;
    }
    Ok(())
}

/// Writes a list of entries: directories, then files, then symlinks, then directory modes and mtimes.
///
/// Directories are created owner-only and get their recorded mode last,
/// deepest first, so a directory captured read-only can still be filled.
fn write_entries(
    store: &Store,
    writer: &TreeWriter,
    entries: &[Entry],
    skip: &dyn Fn(&RelPath) -> bool,
    durability: Durability,
    written_here: &mut Written,
    report: &mut RestoreReport,
) -> Result<(), RestoreError> {
    let kept: Vec<&Entry> = entries.iter().filter(|e| !skip(&e.rel_path)).collect();
    for entry in &kept {
        if matches!(entry.kind, EntryKind::Dir) {
            writer.mkdir(&entry.rel_path, Mode::from_masked(0o700))?;
        }
    }
    for entry in &kept {
        if let EntryKind::File { size, chunks } = &entry.kind {
            let mut file = writer.create_file(&entry.rel_path, entry.mode)?;
            let mut written = 0u64;
            for chunk in chunks {
                let bytes = store.objects().read(*chunk)?;
                file.write_all(&bytes)
                    .map_err(|e| io_err(Path::new(entry.rel_path.as_str()), e))?;
                written += bytes.len() as u64;
            }
            if written != *size {
                return Err(RestoreError::SizeMismatch {
                    path: entry.rel_path.clone(),
                    written,
                    size: *size,
                });
            }
            // The mtime goes on before the flush, so the flush carries it too.
            writer.set_mtime(&entry.rel_path, entry.mtime_ns)?;
            if durability == Durability::PowerLoss {
                // Plain fsync(2); the parent directory's F_FULLFSYNC after the
                // rename flushes the device cache once for all of them.
                rustix::fs::fsync(&file)
                    .map_err(|e| io_err(Path::new(entry.rel_path.as_str()), e.into()))?;
            }
            drop(file);
            written_here.add(written);
            if written_here.files == 1 {
                checkpoint::reached("restore.after_first_file");
            }
        }
    }
    for entry in &kept {
        if let EntryKind::Symlink { target } = &entry.kind {
            writer.symlink(&entry.rel_path, target)?;
            writer.set_mtime(&entry.rel_path, entry.mtime_ns)?;
            if symlink_escapes(&entry.rel_path, target) {
                report.escaping_symlinks.push(entry.rel_path.clone());
            }
            written_here.add(0);
        }
    }
    let sync = durability == Durability::PowerLoss;
    for entry in kept.iter().rev() {
        if matches!(entry.kind, EntryKind::Dir) {
            writer.finish_dir(&entry.rel_path, entry.mode, entry.mtime_ns, sync)?;
        }
    }
    Ok(())
}

/// Flushes every directory under `root`, and `root` itself, deepest first, without following links; with `files`, every regular file too.
///
/// The files restore writes are flushed as they are written, but a file's
/// flush does not make its name durable: the directory that holds it must
/// be flushed too, before the rename that publishes the tree. A tree an
/// earlier run left behind, such as a harness directory a rerun keeps, was
/// maybe written without any flush, so its files are flushed as well. A
/// directory this cannot open is one `write_entries` gave a recorded mode
/// without read permission, and it flushed that one already, children first.
fn sync_tree(root: &Path, files: bool) -> Result<(), RestoreError> {
    use rustix::fs::{Mode as RawBits, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match rustix::fs::open(root, flags, RawBits::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::ACCESS) => return Ok(()),
        Err(e) => return Err(io_err(root, e.into())),
    };
    for entry in fs::read_dir(root).map_err(|e| io_err(root, e))? {
        let entry = entry.map_err(|e| io_err(root, e))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| io_err(&path, e))?;
        if kind.is_dir() {
            sync_tree(&path, files)?;
        } else if files && kind.is_file() {
            let file_flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let file = rustix::fs::open(&path, file_flags, RawBits::empty())
                .map_err(|e| io_err(&path, e.into()))?;
            rustix::fs::fsync(&file).map_err(|e| io_err(&path, e.into()))?;
        }
    }
    rustix::fs::fsync(&fd).map_err(|e| io_err(root, e.into()))
}

/// Re-reads written entries against the store and returns how many matched.
///
/// A file is compared byte for byte, a link by its target, and a directory
/// must be a directory, so a removed empty directory or a replaced link
/// fails like a changed file.
fn verify_entries(
    store: &Store,
    writer: &TreeWriter,
    entries: &[Entry],
    skip: &dyn Fn(&RelPath) -> bool,
    mode: Verify,
) -> Result<u64, RestoreError> {
    let kept: Vec<&Entry> = entries.iter().filter(|e| !skip(&e.rel_path)).collect();
    let mut verified = 0;
    for entry in sample(&kept, mode) {
        let mismatch = || RestoreError::VerifyFailed {
            path: entry.rel_path.clone(),
        };
        let chunks = match &entry.kind {
            EntryKind::File { chunks, .. } => chunks,
            EntryKind::Dir => {
                if !writer.is_dir(&entry.rel_path).map_err(|_| mismatch())? {
                    return Err(mismatch());
                }
                verified += 1;
                continue;
            }
            EntryKind::Symlink { target } => {
                if writer.read_link(&entry.rel_path).map_err(|_| mismatch())? != *target {
                    return Err(mismatch());
                }
                verified += 1;
                continue;
            }
        };
        let mut file = writer.open_file(&entry.rel_path).map_err(|_| mismatch())?;
        let mut buf = Vec::new();
        for chunk in chunks {
            let expected = store.objects().read(*chunk)?;
            buf.clear();
            buf.resize(expected.len(), 0);
            file.read_exact(&mut buf).map_err(|_| mismatch())?;
            if buf != expected {
                return Err(mismatch());
            }
        }
        let mut rest = [0u8; 1];
        if file.read(&mut rest).map_err(|_| mismatch())? != 0 {
            return Err(mismatch());
        }
        verified += 1;
    }
    Ok(verified)
}

/// The longest symlink target restore accepts; longer ones no filesystem can hold.
const SYMLINK_TARGET_LIMIT: u64 = 4096;

/// The kind of one `git ls-tree` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeMode {
    /// A regular file, `100644`.
    File,
    /// An executable file, `100755`.
    Exec,
    /// A symbolic link, `120000`.
    Symlink,
    /// A submodule commit, `160000`.
    Gitlink,
}

impl TreeMode {
    fn parse(text: &str) -> Option<TreeMode> {
        match text {
            "100644" => Some(TreeMode::File),
            "100755" => Some(TreeMode::Exec),
            "120000" => Some(TreeMode::Symlink),
            "160000" => Some(TreeMode::Gitlink),
            _ => None,
        }
    }
}

/// One entry of `git ls-tree -r`.
struct TreeItem {
    mode: TreeMode,
    oid: GitOid,
    rel: RelPath,
}

fn parse_tree(bytes: &[u8]) -> Result<Vec<TreeItem>, RestoreError> {
    let mut items = Vec::new();
    for record in Git::records(bytes) {
        // A name that is not UTF-8 is refused, not rewritten: a lossy
        // decode would write, and verify, a file under another name.
        let Ok(text) = std::str::from_utf8(record) else {
            return Err(RestoreError::BadTreeOutput {
                text: String::from_utf8_lossy(record).into_owned(),
            });
        };
        let bad = || RestoreError::BadTreeOutput {
            text: text.to_owned(),
        };
        let (meta, path) = text.split_once('\t').ok_or_else(bad)?;
        let mut fields = meta.split(' ');
        let mode = fields.next().and_then(TreeMode::parse).ok_or_else(bad)?;
        let _kind = fields.next().ok_or_else(bad)?;
        let oid = fields
            .next()
            .and_then(|o| GitOid::parse(o).ok())
            .ok_or_else(bad)?;
        let rel = RelPath::parse(path).map_err(|source| RestoreError::BadTreePath {
            path: path.to_owned(),
            source,
        })?;
        items.push(TreeItem { mode, oid, rel });
    }
    Ok(items)
}

/// One blob header of `git cat-file --batch`.
struct BlobHeader {
    oid: GitOid,
    size: u64,
}

fn parse_blob_header(header: &str) -> Result<BlobHeader, RestoreError> {
    let bad = || RestoreError::BadTreeOutput {
        text: header.trim().to_owned(),
    };
    let mut fields = header.split_whitespace();
    let oid = fields
        .next()
        .and_then(|o| GitOid::parse(o).ok())
        .ok_or_else(bad)?;
    let kind = fields.next().ok_or_else(bad)?;
    if kind == "missing" {
        return Err(bad());
    }
    let size: u64 = fields.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
    Ok(BlobHeader { oid, size })
}

/// Reads one `cat-file --batch` stream and hands each blob to `on_blob` as a
/// bounded reader, in the order of `expected`, never holding a blob in memory.
///
/// Whatever `on_blob` leaves unread is skipped, so the stream stays in step.
fn read_blob_stream(
    reader: &mut impl BufRead,
    expected: &[GitOid],
    mut on_blob: impl FnMut(usize, u64, &mut dyn Read) -> Result<(), RestoreError>,
) -> Result<(), RestoreError> {
    let pipe = Path::new("cat-file");
    let mut header = String::new();
    for (index, oid) in expected.iter().enumerate() {
        header.clear();
        if reader.read_line(&mut header).map_err(|e| io_err(pipe, e))? == 0 {
            return Err(RestoreError::BadTreeOutput {
                text: format!("stream ended before {oid}"),
            });
        }
        let blob = parse_blob_header(&header)?;
        if blob.oid != *oid {
            return Err(RestoreError::BadTreeOutput {
                text: format!("asked for {oid}, got {}", blob.oid),
            });
        }
        let mut body = reader.by_ref().take(blob.size);
        on_blob(index, blob.size, &mut body)?;
        io::copy(&mut body, &mut io::sink()).map_err(|e| io_err(pipe, e))?;
        let mut newline = [0u8; 1];
        reader
            .read_exact(&mut newline)
            .map_err(|e| io_err(pipe, e))?;
    }
    Ok(())
}

/// Streams the blobs `oids` out of `git cat-file --batch`, in that order.
fn cat_file_batch(
    git: &Git,
    oids: &[GitOid],
    on_blob: impl FnMut(usize, u64, &mut dyn Read) -> Result<(), RestoreError>,
) -> Result<(), RestoreError> {
    let pipe = Path::new("cat-file");
    let mut child = git.spawn_piped(&["cat-file", "--batch"])?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        let source = io::Error::other("cat-file started without its stdin or stdout pipe");
        return Err(GitError::Spawn { source }.into());
    };
    let requests: Vec<String> = oids.iter().map(|o| format!("{o}\n")).collect();
    let feeder = thread::spawn(move || {
        for request in requests {
            if stdin.write_all(request.as_bytes()).is_err() {
                break;
            }
        }
    });
    let mut reader = BufReader::new(stdout);
    let outcome = read_blob_stream(&mut reader, oids, on_blob);
    if outcome.is_err() {
        // git blocks on a full stdout pipe and the feeder on a full stdin
        // pipe; closing our end of stdout and killing git frees both.
        drop(reader);
        let _ = child.kill();
    }
    let _ = feeder.join();
    let status = child.wait().map_err(|e| io_err(pipe, e))?;
    outcome?;
    if !status.success() {
        return Err(GitError::Failed {
            command: "cat-file --batch".to_owned(),
            code: status.code(),
            stderr: String::new(),
        }
        .into());
    }
    Ok(())
}

/// The staging directory a restore writes into, with its writer and the crash it must survive.
#[derive(Clone, Copy)]
struct StagingTarget<'a> {
    staging: &'a Path,
    writer: &'a TreeWriter,
    durability: Durability,
}

/// The snapshots of the bundle chain that ends at `snapshot`, whose
/// manifest is `manifest`, oldest first, from the catalog's `bundle_base`
/// of each, so no other manifest is read.
fn chain_ids(
    store: &Store,
    snapshot: SnapshotId,
    manifest: &SnapshotManifest,
) -> Result<Vec<SnapshotId>, RestoreError> {
    let mut ids = vec![snapshot];
    let mut base = manifest.git.as_ref().and_then(|g| g.bundle_base);
    while let Some(id) = base {
        // Manifest ids are hashes, so only a damaged catalog holds a cycle.
        if ids.contains(&id) {
            return Err(StoreError::ManifestMismatch(id).into());
        }
        match store.snapshot(id) {
            Ok(row) => base = row.bundle_base,
            Err(StoreError::UnknownSnapshot(_)) => {
                return Err(RestoreError::BundleChainBroken { snapshot, base: id });
            }
            Err(e) => return Err(e.into()),
        }
        ids.push(id);
    }
    ids.reverse();
    Ok(ids)
}

/// Puts the objects of every bundle of the chain that ends at `manifest`,
/// oldest first, into the repository `git` runs in at `root`.
///
/// The node's bundle cache (`crate::bundle_cache`) gives the pack files of
/// the links it holds, each checked against its recorded digest, and only
/// the links after them are unpacked, so a resume no longer grows with the
/// chain. Unbundling indexes each pack and writes no refs, so the refs an
/// older link lists can never collide with the ones `set_refs` writes.
/// `git bundle unbundle` checks the bundle's prerequisites with the same
/// `verify_bundle` that `git bundle verify` runs, before it indexes the
/// pack (git's `bundle.c`, since 2.20), and the repository holds exactly
/// the links below, from the cache or unpacked, so a link whose
/// prerequisites they lack fails here, and no separate verify runs.
fn unbundle_chain(
    store: &Store,
    (snapshot, manifest): (SnapshotId, &SnapshotManifest),
    (git, root): (&Git, &Path),
    durability: Durability,
    report: &mut RestoreReport,
) -> Result<(), RestoreError> {
    let ids = chain_ids(store, snapshot, manifest)?;
    let mut cache = BundleCache::open(store.root(), manifest.job_id);
    let held = cache.reuse(&ids);
    let pack_dir = git_dir(root).join("objects").join("pack");
    for index in 0..held {
        for (name, from) in cache.files(index) {
            let to = pack_dir.join(name);
            bundle_cache::place(&from, &to).map_err(|e| io_err(&to, e))?;
            if durability == Durability::PowerLoss {
                let file = fs::File::open(&to).map_err(|e| io_err(&to, e))?;
                rustix::fs::fsync(&file).map_err(|e| io_err(&to, e.into()))?;
            }
        }
        report.bundles_fetched += u32::from(cache.has_bundle(index));
    }
    let mut caching = true;
    for (index, id) in ids.iter().enumerate().skip(held) {
        let decoded;
        let link = if *id == snapshot {
            manifest
        } else {
            decoded = store.manifest(*id)?;
            &decoded
        };
        let state = link.git.as_ref();
        // The catalog's chain must be the manifests' own.
        if state.and_then(|g| g.bundle_base) != index.checked_sub(1).map(|below| ids[below]) {
            return Err(StoreError::ManifestMismatch(*id).into());
        }
        let bundle = state.map_or(&[][..], |g| g.bundle.as_slice());
        let mut written = Vec::new();
        if !bundle.is_empty() {
            let before = pack_files(&pack_dir)?;
            unbundle(store, bundle, git)?;
            written = pack_files(&pack_dir)?
                .into_iter()
                .filter(|name| !before.contains(name))
                .map(|name| pack_dir.join(name))
                .collect();
            report.bundles_fetched += 1;
            report.bundles_unpacked += 1;
        }
        if caching && let Err(e) = cache.add(*id, !bundle.is_empty(), &written) {
            tracing::warn!(error = %e, "could not add a link to the bundle cache");
            caching = false;
        }
    }
    cache.touch_and_evict();
    Ok(())
}

/// The names of the files in a repository's `objects/pack`.
fn pack_files(dir: &Path) -> Result<BTreeSet<String>, RestoreError> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        if let Ok(name) = entry.file_name().into_string() {
            names.insert(name);
        }
    }
    Ok(names)
}

/// Writes the bundle of `chunks` to a scratch file and unpacks it with `git bundle unbundle`.
fn unbundle(store: &Store, chunks: &[ChunkId], git: &Git) -> Result<(), RestoreError> {
    let bundle = TempFile::in_dir(store.objects().tmp_dir());
    let path = bundle.path();
    {
        let mut file = fs::File::create(path).map_err(|e| io_err(path, e))?;
        for chunk in chunks {
            file.write_all(&store.objects().read(*chunk)?)
                .map_err(|e| io_err(path, e))?;
        }
    }
    let path_text = path.to_string_lossy().into_owned();
    git.run(&["bundle", "unbundle", &path_text])?;
    Ok(())
}

/// Adds `oid` as the newest stash entry.
fn push_stash_entry(git: &Git, oid: &GitOid) -> Result<(), RestoreError> {
    // `git stash list` prints the reflog message, so the entry gets the
    // stash commit's own subject back.
    let subject = git.run_text(&["log", "-1", "--format=%s", oid.as_str()])?;
    git.run(&[
        "update-ref",
        "--create-reflog",
        "-m",
        &subject,
        "refs/stash",
        oid.as_str(),
    ])?;
    Ok(())
}

/// Sets every ref, every symbolic ref and `HEAD` exactly as `git_state` records them.
///
/// One `update-ref --no-deref --stdin` transaction sets every ref but
/// `refs/stash` and points each symbolic ref at its target with
/// `symref-update`, so `refs/remotes/origin/HEAD` follows the remote's
/// default branch again instead of naming a frozen commit (M2-06). `HEAD`
/// already points at the snapshot's branch, where `init_repo` put it, or
/// for a detached `HEAD` the same transaction detaches it; `--no-deref`
/// keeps that update on `HEAD` instead of the branch `git init` left it
/// on. git before 2.46 has no `symref-update`; there each symbolic ref
/// takes one `git symbolic-ref` after the transaction of the direct refs,
/// as before. The stash list is then rebuilt oldest first from the refs
/// capture pinned under the job's stash prefix, then `refs/stash` itself,
/// so every entry comes back at its index.
fn set_refs(git: &Git, git_state: &GitState, job_id: JobId) -> Result<(), RestoreError> {
    let mut direct = String::new();
    let mut stash = None;
    for (name, oid) in &git_state.refs {
        if name.as_str() == "refs/stash" {
            stash = Some(oid);
        } else {
            direct.extend(["update ", name.as_str(), " ", oid.as_str(), "\n"]);
        }
    }
    let mut commands = direct.clone();
    // `validate` holds each target in `refs`, which the same transaction sets.
    for (name, target) in &git_state.symbolic_refs {
        commands.extend(["symref-update ", name.as_str(), " ", target.as_str(), "\n"]);
    }
    // git refuses a transaction that updates `HEAD` and also the branch
    // `HEAD` points at, so `HEAD` joins it only when detached, from the
    // branch of `initial_branch`, which no ref of the snapshot names.
    if let GitHead::Detached(oid) = &git_state.head {
        commands.extend(["update HEAD ", oid.as_str(), "\n"]);
    }
    if !commands.is_empty() && !git.update_refs_with_symrefs(commands.as_bytes())? {
        set_refs_one_by_one(git, git_state, &direct)?;
    }
    if let Some(oid) = stash {
        let prefix = stash_prefix(job_id);
        let mut older: Vec<(u64, &GitOid)> = git_state
            .refs
            .iter()
            .filter_map(|(name, oid)| {
                let n = name.as_str().strip_prefix(&prefix)?.parse().ok()?;
                Some((n, oid))
            })
            .collect();
        older.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        for (_, entry) in &older {
            push_stash_entry(git, entry)?;
        }
        push_stash_entry(git, oid)?;
    }
    Ok(())
}

/// Sets the refs of `set_refs` on git before 2.46: the direct ones in one
/// transaction of `direct`, then one `symbolic-ref` per symbolic ref, then
/// a detached `HEAD`; a branch `HEAD` is where `init_repo` put it.
fn set_refs_one_by_one(git: &Git, git_state: &GitState, direct: &str) -> Result<(), RestoreError> {
    if !direct.is_empty() {
        git.run_stdin_env(&["update-ref", "--stdin"], &[], direct.as_bytes())?;
    }
    for (name, target) in &git_state.symbolic_refs {
        git.run(&["symbolic-ref", name.as_str(), target.as_str()])?;
    }
    if let GitHead::Detached(oid) = &git_state.head {
        git.run(&["update-ref", "--no-deref", "HEAD", oid.as_str()])?;
    }
    Ok(())
}

/// The branch `init_repo` has `git init` point `HEAD` at.
///
/// A branch `HEAD` gets its own branch, so `HEAD` is where the snapshot's
/// is from the start and the ref transaction need not touch it. A detached
/// `HEAD` gets a branch no ref of the snapshot names, so the transaction
/// can detach `HEAD` without also moving the branch `HEAD` points at: git
/// refuses that ("multiple updates for 'HEAD' (including one via its
/// referent ...)"), and the `master` a plain `git init` points at is a
/// branch of many repositories. That branch is never created.
fn initial_branch(git_state: &GitState) -> String {
    match &git_state.head {
        GitHead::Branch(branch) => branch.as_str().to_owned(),
        GitHead::Detached(_) => {
            let taken = |name: &str| {
                let full = format!("refs/heads/{name}");
                (git_state.refs.iter().map(|(name, _)| name))
                    .chain(git_state.symbolic_refs.iter().map(|(name, _)| name))
                    .any(|name| name.as_str() == full)
            };
            let mut name = "fabric-detached".to_owned();
            let mut n = 0u32;
            while taken(&name) {
                n += 1;
                name = format!("fabric-detached-{n}");
            }
            name
        }
    }
}

/// The git directory `git init` makes in `root`: no `GIT_*` variable
/// reaches git (`crate::git`), and the global and system config, where
/// `init.templateDir` could be set, are unread, so it is `root/.git`.
fn git_dir(root: &Path) -> PathBuf {
    root.join(".git")
}

/// Creates the empty repository in the object format of `git_state`, its
/// `HEAD` on `initial_branch`, and writes its exclude file.
///
/// The exclude file's place is known from `git_dir`, so no `git rev-parse
/// --git-path` asks for it. `--initial-branch` is from git 2.28, older
/// than the `--object-format` of 2.29 this already needs.
fn init_repo(
    store: &Store,
    (git, root): (&Git, &Path),
    git_state: &GitState,
    power_loss: bool,
) -> Result<(), RestoreError> {
    // A SHA-256 repository cannot unpack into a SHA-1 one; the manifest
    // holds one object format, read from the width of its ids.
    let format = if git_state.index_commit.is_sha256() {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    };
    let initial = format!("--initial-branch={}", initial_branch(git_state));
    git.run(&["init", "-q", format, &initial])?;
    if let Some(chunks) = &git_state.exclude {
        let path = git_dir(root).join("info").join("exclude");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        let mut file = fs::File::create(&path).map_err(|e| io_err(&path, e))?;
        for chunk in chunks {
            file.write_all(&store.objects().read(*chunk)?)
                .map_err(|e| io_err(&path, e))?;
        }
        if power_loss {
            rustix::fs::fsync(&file).map_err(|e| io_err(&path, e.into()))?;
        }
    }
    Ok(())
}

/// Counts the working-tree files restore writes: a path of `W` the index
/// also holds is tracked, the rest is untracked work.
struct WorkCounter {
    indexed: BTreeSet<RelPath>,
    tracked: Written,
    untracked: Written,
}

impl WorkCounter {
    fn new(git: &Git, index_commit: &GitOid) -> Result<WorkCounter, RestoreError> {
        let listing = git.run(&["ls-tree", "-r", "-z", index_commit.as_str()])?;
        Ok(WorkCounter {
            indexed: parse_tree(&listing)?
                .into_iter()
                .map(|item| item.rel)
                .collect(),
            tracked: Written::default(),
            untracked: Written::default(),
        })
    }

    fn add(&mut self, rel: &RelPath, bytes: u64) {
        if self.indexed.contains(rel) {
            self.tracked.add(bytes);
        } else {
            self.untracked.add(bytes);
        }
    }
}

/// The mode each file of `W` is written with: the one `worktree_modes`
/// records (M2-04), or for a path it does not name, as in a format version
/// 2 manifest, 0644 or 0755 as the tree entry says.
///
/// A recorded mode is written as it is, so its owner execute bit must be
/// the one the tree records; capture makes them agree, and a manifest
/// where they differ is refused here, before any file is written.
fn file_modes<'i>(
    items: &'i [TreeItem],
    recorded: &[(RelPath, Mode)],
) -> Result<HashMap<&'i RelPath, Mode>, RestoreError> {
    let mut modes = HashMap::with_capacity(items.len());
    for item in items {
        let executable = match item.mode {
            TreeMode::File => false,
            TreeMode::Exec => true,
            TreeMode::Symlink | TreeMode::Gitlink => continue,
        };
        let mode = match recorded.binary_search_by(|(p, _)| p.cmp(&item.rel)) {
            Ok(index) => recorded[index].1,
            Err(_) => Mode::from_masked(if executable { 0o755 } else { 0o644 }),
        };
        if mode.owner_executable() != executable {
            return Err(RestoreError::ModeDisagreesWithTree {
                path: item.rel.clone(),
                mode,
            });
        }
        modes.insert(&item.rel, mode);
    }
    Ok(modes)
}

/// Restores the repository: objects, refs, `HEAD`, the index and the working tree.
fn restore_git(
    store: &Store,
    snapshot: SnapshotId,
    manifest: &SnapshotManifest,
    git_state: &GitState,
    target: &StagingTarget<'_>,
    report: &mut RestoreReport,
) -> Result<(Git, Vec<TreeItem>), RestoreError> {
    let StagingTarget {
        staging,
        writer,
        durability,
    } = *target;
    let power_loss = durability == Durability::PowerLoss;
    let mut git = Git::new(staging).with_date(manifest.created_at);
    if power_loss {
        git = git.with_full_fsync();
    }
    init_repo(store, (&git, staging), git_state, power_loss)?;

    unbundle_chain(
        store,
        (snapshot, manifest),
        (&git, staging),
        durability,
        report,
    )?;
    set_refs(&git, git_state, manifest.job_id)?;
    git.run(&["read-tree", git_state.index_commit.as_str()])?;

    let listing = git.run(&["ls-tree", "-r", "-z", git_state.worktree_commit.as_str()])?;
    let items = parse_tree(&listing)?;
    let mut counted = WorkCounter::new(&git, &git_state.index_commit)?;
    let mtimes: HashMap<&RelPath, u64> = git_state
        .worktree_mtimes
        .iter()
        .map(|(p, t)| (p, *t))
        .collect();
    let modes = file_modes(&items, &git_state.worktree_modes)?;
    let mut blobs: Vec<&TreeItem> = Vec::with_capacity(items.len());
    for item in &items {
        if item.mode == TreeMode::Gitlink {
            writer.mkdir(&item.rel, Mode::from_masked(0o755))?;
            report.submodules.push(item.rel.clone());
        } else {
            blobs.push(item);
        }
    }
    let oids: Vec<GitOid> = blobs.iter().map(|item| item.oid.clone()).collect();
    let mut symlinks: Vec<(&TreeItem, String)> = Vec::new();
    cat_file_batch(&git, &oids, |index, size, body| {
        let item = blobs[index];
        let path = Path::new(item.rel.as_str());
        match item.mode {
            TreeMode::Symlink => {
                if size > SYMLINK_TARGET_LIMIT {
                    return Err(RestoreError::SymlinkTooLong {
                        path: item.rel.clone(),
                        size,
                    });
                }
                let mut target = String::new();
                body.read_to_string(&mut target)
                    .map_err(|e| io_err(path, e))?;
                symlinks.push((item, target));
            }
            TreeMode::File | TreeMode::Exec | TreeMode::Gitlink => {
                let mode = modes
                    .get(&item.rel)
                    .copied()
                    .unwrap_or(Mode::from_masked(0o644));
                let mut file = writer.create_file(&item.rel, mode)?;
                let written = io::copy(body, &mut file).map_err(|e| io_err(path, e))?;
                if written != size {
                    return Err(RestoreError::SizeMismatch {
                        path: item.rel.clone(),
                        written,
                        size,
                    });
                }
                if let Some(mtime) = mtimes.get(&item.rel) {
                    writer.set_mtime(&item.rel, *mtime)?;
                }
                if power_loss {
                    rustix::fs::fsync(&file).map_err(|e| io_err(path, e.into()))?;
                }
                drop(file);
                counted.add(&item.rel, written);
            }
        }
        Ok(())
    })?;
    for (item, target) in symlinks {
        writer.symlink(&item.rel, &target)?;
        if let Some(mtime) = mtimes.get(&item.rel) {
            writer.set_mtime(&item.rel, *mtime)?;
        }
        if symlink_escapes(&item.rel, &target) {
            report.escaping_symlinks.push(item.rel.clone());
        }
        counted.add(&item.rel, 0);
    }
    report.tracked = counted.tracked;
    report.untracked = counted.untracked;
    // Nonzero whenever a file differs from the index, which is expected here.
    let _ = git.output(&["update-index", "--refresh"], &[])?;
    Ok((git, items))
}

/// Picks about `n` of `items`, spread over the list, or all or none.
fn sample<'a, T>(items: &[&'a T], mode: Verify) -> Vec<&'a T> {
    match mode {
        Verify::None => Vec::new(),
        Verify::All => items.to_vec(),
        Verify::Sample(n) => {
            if n == 0 || items.is_empty() {
                Vec::new()
            } else {
                let step = (items.len() / n).max(1);
                items.iter().step_by(step).copied().collect()
            }
        }
    }
}

/// The most path bytes one `hash-object` command line carries, well under every `ARG_MAX`.
const ARG_BYTES: usize = 128 << 10;

/// Splits `items` into runs whose paths together stay under `ARG_BYTES`.
fn path_batches<'a, 'b>(items: &'b [&'a TreeItem]) -> Vec<&'b [&'a TreeItem]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (index, item) in items.iter().enumerate() {
        let len = item.rel.as_str().len() + 1;
        if index > start && bytes + len > ARG_BYTES {
            batches.push(&items[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += len;
    }
    if start < items.len() {
        batches.push(&items[start..]);
    }
    batches
}

/// Re-reads the working tree against `items`, the entries of commit `W`, and returns how many matched.
///
/// Files are hashed by `git hash-object --no-filters` and compared with the
/// tree's blob ids; links are compared with their blobs' bytes.
fn verify_worktree(
    git: &Git,
    items: &[TreeItem],
    writer: &TreeWriter,
    mode: Verify,
) -> Result<u64, RestoreError> {
    let blobs: Vec<&TreeItem> = items
        .iter()
        .filter(|item| item.mode != TreeMode::Gitlink)
        .collect();
    let chosen = sample(&blobs, mode);
    let mismatch = |item: &TreeItem| RestoreError::VerifyFailed {
        path: item.rel.clone(),
    };
    let mut verified = 0;
    let mut by_path: Vec<&TreeItem> = Vec::new();
    let mut links: Vec<&TreeItem> = Vec::new();
    for item in chosen {
        match item.mode {
            TreeMode::Symlink => links.push(item),
            TreeMode::File | TreeMode::Exec => {
                // A missing or replaced file is a verify failure, not a git error.
                writer.open_file(&item.rel).map_err(|_| mismatch(item))?;
                by_path.push(item);
            }
            TreeMode::Gitlink => {}
        }
    }
    // Paths go as arguments: `--stdin-paths` would C-unquote a name that
    // starts with '"' and drop a trailing '\r', as in macOS's `Icon\r`.
    for batch in path_batches(&by_path) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend(batch.iter().map(|item| item.rel.as_str()));
        let out = git.run(&args)?;
        let text = String::from_utf8_lossy(&out);
        let mut hashes = text.lines();
        for item in batch {
            if hashes.next() != Some(item.oid.as_str()) {
                return Err(mismatch(item));
            }
            verified += 1;
        }
    }
    if !links.is_empty() {
        let oids: Vec<GitOid> = links.iter().map(|item| item.oid.clone()).collect();
        cat_file_batch(git, &oids, |index, size, body| {
            let item = links[index];
            if size > SYMLINK_TARGET_LIMIT {
                return Err(mismatch(item));
            }
            let mut target = String::new();
            body.read_to_string(&mut target)
                .map_err(|_| mismatch(item))?;
            if writer.read_link(&item.rel).map_err(|_| mismatch(item))? != target {
                return Err(mismatch(item));
            }
            verified += 1;
            Ok(())
        })?;
    }
    Ok(verified)
}

/// Adds every path under `base/prefix` to `out`, relative to `base`, without following links.
///
/// A name that is not UTF-8 is added with its bad bytes replaced, so it
/// matches no manifest path.
fn paths_under(base: &Path, prefix: &str, out: &mut BTreeSet<String>) -> Result<(), RestoreError> {
    let dir = base.join(prefix);
    for entry in fs::read_dir(&dir).map_err(|e| io_err(&dir, e))? {
        let entry = entry.map_err(|e| io_err(&dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry
            .file_type()
            .map_err(|e| io_err(&entry.path(), e))?
            .is_dir()
        {
            paths_under(base, &rel, out)?;
        }
        out.insert(rel);
    }
    Ok(())
}

/// Says whether `dir` already holds the harness state of `manifest`.
///
/// A restore renames the harness state into place before the workspace, so
/// a kill between the two renames leaves a complete harness directory next
/// to a half-restored workspace. The rerun checks that the directory holds
/// exactly the manifest's paths, re-reads every file against the store and
/// checks every directory and link; a directory that holds anything else
/// is foreign and is refused. The directory itself then gets the recorded
/// mode and mtime, which a kill just after the rename may have left with
/// the owner bits `TreeWriter::finish_root` added.
fn harness_state_complete(
    store: &Store,
    state: &HarnessState,
    dir: &Path,
) -> Result<Option<u64>, RestoreError> {
    let entries = &state.entries;
    let occupied = RestoreError::TargetNotEmpty {
        path: dir.to_owned(),
    };
    match restore_state(dir)? {
        RestoreState::Absent | RestoreState::HalfRestored { .. } => return Ok(None),
        RestoreState::Present => {}
    }
    let mut found = BTreeSet::new();
    paths_under(dir, "", &mut found)?;
    let expected: BTreeSet<String> = entries
        .iter()
        .map(|e| e.rel_path.as_str().to_owned())
        .collect();
    if entries.is_empty() || found != expected {
        return Err(occupied);
    }
    for entry in entries {
        let full = dir.join(entry.rel_path.as_str());
        let Ok(meta) = fs::symlink_metadata(&full) else {
            return Err(occupied);
        };
        let same = match &entry.kind {
            EntryKind::Dir => meta.is_dir(),
            EntryKind::Symlink { target } => {
                fs::read_link(&full).is_ok_and(|t| t.as_os_str() == OsStr::new(target))
            }
            EntryKind::File { .. } => meta.is_file(),
        };
        if !same {
            return Err(occupied);
        }
    }
    let writer = TreeWriter::open(dir)?;
    let verified = match verify_entries(store, &writer, entries, &|_| false, Verify::All) {
        Ok(verified) => verified,
        Err(RestoreError::VerifyFailed { .. }) => return Err(occupied),
        Err(e) => return Err(e),
    };
    if let Some(root) = state.root {
        writer.settle_root(root, false)?;
    }
    Ok(Some(verified))
}

/// The cache paths whose bytes are not written: rows nothing of which was captured, and rows `carry` turns down.
///
/// A `Partial` cache had a secret or an excluded path left out, and is
/// written when `carry` says so. The caller weighs the hole: written, the
/// cache may look whole to the tool that reads it, and left out, a cache
/// nothing on the target can make again is lost. fabricd's carry decision
/// rebuilds a partial cache a setup step fills and writes one nothing can
/// fill, with a warning (ADR 0004, 2026-10-21), and the store writes
/// exactly what it decides.
fn skipped_caches(
    manifest: &SnapshotManifest,
    carry: &dyn Fn(&CacheEntry) -> bool,
) -> Vec<RelPath> {
    manifest
        .caches
        .iter()
        .filter(|c| c.carry == CarryState::NotCarried || !carry(c))
        .map(|c| c.path.clone())
        .collect()
}

/// Materializes a snapshot into `req.target`.
///
/// The rename of the workspace staging directory is the commit point. The
/// harness state is written and renamed just before it, and a rerun after a
/// kill between the two renames finds the harness state complete and
/// finishes the workspace.
pub fn restore(store: &Store, req: &RestoreRequest<'_>) -> Result<RestoreReport, RestoreError> {
    let manifest = store.manifest(req.snapshot)?;
    if let Some(dir) = req.harness_state {
        refuse_nested(req.target, dir)?;
    }
    check_target_empty(req.target)?;
    let harness_done = match req.harness_state {
        Some(dir) => harness_state_complete(store, &manifest.harness_state, dir)?,
        None => None,
    };
    let staging = fresh_staging(req.target, req.durability)?;
    let writer = TreeWriter::open(&staging)?;
    let mut report = RestoreReport::default();

    let restored = match &manifest.git {
        Some(state) => Some(restore_git(
            store,
            req.snapshot,
            &manifest,
            state,
            &StagingTarget {
                staging: &staging,
                writer: &writer,
                durability: req.durability,
            },
            &mut report,
        )?),
        None => None,
    };
    checkpoint::reached("restore.after_git");

    let skipped = skipped_caches(&manifest, req.carry);
    let skip = |rel: &RelPath| skipped.iter().any(|root| rel.starts_with(root));
    let mut ignored = Written::default();
    write_entries(
        store,
        &writer,
        &manifest.files,
        &skip,
        req.durability,
        &mut ignored,
        &mut report,
    )?;
    report.ignored = ignored;
    report.skipped_caches.clone_from(&skipped);
    checkpoint::reached("restore.after_files");
    report.verified += verify_entries(store, &writer, &manifest.files, &skip, req.verify)?;
    if let Some((git, items)) = &restored {
        report.verified += verify_worktree(git, items, &writer, req.verify)?;
    }

    match (req.harness_state, harness_done) {
        (Some(dir), Some(verified)) => {
            // A kept harness directory may come from a ProcessCrash run.
            if req.durability == Durability::PowerLoss {
                sync_tree(dir, true)?;
            }
            report.verified += verified;
        }
        (Some(dir), None) => {
            let hs_staging = fresh_staging(dir, req.durability)?;
            let hs_writer = TreeWriter::open(&hs_staging)?;
            let keep_all = |_: &RelPath| false;
            let mut harness = Written::default();
            write_entries(
                store,
                &hs_writer,
                &manifest.harness_state.entries,
                &keep_all,
                req.durability,
                &mut harness,
                &mut report,
            )?;
            report.harness = harness;
            report.verified += verify_entries(
                store,
                &hs_writer,
                &manifest.harness_state.entries,
                &keep_all,
                req.verify,
            )?;
            put_in_place(
                &hs_writer,
                (&hs_staging, dir),
                manifest.harness_state.root,
                req.durability,
                None,
            )?;
            checkpoint::reached("restore.after_harness_rename");
        }
        (None, _) => {}
    }

    if let (Some((git, _)), Some(state)) = (&restored, &manifest.git) {
        write_config(git, &staging, &state.config, req.durability)?;
    }
    put_in_place(
        &writer,
        (&staging, req.target),
        manifest.root_meta,
        req.durability,
        Some("restore.before_rename"),
    )?;
    checkpoint::reached("restore.after_rename");
    tracing::info!(snapshot = %req.snapshot, target = %req.target.display(), files = report.total().files, "restored");
    Ok(report)
}

/// Writes the carried config projection, the last step of a git restore (ADR 0004).
///
/// A repeated key's values sit together in the source's order: the first
/// replaces whatever `git init` wrote, and the rest follow it, as `git
/// config --replace-all` and `--add` would leave them.
///
/// `git init` writes a config of `[core]` and, for a SHA-256 repository,
/// `[extensions]`, so every carried key outside `core` belongs to a
/// section the file does not hold yet. Those sections are appended to the
/// file byte for byte as `git config` appends them (`ConfigSection`),
/// without a git process. A `core` key, or a section the file already
/// holds, goes through `git config` as before.
fn write_config(
    git: &Git,
    root: &Path,
    config: &[(GitConfigKey, GitConfigValue)],
    durability: Durability,
) -> Result<(), RestoreError> {
    // These describe the source's filesystem; `git init` probed the target's.
    let carried = config.iter().filter(|(key, _)| {
        !matches!(
            key,
            GitConfigKey::CoreIgnoreCase | GitConfigKey::CoreSymlinks
        )
    });
    let mut sections: Vec<ConfigSection> = Vec::new();
    for (key, value) in carried {
        let name = key.to_string();
        let (header, variable) = ConfigSection::split(&name);
        match sections.iter_mut().find(|s| s.header == header) {
            Some(section) => section
                .lines
                .push((name, variable, value.as_str().to_owned())),
            None => sections.push(ConfigSection {
                header,
                lines: vec![(name, variable, value.as_str().to_owned())],
            }),
        }
    }
    if sections.is_empty() {
        return Ok(());
    }
    let path = &git_dir(root).join("config");
    let existing = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    let held: BTreeSet<&str> = existing
        .lines()
        .filter(|line| line.starts_with('['))
        .collect();
    let (through_git, appended): (Vec<&ConfigSection>, Vec<&ConfigSection>) = (sections.iter())
        .partition(|s| s.header == "[core]\n" || held.contains(s.header.trim_end()));
    if !appended.is_empty() {
        let mut text = String::new();
        if !existing.is_empty() && !existing.ends_with('\n') {
            text.push('\n');
        }
        for section in appended {
            section.write(&mut text);
        }
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| io_err(path, e))?;
        file.write_all(text.as_bytes())
            .map_err(|e| io_err(path, e))?;
        if durability == Durability::PowerLoss {
            rustix::fs::fsync(&file).map_err(|e| io_err(path, e.into()))?;
        }
    }
    for section in through_git {
        let mut previous = None;
        for (name, _, value) in &section.lines {
            let mode = if previous == Some(name) {
                "--add"
            } else {
                "--replace-all"
            };
            git.run(&["config", "--local", mode, name, value])?;
            previous = Some(name);
        }
    }
    Ok(())
}

/// One section of the carried config, with its keys in the projection's order.
struct ConfigSection {
    /// The header line, newline included, as `git config` writes it.
    header: String,
    /// Each key's full name, its variable name and its value.
    lines: Vec<(String, String, String)>,
}

impl ConfigSection {
    /// Splits a key's full name into the header of its section, as git's
    /// `store_create_section` writes it, and its variable name: the
    /// subsection is what lies between the first and the last dot, with
    /// `"` and `\\` escaped.
    fn split(name: &str) -> (String, String) {
        let (section, rest) = name.split_once('.').unwrap_or((name, ""));
        match rest.rsplit_once('.') {
            Some((subsection, variable)) => {
                let mut header = format!("[{section} \"");
                for c in subsection.chars() {
                    if matches!(c, '"' | '\\') {
                        header.push('\\');
                    }
                    header.push(c);
                }
                header.push_str("\"]\n");
                (header, variable.to_owned())
            }
            None => (format!("[{section}]\n"), rest.to_owned()),
        }
    }

    /// Appends the section to `out` as `git config` writes a new section:
    /// the header, then a tab, the variable, ` = ` and the value per line,
    /// the value in double quotes when it starts or ends with a space or
    /// holds `;`, `#` or a carriage return, with `\\`, `"` and a tab
    /// escaped (git's `write_pair`). git quotes a carriage return since
    /// the fix of CVE-2025-48384 (2.50.1, and 2.43.7 on the oldest line it
    /// patched); before, a value that ended in one lost it when read back.
    /// This writer quotes it whatever git the node runs.
    fn write(&self, out: &mut String) {
        out.push_str(&self.header);
        for (_, variable, value) in &self.lines {
            let quote =
                value.starts_with(' ') || value.ends_with(' ') || value.contains([';', '#', '\r']);
            out.push('\t');
            out.push_str(variable);
            out.push_str(" = ");
            if quote {
                out.push('"');
            }
            for c in value.chars() {
                match c {
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '"' | '\\' => {
                        out.push('\\');
                        out.push(c);
                    }
                    _ => out.push(c),
                }
            }
            if quote {
                out.push('"');
            }
            out.push('\n');
        }
    }
}

/// Re-reads the `files` and the working tree of a snapshot under `root` and returns how many matched.
///
/// This is how a caller tells a complete tree from one that was cut short
/// after the rename by a power loss; a half restore before the rename shows
/// as `RestoreState::HalfRestored` instead. The harness state is not covered.
pub fn verify(
    store: &Store,
    snapshot: SnapshotId,
    root: &Path,
    mode: Verify,
    carry: &dyn Fn(&CacheEntry) -> bool,
) -> Result<u64, RestoreError> {
    let manifest = store.manifest(snapshot)?;
    let writer = TreeWriter::open(root)?;
    let skipped = skipped_caches(&manifest, carry);
    let skip = |rel: &RelPath| skipped.iter().any(|root| rel.starts_with(root));
    let mut verified = verify_entries(store, &writer, &manifest.files, &skip, mode)?;
    if let Some(state) = &manifest.git {
        let git = Git::new(root);
        let listing = git.run(&["ls-tree", "-r", "-z", state.worktree_commit.as_str()])?;
        verified += verify_worktree(&git, &parse_tree(&listing)?, &writer, mode)?;
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// Blobs enough to fill both pipes between this process and git.
    const BLOBS: usize = 300;

    /// Writes `config` with one `git config` per key, as restore did before
    /// `write_config` appended the sections itself.
    fn config_through_git(git: &Git, config: &[(GitConfigKey, GitConfigValue)]) {
        let mut previous = None;
        for (key, value) in config {
            if matches!(
                key,
                GitConfigKey::CoreIgnoreCase | GitConfigKey::CoreSymlinks
            ) {
                continue;
            }
            let mode = if previous == Some(key) {
                "--add"
            } else {
                "--replace-all"
            };
            git.run(&["config", "--local", mode, &key.to_string(), value.as_str()])
                .unwrap();
            previous = Some(key);
        }
    }

    /// Restores `config` into two fresh repositories, one through `git
    /// config` and one through `write_config`, and returns both files.
    fn both_configs(config: &[(GitConfigKey, GitConfigValue)]) -> (String, String) {
        let dir = tempfile::tempdir().unwrap();
        let (by_git, written) = (dir.path().join("git"), dir.path().join("written"));
        for root in [&by_git, &written] {
            fs::create_dir(root).unwrap();
            Git::new(root).run(&["init", "-q"]).unwrap();
        }
        config_through_git(&Git::new(&by_git), config);
        write_config(
            &Git::new(&written),
            &written,
            config,
            Durability::ProcessCrash,
        )
        .unwrap();
        (
            fs::read_to_string(git_dir(&by_git).join("config")).unwrap(),
            fs::read_to_string(git_dir(&written).join("config")).unwrap(),
        )
    }

    fn key(text: &str) -> GitConfigKey {
        GitConfigKey::parse(text).unwrap()
    }

    fn value(text: &str) -> GitConfigValue {
        GitConfigValue::parse(text).unwrap()
    }

    /// Every character git's `write_pair` quotes or escapes, a subsection
    /// it escapes, a repeated key, a key of `core` and two the restore
    /// leaves to `git init`.
    #[test]
    fn write_config_leaves_the_bytes_git_config_leaves() {
        let mut config = vec![
            (key("remote.origin.url"), value(" lead and trail ")),
            (
                key("remote.origin.fetch"),
                value("+refs/heads/*:refs/remotes/origin/*"),
            ),
            (
                key("remote.origin.fetch"),
                value("a;b#c \"q\" \\ tab\there"),
            ),
            (key("remote.we\"ird.url"), value("")),
            (key("remote.a.b.pushurl"), value("é = [x]")),
            (key("branch.feat/x.merge"), value("refs/heads/feat/x")),
            (key("branch.main.remote"), value("origin")),
            (key("push.default"), value("simple")),
            (key("core.autocrlf"), value("input")),
            (key("core.ignorecase"), value("false")),
            (key("core.symlinks"), value("false")),
        ];
        config.sort_by(|a, b| a.0.cmp(&b.0));
        let (by_git, written) = both_configs(&config);
        assert_eq!(written, by_git);
    }

    /// A value that holds or ends in a carriage return reads back whole,
    /// whichever git reads it (CVE-2025-48384).
    #[test]
    fn a_carriage_return_in_a_config_value_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::new(dir.path());
        git.run(&["init", "-q"]).unwrap();
        let values = ["ends\r", "\rstarts", "in\rside"];
        let config: Vec<_> = (values.iter())
            .map(|v| (key("remote.origin.fetch"), value(v)))
            .collect();
        write_config(&git, dir.path(), &config, Durability::ProcessCrash).unwrap();
        let read = git
            .run(&["config", "--get-all", "-z", "remote.origin.fetch"])
            .unwrap();
        let read: Vec<&[u8]> = Git::records(&read).collect();
        assert_eq!(read, values.map(str::as_bytes));
    }

    #[test]
    fn write_config_leaves_the_bytes_git_config_leaves_for_any_projection() {
        use proptest::prelude::*;
        use proptest::test_runner::{Config, TestRunner};

        // `a` and `a.p` interleave in sorted order (`remote.a.fetch`,
        // `remote.a.p.url`, `remote.a.url`): git puts the later key of `a`
        // back into the section `a` already has, not after `a.p`.
        let remotes = ["origin", "up", "a", "a.p", "a.b", "we\"ird", "x#y;z", "ü"];
        let branches = ["main", "feat/x", "q\"b", "dot.ted"];
        // No carriage return: git before the fix of CVE-2025-48384 wrote one
        // unquoted (`a_carriage_return_in_a_config_value_comes_back`).
        let chars = [
            ' ', '\t', ';', '#', '"', '\\', 'a', 'b', '/', ':', 'é', '=', '[', ']', '*', '\u{b}',
        ];
        let name = (0usize..10, 0usize..8, 0usize..4).prop_map(move |(kind, r, b)| {
            let (remote, branch) = (remotes[r], branches[b]);
            match kind {
                0 => format!("remote.{remote}.url"),
                1 => format!("remote.{remote}.pushurl"),
                2 => format!("remote.{remote}.fetch"),
                3 => format!("branch.{branch}.remote"),
                4 => format!("branch.{branch}.merge"),
                5 => format!("branch.{branch}.rebase"),
                6 => "push.default".to_owned(),
                7 => "core.autocrlf".to_owned(),
                8 => "core.ignorecase".to_owned(),
                _ => "core.symlinks".to_owned(),
            }
        });
        let text = prop::collection::vec(prop::sample::select(chars.to_vec()), 0..8)
            .prop_map(|chars| chars.into_iter().collect::<String>());
        let strategy = prop::collection::vec((name, text), 0..8);
        let mut runner = TestRunner::new(Config {
            cases: 32,
            ..Config::default()
        });
        runner
            .run(&strategy, |pairs| {
                let mut config: Vec<(GitConfigKey, GitConfigValue)> = pairs
                    .iter()
                    .filter_map(|(k, v)| {
                        Some((GitConfigKey::parse(k).ok()?, GitConfigValue::parse(v).ok()?))
                    })
                    .collect();
                // The order capture writes: by key, a repeated key's values in git's order.
                config.sort_by(|a, b| a.0.cmp(&b.0));
                let (by_git, written) = both_configs(&config);
                prop_assert_eq!(written, by_git);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_tree_name_that_is_not_utf8_is_refused_not_rewritten() {
        let oid = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
        let mut listing = format!("100644 blob {oid}\tok.txt").into_bytes();
        listing.push(0);
        assert_eq!(parse_tree(&listing).unwrap().len(), 1);
        listing.extend_from_slice(format!("100644 blob {oid}\tbad").as_bytes());
        listing.extend_from_slice(&[0xff, 0xfe, 0]);
        assert!(matches!(
            parse_tree(&listing),
            Err(RestoreError::BadTreeOutput { .. })
        ));
    }

    #[test]
    fn a_failing_blob_write_does_not_hang_cat_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let git = Git::new(&root);
        git.run(&["init", "-q"]).unwrap();
        for i in 0..BLOBS {
            fs::write(root.join(format!("f{i}")), format!("{i:01024}")).unwrap();
        }
        git.run(&["add", "-A"]).unwrap();
        let tree = git.run_text(&["write-tree"]).unwrap();
        let oids: Vec<GitOid> = parse_tree(&git.run(&["ls-tree", "-r", "-z", &tree]).unwrap())
            .unwrap()
            .into_iter()
            .map(|item| item.oid)
            .collect();
        assert_eq!(oids.len(), BLOBS);

        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let outcome = cat_file_batch(&git, &oids, |_, _, _| {
                Err(RestoreError::Write(WriteError::Collision {
                    path: PathBuf::from("f0"),
                }))
            });
            let _ = done.send(outcome);
        });
        let outcome = finished
            .recv_timeout(Duration::from_mins(1))
            .expect("restore hung on a failed write");
        assert!(
            matches!(
                outcome,
                Err(RestoreError::Write(WriteError::Collision { .. }))
            ),
            "{outcome:?}"
        );
    }
}
