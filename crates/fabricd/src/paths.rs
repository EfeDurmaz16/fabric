//! Where a node keeps its state and where a job's root lives.
//!
//! Invariant: everything the node writes is under its state directory,
//! except a job root. A job root is `$HOME/fabric/jobs/<job-id>/work`, the
//! same string on every node with the same home (ADR 0011, P1), and a
//! restore lands at the root the snapshot recorded only when that root is
//! exactly this job's root under this node's home. The recorded root comes
//! from a peer, so any other root, under another home or anywhere else,
//! restores into the state directory's fallback root, and the choice is
//! printed with its reason. A root is never inside a folder a sync service
//! rewrites.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use fabric_core::id::{Epoch, JobId};

/// The environment variable that overrides the state directory.
pub const STATE_DIR_ENV: &str = "FABRIC_HOME";

/// Why a path could not be used.
#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    /// The state directory is not an absolute path.
    #[error("the state directory {0} is not an absolute path")]
    RelativeStateDir(PathBuf),
    /// The home directory is not an absolute path.
    #[error("the home directory {0} is not an absolute path")]
    RelativeHome(PathBuf),
    /// The root lies in a folder a sync service rewrites behind the agent's back.
    #[error("{root} is inside {folder}, which {service}; a job root must not be (ADR 0011)")]
    SyncedRoot {
        /// The refused root.
        root: PathBuf,
        /// The synced folder that holds it.
        folder: PathBuf,
        /// What syncs it.
        service: &'static str,
    },
}

/// A node's state directory: `$FABRIC_HOME`, or `~/.fabric`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    /// Picks the state directory from `FABRIC_HOME` when it is set, else `<home>/.fabric`.
    pub fn resolve(fabric_home: Option<&OsStr>, home: &Path) -> Result<Self, PathsError> {
        let root = match fabric_home {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => {
                if !home.is_absolute() {
                    return Err(PathsError::RelativeHome(home.to_owned()));
                }
                home.join(".fabric")
            }
        };
        if !root.is_absolute() {
            return Err(PathsError::RelativeStateDir(root));
        }
        Ok(Self { root })
    }

    /// The directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The node's private key, written once (ADR 0005).
    #[must_use]
    pub fn node_key(&self) -> PathBuf {
        self.root.join("node.key")
    }

    /// The peer book (ADR 0013).
    #[must_use]
    pub fn peers(&self) -> PathBuf {
        self.root.join("peers")
    }

    /// The content-addressed store (ADR 0014).
    #[must_use]
    pub fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    /// The directory that holds one directory per job.
    #[must_use]
    pub fn jobs(&self) -> PathBuf {
        self.root.join("jobs")
    }

    /// The files this node keeps for one job.
    #[must_use]
    pub fn job(&self, job: JobId) -> JobDir {
        JobDir {
            dir: self.jobs().join(job.to_string()),
        }
    }

    /// The control socket the CLI talks to.
    #[must_use]
    pub fn socket(&self) -> PathBuf {
        self.root.join("fabric.sock")
    }

    /// The lock the running daemon holds for its whole life.
    #[must_use]
    pub fn lock(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }

    /// The optional harness configuration file.
    #[must_use]
    pub fn harness_config(&self) -> PathBuf {
        self.root.join("harness.toml")
    }

    /// The directory of the per-workspace locks that keep one installer per tree (ADR 0007).
    #[must_use]
    pub fn tree_locks(&self) -> PathBuf {
        self.root.join("locks")
    }
}

/// The files one node keeps for one job, all node-local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDir {
    dir: PathBuf,
}

impl JobDir {
    /// The job's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The append-only ledger (ADR 0002).
    #[must_use]
    pub fn ledger(&self) -> PathBuf {
        self.dir.join("ledger")
    }

    /// The derived job manifest (ADR 0002).
    #[must_use]
    pub fn manifest(&self) -> PathBuf {
        self.dir.join("manifest")
    }

    /// The workspace marker (ADR 0004); never inside the workspace.
    #[must_use]
    pub fn marker(&self) -> PathBuf {
        self.dir.join("marker")
    }

    /// The record of the harness this node launched (ADR 0003).
    #[must_use]
    pub fn harness(&self) -> PathBuf {
        self.dir.join("harness")
    }

    /// The harness session the job continues on this node.
    #[must_use]
    pub fn session(&self) -> PathBuf {
        self.dir.join("session")
    }

    /// The session a registration recorded before its `Resumed`, until it
    /// moves to `session`.
    #[must_use]
    pub fn session_pending(&self) -> PathBuf {
        self.dir.join("session.pending")
    }

    /// The Codex threads the job's workspace held when a Codex with no
    /// thread to resume was planned here and its launch was recorded, until
    /// a capture or a plan takes the thread that launch made, the oldest one
    /// not among them.
    #[must_use]
    pub fn fresh_codex(&self) -> PathBuf {
        self.dir.join("fresh-codex")
    }

    /// The Codex threads the job's workspace held when a Codex with no
    /// thread to resume was planned here, until the registration of that
    /// launch makes it `fresh_codex`. A plan the CLI never launched leaves
    /// it here, so no thread made later by hand is taken for the job's.
    #[must_use]
    pub fn fresh_codex_planned(&self) -> PathBuf {
        self.dir.join("fresh-codex.planned")
    }

    /// The id of the environment spec whose setup steps the user last let
    /// this node run for the job (ADR 0007); the CLI writes it.
    #[must_use]
    pub fn env_spec_ran(&self) -> PathBuf {
        self.dir.join("env-spec-ran")
    }

    /// Every secret pattern the job's spec held at a capture on this node.
    #[must_use]
    pub fn secret_patterns(&self) -> PathBuf {
        self.dir.join("secret-patterns")
    }

    /// The fork record that freezes the job on this node.
    #[must_use]
    pub fn forked(&self) -> PathBuf {
        self.dir.join("forked")
    }

    /// Where harness state is captured into and restored from.
    #[must_use]
    pub fn harness_state(&self) -> PathBuf {
        self.dir.join("harness-state")
    }

    /// The root used when a snapshot was captured under another home.
    #[must_use]
    pub fn fallback_root(&self) -> PathBuf {
        self.dir.join("work")
    }
}

/// The root of a job made on a node with home `home`: `<home>/fabric/jobs/<job-id>/work`.
#[must_use]
pub fn job_root(home: &Path, job: JobId) -> PathBuf {
    home.join("fabric")
        .join("jobs")
        .join(job.to_string())
        .join("work")
}

/// Where a departed copy of the workspace at `root` goes once another node
/// owns the job at `epoch`: `<root parent>/departed/<epoch>`, beside the
/// root so the rename never crosses a filesystem. Kept, never deleted.
/// `None` for a root with no parent.
#[must_use]
pub fn departed_copy(root: &Path, epoch: Epoch) -> Option<PathBuf> {
    Some(
        root.parent()?
            .join("departed")
            .join(epoch.get().to_string()),
    )
}

/// Where a restore sets aside a workspace an earlier tenure left at
/// `root`, written at `epoch`: `<root parent>/orphan/<epoch>`, beside the
/// root. Kept, never deleted. `None` for a root with no parent.
#[must_use]
pub fn orphan_copy(root: &Path, epoch: Epoch) -> Option<PathBuf> {
    Some(root.parent()?.join("orphan").join(epoch.get().to_string()))
}

/// Where a snapshot restores on this node, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootChoice {
    /// The root the snapshot recorded, which is this job's root under this node's home.
    Same(PathBuf),
    /// The state directory's root, because the snapshot recorded another root.
    Fallback {
        /// Where the workspace goes on this node.
        root: PathBuf,
        /// The root the snapshot recorded.
        captured: PathBuf,
    },
}

impl RootChoice {
    /// The root to restore into.
    #[must_use]
    pub fn root(&self) -> &Path {
        match self {
            RootChoice::Same(root) | RootChoice::Fallback { root, .. } => root,
        }
    }
}

impl fmt::Display for RootChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RootChoice::Same(root) => write!(
                f,
                "root {}: the same path as the source (ADR 0011)",
                root.display()
            ),
            RootChoice::Fallback { root, captured } => write!(
                f,
                "root {}: the source root {} is not this job's root under this node's home, so the job lands in the state directory and a harness keyed to the path cannot continue there",
                root.display(),
                captured.display()
            ),
        }
    }
}

/// Picks the restore root for a snapshot of `id` captured at `captured` (ADR 0011).
#[must_use]
pub fn restore_root(captured: &Path, home: &Path, id: JobId, job: &JobDir) -> RootChoice {
    if captured == job_root(home, id) {
        RootChoice::Same(captured.to_owned())
    } else {
        RootChoice::Fallback {
            root: job.fallback_root(),
            captured: captured.to_owned(),
        }
    }
}

/// Which of the user's folders a sync service rewrites on this node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncedFolders {
    /// iCloud Drive syncs `~/Desktop`.
    pub desktop: bool,
    /// iCloud Drive syncs `~/Documents`.
    pub documents: bool,
}

/// Refuses a root inside a synced folder (ADR 0011, rule 4).
///
/// `~/Library/Mobile Documents` is iCloud Drive itself and is refused on
/// every node; `~/Desktop` and `~/Documents` only where iCloud syncs them.
pub fn check_root(root: &Path, home: &Path, synced: SyncedFolders) -> Result<(), PathsError> {
    let refuse = |folder: PathBuf, service| PathsError::SyncedRoot {
        root: root.to_owned(),
        folder,
        service,
    };
    let icloud = home.join("Library").join("Mobile Documents");
    if root.starts_with(&icloud) {
        return Err(refuse(icloud, "is iCloud Drive"));
    }
    for (synced, name) in [(synced.desktop, "Desktop"), (synced.documents, "Documents")] {
        let folder = home.join(name);
        if synced && root.starts_with(&folder) {
            return Err(refuse(folder, "iCloud Drive syncs on this node"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB: JobId = JobId::from_bytes([1; 16]);

    #[test]
    fn the_state_directory_is_fabric_home_when_set_and_dot_fabric_otherwise() {
        let home = Path::new("/Users/x");
        let set = StateDir::resolve(Some(OsStr::new("/srv/fab")), home).unwrap();
        assert_eq!(set.root(), Path::new("/srv/fab"));
        let unset = StateDir::resolve(None, home).unwrap();
        assert_eq!(unset.root(), Path::new("/Users/x/.fabric"));
        assert!(StateDir::resolve(Some(OsStr::new("fab")), home).is_err());
    }

    #[test]
    fn a_snapshot_from_the_same_home_restores_at_its_own_root() {
        let home = Path::new("/Users/x");
        let job = StateDir::resolve(None, home).unwrap().job(JOB);
        let captured = job_root(home, JOB);
        assert_eq!(
            restore_root(&captured, home, JOB, &job),
            RootChoice::Same(captured)
        );
    }

    #[test]
    fn a_snapshot_from_another_home_restores_under_the_state_directory() {
        let job = StateDir::resolve(None, Path::new("/home/x"))
            .unwrap()
            .job(JOB);
        let captured = job_root(Path::new("/Users/x"), JOB);
        let choice = restore_root(&captured, Path::new("/home/x"), JOB, &job);
        assert_eq!(choice.root(), job.fallback_root());
        assert!(matches!(choice, RootChoice::Fallback { .. }));
    }

    #[test]
    fn a_root_under_this_home_that_is_not_the_jobs_root_restores_under_the_state_directory() {
        let home = Path::new("/Users/x");
        let job = StateDir::resolve(None, home).unwrap().job(JOB);
        for captured in [
            home.join(".ssh"),
            job_root(home, JobId::from_bytes([2; 16])),
        ] {
            let choice = restore_root(&captured, home, JOB, &job);
            assert_eq!(choice.root(), job.fallback_root(), "{}", captured.display());
        }
    }

    #[test]
    fn a_root_in_a_synced_folder_is_refused_only_where_it_syncs() {
        let home = Path::new("/Users/x");
        let synced = SyncedFolders {
            desktop: true,
            documents: false,
        };
        assert!(check_root(&home.join("Desktop/w"), home, synced).is_err());
        assert!(check_root(&home.join("Documents/w"), home, synced).is_ok());
        assert!(
            check_root(
                &home.join("Library/Mobile Documents/w"),
                home,
                SyncedFolders::default()
            )
            .is_err()
        );
        assert!(check_root(&job_root(home, JOB), home, synced).is_ok());
    }
}
