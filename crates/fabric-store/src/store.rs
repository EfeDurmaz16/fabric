//! One node's store: object files, the catalog and the lock that joins them.
//!
//! Invariant: the catalog never names an object that is not on disk. Publish
//! writes and flushes every object before it inserts a row, gc deletes a row
//! before it deletes the file, and `check` can prove the rule holds. Exactly
//! one process holds a store open at a time (ADR 0014, One writer).
//!
//! Inside that process, a [`Store`] is a handle, and [`Store::handle`] makes
//! another on the same store, so captures, restores and reads run beside
//! each other instead of one after another. Objects are immutable files
//! under their hash, so writers of the same object agree; the catalog's
//! connection is behind a lock held for one query or one transaction;
//! publishes run one at a time, from their first look at the catalog to
//! their commit, so two that name the same manifest make one row and a
//! shard flush is never in flight while another publish decides it needs
//! none. `forget` and `gc` need the store alone: they refuse while another
//! handle exists, so a capture is never between its objects and its
//! publish while gc runs, and no publish can name a base being forgotten.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use fabric_core::env::{EnvSpec, EnvSpecError};
use fabric_core::id::{ChunkId, EnvSpecId, SnapshotId};
use fabric_core::path::RelPath;
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
    /// An environment spec does not validate, encode or decode.
    #[error(transparent)]
    EnvSpec(#[from] EnvSpecError),
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
    /// A manifest's bundle base has no git state, or one in another object format, so its bundle cannot come first in the chain.
    #[error("bundle base {base} has no git state in the object format of the snapshot")]
    IncompatibleBundleBase {
        /// The base the manifest names.
        base: SnapshotId,
    },
    /// A file entry's objects do not add up to the size it records, so no restore could write it.
    #[error("{path}: the objects hold {held} bytes but the entry records {size}")]
    SizeMismatch {
        /// The entry's path.
        path: RelPath,
        /// The size the entry records.
        size: u64,
        /// The bytes its objects decompress to.
        held: u64,
    },
    /// A later snapshot's bundle still needs the snapshot's refs.
    #[error("snapshot {base} is the bundle base of a later snapshot")]
    BaseInUse {
        /// The snapshot that cannot be forgotten yet.
        base: SnapshotId,
    },
    /// Another handle on the store exists, and the call needs the store alone.
    #[error("the store has other handles open; {call} needs it alone")]
    Shared {
        /// The call that was refused.
        call: &'static str,
    },
}

/// The object that holds a spec's bytes: an `EnvSpecId` and a `ChunkId`
/// are both blake3 of the same bytes, so the ids are equal.
fn spec_object(id: EnvSpecId) -> ChunkId {
    ChunkId::from_bytes(id.to_bytes())
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
    /// Bundle caches of jobs the store holds no snapshot of any more, removed.
    pub removed_caches: u64,
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
    /// Snapshots whose objects are whole but whose manifest does not assemble
    /// to its id and recorded length, does not decode, names other data
    /// objects than the catalog rows, or has another job, parent, bundle
    /// base or creation time than its catalog row; checked at `Hashes` depth only.
    pub bad_manifests: Vec<SnapshotId>,
}

impl CheckReport {
    /// True when every snapshot is complete, every chain is whole and the counts agree.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
            && self.corrupt.is_empty()
            && self.refcount_mismatches.is_empty()
            && self.broken_chains.is_empty()
            && self.bad_manifests.is_empty()
    }
}

/// A handle on a store that this process holds open.
///
/// Handles made with [`Store::handle`] share the store: each may capture,
/// restore, read and publish beside the others.
#[derive(Debug)]
pub struct Store {
    shared: Arc<Shared>,
}

/// What every handle on one store shares.
#[derive(Debug)]
struct Shared {
    /// The store's directory.
    root: PathBuf,
    objects: ObjectStore,
    /// Held for one query or one transaction, never across object IO.
    catalog: Mutex<Catalog>,
    /// Held for a whole publish, so publishes run one at a time.
    publishing: Mutex<()>,
    _lock: File,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a busy store lock is retried before it counts as held.
const LOCK_PATIENCE: Duration = Duration::from_secs(1);

/// Takes the store's lock, or fails with `Locked` when another process holds it.
///
/// A child that another thread of this process forks shares the lock's
/// open file until it execs, so a lock this process just released can look
/// held for that instant; a busy lock is retried for `LOCK_PATIENCE`.
fn take_lock(root: &Path) -> Result<File, StoreError> {
    let path = root.join("lock");
    let file = File::create(&path).map_err(|e| io_err(&path, e))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_PATIENCE => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(StoreError::Locked {
                    path: root.to_owned(),
                });
            }
            Err(fs::TryLockError::Error(e)) => return Err(io_err(&path, e)),
        }
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
        Ok(Store::new(root, objects, catalog, lock))
    }

    fn new(root: &Path, objects: ObjectStore, catalog: Catalog, lock: File) -> Store {
        Store {
            shared: Arc::new(Shared {
                root: root.to_owned(),
                objects,
                catalog: Mutex::new(catalog),
                publishing: Mutex::new(()),
                _lock: lock,
            }),
        }
    }

    /// Another handle on this store, for work beside this one's. The store
    /// stays open until the last handle is dropped.
    #[must_use]
    pub fn handle(&self) -> Store {
        Store {
            shared: Arc::clone(&self.shared),
        }
    }

    fn catalog(&self) -> MutexGuard<'_, Catalog> {
        locked(&self.shared.catalog)
    }

    /// The store's parts when this is its only handle, for a call that needs it alone.
    fn alone(&mut self, call: &'static str) -> Result<&mut Shared, StoreError> {
        Arc::get_mut(&mut self.shared).ok_or(StoreError::Shared { call })
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
        Ok(Store::new(root, objects, catalog, lock))
    }

    /// The store's directory.
    pub(crate) fn root(&self) -> &Path {
        &self.shared.root
    }

    /// Returns the object files, for writing and reading content.
    #[must_use]
    pub fn objects(&self) -> &ObjectStore {
        &self.shared.objects
    }

    /// Every object a manifest reaches, in list order, bundle and exclude
    /// first, and the environment spec it names last.
    ///
    /// A node that fetches a snapshot from a peer asks for exactly these,
    /// then publishes the manifest; publish refuses while one is missing,
    /// and gc keeps them while the snapshot is kept.
    #[must_use]
    pub fn data_chunks(manifest: &SnapshotManifest) -> Vec<ChunkId> {
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
        if let Some(spec) = manifest.env_spec {
            ids.push(spec_object(spec));
        }
        ids
    }

    /// Stores the canonical bytes of `spec` as one object and returns the
    /// spec's id, which a manifest names in `env_spec`.
    pub fn put_env_spec(&self, spec: &EnvSpec) -> Result<EnvSpecId, StoreError> {
        let bytes = spec.encode()?;
        self.objects().put(&bytes)?;
        Ok(EnvSpecId::of(&bytes))
    }

    /// Reads the environment spec stored under `id`.
    pub fn env_spec(&self, id: EnvSpecId) -> Result<EnvSpec, StoreError> {
        let bytes = self.objects().read(spec_object(id))?;
        Ok(EnvSpec::decode(&bytes)?)
    }

    /// Makes a manifest a published snapshot and returns its id.
    ///
    /// Every object the manifest names must already be in the store. The
    /// manifest's own bytes become objects too, the shard directories are
    /// flushed, and only then does the catalog get the rows. Publishing a
    /// manifest the store already has returns its id and changes nothing. A
    /// manifest above `MAX_MANIFEST_LEN` is refused, since it could never be
    /// read back. A bundle base must be in the store and hold git state in
    /// the manifest's object format: restore unpacks the base's bundles into
    /// a repository of that format before this one's.
    ///
    /// Publishes of all handles run one at a time; captures, restores and
    /// reads run beside one.
    pub fn publish(&mut self, manifest: &SnapshotManifest) -> Result<SnapshotId, StoreError> {
        let bytes = encode_manifest(manifest)?;
        let id = SnapshotId::of(&bytes);
        let _publishing = locked(&self.shared.publishing);
        if self.catalog().snapshot(id)?.is_some() {
            return Ok(id);
        }
        if let Some(git) = &manifest.git
            && let Some(base) = git.bundle_base
        {
            if self.catalog().snapshot(base)?.is_none() {
                return Err(StoreError::MissingBundleBase { base });
            }
            let sha256 = git.index_commit.is_sha256();
            let compatible = self
                .manifest(base)?
                .git
                .is_some_and(|b| b.index_commit.is_sha256() == sha256);
            if !compatible {
                return Err(StoreError::IncompatibleBundleBase { base });
            }
        }
        let mut data = Vec::new();
        for chunk in Self::data_chunks(manifest) {
            let stored_len = match self.objects().stored_len(chunk) {
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
        self.check_sizes(manifest)?;
        let stored = self.objects().put_bytes(&bytes)?;
        let mut manifest_refs = Vec::with_capacity(stored.chunks.len());
        for chunk in stored.chunks {
            self.adopt_unpublished(chunk)?;
            manifest_refs.push(ChunkRef {
                id: chunk,
                stored_len: self.objects().stored_len(chunk)?,
            });
        }
        checkpoint::reached("publish.after_objects");
        self.objects().flush_shards()?;
        checkpoint::reached("publish.after_fsync");
        let row = SnapshotRow {
            id,
            job_id: manifest.job_id,
            parent: manifest.parent,
            bundle_base: manifest.git.as_ref().and_then(|g| g.bundle_base),
            created_at: manifest.created_at,
            manifest_len: bytes.len() as u64,
        };
        self.catalog()
            .insert_snapshot(&row, &manifest_refs, &data, || {
                checkpoint::reached("publish.before_commit");
            })?;
        checkpoint::reached("publish.after_commit");
        tracing::info!(snapshot = %id, objects = data.len(), "published");
        Ok(id)
    }

    /// Refuses a file entry whose objects do not decompress to the size it records.
    ///
    /// Restore checks the same sum after it writes the file; a snapshot that
    /// fails it would publish and never restore.
    fn check_sizes(&self, manifest: &SnapshotManifest) -> Result<(), StoreError> {
        let mut lens: HashMap<ChunkId, u64> = HashMap::new();
        for entry in manifest
            .files
            .iter()
            .chain(manifest.harness_state.entries.iter())
        {
            let EntryKind::File { size, chunks } = &entry.kind else {
                continue;
            };
            let mut held = 0u64;
            for chunk in chunks {
                let len = if let Some(len) = lens.get(chunk) {
                    *len
                } else {
                    let len = self.objects().content_len(*chunk)?;
                    lens.insert(*chunk, len);
                    len
                };
                held = held.saturating_add(len);
            }
            if held != *size {
                return Err(StoreError::SizeMismatch {
                    path: entry.rel_path.clone(),
                    size: *size,
                    held,
                });
            }
        }
        Ok(())
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
        if self.objects().is_fresh(chunk) || self.catalog().has_chunk(chunk)? {
            return Ok(());
        }
        match self.objects().read(chunk) {
            Ok(_) => {}
            Err(ObjectError::Corrupt { id }) => {
                self.objects().remove(id)?;
                return Err(StoreError::CorruptUnpublished { chunk: id });
            }
            Err(ObjectError::Missing { id }) => return Err(StoreError::MissingChunk { chunk: id }),
            Err(e) => return Err(e.into()),
        }
        self.objects().mark_dirty(chunk);
        Ok(())
    }

    /// Returns the catalog row of every snapshot, oldest first.
    pub fn snapshots(&self) -> Result<Vec<SnapshotRow>, StoreError> {
        Ok(self.catalog().snapshots()?)
    }

    /// Returns the catalog row of a snapshot.
    pub fn snapshot(&self, id: SnapshotId) -> Result<SnapshotRow, StoreError> {
        self.catalog()
            .snapshot(id)?
            .ok_or(StoreError::UnknownSnapshot(id))
    }

    /// Assembles, verifies and decodes the manifest of a snapshot.
    ///
    /// Assembly stops at the recorded length: a catalog that lists more
    /// manifest objects than that length holds is a mismatch, found before
    /// the extra objects are read, so a damaged catalog never makes this
    /// hold more than the recorded length and one object.
    pub fn manifest(&self, id: SnapshotId) -> Result<SnapshotManifest, StoreError> {
        let row = self.snapshot(id)?;
        let len = usize::try_from(row.manifest_len).unwrap_or(usize::MAX);
        if len > MAX_MANIFEST_LEN {
            return Err(ManifestError::TooLarge { len }.into());
        }
        let mut bytes = Vec::with_capacity(len);
        let chunks = self.catalog().snapshot_chunks(id, Role::Manifest)?;
        for chunk in chunks {
            if bytes.len() >= len {
                return Err(StoreError::ManifestMismatch(id));
            }
            let part = self.objects().read(chunk.id)?;
            if part.len() > len - bytes.len() {
                return Err(StoreError::ManifestMismatch(id));
            }
            bytes.extend_from_slice(&part);
        }
        if bytes.len() as u64 != row.manifest_len || SnapshotId::of(&bytes) != id {
            return Err(StoreError::ManifestMismatch(id));
        }
        Ok(SnapshotManifest::decode(&bytes)?)
    }

    /// Returns the byte length of a snapshot's manifest and the objects that hold it, in order.
    ///
    /// A peer fetches these objects, joins them and checks that the bytes
    /// hash to the snapshot id before it decodes anything (ADR 0013).
    pub fn manifest_chunks(&self, id: SnapshotId) -> Result<(u64, Vec<ChunkId>), StoreError> {
        let row = self.snapshot(id)?;
        let ids = self
            .catalog()
            .snapshot_chunks(id, Role::Manifest)?
            .into_iter()
            .map(|chunk| chunk.id)
            .collect();
        Ok((row.manifest_len, ids))
    }

    /// Drops a snapshot from the catalog; its objects wait for gc.
    ///
    /// A snapshot whose refs a later bundle excludes is refused: every
    /// restore of that later snapshot would need this one's bundle. Refused
    /// as `Shared` while another handle exists, since a publish there
    /// could name the snapshot as its base.
    pub fn forget(&mut self, id: SnapshotId) -> Result<(), StoreError> {
        let catalog = self
            .alone("forget")?
            .catalog
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if catalog.is_bundle_base(id)? {
            return Err(StoreError::BaseInUse { base: id });
        }
        if catalog.forget_snapshot(id)? {
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
    /// It is refused as `Shared` while another handle exists, so no
    /// capture on another handle is in that window and no staged file of
    /// one is cleared under it.
    pub fn gc(&mut self, now: SystemTime, grace: Duration) -> Result<GcReport, StoreError> {
        let Shared {
            root,
            objects,
            catalog,
            ..
        } = self.alone("gc")?;
        let catalog = catalog.get_mut().unwrap_or_else(PoisonError::into_inner);
        let mut report = GcReport {
            removed_tmp: objects.clear_tmp()?,
            ..GcReport::default()
        };
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX));
        let grace_ms = grace.as_millis().try_into().unwrap_or(u64::MAX);
        report.marked = catalog.mark_unreferenced(now_ms)?;
        let expired = catalog.expired(now_ms, grace_ms)?;
        let ids: Vec<ChunkId> = expired.iter().map(|c| c.id).collect();
        catalog.delete_unreferenced(&ids)?;
        for chunk in &expired {
            objects.remove(chunk.id)?;
            report.removed_chunks += 1;
            report.removed_bytes += chunk.stored_len;
        }
        let known = catalog.chunk_ids()?;
        for (id, mtime) in objects.scan()? {
            let old = now.duration_since(mtime).is_ok_and(|age| age >= grace);
            if !known.contains(&id) && old {
                objects.remove(id)?;
                report.removed_orphans += 1;
            }
        }
        // A job with no snapshot left has no chain a restore could take
        // from its bundle cache; its packs would wait for the cache's own
        // eviction (bundle_cache::MAX_IDLE) otherwise.
        let jobs = catalog
            .snapshots()?
            .into_iter()
            .map(|row| row.job_id)
            .collect();
        report.removed_caches = crate::bundle_cache::remove_all_but(root, &jobs)
            .map_err(|e| io_err(&root.join("bundle-cache"), e))?;
        tracing::info!(?report, "gc done");
        Ok(report)
    }

    /// Checks that every snapshot's objects are on disk, with `Hashes` that they verify, and that chains and counts agree.
    pub fn check(&self, depth: CheckDepth) -> Result<CheckReport, StoreError> {
        // One guard per statement: a struct literal keeps its temporaries,
        // and the catalog's lock is not reentrant.
        let refcount_mismatches = self.catalog().refcount_mismatches()?;
        let broken_chains = self.catalog().broken_chains()?;
        let mut report = CheckReport {
            refcount_mismatches,
            broken_chains,
            ..CheckReport::default()
        };
        let mut seen_ok: HashSet<ChunkId> = HashSet::new();
        let rows = self.catalog().snapshots()?;
        for row in rows {
            report.snapshots += 1;
            let damaged = report.missing.len() + report.corrupt.len();
            for role in [Role::Manifest, Role::Data] {
                let chunks = self.catalog().snapshot_chunks(row.id, role)?;
                for chunk in chunks {
                    if seen_ok.contains(&chunk.id) {
                        continue;
                    }
                    let outcome = match depth {
                        CheckDepth::Presence => self.objects().stored_len(chunk.id).map(|_| ()),
                        CheckDepth::Hashes => self.objects().read(chunk.id).map(|_| ()),
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
            let whole = report.missing.len() + report.corrupt.len() == damaged;
            if depth == CheckDepth::Hashes && whole && !self.manifest_agrees(&row)? {
                report.bad_manifests.push(row.id);
            }
        }
        Ok(report)
    }

    /// Says whether a snapshot's manifest assembles, decodes, names the data rows the catalog holds and matches its row.
    ///
    /// `forget` and gc trust the row's bundle base; a row that lost it
    /// would let the base of a live chain be forgotten.
    fn manifest_agrees(&self, row: &SnapshotRow) -> Result<bool, StoreError> {
        let id = row.id;
        let manifest = match self.manifest(id) {
            Ok(manifest) => manifest,
            Err(StoreError::ManifestMismatch(_) | StoreError::Manifest(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        let rows: Vec<ChunkId> = self
            .catalog()
            .snapshot_chunks(id, Role::Data)?
            .iter()
            .map(|chunk| chunk.id)
            .collect();
        let same_row = row.job_id == manifest.job_id
            && row.parent == manifest.parent
            && row.bundle_base == manifest.git.as_ref().and_then(|g| g.bundle_base)
            && row.created_at == manifest.created_at;
        Ok(same_row && rows == Self::data_chunks(&manifest))
    }
}
