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
    Entry, EntryKind, GitHead, GitState, HarnessState, Mode, Platform, SnapshotManifest,
};
use fabric_store::catalog::CatalogError;
use fabric_store::objects::ChunkParams;
use fabric_store::store::{CheckDepth, Store, StoreError};

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

const JOB: JobId = JobId::from_bytes([3; 16]);

fn params() -> ChunkParams {
    ChunkParams {
        whole_file_limit: 4096,
        min_size: 64,
        avg_size: 256,
        max_size: 1024,
        zstd_level: 3,
    }
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
    })
}

#[test]
fn publish_is_idempotent_and_the_manifest_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params()).unwrap();
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
    assert_eq!(store.catalog().snapshots().unwrap().len(), 1);
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

#[test]
fn publish_refuses_a_manifest_that_names_a_missing_object() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params()).unwrap();
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
    assert!(store.catalog().snapshots().unwrap().is_empty());
    assert!(matches!(
        store.manifest(SnapshotId::of(b"x")),
        Err(StoreError::UnknownSnapshot(_))
    ));
}

#[test]
fn publish_refuses_a_bundle_base_the_store_does_not_have() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params()).unwrap();
    let ghost = SnapshotId::of(b"never published");
    let oid = GitOid::parse(&"0".repeat(40)).unwrap();
    let mut m = manifest(vec![file_entry(&store, "a", b"a").unwrap()], 3).unwrap();
    m.git = Some(GitState {
        bundle: Vec::new(),
        bundle_base: Some(ghost),
        refs: Vec::new(),
        head: GitHead::Branch("main".parse().unwrap()),
        index_commit: oid.clone(),
        worktree_commit: oid,
        worktree_mtimes: Vec::new(),
        config: Vec::new(),
        exclude: None,
    });
    assert!(matches!(
        store.publish(&m),
        Err(StoreError::MissingBundleBase { base }) if base == ghost
    ));
    assert!(store.catalog().snapshots().unwrap().is_empty());
}

#[test]
fn one_process_holds_a_store_and_a_foreign_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), params()).unwrap();
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::Locked { .. })
    ));
    drop(store);
    assert!(matches!(
        Store::create(dir.path(), params()),
        Err(StoreError::AlreadyAStore { .. })
    ));
    assert!(!dir.path().join("catalog.sqlite.tmp").exists());
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(*reopened.objects().params(), params());
    drop(reopened);

    let other = tempfile::tempdir().unwrap();
    assert!(matches!(
        Store::open(other.path()),
        Err(StoreError::NotAStore { .. })
    ));

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
    let mut store = Store::create(dir.path(), params()).unwrap();
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
    assert!(store.objects().contains(a_chunks[0]));

    let second = store.gc(t0 + grace / 2, grace).unwrap();
    assert_eq!(second.removed_chunks, 0);

    let third = store.gc(t0 + grace, grace).unwrap();
    assert_eq!(third.removed_chunks, first.marked);
    assert!(third.removed_bytes > 0);
    assert!(!store.objects().contains(a_chunks[0]));
    assert!(store.objects().contains(shared_chunks[0]));
    assert!(
        store
            .catalog()
            .chunk_ids()
            .unwrap()
            .contains(&shared_chunks[0])
    );
    assert!(store.manifest(b).is_ok());
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

#[test]
fn gc_sweeps_orphan_objects_and_staged_files_only_after_the_grace() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params()).unwrap();
    let orphan = store.objects().put(b"written but never published").unwrap();
    fs::write(dir.path().join("tmp/leftover.tmp"), b"half").unwrap();

    let patient = store
        .gc(SystemTime::now(), Duration::from_hours(1))
        .unwrap();
    assert_eq!(patient.removed_tmp, 1);
    assert_eq!(patient.removed_orphans, 0);
    assert!(store.objects().contains(orphan.id));

    let eager = store.gc(SystemTime::now(), Duration::ZERO).unwrap();
    assert_eq!(eager.removed_orphans, 1);
    assert!(!store.objects().contains(orphan.id));
}

#[test]
fn check_sees_a_missing_object_and_a_corrupt_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params()).unwrap();
    let entry = file_entry(&store, "f", b"content under check").unwrap();
    let id = store
        .publish(&manifest(vec![entry.clone()], 1).unwrap())
        .unwrap();
    let chunks = chunks_of(&entry).unwrap();
    let path = store.objects().object_path(chunks[0]);
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
    let mut store = Store::create(dir.path(), params()).unwrap();
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
fn a_corrupt_object_left_without_a_row_is_removed_not_published() {
    let dir = tempfile::tempdir().unwrap();
    let m;
    let data;
    {
        let store = Store::create(dir.path(), params()).unwrap();
        let entry = file_entry(&store, "a.txt", b"left by a killed publish").unwrap();
        data = chunks_of(&entry).unwrap()[0];
        m = manifest(vec![entry], 7).unwrap();
        store.objects().put_bytes(&m.encode().unwrap()).unwrap();
    }
    let manifest_chunk = ChunkId::of(&m.encode().unwrap());
    let mut store = Store::open(dir.path()).unwrap();
    for chunk in [data, manifest_chunk] {
        let path = store.objects().object_path(chunk);
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
