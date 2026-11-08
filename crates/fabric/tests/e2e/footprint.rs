//! The allocation budget of the CLI's start. `fabric --version` builds
//! the command tree, parses once and exits, so its allocations are what
//! every command pays before it does any work. Main took a regression
//! here once, unseen by every test (docs/evidence/perf-ab/README.md, CLI
//! footprint): the contract built and cloned the whole decorated tree and
//! parsed twice on every run, 2408 allocations in a release build where
//! the fix makes 442, and the perf job of pull request #43 caught it.
//!
//! The counts come from `scripts/perf-ab/counters.c`, loaded with
//! `DYLD_INSERT_LIBRARIES`, so the test runs on macOS only, and only where
//! `cc` builds the library; elsewhere it says so and passes. The counts do
//! not depend on load, so the budget can be tight: about twice what the
//! test build made when the budget was set (825 allocations of 303 KB on
//! the tree with the M5 managed commands), well under half of what the
//! old parse made (1948 of 784 KB). The tree grows with the command set,
//! so a budget hit after new commands is read against the perf job's
//! footprint table before it is raised.

use std::path::Path;
use std::process::Command;

use crate::ab::Page;
use crate::world::Fallible;

/// The allocations `fabric --version` may make, in the test build.
const VERSION_MALLOCS: u64 = 1_600;

/// The bytes those allocations may ask for, in the test build.
const VERSION_MALLOC_BYTES: u64 = 600_000;

#[test]
fn version_allocates_under_its_budget() -> Fallible {
    if !cfg!(target_os = "macos") {
        eprintln!("the counter library loads on macOS only; nothing counted");
        return Ok(());
    }
    let dir = tempfile::Builder::new()
        .prefix("ffoot")
        .tempdir_in("/tmp")?;
    let library = dir.path().join("libcounters.dylib");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/perf-ab/counters.c");
    let built = Command::new("cc")
        .args(["-dynamiclib", "-O2", "-o"])
        .arg(&library)
        .arg(&source)
        .status();
    if !built.is_ok_and(|status| status.success()) {
        eprintln!("cc did not build the counter library; nothing counted");
        return Ok(());
    }
    let counts = dir.path().join("counts");
    std::fs::create_dir(&counts)?;
    let home = dir.path().join("home");
    let output = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .arg("--version")
        .env("DYLD_INSERT_LIBRARIES", &library)
        .env("FABRIC_AB_COUNTS", &counts)
        .env("FABRIC_HOME", &home)
        .output()?;
    assert!(output.status.success(), "{output:?}");
    let mut pages = Vec::new();
    for entry in std::fs::read_dir(&counts)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "page") {
            pages.extend(Page::parse(&std::fs::read(&path)?)?);
        }
    }
    let [page] = pages.as_slice() else {
        return Err(format!("{} counter pages, not one", pages.len()).into());
    };
    let mallocs = page.counts["mallocs"];
    let bytes = page.counts["malloc_bytes"];
    eprintln!("fabric --version: {mallocs} allocations, {bytes} bytes");
    assert!(
        mallocs <= VERSION_MALLOCS,
        "fabric --version made {mallocs} allocations, over its budget of {VERSION_MALLOCS}"
    );
    assert!(
        bytes <= VERSION_MALLOC_BYTES,
        "fabric --version asked for {bytes} bytes, over its budget of {VERSION_MALLOC_BYTES}"
    );
    Ok(())
}
