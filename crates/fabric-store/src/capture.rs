//! Workspace capture (ADR 0004, What a workspace snapshot contains; as
//! built in ADR 0014, Capture).
//!
//! Invariant: capture reads the workspace and writes only objects into the
//! store, two private commits and one ref under `refs/fabric/` into the
//! repository, and deletes the job's older refs under `refs/fabric/`. Its
//! scratch files (index copies, the bundle) live in the store's `tmp/`,
//! which open and gc clear, never in the user's `.git`. It never touches
//! the real index, the working tree or the user's refs. A path or state
//! that could not be restored faithfully fails here, where the user can
//! act. An untracked secret never becomes an object; a tracked or stashed
//! one, already in the user's object store, is named in the report.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use fabric_core::git::{
    BranchName, GitConfigKey, GitConfigValue, GitOid, RefName, credential_range,
};
use fabric_core::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use fabric_core::path::{AbsPath, PathError, RelPath};
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, Entry, EntryKind, GitHead, GitState,
    HarnessState, ManifestError, Mode, Platform, SnapshotManifest,
};

use crate::durable::TempFile;
use crate::git::{Git, GitError};
use crate::objects::{ObjectError, StoredFile};
use crate::policy::{Class, IgnorePolicy, Markers};
use crate::store::{Store, StoreError};

/// An untracked, not ignored file above this many bytes stops capture unless the caller allows it.
pub const LARGE_UNTRACKED_LIMIT: u64 = 64 << 20;

/// How many incremental bundles may chain before the next one is full.
pub const MAX_BUNDLE_CHAIN: u32 = 16;

/// Measures the strings a cache depends on (ADR 0012), per cache path.
pub trait CarryKeys {
    /// Returns the key for the cache at `path`.
    fn key(&self, class: CacheClass, path: &RelPath) -> CarryKey;
}

/// Carry keys for a node that measured nothing: every key is `CarryKey::Unmeasured`, which `CarryKey::matches` never accepts.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnmeasuredKeys;

impl CarryKeys for UnmeasuredKeys {
    fn key(&self, _class: CacheClass, _path: &RelPath) -> CarryKey {
        CarryKey::Unmeasured
    }
}

/// What to capture and under which policy.
pub struct CaptureRequest<'a> {
    /// The workspace root; absolute, and the top level when it is a repository.
    pub root: &'a Path,
    /// The job the snapshot belongs to.
    pub job_id: JobId,
    /// The node doing the capture.
    pub created_by: NodeId,
    /// Unix milliseconds now.
    pub created_at: u64,
    /// The snapshot this one follows, already in the store.
    pub parent: Option<SnapshotId>,
    /// The ignored-file policy.
    pub policy: &'a IgnorePolicy,
    /// Measures carry keys for cache paths.
    pub carry_keys: &'a dyn CarryKeys,
    /// A directory the harness adapter produced, captured whole.
    pub harness_state: Option<&'a Path>,
    /// The environment spec the job had.
    pub env_spec: Option<EnvSpecId>,
    /// Let untracked files above `LARGE_UNTRACKED_LIMIT` enter the working-tree commit.
    pub allow_large_untracked: bool,
}

/// What kind of bundle the capture produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleKind {
    /// Every object the refs reach.
    Full,
    /// Only objects the parent's refs did not reach.
    Incremental,
    /// The parent's refs already reached every object; nothing to carry.
    Empty,
}

/// An index entry flag that commits `I` and `W` cannot record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFlag {
    /// `git add -N`: `write-tree` leaves the entry out, so restore would make the file plain untracked.
    IntentToAdd,
    /// Skip-worktree, which sparse checkout sets: `add -A` leaves the path alone, so `W` would hold a file the work tree does not.
    SkipWorktree,
    /// `git update-index --assume-unchanged`: `add -A` skips the file, so `W` would hold the index's copy, not the bytes on disk.
    AssumeUnchanged,
}

impl std::fmt::Display for IndexFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            IndexFlag::IntentToAdd => "intent-to-add (git add -N)",
            IndexFlag::SkipWorktree => "skip-worktree (sparse checkout)",
            IndexFlag::AssumeUnchanged => "assume-unchanged",
        })
    }
}

/// Why a repository cannot be captured right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyReason {
    /// A rebase is in progress.
    Rebase,
    /// A merge is in progress.
    Merge,
    /// A cherry-pick is in progress.
    CherryPick,
    /// A revert is in progress.
    Revert,
    /// A bisect is in progress.
    Bisect,
    /// Another git process holds `index.lock`.
    IndexLocked,
    /// The index has unresolved conflict entries.
    Conflicts,
}

impl std::fmt::Display for BusyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BusyReason::Rebase => "a rebase is in progress",
            BusyReason::Merge => "a merge is in progress",
            BusyReason::CherryPick => "a cherry-pick is in progress",
            BusyReason::Revert => "a revert is in progress",
            BusyReason::Bisect => "a bisect is in progress",
            BusyReason::IndexLocked => "index.lock exists",
            BusyReason::Conflicts => "the index has conflict entries",
        })
    }
}

/// A ref or `HEAD` that moved while capture made the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovedRef {
    /// `HEAD`.
    Head,
    /// A ref under `refs/`.
    Ref(RefName),
}

impl std::fmt::Display for MovedRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MovedRef::Head => f.write_str("HEAD"),
            MovedRef::Ref(name) => f.write_str(name.as_str()),
        }
    }
}

/// What capture saw, next to the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureReport {
    /// Files and symlinks in `files` and `harness_state`.
    pub files: u64,
    /// Their bytes, uncompressed.
    pub bytes: u64,
    /// Objects this capture wrote that the store did not have.
    pub new_objects: u64,
    /// Bytes on disk those objects took.
    pub new_bytes: u64,
    /// Ignored paths denied by a secret pattern.
    pub denied: Vec<RelPath>,
    /// Ignored paths excluded by `.fabricignore`.
    pub excluded: Vec<RelPath>,
    /// Untracked paths removed from the working-tree commit by the secret patterns or the excludes.
    pub denied_untracked: Vec<RelPath>,
    /// Paths in the harness state directory left out by a secret pattern.
    pub denied_harness: Vec<RelPath>,
    /// Tracked paths that match a secret pattern; they travel in the bundle.
    pub tracked_secrets: Vec<RelPath>,
    /// Paths in the stash's work tree, index or untracked commit that match a secret pattern; they travel in the bundle.
    pub stashed_secrets: Vec<RelPath>,
    /// Symlinks whose target is absolute or climbs above the root.
    pub escaping_symlinks: Vec<RelPath>,
    /// FIFOs, sockets and devices in the workspace, left behind.
    pub special_files: Vec<RelPath>,
    /// FIFOs, sockets and devices in the harness state directory, left behind.
    pub special_harness: Vec<RelPath>,
    /// Nested repositories, tracked submodules aside: in a repository the
    /// whole nested tree stays behind, in a plain directory its files are
    /// carried and only its `.git` stays behind.
    pub nested_repositories: Vec<RelPath>,
    /// Nested repositories in the harness state directory, whose `.git` stayed behind.
    pub nested_harness: Vec<RelPath>,
    /// Config keys whose remote URL lost its userinfo.
    pub stripped_userinfo: Vec<GitConfigKey>,
    /// Allowlisted config keys left behind because `GitConfigValue` refuses the value, or because it is a remote URL with a query or fragment, where a token can travel.
    pub dropped_config: Vec<GitConfigKey>,
    /// The bundle shape; none when the root is not a repository.
    pub bundle: Option<BundleKind>,
}

/// A manifest ready to publish, with the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// The manifest; every object it names is in the store.
    pub manifest: SnapshotManifest,
    /// What capture saw.
    pub report: CaptureReport,
}

/// Why capture stopped.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The store, or an object in it, refused.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The manifest does not validate.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The root is not an absolute path to a directory.
    #[error("{path} is not an absolute directory path")]
    BadRoot {
        /// The path given.
        path: PathBuf,
    },
    /// A path in the workspace breaks a manifest path rule; rename it and retry.
    #[error("path {path:?}: {source}")]
    BadPath {
        /// The path as the filesystem gave it.
        path: String,
        /// Which rule it breaks.
        #[source]
        source: PathError,
    },
    /// A path's mtime is before 1970, which a manifest cannot hold; touch it.
    #[error("{path}: the mtime is before 1970")]
    MtimeBeforeEpoch {
        /// The path.
        path: RelPath,
    },
    /// A file name or a symlink target is not UTF-8, which a manifest cannot hold; rename or relink it.
    #[error("{path:?}: the name or the link target is not UTF-8")]
    NotUtf8 {
        /// The path, with bytes that are not UTF-8 replaced for display.
        path: String,
    },
    /// The root lies inside a repository whose top level is elsewhere.
    #[error("{root} is inside the repository at {toplevel}; the job root must be the top level")]
    NotRepositoryRoot {
        /// The root given.
        root: PathBuf,
        /// The repository's top level.
        toplevel: PathBuf,
    },
    /// The repository is mid-operation or its index is locked or conflicted.
    #[error("git is busy: {reason}")]
    GitBusy {
        /// What was found.
        reason: BusyReason,
    },
    /// The root is a linked worktree, which shares refs with other jobs.
    #[error("{root} is a linked worktree of {common_dir}; use a primary clone")]
    GitLinkedWorktree {
        /// The root given.
        root: PathBuf,
        /// The shared git directory.
        common_dir: PathBuf,
    },
    /// A submodule has local changes no bundle can carry.
    #[error("submodule {path} has local changes")]
    GitSubmoduleDirty {
        /// The submodule path.
        path: RelPath,
    },
    /// The repository is a shallow clone; the bundle cannot carry the boundary `.git/shallow` records.
    #[error("the repository is a shallow clone; run git fetch --unshallow and capture again")]
    GitShallow,
    /// An index entry carries a flag the snapshot cannot reproduce; clear it and capture again.
    #[error("{path} is marked {flag} in the index, which a snapshot cannot carry")]
    GitIndexFlag {
        /// The path of the entry.
        path: RelPath,
        /// The flag.
        flag: IndexFlag,
    },
    /// A path has the LFS filter; the working-tree commit would hold pointers.
    #[error("{path} has filter=lfs, which capture does not support")]
    GitLfsUnsupported {
        /// The first path git says the filter applies to.
        path: RelPath,
    },
    /// Untracked files above the limit would enter the user's object store.
    #[error(
        "untracked files above {LARGE_UNTRACKED_LIMIT} bytes: {paths:?}; ignore them or allow large untracked files"
    )]
    UntrackedTooLarge {
        /// The offending paths.
        paths: Vec<RelPath>,
    },
    /// A path vanished, was replaced or was written to between its listing and its read; capture again.
    #[error("{path} changed while capture ran")]
    WorkspaceChanged {
        /// The path.
        path: RelPath,
    },
    /// A path is both in the ignored listing and in the working-tree commit, so the snapshot could not be restored.
    #[error("{path} is both ignored and in the working-tree commit")]
    IgnoredAndInWorktree {
        /// The path.
        path: RelPath,
    },
    /// A ref or `HEAD` moved while the bundle was made; capture again.
    #[error("{moved} moved while capture ran; capture again")]
    RefsMoved {
        /// The ref that moved, or `HEAD`.
        moved: MovedRef,
    },
    /// The parent snapshot belongs to another job.
    #[error("parent snapshot {parent} belongs to job {job}")]
    ParentJobMismatch {
        /// The parent given.
        parent: SnapshotId,
        /// The job it belongs to.
        job: JobId,
    },
}

/// An object error always arrives as `StoreError::Object`, so a caller matches one shape.
impl From<ObjectError> for CaptureError {
    fn from(error: ObjectError) -> Self {
        CaptureError::Store(StoreError::Object(error))
    }
}

fn io_err(path: &Path, source: io::Error) -> CaptureError {
    CaptureError::Io {
        path: path.to_owned(),
        source,
    }
}

fn rel_path(text: &str) -> Result<RelPath, CaptureError> {
    RelPath::parse(text).map_err(|source| CaptureError::BadPath {
        path: text.to_owned(),
        source,
    })
}

/// Opens the directory that holds `rel` under `base` and returns it with the last name of `rel`.
///
/// `base` itself may be a link, as a harness state directory often is; no
/// component below it is followed. A component that is a link or not a
/// directory any more gives `None`: the tree changed after it was listed,
/// and a path-based open would have read through the link.
fn parent_below<'a>(
    base: &Path,
    rel: &'a RelPath,
) -> io::Result<Option<(rustix::fd::OwnedFd, &'a str)>> {
    use rustix::fs::{Mode as RawBits, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let mut dir = rustix::fs::open(base, flags, RawBits::empty())?;
    let mut parts: Vec<&str> = rel.as_str().split('/').collect();
    let name = parts.pop().unwrap_or_default();
    for part in parts {
        dir = match rustix::fs::openat(&dir, part, flags | OFlags::NOFOLLOW, RawBits::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
    }
    Ok(Some((dir, name)))
}

/// Reads the target of the link at `rel` under `base`, refusing one that is not UTF-8 instead of rewriting it.
fn utf8_link(base: &Path, rel: &RelPath) -> Result<String, CaptureError> {
    let full = base.join(rel.as_str());
    let changed = || CaptureError::WorkspaceChanged { path: rel.clone() };
    let Some((dir, name)) = parent_below(base, rel).map_err(|e| io_err(&full, e))? else {
        return Err(changed());
    };
    let target = match rustix::fs::readlinkat(&dir, name, Vec::new()) {
        Ok(target) => target,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::INVAL) => return Err(changed()),
        Err(e) => return Err(io_err(&full, e.into())),
    };
    String::from_utf8(target.into_bytes()).map_err(|_| CaptureError::NotUtf8 {
        path: rel.as_str().to_owned(),
    })
}

/// What identifies one state of a path: device, inode, size and mtime.
type Stamp = (u64, u64, u64, i64, i64);

fn stamp(meta: &fs::Metadata) -> Stamp {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
    )
}

/// Stamps every path of `paths` under `root` that exists, without following links.
fn stamps<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a RelPath>,
) -> Result<HashMap<RelPath, Stamp>, CaptureError> {
    let mut out = HashMap::new();
    for rel in paths {
        let full = root.join(rel.as_str());
        match fs::symlink_metadata(&full) {
            Ok(meta) => {
                out.insert(rel.clone(), stamp(&meta));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&full, e)),
        }
    }
    Ok(out)
}

/// Returns the mtime of `meta` in nanoseconds since the epoch.
///
/// A manifest records an unsigned time, so an mtime before 1970 is refused
/// instead of being read as a different time.
fn mtime_ns(meta: &fs::Metadata, rel: &RelPath) -> Result<u64, CaptureError> {
    let before = || CaptureError::MtimeBeforeEpoch { path: rel.clone() };
    let secs = u64::try_from(meta.mtime()).map_err(|_| before())?;
    let nanos = u64::try_from(meta.mtime_nsec()).map_err(|_| before())?;
    Ok(secs.saturating_mul(1_000_000_000).saturating_add(nanos))
}

/// True when a link at `rel` with `target` points outside the tree root.
pub(crate) fn symlink_escapes(rel: &RelPath, target: &str) -> bool {
    if target.starts_with('/') {
        return true;
    }
    let mut depth = rel.as_str().matches('/').count();
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            _ => depth += 1,
        }
    }
    false
}

/// What a listed path is; a FIFO, socket or device is never listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoundKind {
    Dir,
    File,
    Symlink,
}

/// One directory, regular file or symlink found on disk.
struct Found {
    rel: RelPath,
    kind: FoundKind,
    meta: fs::Metadata,
}

impl Found {
    /// Wraps a listed path, or returns `None` for a FIFO, socket or device.
    fn new(rel: RelPath, meta: fs::Metadata) -> Option<Found> {
        let file_type = meta.file_type();
        let kind = if file_type.is_dir() {
            FoundKind::Dir
        } else if file_type.is_symlink() {
            FoundKind::Symlink
        } else if file_type.is_file() {
            FoundKind::File
        } else {
            return None;
        };
        Some(Found { rel, kind, meta })
    }
}

/// Opens a listed regular file under `base`, refusing it unless it is still the inode the listing saw.
///
/// A path swapped for a link, a FIFO or another file after the listing
/// fails here, and so does a path whose directory was swapped for a link:
/// the open walks down from `base` through directory handles and follows
/// no link, so the bytes stored under a name are the bytes of the file the
/// secret patterns judged by that name.
fn open_listed(base: &Path, found: &Found) -> Result<File, CaptureError> {
    use rustix::fs::{Mode as RawBits, OFlags};
    let full = base.join(found.rel.as_str());
    let changed = || CaptureError::WorkspaceChanged {
        path: found.rel.clone(),
    };
    let Some((dir, name)) = parent_below(base, &found.rel).map_err(|e| io_err(&full, e))? else {
        return Err(changed());
    };
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = match rustix::fs::openat(&dir, name, flags, RawBits::empty()) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOENT) => return Err(changed()),
        Err(e) => return Err(io_err(&full, e.into())),
    };
    let meta = file.metadata().map_err(|e| io_err(&full, e))?;
    if !meta.is_file() {
        return Err(changed());
    }
    if meta.dev() != found.meta.dev() || meta.ino() != found.meta.ino() {
        return Err(changed());
    }
    Ok(file)
}

/// Stores a listed regular file, refusing it if it changed in place while it was read.
///
/// Only the inode is checked before the read, so the size and mtime of the
/// open file are compared with the listing after it: a write during the
/// read would otherwise store a torn file under the name.
fn store_listed(store: &Store, base: &Path, found: &Found) -> Result<StoredFile, CaptureError> {
    let full = base.join(found.rel.as_str());
    let file = open_listed(base, found)?;
    let stored = store.objects().put_reader(&file, &full)?;
    let after = file.metadata().map_err(|e| io_err(&full, e))?;
    if stored.size != found.meta.len()
        || after.len() != found.meta.len()
        || (after.mtime(), after.mtime_nsec()) != (found.meta.mtime(), found.meta.mtime_nsec())
    {
        return Err(CaptureError::WorkspaceChanged {
            path: found.rel.clone(),
        });
    }
    Ok(stored)
}

/// What `list_tree` found.
#[derive(Default)]
struct Listing {
    found: Vec<Found>,
    nested: Vec<RelPath>,
    special: Vec<RelPath>,
}

/// Lists a directory tree, sorted by path, never following symlinks.
///
/// A `.git` in any letter case, whether a directory or the file a linked
/// worktree or submodule checkout has, is skipped, and the directory
/// holding it is listed in `nested`; one at the base itself is the
/// repository the base is, not content. A FIFO, socket or device is
/// skipped and listed in `special`.
fn list_tree(base: &Path) -> Result<Listing, CaptureError> {
    fn walk(base: &Path, prefix: &str, out: &mut Listing) -> Result<(), CaptureError> {
        let dir = base.join(prefix);
        let mut names: Vec<_> = fs::read_dir(&dir)
            .map_err(|e| io_err(&dir, e))?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()
            .map_err(|e| io_err(&dir, e))?;
        names.sort();
        for name in names {
            let Some(name) = name.to_str().map(str::to_owned) else {
                return Err(CaptureError::NotUtf8 {
                    path: dir.join(&name).to_string_lossy().into_owned(),
                });
            };
            let text = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let full = base.join(&text);
            let meta = fs::symlink_metadata(&full).map_err(|e| io_err(&full, e))?;
            if name.eq_ignore_ascii_case(".git") {
                if !prefix.is_empty() {
                    out.nested.push(rel_path(prefix)?);
                }
                continue;
            }
            let rel = rel_path(&text)?;
            match Found::new(rel.clone(), meta) {
                Some(found) => {
                    let is_dir = found.kind == FoundKind::Dir;
                    out.found.push(found);
                    if is_dir {
                        walk(base, &text, out)?;
                    }
                }
                None => out.special.push(rel),
            }
        }
        Ok(())
    }
    let mut out = Listing::default();
    walk(base, "", &mut out)?;
    Ok(out)
}

/// Every path a listing saw, with the stamp of each, to tell whether the tree changed after it was listed.
///
/// A FIFO, socket or device has no stamp; only its name counts.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census(BTreeMap<RelPath, Option<Stamp>>);

impl Census {
    /// Stamps every found path and records the bare `names` beside them.
    fn of<'a>(found: &[Found], names: impl Iterator<Item = &'a RelPath>) -> Census {
        let mut paths: BTreeMap<RelPath, Option<Stamp>> = found
            .iter()
            .map(|item| (item.rel.clone(), Some(stamp(&item.meta))))
            .collect();
        paths.extend(names.map(|rel| (rel.clone(), None)));
        Census(paths)
    }

    /// Fails with `WorkspaceChanged` on the first path, by name, that only one of the two lists, or else that `later` lists with another stamp.
    ///
    /// An added file also changes its directory's stamp; the file is the
    /// one worth naming.
    fn unchanged_in(&self, later: &Census) -> Result<(), CaptureError> {
        let names: BTreeSet<&RelPath> = self.0.keys().chain(later.0.keys()).collect();
        let changed = names
            .iter()
            .find(|rel| self.0.contains_key(**rel) != later.0.contains_key(**rel))
            .or_else(|| {
                names
                    .iter()
                    .find(|rel| self.0.get(**rel) != later.0.get(**rel))
            });
        match changed {
            Some(rel) => Err(CaptureError::WorkspaceChanged {
                path: (*rel).clone(),
            }),
            None => Ok(()),
        }
    }

    /// Walks `base` again and fails with `WorkspaceChanged` when a path was added, removed, replaced or written since this census.
    fn unchanged_under(&self, base: &Path) -> Result<(), CaptureError> {
        let later = list_tree(base)?;
        self.unchanged_in(&Census::of(&later.found, later.special.iter()))
    }
}

/// Counts of what the policy did under one cache root.
struct CacheSeen {
    class: CacheClass,
    carried: u64,
    denied: u64,
    excluded: u64,
}

struct Capturer<'a> {
    store: &'a Store,
    req: &'a CaptureRequest<'a>,
    report: CaptureReport,
}

impl Capturer<'_> {
    /// Turns a found path into an entry, writing its bytes into the store.
    fn entry_for(&mut self, base: &Path, found: &Found) -> Result<Entry, CaptureError> {
        let meta = &found.meta;
        let kind = match found.kind {
            FoundKind::Symlink => {
                let target = utf8_link(base, &found.rel)?;
                if symlink_escapes(&found.rel, &target) {
                    self.report.escaping_symlinks.push(found.rel.clone());
                }
                self.report.files += 1;
                EntryKind::Symlink { target }
            }
            FoundKind::Dir => EntryKind::Dir,
            FoundKind::File => {
                let stored = store_listed(self.store, base, found)?;
                self.report.files += 1;
                self.report.bytes += stored.size;
                self.report.new_objects += stored.new_objects;
                self.report.new_bytes += stored.new_bytes;
                EntryKind::File {
                    size: stored.size,
                    chunks: stored.chunks,
                }
            }
        };
        Ok(Entry {
            rel_path: found.rel.clone(),
            kind,
            mode: Mode::from_masked(meta.mode()),
            mtime_ns: mtime_ns(meta, &found.rel)?,
        })
    }

    /// Classifies `found` paths, stores the carried ones and returns the entries and cache rows.
    fn classify_and_store(
        &mut self,
        base: &Path,
        found: Vec<Found>,
    ) -> Result<(Vec<Entry>, Vec<CacheEntry>), CaptureError> {
        let policy = self.req.policy;
        let mut caches: BTreeMap<RelPath, CacheSeen> = BTreeMap::new();
        let mut entries = Vec::new();
        let mut carried_dirs: BTreeSet<RelPath> = BTreeSet::new();
        let mut emitted_dirs: BTreeSet<RelPath> = BTreeSet::new();
        let mut markers = DirMarkers {
            root: base,
            seen: HashMap::new(),
        };
        for item in found {
            let class = policy.classify(&item.rel, &mut markers);
            if let Some(root) = class.cache_root() {
                // A cache that holds only directories still gets its row, so
                // restore can ask the target about it like any other.
                caches.entry(root.path.clone()).or_insert(CacheSeen {
                    class: root.class,
                    carried: 0,
                    denied: 0,
                    excluded: 0,
                });
            }
            if item.kind == FoundKind::Dir {
                // A directory travels on its own when the walker listed it
                // and nothing denies it, so an empty directory is not lost.
                if class.carried() {
                    emitted_dirs.insert(item.rel.clone());
                    entries.push(self.entry_for(base, &item)?);
                }
                continue;
            }
            let seen = class.cache_root().map(|root| {
                caches.entry(root.path.clone()).or_insert(CacheSeen {
                    class: root.class,
                    carried: 0,
                    denied: 0,
                    excluded: 0,
                })
            });
            match class {
                Class::Secret { .. } => {
                    if let Some(seen) = seen {
                        seen.denied += 1;
                    }
                    self.report.denied.push(item.rel);
                    continue;
                }
                Class::Excluded { .. } => {
                    if let Some(seen) = seen {
                        seen.excluded += 1;
                    }
                    self.report.excluded.push(item.rel);
                    continue;
                }
                Class::Cache(_) | Class::Precious => {
                    if let Some(seen) = seen {
                        seen.carried += 1;
                    }
                }
            }
            let mut parent = item.rel.as_str();
            while let Some((dir, _)) = parent.rsplit_once('/') {
                if let Ok(dir) = RelPath::parse(dir) {
                    carried_dirs.insert(dir);
                }
                parent = dir;
            }
            entries.push(self.entry_for(base, &item)?);
        }
        for dir in carried_dirs.difference(&emitted_dirs) {
            let full = base.join(dir.as_str());
            let meta = fs::symlink_metadata(&full).map_err(|e| io_err(&full, e))?;
            entries.push(Entry {
                rel_path: dir.clone(),
                kind: EntryKind::Dir,
                mode: Mode::from_masked(meta.mode()),
                mtime_ns: mtime_ns(&meta, dir)?,
            });
        }
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let mut cache_entries = Vec::new();
        for (path, seen) in caches {
            let carry = if seen.denied == 0 && seen.excluded == 0 {
                CarryState::Carried
            } else if seen.carried == 0 {
                CarryState::NotCarried
            } else {
                CarryState::Partial
            };
            cache_entries.push(CacheEntry {
                key: self.req.carry_keys.key(seen.class, &path),
                path,
                class: seen.class,
                carry,
            });
        }
        Ok((entries, cache_entries))
    }

    /// Stores a file capture made and counts it in the report; the file goes with the guard.
    fn store_scratch_file(&mut self, file: TempFile) -> Result<Vec<ChunkId>, CaptureError> {
        let stored = self.store.objects().put_file(file.path())?;
        drop(file);
        self.report.new_objects += stored.new_objects;
        self.report.new_bytes += stored.new_bytes;
        self.report.bytes += stored.size;
        Ok(stored.chunks)
    }

    /// Stores a bundle, records its kind, and returns its chunks with the base it excludes.
    fn store_bundle(
        &mut self,
        built: BuiltBundle,
    ) -> Result<(Vec<ChunkId>, Option<SnapshotId>), CaptureError> {
        let (stored, kind) = match built {
            BuiltBundle::Full(file) => ((self.store_scratch_file(file)?, None), BundleKind::Full),
            BuiltBundle::Incremental { file, base } => (
                (self.store_scratch_file(file)?, Some(base)),
                BundleKind::Incremental,
            ),
            BuiltBundle::Empty { base } => ((Vec::new(), Some(base)), BundleKind::Empty),
        };
        self.report.bundle = Some(kind);
        Ok(stored)
    }

    /// Captures the harness state directory: every path but those the secret list names.
    ///
    /// The adapter's state is not a workspace, so the excludes and cache
    /// classes do not apply, but a secret never travels from here either:
    /// Claude Code keeps `.credentials.json` in its state directory on Linux.
    fn harness_entries(&mut self, base: &Path) -> Result<Vec<Entry>, CaptureError> {
        let listing = list_tree(base)?;
        let census = Census::of(&listing.found, listing.special.iter());
        self.report.nested_harness.extend(listing.nested);
        self.report.special_harness.extend(listing.special);
        let mut entries = Vec::with_capacity(listing.found.len());
        for item in &listing.found {
            if self.req.policy.is_secret(&item.rel) {
                self.report.denied_harness.push(item.rel.clone());
                continue;
            }
            entries.push(self.entry_for(base, item)?);
        }
        // A file the adapter wrote after its directory was listed would
        // otherwise be missing from the snapshot, and lost with the move.
        census.unchanged_under(base)?;
        // The walk lists a directory's children before its siblings, which
        // puts `abc/tool.txt` before `abc.jsonl`; the manifest sorts by bytes.
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(entries)
    }
}

/// Looks up a tool file in the workspace once per directory and name.
struct DirMarkers<'a> {
    root: &'a Path,
    seen: HashMap<(String, String), bool>,
}

impl Markers for DirMarkers<'_> {
    fn has(&mut self, dir: &str, name: &str) -> bool {
        let root = self.root;
        *self
            .seen
            .entry((dir.to_owned(), name.to_owned()))
            .or_insert_with(|| fs::symlink_metadata(root.join(dir).join(name)).is_ok())
    }
}

/// The refs and `HEAD` of a repository at one moment.
#[derive(Debug, PartialEq, Eq)]
struct RefView {
    refs: Vec<(RefName, GitOid)>,
    head: GitHead,
}

impl RefView {
    /// Names what differs in `later`: `HEAD` first, then the first ref by name.
    fn moved_in(&self, later: &RefView) -> Option<MovedRef> {
        if self.head != later.head {
            return Some(MovedRef::Head);
        }
        self.refs
            .iter()
            .chain(&later.refs)
            .filter(|r| !self.refs.contains(r) || !later.refs.contains(r))
            .map(|(name, _)| name)
            .min()
            .map(|name| MovedRef::Ref(name.clone()))
    }
}

/// The bundle capture tries to make.
enum BundlePlan {
    /// Every object the refs reach.
    Full,
    /// Only objects that `tips`, the refs of the snapshot `base`, do not reach.
    Incremental {
        /// The snapshot whose bundles carry the excluded objects.
        base: SnapshotId,
        /// The object ids in `base`'s refs; never empty.
        tips: Vec<GitOid>,
    },
}

impl BundlePlan {
    /// Plans the bundle after `parent`: incremental unless there is no parent
    /// with refs or its chain is already `MAX_BUNDLE_CHAIN` long.
    fn after(
        store: &Store,
        parent: Option<(SnapshotId, &SnapshotManifest)>,
    ) -> Result<BundlePlan, CaptureError> {
        let Some((base, manifest)) = parent else {
            return Ok(BundlePlan::Full);
        };
        let Some(git) = &manifest.git else {
            return Ok(BundlePlan::Full);
        };
        if git.refs.is_empty() || chain_depth(store, manifest)? >= MAX_BUNDLE_CHAIN {
            return Ok(BundlePlan::Full);
        }
        Ok(BundlePlan::Incremental {
            base,
            tips: git.refs.iter().map(|(_, oid)| oid.clone()).collect(),
        })
    }

    /// The parent's tips an incremental bundle leaves out; none for a full one.
    fn tips(&self) -> &[GitOid] {
        match self {
            BundlePlan::Full => &[],
            BundlePlan::Incremental { tips, .. } => tips,
        }
    }
}

/// A bundle capture made, with the file that holds it.
enum BuiltBundle {
    /// Every object the refs reach.
    Full(TempFile),
    /// Only objects the refs of `base` did not reach.
    Incremental {
        /// The bundle.
        file: TempFile,
        /// The snapshot whose refs the bundle excludes.
        base: SnapshotId,
    },
    /// The refs of `base` already reached every object.
    Empty {
        /// The snapshot whose refs reach every object.
        base: SnapshotId,
    },
}

/// The git half of a capture.
struct GitCapture<'a> {
    git: Git,
    root: &'a Path,
    job_id: JobId,
    scratch: PathBuf,
    /// The user's own exclude rules, which every listing and `W` start from.
    user_excludes: String,
    /// A file holding `user_excludes`, passed to every listing as `core.excludesFile`.
    listing_excludes: TempFile,
}

/// Reads the exclude file git would use for this repository under the sandbox of `crate::git`.
///
/// That is `core.excludesFile` from the repository's own config, or else
/// `$XDG_CONFIG_HOME/git/ignore`, or else `$HOME/.config/git/ignore`; git
/// reads the default even when the global config is not read. A missing
/// file is no rules.
fn user_excludes(git: &Git, root: &Path) -> Result<String, CaptureError> {
    let configured = git.output(&["config", "--path", "--get", "core.excludesFile"], &[])?;
    let path = if configured.status.success() {
        let text = String::from_utf8_lossy(&configured.stdout)
            .trim()
            .to_owned();
        Some(root.join(text))
    } else {
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(|h| PathBuf::from(h).join(".config"));
        xdg.or(home).map(|dir| dir.join("git").join("ignore"))
    };
    let Some(path) = path else {
        return Ok(String::new());
    };
    match fs::read(&path) {
        Ok(bytes) => {
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            Ok(text)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(io_err(&path, e)),
    }
}

/// What the git half saw of the work tree, checked again once every file is stored.
struct GitSeen {
    /// Every tracked and untracked path, existing or not.
    paths: Vec<RelPath>,
    /// The stamps of those that existed just before `W` was built.
    stamps: HashMap<RelPath, Stamp>,
    /// The untracked listing: paths, then nested repositories.
    untracked: (Vec<RelPath>, Vec<RelPath>),
}

/// What `capture` checks after every file is stored, by the kind of root.
enum Settle<'a> {
    /// A repository: its paths, untracked listing and ignored files.
    Git(Box<(GitCapture<'a>, GitSeen, Census)>),
    /// A plain directory: its walk.
    Plain(Census),
}

impl GitCapture<'_> {
    /// Fails with `WorkspaceChanged` when a path of the work tree was added, removed or written after capture listed it.
    ///
    /// `W` holds the tracked and untracked paths as their stamps were just
    /// before it was built, and `files` the ignored ones as they were
    /// listed; a path that appeared or changed after that would be missing
    /// from the snapshot, or older in it, and lost with the move.
    fn unchanged_since(&self, seen: &GitSeen, ignored: &Census) -> Result<(), CaptureError> {
        let now = stamps(self.root, seen.paths.iter())?;
        let changed = seen
            .paths
            .iter()
            .filter(|rel| now.get(*rel) != seen.stamps.get(*rel))
            .min();
        if let Some(rel) = changed {
            return Err(CaptureError::WorkspaceChanged { path: rel.clone() });
        }
        let (paths, nested) = self.untracked()?;
        let before: BTreeSet<&RelPath> = seen.untracked.0.iter().chain(&seen.untracked.1).collect();
        let after: BTreeSet<&RelPath> = paths.iter().chain(&nested).collect();
        if let Some(rel) = before.symmetric_difference(&after).min() {
            return Err(CaptureError::WorkspaceChanged {
                path: (*rel).clone(),
            });
        }
        let mut report = CaptureReport::default();
        let found = self.ignored(&mut report)?;
        let names = report
            .special_files
            .iter()
            .chain(&report.nested_repositories);
        ignored.unchanged_in(&Census::of(&found, names))
    }

    /// The `-c` value that makes a listing read the same user exclude rules as `W`.
    fn excludes_flag(&self) -> String {
        format!(
            "core.excludesFile={}",
            self.listing_excludes.path().display()
        )
    }

    fn refuse_bad_states(&self) -> Result<(), CaptureError> {
        let git_dir = fs::canonicalize(self.git.git_path(".")?).ok();
        let common = self.git.run_text(&["rev-parse", "--git-common-dir"])?;
        let common_dir = fs::canonicalize(self.root.join(common)).ok();
        if git_dir != common_dir {
            return Err(CaptureError::GitLinkedWorktree {
                root: self.root.to_owned(),
                common_dir: common_dir.unwrap_or_default(),
            });
        }
        for (marker, reason) in [
            ("rebase-merge", BusyReason::Rebase),
            ("rebase-apply", BusyReason::Rebase),
            ("MERGE_HEAD", BusyReason::Merge),
            ("CHERRY_PICK_HEAD", BusyReason::CherryPick),
            ("REVERT_HEAD", BusyReason::Revert),
            ("BISECT_LOG", BusyReason::Bisect),
        ] {
            if self.git.git_path(marker)?.exists() {
                return Err(CaptureError::GitBusy { reason });
            }
        }
        // A shallow clone's boundary commits name parents it does not
        // have; restored without `.git/shallow`, the repository fails fsck.
        if self
            .git
            .run_text(&["rev-parse", "--is-shallow-repository"])?
            == "true"
        {
            return Err(CaptureError::GitShallow);
        }
        if !self.git.run(&["ls-files", "-u", "-z"])?.is_empty() {
            return Err(CaptureError::GitBusy {
                reason: BusyReason::Conflicts,
            });
        }
        self.refuse_flagged_entries(&[])?;
        // `--ignore-submodules=none` overrides a `submodule.<name>.ignore`
        // in the config or in .gitmodules, which would hide the local work.
        let status = self.git.run(&[
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ])?;
        let mut records = Git::records(&status);
        while let Some(record) = records.next() {
            let text = Git::text("status record", record)?;
            // The path is the last field and, under -z, may hold spaces. A
            // rename record has one more field and is followed by a record
            // that holds the old path.
            let fields = match text.split(' ').next() {
                Some("1") => 9,
                Some("2") => {
                    records.next();
                    10
                }
                _ => continue,
            };
            let path = || rel_path(text.splitn(fields, ' ').last().unwrap_or(""));
            // An intent-to-add entry is added in the work tree and absent from the index.
            if fields == 9 && text.split(' ').nth(1) == Some(".A") {
                return Err(CaptureError::GitIndexFlag {
                    path: path()?,
                    flag: IndexFlag::IntentToAdd,
                });
            }
            let sub = text.split(' ').nth(2).unwrap_or("");
            if sub.starts_with('S') && sub[1..].chars().any(|c| c != '.') {
                return Err(CaptureError::GitSubmoduleDirty { path: path()? });
            }
        }
        // git resolves the attributes itself, from every .gitattributes,
        // tracked or not, and from .git/info/attributes.
        let paths = self.git.run(&[
            "-c",
            &self.excludes_flag(),
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])?;
        let attrs =
            self.git
                .run_stdin_env(&["check-attr", "-z", "--stdin", "filter"], &[], &paths)?;
        let fields: Vec<&[u8]> = attrs.split(|b| *b == 0).collect();
        for triple in fields.chunks_exact(3) {
            if triple[2] == b"lfs" {
                let text = Git::text("lfs path", triple[0])?;
                return Err(CaptureError::GitLfsUnsupported {
                    path: rel_path(text)?,
                });
            }
        }
        Ok(())
    }

    /// Refuses a skip-worktree or assume-unchanged entry in the index `env` names, and returns every path in it.
    fn refuse_flagged_entries(
        &self,
        env: &[(&str, &OsStr)],
    ) -> Result<BTreeSet<String>, CaptureError> {
        // `ls-files -v` tags a skip-worktree entry `S` and an
        // assume-unchanged one in lower case.
        let entries = self.git.run_env(&["ls-files", "-v", "-z"], env)?;
        let mut paths = BTreeSet::new();
        for record in Git::records(&entries) {
            let text = Git::text("index entry", record)?;
            let (tag, path) = text.split_once(' ').unwrap_or((text, ""));
            let flag = match tag {
                "S" | "s" => IndexFlag::SkipWorktree,
                "h" => IndexFlag::AssumeUnchanged,
                _ => {
                    paths.insert(path.to_owned());
                    continue;
                }
            };
            return Err(CaptureError::GitIndexFlag {
                path: rel_path(path)?,
                flag,
            });
        }
        Ok(paths)
    }

    /// Lists untracked, not ignored paths, and apart from them the untracked nested repositories.
    fn untracked(&self) -> Result<(Vec<RelPath>, Vec<RelPath>), CaptureError> {
        let out = self.git.run(&[
            "-c",
            &self.excludes_flag(),
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ])?;
        let mut paths = Vec::new();
        let mut nested = Vec::new();
        for record in Git::records(&out) {
            let text = Git::text("untracked path", record)?;
            match text.strip_suffix('/') {
                Some(dir) => nested.push(rel_path(dir)?),
                None => paths.push(rel_path(text)?),
            }
        }
        Ok((paths, nested))
    }

    /// Lists tracked paths.
    fn tracked(&self) -> Result<Vec<RelPath>, CaptureError> {
        let out = self.git.run(&["ls-files", "-z"])?;
        Git::records(&out)
            .map(|r| rel_path(Git::text("tracked path", r)?))
            .collect()
    }

    /// Lists ignored paths with their metadata; a nested repository is reported, not listed.
    fn ignored(&self, report: &mut CaptureReport) -> Result<Vec<Found>, CaptureError> {
        let listing = self.git.run(&[
            "-c",
            &self.excludes_flag(),
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ])?;
        let mut found = Vec::new();
        for record in Git::records(&listing) {
            let text = Git::text("ignored path", record)?;
            if let Some(dir) = text.strip_suffix('/') {
                report.nested_repositories.push(rel_path(dir)?);
                continue;
            }
            let rel = rel_path(text)?;
            let full = self.root.join(rel.as_str());
            let meta = fs::symlink_metadata(&full).map_err(|e| io_err(&full, e))?;
            match Found::new(rel.clone(), meta) {
                Some(item) => found.push(item),
                None => report.special_files.push(rel),
            }
        }
        Ok(found)
    }

    fn head_commit(&self) -> Result<Option<GitOid>, CaptureError> {
        let out = self
            .git
            .output(&["rev-parse", "--verify", "-q", "HEAD"], &[])?;
        if !out.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        Ok(Some(GitOid::parse(&text).map_err(GitError::from)?))
    }

    /// Copies the real index and builds commit `I` from the copy.
    ///
    /// The flags `refuse_bad_states` refused on the real index are checked
    /// again on the copy `I` is built from, since another git process can
    /// set one between that probe and the copy. An intent-to-add entry is
    /// one the copy holds and its written tree leaves out.
    fn index_commit(&self, head: Option<&GitOid>) -> Result<(GitOid, String), CaptureError> {
        let index = self.git.git_path("index")?;
        let lock = index.with_extension("lock");
        let copy = TempFile::in_dir(&self.scratch);
        // The copy counts only if no lock appeared and the index was not
        // replaced while it was read: git writes `index.lock` and renames it
        // over the index, so a copy taken in between holds the old state.
        let stamp = |path: &Path| match fs::metadata(path) {
            Ok(meta) => Ok(Some((
                meta.dev(),
                meta.ino(),
                meta.len(),
                meta.mtime(),
                meta.mtime_nsec(),
            ))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(path, e)),
        };
        let mut tries = 0;
        loop {
            if !lock.exists() {
                let before = stamp(&index)?;
                match fs::copy(&index, copy.path()) {
                    Ok(_) => {}
                    // No index yet: write-tree on a missing index file writes the empty tree.
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_err(&index, e)),
                }
                if !lock.exists() && stamp(&index)? == before {
                    break;
                }
            }
            tries += 1;
            if tries > 10 {
                return Err(CaptureError::GitBusy {
                    reason: BusyReason::IndexLocked,
                });
            }
            thread::sleep(Duration::from_millis(200));
        }
        let env: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", copy.path().as_os_str())];
        let indexed = self.refuse_flagged_entries(&env)?;
        let tree = self.git.run_text_env(&["write-tree"], &env)?;
        let written = self
            .git
            .run(&["ls-tree", "-r", "-z", "--name-only", &tree])?;
        let mut written: BTreeSet<&[u8]> = Git::records(&written).collect();
        for path in &indexed {
            if !written.remove(path.as_bytes()) {
                return Err(CaptureError::GitIndexFlag {
                    path: rel_path(path)?,
                    flag: IndexFlag::IntentToAdd,
                });
            }
        }
        let commit = self.commit_tree(&tree, head, None, "fabric index")?;
        Ok((commit, tree))
    }

    fn commit_tree(
        &self,
        tree: &str,
        head: Option<&GitOid>,
        second: Option<&GitOid>,
        message: &str,
    ) -> Result<GitOid, CaptureError> {
        let mut args = vec!["commit-tree", tree, "-m", message];
        if let Some(head) = head {
            args.extend(["-p", head.as_str()]);
        }
        if let Some(second) = second {
            args.extend(["-p", second.as_str()]);
        }
        let text = self.git.run_text(&args)?;
        Ok(GitOid::parse(&text).map_err(GitError::from)?)
    }

    /// Builds commit `W` from a private index seeded with `I`'s tree.
    ///
    /// `git add -A` runs with the user's exclude rules followed by the
    /// policy's patterns as the lowest-precedence excludes, which keeps most
    /// secrets out of the user's object store. The policy adds only positive
    /// patterns after the user's rules, so a path the ignored listing names
    /// is never in `W`. A negation in the repository's own ignore files can
    /// override that, so the private index is then listed and every path
    /// that is not tracked and that `leave_out` names is removed before the
    /// tree is written. The removed paths are returned.
    fn worktree_commit(
        &self,
        index_tree: &str,
        head: Option<&GitOid>,
        index_commit: &GitOid,
        excludes: &str,
        tracked: &BTreeSet<RelPath>,
        leave_out: &dyn Fn(&RelPath) -> bool,
    ) -> Result<(GitOid, Vec<RelPath>), CaptureError> {
        let index = TempFile::in_dir(&self.scratch);
        let exclude_file = TempFile::in_dir(&self.scratch);
        let all = format!("{}{excludes}", self.user_excludes);
        fs::write(exclude_file.path(), all).map_err(|e| io_err(exclude_file.path(), e))?;
        let env: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", index.path().as_os_str())];
        self.git.run_env(&["read-tree", index_tree], &env)?;
        let excludes_flag = format!("core.excludesFile={}", exclude_file.path().display());
        self.git
            .run_env(&["-c", &excludes_flag, "add", "-A", "--"], &env)?;
        let mut removed = Vec::new();
        let listing = self.git.run_env(&["ls-files", "-z"], &env)?;
        for record in Git::records(&listing) {
            let rel = rel_path(Git::text("index path", record)?)?;
            if !tracked.contains(&rel) && leave_out(&rel) {
                removed.push(rel);
            }
        }
        if !removed.is_empty() {
            let mut input = Vec::new();
            for rel in &removed {
                input.extend_from_slice(rel.as_str().as_bytes());
                input.push(0);
            }
            self.git.run_stdin_env(
                &["update-index", "--force-remove", "-z", "--stdin"],
                &env,
                &input,
            )?;
        }
        let tree = self.git.run_text_env(&["write-tree"], &env)?;
        let commit = self.commit_tree(&tree, head, Some(index_commit), "fabric worktree")?;
        Ok((commit, removed))
    }

    fn snap_prefix(&self) -> String {
        format!("refs/fabric/{}/snap/", self.job_id)
    }

    /// Lists refs with their ids; older snapshot refs of this job are already deleted.
    fn refs(&self) -> Result<Vec<(RefName, GitOid)>, CaptureError> {
        let out = self
            .git
            .run_text(&["for-each-ref", "--format=%(refname) %(objectname)"])?;
        let mut refs = Vec::new();
        for line in out.lines() {
            let (name, oid) = line.split_once(' ').ok_or_else(|| GitError::Parse {
                what: "ref line",
                text: line.to_owned(),
            })?;
            refs.push((
                RefName::parse(name).map_err(GitError::from)?,
                GitOid::parse(oid).map_err(GitError::from)?,
            ));
        }
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(refs)
    }

    /// Reads the refs and `HEAD` together.
    fn ref_view(&self) -> Result<RefView, CaptureError> {
        Ok(RefView {
            refs: self.refs()?,
            head: self.head()?,
        })
    }

    fn head(&self) -> Result<GitHead, CaptureError> {
        let out = self.git.output(&["symbolic-ref", "-q", "HEAD"], &[])?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            let branch = text
                .strip_prefix("refs/heads/")
                .ok_or_else(|| GitError::Parse {
                    what: "HEAD branch",
                    text: text.clone(),
                })?;
            return Ok(GitHead::Branch(
                BranchName::parse(branch).map_err(GitError::from)?,
            ));
        }
        let text = self.git.run_text(&["rev-parse", "HEAD"])?;
        Ok(GitHead::Detached(
            GitOid::parse(&text).map_err(GitError::from)?,
        ))
    }

    /// Creates the bundle `plan` asks for, or a full one when an incremental one cannot be made; an empty one has no file.
    fn bundle(&self, plan: &BundlePlan) -> Result<BuiltBundle, CaptureError> {
        let path = TempFile::in_dir(&self.scratch);
        let path_text = path.path().to_string_lossy().into_owned();
        if let BundlePlan::Incremental { base, tips } = plan {
            let base = *base;
            let mut args = vec!["bundle", "create", &path_text, "--all", "--not"];
            let mut existing = Vec::new();
            for tip in tips {
                let peeled = format!("{}^{{commit}}", tip.as_str());
                if self
                    .git
                    .succeeds(&["rev-parse", "--verify", "-q", &peeled])?
                {
                    existing.push(tip.as_str());
                }
            }
            let all_exist = existing.len() == tips.len();
            if all_exist {
                args.extend(existing);
                let out = self.git.output(&args, &[])?;
                if out.status.success() && self.git.succeeds(&["bundle", "verify", &path_text])? {
                    return Ok(BuiltBundle::Incremental { file: path, base });
                }
                let stderr = String::from_utf8_lossy(&out.stderr);
                if stderr.contains("empty bundle") {
                    return Ok(BuiltBundle::Empty { base });
                }
                tracing::warn!(stderr = %stderr.trim(), "incremental bundle failed; making a full one");
            }
        }
        self.git.run(&["bundle", "create", &path_text, "--all"])?;
        self.git.run(&["bundle", "verify", &path_text])?;
        Ok(BuiltBundle::Full(path))
    }

    /// Makes the bundle and returns it with the refs and `HEAD` it is proven to hold.
    fn bundle_with_refs(&self, plan: &BundlePlan) -> Result<(BuiltBundle, RefView), CaptureError> {
        let view = self.ref_view()?;
        let built = self.bundle(plan)?;
        if let Some(moved) = view.moved_in(&self.ref_view()?) {
            return Err(CaptureError::RefsMoved { moved });
        }
        self.confirm_bundle(&built, &view, plan.tips())?;
        Ok((built, view))
    }

    /// Proves that the bundle chain holds every object `view` names.
    ///
    /// A ref the bundle lists must have the id `view` gives it. A full
    /// bundle must list every ref. A ref an incremental or empty bundle
    /// leaves out must be reachable from the parent's tips, which the
    /// earlier bundles of the chain carry. Anything else means a ref moved
    /// while the bundle was made, and the snapshot would not restore.
    fn confirm_bundle(
        &self,
        built: &BuiltBundle,
        view: &RefView,
        tips: &[GitOid],
    ) -> Result<(), CaptureError> {
        let listed: BTreeMap<String, String> = match built {
            BuiltBundle::Full(file) | BuiltBundle::Incremental { file, .. } => {
                let path = file.path().to_string_lossy().into_owned();
                let out = self.git.run_text(&["bundle", "list-heads", &path])?;
                out.lines()
                    .filter_map(|line| line.split_once(' '))
                    .map(|(oid, name)| (name.to_owned(), oid.to_owned()))
                    .collect()
            }
            BuiltBundle::Empty { .. } => BTreeMap::new(),
        };
        let moved = |moved: &MovedRef| CaptureError::RefsMoved {
            moved: moved.clone(),
        };
        let mut wanted: Vec<(MovedRef, &GitOid)> = view
            .refs
            .iter()
            .map(|(name, oid)| (MovedRef::Ref(name.clone()), oid))
            .collect();
        if let GitHead::Detached(oid) = &view.head {
            wanted.push((MovedRef::Head, oid));
        }
        let mut left_out = Vec::new();
        for (name, oid) in &wanted {
            match listed.get(&name.to_string()) {
                Some(seen) if seen == oid.as_str() => {}
                Some(_) => return Err(moved(name)),
                None if matches!(built, BuiltBundle::Full(_)) => return Err(moved(name)),
                None => left_out.push((name, *oid)),
            }
        }
        for name in listed.keys() {
            if name != "HEAD" && !wanted.iter().any(|(n, _)| n.to_string() == *name) {
                let name = RefName::parse(name).map_err(GitError::from)?;
                return Err(moved(&MovedRef::Ref(name)));
            }
        }
        if self.reaches_beyond(left_out.iter().map(|(_, oid)| *oid), tips)? {
            // Name the ref; this is the failure path, so one git run per ref is fine.
            for (name, oid) in &left_out {
                if self.reaches_beyond([*oid], tips)? {
                    return Err(moved(name));
                }
            }
        }
        Ok(())
    }

    /// True when `from` reaches a commit that no tip in `tips` reaches.
    fn reaches_beyond<'o>(
        &self,
        from: impl IntoIterator<Item = &'o GitOid>,
        tips: &[GitOid],
    ) -> Result<bool, CaptureError> {
        let mut input = String::new();
        for oid in from {
            input.push_str(oid.as_str());
            input.push('\n');
        }
        if input.is_empty() {
            return Ok(false);
        }
        for tip in tips {
            input.push('^');
            input.push_str(tip.as_str());
            input.push('\n');
        }
        let out = self
            .git
            .run_stdin_env(&["rev-list", "--stdin", "-n1"], &[], input.as_bytes())?;
        Ok(!out.trim_ascii().is_empty())
    }

    fn delete_older_snap_refs(&self, current: &GitOid) -> Result<(), CaptureError> {
        let prefix = self.snap_prefix();
        let out = self
            .git
            .run_text(&["for-each-ref", "--format=%(refname)", &prefix])?;
        for name in out.lines() {
            if name.strip_prefix(&prefix) != Some(current.as_str()) {
                self.git.run(&["update-ref", "-d", name])?;
            }
        }
        Ok(())
    }

    /// Reads the mtime of every path in `W` and reports the links among them that escape the root.
    ///
    /// `before` holds the stamps taken just before `git add -A` built `W`.
    /// A path whose stamp differs now, or that had none, was written while
    /// or after `W` was built, so `W` may hold older bytes than the mtime
    /// recorded here; that fails with `WorkspaceChanged` instead of
    /// publishing a snapshot that loses the edit.
    fn worktree_mtimes(
        &self,
        worktree: &GitOid,
        before: &HashMap<RelPath, Stamp>,
        report: &mut CaptureReport,
    ) -> Result<Vec<(RelPath, u64)>, CaptureError> {
        let out = self
            .git
            .run(&["ls-tree", "-r", "-z", "--name-only", worktree.as_str()])?;
        let mut mtimes = Vec::new();
        for record in Git::records(&out) {
            let rel = rel_path(Git::text("tree path", record)?)?;
            let full = self.root.join(rel.as_str());
            let meta = match fs::symlink_metadata(&full) {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(CaptureError::WorkspaceChanged { path: rel });
                }
                Err(e) => return Err(io_err(&full, e)),
            };
            if before.get(&rel) != Some(&stamp(&meta)) {
                return Err(CaptureError::WorkspaceChanged { path: rel });
            }
            if meta.file_type().is_symlink() {
                let target = utf8_link(self.root, &rel)?;
                if symlink_escapes(&rel, &target) {
                    report.escaping_symlinks.push(rel.clone());
                }
            }
            let mtime = mtime_ns(&meta, &rel)?;
            mtimes.push((rel, mtime));
        }
        mtimes.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(mtimes)
    }

    /// Lists the paths of the stash commit `stash` and of its index and untracked parents that `denies` names.
    ///
    /// The bundle carries `refs/stash` with its parents, so a `.env` saved
    /// by `git stash -u` travels with it although it was never tracked.
    fn stashed_secrets(
        &self,
        stash: &GitOid,
        denies: &dyn Fn(&RelPath) -> bool,
    ) -> Result<Vec<RelPath>, CaptureError> {
        let mut found = BTreeSet::new();
        for suffix in ["", "^2", "^3"] {
            let rev = format!("{}{suffix}", stash.as_str());
            let commit = format!("{rev}^{{commit}}");
            if !self
                .git
                .succeeds(&["rev-parse", "--verify", "-q", &commit])?
            {
                continue;
            }
            let out = self
                .git
                .run(&["ls-tree", "-r", "-z", "--name-only", &rev])?;
            for record in Git::records(&out) {
                let rel = rel_path(Git::text("stash path", record)?)?;
                if denies(&rel) {
                    found.insert(rel);
                }
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Reads the allowlisted projection of the local config.
    ///
    /// A key git holds more than once, such as two `remote.origin.fetch`
    /// refspecs, keeps every value, in the order git lists them: the sort
    /// by key is stable.
    fn config(
        &self,
        report: &mut CaptureReport,
    ) -> Result<Vec<(GitConfigKey, GitConfigValue)>, CaptureError> {
        let out = self.git.run(&["config", "--list", "--local", "-z"])?;
        let mut config: Vec<(GitConfigKey, GitConfigValue)> = Vec::new();
        for record in Git::records(&out) {
            let text = Git::text("config record", record)?;
            let (key, value) = text.split_once('\n').unwrap_or((text, ""));
            let Ok(key) = GitConfigKey::parse(key) else {
                continue;
            };
            let mut value = value.to_owned();
            if key.is_remote_url()
                && let Some(stripped) = strip_userinfo(&value)
            {
                value = stripped;
                report.stripped_userinfo.push(key.clone());
            }
            let Ok(value) = GitConfigValue::parse(&value) else {
                report.dropped_config.push(key);
                continue;
            };
            // Only a query or fragment is left after the strip; the
            // receiving node would refuse the manifest that carried it.
            if key.is_remote_url() && value.has_credential() {
                report.dropped_config.push(key);
                continue;
            }
            config.push((key, value));
        }
        config.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(config)
    }
}

/// Removes the part of a remote URL that can hold a credential (`fabric_core::git::credential_range`).
fn strip_userinfo(url: &str) -> Option<String> {
    let range = credential_range(url)?;
    Some(format!("{}{}", &url[..range.start], &url[range.end..]))
}

/// Captures the workspace described by `req` into `store` and returns the manifest.
///
/// The manifest is not yet published; the caller does that with
/// `Store::publish`, so the ledger can be the commit point (ADR 0004), and
/// runs no `Store::gc` in between, since gc may remove an object the
/// manifest reuses.
pub fn capture(store: &Store, req: &CaptureRequest<'_>) -> Result<Captured, CaptureError> {
    let root_text = req.root.to_str().ok_or_else(|| CaptureError::BadRoot {
        path: req.root.to_owned(),
    })?;
    let root = AbsPath::parse(root_text).map_err(|_| CaptureError::BadRoot {
        path: req.root.to_owned(),
    })?;
    if !req.root.is_dir() {
        return Err(CaptureError::BadRoot {
            path: req.root.to_owned(),
        });
    }
    let parent_manifest = match req.parent {
        Some(parent) => {
            let manifest = store.manifest(parent)?;
            if manifest.job_id != req.job_id {
                return Err(CaptureError::ParentJobMismatch {
                    parent,
                    job: manifest.job_id,
                });
            }
            Some((parent, manifest))
        }
        None => None,
    };
    let mut capturer = Capturer {
        store,
        req,
        report: CaptureReport::default(),
    };
    let (git_state, found, settle) = if let Some(repo) = detect_git(req, store.objects().tmp_dir())?
    {
        repo.refuse_bad_states()?;
        let parent = parent_manifest
            .as_ref()
            .map(|(id, manifest)| (*id, manifest));
        let (state, seen) = capture_git(&mut capturer, &repo, parent)?;
        let report = &mut capturer.report;
        let (special, nested) = (report.special_files.len(), report.nested_repositories.len());
        let found = repo.ignored(report)?;
        let names = report.special_files[special..]
            .iter()
            .chain(&report.nested_repositories[nested..]);
        let ignored = Census::of(&found, names);
        (
            Some(state),
            found,
            Settle::Git(Box::new((repo, seen, ignored))),
        )
    } else {
        let listing = list_tree(req.root)?;
        let census = Census::of(&listing.found, listing.special.iter());
        capturer.report.nested_repositories.extend(listing.nested);
        capturer.report.special_files.extend(listing.special);
        (None, listing.found, Settle::Plain(census))
    };
    let (files, caches) = capturer.classify_and_store(req.root, found)?;
    match &settle {
        Settle::Git(git) => git.0.unchanged_since(&git.1, &git.2)?,
        Settle::Plain(census) => census.unchanged_under(req.root)?,
    }
    let harness_state = match req.harness_state {
        Some(dir) => HarnessState {
            entries: capturer.harness_entries(dir)?,
        },
        None => HarnessState::default(),
    };
    let manifest = SnapshotManifest {
        job_id: req.job_id,
        root,
        parent: req.parent,
        created_at: req.created_at,
        created_by: req.created_by,
        platform: Platform {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        },
        git: git_state,
        files,
        harness_state,
        env_spec: req.env_spec,
        caches,
    };
    if let Some(state) = &manifest.git {
        refuse_overlap(&manifest.files, &state.worktree_mtimes)?;
    }
    crate::store::encode_manifest(&manifest)?;
    Ok(Captured {
        manifest,
        report: capturer.report,
    })
}

/// Refuses a capture whose ignored files and working-tree commit share a path.
///
/// Restore checks out `W` and then writes `files` with exclusive creates, so
/// a shared path would make a snapshot that publishes and never restores.
fn refuse_overlap(files: &[Entry], worktree: &[(RelPath, u64)]) -> Result<(), CaptureError> {
    for entry in files {
        if worktree
            .binary_search_by(|(path, _)| path.cmp(&entry.rel_path))
            .is_ok()
        {
            return Err(CaptureError::IgnoredAndInWorktree {
                path: entry.rel_path.clone(),
            });
        }
    }
    Ok(())
}

/// Returns a git capture when the root is the top level of a repository.
fn detect_git<'a>(
    req: &CaptureRequest<'a>,
    scratch: &Path,
) -> Result<Option<GitCapture<'a>>, CaptureError> {
    let git = Git::new(req.root).with_date(req.created_at);
    let out = git.output(&["rev-parse", "--show-toplevel"], &[])?;
    if !out.status.success() {
        // Only "not a repository" makes the root a plain directory. Any
        // other failure, such as `safe.directory` refusing a repository
        // another user owns or a config git cannot parse, is surfaced:
        // capturing the root as plain files would drop its git state.
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(GitError::Failed {
            command: "rev-parse --show-toplevel".to_owned(),
            code: out.status.code(),
            stderr,
        }
        .into());
    }
    let toplevel = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let same = match (fs::canonicalize(&toplevel), fs::canonicalize(req.root)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same {
        return Err(CaptureError::NotRepositoryRoot {
            root: req.root.to_owned(),
            toplevel,
        });
    }
    let user_excludes = user_excludes(&git, req.root)?;
    let listing_excludes = TempFile::in_dir(scratch);
    fs::write(listing_excludes.path(), &user_excludes)
        .map_err(|e| io_err(listing_excludes.path(), e))?;
    Ok(Some(GitCapture {
        git,
        root: req.root,
        job_id: req.job_id,
        scratch: scratch.to_owned(),
        user_excludes,
        listing_excludes,
    }))
}

/// How many incremental bundles sit between `manifest` and the last full one.
fn chain_depth(store: &Store, manifest: &SnapshotManifest) -> Result<u32, CaptureError> {
    let mut depth = 0;
    let mut base = manifest.git.as_ref().and_then(|g| g.bundle_base);
    while let Some(id) = base {
        depth += 1;
        if depth >= MAX_BUNDLE_CHAIN {
            break;
        }
        base = store.snapshot(id)?.bundle_base;
    }
    Ok(depth)
}

fn capture_git(
    capturer: &mut Capturer<'_>,
    repo: &GitCapture<'_>,
    parent: Option<(SnapshotId, &SnapshotManifest)>,
) -> Result<(GitState, GitSeen), CaptureError> {
    let req = capturer.req;
    let policy = req.policy;
    let (untracked, nested) = repo.untracked()?;
    capturer
        .report
        .nested_repositories
        .extend(nested.iter().cloned());
    let mut too_large = Vec::new();
    for rel in &untracked {
        if policy.denies_anywhere(rel) {
            capturer.report.denied_untracked.push(rel.clone());
            continue;
        }
        let full = req.root.join(rel.as_str());
        let meta = fs::symlink_metadata(&full).map_err(|e| io_err(&full, e))?;
        if meta.is_file() && meta.len() > LARGE_UNTRACKED_LIMIT {
            too_large.push(rel.clone());
        }
    }
    if !too_large.is_empty() && !req.allow_large_untracked {
        return Err(CaptureError::UntrackedTooLarge { paths: too_large });
    }
    let tracked: BTreeSet<RelPath> = repo.tracked()?.into_iter().collect();
    for rel in &tracked {
        if policy.denies_anywhere(rel) {
            capturer.report.tracked_secrets.push(rel.clone());
        }
    }

    let head_commit = repo.head_commit()?;
    let (index_commit, index_tree) = repo.index_commit(head_commit.as_ref())?;
    let under_nested = |rel: &RelPath| nested.iter().any(|root| rel.starts_with(root));
    // A nested repository without a commit makes `add -A` fail, so every
    // nested root is excluded up front as well as removed afterwards.
    let mut excludes = policy.gitignore_lines();
    for root in &nested {
        excludes.push('/');
        excludes.push_str(root.as_str());
        excludes.push_str("/\n");
    }
    let stamped: Vec<RelPath> = tracked.iter().chain(untracked.iter()).cloned().collect();
    let before = stamps(req.root, stamped.iter())?;
    let (worktree_commit, removed) = repo.worktree_commit(
        &index_tree,
        head_commit.as_ref(),
        &index_commit,
        &excludes,
        &tracked,
        &|rel| policy.denies_anywhere(rel) || under_nested(rel),
    )?;
    let denied = &mut capturer.report.denied_untracked;
    for rel in removed {
        if !under_nested(&rel) && !denied.contains(&rel) {
            denied.push(rel);
        }
    }
    denied.sort();
    let snap_ref = format!("{}{}", repo.snap_prefix(), worktree_commit.as_str());
    repo.git
        .run(&["update-ref", &snap_ref, worktree_commit.as_str()])?;

    let plan = BundlePlan::after(capturer.store, parent)?;
    repo.delete_older_snap_refs(&worktree_commit)?;
    let (built, view) = repo.bundle_with_refs(&plan)?;
    let (bundle, bundle_base) = capturer.store_bundle(built)?;
    let RefView { refs, head } = view;
    if let Some((_, stash)) = refs.iter().find(|(name, _)| name.as_str() == "refs/stash") {
        capturer.report.stashed_secrets =
            repo.stashed_secrets(stash, &|rel| policy.denies_anywhere(rel))?;
    }
    let worktree_mtimes = repo.worktree_mtimes(&worktree_commit, &before, &mut capturer.report)?;
    let config = repo.config(&mut capturer.report)?;
    let exclude_path = repo.git.git_path("info/exclude")?;
    // `info/exclude` is git's own file, not workspace content, so a link to
    // a shared exclude file is followed as git follows it.
    let exclude = if exclude_path.is_file() {
        let bytes = fs::read(&exclude_path).map_err(|e| io_err(&exclude_path, e))?;
        let stored = capturer.store.objects().put_bytes(&bytes)?;
        capturer.report.new_objects += stored.new_objects;
        capturer.report.new_bytes += stored.new_bytes;
        Some(stored.chunks)
    } else {
        None
    };
    let state = GitState {
        bundle,
        bundle_base,
        refs,
        head,
        index_commit,
        worktree_commit,
        worktree_mtimes,
        config,
        exclude,
    };
    let seen = GitSeen {
        paths: stamped,
        stamps: before,
        untracked: (untracked, nested),
    };
    Ok((state, seen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ChunkParams;

    fn rel(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    #[test]
    fn symlink_escape_rule() {
        assert!(symlink_escapes(&rel("a"), "/etc/passwd"));
        assert!(symlink_escapes(&rel("a"), "../x"));
        assert!(!symlink_escapes(&rel("d/a"), "../x"));
        assert!(symlink_escapes(&rel("d/a"), "../../x"));
        assert!(!symlink_escapes(&rel("d/a"), "b/../c"));
        assert!(!symlink_escapes(&rel("a"), "./b"));
    }

    #[test]
    fn a_file_replaced_after_its_listing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        let key = dir.path().join("id_ed25519");
        fs::write(&path, "notes").unwrap();
        fs::write(&key, "secret").unwrap();
        let found = Found::new(rel("notes.txt"), fs::symlink_metadata(&path).unwrap()).unwrap();
        // The listed inode stays alive beside it, so no new file can reuse its number.
        fs::rename(&path, dir.path().join("kept")).unwrap();
        std::os::unix::fs::symlink(&key, &path).unwrap();
        assert!(matches!(
            open_listed(dir.path(), &found),
            Err(CaptureError::WorkspaceChanged { .. })
        ));
        fs::remove_file(&path).unwrap();
        fs::write(&path, "another file").unwrap();
        assert!(matches!(
            open_listed(dir.path(), &found),
            Err(CaptureError::WorkspaceChanged { .. })
        ));
    }

    /// A listing that walked through a directory swapped for a link saw the
    /// outside file's own inode, so only the walk from the base catches it.
    #[test]
    fn a_directory_replaced_by_a_link_is_not_read_through() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("work");
        let outside = dir.path().join("home");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("notes.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("sub")).unwrap();
        let listed = fs::symlink_metadata(outside.join("notes.txt")).unwrap();
        let found = Found::new(rel("sub/notes.txt"), listed).unwrap();
        assert!(matches!(
            open_listed(&base, &found),
            Err(CaptureError::WorkspaceChanged { .. })
        ));
        assert!(matches!(
            utf8_link(&base, &rel("sub/notes.txt")),
            Err(CaptureError::WorkspaceChanged { .. })
        ));
    }

    #[test]
    fn a_file_changed_in_place_after_its_listing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(&dir.path().join("store"), ChunkParams::DEFAULT).unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "notes").unwrap();
        let found = Found::new(rel("notes.txt"), fs::symlink_metadata(&path).unwrap()).unwrap();
        fs::write(&path, "notes, rewritten").unwrap();
        assert!(matches!(
            store_listed(&store, dir.path(), &found),
            Err(CaptureError::WorkspaceChanged { .. })
        ));
    }

    /// A file written after `W` was built fails capture instead of travelling with its old bytes.
    #[test]
    fn a_file_written_after_w_was_built_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        fs::create_dir(&root).unwrap();
        let repo = GitCapture {
            git: Git::new(&root).with_date(1_760_000_000_000),
            root: &root,
            job_id: JobId::from_bytes([1; 16]),
            scratch: dir.path().to_owned(),
            user_excludes: String::new(),
            listing_excludes: TempFile::in_dir(dir.path()),
        };
        repo.git.run(&["init", "-q", "-b", "main"]).unwrap();
        fs::write(root.join("a.txt"), "before\n").unwrap();
        repo.git.run(&["add", "a.txt"]).unwrap();
        repo.git.run(&["commit", "-q", "-m", "one"]).unwrap();
        let w = repo.head_commit().unwrap().unwrap();
        let paths = [rel("a.txt")];
        let before = stamps(&root, paths.iter()).unwrap();
        let mut report = CaptureReport::default();
        assert!(repo.worktree_mtimes(&w, &before, &mut report).is_ok());
        fs::write(root.join("a.txt"), "written after W\n").unwrap();
        assert!(matches!(
            repo.worktree_mtimes(&w, &before, &mut report),
            Err(CaptureError::WorkspaceChanged { path }) if path.as_str() == "a.txt"
        ));
    }

    /// A flag set after `refuse_bad_states` probed the real index is refused on the copy `I` is built from.
    #[test]
    fn a_flag_set_after_the_probe_is_refused_on_the_index_copy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        fs::create_dir(&root).unwrap();
        let repo = GitCapture {
            git: Git::new(&root).with_date(1_760_000_000_000),
            root: &root,
            job_id: JobId::from_bytes([1; 16]),
            scratch: dir.path().to_owned(),
            user_excludes: String::new(),
            listing_excludes: TempFile::in_dir(dir.path()),
        };
        repo.git.run(&["init", "-q", "-b", "main"]).unwrap();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        repo.git.run(&["add", "a.txt"]).unwrap();
        repo.git.run(&["commit", "-q", "-m", "one"]).unwrap();
        let head = repo.head_commit().unwrap();
        assert!(repo.index_commit(head.as_ref()).is_ok());

        repo.git
            .run(&["update-index", "--assume-unchanged", "a.txt"])
            .unwrap();
        assert!(matches!(
            repo.index_commit(head.as_ref()),
            Err(CaptureError::GitIndexFlag { flag: IndexFlag::AssumeUnchanged, path }) if path.as_str() == "a.txt"
        ));
        repo.git
            .run(&["update-index", "--no-assume-unchanged", "a.txt"])
            .unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        repo.git.run(&["add", "-N", "b.txt"]).unwrap();
        assert!(matches!(
            repo.index_commit(head.as_ref()),
            Err(CaptureError::GitIndexFlag { flag: IndexFlag::IntentToAdd, path }) if path.as_str() == "b.txt"
        ));
    }

    /// A ref that moves after the bundle is made is refused, full or incremental.
    #[test]
    fn a_ref_the_bundle_does_not_hold_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        fs::create_dir(&root).unwrap();
        let repo = GitCapture {
            git: Git::new(&root).with_date(1_760_000_000_000),
            root: &root,
            job_id: JobId::from_bytes([1; 16]),
            scratch: dir.path().to_owned(),
            user_excludes: String::new(),
            listing_excludes: TempFile::in_dir(dir.path()),
        };
        let commit = |message: &str| {
            repo.git
                .run(&["commit", "-q", "--allow-empty", "-m", message])
                .unwrap();
        };
        repo.git.run(&["init", "-q", "-b", "main"]).unwrap();
        commit("one");
        let view = repo.ref_view().unwrap();
        let full = repo.bundle(&BundlePlan::Full).unwrap();
        assert!(repo.confirm_bundle(&full, &view, &[]).is_ok());

        commit("two");
        let moved = repo.ref_view().unwrap();
        assert!(matches!(
            repo.confirm_bundle(&full, &moved, &[]),
            Err(CaptureError::RefsMoved { moved: MovedRef::Ref(name) }) if name.as_str() == "refs/heads/main"
        ));

        let tips: Vec<GitOid> = moved.refs.iter().map(|(_, oid)| oid.clone()).collect();
        commit("three");
        let view = repo.ref_view().unwrap();
        let plan = BundlePlan::Incremental {
            base: SnapshotId::of(b"parent"),
            tips: tips.clone(),
        };
        let incremental = repo.bundle(&plan).unwrap();
        assert!(matches!(incremental, BuiltBundle::Incremental { .. }));
        assert!(repo.confirm_bundle(&incremental, &view, &tips).is_ok());
        repo.git.run(&["checkout", "-q", "-b", "side"]).unwrap();
        commit("four");
        repo.git.run(&["checkout", "-q", "main"]).unwrap();
        let grown = repo.ref_view().unwrap();
        assert!(matches!(
            repo.confirm_bundle(&incremental, &grown, &tips),
            Err(CaptureError::RefsMoved { moved: MovedRef::Ref(name) }) if name.as_str() == "refs/heads/side"
        ));
    }

    #[test]
    fn userinfo_is_stripped_from_remote_urls() {
        assert_eq!(
            strip_userinfo("https://user:tok@example.com/x.git").as_deref(),
            Some("https://example.com/x.git")
        );
        assert_eq!(strip_userinfo("ssh://git@github.com/x/y"), None);
        assert_eq!(
            strip_userinfo("ssh://git:pw@github.com/x/y").as_deref(),
            Some("ssh://git@github.com/x/y")
        );
        assert_eq!(strip_userinfo("https://example.com/a@b"), None);
        assert_eq!(strip_userinfo("git@github.com:x/y.git"), None);
    }
}
