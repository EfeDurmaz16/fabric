//! Sync a working tree between machines without a node: push it to a
//! directory or over SSH, pull it onto another tree.
//!
//! A push captures the tree with `fabric-store`'s capture, the same one a
//! suspend runs: tracked changes, staged and unstaged, untracked files, the
//! caches the store carries (`node_modules`, `.venv`, `target` and the
//! rest), under the same policy, so `.env` and every other secret-shaped
//! untracked or ignored file never enters the snapshot. The snapshot goes
//! into a store on this machine, then every object the target lacks is
//! copied to the target's store, bundle bases first, and the target's head
//! for the tree's name moves to it. A pull copies the other way and
//! restores the snapshot through the store's staging rename.
//!
//! Invariants:
//! - A snapshot enters a store only through `Store::publish`, after every
//!   object it names arrived and hashed to its id, so a copy cut anywhere
//!   leaves objects without a row, never a row without its objects, and the
//!   next copy sends only what is still missing.
//! - A head moves only by compare and swap under the target store's lock,
//!   and a push whose tree has not seen the target's head is refused.
//! - The tree path holds the old tree or the whole new one: a pull restores
//!   beside it and renames, and a pull cut between the renames is finished
//!   by the next one. The tree it replaced is kept beside it.
//!
//! There is no daemon: the SSH side is this binary's `serve`, started by
//! `ssh` for one sync and gone after it.

pub mod cli;
mod copy;
mod faults;
mod objects;
mod sync;
mod target;
mod tree;
mod wire;

use std::io;
use std::path::PathBuf;

use fabric_core::id::{JobId, SnapshotId};

pub use crate::objects::Counters;
pub use crate::sync::{Home, Outcome, PullRequest, PushRequest, SyncReport, pull, push};
pub use crate::target::{Target, TreeName};
pub use crate::wire::serve;

/// The product's name: the binary, the state directory, the environment
/// variables and the names of the files a pull leaves beside a tree all
/// come from it. The name is not final; a rename changes this constant and
/// the two names in Cargo.toml.
pub const NAME: &str = "fabric-sync";

/// The environment variable named `<NAME>_<suffix>`, such as `FABRIC_SYNC_HOME`.
#[must_use]
pub fn env_var(suffix: &str) -> String {
    format!("{}_{suffix}", NAME.to_ascii_uppercase().replace('-', "_"))
}

/// Why a sync stopped.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// A target that is neither a path nor an SSH address.
    #[error("{text} is not a directory path or an SSH target such as host:path")]
    BadTarget {
        /// What was given.
        text: String,
    },
    /// A tree name with a character outside letters, digits, `.`, `_` and `-`.
    #[error(
        "{text} is not a tree name: use letters, digits, '.', '_' and '-', not starting with '.'"
    )]
    BadName {
        /// What was given.
        text: String,
    },
    /// The tree path cannot be used.
    #[error("{path} cannot be synced: {why}")]
    BadTree {
        /// The path.
        path: PathBuf,
        /// Why.
        why: String,
    },
    /// The state directory is not an absolute path, or no home is known.
    #[error("no state directory: {why}")]
    NoHome {
        /// Why.
        why: String,
    },
    /// The target holds no tree under the name.
    #[error("the target holds no tree named {name}")]
    NothingThere {
        /// The name.
        name: String,
    },
    /// The target's head moved past what this tree has seen.
    #[error("the target's {name} is at {head}, which this tree has not pulled")]
    TargetMoved {
        /// The name.
        name: String,
        /// The target's head.
        head: SnapshotId,
    },
    /// The target holds another tree under the name.
    #[error("the target's {name} is another tree (job {job})")]
    OtherTree {
        /// The name.
        name: String,
        /// The job the target's head belongs to.
        job: JobId,
    },
    /// A pull onto a tree with content, without `--replace`.
    #[error("{path} is not empty")]
    NotEmpty {
        /// The tree.
        path: PathBuf,
    },
    /// The SSH side could not be started or stopped answering.
    #[error("the target did not answer: {reason}")]
    Unreachable {
        /// What happened.
        reason: String,
    },
    /// The other side sent something this side refuses.
    #[error("the other side sent bad data: {reason}")]
    Protocol {
        /// What was wrong.
        reason: String,
    },
    /// The target refused a request.
    #[error("the target refused: {reason}")]
    Refused {
        /// Its reason.
        reason: String,
    },
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        #[source]
        source: io::Error,
    },
    /// The store refused.
    #[error(transparent)]
    Store(#[from] fabric_store::store::StoreError),
    /// Capture refused or failed.
    #[error(transparent)]
    Capture(#[from] fabric_store::capture::CaptureError),
    /// Restore refused or failed.
    #[error(transparent)]
    Restore(#[from] fabric_store::restore::RestoreError),
    /// `.fabricignore` does not read.
    #[error(transparent)]
    Policy(#[from] fabric_store::policy::PolicyError),
    /// An object could not be read or written.
    #[error(transparent)]
    Object(#[from] fabric_store::objects::ObjectError),
}

pub(crate) fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> SyncError {
    let path = path.into();
    move |source| SyncError::Io { path, source }
}
