//! Proves the catalog rules: publish is idempotent and never names a missing
//! object or bundle base, one process holds a store at a time, gc removes
//! nothing before its grace and removes the row before the file, and `check`
//! sees a missing or corrupt object, a wrong refcount and a broken chain.

use std::error::Error;
use std::fs;
use std::time::{Duration, SystemTime};

use fabric_core::git::GitOid;
use fabric_core::id::{ChunkId, JobId, NodeId, SnapshotId};
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::snapshot::{
    Entry, EntryKind, GitHead, GitState, HarnessState, MAX_MANIFEST_LEN, ManifestError, Mode,
    Platform, SnapshotManifest,
};
use fabric_store::catalog::CatalogError;
use fabric_store::objects::{ChunkParams, ObjectError};
use fabric_store::store::{CheckDepth, Store, StoreError};

mod common;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

const JOB: JobId = JobId::from_bytes([3; 16]);

fn params() -> Result<ChunkParams, ObjectError> {
    ChunkParams::new(4096, 64, 256, 1024, 3)
}

fn file_entry(store: &Store, path: &str, content: &[u8]) -> Fallible<Entry> {
    let stored = store.objects().put_bytes(content)?;
    Ok(Entry {
        rel_path: RelPath::parse(path)?,
        kind: EntryKind::File {
            size: stored.size,
            chunks: stored.chunks,
        },
        mode: Mode::new(0o644).ok_or("mode")?,
        mtime_ns: 1_700_000_000_000_000_000,
    })
}

fn chunks_of(entry: &Entry) -> Fallible<Vec<ChunkId>> {
    match &entry.kind {
        EntryKind::File { chunks, .. } => Ok(chunks.clone()),
        _ => Err("not a file entry".into()),
    }
}

fn manifest(files: Vec<Entry>, created_at: u64) -> Fallible<SnapshotManifest> {
    let mut files = files;
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(SnapshotManifest {
        job_id: JOB,
        root: AbsPath::parse("/w")?,
        root_meta: None,
        parent: None,
        created_at,
        created_by: NodeId::from_bytes([9; 32]),
        platform: Platform {
            os: "macos".into(),
            arch: "aarch64".into(),
        },
        git: None,
        files,
        harness_state: HarnessState::default(),
        env_spec: None,
        caches: Vec::new(),
        left_behind: Vec::new(),
    })
}

#[test]
fn publish_is_idempotent_and_the_manifest_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let m = manifest(
        vec![
            file_entry(&store, "a.txt", b"alpha").unwrap(),
            file_entry(&store, "big.bin", &vec![7u8; 10_000]).unwrap(),
        ],
        1,
    )
    .unwrap();
    let id = store.publish(&m).unwrap();
    assert_eq!(store.publish(&m).unwrap(), id);
    assert_eq!(id, m.id().unwrap());
    assert_eq!(store.manifest(id).unwrap(), m);
    let row = store.snapshot(id).unwrap();
    assert_eq!(row.job_id, JOB);
    assert_eq!(row.created_at, 1);
    assert_eq!(row.manifest_len, m.encode().unwrap().len() as u64);
    assert_eq!(store.snapshots().unwrap().len(), 1);
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

/// Reads one object as stored and decompresses it, as a peer receiving it would.
fn as_received(store: &Store, id: ChunkId) -> Fallible<Vec<u8>> {
    let frame = store.objects().read_stored(id)?;
    Ok(zstd::bulk::decompress(&frame, 8 << 20)?)
}

#[test]
fn a_snapshot_rebuilds_in_another_store_from_its_stored_objects_alone() {
    let from_dir = tempfile::tempdir().unwrap();
    let to_dir = tempfile::tempdir().unwrap();
    let mut from = Store::create(from_dir.path(), params().unwrap()).unwrap();
    let mut to = Store::create(to_dir.path(), params().unwrap()).unwrap();
    let m = manifest(
        vec![
            file_entry(&from, "a.txt", b"alpha").unwrap(),
            file_entry(&from, "big.bin", &vec![7u8; 10_000]).unwrap(),
        ],
        4,
    )
    .unwrap();
    let id = from.publish(&m).unwrap();

    let (len, manifest_ids) = from.manifest_chunks(id).unwrap();
    let mut bytes = Vec::new();
    for chunk in manifest_ids {
        let part = as_received(&from, chunk).unwrap();
        to.objects().put(&part).unwrap();
        bytes.extend_from_slice(&part);
    }
    assert_eq!(bytes.len() as u64, len);
    assert_eq!(SnapshotId::of(&bytes), id);
    let decoded = SnapshotManifest::decode(&bytes).unwrap();
    for chunk in Store::data_chunks(&decoded) {
        to.objects()
            .put(&as_received(&from, chunk).unwrap())
            .unwrap();
    }
    assert_eq!(to.publish(&decoded).unwrap(), id);
    assert!(to.check(CheckDepth::Hashes).unwrap().is_clean());
    assert!(matches!(
        from.objects().read_stored(ChunkId::of(b"never written")),
        Err(ObjectError::Missing { .. })
    ));
}

#[test]
fn publish_refuses_a_manifest_that_names_a_missing_object() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let ghost = ChunkId::of(b"never written");
    let m = manifest(
        vec![Entry {
            rel_path: RelPath::parse("ghost").unwrap(),
            kind: EntryKind::File {
                size: 13,
                chunks: vec![ghost],
            },
            mode: Mode::new(0o644).unwrap(),
            mtime_ns: 0,
        }],
        2,
    )
    .unwrap();
    assert!(matches!(
        store.publish(&m),
        Err(StoreError::MissingChunk { chunk }) if chunk == ghost
    ));
    assert!(store.snapshots().unwrap().is_empty());
    assert!(matches!(
        store.manifest(SnapshotId::of(b"x")),
        Err(StoreError::UnknownSnapshot(_))
    ));
}

#[test]
fn publish_refuses_a_file_whose_objects_do_not_add_up_to_its_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let real = file_entry(&store, "real", b"seven b").unwrap();
    let chunks = chunks_of(&real).unwrap();
    for (size, chunks) in [(8, chunks), (5, Vec::new())] {
        let m = manifest(
            vec![Entry {
                rel_path: RelPath::parse("short").unwrap(),
                kind: EntryKind::File { size, chunks },
                mode: Mode::new(0o644).unwrap(),
                mtime_ns: 0,
            }],
            2,
        )
        .unwrap();
        assert!(matches!(
            store.publish(&m),
            Err(StoreError::SizeMismatch { size: s, .. }) if s == size
        ));
    }
    assert!(store.snapshots().unwrap().is_empty());
}

#[test]
fn publish_refuses_a_manifest_no_reader_would_decode() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let mut entry = file_entry(&store, "a", b"a").unwrap();
    // Each chunk id encodes as its 32 bytes.
    let count = MAX_MANIFEST_LEN / 32 + 1;
    let chunks = chunks_of(&entry).unwrap().repeat(count);
    entry.kind = EntryKind::File { size: 1, chunks };
    let m = manifest(vec![entry], 4).unwrap();
    assert!(matches!(
        store.publish(&m),
        Err(StoreError::Manifest(ManifestError::TooLarge { .. }))
    ));
    assert!(store.snapshots().unwrap().is_empty());
}

#[test]
fn publish_refuses_a_bundle_base_the_store_does_not_have() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let ghost = SnapshotId::of(b"never published");
    let oid = GitOid::parse(&"0".repeat(40)).unwrap();
    let mut m = manifest(vec![file_entry(&store, "a", b"a").unwrap()], 3).unwrap();
    m.git = Some(GitState {
        bundle: Vec::new(),
        bundle_base: Some(ghost),
        refs: Vec::new(),
        symbolic_refs: Vec::new(),
        head: GitHead::Branch("main".parse().unwrap()),
        index_commit: oid.clone(),
        worktree_commit: oid,
        worktree_mtimes: Vec::new(),
        worktree_modes: Vec::new(),
        config: Vec::new(),
        exclude: None,
    });
    assert!(matches!(
        store.publish(&m),
        Err(StoreError::MissingBundleBase { base }) if base == ghost
    ));
    assert!(store.snapshots().unwrap().is_empty());
}

/// Git state that names no objects, with ids `width` hex digits wide.
fn empty_git(width: usize, bundle_base: Option<SnapshotId>) -> Fallible<GitState> {
    let oid = GitOid::parse(&"0".repeat(width))?;
    Ok(GitState {
        bundle: Vec::new(),
        bundle_base,
        refs: Vec::new(),
        symbolic_refs: Vec::new(),
        head: GitHead::Branch("main".parse()?),
        index_commit: oid.clone(),
        worktree_commit: oid,
        worktree_mtimes: Vec::new(),
        worktree_modes: Vec::new(),
        config: Vec::new(),
        exclude: None,
    })
}

#[test]
fn publish_refuses_a_bundle_base_without_git_state_in_the_same_format() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let plain = store
        .publish(&manifest(vec![file_entry(&store, "a", b"a").unwrap()], 1).unwrap())
        .unwrap();
    let mut sha1 = manifest(Vec::new(), 2).unwrap();
    sha1.git = Some(empty_git(40, None).unwrap());
    let sha1 = store.publish(&sha1).unwrap();

    for (base, width) in [(plain, 40), (sha1, 64)] {
        let mut child = manifest(Vec::new(), 3).unwrap();
        child.git = Some(empty_git(width, Some(base)).unwrap());
        let err = store.publish(&child).unwrap_err();
        assert!(
            matches!(err, StoreError::IncompatibleBundleBase { base: b } if b == base),
            "{err}"
        );
    }
    let mut child = manifest(Vec::new(), 3).unwrap();
    child.git = Some(empty_git(40, Some(sha1)).unwrap());
    assert!(store.publish(&child).is_ok());
    assert_eq!(store.snapshots().unwrap().len(), 3);
}

#[test]
fn a_second_open_of_a_held_store_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _store = Store::create(dir.path(), params().unwrap()).unwrap();
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::Locked { .. })
    ));
}

#[test]
fn creating_a_store_over_one_is_refused_and_its_params_survive() {
    let dir = tempfile::tempdir().unwrap();
    drop(Store::create(dir.path(), params().unwrap()).unwrap());
    assert!(matches!(
        Store::create(dir.path(), params().unwrap()),
        Err(StoreError::AlreadyAStore { .. })
    ));
    assert!(!dir.path().join("catalog.sqlite.tmp").exists());
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(*reopened.objects().params(), params().unwrap());
}

#[test]
fn opening_a_directory_that_is_not_a_store_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::NotAStore { .. })
    ));
}

#[test]
fn a_catalog_with_a_foreign_schema_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    drop(Store::create(dir.path(), params().unwrap()).unwrap());
    let conn = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
    conn.execute("UPDATE schema_version SET version = 99", [])
        .unwrap();
    drop(conn);
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::Catalog(CatalogError::SchemaVersion {
            found: 99,
            ..
        }))
    ));
}

#[test]
fn gc_keeps_shared_objects_and_removes_the_rest_after_the_grace() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let shared = file_entry(&store, "shared", b"both snapshots have me").unwrap();
    let only_a = file_entry(&store, "only-a", b"first only").unwrap();
    let a = store
        .publish(&manifest(vec![shared.clone(), only_a.clone()], 1).unwrap())
        .unwrap();
    let b = store
        .publish(&manifest(vec![shared.clone()], 2).unwrap())
        .unwrap();
    let a_chunks = chunks_of(&only_a).unwrap();
    let shared_chunks = chunks_of(&shared).unwrap();
    let grace = Duration::from_hours(1);
    let t0 = SystemTime::now();

    store.forget(a).unwrap();
    assert!(matches!(
        store.forget(a),
        Err(StoreError::UnknownSnapshot(_))
    ));
    let first = store.gc(t0, grace).unwrap();
    assert!(first.marked >= 2, "{first:?}");
    assert_eq!(first.removed_chunks, 0);
    assert!(common::object_path(dir.path(), a_chunks[0]).exists());

    let second = store.gc(t0 + grace / 2, grace).unwrap();
    assert_eq!(second.removed_chunks, 0);

    let third = store.gc(t0 + grace, grace).unwrap();
    assert_eq!(third.removed_chunks, first.marked);
    assert!(third.removed_bytes > 0);
    assert!(!common::object_path(dir.path(), a_chunks[0]).exists());
    assert!(common::object_path(dir.path(), shared_chunks[0]).exists());
    assert!(store.manifest(b).is_ok());
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

#[test]
fn gc_sweeps_orphan_objects_and_staged_files_only_after_the_grace() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let orphan = store.objects().put(b"written but never published").unwrap();
    fs::write(dir.path().join("tmp/leftover.tmp"), b"half").unwrap();

    let patient = store
        .gc(SystemTime::now(), Duration::from_hours(1))
        .unwrap();
    assert_eq!(patient.removed_tmp, 1);
    assert_eq!(patient.removed_orphans, 0);
    assert!(common::object_path(dir.path(), orphan.id).exists());

    let eager = store.gc(SystemTime::now(), Duration::ZERO).unwrap();
    assert_eq!(eager.removed_orphans, 1);
    assert!(!common::object_path(dir.path(), orphan.id).exists());
}

#[test]
fn check_sees_a_missing_object_and_a_corrupt_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "f", b"content under check").unwrap();
    let id = store
        .publish(&manifest(vec![entry.clone()], 1).unwrap())
        .unwrap();
    let chunks = chunks_of(&entry).unwrap();
    let path = common::object_path(dir.path(), chunks[0]);
    let good = fs::read(&path).unwrap();

    fs::remove_file(&path).unwrap();
    let report = store.check(CheckDepth::Presence).unwrap();
    assert_eq!(report.missing, vec![(id, chunks[0])]);
    assert!(!report.is_clean());

    fs::write(&path, zstd::bulk::compress(b"other content", 3).unwrap()).unwrap();
    assert!(store.check(CheckDepth::Presence).unwrap().is_clean());
    let report = store.check(CheckDepth::Hashes).unwrap();
    assert_eq!(report.corrupt, vec![(id, chunks[0])]);

    fs::write(&path, good).unwrap();
    let report = store.check(CheckDepth::Hashes).unwrap();
    assert!(report.is_clean());
    assert!(report.refcount_mismatches.is_empty());
}

#[test]
fn check_sees_a_wrong_refcount_and_a_broken_chain() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "f", b"counted").unwrap();
    let id = store
        .publish(&manifest(vec![entry.clone()], 1).unwrap())
        .unwrap();
    let chunk = chunks_of(&entry).unwrap()[0];
    drop(store);

    let ghost = SnapshotId::of(b"gone");
    let conn = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
    conn.execute(
        "UPDATE chunks SET refcount = 7 WHERE id = ?1",
        [chunk.to_bytes()],
    )
    .unwrap();
    conn.execute(
        "UPDATE snapshots SET bundle_base = ?1 WHERE id = ?2",
        rusqlite::params![ghost.to_bytes(), id.to_bytes()],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(dir.path()).unwrap();
    let report = store.check(CheckDepth::Presence).unwrap();
    assert_eq!(report.refcount_mismatches, vec![chunk]);
    assert_eq!(report.broken_chains, vec![(id, ghost)]);
    assert!(!report.is_clean());
}

#[test]
fn check_at_hash_depth_sees_a_manifest_that_does_not_assemble() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "f", b"listed").unwrap();
    let id = store.publish(&manifest(vec![entry], 1).unwrap()).unwrap();
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
    drop(store);

    let conn = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
    conn.execute(
        "UPDATE snapshots SET manifest_len = manifest_len + 1 WHERE id = ?1",
        [id.to_bytes()],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(dir.path()).unwrap();
    assert!(matches!(
        store.manifest(id),
        Err(StoreError::ManifestMismatch(_))
    ));
    let report = store.check(CheckDepth::Hashes).unwrap();
    assert_eq!(report.bad_manifests, vec![id]);
    assert!(report.missing.is_empty() && report.corrupt.is_empty());
    assert!(!report.is_clean());
}

/// A row past the recorded length is a mismatch found before its object is read.
#[test]
fn manifest_assembly_stops_at_the_recorded_length() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "f", b"listed").unwrap();
    let id = store.publish(&manifest(vec![entry], 1).unwrap()).unwrap();
    drop(store);

    // The extra row names an object with a catalog row and no file, so
    // reading it would fail with `Missing` instead of the mismatch.
    let ghost = ChunkId::of(b"never stored");
    let conn = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO chunks (id, stored_len, refcount) VALUES (?1, 1, 1)",
        [ghost.to_bytes()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO snapshot_chunks (snapshot_id, role, ordinal, chunk_id) VALUES (?1, 0, 1000, ?2)",
        rusqlite::params![id.to_bytes(), ghost.to_bytes()],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(dir.path()).unwrap();
    let err = store.manifest(id).unwrap_err();
    assert!(
        matches!(err, StoreError::ManifestMismatch(m) if m == id),
        "{err}"
    );
}

#[test]
fn check_at_hash_depth_sees_a_row_that_disagrees_with_its_manifest() {
    for column in ["created_at", "parent", "bundle_base"] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
        let entry = file_entry(&store, "f", b"row").unwrap();
        let id = store.publish(&manifest(vec![entry], 1).unwrap()).unwrap();
        drop(store);

        let conn = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
        // A bundle base that names the snapshot itself exists, so no chain looks broken.
        let sql = if column == "created_at" {
            "UPDATE snapshots SET created_at = created_at + 1 WHERE id = ?1".to_owned()
        } else {
            format!("UPDATE snapshots SET {column} = ?1 WHERE id = ?1")
        };
        assert_eq!(conn.execute(&sql, [id.to_bytes()]).unwrap(), 1);
        drop(conn);

        let store = Store::open(dir.path()).unwrap();
        let report = store.check(CheckDepth::Hashes).unwrap();
        assert_eq!(report.bad_manifests, vec![id], "{column}");
        assert!(report.broken_chains.is_empty(), "{column}");
    }
}

#[test]
fn check_at_hash_depth_sees_data_rows_the_manifest_does_not_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "f", b"named").unwrap();
    let other = file_entry(&store, "g", b"other").unwrap();
    let id = store.publish(&manifest(vec![entry], 1).unwrap()).unwrap();
    store
        .publish(&manifest(vec![other.clone()], 2).unwrap())
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
    conn.execute(
        "UPDATE snapshot_chunks SET chunk_id = ?1 WHERE snapshot_id = ?2 AND role = 1",
        rusqlite::params![chunks_of(&other).unwrap()[0].to_bytes(), id.to_bytes()],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(dir.path()).unwrap();
    let report = store.check(CheckDepth::Hashes).unwrap();
    assert!(report.bad_manifests.contains(&id));
}

#[test]
fn a_corrupt_object_left_without_a_row_is_removed_not_published() {
    let dir = tempfile::tempdir().unwrap();
    let m;
    let data;
    {
        let store = Store::create(dir.path(), params().unwrap()).unwrap();
        let entry = file_entry(&store, "a.txt", b"left by a killed publish").unwrap();
        data = chunks_of(&entry).unwrap()[0];
        m = manifest(vec![entry], 7).unwrap();
        store.objects().put_bytes(&m.encode().unwrap()).unwrap();
    }
    let manifest_chunk = ChunkId::of(&m.encode().unwrap());
    let mut store = Store::open(dir.path()).unwrap();
    for chunk in [data, manifest_chunk] {
        let path = common::object_path(dir.path(), chunk);
        let good = fs::read(&path).unwrap();
        fs::write(&path, b"lost in a power cut").unwrap();
        assert!(matches!(
            store.publish(&m),
            Err(StoreError::CorruptUnpublished { chunk: c }) if c == chunk
        ));
        assert!(!path.exists());
        fs::write(&path, good).unwrap();
    }
    let id = store.publish(&m).unwrap();
    assert_eq!(store.manifest(id).unwrap(), m);
}

/// The store as the node's tasks reach it, for the concurrency bench.
mod shared_bench {
    use std::path::{Path, PathBuf};
    use std::sync::{Barrier, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use fabric_core::id::{ChunkId, JobId, NodeId, SnapshotId};
    use fabric_store::capture::{CaptureRequest, UnmeasuredKeys, capture};
    use fabric_store::objects::ChunkParams;
    use fabric_store::policy::IgnorePolicy;
    use fabric_store::restore::{Durability, RestoreRequest, Verify, restore};
    use fabric_store::store::Store;

    use super::{Fallible, common};

    /// Files of each workspace, and their size.
    const FILES: usize = 300;
    const FILE_BYTES: usize = 32 << 10;
    /// One file above the whole-file limit, cut into chunks.
    const LARGE_BYTES: usize = 12 << 20;

    /// How the bench's threads reach the store.
    pub(super) trait Access: Sync {
        /// Runs `work` on the store, as `with_store` runs a closure.
        fn with<T>(&self, work: impl FnOnce(&mut Store) -> T) -> T;
    }

    /// The node at the anchor: one mutex held for each whole closure.
    pub(super) struct OneMutex(pub(super) Mutex<Store>);

    impl Access for OneMutex {
        fn with<T>(&self, work: impl FnOnce(&mut Store) -> T) -> T {
            work(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
        }
    }

    /// Writes a workspace of `FILES` files and one large file, all its own bytes.
    pub(super) fn workspace(root: &Path, seed: u64) -> Fallible {
        std::fs::create_dir_all(root.join("src"))?;
        for i in 0..FILES {
            let seed = seed * 10_000 + i as u64;
            std::fs::write(
                root.join(format!("src/f{i}.bin")),
                common::pseudo_random(FILE_BYTES, seed),
            )?;
        }
        std::fs::write(
            root.join("large.bin"),
            common::pseudo_random(LARGE_BYTES, seed * 10_000 + 9_999),
        )?;
        Ok(())
    }

    /// Captures `root` and publishes it, in one closure as a suspend does.
    pub(super) fn capture_publish(store: &mut Store, root: &Path, at: u64) -> Fallible<SnapshotId> {
        let policy = IgnorePolicy::default();
        let captured = capture(
            store,
            &CaptureRequest {
                root,
                job_id: JobId::from_bytes([4; 16]),
                created_by: NodeId::from_bytes([4; 32]),
                created_at: at,
                parent: None,
                policy: &policy,
                carry_keys: &UnmeasuredKeys,
                harness_state: None,
                env_spec: None,
                allow_large_untracked: false,
            },
        )?;
        Ok(store.publish(&captured.manifest)?)
    }

    /// Restores `snapshot` into `target`, as a resume does.
    pub(super) fn restore_into(store: &Store, snapshot: SnapshotId, target: &Path) -> Fallible {
        restore(
            store,
            &RestoreRequest {
                snapshot,
                target,
                harness_state: None,
                verify: Verify::Sample(32),
                durability: Durability::PowerLoss,
                carry: &|_| true,
            },
        )?;
        Ok(())
    }

    /// One round's numbers.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct Round {
        /// From the start until both captures and the restore ended.
        pub(super) makespan: Duration,
        /// The longest and the 99th percentile wait of a reader's short call.
        pub(super) read_max: Duration,
        pub(super) read_p99: Duration,
        pub(super) reads: usize,
    }

    /// A store in `dir` holding the snapshot of `source`, and one of its chunks.
    pub(super) fn seeded(dir: &Path, source: &Path) -> Fallible<(Store, SnapshotId, ChunkId)> {
        let mut store = Store::create(&dir.join("store"), ChunkParams::DEFAULT)?;
        let snapshot = capture_publish(&mut store, source, 1)?;
        let chunk = *Store::data_chunks(&store.manifest(snapshot)?)
            .first()
            .ok_or("no chunk")?;
        Ok((store, snapshot, chunk))
    }

    /// Two captures, one restore and a reader of short calls, at once, on `access`.
    pub(super) fn round<A: Access>(
        access: &A,
        work: &[PathBuf; 2],
        restored: SnapshotId,
        chunk: ChunkId,
        target: &Path,
    ) -> Fallible<Round> {
        let start = Barrier::new(4);
        let heavy_done = std::sync::atomic::AtomicUsize::new(0);
        let started = Instant::now();
        let (makespan, mut waits) =
            std::thread::scope(|scope| -> Fallible<(Duration, Vec<Duration>)> {
                let mut heavy = Vec::new();
                for (i, root) in work.iter().enumerate() {
                    let (start, done) = (&start, &heavy_done);
                    heavy.push(scope.spawn(move || -> Result<(), String> {
                        start.wait();
                        let made = access
                            .with(|store| capture_publish(store, root, 10 + i as u64))
                            .map(|_| ())
                            .map_err(|e| e.to_string());
                        done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        made
                    }));
                }
                let (start2, done2) = (&start, &heavy_done);
                heavy.push(scope.spawn(move || -> Result<(), String> {
                    start2.wait();
                    let made = access
                        .with(|store| restore_into(store, restored, target))
                        .map_err(|e| e.to_string());
                    done2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    made
                }));
                let reader = scope.spawn(|| -> Result<Vec<Duration>, String> {
                    start.wait();
                    let mut waits = Vec::new();
                    while heavy_done.load(std::sync::atomic::Ordering::SeqCst) < 3 {
                        let asked = Instant::now();
                        access
                            .with(|store| -> Fallible {
                                store.snapshot(restored)?;
                                store.objects().read_stored(chunk)?;
                                Ok(())
                            })
                            .map_err(|e| e.to_string())?;
                        waits.push(asked.elapsed());
                        std::thread::sleep(Duration::from_micros(500));
                    }
                    Ok(waits)
                });
                for thread in heavy {
                    thread.join().map_err(|_| "a thread panicked")??;
                }
                let makespan = started.elapsed();
                let waits = reader.join().map_err(|_| "the reader panicked")??;
                Ok((makespan, waits))
            })?;
        waits.sort();
        Ok(Round {
            makespan,
            read_max: *waits.last().ok_or("no read")?,
            read_p99: waits[(waits.len() - 1) * 99 / 100],
            reads: waits.len(),
        })
    }
}

/// M2-19, measured before the store lock is narrowed: two captures, a
/// restore and a reader of short calls (a snapshot's row and one stored
/// chunk, as a peer's fetch asks) share one store, each through one
/// mutex held for its whole closure, as the node at the anchor holds it.
/// Prints each round. Ignored; run it with `cargo test -p fabric-store
/// --release --test store -- --ignored --exact shared_store_bench
/// --nocapture`, with `FABRIC_BENCH_ROUNDS` rounds (default 15).
#[test]
#[ignore = "benchmark; run on demand with --ignored --nocapture"]
fn shared_store_bench() {
    use shared_bench::{OneMutex, round, seeded, workspace};
    let rounds: usize = std::env::var("FABRIC_BENCH_ROUNDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(15);
    let fixture = tempfile::tempdir().unwrap();
    let work = [fixture.path().join("w1"), fixture.path().join("w2")];
    let source = fixture.path().join("source");
    workspace(&work[0], 1).unwrap();
    workspace(&work[1], 2).unwrap();
    workspace(&source, 3).unwrap();
    println!("round\tarm\tmakespan_ms\tread_p99_ms\tread_max_ms\treads");
    for n in 0..rounds {
        let dir = tempfile::tempdir().unwrap();
        let (store, restored, chunk) = seeded(dir.path(), &source).unwrap();
        let access = OneMutex(std::sync::Mutex::new(store));
        let r = round(
            &access,
            &work,
            restored,
            chunk,
            &dir.path().join("restored"),
        )
        .unwrap();
        println!(
            "{n}\tone_mutex\t{:.1}\t{:.1}\t{:.1}\t{}",
            r.makespan.as_secs_f64() * 1e3,
            r.read_p99.as_secs_f64() * 1e3,
            r.read_max.as_secs_f64() * 1e3,
            r.reads
        );
    }
}
