//! Proves the object store's rules: an object is written once and never
//! rewritten, a corrupt file is an error and not content, and a file above
//! the whole-file limit is cut into chunks that reassemble to the same bytes
//! whatever path they came in by.

use std::fs;
use std::os::unix::fs::MetadataExt;

use fabric_core::id::ChunkId;
use fabric_store::objects::{ChunkParams, ObjectError};
use fabric_store::store::Store;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

fn small_params() -> ChunkParams {
    ChunkParams {
        whole_file_limit: 4096,
        min_size: 64,
        avg_size: 256,
        max_size: 1024,
        zstd_level: 3,
    }
}

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[3]
        })
        .collect()
}

#[test]
fn an_object_is_written_once_and_read_back_verified() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params()).unwrap();
    let objects = store.objects();
    let first = objects.put(b"hello").unwrap();
    assert!(first.new);
    let path = objects.object_path(first.id);
    let inode = fs::metadata(&path).unwrap().ino();
    let second = objects.put(b"hello").unwrap();
    assert!(!second.new);
    assert_eq!(second.id, first.id);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(objects.read(first.id).unwrap(), b"hello");
    assert!(objects.contains(first.id));
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
}

#[test]
fn a_corrupt_object_is_an_error_not_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params()).unwrap();
    let objects = store.objects();
    let stored = objects.put(b"the right bytes").unwrap();
    let path = objects.object_path(stored.id);
    let other = zstd::bulk::compress(b"the wrong bytes", 3).unwrap();
    fs::write(&path, other).unwrap();
    assert!(matches!(
        objects.read(stored.id),
        Err(ObjectError::Corrupt { id }) if id == stored.id
    ));
    fs::write(&path, b"not zstd at all").unwrap();
    assert!(matches!(
        objects.read(stored.id),
        Err(ObjectError::Corrupt { .. })
    ));
    let missing = ChunkId::of(b"never stored");
    assert!(matches!(
        objects.read(missing),
        Err(ObjectError::Missing { id }) if id == missing
    ));
}

#[test]
fn a_large_file_is_chunked_the_same_from_disk_and_from_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params()).unwrap();
    let objects = store.objects();
    let bytes = pseudo_random(20_000, 42);
    let file = dir.path().join("big.bin");
    fs::write(&file, &bytes).unwrap();
    let from_disk = objects.put_file(&file).unwrap();
    let from_memory = objects.put_bytes(&bytes).unwrap();
    assert!(
        from_disk.chunks.len() > 1,
        "{} chunks",
        from_disk.chunks.len()
    );
    assert_eq!(from_disk.chunks, from_memory.chunks);
    assert_eq!(from_disk.size, 20_000);
    assert_eq!(from_memory.new_objects, 0);
    let mut joined = Vec::new();
    for chunk in &from_disk.chunks {
        let part = objects.read(*chunk).unwrap();
        assert!(part.len() <= 1024);
        joined.extend(part);
    }
    assert_eq!(joined, bytes);
    let small = objects.put_bytes(&bytes[..4096]).unwrap();
    assert_eq!(small.chunks.len(), 1);
}

#[test]
fn chunking_round_trips_and_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params()).unwrap();
    let objects = store.objects();
    let mut runner = TestRunner::new(Config {
        cases: 48,
        ..Config::default()
    });
    let strategy = (0usize..12_000, any::<u64>()).prop_map(|(len, seed)| pseudo_random(len, seed));
    runner
        .run(&strategy, |bytes| {
            let fail = |e: ObjectError| TestCaseError::fail(e.to_string());
            let first = objects.put_bytes(&bytes).map_err(fail)?;
            let second = objects.put_bytes(&bytes).map_err(fail)?;
            prop_assert_eq!(&first.chunks, &second.chunks);
            prop_assert_eq!(second.new_objects, 0);
            prop_assert_eq!(first.size, bytes.len() as u64);
            let mut joined = Vec::new();
            for chunk in &first.chunks {
                joined.extend(objects.read(*chunk).map_err(fail)?);
            }
            prop_assert_eq!(joined, bytes);
            Ok(())
        })
        .unwrap();
}

#[test]
fn chunk_parameters_outside_the_chunker_rules_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bad = ChunkParams {
        avg_size: 8 << 20,
        ..ChunkParams::DEFAULT
    };
    let err = Store::create(dir.path(), bad).unwrap_err();
    assert!(err.to_string().contains("avg_size"), "{err}");
    let inverted = ChunkParams {
        min_size: 2 << 20,
        avg_size: 1 << 20,
        ..ChunkParams::DEFAULT
    };
    assert!(inverted.validate().is_err());
    assert!(ChunkParams::DEFAULT.validate().is_ok());
}

#[test]
fn a_path_that_is_not_a_regular_file_is_refused_unread() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(&dir.path().join("store"), small_params()).unwrap();
    let objects = store.objects();
    let secret = dir.path().join("secret");
    fs::write(&secret, "do not read").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let fifo = dir.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    for path in [link, fifo] {
        let err = objects.put_file(&path).unwrap_err();
        assert!(matches!(err, ObjectError::NotRegular { .. }), "{err}");
    }
}
