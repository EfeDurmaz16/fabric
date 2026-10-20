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

use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::path::RelPath;
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, Entry, EntryKind, GitHead, Mode,
};
use fabric_store::capture::{
    BundleKind, CaptureError, CaptureRequest, Captured, UnmeasuredKeys, capture,
};
use fabric_store::git::Git;
use fabric_store::objects::ChunkParams;
use fabric_store::policy::{IgnorePolicy, PolicyError, SECRET_PATTERNS};
use fabric_store::restore::{
    Durability, RestoreError, RestoreReport, RestoreRequest, RestoreState, Verify, restore,
    restore_state, verify,
};
use fabric_store::store::{CheckDepth, Store, StoreError};
use fabric_store::writer::WriteError;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

const JOB: JobId = JobId::from_bytes([5; 16]);
const NODE: NodeId = NodeId::from_bytes([2; 32]);
const T0: u64 = 1_760_000_000_000;

fn git(root: &Path) -> Git {
    Git::new(root, T0)
}

fn set_mtime(path: &Path, secs: u64, nanos: u32) -> Fallible {
    let time = UNIX_EPOCH + Duration::new(secs, nanos);
    File::open(path)?.set_modified(time)?;
    Ok(())
}

fn store_in(dir: &Path) -> Fallible<Store> {
    Ok(Store::create(
        &dir.join("store"),
        ChunkParams {
            whole_file_limit: 64 << 10,
            min_size: 4 << 10,
            avg_size: 16 << 10,
            max_size: 64 << 10,
            zstd_level: 3,
        },
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
        max_file_size: None,
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
    Ok(git(root).run_text(args)?)
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

    assert_eq!(report.bundle, BundleKind::Full);
    assert_eq!(report.denied, Vec::<RelPath>::new());
    assert_eq!(
        report.denied_untracked,
        vec![RelPath::parse(".env").unwrap()]
    );
    assert_eq!(report.stripped_userinfo, vec!["remote.origin.url"]);
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
            key: CarryKey::Other {
                tool: "unmeasured".to_owned(),
                version: String::new(),
            },
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
    // Three ignored files plus the nine entries of W.
    assert_eq!(restored.verified, 12);
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
        12
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
    assert_eq!(second.report.bundle, BundleKind::Incremental);
    assert_eq!(second.manifest.git.as_ref().unwrap().bundle_base, Some(s1));
    assert_eq!(second.manifest.parent, Some(s1));
    // The new bundle, the changed log and nothing from node_modules.
    assert!(
        second.report.new_objects <= 3,
        "{}",
        second.report.new_objects
    );
    let s2 = store.publish(&second.manifest).unwrap();

    let third = capture(&store, &request(&root, &policy, Some(s2))).unwrap();
    assert_eq!(third.report.bundle, BundleKind::Empty);
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
    assert!(!root.join(".git/fabric-tmp").exists());
    assert!(!target.join(".git/fabric-tmp").exists());
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
    assert_eq!(report.bundle, BundleKind::NoGit);

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
        Err(PolicyError::BadPattern { .. })
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
    assert_eq!(report.nested_repositories, vec!["nested".to_owned()]);
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

    let mut through_link = manifest.clone();
    through_link.files = vec![entry("escape/evil")];
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

    let mut over_file = manifest.clone();
    over_file.files = vec![entry("tracked.txt")];
    let id = store.publish(&over_file).unwrap();
    let err = restore_into(&store, id, &dir.path().join("t2")).unwrap_err();
    assert!(
        matches!(err, RestoreError::Write(WriteError::Collision { .. })),
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
fn the_size_cap_and_the_large_untracked_guard_name_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();

    let mut capped = request(&root, &policy, None);
    capped.max_file_size = Some(10);
    let err = capture(&store, &capped).unwrap_err();
    assert!(
        matches!(&err, CaptureError::FileTooLarge { path, cap: 10, .. }
            if path.as_str() == "build.log" || path.as_str().starts_with("node_modules/")),
        "{err}"
    );

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

#[test]
fn busy_repositories_and_linked_worktrees_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    make_repo(&root).unwrap();
    let store = store_in(dir.path()).unwrap();
    let policy = IgnorePolicy::default();

    let merge_head = git(&root).git_path("MERGE_HEAD").unwrap();
    fs::write(&merge_head, "0000000000000000000000000000000000000000\n").unwrap();
    let err = capture(&store, &request(&root, &policy, None)).unwrap_err();
    assert!(
        matches!(&err, CaptureError::GitBusy { reason } if reason.contains("MERGE_HEAD")),
        "{err}"
    );
    fs::remove_file(&merge_head).unwrap();

    let linked = dir.path().join("linked");
    git(&root)
        .run(&["worktree", "add", "-q", linked.to_str().unwrap(), "feature"])
        .unwrap();
    let err = capture(&store, &request(&linked, &policy, None)).unwrap_err();
    assert!(
        matches!(err, CaptureError::GitLinkedWorktree { .. }),
        "{err}"
    );

    let sub = root.join("d");
    let err = capture(&store, &request(&sub, &policy, None)).unwrap_err();
    assert!(
        matches!(err, CaptureError::NotRepositoryRoot { .. }),
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
    assert_eq!(
        verify(&store, snapshot, &target, Verify::All, &|_| true).unwrap(),
        2
    );
    fs::write(target.join("a/b/deep.txt"), "tampered\n").unwrap();
    assert!(matches!(
        verify(&store, snapshot, &target, Verify::All, &|_| true),
        Err(RestoreError::VerifyFailed { path }) if path.as_str() == "a/b/deep.txt"
    ));
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
fn restore_state_tells_absent_from_half_restored_and_present() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("t");
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Absent);
    fs::create_dir(&target).unwrap();
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Absent);
    fs::create_dir(dir.path().join("t.fabric-restoring")).unwrap();
    assert!(matches!(
        restore_state(&target).unwrap(),
        RestoreState::HalfRestored { .. }
    ));
    fs::remove_dir(dir.path().join("t.fabric-restoring")).unwrap();
    fs::write(target.join("x"), "x").unwrap();
    assert_eq!(restore_state(&target).unwrap(), RestoreState::Present);
}
