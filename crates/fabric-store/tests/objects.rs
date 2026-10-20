//! Proves the object store's rules: an object is written once and never
//! rewritten, a corrupt file is an error and not content, and a file above
//! the whole-file limit is cut into chunks that reassemble to the same bytes
//! whatever path they came in by.

use std::fs;
use std::os::unix::fs::MetadataExt;

use fabric_core::id::ChunkId;
use fabric_store::objects::{ChunkParams, MAX_OBJECT_LEN, ObjectError};
use fabric_store::store::Store;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

mod common;

fn small_params() -> Result<ChunkParams, ObjectError> {
    ChunkParams::new(4096, 64, 256, 1024, 3)
}

#[test]
fn an_object_is_written_once_and_read_back_verified() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params().unwrap()).unwrap();
    let objects = store.objects();
    let first = objects.put(b"hello").unwrap();
    assert!(first.new);
    let path = common::object_path(dir.path(), first.id);
    let inode = fs::metadata(&path).unwrap().ino();
    let second = objects.put(b"hello").unwrap();
    assert!(!second.new);
    assert_eq!(second.id, first.id);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(objects.read(first.id).unwrap(), b"hello");
    assert!(common::object_path(dir.path(), first.id).exists());
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
}

#[test]
fn a_corrupt_object_is_an_error_not_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params().unwrap()).unwrap();
    let objects = store.objects();
    let stored = objects.put(b"the right bytes").unwrap();
    let path = common::object_path(dir.path(), stored.id);
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
fn an_object_above_the_bound_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params().unwrap()).unwrap();
    let len = MAX_OBJECT_LEN as usize + 1;
    assert!(matches!(
        store.objects().put(&vec![0u8; len]),
        Err(ObjectError::TooLarge { len: refused }) if refused == len
    ));
    let written: usize = fs::read_dir(dir.path().join("objects"))
        .unwrap()
        .map(|shard| fs::read_dir(shard.unwrap().path()).unwrap().count())
        .sum();
    assert_eq!(written, 0);
}

#[test]
fn a_large_file_is_chunked_the_same_from_disk_and_from_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), small_params().unwrap()).unwrap();
    let objects = store.objects();
    let bytes = common::pseudo_random(20_000, 42);
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
    let store = Store::create(dir.path(), small_params().unwrap()).unwrap();
    let objects = store.objects();
    let mut runner = TestRunner::new(Config {
        cases: 48,
        ..Config::default()
    });
    let strategy =
        (0usize..12_000, any::<u64>()).prop_map(|(len, seed)| common::pseudo_random(len, seed));
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
    let mib = 1 << 20;
    let refused = |(limit, min, avg, max, level): (u32, u32, u32, u32, i32), what: &str| {
        let err = ChunkParams::new(limit, min, avg, max, level).unwrap_err();
        assert!(err.to_string().contains(what), "{err}");
    };
    refused((8 * mib, mib, 8 * mib, 8 * mib, 3), "avg_size");
    refused((8 * mib, 2 * mib, mib, 8 * mib, 3), "min <= avg <= max");
    refused((16 * mib, mib, 4 * mib, 8 * mib, 3), "8 MiB");
    refused((8 * mib, mib, 4 * mib, 16 * mib, 3), "8 MiB");
    refused((8 * mib, mib, 4 * mib, 8 * mib, 0), "zstd_level");
    assert_eq!(
        ChunkParams::new(8 * mib, mib, 4 * mib, 8 * mib, 3).unwrap(),
        ChunkParams::DEFAULT
    );
}

#[test]
fn a_path_that_is_not_a_regular_file_is_refused_unread() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(&dir.path().join("store"), small_params().unwrap()).unwrap();
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
