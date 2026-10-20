//! The one error type of the node.
//!
//! Invariant: every refusal says which node owns the job, what state a
//! move is in, or which file failed, so the CLI prints the one command that
//! resolves it without parsing text (ADR 0003, Consequences).

use std::path::PathBuf;

use fabric_core::id::{Epoch, JobId, MoveId, NodeId, SnapshotId};
use fabric_core::lease::FenceError;
use fabric_core::ledger::{LedgerError, StoreError as LedgerStoreError};

use crate::harnesses::HarnessConfigError;
use crate::ledger_file::LedgerFileError;
use crate::local::LocalError;
use crate::paths::PathsError;

/// Why a node operation failed.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Another daemon holds this state directory.
    #[error("another fabric daemon holds {0}")]
    AlreadyRunning(PathBuf),
    /// The control socket's path is longer than a Unix socket path may be.
    #[error("the control socket {0} is too long for a Unix socket; use a shorter FABRIC_HOME")]
    SocketPathTooLong(PathBuf),
    /// No job with this id is known on this node.
    #[error("job {0} is not known on this node; name the node that holds it with --on")]
    UnknownJob(JobId),
    /// The job is owned by another node.
    #[error(
        "job {job} is owned by {owner} at epoch {epoch}; run `fabric job resume {job}` here to pull it"
    )]
    NotOwner {
        /// The job.
        job: JobId,
        /// The owner of record.
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// This node handed the job to another node; its copy never runs again.
    #[error(
        "job {job} departed to {owner}, which owns it at epoch {epoch}; this copy does not run. Run `fabric job resume {job}` to pull it back"
    )]
    Departed {
        /// The job.
        job: JobId,
        /// The owner.
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// A move this node started is open; nothing runs until the target decides.
    #[error(
        "job {job} is departing to {target} under move {move_id}; it cannot run until {target} answers. Run `fabric job move {job} --to {target}` to ask again, or `fabric job steal {job} --force` on another node"
    )]
    Departing {
        /// The job.
        job: JobId,
        /// The open move.
        move_id: MoveId,
        /// The node that decides it.
        target: NodeId,
    },
    /// The job ran on this node while a move copied it, so the copy is out of date.
    #[error(
        "job {job} ran on this node while {target} copied it, so nothing moved; run `fabric job move {job} --to {target}` again once it has stopped"
    )]
    RanDuringMove {
        /// The job.
        job: JobId,
        /// The target of the move.
        target: NodeId,
    },
    /// A harness for the job already runs on this node.
    #[error("a harness for job {job} already runs as pid {pid}")]
    AlreadyRunningHarness {
        /// The job.
        job: JobId,
        /// Its pid.
        pid: u32,
    },
    /// The job has no workspace on this node.
    #[error("job {0} has no workspace on this node; run `fabric job resume {0}`")]
    NoWorkspace(JobId),
    /// The workspace is mid-suspend or mid-restore.
    #[error("job {job}: {what} did not finish; run `fabric job {command} {job}` to finish it")]
    Unfinished {
        /// The job.
        job: JobId,
        /// What did not finish.
        what: &'static str,
        /// The command that finishes it.
        command: &'static str,
    },
    /// The job was never suspended, so there is no snapshot to restore or move.
    #[error("job {0} has no snapshot yet; suspend it on its owner first")]
    NoSnapshot(JobId),
    /// The restore root already holds files this node does not know.
    #[error(
        "{root} holds files and this node has no marker for them; move them aside, then resume again"
    )]
    WorkspaceOccupied {
        /// The root.
        root: PathBuf,
    },
    /// The peer is not paired, or has no address to dial.
    #[error("{0} is not paired with an address; run `fabric node add {0} <host:port>`")]
    UnknownPeer(NodeId),
    /// The peer could not be reached.
    #[error("{peer} is unreachable: {reason}")]
    Unreachable {
        /// The peer.
        peer: NodeId,
        /// What failed.
        reason: String,
    },
    /// The peer answered with something the protocol does not allow here.
    #[error("{peer} answered: {reason}")]
    Peer {
        /// The peer.
        peer: NodeId,
        /// What it said.
        reason: String,
    },
    /// The preflight found rows that stop the move.
    #[error("the preflight refused the move: {0}; pass --force to waive what can be waived")]
    Preflight(String),
    /// A steal without `--force`.
    #[error("{0}")]
    StealNeedsForce(String),
    /// A snapshot could not be fetched whole.
    #[error("snapshot {snapshot} could not be fetched from {peer}: {reason}")]
    Fetch {
        /// The snapshot.
        snapshot: SnapshotId,
        /// The peer.
        peer: NodeId,
        /// What failed.
        reason: String,
    },
    /// The fencing token is stale.
    #[error(transparent)]
    Fence(#[from] FenceError),
    /// The ledger rules refused.
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    /// The ledger file failed.
    #[error(transparent)]
    LedgerFile(#[from] LedgerFileError),
    /// A node-local file failed.
    #[error(transparent)]
    Local(#[from] LocalError),
    /// A path rule refused.
    #[error(transparent)]
    Paths(#[from] PathsError),
    /// The store failed.
    #[error(transparent)]
    Store(#[from] fabric_store::store::StoreError),
    /// The ignore policy refused a pattern.
    #[error(transparent)]
    Policy(#[from] fabric_store::policy::PolicyError),
    /// Capture failed.
    #[error(transparent)]
    Capture(#[from] fabric_store::capture::CaptureError),
    /// Restore failed.
    #[error(transparent)]
    Restore(#[from] fabric_store::restore::RestoreError),
    /// A harness adapter failed.
    #[error(transparent)]
    Harness(#[from] fabric_harness::HarnessError),
    /// A harness could not be named or configured.
    #[error(transparent)]
    HarnessConfig(#[from] HarnessConfigError),
    /// The network layer failed to start.
    #[error(transparent)]
    Transport(#[from] fabric_net::transport::TransportError),
    /// The node key could not be loaded.
    #[error(transparent)]
    Identity(#[from] fabric_net::identity::IdentityError),
    /// The peer book failed.
    #[error(transparent)]
    PeerBook(#[from] fabric_net::peers::PeerBookError),
    /// A wire value was out of bounds.
    #[error(transparent)]
    Wire(#[from] fabric_net::wire::WireError),
    /// The fault spec of the end-to-end test is not one this build knows.
    #[error(transparent)]
    Fault(#[from] crate::faults::UnknownFault),
    /// A blocking task panicked or was cancelled.
    #[error("a blocking task ended early: {0}")]
    Task(String),
}

impl From<LedgerStoreError<LedgerFileError>> for NodeError {
    fn from(err: LedgerStoreError<LedgerFileError>) -> Self {
        match err {
            LedgerStoreError::Store(e) => e.into(),
            LedgerStoreError::Ledger(e) => e.into(),
        }
    }
}

impl NodeError {
    /// A filesystem error at `path`.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        NodeError::Io {
            path: path.into(),
            source,
        }
    }
}
