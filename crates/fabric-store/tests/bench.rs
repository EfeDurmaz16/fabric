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
            common::pseudo_random(200 << 10, i),
        )?;
    }
    fs::create_dir_all(root.join("data"))?;
    fs::write(
        root.join("data/large1.bin"),
        common::pseudo_random(LARGE_FILE_BYTES, 1),
    )?;
    fs::write(
        root.join("data/large2.bin"),
        common::pseudo_random(LARGE_FILE_BYTES, 2),
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
    Ok((
        started.elapsed(),
        report.total().files,
        report.bundles_fetched,
    ))
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

/// Commits one change to `root` and captures it after `parent`.
fn commit_step(
    store: &mut Store,
    root: &Path,
    step: u32,
    parent: Option<SnapshotId>,
) -> Fallible<SnapshotId> {
    fs::write(
        root.join("src/step.rs"),
        format!("pub const STEP: u32 = {step};\n"),
    )?;
    let git = TestGit::new(root, 1_760_000_000_000 + u64::from(step) * 1000);
    git.run(&["add", "-A"])?;
    git.run(&["commit", "-q", "-m", &format!("step {step}")])?;
    Ok(snapshot(store, root, parent)?.id)
}

/// Restores `id` into `target` as a node does, flushed for a power loss,
/// and returns the time and how many links it unpacked with git.
fn restore_as_node(store: &Store, id: SnapshotId, target: &Path) -> Fallible<(Duration, u32)> {
    let started = Instant::now();
    let report = restore(
        store,
        &RestoreRequest {
            snapshot: id,
            target,
            harness_state: None,
            verify: Verify::Sample(32),
            durability: Durability::PowerLoss,
            carry: &|_| true,
        },
    )?;
    Ok((started.elapsed(), report.bundles_unpacked))
}

/// A restore at each length of a bundle chain, first with the node's
/// bundle cache empty, which unpacks every link as every restore did
/// before the cache, then right after a restore of the link below, as a
/// resume after the job's last suspend on another node finds it. Each is
/// the fastest of `REPEATS`, taken in turns, so a burst of load elsewhere
/// on the machine reaches both alike and is left out.
#[test]
#[ignore = "benchmark; run on demand with --ignored --nocapture"]
fn restore_time_by_chain_length() {
    const REPEATS: usize = 5;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir_all(root.join("src")).unwrap();
    for i in 0..TRACKED_FILES {
        fs::write(
            root.join(format!("src/mod{i}.rs")),
            format!("pub fn f{i}() -> u64 {{ {i} }}\n").repeat(40),
        )
        .unwrap();
    }
    TestGit::new(&root, 1_760_000_000_000)
        .run(&["init", "-q", "-b", "main"])
        .unwrap();
    let store_dir = dir.path().join("store");
    let mut store = Store::create(&store_dir, ChunkParams::DEFAULT).unwrap();
    let mut chain = Vec::new();
    for step in 0..17 {
        let parent = chain.last().copied();
        chain.push(commit_step(&mut store, &root, step, parent).unwrap());
    }
    let cache = store_dir.join("bundle-cache");
    let mut target = 0;
    let mut next = || {
        target += 1;
        dir.path().join(format!("restore-{target}"))
    };
    println!("| links | cache empty: ms (unpacked) | after the link below: ms (unpacked) |");
    println!("|---:|---:|---:|");
    for (index, id) in chain.iter().enumerate() {
        let (mut cold, mut warm) = ((Duration::MAX, 0), (Duration::MAX, 0));
        for _ in 0..REPEATS {
            let _ = fs::remove_dir_all(&cache);
            if index > 0 {
                restore_as_node(&store, chain[index - 1], &next()).unwrap();
            }
            warm = warm.min(restore_as_node(&store, *id, &next()).unwrap());
            let _ = fs::remove_dir_all(&cache);
            cold = cold.min(restore_as_node(&store, *id, &next()).unwrap());
        }
        println!(
            "| {} | {:.1} ({}) | {:.1} ({}) |",
            index + 1,
            cold.0.as_secs_f64() * 1000.0,
            cold.1,
            warm.0.as_secs_f64() * 1000.0,
            warm.1
        );
    }
}
