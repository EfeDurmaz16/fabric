//! Workspace restore (ADR 0004, Restore).
//!
//! Invariant: restore writes into a staging directory beside the target and
//! renames it into place only after every byte is written and, when asked,
//! verified. Nothing is written through a symlink or outside the staging
//! directory, an existing target with content is never touched, and a crash
//! leaves either no target or a complete one: a staging directory that
//! outlives its process is the sign of a restore that did not finish.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;

use fabric_core::id::SnapshotId;
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::{
    CacheEntry, CarryState, Entry, EntryKind, GitHead, GitState, SnapshotManifest,
};

use crate::capture::symlink_escapes;
use crate::checkpoint;
use crate::durable;
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
    /// A power loss: every file's pages reach the drive before the rename, and the rename's directory flush empties the drive cache.
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
    /// Files re-read and found equal to the store.
    pub verified: u64,
    /// Cache rows whose bytes were not written.
    pub skipped_caches: Vec<RelPath>,
    /// Submodule paths left uninitialized.
    pub submodules: Vec<RelPath>,
    /// Symlinks written whose target is absolute or climbs above the root.
    pub escaping_symlinks: Vec<RelPath>,
    /// Bundles fetched into the repository.
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
    /// The store refused.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The tree writer refused a path.
    #[error(transparent)]
    Write(#[from] WriteError),
    /// An object could not be read.
    #[error(transparent)]
    Object(#[from] ObjectError),
    /// The target is not an absolute path with a parent directory.
    #[error("{path} is not an absolute path with a parent")]
    BadTarget {
        /// The path given.
        path: PathBuf,
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
    /// A bundle in the chain names a base snapshot the store does not have.
    #[error("bundle chain of {snapshot} needs {base}, which is not in the store")]
    BundleChainBroken {
        /// The snapshot being restored.
        snapshot: SnapshotId,
        /// The missing base.
        base: SnapshotId,
    },
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
#[must_use]
pub fn restore_state(target: &Path) -> RestoreState {
    if let Ok(staging) = staging_path(target)
        && staging.exists()
    {
        return RestoreState::HalfRestored { staging };
    }
    match fs::read_dir(target) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                RestoreState::Present
            } else {
                RestoreState::Absent
            }
        }
        Err(_) => RestoreState::Absent,
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

/// Makes a fresh staging directory for `target`, removing a stale one.
fn fresh_staging(target: &Path) -> Result<PathBuf, RestoreError> {
    let staging = staging_path(target)?;
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

/// Writes a list of entries: directories, then files, then symlinks, then directory mtimes.
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
            writer.mkdir(&entry.rel_path, entry.mode.bits())?;
        }
    }
    for entry in &kept {
        if let EntryKind::File { size, chunks } = &entry.kind {
            let mut file = writer.create_file(&entry.rel_path, entry.mode.bits())?;
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
    let chosen: Vec<&Entry> = match mode {
        Verify::None => Vec::new(),
        Verify::All => files,
        Verify::Sample(n) => {
            if n == 0 || files.is_empty() {
                Vec::new()
            } else {
                let step = (files.len() / n).max(1);
                files.iter().step_by(step).copied().collect()
            }
        }
    };
    let mut verified = 0;
    for entry in chosen {
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

/// One entry of `git ls-tree -r`.
struct TreeItem {
    mode: &'static str,
    oid: String,
    rel: RelPath,
}

fn parse_tree(bytes: &[u8]) -> Result<Vec<TreeItem>, RestoreError> {
    let mut items = Vec::new();
    for record in Git::records(bytes) {
        let text = String::from_utf8_lossy(record).into_owned();
        let bad = || RestoreError::BadTreeOutput { text: text.clone() };
        let (meta, path) = text.split_once('\t').ok_or_else(bad)?;
        let mut fields = meta.split(' ');
        let mode = match fields.next() {
            Some("100644") => "100644",
            Some("100755") => "100755",
            Some("120000") => "120000",
            Some("160000") => "160000",
            _ => return Err(bad()),
        };
        let _kind = fields.next().ok_or_else(bad)?;
        let oid = fields.next().ok_or_else(bad)?.to_owned();
        let rel = RelPath::parse(path).map_err(|source| RestoreError::BadTreePath {
            path: path.to_owned(),
            source,
        })?;
        items.push(TreeItem { mode, oid, rel });
    }
    Ok(items)
}

/// Streams blobs out of `git cat-file --batch` and hands each to `on_blob`.
fn cat_file_batch(
    git: &Git,
    oids: Vec<String>,
    mut on_blob: impl FnMut(&str, &[u8]) -> Result<(), RestoreError>,
) -> Result<(), RestoreError> {
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
    let feeder = thread::spawn(move || {
        for oid in oids {
            if writeln!(stdin, "{oid}").is_err() {
                break;
            }
        }
    });
    let mut reader = BufReader::new(stdout);
    let outcome = (|| {
        let mut header = String::new();
        loop {
            header.clear();
            if reader
                .read_line(&mut header)
                .map_err(|e| io_err(Path::new("cat-file"), e))?
                == 0
            {
                return Ok(());
            }
            let bad = || RestoreError::BadTreeOutput {
                text: header.trim().to_owned(),
            };
            let mut fields = header.split_whitespace();
            let oid = fields.next().ok_or_else(bad)?.to_owned();
            let kind = fields.next().ok_or_else(bad)?;
            if kind == "missing" {
                return Err(bad());
            }
            let size: usize = fields.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
            let mut bytes = vec![0u8; size];
            reader
                .read_exact(&mut bytes)
                .map_err(|e| io_err(Path::new("cat-file"), e))?;
            let mut newline = [0u8; 1];
            reader
                .read_exact(&mut newline)
                .map_err(|e| io_err(Path::new("cat-file"), e))?;
            on_blob(&oid, &bytes)?;
        }
    })();
    let _ = feeder.join();
    let status = child.wait().map_err(|e| io_err(Path::new("cat-file"), e))?;
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

/// Restores the repository: objects, refs, `HEAD`, the index and the working tree.
fn restore_git(
    store: &Store,
    snapshot: SnapshotId,
    manifest: &SnapshotManifest,
    git_state: &GitState,
    staging: &Path,
    writer: &TreeWriter,
    report: &mut RestoreReport,
) -> Result<Git, RestoreError> {
    let git = Git::new(staging, manifest.created_at);
    git.run(&["init", "-q"])?;
    let scratch = git.git_path("fabric-tmp")?;
    fs::create_dir_all(&scratch).map_err(|e| io_err(&scratch, e))?;
    if let Some(chunks) = &git_state.exclude {
        let path = git.git_path("info/exclude")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        let mut bytes = Vec::new();
        for chunk in chunks {
            bytes.extend_from_slice(&store.objects().read(*chunk)?);
        }
        fs::write(&path, bytes).map_err(|e| io_err(&path, e))?;
    }

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
        let path = durable::temp_path(&scratch);
        {
            let mut file = fs::File::create(&path).map_err(|e| io_err(&path, e))?;
            for chunk in &state.bundle {
                file.write_all(&store.objects().read(*chunk)?)
                    .map_err(|e| io_err(&path, e))?;
            }
        }
        let path_text = path.to_string_lossy().into_owned();
        git.run(&["bundle", "verify", &path_text])?;
        git.run(&[
            "fetch",
            "-q",
            "--update-head-ok",
            &path_text,
            "+refs/*:refs/*",
        ])?;
        let _ = fs::remove_file(&path);
        report.bundles_fetched += 1;
    }

    let existing = git.run_text(&["for-each-ref", "--format=%(refname)"])?;
    for name in existing.lines() {
        if !git_state.refs.iter().any(|(r, _)| r.as_str() == name) {
            git.run(&["update-ref", "-d", name])?;
        }
    }
    for (name, oid) in &git_state.refs {
        if name.as_str() == "refs/stash" {
            // `git stash list` prints the reflog message, so the entry gets
            // the stash commit's own subject back.
            let subject = git.run_text(&["log", "-1", "--format=%s", oid.as_str()])?;
            let _ = git.output(&["update-ref", "-d", "refs/stash"], &[])?;
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
    git.run(&["read-tree", git_state.index_commit.as_str()])?;

    let listing = git.run(&["ls-tree", "-r", "-z", git_state.worktree_commit.as_str()])?;
    let items = parse_tree(&listing)?;
    let mtimes: HashMap<&RelPath, u64> = git_state
        .worktree_mtimes
        .iter()
        .map(|(p, t)| (p, *t))
        .collect();
    let mut by_oid: HashMap<String, Vec<usize>> = HashMap::new();
    let mut blob_oids = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if item.mode == "160000" {
            writer.mkdir(&item.rel, 0o755)?;
            report.submodules.push(item.rel.clone());
        } else {
            if !by_oid.contains_key(&item.oid) {
                blob_oids.push(item.oid.clone());
            }
            by_oid.entry(item.oid.clone()).or_default().push(index);
        }
    }
    let mut symlinks: Vec<(usize, String)> = Vec::new();
    cat_file_batch(&git, blob_oids, |oid, bytes| {
        let Some(indexes) = by_oid.get(oid) else {
            return Err(RestoreError::BadTreeOutput {
                text: oid.to_owned(),
            });
        };
        for &index in indexes {
            let item = &items[index];
            match item.mode {
                "120000" => {
                    symlinks.push((index, String::from_utf8_lossy(bytes).into_owned()));
                }
                mode => {
                    let bits = if mode == "100755" { 0o755 } else { 0o644 };
                    let mut file = writer.create_file(&item.rel, bits)?;
                    file.write_all(bytes)
                        .map_err(|e| io_err(Path::new(item.rel.as_str()), e))?;
                    drop(file);
                    if let Some(mtime) = mtimes.get(&item.rel) {
                        writer.set_mtime(&item.rel, *mtime)?;
                    }
                }
            }
        }
        Ok(())
    })?;
    for (index, target) in symlinks {
        let item = &items[index];
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
    let _ = fs::remove_dir_all(&scratch);
    Ok(git)
}

/// Materializes a snapshot into `req.target`.
pub fn restore(store: &Store, req: &RestoreRequest<'_>) -> Result<RestoreReport, RestoreError> {
    let manifest = store.manifest(req.snapshot)?;
    check_target_empty(req.target)?;
    if let Some(dir) = req.harness_state {
        check_target_empty(dir)?;
    }
    let staging = fresh_staging(req.target)?;
    let writer = TreeWriter::open(&staging)?;
    let mut report = RestoreReport::default();

    let git = match &manifest.git {
        Some(state) => Some(restore_git(
            store,
            req.snapshot,
            &manifest,
            state,
            &staging,
            &writer,
            &mut report,
        )?),
        None => None,
    };
    checkpoint::reached("restore.after_git");

    let skipped: Vec<RelPath> = manifest
        .caches
        .iter()
        .filter(|c| c.carry == CarryState::NotCarried || !(req.carry)(c))
        .map(|c| c.path.clone())
        .collect();
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

    if let Some(dir) = req.harness_state {
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
    }

    if let (Some(git), Some(state)) = (&git, &manifest.git) {
        for (key, value) in &state.config {
            git.run(&["config", "--local", &key.to_string(), value.as_str()])?;
        }
    }
    checkpoint::reached("restore.before_rename");
    move_into_place(&staging, req.target)?;
    checkpoint::reached("restore.after_rename");
    tracing::info!(snapshot = %req.snapshot, target = %req.target.display(), files = report.files, "restored");
    Ok(report)
}

/// Re-reads the `files` of a snapshot under `root` and returns how many matched.
///
/// This is how a caller tells a complete tree from one that was cut short
/// after the rename by a power loss; a half restore before the rename shows
/// as `RestoreState::HalfRestored` instead.
pub fn verify(
    store: &Store,
    snapshot: SnapshotId,
    root: &Path,
    mode: Verify,
    carry: &dyn Fn(&CacheEntry) -> bool,
) -> Result<u64, RestoreError> {
    let manifest = store.manifest(snapshot)?;
    let writer = TreeWriter::open(root)?;
    let skipped: Vec<RelPath> = manifest
        .caches
        .iter()
        .filter(|c| c.carry == CarryState::NotCarried || !carry(c))
        .map(|c| c.path.clone())
        .collect();
    let skip = |rel: &RelPath| skipped.iter().any(|root| rel.starts_with(root));
    verify_entries(store, &writer, &manifest.files, &skip, mode)
}
