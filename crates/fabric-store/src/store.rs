//! One node's store: object files, the catalog and the lock that joins them.
//!
//! Invariant: the catalog never names an object that is not on disk. Publish
//! writes and flushes every object before it inserts a row, gc deletes a row
//! before it deletes the file, and `check` can prove the rule holds. Exactly
//! one process holds a store open at a time (ADR 0004, One writer per data
//! directory).

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fabric_core::id::{ChunkId, SnapshotId};
use fabric_core::snapshot::{EntryKind, MAX_MANIFEST_LEN, ManifestError, SnapshotManifest};

use crate::catalog::{Catalog, CatalogError, ChunkRef, Role, SnapshotRow};
use crate::checkpoint;
use crate::objects::{ChunkParams, ObjectError, ObjectStore};

/// Why the store refused.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the call was made on.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// Another process holds the store open.
    #[error("store at {path} is held by another process")]
    Locked {
        /// The store root.
        path: PathBuf,
    },
    /// The directory has no catalog; it was never created as a store.
    #[error("{path} is not a store")]
    NotAStore {
        /// The directory that was opened.
        path: PathBuf,
    },
    /// An object could not be written or read.
    #[error(transparent)]
    Object(#[from] ObjectError),
    /// The catalog refused.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The manifest does not validate or encode.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// A manifest names an object that is not in the store.
    #[error("manifest names object {chunk}, which is not in the store")]
    MissingChunk {
        /// The object the manifest names.
        chunk: ChunkId,
    },
    /// An object found on disk without a catalog row was corrupt and has been removed; capture again.
    #[error("object {chunk} had no catalog row and was corrupt; it was removed, capture again")]
    CorruptUnpublished {
        /// The object that was removed.
        chunk: ChunkId,
    },
    /// No snapshot with the id is in the catalog.
    #[error("snapshot {0} is not in the store")]
    UnknownSnapshot(SnapshotId),
    /// The assembled manifest bytes do not hash to the snapshot id.
    #[error("manifest bytes of snapshot {0} do not hash to its id")]
    ManifestMismatch(SnapshotId),
    /// The directory already holds a catalog.
    #[error("{path} is already a store")]
    AlreadyAStore {
        /// The store root.
        path: PathBuf,
    },
    /// A manifest's bundle excludes the refs of a snapshot the store does not have.
    #[error("bundle base {base} is not in the store")]
    MissingBundleBase {
        /// The base the manifest names.
        base: SnapshotId,
    },
    /// A later snapshot's bundle still needs the snapshot's refs.
    #[error("snapshot {base} is the bundle base of a later snapshot")]
    BaseInUse {
        /// The snapshot that cannot be forgotten yet.
        base: SnapshotId,
    },
}

fn io_err(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Encodes a manifest, refusing one longer than `MAX_MANIFEST_LEN`, which no reader decodes.
pub(crate) fn encode_manifest(manifest: &SnapshotManifest) -> Result<Vec<u8>, ManifestError> {
    let bytes = manifest.encode()?;
    if bytes.len() > MAX_MANIFEST_LEN {
        return Err(ManifestError::TooLarge { len: bytes.len() });
    }
    Ok(bytes)
}

/// What one gc pass removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Chunks that lost their last reference and were stamped in this pass.
    pub marked: u64,
    /// Catalog chunks removed because their grace had run out.
    pub removed_chunks: u64,
    /// Bytes on disk those chunks held.
    pub removed_bytes: u64,
    /// Object files with no catalog row, older than the grace, removed.
    pub removed_orphans: u64,
    /// Files under `tmp/` removed.
    pub removed_tmp: u64,
}

/// How deep `check` looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckDepth {
    /// Every referenced object file exists.
    Presence,
    /// Every referenced object decompresses to bytes that hash to its id.
    Hashes,
}

/// What `check` found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// Snapshots in the catalog.
    pub snapshots: u64,
    /// Objects a snapshot names that are not on disk.
    pub missing: Vec<(SnapshotId, ChunkId)>,
    /// Objects a snapshot names whose file does not hash to its name.
    pub corrupt: Vec<(SnapshotId, ChunkId)>,
    /// Chunks whose refcount disagrees with the rows that name them.
    pub refcount_mismatches: Vec<ChunkId>,
    /// Snapshots whose bundle base is not in the catalog, with that base.
    pub broken_chains: Vec<(SnapshotId, SnapshotId)>,
}

impl CheckReport {
    /// True when every snapshot is complete, every chain is whole and the counts agree.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
            && self.corrupt.is_empty()
            && self.refcount_mismatches.is_empty()
            && self.broken_chains.is_empty()
    }
}

/// A store that this process holds open.
#[derive(Debug)]
pub struct Store {
    objects: ObjectStore,
    catalog: Catalog,
    _lock: File,
}

fn take_lock(root: &Path) -> Result<File, StoreError> {
    let path = root.join("lock");
    let file = File::create(&path).map_err(|e| io_err(&path, e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(StoreError::Locked {
            path: root.to_owned(),
        }),
        Err(fs::TryLockError::Error(e)) => Err(io_err(&path, e)),
    }
}

impl Store {
    /// Creates a store in `root`, which must not already be one.
    pub fn create(root: &Path, params: ChunkParams) -> Result<Store, StoreError> {
        fs::create_dir_all(root).map_err(|e| io_err(root, e))?;
        let lock = take_lock(root)?;
        let catalog_path = root.join("catalog.sqlite");
        if catalog_path.exists() {
            return Err(StoreError::AlreadyAStore {
                path: root.to_owned(),
            });
        }
        let objects = ObjectStore::new(root, params);
        objects.create_dirs()?;
        let catalog = Catalog::create(&catalog_path, &params)?;
        Ok(Store {
            objects,
            catalog,
            _lock: lock,
        })
    }

    /// Opens an existing store and clears its staging directory.
    pub fn open(root: &Path) -> Result<Store, StoreError> {
        let catalog_path = root.join("catalog.sqlite");
        if !catalog_path.is_file() {
            return Err(StoreError::NotAStore {
                path: root.to_owned(),
            });
        }
        let lock = take_lock(root)?;
        let catalog = Catalog::open(&catalog_path)?;
        let objects = ObjectStore::new(root, catalog.params()?);
        objects.create_dirs()?;
        let removed = objects.clear_tmp()?;
        if removed > 0 {
            tracing::info!(removed, "cleared staged files left by an earlier process");
        }
        Ok(Store {
            objects,
            catalog,
            _lock: lock,
        })
    }

    /// Returns the object files, for writing and reading content.
    #[must_use]
    pub fn objects(&self) -> &ObjectStore {
        &self.objects
    }

    /// Every object a manifest reaches, in list order, bundle and exclude first.
    fn data_chunks(manifest: &SnapshotManifest) -> Vec<ChunkId> {
        let mut ids = Vec::new();
        if let Some(git) = &manifest.git {
            ids.extend(git.bundle.iter().copied());
            if let Some(exclude) = &git.exclude {
                ids.extend(exclude.iter().copied());
            }
        }
        for entry in manifest
            .files
            .iter()
            .chain(manifest.harness_state.entries.iter())
        {
            if let EntryKind::File { chunks, .. } = &entry.kind {
                ids.extend(chunks.iter().copied());
            }
        }
        ids
    }

    /// Makes a manifest a published snapshot and returns its id.
    ///
    /// Every object the manifest names must already be in the store. The
    /// manifest's own bytes become objects too, the shard directories are
    /// flushed, and only then does the catalog get the rows. Publishing a
    /// manifest the store already has returns its id and changes nothing. A
    /// manifest above `MAX_MANIFEST_LEN` is refused, since it could never be
    /// read back.
    pub fn publish(&mut self, manifest: &SnapshotManifest) -> Result<SnapshotId, StoreError> {
        let bytes = encode_manifest(manifest)?;
        let id = SnapshotId::of(&bytes);
        if self.catalog.snapshot(id)?.is_some() {
            return Ok(id);
        }
        if let Some(base) = manifest.git.as_ref().and_then(|g| g.bundle_base)
            && self.catalog.snapshot(base)?.is_none()
        {
            return Err(StoreError::MissingBundleBase { base });
        }
        let mut data = Vec::new();
        for chunk in Self::data_chunks(manifest) {
            let stored_len = match self.objects.stored_len(chunk) {
                Ok(len) => len,
                Err(ObjectError::Missing { .. }) => {
                    return Err(StoreError::MissingChunk { chunk });
                }
                Err(e) => return Err(e.into()),
            };
            self.adopt_unpublished(chunk)?;
            data.push(ChunkRef {
                id: chunk,
                stored_len,
            });
        }
        let stored = self.objects.put_bytes(&bytes)?;
        let mut manifest_refs = Vec::with_capacity(stored.chunks.len());
        for chunk in stored.chunks {
            self.adopt_unpublished(chunk)?;
            manifest_refs.push(ChunkRef {
                id: chunk,
                stored_len: self.objects.stored_len(chunk)?,
            });
        }
        checkpoint::reached("publish.after_objects");
        self.objects.flush_shards()?;
        checkpoint::reached("publish.after_fsync");
        let row = SnapshotRow {
            id,
            job_id: manifest.job_id,
            parent: manifest.parent,
            bundle_base: manifest.git.as_ref().and_then(|g| g.bundle_base),
            created_at: manifest.created_at,
            manifest_len: bytes.len() as u64,
        };
        self.catalog
            .insert_snapshot(&row, &manifest_refs, &data, || {
                checkpoint::reached("publish.before_commit");
            })?;
        checkpoint::reached("publish.after_commit");
        tracing::info!(snapshot = %id, objects = data.len(), "published");
        Ok(id)
    }

    /// Makes an object on disk safe for a catalog row to name.
    ///
    /// An object with a row, or one this process linked since the last
    /// flush, needs nothing. Any other was linked by a process that may
    /// have died before its flush, and after a power loss its name can be
    /// durable while its bytes are not: its bytes are verified, a corrupt
    /// file is removed so the next capture writes it again, and its shard is
    /// marked for the flush that comes before the row.
    fn adopt_unpublished(&self, chunk: ChunkId) -> Result<(), StoreError> {
        if self.objects.is_fresh(chunk) || self.catalog.has_chunk(chunk)? {
            return Ok(());
        }
        match self.objects.read(chunk) {
            Ok(_) => {}
            Err(ObjectError::Corrupt { id }) => {
                self.objects.remove(id)?;
                return Err(StoreError::CorruptUnpublished { chunk: id });
            }
            Err(ObjectError::Missing { id }) => return Err(StoreError::MissingChunk { chunk: id }),
            Err(e) => return Err(e.into()),
        }
        self.objects.mark_dirty(chunk);
        Ok(())
    }

    /// Returns the catalog row of every snapshot, oldest first.
    pub fn snapshots(&self) -> Result<Vec<SnapshotRow>, StoreError> {
        Ok(self.catalog.snapshots()?)
    }

    /// Returns the catalog row of a snapshot.
    pub fn snapshot(&self, id: SnapshotId) -> Result<SnapshotRow, StoreError> {
        self.catalog
            .snapshot(id)?
            .ok_or(StoreError::UnknownSnapshot(id))
    }

    /// Assembles, verifies and decodes the manifest of a snapshot.
    pub fn manifest(&self, id: SnapshotId) -> Result<SnapshotManifest, StoreError> {
        let row = self.snapshot(id)?;
        let len = usize::try_from(row.manifest_len).unwrap_or(usize::MAX);
        if len > MAX_MANIFEST_LEN {
            return Err(ManifestError::TooLarge { len }.into());
        }
        let mut bytes = Vec::with_capacity(len);
        for chunk in self.catalog.snapshot_chunks(id, Role::Manifest)? {
            bytes.extend_from_slice(&self.objects.read(chunk.id)?);
        }
        if bytes.len() as u64 != row.manifest_len || SnapshotId::of(&bytes) != id {
            return Err(StoreError::ManifestMismatch(id));
        }
        Ok(SnapshotManifest::decode(&bytes)?)
    }

    /// Drops a snapshot from the catalog; its objects wait for gc.
    ///
    /// A snapshot whose refs a later bundle excludes is refused: every
    /// restore of that later snapshot would need this one's bundle.
    pub fn forget(&mut self, id: SnapshotId) -> Result<(), StoreError> {
        if self.catalog.is_bundle_base(id)? {
            return Err(StoreError::BaseInUse { base: id });
        }
        if self.catalog.forget_snapshot(id)? {
            Ok(())
        } else {
            Err(StoreError::UnknownSnapshot(id))
        }
    }

    /// Removes what no snapshot has referenced for at least `grace`.
    ///
    /// A chunk is stamped when its last reference goes and deleted on a
    /// later pass once the stamp is `grace` old; its row goes before its
    /// file. An object file with no row at all, left by a publish that did
    /// not reach the catalog, is removed once its mtime is `grace` old.
    ///
    /// gc must not run between a capture and the publish of its manifest:
    /// capture can reuse an object that has no row without refreshing its
    /// mtime, so gc may remove it, and publish then fails with `MissingChunk`.
    pub fn gc(&mut self, now: SystemTime, grace: Duration) -> Result<GcReport, StoreError> {
        let mut report = GcReport {
            removed_tmp: self.objects.clear_tmp()?,
            ..GcReport::default()
        };
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX));
        let grace_ms = grace.as_millis().try_into().unwrap_or(u64::MAX);
        report.marked = self.catalog.mark_unreferenced(now_ms)?;
        let expired = self.catalog.expired(now_ms, grace_ms)?;
        let ids: Vec<ChunkId> = expired.iter().map(|c| c.id).collect();
        self.catalog.delete_unreferenced(&ids)?;
        for chunk in &expired {
            self.objects.remove(chunk.id)?;
            report.removed_chunks += 1;
            report.removed_bytes += chunk.stored_len;
        }
        let known = self.catalog.chunk_ids()?;
        for (id, mtime) in self.objects.scan()? {
            let old = now.duration_since(mtime).is_ok_and(|age| age >= grace);
            if !known.contains(&id) && old {
                self.objects.remove(id)?;
                report.removed_orphans += 1;
            }
        }
        tracing::info!(?report, "gc done");
        Ok(report)
    }

    /// Checks that every snapshot's objects are on disk, with `Hashes` that they verify, and that chains and counts agree.
    pub fn check(&self, depth: CheckDepth) -> Result<CheckReport, StoreError> {
        let mut report = CheckReport {
            refcount_mismatches: self.catalog.refcount_mismatches()?,
            broken_chains: self.catalog.broken_chains()?,
            ..CheckReport::default()
        };
        let mut seen_ok: HashSet<ChunkId> = HashSet::new();
        for row in self.catalog.snapshots()? {
            report.snapshots += 1;
            for role in [Role::Manifest, Role::Data] {
                for chunk in self.catalog.snapshot_chunks(row.id, role)? {
                    if seen_ok.contains(&chunk.id) {
                        continue;
                    }
                    let outcome = match depth {
                        CheckDepth::Presence => self.objects.stored_len(chunk.id).map(|_| ()),
                        CheckDepth::Hashes => self.objects.read(chunk.id).map(|_| ()),
                    };
                    match outcome {
                        Ok(()) => {
                            seen_ok.insert(chunk.id);
                        }
                        Err(ObjectError::Missing { id }) => report.missing.push((row.id, id)),
                        Err(ObjectError::Corrupt { id }) => report.corrupt.push((row.id, id)),
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }
        Ok(report)
    }
}
