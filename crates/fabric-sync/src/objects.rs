//! One side of a copy: the objects and snapshots of a store, here or behind
//! the SSH link, and what a copy counted.

use fabric_core::id::{ChunkId, SnapshotId};
use fabric_core::snapshot::SnapshotManifest;
use fabric_store::objects::{MAX_OBJECT_LEN, ObjectError};
use fabric_store::store::{Store, StoreError};

use crate::SyncError;

/// The most bytes one stored object can take: zstd's worst case for the largest object.
pub(crate) fn max_stored_len() -> usize {
    zstd::zstd_safe::compress_bound(MAX_OBJECT_LEN as usize)
}

/// The longest manifest either side decodes, the store's own bound.
pub(crate) const MAX_MANIFEST_LEN: usize = 64 << 20;

/// The objects and snapshots of one store.
pub(crate) trait Objects {
    /// True when the store holds the snapshot whole.
    fn has_snapshot(&mut self, id: SnapshotId) -> Result<bool, SyncError>;
    /// The byte length of a snapshot's manifest and the objects that hold it, in order.
    fn manifest_chunks(&mut self, id: SnapshotId) -> Result<(u64, Vec<ChunkId>), SyncError>;
    /// The ids among `ids` the store does not hold, in order.
    fn missing(&mut self, ids: &[ChunkId]) -> Result<Vec<ChunkId>, SyncError>;
    /// The stored bytes of a prefix of `ids`, at least one, in order; the
    /// caller asks again for the rest.
    fn get(&mut self, ids: &[ChunkId]) -> Result<Vec<Vec<u8>>, SyncError>;
    /// Stores objects given as stored, each checked against its id.
    fn put(&mut self, chunks: &[(ChunkId, Vec<u8>)]) -> Result<(), SyncError>;
    /// Publishes the manifest whose bytes are `manifest`; every object it names is already in the store.
    fn publish(&mut self, manifest: &[u8]) -> Result<SnapshotId, SyncError>;
}

/// The most stored bytes one `get` answers with.
pub(crate) const MAX_BATCH_BYTES: usize = 32 << 20;

/// A store on this machine.
pub(crate) struct StoreObjects<'a>(pub(crate) &'a mut Store);

impl Objects for StoreObjects<'_> {
    fn has_snapshot(&mut self, id: SnapshotId) -> Result<bool, SyncError> {
        has_snapshot(self.0, id)
    }

    fn manifest_chunks(&mut self, id: SnapshotId) -> Result<(u64, Vec<ChunkId>), SyncError> {
        Ok(self.0.manifest_chunks(id)?)
    }

    fn missing(&mut self, ids: &[ChunkId]) -> Result<Vec<ChunkId>, SyncError> {
        missing(self.0, ids)
    }

    fn get(&mut self, ids: &[ChunkId]) -> Result<Vec<Vec<u8>>, SyncError> {
        get(self.0, ids)
    }

    fn put(&mut self, chunks: &[(ChunkId, Vec<u8>)]) -> Result<(), SyncError> {
        put(self.0, chunks)
    }

    fn publish(&mut self, manifest: &[u8]) -> Result<SnapshotId, SyncError> {
        publish(self.0, manifest)
    }
}

pub(crate) fn has_snapshot(store: &Store, id: SnapshotId) -> Result<bool, SyncError> {
    match store.snapshot(id) {
        Ok(_) => Ok(true),
        Err(StoreError::UnknownSnapshot(_)) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// An object on disk without a catalog row, which a cut copy leaves, counts
/// as held: publish verifies such an object before a row names it.
pub(crate) fn missing(store: &Store, ids: &[ChunkId]) -> Result<Vec<ChunkId>, SyncError> {
    let mut out = Vec::new();
    for &id in ids {
        match store.objects().stored_len(id) {
            Ok(_) => {}
            Err(ObjectError::Missing { .. }) => out.push(id),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

pub(crate) fn get(store: &Store, ids: &[ChunkId]) -> Result<Vec<Vec<u8>>, SyncError> {
    let mut out = Vec::new();
    let mut held = 0usize;
    for &id in ids {
        let bytes = store.objects().read_stored(id)?;
        if !out.is_empty() && held + bytes.len() > MAX_BATCH_BYTES {
            break;
        }
        held += bytes.len();
        out.push(bytes);
    }
    Ok(out)
}

/// Decompresses each object under the largest object's bound and stores it;
/// bytes that do not hash to the id they came under are refused.
pub(crate) fn put(store: &Store, chunks: &[(ChunkId, Vec<u8>)]) -> Result<(), SyncError> {
    for (id, stored) in chunks {
        let bad = |why: &str| SyncError::Protocol {
            reason: format!("object {id} {why}"),
        };
        if stored.len() > max_stored_len() {
            return Err(bad("is longer than any stored object"));
        }
        let bytes = zstd::bulk::decompress(stored, MAX_OBJECT_LEN as usize)
            .map_err(|_| bad("is not one zstd frame of at most the largest object"))?;
        if ChunkId::of(&bytes) != *id {
            return Err(bad("does not hash to its id"));
        }
        store.objects().put(&bytes)?;
    }
    Ok(())
}

pub(crate) fn publish(store: &mut Store, manifest: &[u8]) -> Result<SnapshotId, SyncError> {
    if manifest.len() > MAX_MANIFEST_LEN {
        return Err(SyncError::Protocol {
            reason: format!("a manifest of {} bytes", manifest.len()),
        });
    }
    let id = SnapshotId::of(manifest);
    let decoded = SnapshotManifest::decode(manifest).map_err(|e| SyncError::Protocol {
        reason: format!("manifest {id} does not decode: {e}"),
    })?;
    let published = store.publish(&decoded)?;
    if published != id {
        return Err(SyncError::Protocol {
            reason: format!("manifest {id} published as {published}"),
        });
    }
    Ok(id)
}

/// What one sync moved, for the report and the benchmark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Snapshots the receiving store did not hold and now does: the one synced and the bundle bases it needed.
    pub snapshots: u64,
    /// Distinct objects those snapshots name.
    pub objects_needed: u64,
    /// The objects among them that were sent; the rest the receiver already held.
    pub objects_sent: u64,
    /// Stored bytes sent: the objects and the manifests.
    pub bytes_sent: u64,
}

impl Counters {
    /// The share of needed objects the receiver already held, from 0 to 1;
    /// 1 when nothing was needed.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a ratio for a report; object counts stay far below 2^52"
    )]
    pub fn hit_ratio(&self) -> f64 {
        if self.objects_needed == 0 {
            1.0
        } else {
            1.0 - self.objects_sent as f64 / self.objects_needed as f64
        }
    }
}
