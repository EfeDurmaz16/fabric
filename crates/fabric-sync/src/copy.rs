//! Copies a snapshot, and the bundle bases it needs, from one store to
//! another, sending only the objects the receiver lacks.
//!
//! Mirrors the node's fetch (fabricd's transfer.rs): manifests are joined
//! and checked against their snapshot id before they are decoded, bases are
//! published first because publish refuses a base it lacks, and a snapshot
//! is published only after all its objects arrived.

use std::collections::HashSet;

use fabric_core::id::{ChunkId, SnapshotId};
use fabric_core::snapshot::SnapshotManifest;
use fabric_store::store::Store;

use crate::SyncError;
use crate::faults;
use crate::objects::{Counters, MAX_MANIFEST_LEN, Objects};

/// The longest bundle chain followed: sixteen incremental links and the full bundle under them.
const MAX_CHAIN: usize = 17;

/// How many ids one `missing` question carries.
const MISSING_BATCH: usize = 1024;

/// Copies `snapshot` from `from` into `to`.
pub(crate) fn copy_snapshot(
    from: &mut dyn Objects,
    to: &mut dyn Objects,
    snapshot: SnapshotId,
    counters: &mut Counters,
) -> Result<(), SyncError> {
    let mut pending = vec![snapshot];
    let mut manifests: Vec<(SnapshotId, Vec<u8>, SnapshotManifest)> = Vec::new();
    while let Some(next) = pending.pop() {
        if to.has_snapshot(next)? {
            continue;
        }
        if manifests.len() >= MAX_CHAIN {
            return Err(SyncError::Protocol {
                reason: format!("the bundle chain of {snapshot} is longer than {MAX_CHAIN}"),
            });
        }
        let (bytes, manifest) = fetch_manifest(from, next)?;
        if let Some(base) = manifest.git.as_ref().and_then(|git| git.bundle_base) {
            pending.push(base);
        }
        manifests.push((next, bytes, manifest));
    }
    let mut seen: HashSet<ChunkId> = HashSet::new();
    for (id, bytes, manifest) in manifests.into_iter().rev() {
        let needed: Vec<ChunkId> = Store::data_chunks(&manifest)
            .into_iter()
            .filter(|chunk| seen.insert(*chunk))
            .collect();
        counters.objects_needed += needed.len() as u64;
        for ids in needed.chunks(MISSING_BATCH) {
            let missing = to.missing(ids)?;
            send(from, to, &missing, counters)?;
        }
        let published = to.publish(&bytes)?;
        if published != id {
            return Err(SyncError::Protocol {
                reason: format!("manifest {id} published as {published}"),
            });
        }
        counters.bytes_sent += bytes.len() as u64;
        counters.snapshots += 1;
    }
    Ok(())
}

/// Reads a manifest's objects from `from`, joined only while they stay
/// within the length it declared, and checks the bytes hash to the id.
fn fetch_manifest(
    from: &mut dyn Objects,
    id: SnapshotId,
) -> Result<(Vec<u8>, SnapshotManifest), SyncError> {
    let bad = |reason: String| SyncError::Protocol { reason };
    let (len, ids) = from.manifest_chunks(id)?;
    let declared = usize::try_from(len).unwrap_or(usize::MAX);
    if declared > MAX_MANIFEST_LEN {
        return Err(bad(format!("manifest {id} declares {len} bytes")));
    }
    let mut bytes = Vec::with_capacity(declared);
    let mut at = 0;
    while at < ids.len() {
        let got = from.get(&ids[at..])?;
        if got.is_empty() {
            return Err(bad(format!("no objects for manifest {id}")));
        }
        for stored in got {
            let part = decompress(ids[at], &stored)?;
            if bytes.len() + part.len() > declared {
                return Err(bad(format!("manifest {id} holds more than {len} bytes")));
            }
            bytes.extend_from_slice(&part);
            at += 1;
        }
    }
    if bytes.len() != declared || SnapshotId::of(&bytes) != id {
        return Err(bad(format!("manifest {id} does not hash to its id")));
    }
    let manifest = SnapshotManifest::decode(&bytes)
        .map_err(|e| bad(format!("manifest {id} does not decode: {e}")))?;
    Ok((bytes, manifest))
}

fn decompress(id: ChunkId, stored: &[u8]) -> Result<Vec<u8>, SyncError> {
    let bad = |why: &str| SyncError::Protocol {
        reason: format!("object {id} {why}"),
    };
    if stored.len() > crate::objects::max_stored_len() {
        return Err(bad("is longer than any stored object"));
    }
    let bytes = zstd::bulk::decompress(stored, fabric_store::objects::MAX_OBJECT_LEN as usize)
        .map_err(|_| bad("is not one zstd frame"))?;
    if ChunkId::of(&bytes) != id {
        return Err(bad("does not hash to its id"));
    }
    Ok(bytes)
}

/// Sends the objects `ids` from `from` to `to`, in batches `get` bounds.
fn send(
    from: &mut dyn Objects,
    to: &mut dyn Objects,
    ids: &[ChunkId],
    counters: &mut Counters,
) -> Result<(), SyncError> {
    let mut at = 0;
    while at < ids.len() {
        let got = from.get(&ids[at..])?;
        if got.is_empty() || got.len() > ids.len() - at {
            return Err(SyncError::Protocol {
                reason: format!("asked for {} objects, got {}", ids.len() - at, got.len()),
            });
        }
        let batch: Vec<(ChunkId, Vec<u8>)> = ids[at..].iter().copied().zip(got).collect();
        let bytes: u64 = batch.iter().map(|(_, b)| b.len() as u64).sum();
        to.put(&batch)?;
        at += batch.len();
        counters.objects_sent += batch.len() as u64;
        counters.bytes_sent += bytes;
        faults::hit("copy.mid_objects");
    }
    Ok(())
}
