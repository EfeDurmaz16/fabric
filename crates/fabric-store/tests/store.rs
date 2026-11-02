//! Proves the catalog rules: publish is idempotent and never names a missing
//! object or bundle base, one process holds a store at a time, gc removes
//! nothing before its grace and removes the row before the file, and `check`
//! sees a missing or corrupt object, a wrong refcount and a broken chain.
//! Handles of one store capture, restore, read and publish at once and leave
//! it clean, and gc and forget wait for the store alone.

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

/// Writes a small workspace: files every workspace of the test shares,
/// so handles write the same objects at once, and files of its own,
/// some above the whole-file limit so they are cut into chunks.
fn small_workspace(root: &std::path::Path, seed: u64) -> Fallible {
    fs::create_dir_all(root.join("shared"))?;
    fs::create_dir_all(root.join("own"))?;
    for i in 0..8_u64 {
        fs::write(
            root.join(format!("shared/s{i}")),
            common::pseudo_random(3_000 + 1_500 * usize::try_from(i)?, 100 + i),
        )?;
        fs::write(
            root.join(format!("own/o{i}")),
            common::pseudo_random(2_000 + 2_500 * usize::try_from(i)?, seed * 100 + i),
        )?;
    }
    Ok(())
}

/// Every file under `root`, by its path below it, with its bytes.
fn files_under(root: &std::path::Path) -> Fallible<std::collections::BTreeMap<String, Vec<u8>>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root)?.to_string_lossy().into_owned();
                out.insert(rel, fs::read(&path)?);
            }
        }
    }
    Ok(out)
}

/// M2-19: handles on one store publish the same manifest at once, as two
/// fetches of one snapshot from two peers do. Publishes run one at a
/// time, so each answers the same id, the catalog holds one row and the
/// counts agree.
#[test]
fn handles_that_publish_one_manifest_at_once_make_one_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "a", &common::pseudo_random(20_000, 5)).unwrap();
    let manifest = manifest(vec![entry], 1).unwrap();
    let start = std::sync::Barrier::new(8);
    let ids: Vec<SnapshotId> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (mut handle, manifest, start) = (store.handle(), &manifest, &start);
                scope.spawn(move || {
                    start.wait();
                    handle.publish(manifest).unwrap()
                })
            })
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    });
    assert!(ids.windows(2).all(|pair| pair[0] == pair[1]), "{ids:?}");
    assert_eq!(store.snapshots().unwrap().len(), 1);
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

/// M2-19: captures with their publish, restores and reads run at once on
/// handles of one store, as the node's tasks now do, the captures
/// writing some of the same objects. Each finishes, each restore holds
/// its snapshot's files, and the store checks clean, its counts too.
#[test]
fn captures_restores_and_reads_on_handles_at_once_leave_a_clean_store() {
    use shared_bench::{capture_publish, restore_into};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("store"), params().unwrap()).unwrap();
    let roots: Vec<_> = (0..5_u64)
        .map(|n| {
            let root = dir.path().join(format!("w{n}"));
            small_workspace(&root, n).unwrap();
            root
        })
        .collect();
    let seeded = capture_publish(&mut store, &roots[0], 1).unwrap();
    let done = std::sync::atomic::AtomicBool::new(false);
    let start = std::sync::Barrier::new(7);
    let made: Vec<SnapshotId> = std::thread::scope(|scope| {
        let reader = {
            let (handle, done, start) = (store.handle(), &done, &start);
            scope.spawn(move || {
                start.wait();
                let mut reads = 0;
                while !done.load(std::sync::atomic::Ordering::SeqCst) || reads == 0 {
                    handle.manifest(seeded).unwrap();
                    assert!(handle.check(CheckDepth::Presence).unwrap().is_clean());
                    reads += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        let restorers: Vec<_> = (0..2)
            .map(|n| {
                let (handle, start) = (store.handle(), &start);
                let target = dir.path().join(format!("restored{n}"));
                scope.spawn(move || {
                    start.wait();
                    restore_into(&handle, seeded, &target).unwrap();
                    target
                })
            })
            .collect();
        let captures: Vec<_> = roots[1..]
            .iter()
            .enumerate()
            .map(|(n, root)| {
                let (mut handle, start) = (store.handle(), &start);
                scope.spawn(move || {
                    start.wait();
                    capture_publish(&mut handle, root, 10 + n as u64).unwrap()
                })
            })
            .collect();
        let made = captures.into_iter().map(|t| t.join().unwrap()).collect();
        for restorer in restorers {
            let target = restorer.join().unwrap();
            assert_eq!(
                files_under(&target).unwrap(),
                files_under(&roots[0]).unwrap()
            );
        }
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        reader.join().unwrap();
        made
    });
    assert_eq!(store.snapshots().unwrap().len(), 1 + made.len());
    for (root, snapshot) in roots[1..].iter().zip(&made) {
        let target = dir.path().join(format!("check-{snapshot}"));
        restore_into(&store, *snapshot, &target).unwrap();
        assert_eq!(files_under(&target).unwrap(), files_under(root).unwrap());
    }
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

/// M2-19: gc and forget need the store alone, as they did when a store
/// was one value, so no capture on another handle sits between its
/// objects and its publish under gc, and no publish names a base being
/// forgotten. With another handle open they are refused as `Shared` and
/// change nothing; once it is dropped they run.
#[test]
fn gc_and_forget_are_refused_while_another_handle_lives() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), params().unwrap()).unwrap();
    let entry = file_entry(&store, "a", b"forget me").unwrap();
    let a = store.publish(&manifest(vec![entry], 1).unwrap()).unwrap();
    let other = store.handle();
    assert!(matches!(
        store.forget(a),
        Err(StoreError::Shared { call: "forget", .. })
    ));
    assert!(matches!(
        store.gc(SystemTime::now(), Duration::ZERO),
        Err(StoreError::Shared { call: "gc", .. })
    ));
    assert_eq!(other.snapshots().unwrap().len(), 1);
    drop(other);
    store.forget(a).unwrap();
    let removed = store.gc(SystemTime::now(), Duration::ZERO).unwrap();
    assert!(removed.marked > 0);
}

/// A handle keeps the store open after the one it came from is dropped:
/// another open still finds it held, and opens it once the last handle
/// is gone.
#[test]
fn the_last_handle_keeps_the_store_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), params().unwrap()).unwrap();
    let handle = store.handle();
    drop(store);
    assert!(matches!(
        Store::open(dir.path()),
        Err(StoreError::Locked { .. })
    ));
    assert!(handle.snapshots().unwrap().is_empty());
    drop(handle);
    Store::open(dir.path()).unwrap();
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

    /// The node now: each closure on a handle of its own, as `with_store` runs it.
    pub(super) struct Handles(pub(super) Store);

    impl Access for Handles {
        fn with<T>(&self, work: impl FnOnce(&mut Store) -> T) -> T {
            work(&mut self.0.handle())
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

    /// The median of `values`.
    pub(super) fn median(values: &[f64]) -> f64 {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        if n % 2 == 1 {
            sorted[n / 2]
        } else {
            f64::midpoint(sorted[n / 2 - 1], sorted[n / 2])
        }
    }

    /// The median of `ratios` with its 95% percentile bootstrap interval:
    /// 10,000 resamples from a fixed seed, as `scripts/bench-ab.sh` reports.
    pub(super) fn bootstrap(ratios: &[f64]) -> (f64, f64, f64) {
        let mut state: u64 = 0x5eed_f00d;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut medians: Vec<f64> = (0..10_000)
            .map(|_| {
                let sample: Vec<f64> = (0..ratios.len())
                    .map(|_| ratios[usize::try_from(next()).unwrap_or(0) % ratios.len()])
                    .collect();
                median(&sample)
            })
            .collect();
        medians.sort_by(f64::total_cmp);
        (median(ratios), medians[250], medians[9_749])
    }
}

/// M2-19: two captures with their publish, a restore and a reader of
/// short calls (a snapshot's row and one stored chunk, as a peer's fetch
/// asks) share one store at once. The anchor's arm reaches it through one
/// mutex held for each whole closure, as the node did; the candidate's
/// through a handle per closure, as `with_store` now does. The arms take
/// turns round by round, the one that goes first alternating (ABAB), each
/// round on a store of its own. Prints every round, then each measure's
/// median ratio, candidate over anchor, with its 95% bootstrap interval,
/// and checks that both arms wrote the same objects. Ignored; run it
/// with `cargo test -p fabric-store --release --test store -- --ignored
/// --exact shared_store_bench --nocapture`, with `FABRIC_BENCH_PAIRS`
/// pairs (default 15, the fewest the performance rule accepts).
#[test]
#[ignore = "benchmark; run on demand with --ignored --nocapture"]
fn shared_store_bench() {
    use shared_bench::{Handles, OneMutex, Round, bootstrap, round, seeded, workspace};
    let pairs: usize = std::env::var("FABRIC_BENCH_PAIRS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(15);
    let fixture = tempfile::tempdir().unwrap();
    let work = [fixture.path().join("w1"), fixture.path().join("w2")];
    let source = fixture.path().join("source");
    workspace(&work[0], 1).unwrap();
    workspace(&work[1], 2).unwrap();
    workspace(&source, 3).unwrap();
    let run = |anchor: bool| -> (Round, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let (store, restored, chunk) = seeded(dir.path(), &source).unwrap();
        let target = dir.path().join("restored");
        let (measured, store) = if anchor {
            let access = OneMutex(std::sync::Mutex::new(store));
            let measured = round(&access, &work, restored, chunk, &target).unwrap();
            (measured, access.0.into_inner().unwrap())
        } else {
            let access = Handles(store);
            let measured = round(&access, &work, restored, chunk, &target).unwrap();
            (measured, access.0)
        };
        let mut snapshots: Vec<String> = store
            .snapshots()
            .unwrap()
            .iter()
            .map(|row| row.id.to_string())
            .collect();
        snapshots.sort_unstable();
        (measured, snapshots)
    };
    println!("pair\tarm\tmakespan_ms\tread_p99_ms\tread_max_ms\treads");
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    // The anchor's reader makes one or two calls a round, so its p99 is
    // its max: only the max and the makespan are compared.
    let mut ratios: [Vec<f64>; 2] = Default::default();
    for pair in 0..pairs {
        let order = if pair % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        };
        let mut got: [Option<(Round, Vec<String>)>; 2] = [None, None];
        for anchor in order {
            let (r, objects) = run(anchor);
            println!(
                "{pair}\t{}\t{:.1}\t{:.2}\t{:.2}\t{}",
                if anchor { "anchor" } else { "candidate" },
                ms(r.makespan),
                ms(r.read_p99),
                ms(r.read_max),
                r.reads
            );
            got[usize::from(!anchor)] = Some((r, objects));
        }
        let [Some((a, a_made)), Some((c, c_made))] = got else {
            panic!("an arm did not run");
        };
        assert_eq!(a_made, c_made, "the arms published other snapshots");
        ratios[0].push(ms(c.makespan) / ms(a.makespan));
        ratios[1].push(ms(c.read_max) / ms(a.read_max));
    }
    for (name, ratios) in ["makespan", "read max"].iter().zip(&ratios) {
        let (median, low, high) = bootstrap(ratios);
        println!(
            "{name}: candidate/anchor median {median:.3} [{low:.3}-{high:.3}] over {pairs} pairs"
        );
    }
}
