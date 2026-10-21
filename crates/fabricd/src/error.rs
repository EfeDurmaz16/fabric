//! The one error type of the node.
//!
//! Invariant: every refusal says which node owns the job, what state a
//! move is in, or which file failed, so the CLI prints the one command that
//! resolves it without parsing text (ADR 0003, Consequences).

use std::path::PathBuf;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, MoveId, NodeId, SnapshotId};
use fabric_core::lease::FenceError;
use fabric_core::ledger::{LedgerError, StoreError as LedgerStoreError};

use crate::control::Refusal;
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
    /// The state directory, or an entry in it, is not this user's alone to take.
    #[error("{path}: {reason}")]
    UnsafeStateDir {
        /// The directory.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// Another daemon holds this state directory.
    #[error("another fabric daemon holds {0}")]
    AlreadyRunning(PathBuf),
    /// The control socket's path is longer than a Unix socket path may be.
    #[error("the control socket {0} is too long for a Unix socket; use a shorter FABRIC_HOME")]
    SocketPathTooLong(PathBuf),
    /// No job with this id is known on this node.
    #[error(
        "job {0} is not known on this node; run `fabric job resume {0} --on <node>` to pull it from the node that holds it"
    )]
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
    /// A peer's ledger forked from this node's at an epoch at least as high, so the job is frozen here.
    #[error(
        "job {job} is frozen on this node: {peer} holds a ledger for it at epoch {epoch} that does not continue this node's, so it was stolen while this node still ran it. This copy never runs again and its workspace stays where it is; run `fabric job resume {job}` on {peer} to continue there"
    )]
    Forked {
        /// The job.
        job: JobId,
        /// The peer whose ledger forked.
        peer: NodeId,
        /// The highest epoch in the peer's ledger.
        epoch: Epoch,
    },
    /// A peer's ledger forked from this node's at a lower epoch; the peer is the side that stops.
    #[error(
        "{peer}'s ledger for job {job} forked from this node's at epoch {epoch}, behind this node's; it stops running the job when it learns this node's records"
    )]
    ForkBehind {
        /// The job.
        job: JobId,
        /// The peer whose ledger forked.
        peer: NodeId,
        /// The highest epoch in the peer's ledger.
        epoch: Epoch,
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
    /// A resume under another harness ran while the move's preflight checked the target for the old one.
    #[error(
        "job {job} was resumed under {now} while the preflight checked {target} for {checked}, so nothing moved; run `fabric job move {job} --to {target}` again, which checks {now}"
    )]
    HarnessChangedDuringMove {
        /// The job.
        job: JobId,
        /// The harness the preflight checked.
        checked: Box<HarnessKind>,
        /// The harness the job runs under now.
        now: Box<HarnessKind>,
        /// The target of the move.
        target: NodeId,
    },
    /// The holder resumed under another harness after a pull's preflight checked this node for the old one.
    #[error(
        "job {job} was resumed under {now} on {holder} after the preflight checked this node for {checked}, so the move was aborted and {holder} keeps the job; run `fabric job resume {job}` again, which checks {now}"
    )]
    HarnessChangedDuringPull {
        /// The job.
        job: JobId,
        /// The node that held the job.
        holder: NodeId,
        /// The harness the preflight checked.
        checked: Box<HarnessKind>,
        /// The harness the job runs under now.
        now: Box<HarnessKind>,
    },
    /// The workspace changed after the snapshot a move copied, by hand or by a harness Fabric did not start.
    #[error(
        "the workspace of job {job} at {root} changed after the snapshot {target} copied, so nothing moved; run `fabric job move {job} --to {target}` again, which captures the change"
    )]
    ChangedDuringMove {
        /// The job.
        job: JobId,
        /// The root.
        root: PathBuf,
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
    /// The launched harness is not running as the leader of its own group, or its start time could not be read.
    #[error(
        "the harness pid {pid} is not running as the leader of its own process group, or its start time cannot be read, so it is not recorded"
    )]
    HarnessGone {
        /// The pid the CLI named.
        pid: std::num::NonZeroU32,
    },
    /// The workspace at the root was left by an earlier tenure of this node.
    #[error(
        "job {job}: the workspace at {root} was left at epoch {epoch}, before this node's tenure, and may differ from the job's latest snapshot; run `fabric job resume {job}`, which sets it aside and restores the latest snapshot"
    )]
    StaleWorkspace {
        /// The job.
        job: JobId,
        /// The root.
        root: PathBuf,
        /// The epoch its marker was written at.
        epoch: Epoch,
    },
    /// The workspace root is a link or a file, put there in place of the directory Fabric wrote.
    #[error(
        "job {job}: {root} is not a directory but a link or a file, so nothing runs or is captured there; move it aside, then run `fabric job resume {job}`, which restores the latest snapshot when the workspace was clean"
    )]
    RootNotADirectory {
        /// The job.
        job: JobId,
        /// The root.
        root: PathBuf,
    },
    /// The job's ledger moved past the head a run plan was made at, so the plan is stale.
    #[error(
        "job {job} changed after this launch was planned, by another run or resume, a suspend or a record from a peer, so nothing was launched; run the command again"
    )]
    PlanOvertaken {
        /// The job.
        job: JobId,
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
    /// A process holds the workspace's tree lock: a run or a resume while
    /// it runs setup steps or launches, or a process a setup step left
    /// running with the lock as its input.
    #[error(
        "job {job}: nothing was captured, since a process holds {lock}, the lock over {root}: a `fabric job run` or `resume` running setup steps or launching, or a process a setup step left running, which holds it until it exits. `lsof {lock}` names it; try again once it is gone"
    )]
    SetupRunning {
        /// The job.
        job: JobId,
        /// The root.
        root: PathBuf,
        /// The lock file its holder has open.
        lock: PathBuf,
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
    /// A `host:port` the user typed does not parse.
    #[error("{text} is not host:port: {reason}")]
    BadAddress {
        /// What the user typed.
        text: String,
        /// Why it does not parse.
        reason: String,
    },
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
    #[error(
        "the preflight refused the move: {0}; `fabric job move` and `fabric job resume` take --force to waive what can be waived"
    )]
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
    /// The workspace's environment could not be detected, so no node could materialize it.
    #[error("environment detection: {0}")]
    Detect(#[from] fabric_env::detect::DetectError),
    /// The workspace's tree lock could not be taken.
    #[error(transparent)]
    Materialize(#[from] fabric_env::materialize::MaterializeError),
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
    /// The kind of refusal this is, for the CLI's exit code.
    #[must_use]
    pub fn refusal(&self) -> Refusal {
        match self {
            NodeError::NotOwner { .. }
            | NodeError::Departed { .. }
            | NodeError::UnknownJob(_)
            | NodeError::Fence(_) => Refusal::NotOwner,
            NodeError::Departing { .. } | NodeError::Forked { .. } => Refusal::Frozen,
            NodeError::Unreachable { .. } | NodeError::UnknownPeer(_) => Refusal::Unreachable,
            NodeError::Preflight(_) | NodeError::StealNeedsForce(_) => Refusal::NeedsForce,
            NodeError::Unfinished { .. }
            | NodeError::SetupRunning { .. }
            | NodeError::AlreadyRunningHarness { .. }
            | NodeError::RanDuringMove { .. }
            | NodeError::HarnessChangedDuringMove { .. }
            | NodeError::HarnessChangedDuringPull { .. }
            | NodeError::ChangedDuringMove { .. }
            | NodeError::WorkspaceOccupied { .. }
            | NodeError::NoWorkspace(_)
            | NodeError::RootNotADirectory { .. }
            | NodeError::PlanOvertaken { .. }
            | NodeError::StaleWorkspace { .. }
            | NodeError::NoSnapshot(_)
            | NodeError::HarnessGone { .. } => Refusal::JobState,
            _ => Refusal::Failed,
        }
    }

    /// A filesystem error at `path`.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        NodeError::Io {
            path: path.into(),
            source,
        }
    }
}
