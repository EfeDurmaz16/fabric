//! Fetching a snapshot from a peer into this node's store (ADR 0013, ADR 0014).
//!
//! Invariant: a snapshot enters the catalog only through `Store::publish`,
//! after every object it names is on disk and was checked against its hash
//! on arrival, so a fetch cut anywhere leaves at most objects without a row,
//! never a row without its objects. The manifest's bytes are joined and
//! checked against the snapshot id before they are decoded. A retry asks
//! only for what the store still lacks, so a dropped connection costs the
//! chunks in flight and nothing more.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fabric_core::id::{ChunkId, SnapshotId};
use fabric_core::snapshot::SnapshotManifest;
use fabric_net::rpc::Session;
use fabric_net::wire::{ChunkBatch, MAX_CHUNK_BATCH};
use fabric_store::objects::ObjectError;
use fabric_store::store::{Store, StoreError};

use crate::error::NodeError;
use crate::faults::{self, Point};

/// How long a chunk stream may go without a byte before it is cut.
pub const CHUNK_IDLE: Duration = Duration::from_secs(30);

/// How long a manifest's chunk list may take to arrive.
pub const MANIFEST_DEADLINE: Duration = Duration::from_secs(30);

/// How many times a batch is asked for again after a failure.
const RETRIES: usize = 3;

/// The longest bundle chain followed: ADR 0014's sixteen links and the full bundle under them.
const MAX_CHAIN: usize = 17;

/// Runs `work` on the store off the async runtime.
pub(crate) async fn with_store<T, F>(store: &Arc<Mutex<Store>>, work: F) -> Result<T, NodeError>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> Result<T, NodeError> + Send + 'static,
{
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || {
        let mut guard = store.lock().unwrap_or_else(PoisonError::into_inner);
        work(&mut guard)
    })
    .await
    .map_err(|e| NodeError::Task(e.to_string()))?
}

/// True when the store holds `snapshot` whole: publish wrote its row only after every object.
pub(crate) async fn is_complete(
    store: &Arc<Mutex<Store>>,
    snapshot: SnapshotId,
) -> Result<bool, NodeError> {
    with_store(store, move |store| match store.snapshot(snapshot) {
        Ok(_) => Ok(true),
        Err(StoreError::UnknownSnapshot(_)) => Ok(false),
        Err(e) => Err(e.into()),
    })
    .await
}

/// Fetches `snapshot`, and the bundle bases it needs, from the peer behind `session`.
pub async fn fetch_snapshot(
    store: &Arc<Mutex<Store>>,
    session: &Session,
    snapshot: SnapshotId,
) -> Result<(), NodeError> {
    let fail = |reason: String| NodeError::Fetch {
        snapshot,
        peer: session.peer(),
        reason,
    };
    // Bundle bases are published first: publish refuses a base it lacks.
    let mut pending = vec![snapshot];
    let mut manifests: Vec<(SnapshotId, SnapshotManifest)> = Vec::new();
    while let Some(next) = pending.pop() {
        if is_complete(store, next).await? {
            continue;
        }
        if manifests.len() >= MAX_CHAIN {
            return Err(fail(format!("the bundle chain is longer than {MAX_CHAIN}")));
        }
        let manifest = fetch_manifest(store, session, next).await?;
        if let Some(base) = manifest.git.as_ref().and_then(|git| git.bundle_base) {
            pending.push(base);
        }
        manifests.push((next, manifest));
    }
    for (id, manifest) in manifests.into_iter().rev() {
        fetch_chunks(store, session, Store::data_chunks(&manifest), true).await?;
        let published = with_store(store, move |store| Ok(store.publish(&manifest)?)).await?;
        if published != id {
            return Err(fail(format!("the manifest published as {published}")));
        }
        tracing::info!(snapshot = %id, peer = %session.peer(), "fetched");
    }
    Ok(())
}

async fn fetch_manifest(
    store: &Arc<Mutex<Store>>,
    session: &Session,
    snapshot: SnapshotId,
) -> Result<SnapshotManifest, NodeError> {
    let fail = |reason: String| NodeError::Fetch {
        snapshot,
        peer: session.peer(),
        reason,
    };
    let listed = session
        .want_snapshot(snapshot, MANIFEST_DEADLINE)
        .await
        .map_err(|e| fail(e.to_string()))?;
    let ids = listed.ids().to_vec();
    fetch_chunks(store, session, ids.clone(), false).await?;
    let len = listed.byte_len();
    let bytes = with_store(store, move |store| {
        let mut bytes = Vec::new();
        for id in ids {
            bytes.extend_from_slice(&store.objects().read(id).map_err(StoreError::from)?);
        }
        Ok(bytes)
    })
    .await?;
    if bytes.len() as u64 != len || SnapshotId::of(&bytes) != snapshot {
        return Err(fail(
            "the manifest bytes do not hash to the snapshot id".to_owned(),
        ));
    }
    SnapshotManifest::decode(&bytes).map_err(|e| fail(e.to_string()))
}

/// Fetches every chunk in `ids` the store lacks, in batches, asking again for what a failure left out.
///
/// `data` marks the fetch of a snapshot's data objects, where the
/// `mid_transfer` fault point sits.
async fn fetch_chunks(
    store: &Arc<Mutex<Store>>,
    session: &Session,
    ids: Vec<ChunkId>,
    data: bool,
) -> Result<(), NodeError> {
    let mut missing = with_store(store, move |store| {
        let mut missing = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            if !seen.insert(id) {
                continue;
            }
            match store.objects().stored_len(id) {
                Ok(_) => {}
                Err(ObjectError::Missing { .. }) => missing.push(id),
                Err(e) => return Err(StoreError::from(e).into()),
            }
        }
        Ok(missing)
    })
    .await?;
    let mut first = data;
    while !missing.is_empty() {
        let rest = missing.split_off(missing.len().min(MAX_CHUNK_BATCH));
        let batch = std::mem::replace(&mut missing, rest);
        let mut wanted = batch;
        let mut attempt = 0;
        while !wanted.is_empty() {
            match fetch_batch(store, session, wanted.clone(), &mut first).await {
                Ok(()) => wanted.clear(),
                Err((left, reason)) => {
                    attempt += 1;
                    if attempt > RETRIES {
                        return Err(NodeError::Peer {
                            peer: session.peer(),
                            reason: format!("chunks did not arrive: {reason}"),
                        });
                    }
                    tracing::info!(peer = %session.peer(), left = left.len(), %reason, "asking again for chunks");
                    wanted = left;
                }
            }
        }
    }
    Ok(())
}

/// Fetches one batch; on failure returns the ids that did not arrive.
async fn fetch_batch(
    store: &Arc<Mutex<Store>>,
    session: &Session,
    ids: Vec<ChunkId>,
    first: &mut bool,
) -> Result<(), (Vec<ChunkId>, String)> {
    let batch = ChunkBatch::new(ids.clone()).map_err(|e| (ids.clone(), e.to_string()))?;
    let mut stream = session
        .want_chunks(batch, CHUNK_IDLE)
        .await
        .map_err(|e| (ids.clone(), e.to_string()))?;
    loop {
        match stream.next().await {
            Ok(Some(chunk)) => {
                let id = chunk.id();
                let bytes = chunk.bytes().to_vec();
                let stored = with_store(store, move |store| {
                    Ok(store.objects().put(&bytes).map_err(StoreError::from)?)
                })
                .await;
                let reason = match stored {
                    Ok(stored) if stored.id == id => None,
                    Ok(stored) => Some(format!("chunk {id} stored as {}", stored.id)),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(reason) = reason {
                    // The stream counts the chunk as arrived; it is not stored.
                    let mut left: Vec<ChunkId> = stream.missing().collect();
                    left.push(id);
                    return Err((left, reason));
                }
                if std::mem::take(first) {
                    faults::hit(Point::MidTransfer).await;
                }
            }
            Ok(None) => return Ok(()),
            Err(e) => return Err((stream.missing().collect(), e.to_string())),
        }
    }
}
