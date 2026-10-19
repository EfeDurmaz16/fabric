//! Snapshot manifests (ADR 0004).
//!
//! Invariant: a manifest describes a workspace by relative paths and content
//! hashes and holds no file bytes. Its canonical postcard encoding is the
//! thing that is hashed, so two manifests with the same fields have the same
//! `SnapshotId` on every node and in every version of this crate that writes
//! format version 1.

use serde::{Deserialize, Serialize};

use crate::harness::HarnessState;
use crate::id::{ChunkId, EnvSpecId, JobId, NodeId, SnapshotId};
use crate::path::{AbsPath, RelPath};

/// The manifest format version this crate writes.
pub const MANIFEST_VERSION: u16 = 1;

/// The most bytes a reader decodes as a manifest; a memory bound against a
/// hostile peer, not a cap on snapshots.
pub const MAX_MANIFEST_LEN: usize = 64 << 20;

/// Files at or below this size are one object; larger files are chunked.
pub const WHOLE_FILE_LIMIT: u64 = 8 << 20;

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
    /// Two entries in the same tree have the same path.
    #[error("entry {index} repeats a path already in the tree")]
    DuplicatePath {
        /// Zero-based index of the second entry, within its tree.
        index: usize,
    },
    /// A directory or symlink entry lists chunks.
    #[error("entry {index} is not a file but lists chunks")]
    ChunksOnNonFile {
        /// Zero-based index of the entry, within its tree.
        index: usize,
    },
    /// The bytes do not decode, or the value does not encode. A path that
    /// breaks the rules of `RelPath` or `AbsPath` surfaces here too.
    #[error("manifest codec: {0}")]
    Codec(#[from] postcard::Error),
}

/// What one snapshot of a workspace contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Format version of this struct.
    pub version: u16,
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
    /// Ignored files selected by policy, or every path when `git` is `None`.
    pub files: Vec<Entry>,
    /// Opaque state owned by the harness adapter.
    pub harness_state: HarnessState,
    /// The environment spec the job had at capture.
    pub env_spec: Option<EnvSpecId>,
    /// One row per dependency, build or tool-cache path the policy saw.
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
    /// What kind of entry it is.
    pub kind: EntryKind,
    /// Permission bits; only the low nine travel.
    pub mode: u32,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: u64,
    /// Byte length of a file; zero for other kinds.
    pub size: u64,
    /// The objects that hold the file's bytes, in order; empty for other kinds.
    pub chunks: Vec<ChunkId>,
}

/// The kind of a tree entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file.
    File,
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
    /// False when the target cannot use the bytes or the user excluded them.
    pub carried: bool,
    /// The exact strings the source measured; the target compares before trusting the bytes.
    pub key: CarryKey,
    /// True when the secret policy denied something under it; materialize treats it as missing.
    pub partial: bool,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitState {
    /// The objects of the git bundle, full or incremental.
    pub bundle: Vec<ChunkId>,
    /// The snapshot whose refs this bundle excludes; `None` means full.
    pub bundle_base: Option<SnapshotId>,
    /// Every ref name and object id at capture time.
    pub refs: Vec<(String, String)>,
    /// What `HEAD` pointed at.
    pub head: GitHead,
    /// Commit `I`: its tree is the index at capture time.
    pub index_commit: String,
    /// Commit `W`: its tree is the working tree at capture time.
    pub worktree_commit: String,
    /// Modification time in nanoseconds for every path in `W`.
    pub worktree_mtimes: Vec<(RelPath, u64)>,
    /// The allowlisted projection of the repository config, never the file.
    pub config: Vec<(String, String)>,
    /// The objects of `.git/info/exclude`, if present.
    pub exclude: Option<Vec<ChunkId>>,
}

/// What `HEAD` pointed at when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitHead {
    /// A branch name, possibly unborn.
    Branch(String),
    /// A detached commit id.
    Detached(String),
}

fn check_tree(entries: &[Entry]) -> Result<(), ManifestError> {
    let mut seen = std::collections::HashSet::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if !seen.insert(&entry.rel_path) {
            return Err(ManifestError::DuplicatePath { index });
        }
        if entry.kind != EntryKind::File && !entry.chunks.is_empty() {
            return Err(ManifestError::ChunksOnNonFile { index });
        }
    }
    Ok(())
}

impl SnapshotManifest {
    /// Checks the manifest's own rules.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version != MANIFEST_VERSION {
            return Err(ManifestError::UnknownVersion(self.version));
        }
        check_tree(&self.files)?;
        check_tree(&self.harness_state.entries)?;
        let mut seen = std::collections::HashSet::with_capacity(self.caches.len());
        if let Some(index) = self.caches.iter().position(|c| !seen.insert(&c.path)) {
            return Err(ManifestError::DuplicatePath { index });
        }
        Ok(())
    }

    /// Returns the canonical bytes of a valid manifest.
    pub fn encode(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        Ok(postcard::to_stdvec(self)?)
    }

    /// Decodes bytes under the size bound and checks the result.
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_LEN {
            return Err(ManifestError::TooLarge { len: bytes.len() });
        }
        let manifest: SnapshotManifest = postcard::from_bytes(bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Returns the snapshot's identity: blake3 of its canonical bytes.
    pub fn id(&self) -> Result<SnapshotId, ManifestError> {
        Ok(SnapshotId::of(&self.encode()?))
    }
}
