//! Fetching a snapshot from a peer into this node's store (ADR 0013, ADR 0014).
//!
//! Invariant: a snapshot enters the catalog only through `Store::publish`,
//! after every object it names is on disk and was checked against its hash
//! on arrival, so a fetch cut anywhere leaves at most objects without a row,
//! never a row without its objects. The manifest's bytes are joined and
//! checked against the snapshot id before they are decoded, and the chunks
//! a peer lists for a manifest are stored only while what they hold stays
//! within the length it declared. A retry asks only for what the store
//! still lacks, so a dropped connection costs the chunks in flight and
//! nothing more.

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
        // The manifest hashed to its id, so its chunk ids are its own.
        fetch_chunks(store, session, Store::data_chunks(&manifest), true, None).await?;
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
    let len = listed.byte_len();
    // The ids are the peer's word until the joined bytes hash to the
    // snapshot id, so what they hold is bounded by the declared length.
    fetch_chunks(store, session, ids.clone(), false, Some(len)).await?;
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
/// `mid_transfer` fault point sits. With `limit`, the fetch stops before
/// it stores a chunk that takes the bytes fetched past it.
async fn fetch_chunks(
    store: &Arc<Mutex<Store>>,
    session: &Session,
    ids: Vec<ChunkId>,
    data: bool,
    limit: Option<u64>,
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
    let mut room = limit;
    while !missing.is_empty() {
        let rest = missing.split_off(missing.len().min(MAX_CHUNK_BATCH));
        let batch = std::mem::replace(&mut missing, rest);
        let mut wanted = batch;
        let mut attempt = 0;
        while !wanted.is_empty() {
            match fetch_batch(store, session, wanted.clone(), &mut first, &mut room).await {
                Ok(()) => wanted.clear(),
                Err(BatchError::PastLimit) => {
                    return Err(NodeError::Peer {
                        peer: session.peer(),
                        reason: format!(
                            "its chunks hold more than the {} bytes it declared",
                            limit.unwrap_or_default()
                        ),
                    });
                }
                Err(BatchError::Retry(left, reason)) => {
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

/// Why a batch stopped.
enum BatchError {
    /// The ids that did not arrive, and why; they are asked for again.
    Retry(Vec<ChunkId>, String),
    /// A chunk would take the bytes fetched past the limit; nothing is asked again.
    PastLimit,
}

/// Fetches one batch, storing no chunk that does not fit in `room`, which
/// each stored chunk shrinks.
async fn fetch_batch(
    store: &Arc<Mutex<Store>>,
    session: &Session,
    ids: Vec<ChunkId>,
    first: &mut bool,
    room: &mut Option<u64>,
) -> Result<(), BatchError> {
    let retry = |ids: &[ChunkId], reason: String| BatchError::Retry(ids.to_vec(), reason);
    let batch = ChunkBatch::new(ids.clone()).map_err(|e| retry(&ids, e.to_string()))?;
    let mut stream = session
        .want_chunks(batch, CHUNK_IDLE)
        .await
        .map_err(|e| retry(&ids, e.to_string()))?;
    loop {
        match stream.next().await {
            Ok(Some(chunk)) => {
                let id = chunk.id();
                let len = chunk.bytes().len() as u64;
                if room.is_some_and(|room| len > room) {
                    return Err(BatchError::PastLimit);
                }
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
                    return Err(BatchError::Retry(left, reason));
                }
                if let Some(room) = room.as_mut() {
                    *room -= len;
                }
                if std::mem::take(first) {
                    faults::hit(Point::MidTransfer).await;
                }
            }
            Ok(None) => return Ok(()),
            Err(e) => return Err(BatchError::Retry(stream.missing().collect(), e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use fabric_core::BoxFuture;
    use fabric_core::id::{MoveId, NodeId, RecordHash};
    use fabric_core::lease::FencingToken;
    use fabric_core::moves::MoveOffer;
    use fabric_net::identity::NodeIdentity;
    use fabric_net::rpc::{JobFuture, ServeLimits, Service};
    use fabric_net::tcp::TcpTlsTransport;
    use fabric_net::tls::Trust;
    use fabric_net::transport::{Connection, Transport};
    use fabric_net::wire::{
        Appended, Facts, Fault, FaultText, ManifestChunks, MoveRequested, MoveState, Offered,
        Pulled, RecordBytes, RecordPage, VersionText,
    };
    use fabric_store::objects::ChunkParams;

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(5);

    #[derive(Debug)]
    struct Anyone;

    impl Trust for Anyone {
        fn is_paired(&self, _: NodeId) -> bool {
            true
        }
    }

    /// A peer whose manifest says it is 16 bytes long and lists chunks of
    /// 1 MiB each.
    struct Lying {
        store: Mutex<Store>,
        ids: Vec<ChunkId>,
    }

    fn not_here<T>() -> JobFuture<'static, T> {
        Box::pin(async { Err(Fault::Internal(FaultText::truncated("not in this test"))) })
    }

    impl Service for Lying {
        fn pull(&self, _: NodeId, _: FencingToken, _: Option<RecordHash>) -> JobFuture<'_, Pulled> {
            not_here()
        }

        fn append(&self, _: NodeId, _: FencingToken, _: RecordPage) -> JobFuture<'_, Appended> {
            not_here()
        }

        fn move_request(&self, _: NodeId, _: FencingToken) -> JobFuture<'_, MoveRequested> {
            not_here()
        }

        fn offer(&self, _: NodeId, _: FencingToken, _: MoveOffer) -> JobFuture<'_, Offered> {
            not_here()
        }

        fn move_status(&self, _: NodeId, _: FencingToken, _: MoveId) -> JobFuture<'_, MoveState> {
            not_here()
        }

        fn move_abort(&self, _: NodeId, _: FencingToken, _: MoveId) -> JobFuture<'_, RecordBytes> {
            not_here()
        }

        fn manifest(
            &self,
            _: NodeId,
            _: SnapshotId,
        ) -> BoxFuture<'_, Result<ManifestChunks, Fault>> {
            let listed = ManifestChunks::new(16, self.ids.clone())
                .map_err(|e| Fault::Internal(FaultText::truncated(&e.to_string())));
            Box::pin(async move { listed })
        }

        fn chunk(&self, _: NodeId, id: ChunkId) -> BoxFuture<'_, Result<Vec<u8>, Fault>> {
            let stored = self
                .store
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .objects()
                .read_stored(id)
                .map_err(|e| Fault::Internal(FaultText::truncated(&e.to_string())));
            Box::pin(async move { stored })
        }

        fn facts(&self, _: NodeId) -> BoxFuture<'_, Result<Facts, Fault>> {
            Box::pin(async { Err(Fault::Internal(FaultText::truncated("not in this test"))) })
        }
    }

    /// A paired peer that declares a manifest of 16 bytes and lists
    /// chunks of 1 MiB each must not get them stored: the fetch stops at
    /// the first byte past the declared length, before the store and the
    /// memory that joins the manifest fill up.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_manifest_whose_chunks_outgrow_its_length_is_cut_off() {
        let dir = tempfile::tempdir().unwrap();
        let source = Store::create(&dir.path().join("source"), ChunkParams::DEFAULT).unwrap();
        let ids: Vec<ChunkId> = (0..3_u8)
            .map(|n| source.objects().put(&vec![n; 1 << 20]).unwrap().id)
            .collect();
        let here = Arc::new(Mutex::new(
            Store::create(&dir.path().join("here"), ChunkParams::DEFAULT).unwrap(),
        ));
        let (liar, asker) = (
            NodeIdentity::generate().unwrap(),
            NodeIdentity::generate().unwrap(),
        );
        let (liar_id, asker_id) = (liar.node_id(), asker.node_id());
        let loopback: SocketAddr = ([127, 0, 0, 1], 0).into();
        let serving: Arc<dyn Transport> =
            Arc::new(TcpTlsTransport::bind(&liar, Arc::new(Anyone), loopback).unwrap());
        let addr = serving.local_addr().unwrap();
        let service = Arc::new(Lying {
            store: Mutex::new(source),
            ids: ids.clone(),
        });
        tokio::spawn(async move {
            let connection: Arc<dyn Connection> = Arc::from(serving.accept().await.unwrap());
            let session = Session::accept(
                connection,
                liar_id,
                VersionText::new("test").unwrap(),
                DEADLINE,
            )
            .await
            .unwrap();
            session
                .serve(service, Arc::new(Anyone), &ServeLimits::new())
                .await
        });
        let dialing = TcpTlsTransport::bind(&asker, Arc::new(Anyone), loopback).unwrap();
        let connection = Arc::from(dialing.connect(liar_id, addr).await.unwrap());
        let session = Session::connect(
            connection,
            asker_id,
            VersionText::new("test").unwrap(),
            DEADLINE,
        )
        .await
        .unwrap();

        let fetched = fetch_snapshot(&here, &session, SnapshotId::of(b"a snapshot")).await;
        assert!(fetched.is_err(), "{fetched:?}");
        let here = here.lock().unwrap_or_else(PoisonError::into_inner);
        let stored: Vec<&ChunkId> = ids
            .iter()
            .filter(|id| here.objects().stored_len(**id).is_ok())
            .collect();
        assert!(
            stored.is_empty(),
            "chunks past the declared length were stored: {stored:?}"
        );
    }
}
