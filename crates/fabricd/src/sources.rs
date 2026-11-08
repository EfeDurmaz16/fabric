//! Source bindings on the node that runs an attempt (ADR 0025): a git
//! repository cloned at its exact revision, a snapshot restored through
//! the existing restore path, an empty workspace, and the import of a
//! local directory into a snapshot through the existing capture path.
//!
//! Invariant: a git clone names a full commit, never a ref, and runs with
//! no credential helper and no terminal prompt, so what the node reads is
//! what the binding names or nothing. An import reads the directory and
//! writes nothing in it. Adoption of an active checkout as a managed
//! workspace is refused with its reason: `Repository { path }` is the
//! local socket's M2 form and names a tree this node never adopts.

use std::path::{Path, PathBuf};

use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{CommitId, SourceBinding, SourceError};
use fabric_core::path::{AbsPath, RelPath};
use fabric_store::capture::{CaptureRequest, UnmeasuredKeys, capture};
use fabric_store::policy::IgnorePolicy;
use fabric_store::restore::{Durability, RestoreRequest, Verify, restore};
use fabric_store::store::Store;
use ulid::Ulid;

use crate::clock::now_ms;
use crate::node::Node;
use crate::transfer::with_store;

/// What `prepare` left at the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// A clone checked out at the commit.
    Cloned {
        /// The root.
        root: AbsPath,
        /// The commit at `HEAD`.
        commit: CommitId,
    },
    /// A snapshot restored fresh.
    Restored {
        /// The root.
        root: AbsPath,
        /// The snapshot.
        snapshot: SnapshotId,
    },
    /// An empty directory.
    Empty {
        /// The root.
        root: AbsPath,
    },
}

fn unavailable(reason: impl std::fmt::Display) -> SourceError {
    SourceError::Unavailable {
        reason: reason.to_string(),
    }
}

fn abs(path: &Path) -> Result<AbsPath, SourceError> {
    path.to_str()
        .and_then(|text| AbsPath::parse(text).ok())
        .ok_or_else(|| unavailable(format!("{} is not an absolute UTF-8 path", path.display())))
}

/// A git command with no credential helper, no prompt and none of the
/// caller's `GIT_*` environment, so a clone reads what the URI names or fails.
/// Auto maintenance is off: since git 2.46 a fetch or commit detaches a
/// `git maintenance run` that goes on writing under `.git` after the
/// command returned, a process in the workspace that no task of the job
/// owns.
fn git(cwd: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd.current_dir(cwd)
        .args(["-c", "credential.helper="])
        .args(["-c", "core.askPass="])
        .args(["-c", "maintenance.auto=false"])
        .args(["-c", "gc.auto=0"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ASKPASS", "/bin/false")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null());
    cmd
}

fn run(cmd: &mut std::process::Command, what: &str) -> Result<String, SourceError> {
    let output = cmd
        .output()
        .map_err(|e| unavailable(format!("{what}: {e}")))?;
    if !output.status.success() {
        return Err(unavailable(format!(
            "{what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Makes `root` as an absent or empty directory.
fn fresh_root(root: &Path) -> Result<(), SourceError> {
    match std::fs::read_dir(root) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(unavailable(format!("{} is not empty", root.display())));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir_all(root)
            .map_err(|e| unavailable(format!("{}: {e}", root.display()))),
        Err(e) => Err(unavailable(format!("{}: {e}", root.display()))),
    }
}

/// Prepares `source` at `root` on this node; `store` is needed for a
/// snapshot. Blocking: the clone and the restore run as long as they run.
pub fn prepare_in(
    root: &Path,
    source: &SourceBinding,
    store: Option<&Store>,
) -> Result<Prepared, SourceError> {
    prepare_at(root, source, store, None)
}

/// As [`prepare_in`]; a snapshot's harness state, when it holds one, is
/// written to `harness_state` too, an absent or empty directory.
fn prepare_at(
    root: &Path,
    source: &SourceBinding,
    store: Option<&Store>,
    harness_state: Option<&Path>,
) -> Result<Prepared, SourceError> {
    let root_abs = abs(root)?;
    match source {
        SourceBinding::Repository { .. } => Err(SourceError::Adoption),
        SourceBinding::Empty => {
            fresh_root(root)?;
            Ok(Prepared::Empty { root: root_abs })
        }
        SourceBinding::Git { uri, commit } => {
            fresh_root(root)?;
            let parent = root.parent().unwrap_or(root);
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| unavailable("the root has no name"))?;
            // `--` ends the options: a URI never reads as one.
            run(
                git(parent).args([
                    "clone",
                    "--quiet",
                    "--no-checkout",
                    "--",
                    uri.as_str(),
                    name,
                ]),
                "git clone",
            )?;
            run(
                git(root).args(["checkout", "--quiet", "--detach", commit.as_str(), "--"]),
                "git checkout",
            )?;
            let head = run(git(root).args(["rev-parse", "HEAD"]), "git rev-parse")?;
            if head != commit.as_str() {
                return Err(unavailable(format!("HEAD is {head}, not the commit named")));
            }
            Ok(Prepared::Cloned {
                root: root_abs,
                commit: commit.clone(),
            })
        }
        SourceBinding::Snapshot { snapshot } => {
            let store =
                store.ok_or_else(|| unavailable("no store to restore the snapshot from"))?;
            if root.exists() {
                fresh_root(root)?;
                let _ = std::fs::remove_dir(root);
            } else if let Some(parent) = root.parent() {
                // The restore stages beside the root, so on a node that
                // never held the job its parents are made first.
                std::fs::create_dir_all(parent)
                    .map_err(|e| unavailable(format!("{}: {e}", parent.display())))?;
            }
            restore(
                store,
                &RestoreRequest {
                    snapshot: *snapshot,
                    target: root,
                    harness_state,
                    verify: Verify::None,
                    durability: Durability::ProcessCrash,
                    carry: &|_| true,
                },
            )
            .map_err(unavailable)?;
            Ok(Prepared::Restored {
                root: root_abs,
                snapshot: *snapshot,
            })
        }
    }
}

/// Captures `directory` into a snapshot through the existing capture
/// path, as `node` made it, without touching the directory. `allowlist`
/// names the paths that may leave although a secret pattern matches them.
pub fn import_from(
    store: &mut Store,
    node: NodeId,
    directory: &Path,
    allowlist: &[RelPath],
) -> Result<SnapshotId, SourceError> {
    if !directory.is_dir() {
        return Err(unavailable(format!(
            "{} is not a directory",
            directory.display()
        )));
    }
    let mut policy = IgnorePolicy::default();
    policy.load_fabricignore(directory).map_err(unavailable)?;
    for path in allowlist {
        policy.allow_export(path.clone());
    }
    let captured = capture(
        store,
        &CaptureRequest {
            root: directory,
            job_id: JobId::from_bytes(Ulid::generate().to_bytes()),
            created_by: node,
            created_at: now_ms(),
            parent: None,
            policy: &policy,
            carry_keys: &UnmeasuredKeys,
            harness_state: None,
            env_spec: None,
            allow_large_untracked: false,
        },
    )
    .map_err(unavailable)?;
    store.publish(&captured.manifest).map_err(unavailable)
}

/// The root a managed job's attempt runs in on this node.
pub fn root_for(node: &Node, job: JobId) -> PathBuf {
    crate::paths::job_root(&node.cfg.home, job)
}

/// Prepares `source` as `job`'s workspace on this node.
/// A snapshot's harness state, when it holds one, is written to
/// `harness_state` too, an absent or empty directory.
pub async fn prepare(
    node: &Node,
    job: JobId,
    source: &SourceBinding,
    harness_state: Option<PathBuf>,
) -> Result<Prepared, SourceError> {
    let root = root_for(node, job);
    let source = source.clone();
    if matches!(source, SourceBinding::Snapshot { .. }) {
        let root = root.clone();
        return with_store(&node.store, move |store| {
            Ok(prepare_at(
                &root,
                &source,
                Some(store),
                harness_state.as_deref(),
            ))
        })
        .await
        .map_err(unavailable)?;
    }
    tokio::task::spawn_blocking(move || prepare_in(&root, &source, None))
        .await
        .map_err(unavailable)?
}

/// Captures `directory` into a snapshot through the existing capture path without touching it.
pub async fn import(
    node: &Node,
    directory: &AbsPath,
    allowlist: &[RelPath],
) -> Result<SnapshotId, SourceError> {
    let directory = PathBuf::from(directory.as_str());
    let allowlist = allowlist.to_vec();
    let me = node.id;
    with_store(&node.store, move |store| {
        Ok(import_from(store, me, &directory, &allowlist))
    })
    .await
    .map_err(unavailable)?
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use fabric_core::managed::GitUri;

    use super::*;

    const NODE: NodeId = NodeId::from_bytes([3; 32]);

    fn sh(cwd: &Path, script: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            // No detached maintenance after a commit: the fixture owns
            // every process that writes under the checkout it compares.
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "maintenance.auto")
            .env("GIT_CONFIG_VALUE_0", "false")
            .env("GIT_CONFIG_KEY_1", "gc.auto")
            .env("GIT_CONFIG_VALUE_1", "0")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@localhost")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@localhost")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{script}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A repository with two commits; returns the first commit's id.
    fn repo_with_two_commits(dir: &Path) -> String {
        sh(
            dir,
            "git init -q -b main . && echo one > a.txt && git add a.txt && git commit -q -m one",
        );
        let first = sh(dir, "git rev-parse HEAD");
        sh(
            dir,
            "echo two > b.txt && git add b.txt && git commit -q -m two",
        );
        first
    }

    fn store(dir: &Path) -> Store {
        Store::create(
            &dir.join("store"),
            fabric_store::objects::ChunkParams::DEFAULT,
        )
        .unwrap()
    }

    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, (Vec<u8>, u128)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let meta = std::fs::metadata(&path).unwrap();
                let mtime = meta
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                out.insert(rel, (std::fs::read(&path).unwrap(), mtime));
            }
        }
    }

    fn tree(root: &Path) -> BTreeMap<String, (Vec<u8>, u128)> {
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn a_git_source_is_cloned_at_its_exact_revision() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        let first = repo_with_two_commits(&origin);
        let uri = GitUri::parse(&format!("file://{}", origin.display())).unwrap();
        let source = SourceBinding::Git {
            uri: uri.clone(),
            commit: CommitId::parse(&first).unwrap(),
        };
        let root = dir.path().join("work");
        let prepared = prepare_in(&root, &source, None).unwrap();
        assert_eq!(
            prepared,
            Prepared::Cloned {
                root: AbsPath::parse(root.to_str().unwrap()).unwrap(),
                commit: CommitId::parse(&first).unwrap()
            }
        );
        assert_eq!(sh(&root, "git rev-parse HEAD"), first);
        assert!(root.join("a.txt").exists());
        assert!(
            !root.join("b.txt").exists(),
            "the second commit is not checked out"
        );
        // A commit the repository does not have is unavailable before any launch.
        let missing = SourceBinding::Git {
            uri,
            commit: CommitId::parse(&"f".repeat(40)).unwrap(),
        };
        let root2 = dir.path().join("work2");
        assert!(matches!(
            prepare_in(&root2, &missing, None),
            Err(SourceError::Unavailable { .. })
        ));
        // A non-empty root is never cloned over.
        assert!(matches!(
            prepare_in(&root, &source, None),
            Err(SourceError::Unavailable { .. })
        ));
    }

    #[test]
    fn an_import_leaves_the_directory_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "alpha").unwrap();
        std::fs::write(src.join("sub/b.txt"), "beta").unwrap();
        let before = tree(&src);
        let mut store = store(dir.path());
        let snapshot = import_from(&mut store, NODE, &src, &[]).unwrap();
        assert_eq!(tree(&src), before, "the directory is as it was");
        // The snapshot restores elsewhere with the same files.
        let root = dir.path().join("restored");
        let prepared =
            prepare_in(&root, &SourceBinding::Snapshot { snapshot }, Some(&store)).unwrap();
        assert!(matches!(prepared, Prepared::Restored { .. }));
        let restored: BTreeMap<_, _> = tree(&root).into_iter().map(|(k, (b, _))| (k, b)).collect();
        assert_eq!(restored.get("a.txt").unwrap(), b"alpha");
        assert_eq!(restored.get("sub/b.txt").unwrap(), b"beta");
        assert_eq!(tree(&src), before, "nor after the restore");
    }

    /// A snapshot restores on a node that never held the job, whose root's
    /// parent directories do not exist yet, as a continuation's target.
    #[test]
    fn a_snapshot_restores_where_no_parent_of_the_root_exists() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), "alpha").unwrap();
        let mut store = store(dir.path());
        let snapshot = import_from(&mut store, NODE, &src, &[]).unwrap();
        let root = dir.path().join("home/fabric/jobs/job/work");
        let prepared =
            prepare_in(&root, &SourceBinding::Snapshot { snapshot }, Some(&store)).unwrap();
        assert!(matches!(prepared, Prepared::Restored { .. }));
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"alpha");
    }

    #[test]
    fn an_empty_source_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("empty");
        let prepared = prepare_in(&root, &SourceBinding::Empty, None).unwrap();
        assert_eq!(
            prepared,
            Prepared::Empty {
                root: AbsPath::parse(root.to_str().unwrap()).unwrap()
            }
        );
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::write(root.join("x"), "x").unwrap();
        assert!(matches!(
            prepare_in(&root, &SourceBinding::Empty, None),
            Err(SourceError::Unavailable { .. })
        ));
    }

    #[test]
    fn adoption_is_refused_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        repo_with_two_commits(&checkout);
        let before = tree(&checkout);
        let source = SourceBinding::Repository {
            path: AbsPath::parse(checkout.to_str().unwrap()).unwrap(),
        };
        assert_eq!(
            prepare_in(&dir.path().join("work"), &source, None),
            Err(SourceError::Adoption)
        );
        assert_eq!(
            SourceError::Adoption.to_string(),
            "adopting an active checkout as a managed job's workspace is not supported"
        );
        assert_eq!(tree(&checkout), before);
        assert!(!dir.path().join("work").exists());
    }

    #[test]
    fn git_leaves_no_maintenance_behind() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = git(dir.path());
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for setting in ["maintenance.auto=false", "gc.auto=0"] {
            let at = args
                .iter()
                .position(|a| a == setting)
                .unwrap_or_else(|| panic!("{setting} is not passed: {args:?}"));
            assert_eq!(args[at - 1], "-c", "{args:?}");
        }
    }

    #[test]
    fn a_credential_in_a_uri_is_refused() {
        assert_eq!(
            GitUri::parse("https://user:ghp_xxx@github.com/o/r.git"),
            Err(SourceError::CredentialInUri)
        );
        assert_eq!(
            GitUri::parse("ssh://git@github.com/o/r.git"),
            Err(SourceError::CredentialInUri)
        );
        // A URI that parses but names nothing reachable is unavailable, and
        // the clone asked no prompt for it.
        let dir = tempfile::tempdir().unwrap();
        let source = SourceBinding::Git {
            uri: GitUri::parse("file:///nonexistent/fabric-test-repo").unwrap(),
            commit: CommitId::parse(&"a".repeat(40)).unwrap(),
        };
        let started = std::time::Instant::now();
        assert!(matches!(
            prepare_in(&dir.path().join("work"), &source, None),
            Err(SourceError::Unavailable { .. })
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
    }

    #[test]
    fn a_secret_path_leaves_only_when_the_allowlist_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("app.txt"), "app").unwrap();
        std::fs::write(src.join(".env"), "TOKEN=x").unwrap();
        std::fs::write(src.join("server.key"), "key").unwrap();
        let mut store = store(dir.path());
        let kept_back = import_from(&mut store, NODE, &src, &[]).unwrap();
        let root = dir.path().join("r1");
        prepare_in(
            &root,
            &SourceBinding::Snapshot {
                snapshot: kept_back,
            },
            Some(&store),
        )
        .unwrap();
        assert!(root.join("app.txt").exists());
        assert!(!root.join(".env").exists(), ".env is kept back by default");
        assert!(!root.join("server.key").exists());
        let allowed =
            import_from(&mut store, NODE, &src, &[RelPath::parse(".env").unwrap()]).unwrap();
        assert_ne!(allowed, kept_back);
        let root = dir.path().join("r2");
        prepare_in(
            &root,
            &SourceBinding::Snapshot { snapshot: allowed },
            Some(&store),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".env")).unwrap(),
            "TOKEN=x"
        );
        assert!(
            !root.join("server.key").exists(),
            "only the named path leaves"
        );
    }
}
