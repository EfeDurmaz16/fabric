//! Proves capture and restore against real repositories: the staged and
//! unstaged split survives, refs and the stash come back, secrets never enter
//! a manifest, a second snapshot carries only new objects, a hostile manifest
//! cannot write outside the target, and the refusals of ADR 0004 fire.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use fabric_core::git::GitConfigKey;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::path::RelPath;
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, Entry, EntryKind, GitHead, ManifestError, Mode,
};
use fabric_store::capture::{
    BundleKind, BusyReason, CaptureError, CaptureRequest, Captured, CarryKeys, IndexFlag,
    MAX_BUNDLE_CHAIN, UnmeasuredKeys, capture,
};
use fabric_store::objects::ChunkParams;
use fabric_store::policy::{IgnorePolicy, PolicyError, SECRET_PATTERNS};
use fabric_store::restore::{
    Durability, RestoreError, RestoreReport, RestoreRequest, RestoreState, Verify, restore,
    restore_state, verify,
};
use fabric_store::store::{CheckDepth, Store, StoreError};
use fabric_store::writer::WriteError;

mod common;

use common::TestGit;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

const JOB: JobId = JobId::from_bytes([5; 16]);
const NODE: NodeId = NodeId::from_bytes([2; 32]);
const T0: u64 = 1_760_000_000_000;

fn git(root: &Path) -> TestGit {
    TestGit::new(root, T0)
}

fn set_mtime(path: &Path, secs: u64, nanos: u32) -> Fallible {
    let time = UNIX_EPOCH + Duration::new(secs, nanos);
    File::open(path)?.set_modified(time)?;
    Ok(())
}

fn store_in(dir: &Path) -> Fallible<Store> {
    Ok(Store::create(
        &dir.join("store"),
        ChunkParams::new(64 << 10, 4 << 10, 16 << 10, 64 << 10, 3)?,
    )?)
}

fn request<'a>(
    root: &'a Path,
    policy: &'a IgnorePolicy,
    parent: Option<SnapshotId>,
) -> CaptureRequest<'a> {
    CaptureRequest {
        root,
        job_id: JOB,
        created_by: NODE,
        created_at: T0 + u64::from(parent.is_some()) * 60_000,
        parent,
        policy,
        carry_keys: &UnmeasuredKeys,
        harness_state: None,
        env_spec: None,
        allow_large_untracked: false,
    }
}

fn restore_into(
    store: &Store,
    snapshot: SnapshotId,
    target: &Path,
) -> Result<RestoreReport, RestoreError> {
    restore(
        store,
        &RestoreRequest {
            snapshot,
            target,
            harness_state: None,
            verify: Verify::All,
            durability: Durability::ProcessCrash,
            carry: &|_| true,
        },
    )
}

/// A repository with every kind of change ADR 0004 names.
fn make_repo(root: &Path) -> Fallible {
    fs::create_dir_all(root.join("d"))?;
    fs::write(root.join("a"), "a\n")?;
    fs::write(root.join("d/b"), "b\n")?;
    fs::write(root.join("d/c"), "c\n")?;
    fs::write(root.join("e"), "e\n")?;
    fs::write(root.join("g"), "g\n")?;
    fs::write(root.join("exec.sh"), "#!/bin/sh\n")?;
    fs::set_permissions(root.join("exec.sh"), fs::Permissions::from_mode(0o755))?;
    symlink("a", root.join("link"))?;
    fs::write(root.join(".gitignore"), "*.log\nnode_modules/\n")?;
    let g = git(root);
    g.run(&["init", "-q", "-b", "main"])?;
    g.run(&["add", "-A"])?;
    g.run(&["commit", "-q", "-m", "one"])?;
    g.run(&["tag", "v1"])?;
    g.run(&["branch", "feature"])?;
    g.run(&[
        "remote",
        "add",
        "origin",
        "https://user:token@example.com/x.git",
    ])?;
    fs::write(root.join("a"), "a stashed\n")?;
    g.run(&["stash", "push", "-q", "-m", "wip"])?;
    fs::write(g.git_path("info/exclude")?, "scratch/\n")?;

    fs::write(root.join("a"), "a staged\n")?;
    g.run(&["add", "a"])?;
    fs::write(root.join("d/b"), "b unstaged\n")?;
    fs::remove_file(root.join("d/c"))?;
    g.run(&["mv", "e", "f"])?;
    fs::set_permissions(root.join("g"), fs::Permissions::from_mode(0o755))?;
    fs::write(root.join("new.txt"), "untracked\n")?;
    symlink("d/b", root.join("newlink"))?;
    fs::write(root.join(".env"), "SECRET=1\n")?;
    fs::write(root.join("build.log"), "precious ignored\n")?;
    fs::create_dir_all(root.join("node_modules/pkg"))?;
    fs::write(
        root.join("node_modules/pkg/index.js"),
        "module.exports = 1;\n",
    )?;
    fs::write(
        root.join("node_modules/pkg/cert.pem"),
        "fixture, not a secret here\n",
    )?;
    set_mtime(
        &root.join("node_modules/pkg/index.js"),
        1_600_000_000,
        123_456_789,
    )?;
    set_mtime(&root.join("build.log"), 1_650_000_000, 42)?;
    set_mtime(&root.join("d/b"), 1_655_000_000, 7)?;
    Ok(())
}

fn status(root: &Path) -> Fallible<String> {
    Ok(git(root)
        .run_text(&["status", "--porcelain=v2", "--untracked-files=all"])?
        .lines()
        .filter(|l| !l.ends_with(" .env"))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn git_text(root: &Path, args: &[&str]) -> Fallible<String> {
    git(root).run_text(args)
}

fn mtime_of(path: &Path) -> Fallible<(i64, i64)> {
    let meta = fs::metadata(path)?;
    Ok((meta.mtime(), meta.mtime_nsec()))
}

/// Returns the first git view that differs between two repositories, if any.
fn git_difference(root: &Path, target: &Path) -> Fallible<Option<String>> {
    if status(target)? != status(root)? {
        return Ok(Some("status".to_owned()));
    }
    for args in [
        &["diff"][..],
        &["diff", "--cached"],
        &["for-each-ref"],
        &["stash", "list"],
        &["log", "--oneline", "--all"],
    ] {
        if git_text(target, args)? != git_text(root, args)? {
            return Ok(Some(args.join(" ")));
        }
    }
    Ok(None)
}

#[test]
fn git_state_survives_capture_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();

    assert_eq!(report.bundle, Some(BundleKind::Full));
    assert_eq!(report.denied, Vec::<RelPath>::new());
    assert_eq!(
        report.denied_untracked,
        vec![RelPath::parse(".env").unwrap()]
    );
    assert_eq!(
        report.stripped_userinfo,
        vec![GitConfigKey::parse("remote.origin.url").unwrap()]
    );
    assert!(report.tracked_secrets.is_empty());
    let state = manifest.git.as_ref().unwrap();
    assert_eq!(state.head, GitHead::Branch("main".parse().unwrap()));
    assert!(state.refs.iter().any(|(r, _)| r.as_str() == "refs/stash"));
    assert!(state.refs.iter().any(|(r, _)| r.as_str() == "refs/tags/v1"));
    assert!(state.exclude.is_some());
    let paths: Vec<&str> = manifest.files.iter().map(|e| e.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "build.log",
            "d",
            "node_modules",
            "node_modules/pkg",
            "node_modules/pkg/cert.pem",
            "node_modules/pkg/index.js"
        ]
    );
    assert_eq!(
        manifest.caches,
        vec![CacheEntry {
            path: RelPath::parse("node_modules").unwrap(),
            class: CacheClass::Deps,
            carry: CarryState::Carried,
            key: CarryKey::Unmeasured,
        }]
    );

    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    let restored = restore(
        &store,
        &RestoreRequest {
            snapshot,
            target: &target,
            harness_state: None,
            verify: Verify::All,
            durability: Durability::PowerLoss,
            carry: &|_| true,
        },
    )
    .unwrap();
    assert_eq!(restored.files, 3);
    // Three ignored files, two directories of theirs, one of W and the nine entries of W.
    assert_eq!(restored.verified, 15);
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Present);

    assert_eq!(git_difference(&root, &target).unwrap(), None);
    assert_eq!(
        git_text(&target, &["config", "remote.origin.url"]).unwrap(),
        "https://example.com/x.git"
    );
    assert_eq!(
        fs::read_to_string(git(&target).git_path("info/exclude").unwrap()).unwrap(),
        "scratch/\n"
    );
    assert!(!target.join(".env").exists());
    assert_eq!(
        fs::read_link(target.join("link")).unwrap(),
        PathBuf::from("a")
    );
    assert_eq!(
        fs::read_link(target.join("newlink")).unwrap(),
        PathBuf::from("d/b")
    );
    assert_eq!(
        fs::metadata(target.join("exec.sh")).unwrap().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(target.join("g")).unwrap().mode() & 0o777,
        0o755
    );
    assert_eq!(
        mtime_of(&target.join("node_modules/pkg/index.js")).unwrap(),
        (1_600_000_000, 123_456_789)
    );
    assert_eq!(
        mtime_of(&target.join("build.log")).unwrap(),
        (1_650_000_000, 42)
    );
    assert_eq!(mtime_of(&target.join("d/b")).unwrap(), (1_655_000_000, 7));
    assert_eq!(
        fs::read_to_string(target.join("new.txt")).unwrap(),
        "untracked\n"
    );
    assert!(store.check(CheckDepth::Hashes).unwrap().is_clean());
}

/// A directory of tracked files keeps its mode and mtime, which no git tree records.
#[test]
fn a_tracked_directory_keeps_its_mode_and_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir_all(root.join("private/keys")).unwrap();
    fs::write(root.join("private/keys/notes.txt"), "notes\n").unwrap();
    let g = git(&root);
    g.run(&["init", "-q", "-b", "main"]).unwrap();
    g.run(&["add", "-A"]).unwrap();
    g.run(&["commit", "-q", "-m", "one"]).unwrap();
    fs::set_permissions(root.join("private"), fs::Permissions::from_mode(0o700)).unwrap();
    set_mtime(&root.join("private/keys"), 1_500_000_000, 5).unwrap();

    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    assert_eq!(
        fs::metadata(target.join("private")).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(
        mtime_of(&target.join("private/keys")).unwrap(),
        (1_500_000_000, 5)
    );
    assert_eq!(git_difference(&root, &target).unwrap(), None);
}

/// An empty directory in a repository, untracked or ignored, travels like one in a plain directory.
#[test]
fn an_empty_directory_in_a_repository_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    fs::write(root.join(".gitignore"), "build/\n").unwrap();
    for empty in ["logs", "build/cache", "deep/er/est"] {
        fs::create_dir_all(root.join(empty)).unwrap();
    }
    set_mtime(&root.join("logs"), 1_500_000_000, 3).unwrap();

    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    for empty in ["logs", "build/cache", "deep/er/est"] {
        assert!(target.join(empty).is_dir(), "{empty}");
        assert_eq!(
            fs::read_dir(target.join(empty)).unwrap().count(),
            0,
            "{empty}"
        );
    }
    assert_eq!(mtime_of(&target.join("logs")).unwrap(), (1_500_000_000, 3));
}

#[test]
fn a_sha256_repository_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a"), "a\n").unwrap();
    let g = git(&root);
    g.run(&["init", "-q", "-b", "main", "--object-format=sha256"])
        .unwrap();
    g.run(&["add", "-A"]).unwrap();
    g.run(&["commit", "-q", "-m", "one"]).unwrap();
    fs::write(root.join("a"), "a staged\n").unwrap();
    g.run(&["add", "a"]).unwrap();
    fs::write(root.join("a"), "a unstaged\n").unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    assert_eq!(
        git_text(&target, &["rev-parse", "--show-object-format"]).unwrap(),
        "sha256"
    );
    assert_eq!(git_difference(&root, &target).unwrap(), None);
}

#[test]
fn verify_sees_a_tampered_file_and_link_in_the_working_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();

    assert_eq!(
        verify(&store, snapshot, &target, Verify::All, &|_| true).unwrap(),
        15
    );
    fs::write(target.join("new.txt"), "cut short").unwrap();
    assert!(matches!(
        verify(&store, snapshot, &target, Verify::All, &|_| true),
        Err(RestoreError::VerifyFailed { path }) if path.as_str() == "new.txt"
    ));
    fs::write(target.join("new.txt"), "untracked\n").unwrap();
    fs::remove_file(target.join("newlink")).unwrap();
    symlink("a", target.join("newlink")).unwrap();
    assert!(matches!(
        verify(&store, snapshot, &target, Verify::All, &|_| true),
        Err(RestoreError::VerifyFailed { path }) if path.as_str() == "newlink"
    ));
}

#[test]
fn a_second_snapshot_carries_only_new_objects_and_restores_through_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let first = capture(&store, &request(&root, &policy, None)).unwrap();
    let s1 = store.publish(&first.manifest).unwrap();

    fs::write(root.join("second.txt"), "second commit\n").unwrap();
    git(&root).run(&["add", "second.txt"]).unwrap();
    git(&root).run(&["commit", "-q", "-m", "two"]).unwrap();
    fs::write(root.join("build.log"), "precious ignored, changed\n").unwrap();
    let second = capture(&store, &request(&root, &policy, Some(s1))).unwrap();
    assert_eq!(second.report.bundle, Some(BundleKind::Incremental));
    assert_eq!(second.manifest.git.as_ref().unwrap().bundle_base, Some(s1));
    assert_eq!(second.manifest.parent, Some(s1));
    // The new bundle, the changed log and nothing from node_modules.
    assert!(
        second.report.new_objects <= 3,
        "{}",
        second.report.new_objects
    );
    let s2 = store.publish(&second.manifest).unwrap();

    let user_refs = |root: &Path| -> Vec<String> {
        git_text(root, &["for-each-ref"])
            .unwrap()
            .lines()
            .filter(|line| !line.contains("\trefs/fabric/"))
            .map(str::to_owned)
            .collect()
    };
    let before = user_refs(&root);
    let third = capture(&store, &request(&root, &policy, Some(s2))).unwrap();
    assert_eq!(user_refs(&root), before);
    assert_eq!(third.report.bundle, Some(BundleKind::Empty));
    assert!(third.manifest.git.as_ref().unwrap().bundle.is_empty());
    let s3 = store.publish(&third.manifest).unwrap();

    let snap_refs = git_text(
        &root,
        &["for-each-ref", "--format=%(refname)", "refs/fabric/"],
    )
    .unwrap();
    assert_eq!(snap_refs.lines().count(), 1, "{snap_refs}");

    let target = dir.path().join("restored");
    let report = restore_into(&store, s3, &target).unwrap();
    assert_eq!(report.bundles_fetched, 2);
    assert_eq!(git_difference(&root, &target).unwrap(), None);
    assert_eq!(
        fs::read_to_string(target.join("build.log")).unwrap(),
        "precious ignored, changed\n"
    );
    // Scratch files live in the store's tmp/ and are gone afterwards.
    assert_eq!(
        fs::read_dir(dir.path().join("store/tmp")).unwrap().count(),
        0
    );

    // s2's bundle excludes s1's refs, so s1 must outlive it.
    assert!(matches!(store.forget(s1), Err(StoreError::BaseInUse { base }) if base == s1));
    assert!(store.check(CheckDepth::Presence).unwrap().is_clean());
    store.forget(s3).unwrap();
    store.forget(s2).unwrap();
    store.forget(s1).unwrap();
}

fn sample_for(pattern: &str) -> String {
    let name = pattern.replace('*', "x");
    if matches!(pattern, ".aws" | ".ssh" | ".gnupg") {
        format!("{name}/inner")
    } else {
        name
    }
}

fn secret_fixture(root: &Path) -> Fallible {
    for base in ["", "precious/"] {
        for pattern in SECRET_PATTERNS {
            let path = root.join(format!("{base}{}", sample_for(pattern)));
            fs::create_dir_all(path.parent().ok_or("parent")?)?;
            fs::write(&path, "secret")?;
        }
    }
    fs::write(root.join("precious/keep.txt"), "keep")?;
    fs::create_dir_all(root.join("node_modules/pkg"))?;
    fs::write(root.join("node_modules/pkg/cert.pem"), "fixture")?;
    fs::write(root.join("node_modules/pkg/.env"), "leak")?;
    fs::write(root.join(".fabricignore"), "scratch\n")?;
    fs::create_dir_all(root.join("scratch"))?;
    fs::write(root.join("scratch/out.bin"), "excluded")?;
    Ok(())
}

#[test]
fn every_secret_pattern_keeps_its_file_out_of_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    secret_fixture(&root).unwrap();

    let store = store_in(dir.path()).unwrap();
    let mut policy = IgnorePolicy::default();
    policy.load_fabricignore(&root).unwrap();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert!(manifest.git.is_none());
    assert_eq!(report.bundle, None);

    let files: Vec<&str> = manifest.files.iter().map(|e| e.rel_path.as_str()).collect();
    for base in ["", "precious/"] {
        for pattern in SECRET_PATTERNS {
            let sample = format!("{base}{}", sample_for(pattern));
            let rel = RelPath::parse(&sample).unwrap();
            assert!(
                report.denied.contains(&rel),
                "{pattern}: {sample} not denied"
            );
            assert!(
                !files.contains(&sample.as_str()),
                "{pattern}: {sample} in manifest"
            );
        }
    }
    assert!(files.contains(&"precious/keep.txt"));
    assert!(files.contains(&"node_modules/pkg/cert.pem"));
    assert!(!files.contains(&"node_modules/pkg/.env"));
    assert!(!files.contains(&"scratch/out.bin"));
    assert_eq!(
        report.excluded,
        vec![RelPath::parse("scratch/out.bin").unwrap()]
    );
    assert!(files.contains(&".fabricignore"));
    assert_eq!(manifest.caches.len(), 1);
    assert_eq!(manifest.caches[0].carry, CarryState::Partial);
    assert_eq!(report.denied.len(), SECRET_PATTERNS.len() * 2 + 1);
}

fn worktree_paths(
    root: &Path,
    manifest: &fabric_core::snapshot::SnapshotManifest,
) -> Fallible<Vec<String>> {
    let w = manifest
        .git
        .as_ref()
        .ok_or("no git state")?
        .worktree_commit
        .clone();
    Ok(
        git_text(root, &["ls-tree", "-r", "--name-only", w.as_str()])?
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
fn a_gitignore_negation_cannot_put_a_secret_into_the_worktree_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    fs::write(
        root.join(".gitignore"),
        "*.log\nnode_modules/\n*.local\n!.env.local\n",
    )
    .unwrap();
    fs::write(root.join(".env.local"), "TOKEN=1\n").unwrap();
    fs::create_dir(root.join("keys")).unwrap();
    fs::write(root.join("keys/server.pem"), "pem\n").unwrap();
    fs::write(root.join(".fabricignore"), "keys\n").unwrap();
    let store = store_in(dir.path()).unwrap();
    let mut policy = IgnorePolicy::default();
    policy.load_fabricignore(&root).unwrap();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();

    let mut denied = report.denied_untracked.clone();
    denied.sort();
    assert_eq!(
        denied,
        vec![
            RelPath::parse(".env").unwrap(),
            RelPath::parse(".env.local").unwrap(),
            RelPath::parse("keys/server.pem").unwrap(),
        ]
    );
    let paths = worktree_paths(&root, &manifest).unwrap();
    assert!(paths.contains(&".gitignore".to_owned()));
    assert!(paths.contains(&"new.txt".to_owned()));
    for secret in [".env", ".env.local", "keys/server.pem"] {
        assert!(
            !paths.contains(&secret.to_owned()),
            "{secret} in W: {paths:?}"
        );
    }
    assert!(matches!(
        policy.add_exclude("!.env"),
        Err(PolicyError::BadPolicyPattern { .. })
    ));
}

#[test]
fn an_untracked_nested_repository_is_left_behind_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("inner.txt"), "inner\n").unwrap();
    git(&nested).run(&["init", "-q"]).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert_eq!(
        report.nested_repositories,
        vec![RelPath::parse("nested").unwrap()]
    );
    assert_eq!(
        report.denied_untracked,
        vec![RelPath::parse(".env").unwrap()]
    );
    let paths = worktree_paths(&root, &manifest).unwrap();
    assert!(!paths.iter().any(|p| p.starts_with("nested")), "{paths:?}");

    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    assert!(!target.join("nested").exists());
    assert_eq!(
        fs::read_to_string(target.join("new.txt")).unwrap(),
        "untracked\n"
    );
}

fn hostile_repo(root: &Path, outside: &Path) -> Fallible {
    fs::create_dir(root)?;
    fs::write(root.join("tracked.txt"), "tracked\n")?;
    symlink(outside, root.join("escape"))?;
    let g = git(root);
    g.run(&["init", "-q", "-b", "main"])?;
    g.run(&["add", "-A"])?;
    g.run(&["commit", "-q", "-m", "one"])?;
    Ok(())
}

#[test]
fn a_hostile_manifest_cannot_write_through_a_link_or_over_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let root = dir.path().join("work");
    hostile_repo(&root, &outside).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert_eq!(
        report.escaping_symlinks,
        vec![RelPath::parse("escape").unwrap()]
    );

    let evil = store.objects().put_bytes(b"evil").unwrap();
    let entry = |path: &str| Entry {
        rel_path: RelPath::parse(path).unwrap(),
        kind: EntryKind::File {
            size: evil.size,
            chunks: evil.chunks.clone(),
        },
        mode: Mode::new(0o644).unwrap(),
        mtime_ns: 0,
    };

    // A path below a working-tree link is refused before it can publish.
    let mut through_link = manifest.clone();
    through_link.files = vec![entry("escape/evil")];
    let err = store.publish(&through_link).unwrap_err();
    assert!(
        matches!(&err, StoreError::Manifest(ManifestError::TreesNest { files, worktree })
            if files.as_str() == "escape/evil" && worktree.as_str() == "escape"),
        "{err}"
    );
    // Validation sees the working tree only through `worktree_mtimes`, which
    // a peer can leave the link out of; the writer still refuses the path.
    let git_state = through_link.git.as_mut().unwrap();
    git_state
        .worktree_mtimes
        .retain(|(path, _)| path.as_str() != "escape");
    let id = store.publish(&through_link).unwrap();
    let target = dir.path().join("t1");
    let err = restore_into(&store, id, &target).unwrap_err();
    assert!(
        matches!(err, RestoreError::Write(WriteError::ThroughSymlink { .. })),
        "{err}"
    );
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    assert!(matches!(
        restore_state(&target).unwrap(),
        RestoreState::HalfRestored { .. }
    ));
    assert!(!target.exists());

    // A path in both trees is refused before it can publish.
    let mut over_file = manifest.clone();
    over_file.files = vec![entry("tracked.txt")];
    let err = store.publish(&over_file).unwrap_err();
    assert!(
        matches!(&err, StoreError::Manifest(ManifestError::InBothTrees { path }) if path.as_str() == "tracked.txt"),
        "{err}"
    );

    let occupied = dir.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("mine"), "do not touch").unwrap();
    let err = restore_into(&store, id, &occupied).unwrap_err();
    assert!(matches!(err, RestoreError::TargetNotEmpty { .. }), "{err}");
    assert_eq!(restore_state(&occupied).unwrap(), RestoreState::Present);
    assert_eq!(fs::read_dir(&occupied).unwrap().count(), 1);
}

#[test]
fn the_large_untracked_guard_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();

    let huge = root.join("huge.bin");
    File::create(&huge)
        .unwrap()
        .set_len((64 << 20) + 1)
        .unwrap();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::UntrackedTooLarge { paths }
            if paths == &[RelPath::parse("huge.bin").unwrap()]),
        "{err}"
    );
    let mut allowed = request(&root, &policy, None);
    allowed.allow_large_untracked = true;
    assert!(capture(&store, &allowed).is_ok());
}

/// A repository with every kind of change, a store and the default policy.
struct BusyFixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    policy: IgnorePolicy,
}

impl BusyFixture {
    fn new() -> Fallible<BusyFixture> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("work");
        make_repo(&root)?;
        let store = store_in(dir.path())?;
        Ok(BusyFixture {
            dir,
            root,
            store,
            policy: IgnorePolicy::default(),
        })
    }

    /// Captures at `root` and returns the refusal, or `None` when capture succeeds.
    fn capture_at(&self, root: &Path) -> Option<CaptureError> {
        capture(&self.store, &request(root, &self.policy, None)).err()
    }

    fn capture_err(&self) -> Option<CaptureError> {
        self.capture_at(&self.root)
    }
}

#[test]
fn a_merge_in_progress_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    let merge_head = git(&fixture.root).git_path("MERGE_HEAD").unwrap();
    fs::write(&merge_head, "0000000000000000000000000000000000000000\n").unwrap();
    let err = fixture.capture_err().unwrap();
    assert!(
        matches!(
            err,
            CaptureError::GitBusy {
                reason: BusyReason::Merge
            }
        ),
        "{err}"
    );
}

#[test]
fn a_linked_worktree_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    let linked = fixture.dir.path().join("linked");
    git(&fixture.root)
        .run(&["worktree", "add", "-q", linked.to_str().unwrap(), "feature"])
        .unwrap();
    let err = fixture.capture_at(&linked).unwrap();
    assert!(
        matches!(err, CaptureError::GitLinkedWorktree { .. }),
        "{err}"
    );
}

#[test]
fn a_root_below_the_top_level_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    let err = fixture.capture_at(&fixture.root.join("d")).unwrap();
    assert!(
        matches!(err, CaptureError::NotRepositoryRoot { .. }),
        "{err}"
    );
}

#[test]
fn a_rebase_in_progress_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    fs::create_dir(git(&fixture.root).git_path("rebase-merge").unwrap()).unwrap();
    let err = fixture.capture_err().unwrap();
    assert!(
        matches!(
            err,
            CaptureError::GitBusy {
                reason: BusyReason::Rebase
            }
        ),
        "{err}"
    );
}

#[test]
fn a_held_index_lock_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    fs::write(git(&fixture.root).git_path("index.lock").unwrap(), "").unwrap();
    let err = fixture.capture_err().unwrap();
    assert!(
        matches!(
            err,
            CaptureError::GitBusy {
                reason: BusyReason::IndexLocked
            }
        ),
        "{err}"
    );
}

#[test]
fn unmerged_index_entries_are_refused() {
    let fixture = BusyFixture::new().unwrap();
    // A stash applied over a conflicting commit leaves unmerged entries and no MERGE_HEAD.
    let g = git(&fixture.root);
    g.run(&["stash", "push", "-q"]).unwrap();
    fs::write(fixture.root.join("a"), "a committed\n").unwrap();
    g.run(&["commit", "-q", "-am", "conflicting"]).unwrap();
    assert!(!g.succeeds(&["stash", "apply", "-q"]).unwrap());
    let err = fixture.capture_err().unwrap();
    assert!(
        matches!(
            err,
            CaptureError::GitBusy {
                reason: BusyReason::Conflicts
            }
        ),
        "{err}"
    );
}

#[test]
fn a_repository_git_cannot_read_is_an_error_not_a_plain_directory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let config = root.join(".git/config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[broken\n");
    fs::write(&config, text).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(matches!(err, CaptureError::Git(_)), "{err}");
}

#[test]
fn a_shallow_clone_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    fs::create_dir(&upstream).unwrap();
    let u = git(&upstream);
    u.run(&["init", "-q", "-b", "main"]).unwrap();
    for message in ["one", "two"] {
        u.run(&["commit", "-q", "--allow-empty", "-m", message])
            .unwrap();
    }
    let url = format!("file://{}", upstream.display());
    git(dir.path())
        .run(&["clone", "-q", "--depth", "1", &url, "work"])
        .unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let root = dir.path().join("work");
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(matches!(err, CaptureError::GitShallow), "{err}");
}

#[test]
fn an_intent_to_add_entry_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    git(&root).run(&["add", "-N", "new.txt"]).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(
            &err,
            CaptureError::GitIndexFlag { path, flag: IndexFlag::IntentToAdd }
                if path.as_str() == "new.txt"
        ),
        "{err}"
    );
}

#[test]
fn a_sparse_checkout_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let g = git(&root);
    g.run(&["stash", "push", "-q", "--include-untracked"])
        .unwrap();
    g.run(&["sparse-checkout", "set", "--no-cone", "/a", "/d/"])
        .unwrap();
    assert!(!root.join("exec.sh").exists());
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(
            &err,
            CaptureError::GitIndexFlag {
                flag: IndexFlag::SkipWorktree,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn an_assume_unchanged_entry_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    git(&root)
        .run(&["update-index", "--assume-unchanged", "d/b"])
        .unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(
            &err,
            CaptureError::GitIndexFlag { path, flag: IndexFlag::AssumeUnchanged }
                if path.as_str() == "d/b"
        ),
        "{err}"
    );
}

#[test]
fn an_lfs_filter_on_an_untracked_file_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    // Set only in .git/info/attributes.
    let info = git(&fixture.root).git_path("info/attributes").unwrap();
    fs::write(&info, "*.bin filter=lfs\n").unwrap();
    fs::write(fixture.root.join("blob.bin"), "large\n").unwrap();
    let err = fixture.capture_err().unwrap();
    assert!(
        matches!(&err, CaptureError::GitLfsUnsupported { path } if path.as_str() == "blob.bin"),
        "{err}"
    );
}

/// A clean filter puts what it makes of a file into `W`, and restore
/// writes blobs with no smudge, so the target would hold the cleaned bytes:
/// git-crypt's ciphertext where the agent had plaintext. An attribute
/// whose driver is not configured changes nothing and is not refused.
#[test]
fn a_path_under_a_configured_clean_filter_is_refused() {
    let fixture = BusyFixture::new().unwrap();
    fs::write(fixture.root.join(".gitattributes"), "*.up filter=upper\n").unwrap();
    fs::write(fixture.root.join("note.up"), "plain\n").unwrap();
    assert!(fixture.capture_err().is_none());
    git(&fixture.root)
        .run(&["config", "filter.upper.clean", "tr a-z A-Z"])
        .unwrap();
    let err = fixture.capture_err().unwrap();
    assert!(
        matches!(
            &err,
            CaptureError::GitFilterUnsupported { path, filter }
                if path.as_str() == "note.up" && filter == "upper"
        ),
        "{err}"
    );
}

type Listing = BTreeMap<String, (String, u32, i64, i64)>;

fn listing(root: &Path) -> Fallible<Listing> {
    fn walk(base: &Path, rel: &str, out: &mut Listing) -> Fallible {
        for entry in fs::read_dir(base.join(rel))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let meta = fs::symlink_metadata(base.join(&path))?;
            let kind = if meta.file_type().is_symlink() {
                format!("link:{}", fs::read_link(base.join(&path))?.display())
            } else if meta.is_dir() {
                "dir".to_owned()
            } else {
                format!("file:{}", fs::read_to_string(base.join(&path))?)
            };
            out.insert(
                path.clone(),
                (kind, meta.mode() & 0o777, meta.mtime(), meta.mtime_nsec()),
            );
            if meta.is_dir() {
                walk(base, &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out)?;
    Ok(out)
}

#[test]
fn a_secret_in_the_harness_state_is_left_behind_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    let state = dir.path().join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(state.join("sessions")).unwrap();
    fs::write(state.join("sessions/s1.jsonl"), "{}\n").unwrap();
    fs::write(state.join(".credentials.json"), "{\"token\":1}\n").unwrap();

    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let mut req = request(&root, &policy, None);
    req.harness_state = Some(&state);
    let Captured { manifest, report } = capture(&store, &req).unwrap();
    let paths: Vec<&str> = manifest
        .harness_state
        .entries
        .iter()
        .map(|e| e.rel_path.as_str())
        .collect();
    assert_eq!(paths, vec!["sessions", "sessions/s1.jsonl"]);
    assert_eq!(
        report.denied_harness,
        vec![RelPath::parse(".credentials.json").unwrap()]
    );
}

#[test]
fn a_git_file_in_a_plain_directory_is_left_behind_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    fs::create_dir_all(root.join("checkout")).unwrap();
    fs::write(root.join("checkout/.git"), "gitdir: ../../elsewhere\n").unwrap();
    fs::write(root.join("checkout/x.txt"), "x\n").unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    let files: Vec<&str> = manifest.files.iter().map(|e| e.rel_path.as_str()).collect();
    assert_eq!(files, vec!["checkout", "checkout/x.txt"]);
    assert_eq!(
        report.nested_repositories,
        vec![RelPath::parse("checkout").unwrap()]
    );
}

#[test]
fn an_mtime_before_1970_is_refused_not_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("old.txt"), "old\n").unwrap();
    let half_second_before = UNIX_EPOCH - Duration::from_millis(500);
    File::open(root.join("old.txt"))
        .unwrap()
        .set_modified(half_second_before)
        .unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::MtimeBeforeEpoch { path } if path.as_str() == "old.txt"),
        "{err}"
    );
}

#[test]
fn a_link_target_that_is_not_utf8_is_refused_not_rewritten() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    fs::create_dir_all(&root).unwrap();
    symlink(std::ffi::OsStr::from_bytes(b"bad\xff"), root.join("link")).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::NotUtf8 { path } if path == "link"),
        "{err}"
    );
}

fn plain_tree(root: &Path, state: &Path) -> Fallible {
    fs::create_dir_all(root.join("a/b"))?;
    fs::create_dir_all(root.join("empty"))?;
    fs::write(root.join("a/b/deep.txt"), "deep\n")?;
    fs::write(root.join("run.sh"), "#!/bin/sh\n")?;
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o750))?;
    symlink("a/b/deep.txt", root.join("alias"))?;
    set_mtime(&root.join("a/b/deep.txt"), 1_500_000_000, 999)?;
    set_mtime(&root.join("a/b"), 1_500_000_001, 1)?;
    set_mtime(&root.join("empty"), 1_500_000_002, 2)?;
    fs::create_dir_all(state.join("sessions/s1"))?;
    fs::write(state.join("sessions/s1/tool.txt"), "tool\n")?;
    fs::write(state.join("sessions/s1.jsonl"), "{}\n")?;
    Ok(())
}

/// Carry keys that write a file into the workspace the first time capture asks, after the listing.
struct WritesLate<'a> {
    file: &'a Path,
}

impl CarryKeys for WritesLate<'_> {
    fn key(&self, _class: CacheClass, _path: &RelPath) -> CarryKey {
        if !self.file.exists() {
            // A failed write leaves nothing to find, and the test's assertion fails.
            let _ = fs::write(self.file, "written after the listing\n");
        }
        CarryKey::Unmeasured
    }
}

/// A file a still-running harness writes after its directory was listed fails the capture instead of being left out.
#[test]
fn a_file_added_after_the_listing_fails_the_capture() {
    for repository in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "1;\n").unwrap();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        if repository {
            fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
            let g = git(&root);
            g.run(&["init", "-q", "-b", "main"]).unwrap();
            g.run(&["add", "-A"]).unwrap();
            g.run(&["commit", "-q", "-m", "one"]).unwrap();
        }
        let store = store_in(dir.path()).unwrap();
        let policy = IgnorePolicy::default();
        for late in ["late.txt", "node_modules/pkg/late.js"] {
            let file = root.join(late);
            let keys = WritesLate { file: &file };
            let mut req = request(&root, &policy, None);
            req.carry_keys = &keys;
            let err = capture(&store, &req).unwrap_err();
            assert!(
                matches!(&err, CaptureError::WorkspaceChanged { path } if path.as_str() == late),
                "repository {repository}: {err}"
            );
            fs::remove_file(&file).unwrap();
        }
    }
}

/// git leaves a directory it cannot read out of its listings, with a
/// warning, as a container's database volume owned by another user: the
/// walk that finds empty directories leaves it out too and the report
/// names it, instead of failing every capture of the repository.
#[test]
fn a_directory_capture_cannot_read_is_left_behind_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir_all(root.join("pgdata")).unwrap();
    fs::write(root.join("pgdata/PG_VERSION"), "17\n").unwrap();
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join(".gitignore"), "pgdata/\n").unwrap();
    let g = git(&root);
    g.run(&["init", "-q", "-b", "main"]).unwrap();
    g.run(&["add", "-A"]).unwrap();
    g.run(&["commit", "-q", "-m", "one"]).unwrap();
    let pgdata = root.join("pgdata");
    fs::set_permissions(&pgdata, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&pgdata).is_ok() {
        // Run as root, which reads any directory: nothing to prove here.
        fs::set_permissions(&pgdata, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let captured = capture(&store, &request(&root, &policy, None));
    fs::set_permissions(&pgdata, fs::Permissions::from_mode(0o755)).unwrap();
    let Captured { report, .. } = captured.unwrap();
    assert_eq!(report.unreadable, vec![RelPath::parse("pgdata").unwrap()]);
}

#[test]
fn a_plain_directory_and_harness_state_round_trip_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    let state = dir.path().join("state");
    plain_tree(&root, &state).unwrap();

    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let mut req = request(&root, &policy, None);
    req.harness_state = Some(&state);
    let Captured { manifest, report } = capture(&store, &req).unwrap();
    assert_eq!(report.files, 5);
    assert_eq!(manifest.harness_state.entries.len(), 4);
    let snapshot = store.publish(&manifest).unwrap();

    let target = dir.path().join("restored");
    let state_target = dir.path().join("state-restored");
    let report = restore(
        &store,
        &RestoreRequest {
            snapshot,
            target: &target,
            harness_state: Some(&state_target),
            verify: Verify::Sample(1),
            durability: Durability::PowerLoss,
            carry: &|_| true,
        },
    )
    .unwrap();
    assert_eq!(report.files, 5);
    assert_eq!(report.verified, 2);
    assert_eq!(listing(&target).unwrap(), listing(&root).unwrap());
    assert_eq!(listing(&state_target).unwrap(), listing(&state).unwrap());
    // Three directories, two files and a link.
    assert_eq!(
        verify(&store, snapshot, &target, Verify::All, &|_| true).unwrap(),
        6
    );
    fs::remove_dir(target.join("empty")).unwrap();
    assert!(matches!(
        verify(&store, snapshot, &target, Verify::All, &|_| true),
        Err(RestoreError::VerifyFailed { path }) if path.as_str() == "empty"
    ));
    fs::create_dir(target.join("empty")).unwrap();
    fs::remove_file(target.join("alias")).unwrap();
    symlink("run.sh", target.join("alias")).unwrap();
    assert!(matches!(
        verify(&store, snapshot, &target, Verify::All, &|_| true),
        Err(RestoreError::VerifyFailed { path }) if path.as_str() == "alias"
    ));
    fs::remove_file(target.join("alias")).unwrap();
    symlink("a/b/deep.txt", target.join("alias")).unwrap();
    fs::write(target.join("a/b/deep.txt"), "tampered\n").unwrap();
    assert!(matches!(
        verify(&store, snapshot, &target, Verify::All, &|_| true),
        Err(RestoreError::VerifyFailed { path }) if path.as_str() == "a/b/deep.txt"
    ));
}

#[test]
fn verify_reports_a_file_replaced_by_a_fifo_instead_of_waiting_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, snapshot) = plain_snapshot(dir.path()).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    fs::remove_file(target.join("run.sh")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(target.join("run.sh"))
        .status()
        .unwrap();
    assert!(made.success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = verify(&store, snapshot, &target, Verify::All, &|_| true);
        let _ = tx.send(outcome);
    });
    let outcome = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("verify blocked on the FIFO");
    assert!(
        matches!(&outcome, Err(RestoreError::VerifyFailed { path }) if path.as_str() == "run.sh"),
        "{outcome:?}"
    );
}

/// Gives the owner write permission on every directory under `dir`, so the temp directory can be removed.
fn make_writable(dir: &Path) -> Fallible {
    let meta = fs::symlink_metadata(dir)?;
    if meta.is_dir() {
        fs::set_permissions(dir, fs::Permissions::from_mode(meta.mode() | 0o700))?;
        for entry in fs::read_dir(dir)? {
            make_writable(&entry?.path())?;
        }
    }
    Ok(())
}

#[test]
fn a_read_only_directory_that_holds_files_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    fs::create_dir_all(root.join("ro/sub")).unwrap();
    fs::write(root.join("ro/sub/f.txt"), "f\n").unwrap();
    fs::set_permissions(root.join("ro/sub"), fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(root.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();

    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    let outcome = restore_into(&store, snapshot, &target);
    let same = outcome.is_ok() && listing(&target).unwrap() == listing(&root).unwrap();
    make_writable(dir.path()).unwrap();
    outcome.unwrap();
    assert!(same);
}

/// Captures `plain_tree` with its harness state and publishes it.
fn plain_snapshot(dir: &Path) -> Fallible<(Store, SnapshotId)> {
    let root = dir.join("plain");
    let state = dir.join("state");
    plain_tree(&root, &state)?;
    let mut store = store_in(dir)?;
    let policy = IgnorePolicy::default();
    let mut req = request(&root, &policy, None);
    req.harness_state = Some(&state);
    let Captured { manifest, .. } = capture(&store, &req)?;
    let snapshot = store.publish(&manifest)?;
    Ok((store, snapshot))
}

fn restore_with_state(
    store: &Store,
    snapshot: SnapshotId,
    target: &Path,
    state: &Path,
) -> Result<RestoreReport, RestoreError> {
    restore(
        store,
        &RestoreRequest {
            snapshot,
            target,
            harness_state: Some(state),
            verify: Verify::All,
            durability: Durability::ProcessCrash,
            carry: &|_| true,
        },
    )
}

/// Runs the flush of a kept harness directory; a power cut is not simulated.
#[test]
fn a_power_loss_rerun_keeps_and_flushes_a_complete_harness_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (store, snapshot) = plain_snapshot(dir.path()).unwrap();
    let state = dir.path().join("state-restored");
    restore_with_state(&store, snapshot, &dir.path().join("first"), &state).unwrap();
    let report = restore(
        &store,
        &RestoreRequest {
            snapshot,
            target: &dir.path().join("second"),
            harness_state: Some(&state),
            verify: Verify::All,
            durability: Durability::PowerLoss,
            carry: &|_| true,
        },
    )
    .unwrap();
    assert_eq!(report.verified, 10);
}

#[test]
fn a_harness_directory_with_foreign_files_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, snapshot) = plain_snapshot(dir.path()).unwrap();
    let state = dir.path().join("state-restored");
    restore_with_state(&store, snapshot, &dir.path().join("first"), &state).unwrap();
    fs::write(state.join("other-session.jsonl"), "{}\n").unwrap();
    let err = restore_with_state(&store, snapshot, &dir.path().join("second"), &state).unwrap_err();
    assert!(
        matches!(&err, RestoreError::TargetNotEmpty { path } if *path == state),
        "{err}"
    );
}

#[test]
fn a_harness_directory_inside_the_target_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, snapshot) = plain_snapshot(dir.path()).unwrap();
    let target = dir.path().join("restored");
    fs::create_dir(&target).unwrap();
    let err = restore_with_state(&store, snapshot, &target, &target.join(".claude")).unwrap_err();
    assert!(
        matches!(err, RestoreError::NestedDirectories { .. }),
        "{err}"
    );
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Absent);
}

#[test]
fn a_cache_the_target_cannot_use_is_skipped_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    let report = restore(
        &store,
        &RestoreRequest {
            snapshot,
            target: &target,
            harness_state: None,
            verify: Verify::All,
            durability: Durability::ProcessCrash,
            carry: &|entry: &CacheEntry| entry.class != CacheClass::Deps,
        },
    )
    .unwrap();
    assert_eq!(
        report.skipped_caches,
        vec![RelPath::parse("node_modules").unwrap()]
    );
    assert!(!target.join("node_modules").exists());
    assert!(target.join("build.log").exists());
    assert_eq!(report.files, 1);
    assert_eq!(status(&target).unwrap(), status(&root).unwrap());
}

#[test]
fn a_partial_cache_is_skipped_even_when_the_target_could_use_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("node_modules/pkg/index.js"), "x\n").unwrap();
    fs::write(root.join("node_modules/pkg/.env"), "leak\n").unwrap();
    fs::write(root.join("keep.txt"), "keep\n").unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert_eq!(manifest.caches[0].carry, CarryState::Partial);
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    let report = restore_into(&store, snapshot, &target).unwrap();
    assert_eq!(
        report.skipped_caches,
        vec![RelPath::parse("node_modules").unwrap()]
    );
    assert!(!target.join("node_modules").exists());
    assert!(target.join("keep.txt").exists());
}

#[test]
fn a_cache_of_empty_directories_gets_a_row_the_target_can_turn_down() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
    fs::write(root.join("keep.txt"), "keep\n").unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert_eq!(manifest.caches.len(), 1);
    assert_eq!(manifest.caches[0].carry, CarryState::Carried);
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    let report = restore(
        &store,
        &RestoreRequest {
            snapshot,
            target: &target,
            harness_state: None,
            verify: Verify::All,
            durability: Durability::ProcessCrash,
            carry: &|_| false,
        },
    )
    .unwrap();
    assert_eq!(
        report.skipped_caches,
        vec![RelPath::parse("node_modules").unwrap()]
    );
    assert!(!target.join("node_modules").exists());
}

#[test]
fn restore_state_tells_absent_from_half_restored_and_present() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("t");
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Absent);
    fs::create_dir(&target).unwrap();
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Absent);
    fs::create_dir_all(dir.path().join("t.fabric-restoring/tree")).unwrap();
    assert!(matches!(
        restore_state(&target).unwrap(),
        RestoreState::HalfRestored { .. }
    ));
    fs::remove_dir_all(dir.path().join("t.fabric-restoring")).unwrap();
    fs::write(target.join("x"), "x").unwrap();
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Present);
}

/// A directory that happens to carry the staging name is refused, not deleted.
#[test]
fn a_staging_directory_restore_did_not_make_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, snapshot) = plain_snapshot(dir.path()).unwrap();
    let target = dir.path().join("t");
    let foreign = dir.path().join("t.fabric-restoring");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("mine.txt"), "not a restore's\n").unwrap();
    let err = restore_into(&store, snapshot, &target).unwrap_err();
    assert!(
        matches!(&err, RestoreError::ForeignStaging { path } if *path == foreign),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(foreign.join("mine.txt")).unwrap(),
        "not a restore's\n"
    );
    assert!(!target.exists());
}

/// The child half: runs with `XDG_CONFIG_HOME` pointing at a git ignore file of its own.
#[test]
fn a_file_only_the_global_ignore_names_child() {
    let Ok(dir) = std::env::var("FABRIC_GLOBAL_IGNORE_CHILD") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let root = dir.join("work");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a"), "a\n").unwrap();
    let g = git(&root);
    g.run(&["init", "-q", "-b", "main"]).unwrap();
    g.run(&["add", "-A"]).unwrap();
    g.run(&["commit", "-q", "-m", "one"]).unwrap();
    fs::create_dir(root.join(".claude")).unwrap();
    fs::write(root.join(".claude/settings.local.json"), "{}\n").unwrap();

    let mut store = store_in(&dir).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let files: Vec<&str> = manifest.files.iter().map(|e| e.rel_path.as_str()).collect();
    assert!(files.contains(&".claude/settings.local.json"), "{files:?}");
    let in_w = worktree_paths(&root, &manifest).unwrap();
    assert_eq!(in_w, vec!["a".to_owned()]);

    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    assert_eq!(
        fs::read_to_string(target.join(".claude/settings.local.json")).unwrap(),
        "{}\n"
    );
}

#[test]
fn a_file_only_the_global_ignore_names_restores() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(xdg.join("git")).unwrap();
    fs::write(xdg.join("git/ignore"), "**/.claude/settings.local.json\n").unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_file_only_the_global_ignore_names_child"])
        .env("FABRIC_GLOBAL_IGNORE_CHILD", dir.path())
        .env("XDG_CONFIG_HOME", &xdg)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("1 passed"), "{stdout}");
}

#[test]
fn a_fifo_or_socket_is_left_behind_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    let state = dir.path().join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(root.join("keep.txt"), "keep\n").unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(root.join("dev.sock")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(state.join("pipe"))
            .status()
            .unwrap()
            .success()
    );

    let (done, finished) = std::sync::mpsc::channel();
    let base = dir.path().to_owned();
    std::thread::spawn(move || {
        let store = store_in(&base).unwrap();
        let policy = IgnorePolicy::default();
        let mut req = request(&root, &policy, None);
        req.harness_state = Some(&state);
        let _ = done.send(capture(&store, &req).map_err(|e| e.to_string()));
    });
    let Captured { manifest, report } = finished
        .recv_timeout(Duration::from_mins(1))
        .expect("capture hung on a FIFO")
        .unwrap();
    let files: Vec<&str> = manifest.files.iter().map(|e| e.rel_path.as_str()).collect();
    assert_eq!(files, vec!["keep.txt"]);
    assert!(manifest.harness_state.entries.is_empty());
    assert_eq!(
        report.special_files,
        vec![RelPath::parse("dev.sock").unwrap()]
    );
    assert_eq!(
        report.special_harness,
        vec![RelPath::parse("pipe").unwrap()]
    );
}

/// Makes a repository at `<dir>/work` with a committed submodule at `my lib`.
fn repo_with_submodule(dir: &Path) -> Fallible<PathBuf> {
    let upstream = dir.join("upstream");
    fs::create_dir(&upstream)?;
    fs::write(upstream.join("lib.rs"), "one\n")?;
    let u = git(&upstream);
    u.run(&["init", "-q", "-b", "main"])?;
    u.run(&["add", "-A"])?;
    u.run(&["commit", "-q", "-m", "lib"])?;
    let root = dir.join("work");
    fs::create_dir(&root)?;
    let g = git(&root);
    g.run(&["init", "-q", "-b", "main"])?;
    g.run(&[
        "submodule",
        "add",
        "-q",
        upstream.to_str().ok_or("path")?,
        "my lib",
    ])?;
    g.run(&["commit", "-q", "-m", "with a submodule"])?;
    Ok(root)
}

#[test]
fn a_submodule_the_config_ignores_is_still_refused_when_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo_with_submodule(dir.path()).unwrap();
    git(&root)
        .run(&["config", "submodule.my lib.ignore", "all"])
        .unwrap();
    fs::write(root.join("my lib/lib.rs"), "changed\n").unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::GitSubmoduleDirty { path } if path.as_str() == "my lib"),
        "{err}"
    );
}

#[test]
fn a_renamed_dirty_submodule_is_refused_by_its_new_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo_with_submodule(dir.path()).unwrap();
    git(&root).run(&["mv", "my lib", "your lib"]).unwrap();
    fs::write(root.join("your lib/lib.rs"), "changed\n").unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::GitSubmoduleDirty { path } if path.as_str() == "your lib"),
        "{err}"
    );
}

#[test]
fn a_submodule_with_local_changes_is_refused_by_its_whole_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo_with_submodule(dir.path()).unwrap();
    fs::write(root.join("my lib/lib.rs"), "changed\n").unwrap();

    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::GitSubmoduleDirty { path } if path.as_str() == "my lib"),
        "{err}"
    );
}

/// Commits one small change and captures it after `parent`.
fn commit_and_capture(
    store: &mut Store,
    root: &Path,
    step: u32,
    parent: Option<SnapshotId>,
) -> Fallible<(SnapshotId, Captured)> {
    fs::write(root.join("step"), format!("{step}\n"))?;
    let g = git(root);
    g.run(&["add", "step"])?;
    g.run(&["commit", "-q", "-m", &format!("step {step}")])?;
    let policy = IgnorePolicy::default();
    let captured = capture(store, &request(root, &policy, parent))?;
    let id = store.publish(&captured.manifest)?;
    Ok((id, captured))
}

fn one_commit_repo(root: &Path) -> Fallible {
    fs::create_dir(root)?;
    fs::write(root.join("a"), "a\n")?;
    let g = git(root);
    g.run(&["init", "-q", "-b", "main"])?;
    g.run(&["add", "-A"])?;
    g.run(&["commit", "-q", "-m", "one"])?;
    Ok(())
}

#[test]
fn the_bundle_after_the_longest_allowed_chain_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let mut parent = None;
    for step in 0..=MAX_BUNDLE_CHAIN + 1 {
        let (id, captured) = commit_and_capture(&mut store, &root, step, parent).unwrap();
        let state = captured.manifest.git.as_ref().unwrap();
        let expected = if step == 0 || step == MAX_BUNDLE_CHAIN + 1 {
            BundleKind::Full
        } else {
            BundleKind::Incremental
        };
        assert_eq!(captured.report.bundle, Some(expected), "step {step}");
        assert_eq!(
            state.bundle_base.is_none(),
            expected == BundleKind::Full,
            "step {step}"
        );
        parent = Some(id);
    }
}

#[test]
fn a_parent_tip_gone_from_the_repository_makes_a_full_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let (first, _) = commit_and_capture(&mut store, &root, 0, None).unwrap();

    // Rewrite history so that no ref reaches the parent's tips, then prune them.
    let g = git(&root);
    g.run(&["checkout", "-q", "--orphan", "fresh"]).unwrap();
    g.run(&["commit", "-q", "-m", "rewritten"]).unwrap();
    g.run(&["branch", "-q", "-D", "main"]).unwrap();
    g.run(&["branch", "-q", "-m", "main"]).unwrap();
    let snap_refs = g
        .run_text(&["for-each-ref", "--format=%(refname)", "refs/fabric/"])
        .unwrap();
    for name in snap_refs.lines() {
        g.run(&["update-ref", "-d", name]).unwrap();
    }
    g.run(&["reflog", "expire", "--expire=now", "--all"])
        .unwrap();
    g.run(&["gc", "-q", "--prune=now"]).unwrap();

    let (second, captured) = commit_and_capture(&mut store, &root, 1, Some(first)).unwrap();
    assert_eq!(captured.report.bundle, Some(BundleKind::Full));
    assert_eq!(captured.manifest.git.as_ref().unwrap().bundle_base, None);
    let target = dir.path().join("restored");
    let report = restore_into(&store, second, &target).unwrap();
    assert_eq!(report.bundles_fetched, 1);
    assert_eq!(git_difference(&root, &target).unwrap(), None);
}

#[test]
fn a_parent_from_another_job_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let (first, _) = commit_and_capture(&mut store, &root, 0, None).unwrap();
    let policy = IgnorePolicy::default();
    let mut other = request(&root, &policy, Some(first));
    other.job_id = JobId::from_bytes([6; 16]);
    let err = capture(&store, &other).unwrap_err();
    assert!(
        matches!(err, CaptureError::ParentJobMismatch { parent, job } if parent == first && job == JOB),
        "{err}"
    );
}

#[test]
fn a_restore_whose_bundle_base_is_gone_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let (first, _) = commit_and_capture(&mut store, &root, 0, None).unwrap();
    let (second, _) = commit_and_capture(&mut store, &root, 1, Some(first)).unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(dir.path().join("store/catalog.sqlite")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute("DELETE FROM snapshots WHERE id = ?1", [first.to_bytes()])
        .unwrap();
    drop(conn);

    let store = Store::open(&dir.path().join("store")).unwrap();
    let err = restore_into(&store, second, &dir.path().join("restored")).unwrap_err();
    assert!(
        matches!(err, RestoreError::BundleChainBroken { snapshot, base }
            if snapshot == second && base == first),
        "{err}"
    );
}

#[test]
fn names_git_would_unquote_or_trim_verify_after_restore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    fs::write(root.join("\"q"), "quoted\n").unwrap();
    fs::write(root.join("x\r"), "carriage return\n").unwrap();
    let g = git(&root);
    g.run(&["add", "-A"]).unwrap();
    g.run(&["commit", "-q", "-m", "odd names"]).unwrap();
    fs::write(root.join("Icon\r"), "").unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let report = restore_into(&store, snapshot, &dir.path().join("restored")).unwrap();
    assert_eq!(report.verified, 4);
}

#[test]
fn a_secret_saved_in_the_stash_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    fs::write(root.join(".env"), "AWS_SECRET=hunter2\n").unwrap();
    git(&root)
        .run(&["stash", "push", "-q", "--include-untracked"])
        .unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { report, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert_eq!(
        report.stashed_secrets,
        vec![RelPath::parse(".env").unwrap()]
    );
}

/// A file whose bytes git converts on check-in comes back with the bytes it had, not the converted blob.
#[test]
fn a_file_git_converts_on_check_in_restores_with_its_own_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir(&root).unwrap();
    let g = git(&root);
    g.run(&["init", "-q", "-b", "main"]).unwrap();
    fs::write(
        root.join(".gitattributes"),
        "*.txt text eol=crlf\n*.id ident\n",
    )
    .unwrap();
    fs::write(root.join("lines.txt"), "one\r\ntwo\r\n").unwrap();
    fs::write(root.join("stamp.id"), "$Id$\n").unwrap();
    g.run(&["add", "-A"]).unwrap();
    g.run(&["commit", "-q", "-m", "one"]).unwrap();
    // Checkout expands `$Id$`; the blob keeps it collapsed.
    fs::remove_file(root.join("stamp.id")).unwrap();
    g.run(&["checkout", "--", "stamp.id"]).unwrap();
    fs::write(root.join("new.txt"), "untracked\r\n").unwrap();
    let expanded = fs::read(root.join("stamp.id")).unwrap();
    assert_ne!(expanded, b"$Id$\n");

    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert!(report.filtered.is_empty(), "{:?}", report.filtered);
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    for name in ["lines.txt", "stamp.id", "new.txt"] {
        assert_eq!(
            fs::read(target.join(name)).unwrap(),
            fs::read(root.join(name)).unwrap(),
            "{name}"
        );
    }
    assert_eq!(git_difference(&root, &target).unwrap(), None);
}

/// Every stash entry comes back at its index, and a secret in an older one is named.
#[test]
fn every_stash_entry_survives_capture_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let g = git(&root);
    fs::write(root.join(".env"), "AWS_SECRET=hunter2\n").unwrap();
    g.run(&["stash", "push", "-q", "--include-untracked", "-m", "oldest"])
        .unwrap();
    for message in ["middle", "newest"] {
        fs::write(root.join("a"), format!("{message}\n")).unwrap();
        g.run(&["stash", "push", "-q", "-m", message]).unwrap();
    }
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    assert_eq!(
        report.stashed_secrets,
        vec![RelPath::parse(".env").unwrap()]
    );
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();

    let list = git_text(&target, &["stash", "list"]).unwrap();
    assert_eq!(list, git_text(&root, &["stash", "list"]).unwrap());
    assert_eq!(list.lines().count(), 3, "{list}");
    for n in 0..3 {
        let entry = format!("stash@{{{n}}}");
        assert_eq!(
            git_text(&target, &["rev-parse", &entry]).unwrap(),
            git_text(&root, &["rev-parse", &entry]).unwrap(),
            "{entry}"
        );
    }
}

/// Captures a branch named `before`, replaces it with `after` and restores the incremental snapshot.
fn rename_branch_across_an_incremental_link(before: &str, after: &str) -> Fallible {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("work");
    one_commit_repo(&root)?;
    let g = git(&root);
    g.run(&["branch", before])?;
    let mut store = store_in(dir.path())?;
    let policy = IgnorePolicy::default();
    let first = capture(&store, &request(&root, &policy, None))?;
    let s1 = store.publish(&first.manifest)?;

    g.run(&["branch", "-D", before])?;
    g.run(&["commit", "-q", "--allow-empty", "-m", "two"])?;
    g.run(&["branch", after])?;
    let second = capture(&store, &request(&root, &policy, Some(s1)))?;
    assert_eq!(second.report.bundle, Some(BundleKind::Incremental));
    let s2 = store.publish(&second.manifest)?;

    let target = dir.path().join("restored");
    restore_into(&store, s2, &target)?;
    assert_eq!(git_difference(&root, &target)?, None);
    Ok(())
}

#[test]
fn a_branch_that_becomes_a_directory_of_branches_restores() {
    rename_branch_across_an_incremental_link("feature", "feature/x").unwrap();
}

#[test]
fn a_directory_of_branches_that_becomes_a_branch_restores() {
    rename_branch_across_an_incremental_link("feature/x", "feature").unwrap();
}

#[test]
fn the_target_keeps_the_filesystem_settings_git_init_probed() {
    let dir = tempfile::tempdir().unwrap();
    let probe = dir.path().join("probe");
    fs::create_dir(&probe).unwrap();
    git(&probe).run(&["init", "-q"]).unwrap();
    let probed = git_text(&probe, &["config", "--bool", "core.ignorecase"]).unwrap_or_default();
    let opposite = if probed == "true" { "false" } else { "true" };

    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    git(&root)
        .run(&["config", "core.ignorecase", opposite])
        .unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    let restored = git_text(&target, &["config", "--bool", "core.ignorecase"]).unwrap_or_default();
    assert_eq!(restored, probed);
}

#[test]
fn a_remote_url_with_a_query_token_is_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    git(&root)
        .run(&[
            "remote",
            "add",
            "origin",
            "https://example.com/x.git?access_token=sekrit",
        ])
        .unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    let key = GitConfigKey::parse("remote.origin.url").unwrap();
    assert_eq!(report.dropped_config, vec![key]);
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    let config = fs::read_to_string(target.join(".git/config")).unwrap();
    assert!(!config.contains("sekrit"), "{config}");
}

#[test]
fn a_credential_in_a_branch_remote_url_is_stripped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    git(&root)
        .run(&[
            "config",
            "branch.main.remote",
            "https://token@example.com/x.git",
        ])
        .unwrap();
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    let key = GitConfigKey::parse("branch.main.remote").unwrap();
    assert_eq!(report.stripped_userinfo, vec![key]);
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    assert_eq!(
        git_text(&target, &["config", "branch.main.remote"]).unwrap(),
        "https://example.com/x.git"
    );
}

/// git holds a key more than once after `git config --add`, as for a
/// second fetch refspec that maps pull requests or a second remote URL for
/// a mirror. Every value travels in git's order, which decides the URL git
/// fetches from.
#[test]
fn every_value_of_a_repeated_config_key_travels_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let g = git(&root);
    g.run(&["remote", "add", "origin", "https://example.com/a.git"])
        .unwrap();
    for (key, value) in [
        (
            "remote.origin.fetch",
            "+refs/pull/*/head:refs/remotes/origin/pr/*",
        ),
        ("remote.origin.url", "https://example.com/b.git"),
    ] {
        g.run(&["config", "--add", key, value]).unwrap();
    }
    let mut store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, .. } = capture(&store, &request(&root, &policy, None)).unwrap();
    let snapshot = store.publish(&manifest).unwrap();
    let target = dir.path().join("restored");
    restore_into(&store, snapshot, &target).unwrap();
    assert_eq!(
        git_text(&target, &["config", "--get-all", "remote.origin.fetch"]).unwrap(),
        "+refs/heads/*:refs/remotes/origin/*\n+refs/pull/*/head:refs/remotes/origin/pr/*"
    );
    assert_eq!(
        git_text(&target, &["config", "--get-all", "remote.origin.url"]).unwrap(),
        "https://example.com/a.git\nhttps://example.com/b.git"
    );
}

#[test]
fn a_config_value_the_projection_refuses_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    one_commit_repo(&root).unwrap();
    let config = git(&root).git_path("config").unwrap();
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[remote \"odd\"]\n\turl = -oProxyCommand=x\n");
    fs::write(&config, text).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();
    let Captured { manifest, report } = capture(&store, &request(&root, &policy, None)).unwrap();
    let key = GitConfigKey::parse("remote.odd.url").unwrap();
    assert_eq!(report.dropped_config, vec![key.clone()]);
    assert!(
        !manifest
            .git
            .unwrap()
            .config
            .iter()
            .any(|(carried, _)| *carried == key)
    );
}
