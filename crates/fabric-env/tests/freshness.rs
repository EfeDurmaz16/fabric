//! A cache's proof of freshness covers what fills it (ENV-1, ENV-2): the
//! steps that fill the cache and every dependency file they read, not only
//! the ecosystem's files at the root.

use std::error::Error;
use std::fs;
use std::path::Path;

use fabric_core::path::RelPath;
use fabric_env::detect::{Detection, detect};
use fabric_env::materialize::{CacheHealth, NodeEnv, SENTINEL, TreeLock, plan, sentinel_text};

type TestResult<T> = Result<T, Box<dyn Error>>;

/// Writes each `(path, contents)` under `root`.
fn write(root: &Path, files: &[(&str, &str)]) -> TestResult<()> {
    for (path, contents) in files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
    }
    Ok(())
}

/// Takes the tree's lock in this test binary's own lock directory.
fn take_lock(root: &Path) -> TestResult<TreeLock> {
    Ok(TreeLock::acquire(
        &Path::new(env!("CARGO_TARGET_TMPDIR")).join("locks"),
        root,
    )?)
}

/// Writes the sentinel a finished install leaves in `cache`.
fn seal(detected: &Detection, cache: &str) -> TestResult<()> {
    let text = sentinel_text(detected, &RelPath::parse(cache)?).ok_or("no step fills the cache")?;
    fs::create_dir_all(detected.root().join(cache))?;
    fs::write(detected.root().join(cache).join(SENTINEL), text)?;
    Ok(())
}

const RECIPE_V1: &str = r#"
[[cache]]
path = "out"
class = "build"

[[setup]]
program = "sh"
args = ["-c", "mkdir -p out && echo v1 > out/version"]
"#;

/// ENV-1: a cache filled by the v1 recipe is not proof for the v2 recipe;
/// the next plan runs the changed step and the cache holds its output.
#[test]
fn a_changed_setup_recipe_fills_its_cache_again() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(dir.path(), &[(".fabric/env.toml", RECIPE_V1)]).unwrap();
    plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap()
    .ready()
    .unwrap()
    .execute(&lock)
    .unwrap();
    write(
        dir.path(),
        &[(".fabric/env.toml", &RECIPE_V1.replace("echo v1", "echo v2"))],
    )
    .unwrap();
    let again = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert_ne!(again.caches()[0].health, CacheHealth::Healthy);
    assert_eq!(again.steps().len(), 1);
    again.ready().unwrap().execute(&lock).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("out/version")).unwrap(),
        "v2\n"
    );
}

/// ENV-2: `requirements.txt` that includes `requirements/base.txt` with
/// `-r` depends on both; an edit of the included file makes the `.venv`
/// proof stale.
#[test]
fn an_edited_included_requirements_file_makes_the_venv_stale() {
    let dir = tempfile::tempdir().unwrap();
    let lock = take_lock(dir.path()).unwrap();
    write(
        dir.path(),
        &[
            ("requirements.txt", "-r requirements/base.txt\n"),
            ("requirements/base.txt", "example==1\n"),
        ],
    )
    .unwrap();
    seal(&detect(dir.path(), &[]).unwrap(), ".venv").unwrap();
    let warm = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert!(
        warm.caches()
            .iter()
            .all(|cache| cache.health == CacheHealth::Healthy),
        "{:?}",
        warm.caches()
    );
    write(dir.path(), &[("requirements/base.txt", "example==2\n")]).unwrap();
    let edited = plan(
        &detect(dir.path(), &[]).unwrap(),
        &lock,
        &NodeEnv::current(),
    )
    .unwrap();
    assert!(
        edited
            .caches()
            .iter()
            .any(|cache| cache.health != CacheHealth::Healthy),
        "{:?}",
        edited.caches()
    );
}
