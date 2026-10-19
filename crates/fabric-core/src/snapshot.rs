//! Snapshot manifests (ADR 0004).
//!
//! Invariant: a manifest describes a workspace by relative paths and content
//! hashes and holds no file bytes. Every list in it is strictly sorted by its
//! key, so one tree has one manifest, and its postcard encoding is the thing
//! that is hashed: two manifests with the same fields have the same
//! `SnapshotId` on every node and in every version of this crate that writes
//! format version 1. A tree never places an entry below a file or a symlink,
//! and no mode bit above the low nine travels.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::codec::{self, CodecError, VersionedError};
use crate::git::{BranchName, GitConfigKey, GitConfigValue, GitOid, RefName};
use crate::harness::HarnessState;
use crate::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use crate::path::{AbsPath, RelPath};

/// The manifest format version this crate writes; the first varint of every encoded manifest.
pub const MANIFEST_VERSION: u16 = 1;

/// The most bytes a reader decodes as a manifest; a memory bound against a
/// hostile peer, not a cap on snapshots.
pub const MAX_MANIFEST_LEN: usize = 64 << 20;

/// Why a manifest was refused.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The bytes are longer than a reader may decode.
    #[error("manifest is {len} bytes, above the {MAX_MANIFEST_LEN} byte decode bound")]
    TooLarge {
        /// The byte length that was offered.
        len: usize,
    },
    /// The bytes carry a version this crate does not know.
    #[error("manifest version {0} is not supported")]
    UnknownVersion(u16),
    /// A list is not strictly sorted by its key, which also catches a repeated key.
    #[error("{list} entry {index} is not strictly after the one before it")]
    OutOfOrder {
        /// Which list: `files`, `harness_state`, `caches`, `refs`, `worktree_mtimes` or `config`.
        list: &'static str,
        /// Zero-based index of the entry, within its list.
        index: usize,
    },
    /// An entry carries a mode bit above the low nine.
    #[error("entry {index} has mode bits above 0o777")]
    BadMode {
        /// Zero-based index of the entry, within its tree.
        index: usize,
    },
    /// An entry lies below a path that an earlier entry says is a file or a symlink.
    #[error("entry {index} lies below a file or symlink entry")]
    UnderNonDir {
        /// Zero-based index of the entry, within its tree.
        index: usize,
    },
    /// The bytes do not decode, or the value does not encode. A path that
    /// breaks the rules of `RelPath` or `AbsPath` surfaces here too.
    #[error("manifest codec: {0}")]
    Codec(#[from] CodecError),
}

/// What one snapshot of a workspace contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// The job the snapshot belongs to.
    pub job_id: JobId,
    /// The absolute workspace path the capture ran in; the target root on every node.
    pub root: AbsPath,
    /// The snapshot this one follows, if any.
    pub parent: Option<SnapshotId>,
    /// Unix milliseconds at capture; informational.
    pub created_at: u64,
    /// The node that captured it.
    pub created_by: NodeId,
    /// The operating system and architecture of the capturing node.
    pub platform: Platform,
    /// Git state, or `None` when the workspace is not a repository.
    pub git: Option<GitState>,
    /// Ignored files selected by policy, or every path when `git` is `None`; sorted by path.
    pub files: Vec<Entry>,
    /// Opaque state owned by the harness adapter.
    pub harness_state: HarnessState,
    /// The environment spec the job had at capture.
    pub env_spec: Option<EnvSpecId>,
    /// One row per dependency, build or tool-cache path the policy saw; sorted by path.
    pub caches: Vec<CacheEntry>,
}

/// The operating system and architecture of a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// The operating system name, such as `macos`.
    pub os: String,
    /// The architecture name, such as `arm64`.
    pub arch: String,
}

/// One file, directory or symlink in a captured tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The path, relative to the tree's root.
    pub rel_path: RelPath,
    /// What kind of entry it is, with the bytes when it is a file.
    pub kind: EntryKind,
    /// Permission bits; only the low nine exist, `validate` refuses the rest.
    pub mode: u32,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: u64,
}

/// The kind of a tree entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file.
    File {
        /// Byte length of the file.
        size: u64,
        /// The objects that hold the file's bytes, in order.
        chunks: Vec<ChunkId>,
    },
    /// A directory.
    Dir,
    /// A symbolic link written verbatim; restore never writes through it.
    Symlink {
        /// The unparsed link target.
        target: String,
    },
}

/// One cache path and whether the bytes under it were carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The cache path, relative to the root.
    pub path: RelPath,
    /// Which class the policy gave it.
    pub class: CacheClass,
    /// Whether and how far the bytes under the path travel.
    pub carry: CarryState,
    /// The exact strings the source measured; the target compares before trusting the bytes.
    pub key: CarryKey,
}

/// How much of a cache path travels with the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CarryState {
    /// Every byte under the path travels.
    Carried,
    /// The bytes travel but the secret policy denied something under the path; materialize treats it as missing.
    Partial,
    /// Nothing under the path travels: the target cannot use the bytes or the user excluded them.
    NotCarried,
}

/// The class of an ignored cache path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheClass {
    /// A dependency directory the environment spec names.
    Deps,
    /// A build output directory the environment spec names.
    Build,
    /// A known tool cache; dropping it costs only time.
    ToolCache,
}

/// The exact strings a cache depends on (ADR 0012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CarryKey {
    /// A `node_modules` tree.
    Node {
        /// The Node major version, such as `22`.
        node_major: String,
        /// The package manager name, such as `pnpm`.
        package_manager: String,
        /// The package manager major version.
        pm_major: String,
        /// The pnpm store path, when the manager is pnpm.
        pnpm_store_path: Option<String>,
    },
    /// A Cargo `target/` directory.
    Rust {
        /// The whole output of `rustc -vV`.
        rustc_vv: String,
        /// The `CARGO_HOME` path.
        cargo_home: AbsPath,
    },
    /// A Python virtual environment.
    Python {
        /// The interpreter's version string.
        python_version: String,
        /// The interpreter's home path.
        python_home: AbsPath,
    },
    /// Any other ecosystem.
    Other {
        /// The tool's name.
        tool: String,
        /// The tool's version string.
        version: String,
    },
}

/// Git state carried by commits and refs, never by `.git` files.
///
/// Every name in it is a parsed type from `crate::git`, so a manifest from a
/// peer cannot hold a config key outside the allowlist, a ref name git would
/// refuse, or a value that reads as an option on a command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitState {
    /// The objects of the git bundle, full or incremental.
    pub bundle: Vec<ChunkId>,
    /// The snapshot whose refs this bundle excludes; `None` means full.
    pub bundle_base: Option<SnapshotId>,
    /// Every ref name and object id at capture time; sorted by name.
    pub refs: Vec<(RefName, GitOid)>,
    /// What `HEAD` pointed at.
    pub head: GitHead,
    /// Commit `I`: its tree is the index at capture time.
    pub index_commit: GitOid,
    /// Commit `W`: its tree is the working tree at capture time.
    pub worktree_commit: GitOid,
    /// Modification time in nanoseconds for every path in `W`; sorted by path.
    pub worktree_mtimes: Vec<(RelPath, u64)>,
    /// The allowlisted projection of the repository config, never the file; sorted by key.
    pub config: Vec<(GitConfigKey, GitConfigValue)>,
    /// The objects of `.git/info/exclude`, if present.
    pub exclude: Option<Vec<ChunkId>>,
}

/// What `HEAD` pointed at when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitHead {
    /// A branch name, possibly unborn.
    Branch(BranchName),
    /// A detached commit id.
    Detached(GitOid),
}

impl From<VersionedError> for ManifestError {
    fn from(err: VersionedError) -> Self {
        match err {
            VersionedError::UnknownVersion(v) => ManifestError::UnknownVersion(v),
            VersionedError::Codec(c) => ManifestError::Codec(c),
        }
    }
}

/// Returns the index of the first key that is not strictly after the one
/// before it, which also finds a repeated key.
pub(crate) fn first_out_of_order<K: Ord>(keys: impl Iterator<Item = K>) -> Option<usize> {
    let mut previous: Option<K> = None;
    for (index, key) in keys.enumerate() {
        if previous.as_ref().is_some_and(|p| *p >= key) {
            return Some(index);
        }
        previous = Some(key);
    }
    None
}

/// Checks that `keys` rise strictly.
fn check_sorted<K: Ord>(
    list: &'static str,
    keys: impl Iterator<Item = K>,
) -> Result<(), ManifestError> {
    match first_out_of_order(keys) {
        Some(index) => Err(ManifestError::OutOfOrder { list, index }),
        None => Ok(()),
    }
}

fn check_tree(list: &'static str, entries: &[Entry]) -> Result<(), ManifestError> {
    check_sorted(list, entries.iter().map(|e| &e.rel_path))?;
    let mut non_dirs = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.mode & !0o777 != 0 {
            return Err(ManifestError::BadMode { index });
        }
        let path = entry.rel_path.as_str();
        let under_non_dir = path
            .match_indices('/')
            .any(|(i, _)| non_dirs.contains(&path[..i]));
        if under_non_dir {
            return Err(ManifestError::UnderNonDir { index });
        }
        if !matches!(entry.kind, EntryKind::Dir) {
            non_dirs.insert(path);
        }
    }
    Ok(())
}

impl SnapshotManifest {
    /// Checks the manifest's own rules.
    pub fn validate(&self) -> Result<(), ManifestError> {
        check_tree("files", &self.files)?;
        check_tree("harness_state", &self.harness_state.entries)?;
        check_sorted("caches", self.caches.iter().map(|c| &c.path))?;
        if let Some(git) = &self.git {
            check_sorted("refs", git.refs.iter().map(|(name, _)| name))?;
            check_sorted(
                "worktree_mtimes",
                git.worktree_mtimes.iter().map(|(p, _)| p),
            )?;
            check_sorted("config", git.config.iter().map(|(key, _)| key))?;
        }
        Ok(())
    }

    /// Returns the canonical bytes of a valid manifest: the version, then the fields.
    pub fn encode(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        Ok(codec::encode_versioned(MANIFEST_VERSION, self)?)
    }

    /// Decodes bytes under the size bound, refusing other versions, and checks the result.
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_LEN {
            return Err(ManifestError::TooLarge { len: bytes.len() });
        }
        let manifest: SnapshotManifest = codec::decode_versioned(bytes, MANIFEST_VERSION)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Returns the snapshot's identity: blake3 of its canonical bytes.
    pub fn id(&self) -> Result<SnapshotId, ManifestError> {
        Ok(SnapshotId::of(&self.encode()?))
    }
}
