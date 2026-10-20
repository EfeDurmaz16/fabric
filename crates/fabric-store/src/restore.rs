//! Workspace restore (ADR 0004, Restore; as built in ADR 0014, Restore).
//!
//! Invariant: restore writes into a staging directory beside the target and
//! renames it into place only after every byte is written and, when asked,
//! verified. Nothing is written through a symlink or outside the staging
//! directory, an existing target with content is never touched, and a crash
//! leaves either no target or a complete one: a staging directory that
//! outlives its process is the sign of a restore that did not finish.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;

use fabric_core::git::{GitConfigKey, GitOid};
use fabric_core::id::SnapshotId;
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::{
    CacheEntry, CarryState, Entry, EntryKind, GitHead, GitState, Mode, SnapshotManifest,
};

use crate::capture::symlink_escapes;
use crate::checkpoint;
use crate::durable::{self, TempFile};
use crate::git::{Git, GitError};
use crate::objects::ObjectError;
use crate::store::{Store, StoreError};
use crate::writer::{TreeWriter, WriteError};

/// The suffix of the staging directory a restore writes into.
pub const STAGING_SUFFIX: &str = ".fabric-restoring";

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
    /// A power loss: every file restore writes, and the index, refs, packs and config git writes, reach the drive before the rename, and the rename's directory flush empties the drive cache.
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

/// What restore wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Files and symlinks written from `files` and `harness_state`.
    pub files: u64,
    /// Their bytes.
    pub bytes: u64,
    /// Files and links re-read and found equal to the store or to the working-tree commit.
    pub verified: u64,
    /// Cache rows whose bytes were not written.
    pub skipped_caches: Vec<RelPath>,
    /// Submodule paths left uninitialized.
    pub submodules: Vec<RelPath>,
    /// Symlinks written whose target is absolute or climbs above the root.
    pub escaping_symlinks: Vec<RelPath>,
    /// Bundles unpacked into the repository.
    pub bundles_fetched: u32,
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

/// Says whether `target` is absent, half restored or present.
pub fn restore_state(target: &Path) -> Result<RestoreState, RestoreError> {
    if let Ok(staging) = staging_path(target)
        && staging.exists()
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

/// Makes a fresh staging directory for `target`, removing a stale one.
fn fresh_staging(target: &Path) -> Result<PathBuf, RestoreError> {
    let staging = staging_path(target)?;
    if fs::symlink_metadata(&staging).is_ok_and(|meta| meta.is_dir()) {
        open_up(&staging)?;
    }
    match fs::remove_dir_all(&staging) {
        Ok(()) => tracing::info!(path = %staging.display(), "removed a half-finished restore"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&staging, e)),
    }
    fs::create_dir(&staging).map_err(|e| io_err(&staging, e))?;
    Ok(staging)
}

/// Renames the staging directory onto the target and flushes the parent.
fn move_into_place(staging: &Path, target: &Path) -> Result<(), RestoreError> {
    fs::rename(staging, target).map_err(|e| io_err(target, e))?;
    if let Some(parent) = target.parent() {
        durable::fsync_dir(parent).map_err(|e| io_err(parent, e))?;
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
            if durability == Durability::PowerLoss {
                // Plain fsync(2); the parent directory's F_FULLFSYNC after the
                // rename flushes the device cache once for all of them.
                rustix::fs::fsync(&file)
                    .map_err(|e| io_err(Path::new(entry.rel_path.as_str()), e.into()))?;
            }
            drop(file);
            writer.set_mtime(&entry.rel_path, entry.mtime_ns)?;
            report.files += 1;
            report.bytes += written;
            if report.files == 1 {
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
            report.files += 1;
        }
    }
    for entry in kept.iter().rev() {
        if matches!(entry.kind, EntryKind::Dir) {
            writer.set_dir_mode(&entry.rel_path, entry.mode)?;
            writer.set_mtime(&entry.rel_path, entry.mtime_ns)?;
        }
    }
    Ok(())
}

/// Re-reads written files against the store and returns how many matched.
fn verify_entries(
    store: &Store,
    writer: &TreeWriter,
    entries: &[Entry],
    skip: &dyn Fn(&RelPath) -> bool,
    mode: Verify,
) -> Result<u64, RestoreError> {
    let files: Vec<&Entry> = entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::File { .. }) && !skip(&e.rel_path))
        .collect();
    let mut verified = 0;
    for entry in sample(&files, mode) {
        let EntryKind::File { chunks, .. } = &entry.kind else {
            continue;
        };
        let mut file = writer.open_file(&entry.rel_path)?;
        let mismatch = || RestoreError::VerifyFailed {
            path: entry.rel_path.clone(),
        };
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
        let text = String::from_utf8_lossy(record).into_owned();
        let bad = || RestoreError::BadTreeOutput { text: text.clone() };
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
    let Some(mut stdin) = child.stdin.take() else {
        return Err(RestoreError::BadTreeOutput {
            text: "cat-file has no stdin".to_owned(),
        });
    };
    let Some(stdout) = child.stdout.take() else {
        return Err(RestoreError::BadTreeOutput {
            text: "cat-file has no stdout".to_owned(),
        });
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

/// Unpacks every bundle of the chain that ends at `manifest`, oldest first, into `git`'s repository.
///
/// Unbundling indexes each pack and writes no refs, so the refs an older
/// link lists can never collide with the ones `set_refs` writes.
fn unbundle_chain(
    store: &Store,
    snapshot: SnapshotId,
    manifest: &SnapshotManifest,
    git: &Git,
    report: &mut RestoreReport,
) -> Result<(), RestoreError> {
    // The chain of bundles, oldest first.
    let mut chain = vec![manifest.clone()];
    loop {
        let base = chain
            .last()
            .and_then(|m| m.git.as_ref())
            .and_then(|g| g.bundle_base);
        let Some(base) = base else { break };
        match store.manifest(base) {
            Ok(m) => chain.push(m),
            Err(StoreError::UnknownSnapshot(_)) => {
                return Err(RestoreError::BundleChainBroken { snapshot, base });
            }
            Err(e) => return Err(e.into()),
        }
    }
    chain.reverse();
    for link in &chain {
        let Some(state) = &link.git else { continue };
        if state.bundle.is_empty() {
            continue;
        }
        let bundle = TempFile::in_dir(store.objects().tmp_dir());
        let path = bundle.path();
        {
            let mut file = fs::File::create(path).map_err(|e| io_err(path, e))?;
            for chunk in &state.bundle {
                file.write_all(&store.objects().read(*chunk)?)
                    .map_err(|e| io_err(path, e))?;
            }
        }
        let path_text = path.to_string_lossy().into_owned();
        git.run(&["bundle", "verify", &path_text])?;
        git.run(&["bundle", "unbundle", &path_text])?;
        report.bundles_fetched += 1;
    }
    Ok(())
}

/// Sets every ref and `HEAD` exactly as `git_state` records them.
fn set_refs(git: &Git, git_state: &GitState) -> Result<(), RestoreError> {
    for (name, oid) in &git_state.refs {
        if name.as_str() == "refs/stash" {
            // `git stash list` prints the reflog message, so the entry gets
            // the stash commit's own subject back.
            let subject = git.run_text(&["log", "-1", "--format=%s", oid.as_str()])?;
            git.run(&[
                "update-ref",
                "--create-reflog",
                "-m",
                &subject,
                "refs/stash",
                oid.as_str(),
            ])?;
        } else {
            git.run(&["update-ref", name.as_str(), oid.as_str()])?;
        }
    }
    match &git_state.head {
        GitHead::Branch(branch) => {
            let full = format!("refs/heads/{branch}");
            git.run(&["symbolic-ref", "HEAD", &full])?;
        }
        GitHead::Detached(oid) => {
            git.run(&["update-ref", "--no-deref", "HEAD", oid.as_str()])?;
        }
    }
    Ok(())
}

/// Restores the repository: objects, refs, `HEAD`, the index and the working tree.
fn restore_git(
    store: &Store,
    snapshot: SnapshotId,
    manifest: &SnapshotManifest,
    git_state: &GitState,
    target: &StagingTarget<'_>,
    report: &mut RestoreReport,
) -> Result<Git, RestoreError> {
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
    git.run(&["init", "-q"])?;
    if let Some(chunks) = &git_state.exclude {
        let path = git.git_path("info/exclude")?;
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

    unbundle_chain(store, snapshot, manifest, &git, report)?;
    set_refs(&git, git_state)?;
    git.run(&["read-tree", git_state.index_commit.as_str()])?;

    let listing = git.run(&["ls-tree", "-r", "-z", git_state.worktree_commit.as_str()])?;
    let items = parse_tree(&listing)?;
    let mtimes: HashMap<&RelPath, u64> = git_state
        .worktree_mtimes
        .iter()
        .map(|(p, t)| (p, *t))
        .collect();
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
                let bits = if item.mode == TreeMode::Exec {
                    0o755
                } else {
                    0o644
                };
                let mut file = writer.create_file(&item.rel, Mode::from_masked(bits))?;
                let written = io::copy(body, &mut file).map_err(|e| io_err(path, e))?;
                if written != size {
                    return Err(RestoreError::SizeMismatch {
                        path: item.rel.clone(),
                        written,
                        size,
                    });
                }
                if power_loss {
                    rustix::fs::fsync(&file).map_err(|e| io_err(path, e.into()))?;
                }
                drop(file);
                if let Some(mtime) = mtimes.get(&item.rel) {
                    writer.set_mtime(&item.rel, *mtime)?;
                }
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
    }
    // Nonzero whenever a file differs from the index, which is expected here.
    let _ = git.output(&["update-index", "--refresh"], &[])?;
    Ok(git)
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

/// Re-reads the working tree against commit `W` and returns how many entries matched.
///
/// Files are hashed by `git hash-object --no-filters` and compared with the
/// tree's blob ids; links are compared with their blobs' bytes.
fn verify_worktree(
    git: &Git,
    worktree: &GitOid,
    writer: &TreeWriter,
    mode: Verify,
) -> Result<u64, RestoreError> {
    let listing = git.run(&["ls-tree", "-r", "-z", worktree.as_str()])?;
    let items = parse_tree(&listing)?;
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
/// is foreign and is refused.
fn harness_state_complete(
    store: &Store,
    entries: &[Entry],
    dir: &Path,
) -> Result<Option<u64>, RestoreError> {
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
    match verify_entries(store, &writer, entries, &|_| false, Verify::All) {
        Ok(verified) => Ok(Some(verified)),
        Err(RestoreError::VerifyFailed { .. }) => Err(occupied),
        Err(e) => Err(e),
    }
}

/// The cache paths whose bytes are not written: rows not carried and rows `carry` turns down.
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
        Some(dir) => harness_state_complete(store, &manifest.harness_state.entries, dir)?,
        None => None,
    };
    let staging = fresh_staging(req.target)?;
    let writer = TreeWriter::open(&staging)?;
    let mut report = RestoreReport::default();

    let git = match &manifest.git {
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
    write_entries(
        store,
        &writer,
        &manifest.files,
        &skip,
        req.durability,
        &mut report,
    )?;
    report.skipped_caches.clone_from(&skipped);
    checkpoint::reached("restore.after_files");
    report.verified += verify_entries(store, &writer, &manifest.files, &skip, req.verify)?;
    if let (Some(git), Some(state)) = (&git, &manifest.git) {
        report.verified += verify_worktree(git, &state.worktree_commit, &writer, req.verify)?;
    }

    match (req.harness_state, harness_done) {
        (Some(_), Some(verified)) => report.verified += verified,
        (Some(dir), None) => {
            let hs_staging = fresh_staging(dir)?;
            let hs_writer = TreeWriter::open(&hs_staging)?;
            let keep_all = |_: &RelPath| false;
            write_entries(
                store,
                &hs_writer,
                &manifest.harness_state.entries,
                &keep_all,
                req.durability,
                &mut report,
            )?;
            report.verified += verify_entries(
                store,
                &hs_writer,
                &manifest.harness_state.entries,
                &keep_all,
                req.verify,
            )?;
            move_into_place(&hs_staging, dir)?;
            checkpoint::reached("restore.after_harness_rename");
        }
        (None, _) => {}
    }

    if let (Some(git), Some(state)) = (&git, &manifest.git) {
        for (key, value) in &state.config {
            // These describe the source's filesystem; `git init` probed the target's.
            if matches!(
                key,
                GitConfigKey::CoreIgnoreCase | GitConfigKey::CoreSymlinks
            ) {
                continue;
            }
            git.run(&["config", "--local", &key.to_string(), value.as_str()])?;
        }
    }
    checkpoint::reached("restore.before_rename");
    move_into_place(&staging, req.target)?;
    checkpoint::reached("restore.after_rename");
    tracing::info!(snapshot = %req.snapshot, target = %req.target.display(), files = report.files, "restored");
    Ok(report)
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
        verified += verify_worktree(&git, &state.worktree_commit, &writer, mode)?;
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
