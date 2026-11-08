//! The sync scenario of the bench: what `fabric-sync` moves for a dirty
//! tree with a `node_modules`, as counters, never times.
//!
//! A repository of 300 tracked files with staged, unstaged and untracked
//! work, a `node_modules` of 2,000 files and a `dist`, pushed from one home
//! to a directory target and pulled into a second home. Then ten tracked
//! files and one dependency change, and the push and the pull run again,
//! then a push with nothing changed. Each op prints a `COUNTER` line per
//! counter (bytes sent against the tree's size on disk and the snapshot's
//! stored size, objects sent against objects needed, the hit ratio, the
//! files) and a table. The counters do not depend on the machine's load:
//! the objects are the same on every machine for the same tree, and only
//! the manifest's bytes move by a few, since it holds the tree's path and
//! the time. A change that sends more shows as a different number. It checks nothing and is ignored by default;
//! `scripts/bench-m2.sh` runs it beside the M2 bench.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use fabric_sync::{Home, PullRequest, PushRequest, SyncReport, Target, pull, push};

use crate::world::Fallible;

const TRACKED: usize = 300;
const PACKAGES: usize = 80;
const PER_PACKAGE: usize = 25;

fn git(dir: &Path, args: &[&str]) -> Fallible {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@example.com",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "1760000000 +0000")
        .env("GIT_COMMITTER_DATE", "1760000000 +0000")
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed").into())
    }
}

/// Bytes that do not compress, the same for the same seed.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn build(root: &Path) -> Fallible {
    fs::create_dir_all(root.join("src"))?;
    for i in 0..TRACKED {
        fs::write(
            root.join(format!("src/mod{i}.rs")),
            format!("pub fn f{i}() -> u64 {{ {i} }}\n").repeat(40),
        )?;
    }
    fs::write(root.join(".gitignore"), "node_modules/\ndist/\n")?;
    fs::write(root.join("package.json"), "{\"name\":\"bench\"}\n")?;
    git(root, &["init", "-q", "-b", "main"])?;
    git(root, &["add", "-A"])?;
    git(root, &["commit", "-q", "-m", "one"])?;
    fs::write(root.join("src/mod0.rs"), "staged\n")?;
    git(root, &["add", "src/mod0.rs"])?;
    fs::write(root.join("src/mod1.rs"), "unstaged\n")?;
    fs::write(root.join("notes.md"), "untracked\n")?;
    fs::write(root.join(".env"), "TOKEN=never-travels\n")?;
    for pkg in 0..PACKAGES {
        let dir = root.join(format!("node_modules/pkg{pkg}/lib"));
        fs::create_dir_all(&dir)?;
        for i in 0..PER_PACKAGE {
            let seed = (pkg * PER_PACKAGE + i) as u64;
            fs::write(
                dir.join(format!("m{i}.js")),
                noise(seed, 1000 + (pkg * 97 + i * 31) % 3000),
            )?;
        }
    }
    fs::create_dir_all(root.join("dist"))?;
    for i in 0..10u64 {
        fs::write(
            root.join(format!("dist/bundle{i}.js")),
            noise(10_000 + i, 64 << 10),
        )?;
    }
    Ok(())
}

fn change(root: &Path) -> Fallible {
    for i in 2..12 {
        fs::write(
            root.join(format!("src/mod{i}.rs")),
            format!("changed {i}\n"),
        )?;
    }
    fs::write(
        root.join("node_modules/pkg0/lib/m0.js"),
        noise(99_999, 2000),
    )?;
    Ok(())
}

/// Files and bytes under `root`, `.git` left out.
fn on_disk(root: &Path) -> Fallible<(u64, u64)> {
    let mut files = 0;
    let mut bytes = 0;
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                if entry.file_name() != ".git" {
                    stack.push(entry.path());
                }
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    Ok((files, bytes))
}

fn row(out: &mut String, table: &mut String, op: &str, report: &SyncReport, tree: (u64, u64)) {
    let c = report.counters;
    let counters = [
        ("tree_files", tree.0),
        ("tree_bytes", tree.1),
        ("snapshot_files", report.files),
        ("snapshot_bytes", report.snapshot_bytes),
        ("snapshots_sent", c.snapshots),
        ("objects_needed", c.objects_needed),
        ("objects_sent", c.objects_sent),
        ("bytes_sent", c.bytes_sent),
    ];
    for (key, value) in counters {
        let _ = writeln!(out, "COUNTER\t{op}\t{key}\t{value}");
    }
    let _ = writeln!(out, "COUNTER\t{op}\thit_ratio\t{:.4}", c.hit_ratio());
    #[expect(clippy::cast_precision_loss, reason = "a ratio in a report")]
    let share = c.bytes_sent as f64 / tree.1.max(1) as f64;
    let _ = writeln!(
        table,
        "| {op} | {} | {} | {} | {} / {} | {:.3} | {:.4} |",
        tree.0,
        tree.1,
        c.bytes_sent,
        c.objects_sent,
        c.objects_needed,
        c.hit_ratio(),
        share
    );
}

/// Pushes and pulls the bench tree and prints the counters; checks nothing.
#[test]
#[ignore = "benchmark; scripts/bench-m2.sh runs it"]
fn counters() -> Fallible {
    let dir = tempfile::tempdir()?;
    let (a, b) = (dir.path().join("a/app"), dir.path().join("b/app"));
    let (home_a, home_b) = (
        Home::at(&dir.path().join("home-a")),
        Home::at(&dir.path().join("home-b")),
    );
    let target = Target::Dir(dir.path().join("target"));
    let pushed = |home: &Home| -> Fallible<SyncReport> {
        Ok(push(
            home,
            &PushRequest {
                tree: a.clone(),
                target: target.clone(),
                name: None,
                force: false,
            },
        )?)
    };
    let pulled = |replace: bool| -> Fallible<SyncReport> {
        Ok(pull(
            &home_b,
            &PullRequest {
                tree: b.clone(),
                target: target.clone(),
                name: None,
                replace,
            },
        )?)
    };
    build(&a)?;
    let (mut out, mut table) = (String::new(), String::new());
    row(
        &mut out,
        &mut table,
        "push cold",
        &pushed(&home_a)?,
        on_disk(&a)?,
    );
    row(
        &mut out,
        &mut table,
        "pull cold",
        &pulled(false)?,
        on_disk(&b)?,
    );
    change(&a)?;
    row(
        &mut out,
        &mut table,
        "push delta",
        &pushed(&home_a)?,
        on_disk(&a)?,
    );
    row(
        &mut out,
        &mut table,
        "pull delta",
        &pulled(true)?,
        on_disk(&b)?,
    );
    row(
        &mut out,
        &mut table,
        "push unchanged",
        &pushed(&home_a)?,
        on_disk(&a)?,
    );
    println!(
        "{out}\n### Sync counters\n\n| op | tree files | tree bytes | bytes sent | objects sent / needed | hit ratio | sent / tree |\n|---|---:|---:|---:|---:|---:|---:|\n{table}"
    );
    Ok(())
}
