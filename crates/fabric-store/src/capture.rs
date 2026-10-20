//! Workspace capture (ADR 0004, What a workspace snapshot contains).
//!
//! Invariant: capture reads the workspace and writes only objects into the
//! store, two private commits and one ref under `refs/fabric/` into the
//! repository, and deletes the job's older refs under `refs/fabric/`. Its
//! scratch files (index copies, the bundle) live in the store's `tmp/`,
//! which open and gc clear, never in the user's `.git`. It never touches
//! the real index, the working tree or the user's refs. A path or state that could not be restored faithfully fails
//! here, where the user can act, and a secret never becomes an object.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use fabric_core::git::{BranchName, GitConfigKey, GitConfigValue, GitOid, RefName};
use fabric_core::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use fabric_core::path::{AbsPath, PathError, RelPath};
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, Entry, EntryKind, GitHead, GitState,
    HarnessState, ManifestError, Mode, Platform, SnapshotManifest,
};

use crate::durable::TempFile;
use crate::git::{Git, GitError};
use crate::objects::ObjectError;
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

/// Carry keys for a node that measured nothing; no target ever matches them.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnmeasuredKeys;

impl CarryKeys for UnmeasuredKeys {
    fn key(&self, _class: CacheClass, _path: &RelPath) -> CarryKey {
        CarryKey::Other {
            tool: "unmeasured".to_owned(),
            version: String::new(),
        }
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
    /// A per-file size above which capture fails naming the file.
    pub max_file_size: Option<u64>,
    /// Let untracked files above `LARGE_UNTRACKED_LIMIT` enter the working-tree commit.
    pub allow_large_untracked: bool,
}

/// What kind of bundle the capture produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BundleKind {
    /// The root is not a repository.
    #[default]
    NoGit,
    /// Every object the refs reach.
    Full,
    /// Only objects the parent's refs did not reach.
    Incremental,
    /// The parent's refs already reached every object; nothing to carry.
    Empty,
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
    /// Symlinks whose target is absolute or climbs above the root.
    pub escaping_symlinks: Vec<RelPath>,
    /// Nested repositories left behind, tracked submodules aside.
    pub nested_repositories: Vec<String>,
    /// Config keys whose remote URL lost its userinfo.
    pub stripped_userinfo: Vec<String>,
    /// Repeated config keys where only the last value travels.
    pub repeated_config: Vec<String>,
    /// The bundle shape.
    pub bundle: BundleKind,
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
    /// The store refused.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An object could not be written.
    #[error(transparent)]
    Object(#[from] ObjectError),
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
        reason: String,
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
        path: String,
    },
    /// A path has the LFS filter; the working-tree commit would hold pointers.
    #[error("{path} has filter=lfs, which capture does not support")]
    GitLfsUnsupported {
        /// The first path git says the filter applies to.
        path: String,
    },
    /// Untracked files above the limit would enter the user's object store.
    #[error(
        "untracked files above {LARGE_UNTRACKED_LIMIT} bytes: {paths:?}; ignore them or allow large untracked files"
    )]
    UntrackedTooLarge {
        /// The offending paths.
        paths: Vec<RelPath>,
    },
    /// A file is above the caller's size cap.
    #[error("{path} is {size} bytes, above the cap of {cap}")]
    FileTooLarge {
        /// The file.
        path: RelPath,
        /// Its size.
        size: u64,
        /// The cap.
        cap: u64,
    },
    /// A path the working-tree commit lists vanished before it was measured.
    #[error("{path} changed while capture ran")]
    WorkspaceChanged {
        /// The path.
        path: RelPath,
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

fn mode_of(raw: u32) -> Mode {
    #[expect(
        clippy::unwrap_used,
        reason = "the mask leaves the low nine bits, which Mode::new always accepts"
    )]
    u16::try_from(raw & 0o777).ok().and_then(Mode::new).unwrap()
}

fn mtime_ns(meta: &fs::Metadata) -> u64 {
    let secs = u64::try_from(meta.mtime()).unwrap_or(0);
    let nanos = u64::try_from(meta.mtime_nsec()).unwrap_or(0);
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
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

/// One directory, file or symlink found on disk.
struct Found {
    rel: RelPath,
    meta: fs::Metadata,
}

/// Lists a directory tree, sorted by path, never following symlinks.
///
/// A directory named `.git` in any letter case is a nested repository: it
/// is skipped and its path is appended to `nested`.
fn list_tree(base: &Path, nested: &mut Vec<String>) -> Result<Vec<Found>, CaptureError> {
    fn walk(
        base: &Path,
        prefix: &str,
        out: &mut Vec<Found>,
        nested: &mut Vec<String>,
    ) -> Result<(), CaptureError> {
        let dir = base.join(prefix);
        let mut names: Vec<_> = fs::read_dir(&dir)
            .map_err(|e| io_err(&dir, e))?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()
            .map_err(|e| io_err(&dir, e))?;
        names.sort();
        for name in names {
            let Some(name) = name.to_str().map(str::to_owned) else {
                return Err(CaptureError::BadPath {
                    path: name.to_string_lossy().into_owned(),
                    source: PathError::NotNfc,
                });
            };
            let text = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let full = base.join(&text);
            let meta = fs::symlink_metadata(&full).map_err(|e| io_err(&full, e))?;
            if meta.is_dir() && name.eq_ignore_ascii_case(".git") {
                nested.push(text);
                continue;
            }
            let rel = rel_path(&text)?;
            let is_dir = meta.is_dir();
            out.push(Found { rel, meta });
            if is_dir {
                walk(base, &text, out, nested)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(base, "", &mut out, nested)?;
    Ok(out)
}

/// Counts of what the policy did under one cache root.
#[derive(Default)]
struct CacheSeen {
    class: Option<CacheClass>,
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
        let full = base.join(found.rel.as_str());
        let kind = if meta.file_type().is_symlink() {
            let target = fs::read_link(&full).map_err(|e| io_err(&full, e))?;
            let target = target.to_string_lossy().into_owned();
            if symlink_escapes(&found.rel, &target) {
                self.report.escaping_symlinks.push(found.rel.clone());
            }
            self.report.files += 1;
            EntryKind::Symlink { target }
        } else if meta.is_dir() {
            EntryKind::Dir
        } else {
            if let Some(cap) = self.req.max_file_size
                && meta.len() > cap
            {
                return Err(CaptureError::FileTooLarge {
                    path: found.rel.clone(),
                    size: meta.len(),
                    cap,
                });
            }
            let stored = self.store.objects().put_file(&full)?;
            self.report.files += 1;
            self.report.bytes += stored.size;
            self.report.new_objects += stored.new_objects;
            self.report.new_bytes += stored.new_bytes;
            EntryKind::File {
                size: stored.size,
                chunks: stored.chunks,
            }
        };
        Ok(Entry {
            rel_path: found.rel.clone(),
            kind,
            mode: mode_of(meta.mode()),
            mtime_ns: mtime_ns(meta),
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
            let classification = policy.classify(&item.rel, &mut markers);
            if item.meta.is_dir() {
                // A directory travels on its own when the walker listed it
                // and nothing denies it, so an empty directory is not lost.
                if classification.class.carried() {
                    emitted_dirs.insert(item.rel.clone());
                    entries.push(self.entry_for(base, &item)?);
                }
                continue;
            }
            let seen = classification.cache_root.as_ref().map(|(root, class)| {
                let seen = caches.entry(root.clone()).or_default();
                seen.class = Some(*class);
                seen
            });
            match classification.class {
                Class::Secret => {
                    if let Some(seen) = seen {
                        seen.denied += 1;
                    }
                    self.report.denied.push(item.rel);
                    continue;
                }
                Class::Excluded => {
                    if let Some(seen) = seen {
                        seen.excluded += 1;
                    }
                    self.report.excluded.push(item.rel);
                    continue;
                }
                Class::Deps | Class::Build | Class::ToolCache | Class::Precious => {
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
                mode: mode_of(meta.mode()),
                mtime_ns: mtime_ns(&meta),
            });
        }
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let mut cache_entries = Vec::new();
        for (path, seen) in caches {
            let Some(class) = seen.class else { continue };
            let carry = if seen.carried == 0 {
                CarryState::NotCarried
            } else if seen.denied > 0 || seen.excluded > 0 {
                CarryState::Partial
            } else {
                CarryState::Carried
            };
            cache_entries.push(CacheEntry {
                key: self.req.carry_keys.key(class, &path),
                path,
                class,
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

    /// Captures the harness state directory: every path but those the secret list names.
    ///
    /// The adapter's state is not a workspace, so the excludes and cache
    /// classes do not apply, but a secret never travels from here either:
    /// Claude Code keeps `.credentials.json` in its state directory on Linux.
    fn harness_entries(&mut self, base: &Path) -> Result<Vec<Entry>, CaptureError> {
        let mut nested = Vec::new();
        let found = list_tree(base, &mut nested)?;
        self.report.nested_repositories.extend(nested);
        let mut entries = Vec::with_capacity(found.len());
        for item in &found {
            if self.req.policy.is_secret(&item.rel) {
                self.report.denied_harness.push(item.rel.clone());
                continue;
            }
            entries.push(self.entry_for(base, item)?);
        }
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

/// The git half of a capture.
struct GitCapture<'a> {
    git: Git,
    root: &'a Path,
    job_id: JobId,
    scratch: PathBuf,
}

impl GitCapture<'_> {
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
        for marker in [
            "rebase-merge",
            "rebase-apply",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
        ] {
            if self.git.git_path(marker)?.exists() {
                return Err(CaptureError::GitBusy {
                    reason: format!("{marker} exists"),
                });
            }
        }
        if !self.git.run(&["ls-files", "-u", "-z"])?.is_empty() {
            return Err(CaptureError::GitBusy {
                reason: "the index has conflict entries".to_owned(),
            });
        }
        let status = self
            .git
            .run(&["status", "--porcelain=v2", "-z", "--untracked-files=no"])?;
        for record in Git::records(&status) {
            let text = Git::text("status record", record)?;
            let mut fields = text.split(' ');
            if fields.next() != Some("1") {
                continue;
            }
            let _xy = fields.next();
            let sub = fields.next().unwrap_or("");
            if sub.starts_with('S') && sub[1..].chars().any(|c| c != '.') {
                let path = text.rsplit(' ').next().unwrap_or("").to_owned();
                return Err(CaptureError::GitSubmoduleDirty { path });
            }
        }
        // git resolves the attributes itself, from every .gitattributes,
        // tracked or not, and from .git/info/attributes.
        let paths = self.git.run(&[
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
                return Err(CaptureError::GitLfsUnsupported {
                    path: String::from_utf8_lossy(triple[0]).into_owned(),
                });
            }
        }
        Ok(())
    }

    /// Lists untracked, not ignored paths; an untracked nested repository is reported, not listed.
    fn untracked(&self, report: &mut CaptureReport) -> Result<Vec<RelPath>, CaptureError> {
        let out = self
            .git
            .run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        let mut paths = Vec::new();
        for record in Git::records(&out) {
            let text = Git::text("untracked path", record)?;
            if let Some(dir) = text.strip_suffix('/') {
                report.nested_repositories.push(dir.to_owned());
                continue;
            }
            paths.push(rel_path(text)?);
        }
        Ok(paths)
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
                report.nested_repositories.push(dir.to_owned());
                continue;
            }
            let rel = rel_path(text)?;
            let full = self.root.join(rel.as_str());
            let meta = fs::symlink_metadata(&full).map_err(|e| io_err(&full, e))?;
            found.push(Found { rel, meta });
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
    fn index_commit(&self, head: Option<&GitOid>) -> Result<(GitOid, String), CaptureError> {
        let index = self.git.git_path("index")?;
        let lock = index.with_extension("lock");
        let copy = TempFile::in_dir(&self.scratch);
        let mut tries = 0;
        loop {
            if lock.exists() {
                tries += 1;
                if tries > 10 {
                    return Err(CaptureError::GitBusy {
                        reason: "index.lock exists".to_owned(),
                    });
                }
                thread::sleep(Duration::from_millis(200));
                continue;
            }
            match fs::copy(&index, copy.path()) {
                Ok(_) => break,
                // No index yet: write-tree on a missing index file writes the empty tree.
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(e) => return Err(io_err(&index, e)),
            }
        }
        let env: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", copy.path().as_os_str())];
        let tree = self.git.run_text_env(&["write-tree"], &env)?;
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
    /// `git add -A` runs with the policy's patterns as the lowest-precedence
    /// excludes, which keeps most secrets out of the user's object store. A
    /// negation in the repository's own ignore files can override that, so
    /// the private index is then listed and every path that is not tracked
    /// and that `leave_out` names is removed before the tree is written. The
    /// removed paths are returned.
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
        fs::write(exclude_file.path(), excludes).map_err(|e| io_err(exclude_file.path(), e))?;
        let env: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", index.path().as_os_str())];
        {
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

    /// Creates the bundle and returns its path with its kind, or no path for an empty one.
    fn bundle(
        &self,
        tips: &[GitOid],
        chain_full: bool,
    ) -> Result<(Option<TempFile>, BundleKind), CaptureError> {
        let path = TempFile::in_dir(&self.scratch);
        let path_text = path.path().to_string_lossy().into_owned();
        let incremental = !tips.is_empty() && !chain_full;
        if incremental {
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
                    return Ok((Some(path), BundleKind::Incremental));
                }
                let stderr = String::from_utf8_lossy(&out.stderr);
                if stderr.contains("empty bundle") {
                    return Ok((None, BundleKind::Empty));
                }
                tracing::warn!(stderr = %stderr.trim(), "incremental bundle failed; making a full one");
            }
        }
        self.git.run(&["bundle", "create", &path_text, "--all"])?;
        self.git.run(&["bundle", "verify", &path_text])?;
        Ok((Some(path), BundleKind::Full))
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
    fn worktree_mtimes(
        &self,
        worktree: &GitOid,
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
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&full).map_err(|e| io_err(&full, e))?;
                if symlink_escapes(&rel, &target.to_string_lossy()) {
                    report.escaping_symlinks.push(rel.clone());
                }
            }
            mtimes.push((rel, mtime_ns(&meta)));
        }
        mtimes.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(mtimes)
    }

    /// Reads the allowlisted projection of the local config.
    fn config(
        &self,
        report: &mut CaptureReport,
    ) -> Result<Vec<(GitConfigKey, GitConfigValue)>, CaptureError> {
        let out = self.git.run(&["config", "--list", "--local", "-z"])?;
        let mut config: BTreeMap<GitConfigKey, GitConfigValue> = BTreeMap::new();
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
                report.stripped_userinfo.push(key.to_string());
            }
            let Ok(value) = GitConfigValue::parse(&value) else {
                continue;
            };
            if config.insert(key.clone(), value).is_some() {
                report.repeated_config.push(key.to_string());
            }
        }
        Ok(config.into_iter().collect())
    }
}

/// Removes `user[:password]@` from a `scheme://` URL's authority.
fn strip_userinfo(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let at = rest[..end].rfind('@')?;
    Some(format!("{scheme}://{}", &rest[at + 1..]))
}

/// Captures the workspace described by `req` into `store` and returns the manifest.
///
/// The manifest is not yet published; the caller does that with
/// `Store::publish`, so the ledger can be the commit point (ADR 0004).
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
            Some(manifest)
        }
        None => None,
    };
    let mut capturer = Capturer {
        store,
        req,
        report: CaptureReport::default(),
    };
    let (git_state, found) = if let Some(gc) = detect_git(req, store.objects().tmp_dir())? {
        gc.refuse_bad_states()?;
        let state = capture_git(&mut capturer, &gc, parent_manifest.as_ref())?;
        (Some(state), gc.ignored(&mut capturer.report)?)
    } else {
        let mut nested = Vec::new();
        let found = list_tree(req.root, &mut nested)?;
        capturer.report.nested_repositories.extend(nested);
        (None, found)
    };
    let (files, caches) = capturer.classify_and_store(req.root, found)?;
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
    manifest.validate()?;
    Ok(Captured {
        manifest,
        report: capturer.report,
    })
}

/// Returns a git capture when the root is the top level of a repository.
fn detect_git<'a>(
    req: &CaptureRequest<'a>,
    scratch: &Path,
) -> Result<Option<GitCapture<'a>>, CaptureError> {
    let git = Git::new(req.root, req.created_at);
    let out = git.output(&["rev-parse", "--show-toplevel"], &[])?;
    if !out.status.success() {
        return Ok(None);
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
    Ok(Some(GitCapture {
        git,
        root: req.root,
        job_id: req.job_id,
        scratch: scratch.to_owned(),
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
    gc: &GitCapture<'_>,
    parent: Option<&SnapshotManifest>,
) -> Result<GitState, CaptureError> {
    let req = capturer.req;
    let policy = req.policy;
    let untracked = gc.untracked(&mut capturer.report)?;
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
    let tracked: BTreeSet<RelPath> = gc.tracked()?.into_iter().collect();
    for rel in &tracked {
        if policy.denies_anywhere(rel) {
            capturer.report.tracked_secrets.push(rel.clone());
        }
    }
    let nested: Vec<RelPath> = capturer
        .report
        .nested_repositories
        .iter()
        .map(|dir| rel_path(dir))
        .collect::<Result<_, _>>()?;

    let head_commit = gc.head_commit()?;
    let (index_commit, index_tree) = gc.index_commit(head_commit.as_ref())?;
    let under_nested = |rel: &RelPath| nested.iter().any(|root| rel.starts_with(root));
    // A nested repository without a commit makes `add -A` fail, so every
    // nested root is excluded up front as well as removed afterwards.
    let mut excludes = policy.gitignore_lines();
    for root in &nested {
        excludes.push('/');
        excludes.push_str(root.as_str());
        excludes.push_str("/\n");
    }
    let (worktree_commit, removed) = gc.worktree_commit(
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
    let snap_ref = format!("{}{}", gc.snap_prefix(), worktree_commit.as_str());
    gc.git
        .run(&["update-ref", &snap_ref, worktree_commit.as_str()])?;

    let (tips, chain_full, parent_id): (Vec<GitOid>, bool, Option<SnapshotId>) = match parent {
        Some(manifest) => match &manifest.git {
            Some(parent_git) => (
                parent_git.refs.iter().map(|(_, oid)| oid.clone()).collect(),
                chain_depth(capturer.store, manifest)? >= MAX_BUNDLE_CHAIN,
                req.parent,
            ),
            None => (Vec::new(), true, None),
        },
        None => (Vec::new(), true, None),
    };
    let (bundle_path, kind) = gc.bundle(&tips, chain_full)?;
    let (bundle, bundle_base) = match (bundle_path, kind) {
        (Some(file), BundleKind::Incremental) => (capturer.store_scratch_file(file)?, parent_id),
        (Some(file), _) => (capturer.store_scratch_file(file)?, None),
        (None, _) => (Vec::new(), parent_id),
    };
    capturer.report.bundle = kind;
    gc.delete_older_snap_refs(&worktree_commit)?;

    let refs = gc.refs()?;
    let head = gc.head()?;
    let worktree_mtimes = gc.worktree_mtimes(&worktree_commit, &mut capturer.report)?;
    let config = gc.config(&mut capturer.report)?;
    let exclude_path = gc.git.git_path("info/exclude")?;
    let exclude = if exclude_path.is_file() {
        let stored = capturer.store.objects().put_file(&exclude_path)?;
        capturer.report.new_objects += stored.new_objects;
        capturer.report.new_bytes += stored.new_bytes;
        Some(stored.chunks)
    } else {
        None
    };
    Ok(GitState {
        bundle,
        bundle_base,
        refs,
        head,
        index_commit,
        worktree_commit,
        worktree_mtimes,
        config,
        exclude,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn userinfo_is_stripped_from_remote_urls() {
        assert_eq!(
            strip_userinfo("https://user:tok@example.com/x.git").as_deref(),
            Some("https://example.com/x.git")
        );
        assert_eq!(
            strip_userinfo("ssh://git@github.com/x/y").as_deref(),
            Some("ssh://github.com/x/y")
        );
        assert_eq!(strip_userinfo("https://example.com/a@b"), None);
        assert_eq!(strip_userinfo("git@github.com:x/y.git"), None);
    }
}
