//! Measures capture, publish and restore on a synthetic workspace and prints
//! the numbers. Ignored by default; run it with `cargo test -p fabric-store
//! --release --test bench -- --ignored --nocapture`.

use std::error::Error;
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_store::capture::{BundleKind, CaptureRequest, UnmeasuredKeys, capture};
use fabric_store::objects::ChunkParams;
use fabric_store::policy::IgnorePolicy;
use fabric_store::restore::{Durability, RestoreRequest, Verify, restore};
use fabric_store::store::Store;

mod common;

use common::TestGit;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

const SMALL_FILES: usize = 5_000;
const TRACKED_FILES: usize = 300;
const LARGE_FILE_BYTES: usize = 24 << 20;

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

fn build_workspace(root: &Path) -> Fallible {
    fs::create_dir_all(root.join("src"))?;
    for i in 0..TRACKED_FILES {
        fs::write(
            root.join(format!("src/mod{i}.rs")),
            format!("pub fn f{i}() -> u64 {{ {i} }}\n").repeat(40),
        )?;
    }
    fs::write(root.join(".gitignore"), "node_modules/\ndist/\ndata/\n")?;
    let git = TestGit::new(root, 1_760_000_000_000);
    git.run(&["init", "-q", "-b", "main"])?;
    git.run(&["add", "-A"])?;
    git.run(&["commit", "-q", "-m", "one"])?;
    for pkg in 0..SMALL_FILES / 25 {
        let dir = root.join(format!("node_modules/pkg{pkg}/lib"));
        fs::create_dir_all(&dir)?;
        for i in 0..25 {
            let len = 100 + (pkg * 31 + i * 97) % 4000;
            fs::write(
                dir.join(format!("m{i}.js")),
                format!("// pkg{pkg} m{i}\n").repeat(len / 14 + 1),
            )?;
        }
    }
    fs::create_dir_all(root.join("dist"))?;
    for i in 0..20u64 {
        fs::write(
            root.join(format!("dist/bundle{i}.js")),
            pseudo_random(200 << 10, i),
        )?;
    }
    fs::create_dir_all(root.join("data"))?;
    fs::write(
        root.join("data/large1.bin"),
        pseudo_random(LARGE_FILE_BYTES, 1),
    )?;
    fs::write(
        root.join("data/large2.bin"),
        pseudo_random(LARGE_FILE_BYTES, 2),
    )?;
    Ok(())
}

fn mutate_workspace(root: &Path) -> Fallible {
    let git = TestGit::new(root, 1_760_000_060_000);
    for i in 0..10 {
        fs::write(
            root.join(format!("src/mod{i}.rs")),
            format!("pub fn f{i}() -> u64 {{ {i} + 1 }}\n").repeat(40),
        )?;
    }
    git.run(&["add", "-A"])?;
    git.run(&["commit", "-q", "-m", "two"])?;
    for pkg in 0..2 {
        for i in 0..25 {
            fs::write(
                root.join(format!("node_modules/pkg{pkg}/lib/m{i}.js")),
                format!("// changed pkg{pkg} m{i}\n").repeat(50),
            )?;
        }
    }
    let mut large = File::options()
        .write(true)
        .open(root.join("data/large1.bin"))?;
    large.seek(SeekFrom::Start(12 << 20))?;
    large.write_all(&[0xAB; 1024])?;
    Ok(())
}

fn dir_bytes(path: &Path) -> Fallible<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_bytes(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

/// One snapshot's numbers.
struct Measured {
    id: SnapshotId,
    captured_in: Duration,
    published_in: Duration,
    files: u64,
    bytes: u64,
    bundle: Option<BundleKind>,
    new_objects: u64,
    new_bytes: u64,
    manifest_len: usize,
}

fn snapshot(store: &mut Store, root: &Path, parent: Option<SnapshotId>) -> Fallible<Measured> {
    let policy = IgnorePolicy::default();
    let started = Instant::now();
    let captured = capture(
        store,
        &CaptureRequest {
            root,
            job_id: JobId::from_bytes([1; 16]),
            created_by: NodeId::from_bytes([1; 32]),
            created_at: 1_760_000_000_000 + u64::from(parent.is_some()) * 60_000,
            parent,
            policy: &policy,
            carry_keys: &UnmeasuredKeys,
            harness_state: None,
            env_spec: None,
            allow_large_untracked: false,
        },
    )?;
    let captured_in = started.elapsed();
    let started = Instant::now();
    let id = store.publish(&captured.manifest)?;
    let r = captured.report;
    Ok(Measured {
        id,
        captured_in,
        published_in: started.elapsed(),
        files: r.files,
        bytes: r.bytes,
        bundle: r.bundle,
        new_objects: r.new_objects,
        new_bytes: r.new_bytes,
        manifest_len: captured.manifest.encode()?.len(),
    })
}

fn restore_timed(store: &Store, id: SnapshotId, target: &Path) -> Fallible<(Duration, u64, u32)> {
    let started = Instant::now();
    let report = restore(
        store,
        &RestoreRequest {
            snapshot: id,
            target,
            harness_state: None,
            verify: Verify::None,
            durability: Durability::ProcessCrash,
            carry: &|_| true,
        },
    )?;
    Ok((started.elapsed(), report.files, report.bundles_fetched))
}

#[test]
#[ignore = "benchmark; run on demand with --ignored --nocapture"]
fn capture_and_restore_timings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    let store_dir = dir.path().join("store");
    let started = Instant::now();
    build_workspace(&root).unwrap();
    println!(
        "fixture: {} MiB in {:.2?}",
        dir_bytes(&root).unwrap() >> 20,
        started.elapsed()
    );
    let mut store = Store::create(&store_dir, ChunkParams::DEFAULT).unwrap();

    let report = |label: &str, m: &Measured, store: &Store| {
        println!(
            "{label}: capture {:.2?} ({} files, {} MiB, bundle {:?}); publish {:.2?}; new objects {} ({} MiB on disk); manifest {} KiB; store {} MiB",
            m.captured_in,
            m.files,
            m.bytes >> 20,
            m.bundle,
            m.published_in,
            m.new_objects,
            m.new_bytes >> 20,
            m.manifest_len >> 10,
            dir_bytes(&store_dir).unwrap() >> 20
        );
        let (elapsed, files, bundles) =
            restore_timed(store, m.id, &dir.path().join(format!("restore-{label}"))).unwrap();
        println!("{label}: restore {elapsed:.2?} ({files} files, {bundles} bundles)");
    };
    let first = snapshot(&mut store, &root, None).unwrap();
    report("first", &first, &store);
    mutate_workspace(&root).unwrap();
    let second = snapshot(&mut store, &root, Some(first.id)).unwrap();
    report("second", &second, &store);
}
