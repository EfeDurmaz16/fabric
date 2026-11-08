//! Push and pull.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::id::{ChunkId, JobId, NodeId, SnapshotId};
use fabric_core::snapshot::{EntryKind, SnapshotManifest};
use fabric_store::capture::{CaptureRequest, UnmeasuredKeys, capture};
use fabric_store::objects::ChunkParams;
use fabric_store::policy::IgnorePolicy;
use fabric_store::restore::{Durability, RestoreRequest, Verify, restore};
use fabric_store::store::{Store, StoreError};

use crate::copy::copy_snapshot;
use crate::objects::{Counters, StoreObjects, has_snapshot};
use crate::target::{DirTarget, Head, Remote, Target, TreeName, write_atomic};
use crate::tree::{self, Beside};
use crate::wire::SshRemote;
use crate::{NAME, SyncError, faults, io_err};

/// The state of this machine's side: a store and, per tree, the job it
/// syncs under and the last snapshot it pushed or pulled.
#[derive(Debug, Clone)]
pub struct Home {
    dir: PathBuf,
}

impl Home {
    /// The directory `<NAME>_HOME` names, else `$XDG_DATA_HOME/<NAME>`,
    /// else `~/.local/share/<NAME>`; each must be an absolute path.
    pub fn from_env() -> Result<Home, SyncError> {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let own = crate::env_var("HOME");
        let dir = if let Some(dir) = var(&own) {
            dir
        } else if let Some(data) = var("XDG_DATA_HOME") {
            data.join(NAME)
        } else if let Some(home) = var("HOME") {
            home.join(".local/share").join(NAME)
        } else {
            return Err(SyncError::NoHome {
                why: format!("none of {own}, XDG_DATA_HOME and HOME is set"),
            });
        };
        if !dir.is_absolute() {
            return Err(SyncError::NoHome {
                why: format!("{} is not an absolute path", dir.display()),
            });
        }
        Ok(Home { dir })
    }

    /// A home at `dir`.
    #[must_use]
    pub fn at(dir: &Path) -> Home {
        Home {
            dir: dir.to_owned(),
        }
    }

    fn open_store(&self) -> Result<Store, SyncError> {
        let root = self.dir.join("store");
        match Store::open(&root) {
            Ok(store) => Ok(store),
            Err(StoreError::NotAStore { .. }) => Ok(Store::create(&root, ChunkParams::DEFAULT)?),
            Err(e) => Err(e.into()),
        }
    }

    fn state_path(&self, tree: &Path) -> PathBuf {
        let key = ChunkId::of(tree.as_os_str().as_encoded_bytes());
        self.dir.join("trees").join(key.to_string())
    }

    /// The job and last snapshot of `tree`; the first line of the file, the tree's path the second.
    fn state(&self, tree: &Path) -> Result<Option<Head>, SyncError> {
        let path = self.state_path(tree);
        match fs::read_to_string(&path) {
            Ok(text) => Head::read(text.lines().next().unwrap_or_default())
                .map(Some)
                .ok_or_else(|| SyncError::BadTree {
                    path: tree.to_owned(),
                    why: format!("its state at {} does not read", path.display()),
                }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(&path)(e)),
        }
    }

    fn pending_path(&self, tree: &Path) -> PathBuf {
        self.state_path(tree).with_extension("pending")
    }

    /// Records the snapshot a pull is about to write onto `tree`.
    fn set_pending(&self, tree: &Path, head: Head) -> Result<(), SyncError> {
        self.write_state(&self.pending_path(tree), tree, head)
    }

    /// Finishes what a cut pull left beside `tree`, and records what the tree holds.
    fn recover(&self, tree: &Path) -> Result<(), SyncError> {
        let path = self.pending_path(tree);
        let pending = match fs::read_to_string(&path) {
            Ok(text) => Head::read(text.lines().next().unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_err(&path)(e)),
        };
        if tree::recover(tree, pending.is_some())?
            && let Some(head) = pending
        {
            self.save(tree, head)?;
        }
        self.clear_pending(tree)
    }

    fn clear_pending(&self, tree: &Path) -> Result<(), SyncError> {
        let path = self.pending_path(tree);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(&path)(e)),
        }
    }

    fn save(&self, tree: &Path, head: Head) -> Result<(), SyncError> {
        self.write_state(&self.state_path(tree), tree, head)
    }

    fn write_state(&self, path: &Path, tree: &Path, head: Head) -> Result<(), SyncError> {
        let dir = self.dir.join("trees");
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let text = format!("{}{}\n", head.line(), tree.display());
        write_atomic(path, text.as_bytes())
    }

    /// The id this machine signs its snapshots' `created_by` with; informational.
    fn node(&self) -> NodeId {
        NodeId::from_bytes(ChunkId::of(self.dir.as_os_str().as_encoded_bytes()).to_bytes())
    }
}

/// What a sync did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A snapshot was captured and the target's head moved to it.
    Pushed,
    /// The target's snapshot was restored onto the tree.
    Pulled,
    /// The tree already holds the target's head; nothing was written.
    Unchanged,
}

impl Outcome {
    /// The word the JSON answer carries.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pushed => "pushed",
            Outcome::Pulled => "pulled",
            Outcome::Unchanged => "unchanged",
        }
    }
}

/// What a push or pull did, with the counters a benchmark records.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncReport {
    /// What happened.
    pub outcome: Outcome,
    /// The tree's name on the target.
    pub name: TreeName,
    /// The snapshot the target's head and the tree now share.
    pub snapshot: SnapshotId,
    /// Paths the snapshot holds: the working tree's tracked and untracked
    /// files and the carried ignored ones, or every file of a plain directory.
    pub files: u64,
    /// Stored bytes of every object the snapshot needs, compressed: the
    /// tree's size as both stores keep it.
    pub snapshot_bytes: u64,
    /// What the copy moved.
    pub counters: Counters,
    /// Untracked or ignored paths a secret pattern or `.fabricignore` kept off the snapshot.
    pub left_behind: Vec<String>,
    /// Tracked or stashed paths that look like secrets; git history carries them.
    pub tracked_secrets: Vec<String>,
    /// Things the user should know, such as native addons made for another platform.
    pub warnings: Vec<String>,
}

/// What to push.
#[derive(Debug, Clone)]
pub struct PushRequest {
    /// The tree; the top level when it is a repository.
    pub tree: PathBuf,
    /// Where to.
    pub target: Target,
    /// The name there; the tree's directory name when `None`.
    pub name: Option<TreeName>,
    /// Move the target's head even when this tree has not pulled it.
    pub force: bool,
}

/// What to pull.
#[derive(Debug, Clone)]
pub struct PullRequest {
    /// The tree to write; absent, empty, or replaced with `replace`.
    pub tree: PathBuf,
    /// Where from.
    pub target: Target,
    /// The name there; the tree's directory name when `None`.
    pub name: Option<TreeName>,
    /// Replace a tree with content, keeping it beside the new one.
    pub replace: bool,
}

fn connect(target: &Target, create: bool) -> Result<Box<dyn Remote>, SyncError> {
    Ok(match target {
        Target::Dir(dir) => Box::new(DirTarget::open(dir, create)?),
        Target::Ssh { host, port, path } => {
            Box::new(SshRemote::connect(host, *port, path, create)?)
        }
    })
}

/// The policy a suspend of `tree` captures under: the built-in lists,
/// `.fabricignore`, and the secret patterns and caches `.fabric/env.toml`
/// declares, as the node's suspend widens it.
fn policy_for(tree: &Path) -> Result<IgnorePolicy, SyncError> {
    let mut policy = IgnorePolicy::default();
    policy.load_fabricignore(tree)?;
    let declared = fabric_env::detect::declared_policy(tree)?;
    for pattern in &declared.secret_patterns {
        policy.add_secret_pattern(pattern.as_str())?;
    }
    for (class, path) in &declared.caches {
        policy.add_declared_cache(*class, path);
    }
    Ok(policy)
}

fn name_of(tree: &Path, given: Option<&TreeName>) -> Result<TreeName, SyncError> {
    if let Some(name) = given {
        return Ok(name.clone());
    }
    let base = tree
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    TreeName::parse(&base)
}

/// The tree's absolute path with its parent resolved, the tree itself
/// maybe absent; a link at the tree path is refused.
fn tree_path(tree: &Path) -> Result<PathBuf, SyncError> {
    let bad = |why: &str| SyncError::BadTree {
        path: tree.to_owned(),
        why: why.to_owned(),
    };
    let absolute = if tree.is_absolute() {
        tree.to_owned()
    } else {
        std::env::current_dir().map_err(io_err("."))?.join(tree)
    };
    let name = absolute
        .file_name()
        .filter(|n| *n != "..")
        .ok_or_else(|| bad("it names no directory"))?
        .to_owned();
    let parent = absolute.parent().ok_or_else(|| bad("it has no parent"))?;
    let parent = fs::canonicalize(parent).map_err(io_err(parent))?;
    let path = parent.join(name);
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(bad("it is a symbolic link"));
    }
    Ok(path)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A job id for a tree synced the first time: the time, then a hash of the
/// time, the process and the path, so two machines never pick the same one.
fn new_job(tree: &Path) -> JobId {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seed = format!("{nanos} {} {}", std::process::id(), tree.display());
    let hash = ChunkId::of(seed.as_bytes()).to_bytes();
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&now_ms().to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&hash[..10]);
    JobId::from_bytes(bytes)
}

/// True when `ancestor` is `from` or one of its parents in `store`.
fn descends(store: &Store, from: SnapshotId, ancestor: SnapshotId) -> Result<bool, SyncError> {
    let parents: HashMap<SnapshotId, Option<SnapshotId>> = store
        .snapshots()?
        .into_iter()
        .map(|row| (row.id, row.parent))
        .collect();
    let mut at = Some(from);
    let mut steps = 0usize;
    while let Some(id) = at {
        if id == ancestor {
            return Ok(true);
        }
        steps += 1;
        if steps > parents.len() {
            break;
        }
        at = parents.get(&id).copied().flatten();
    }
    Ok(false)
}

/// The paths a manifest holds and the stored bytes of its objects.
fn sizes(store: &Store, manifest: &SnapshotManifest) -> Result<(u64, u64), SyncError> {
    let mut files = manifest
        .files
        .iter()
        .filter(|e| !matches!(e.kind, EntryKind::Dir))
        .count() as u64;
    if let Some(git) = &manifest.git {
        files += git.worktree_mtimes.len() as u64;
    }
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0u64;
    for id in Store::data_chunks(manifest) {
        if seen.insert(id) {
            bytes += store.objects().stored_len(id)?;
        }
    }
    Ok((files, bytes))
}

/// Captures `req.tree` and pushes the snapshot to `req.target`.
pub fn push(home: &Home, req: &PushRequest) -> Result<SyncReport, SyncError> {
    faults::check().map_err(|reason| SyncError::Refused { reason })?;
    let tree = tree_path(&req.tree)?;
    home.recover(&tree)?;
    if !tree.is_dir() {
        return Err(SyncError::BadTree {
            path: tree,
            why: "it is not a directory".to_owned(),
        });
    }
    let name = name_of(&tree, req.name.as_ref())?;
    let mut store = home.open_store()?;
    let state = match home.state(&tree)? {
        Some(state) if has_snapshot(&store, state.snapshot)? => Some(state),
        _ => None,
    };
    let mut remote = connect(&req.target, true)?;
    let head = remote.head(&name)?;
    if let Some(head) = head
        && !req.force
    {
        match state {
            Some(state) if state.job != head.job => {
                return Err(SyncError::OtherTree {
                    name: name.to_string(),
                    job: head.job,
                });
            }
            Some(state) if descends(&store, state.snapshot, head.snapshot)? => {}
            _ => {
                return Err(SyncError::TargetMoved {
                    name: name.to_string(),
                    head: head.snapshot,
                });
            }
        }
    }
    let job = state.map_or_else(|| new_job(&tree), |state| state.job);
    let policy = policy_for(&tree)?;
    let captured = capture(
        &store,
        &CaptureRequest {
            root: &tree,
            job_id: job,
            created_by: home.node(),
            created_at: now_ms(),
            parent: state.map(|state| state.snapshot),
            policy: &policy,
            carry_keys: &UnmeasuredKeys,
            harness_state: None,
            env_spec: None,
            allow_large_untracked: false,
        },
    )?;
    let snapshot = store.publish(&captured.manifest)?;
    // The snapshot is this tree's newest whether or not the push below
    // finishes: the next capture follows it, and the next push sends it.
    home.save(&tree, Head { job, snapshot })?;
    let mut counters = Counters::default();
    copy_snapshot(
        &mut StoreObjects(&mut store),
        &mut *remote,
        snapshot,
        &mut counters,
    )?;
    remote.set_head(
        &name,
        head.map(|head| head.snapshot),
        Head { job, snapshot },
    )?;
    remote.close()?;
    let (files, snapshot_bytes) = sizes(&store, &captured.manifest)?;
    let report = captured.report;
    let shown = |paths: Vec<fabric_core::path::RelPath>| -> Vec<String> {
        paths.iter().map(|p| p.as_str().to_owned()).collect()
    };
    let mut left_behind = shown(report.denied);
    left_behind.extend(shown(report.denied_untracked));
    left_behind.extend(shown(report.excluded));
    left_behind.sort();
    let mut tracked_secrets = shown(report.tracked_secrets);
    tracked_secrets.extend(shown(report.stashed_secrets));
    Ok(SyncReport {
        outcome: Outcome::Pushed,
        name,
        snapshot,
        files,
        snapshot_bytes,
        counters,
        left_behind,
        tracked_secrets,
        warnings: Vec::new(),
    })
}

/// Pulls the target's snapshot of the tree's name onto `req.tree`.
pub fn pull(home: &Home, req: &PullRequest) -> Result<SyncReport, SyncError> {
    faults::check().map_err(|reason| SyncError::Refused { reason })?;
    if let Some(parent) = req.tree.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let tree = tree_path(&req.tree)?;
    home.recover(&tree)?;
    let name = name_of(&tree, req.name.as_ref())?;
    let mut store = home.open_store()?;
    let mut remote = connect(&req.target, false)?;
    let head = remote.head(&name)?.ok_or_else(|| SyncError::NothingThere {
        name: name.to_string(),
    })?;
    let empty = tree::is_empty(&tree)?;
    let state = home.state(&tree)?;
    if !empty && state.map(|state| state.snapshot) == Some(head.snapshot) {
        remote.close()?;
        let manifest = store.manifest(head.snapshot)?;
        let (files, snapshot_bytes) = sizes(&store, &manifest)?;
        return Ok(report(
            Outcome::Unchanged,
            name,
            head.snapshot,
            files,
            snapshot_bytes,
            Counters::default(),
            &manifest,
        ));
    }
    if !empty && !req.replace {
        return Err(SyncError::NotEmpty { path: tree });
    }
    let mut counters = Counters::default();
    copy_snapshot(
        &mut *remote,
        &mut StoreObjects(&mut store),
        head.snapshot,
        &mut counters,
    )?;
    remote.close()?;
    let manifest = store.manifest(head.snapshot)?;
    let beside = Beside::of(&tree)?;
    let into = if empty {
        home.set_pending(&tree, head)?;
        tree.clone()
    } else {
        tree::remove_tree(&beside.previous)?;
        beside.incoming.clone()
    };
    restore(
        &store,
        &RestoreRequest {
            snapshot: head.snapshot,
            target: &into,
            harness_state: None,
            verify: Verify::Sample(32),
            durability: Durability::ProcessCrash,
            carry: &|_| true,
        },
    )?;
    if !empty {
        home.set_pending(&tree, head)?;
        tree::swap_in(&tree, &beside)?;
    }
    home.save(&tree, head)?;
    home.clear_pending(&tree)?;
    let (files, snapshot_bytes) = sizes(&store, &manifest)?;
    Ok(report(
        Outcome::Pulled,
        name,
        head.snapshot,
        files,
        snapshot_bytes,
        counters,
        &manifest,
    ))
}

fn report(
    outcome: Outcome,
    name: TreeName,
    snapshot: SnapshotId,
    files: u64,
    snapshot_bytes: u64,
    counters: Counters,
    manifest: &SnapshotManifest,
) -> SyncReport {
    let here = (std::env::consts::OS, std::env::consts::ARCH);
    let there = (
        manifest.platform.os.as_str(),
        manifest.platform.arch.as_str(),
    );
    let warnings = manifest
        .caches
        .iter()
        .filter(|cache| cache.native_files > 0 && here != there)
        .map(|cache| {
            format!(
                "{} holds {} native addons built for {}-{}; rebuild them here",
                cache.path.as_str(),
                cache.native_files,
                there.0,
                there.1
            )
        })
        .collect();
    SyncReport {
        outcome,
        name,
        snapshot,
        files,
        snapshot_bytes,
        counters,
        left_behind: manifest
            .left_behind
            .iter()
            .map(|left| left.path.as_str().to_owned())
            .collect(),
        tracked_secrets: Vec::new(),
        warnings,
    }
}
