//! Push and pull through the real binary, between two homes on this
//! machine, to a directory target and over SSH through a stand-in `ssh`
//! that runs the remote command here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fabric_core::id::ChunkId;
use fabric_core::snapshot::CacheClass;
use fabric_store::store::Store;

const BIN: &str = env!("CARGO_BIN_EXE_fabric-sync");
const SECRET: &str = "sk-live-0123456789abcdef-never-travels";

struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let dir = tempfile::Builder::new().prefix("fsync").tempdir().unwrap();
        for sub in ["a", "b", "home-a", "home-b", "home", "bin"] {
            fs::create_dir(dir.path().join(sub)).unwrap();
        }
        // A stand-in for ssh: skips its options up to `--`, drops the host
        // and runs the remote command with this machine's shell.
        let ssh = dir.path().join("bin/ssh");
        fs::write(
            &ssh,
            "#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift 2\nexec /bin/sh -c \"$*\"\n",
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        World { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    /// `fabric-sync args...` as the machine whose home is `home`.
    fn run(&self, home: &str, args: &[&str], fault: Option<&str>) -> Output {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("FABRIC_SYNC_HOME", self.path(home))
            .env("HOME", self.path("home"))
            .env("FABRIC_SYNC_SSH", self.path("bin/ssh"))
            .env("FABRIC_SYNC_REMOTE", BIN)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("FABRIC_SYNC_FAULT");
        if let Some(fault) = fault {
            command.env("FABRIC_SYNC_FAULT", fault);
        }
        command.output().unwrap()
    }

    /// Runs and expects success; returns the JSON answer.
    fn ok(&self, home: &str, args: &[&str]) -> serde_json::Value {
        let mut args = args.to_vec();
        args.push("--json");
        let output = self.run(home, &args, None);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stderr, b"", "{args:?} wrote to stderr");
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "1760000000 +0000")
        .env("GIT_COMMITTER_DATE", "1760000000 +0000")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// `len` characters that do not compress, the same for the same seed.
fn noise(seed: usize, len: usize) -> String {
    let mut state = (seed as u64).wrapping_mul(6_364_136_223_846_793_005);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(b'a' + u8::try_from(state >> 59).unwrap() % 26)
        })
        .collect()
}

/// A repository with every kind of uncommitted work, a `node_modules` and secrets.
fn dirty_tree(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    write(&root.join(".gitignore"), "node_modules/\n.env.local\n");
    write(&root.join("package.json"), "{\"name\":\"app\"}\n");
    write(&root.join("src/index.js"), "console.log(1);\n");
    write(&root.join("src/staged.js"), "one\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "one"]);
    write(&root.join("src/staged.js"), "two, staged\n");
    git(root, &["add", "src/staged.js"]);
    write(&root.join("src/index.js"), "console.log(2); // unstaged\n");
    write(&root.join("notes.txt"), "untracked work\n");
    write(&root.join(".env"), &format!("KEY={SECRET}\n"));
    write(&root.join(".env.local"), &format!("KEY={SECRET}\n"));
    for pkg in 0..40 {
        let dir = root.join(format!("node_modules/pkg{pkg}"));
        write(
            &dir.join("package.json"),
            &format!("{{\"name\":\"pkg{pkg}\"}}\n"),
        );
        write(&dir.join("index.js"), &noise(pkg, 16 << 10));
    }
    let bin = root.join("node_modules/.bin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink("../pkg1/index.js", bin.join("pkg1")).unwrap();
    fs::set_permissions(
        root.join("node_modules/pkg1/index.js"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
}

/// Every path under `root` but `.git` and the secrets, with its kind,
/// mode, mtime and bytes or link target.
fn listing(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if rel == ".git" {
                continue;
            }
            let meta = fs::symlink_metadata(&path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            let value = if meta.file_type().is_symlink() {
                format!("link {}", fs::read_link(&path).unwrap().display())
            } else if meta.is_dir() {
                walk(root, &path, out);
                format!("dir {mode:o}")
            } else {
                format!(
                    "file {mode:o} {} {:?}",
                    meta.mtime_nsec() + meta.mtime() * 1_000_000_000,
                    fs::read(&path).unwrap()
                )
            };
            out.insert(rel, value);
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The tree as git sees it: the head, the index and the work tree against it.
fn git_view(root: &Path) -> (String, String, String) {
    (
        git(root, &["rev-parse", "HEAD"]),
        git(root, &["diff", "--cached"]),
        git(root, &["status", "--porcelain=v2", "--untracked-files=all"]),
    )
}

/// Every object of the store under `target`, decompressed.
fn objects(target: &Path) -> Vec<Vec<u8>> {
    let store = Store::open(&target.join("store")).unwrap();
    let mut out = Vec::new();
    for shard in fs::read_dir(target.join("store/objects")).unwrap() {
        let shard = shard.unwrap();
        for object in fs::read_dir(shard.path()).unwrap() {
            let name = object.unwrap().file_name().to_string_lossy().into_owned();
            let id: ChunkId = name.parse().unwrap();
            out.push(store.objects().read(id).unwrap());
        }
    }
    out
}

fn same_tree(a: &Path, b: &Path) {
    let (left, right) = (listing(a), listing(b));
    let differ: Vec<&String> = left
        .keys()
        .chain(right.keys())
        .filter(|k| left.get(*k) != right.get(*k))
        .collect();
    assert!(differ.is_empty(), "the trees differ at {differ:?}");
    assert_eq!(git_view(a), git_view(b));
}

fn without_secrets(root: &Path) -> PathBuf {
    let copy = root.with_extension("expected");
    let status = Command::new("cp")
        .arg("-Rp")
        .arg(root)
        .arg(&copy)
        .status()
        .unwrap();
    assert!(status.success());
    fs::remove_file(copy.join(".env")).unwrap();
    fs::remove_file(copy.join(".env.local")).unwrap();
    copy
}

fn round_trip(target: &str) {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    let target = target.replace("{}", world.path("target").to_str().unwrap());
    let pushed = world.ok("home-a", &["push", &target, "--tree", a.to_str().unwrap()]);
    assert_eq!(pushed["outcome"], "pushed");
    let b = world.path("b/app");
    let pulled = world.ok("home-b", &["pull", &target, "--tree", b.to_str().unwrap()]);
    assert_eq!(pulled["outcome"], "pulled");
    assert_eq!(pulled["sync"]["snapshot"], pushed["sync"]["snapshot"]);
    same_tree(&without_secrets(&a), &b);
}

#[test]
fn a_dirty_tree_with_node_modules_round_trips_byte_identical_through_a_directory() {
    round_trip("{}");
}

#[test]
fn a_dirty_tree_with_node_modules_round_trips_byte_identical_over_ssh() {
    round_trip("box:{}");
}

#[test]
fn a_dotenv_never_travels() {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    write(
        &a.join("node_modules/pkg2/.env"),
        &format!("KEY={SECRET}\n"),
    );
    let target = world.path("target");
    let pushed = world.ok(
        "home-a",
        &[
            "push",
            target.to_str().unwrap(),
            "--tree",
            a.to_str().unwrap(),
        ],
    );
    let left: Vec<&str> = pushed["sync"]["left_behind"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for path in [".env", ".env.local", "node_modules/pkg2/.env"] {
        assert!(
            left.contains(&path),
            "{path} is not named left behind: {left:?}"
        );
    }
    for object in objects(&target) {
        assert!(
            !object.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "the secret is in an object on the target"
        );
    }
    let b = world.path("b/app");
    world.ok(
        "home-b",
        &[
            "pull",
            target.to_str().unwrap(),
            "--tree",
            b.to_str().unwrap(),
        ],
    );
    for path in [".env", ".env.local", "node_modules/pkg2/.env"] {
        assert!(!b.join(path).exists(), "{path} reached the other tree");
    }
}

#[test]
fn a_second_sync_moves_only_the_delta() {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    let target = world.path("target");
    let (t, ta) = (target.to_str().unwrap(), a.to_str().unwrap());
    let first = world.ok("home-a", &["push", t, "--tree", ta]);
    let b = world.path("b/app");
    let tb = b.to_str().unwrap();
    let first_pull = world.ok("home-b", &["pull", t, "--tree", tb]);

    write(
        &a.join("src/index.js"),
        "console.log(3); // a second edit\n",
    );
    let second = world.ok("home-a", &["push", t, "--tree", ta]);
    let second_pull = world.ok("home-b", &["pull", t, "--tree", tb, "--replace"]);
    for (one, two) in [(&first, &second), (&first_pull, &second_pull)] {
        let (one, two) = (&one["sync"], &two["sync"]);
        let sent = two["objects_sent"].as_u64().unwrap();
        assert!(
            sent > 0 && sent <= 6,
            "the second sync sent {sent} objects: {two}"
        );
        assert!(
            two["bytes_sent"].as_u64().unwrap() * 10 < one["bytes_sent"].as_u64().unwrap(),
            "the second sync sent {} bytes, the first {}",
            two["bytes_sent"],
            one["bytes_sent"]
        );
        assert!(two["hit_ratio"].as_f64().unwrap() > 0.9, "{two}");
    }
    same_tree(&without_secrets(&a), &b);
    assert!(
        b.with_extension("fabric-sync-previous").is_dir(),
        "the old tree is not kept"
    );
    let again = world.ok("home-b", &["pull", t, "--tree", tb]);
    assert_eq!(again["outcome"], "unchanged");
}

/// A pull killed at `fault`: the tree path holds the old tree or nothing,
/// never part of a tree, and the next pull finishes with the new one.
fn interrupted(fault: &str, replace: bool) {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    let target = world.path("target");
    let (t, ta) = (target.to_str().unwrap(), a.to_str().unwrap());
    let b = world.path("b/app");
    let tb = b.to_str().unwrap();
    world.ok("home-a", &["push", t, "--tree", ta]);
    let mut args = vec!["pull", t, "--tree", tb];
    if replace {
        world.ok("home-b", &args);
        write(&a.join("src/index.js"), "console.log(4);\n");
        for pkg in 40..60 {
            write(
                &a.join(format!("node_modules/pkg{pkg}/index.js")),
                &"x\n".repeat(pkg * 50),
            );
        }
        world.ok("home-a", &["push", t, "--tree", ta]);
        args.push("--replace");
    }
    let old = replace.then(|| listing(&b));
    let output = world.run("home-b", &args, Some(fault));
    assert!(!output.status.success(), "{fault}: the pull did not stop");
    assert_eq!(
        output.status.code(),
        None,
        "{fault}: the pull exited instead of dying"
    );
    match &old {
        Some(old) if b.exists() => assert_eq!(&listing(&b), old, "{fault}: the old tree changed"),
        _ => assert!(
            !b.exists() || listing(&b).is_empty(),
            "{fault}: part of a tree at the path"
        ),
    }
    let next = world.ok("home-b", &args);
    let finished = if fault == "pull.mid_swap" {
        "unchanged"
    } else {
        "pulled"
    };
    assert_eq!(next["outcome"], finished, "{fault}: {next}");
    same_tree(&without_secrets(&a), &b);
    let beside: Vec<String> = fs::read_dir(world.path("b"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        beside
            .iter()
            .all(|name| name == "app" || name == "app.fabric-sync-previous"),
        "{fault}: left beside the tree: {beside:?}"
    );
}

#[test]
fn an_interrupted_first_pull_leaves_no_half_tree() {
    interrupted("copy.mid_objects", false);
}

#[test]
fn a_pull_interrupted_while_copying_leaves_the_old_tree() {
    interrupted("copy.mid_objects", true);
}

#[test]
fn a_pull_interrupted_before_the_swap_leaves_the_old_tree() {
    interrupted("pull.before_swap", true);
}

#[test]
fn a_pull_interrupted_between_the_renames_is_finished_by_the_next() {
    interrupted("pull.mid_swap", true);
}

#[test]
fn a_push_over_a_head_this_tree_has_not_pulled_is_refused() {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    let target = world.path("target");
    let (t, ta) = (target.to_str().unwrap(), a.to_str().unwrap());
    world.ok("home-a", &["push", t, "--tree", ta]);
    let b = world.path("b/app");
    let tb = b.to_str().unwrap();
    world.ok("home-b", &["pull", t, "--tree", tb]);
    write(&b.join("notes.txt"), "from b\n");
    world.ok("home-b", &["push", t, "--tree", tb]);

    let output = world.run("home-a", &["push", t, "--tree", ta, "--json"], None);
    assert_eq!(output.status.code(), Some(84));
    assert_eq!(output.stdout, b"");
    let doc: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(doc["reason"], "target_moved", "{doc}");
    assert_eq!(doc["status"], "action_required", "{doc}");
    assert!(
        doc["next"][0]["argv"]
            .as_array()
            .unwrap()
            .contains(&"--force".into())
    );

    world.ok("home-a", &["pull", t, "--tree", ta, "--replace"]);
    assert_eq!(fs::read_to_string(a.join("notes.txt")).unwrap(), "from b\n");
    world.ok("home-a", &["push", t, "--tree", ta]);
}

#[test]
fn a_pull_onto_a_tree_with_content_needs_replace() {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    let target = world.path("target");
    let (t, ta) = (target.to_str().unwrap(), a.to_str().unwrap());
    world.ok("home-a", &["push", t, "--tree", ta]);
    let b = world.path("b/app");
    write(&b.join("mine.txt"), "mine\n");
    let output = world.run("home-b", &["pull", t, "--tree", b.to_str().unwrap()], None);
    assert_eq!(output.status.code(), Some(84));
    assert_eq!(output.stdout, b"");
    let err = String::from_utf8(output.stderr).unwrap();
    assert!(err.starts_with("Error: "), "{err}");
    assert!(err.contains("--replace"), "{err}");
    assert_eq!(fs::read_to_string(b.join("mine.txt")).unwrap(), "mine\n");
}

#[test]
fn the_secret_patterns_and_caches_a_tree_declares_hold_on_a_push() {
    let world = World::new();
    let a = world.path("a/app");
    dirty_tree(&a);
    write(
        &a.join(".fabric/env.toml"),
        "secret_patterns = [\"config/creds.json\"]\n\n[[cache]]\npath = \"data\"\nclass = \"build\"\n",
    );
    write(&a.join(".gitignore"), "node_modules/\n.env.local\ndata/\n");
    write(
        &a.join("config/creds.json"),
        &format!("{{\"key\":\"{SECRET}\"}}\n"),
    );
    write(&a.join("data/out.bin"), "built\n");
    let target = world.path("target");
    let pushed = world.ok(
        "home-a",
        &[
            "push",
            target.to_str().unwrap(),
            "--tree",
            a.to_str().unwrap(),
        ],
    );
    let left = pushed["sync"]["left_behind"].as_array().unwrap();
    assert!(left.contains(&"config/creds.json".into()), "{left:?}");
    for object in objects(&target) {
        assert!(
            !object.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "the declared secret is in an object on the target"
        );
    }
    let store = Store::open(&target.join("store")).unwrap();
    let id = pushed["sync"]["snapshot"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let caches = store.manifest(id).unwrap().caches;
    assert!(
        caches
            .iter()
            .any(|c| c.path.as_str() == "data" && c.class == CacheClass::Build),
        "the declared cache lost its class: {caches:?}"
    );
}
