//! The one error type of the node.
//!
//! Invariant: every refusal says which node owns the job, what state a
//! move is in, or which file failed, so the CLI prints the one command that
//! resolves it without parsing text (ADR 0003, Consequences). A client gets
//! the same facts typed, as the reason of a refusal (ADR 0018).

use std::path::PathBuf;

use fabric_client::api::{Reason, Refusal, StealWarning, Step};
use fabric_core::id::{Epoch, JobId, MoveId, NodeId, SnapshotId};
use fabric_core::lease::FenceError;
use fabric_core::ledger::{LedgerError, StoreError as LedgerStoreError};
use fabric_harness::HarnessError;

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
        pid: std::num::NonZeroU32,
    },
    /// The launched harness is not running as the leader of its own group, or its start time could not be read.
    #[error(
        "the harness pid {pid} is not running as the leader of its own process group, or its start time cannot be read, so it is not recorded"
    )]
    HarnessGone {
        /// The pid the CLI named.
        pid: std::num::NonZeroU32,
    },
    /// A session was named for a harness this node no longer records.
    #[error(
        "job {job}: pid {pid} is not the harness this node records, because a suspend, a move or a fence stopped it, so its session {session} is not recorded"
    )]
    HarnessNotRecorded {
        /// The job.
        job: JobId,
        /// The pid the CLI named.
        pid: std::num::NonZeroU32,
        /// The session it named.
        session: fabric_core::session::SessionId,
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
    /// The job has no workspace on this node.
    #[error("job {0} has no workspace on this node; run `fabric job resume {0}`")]
    NoWorkspace(JobId),
    /// The workspace is mid-suspend or mid-restore.
    #[error(
        "job {job}: {} did not finish; run `fabric job {} {job}` to finish it",
        unfinished_what(*step),
        unfinished_command(*step)
    )]
    Unfinished {
        /// The job.
        job: JobId,
        /// Which step did not finish.
        step: Step,
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
    /// The peer answered with a fact that stops the command, such as a
    /// newer epoch or a ledger that does not continue this one; a client
    /// reads `fact`, and the sentence says what to do.
    #[error("{peer} answered: {reason}")]
    PeerFact {
        /// The peer.
        peer: NodeId,
        /// What it said, with what to do about it.
        reason: String,
        /// The fact as the client API names it.
        fact: Box<Reason>,
    },
    /// A ledger rule refused what was asked of the job, such as a move to
    /// the owner itself or a steal by the owner or by a move's target; a
    /// client reads `fact`, and the sentence is the rule's.
    #[error("{error}")]
    LedgerFact {
        /// The rule's refusal.
        error: LedgerError,
        /// The fact as the client API names it.
        fact: Box<Reason>,
    },
    /// The preflight found rows that stop the move; the facts of those rows.
    #[error(
        "the preflight refused the move: {}; `fabric job move` and `fabric job resume` take --force to waive what can be waived",
        .0.join(", ")
    )]
    Preflight(Vec<String>),
    /// A steal without `--force`.
    #[error(
        "a steal would bump the epoch without {}'s consent: {described}. Pass --force to do it",
        warning.owner
    )]
    StealNeedsForce {
        /// What the steal would cost.
        warning: StealWarning,
        /// The same, in words.
        described: String,
    },
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
    /// The refusal a client gets for this error: its reason with its data,
    /// and this error's sentence (ADR 0018, Refusals).
    #[must_use]
    pub fn to_refusal(&self) -> Refusal {
        Refusal::new(self.reason(), self.to_string())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per error, so the whole mapping reads in one place"
    )]
    fn reason(&self) -> Reason {
        match self {
            NodeError::UnknownJob(job) => Reason::UnknownJob { job: *job },
            NodeError::NotOwner { job, owner, epoch } => Reason::NotOwner {
                job: *job,
                owner: *owner,
                epoch: *epoch,
            },
            NodeError::Departed { job, owner, epoch } => Reason::Departed {
                job: *job,
                owner: *owner,
                epoch: *epoch,
            },
            NodeError::Fence(FenceError::StaleEpoch { yours, current }) => Reason::StaleEpoch {
                yours: *yours,
                current: *current,
            },
            NodeError::Fence(FenceError::AheadOfUs { yours, current }) => Reason::EpochAhead {
                yours: *yours,
                current: *current,
            },
            NodeError::Fence(FenceError::WrongJob { token, job }) => Reason::WrongJob {
                token: *token,
                job: *job,
            },
            NodeError::Departing {
                job,
                move_id,
                target,
            } => Reason::Departing {
                job: *job,
                move_id: *move_id,
                target: *target,
            },
            NodeError::Forked { job, peer, epoch } => Reason::Forked {
                job: *job,
                peer: *peer,
                epoch: *epoch,
            },
            NodeError::ForkBehind { job, peer, epoch } => Reason::ForkBehind {
                job: *job,
                peer: *peer,
                epoch: *epoch,
            },
            NodeError::Unreachable { peer, .. } => Reason::Unreachable { peer: *peer },
            NodeError::UnknownPeer(node) => Reason::UnknownPeer { node: *node },
            NodeError::Preflight(facts) => Reason::Preflight {
                facts: facts.clone(),
            },
            NodeError::StealNeedsForce { warning, .. } => {
                Reason::StealNeedsForce { warning: *warning }
            }
            NodeError::AlreadyRunningHarness { job, pid } => Reason::HarnessRunning {
                job: *job,
                pid: *pid,
            },
            NodeError::HarnessGone { pid } => Reason::HarnessGone { pid: *pid },
            NodeError::HarnessNotRecorded { job, pid, session } => Reason::HarnessNotRecorded {
                job: *job,
                pid: *pid,
                session: session.clone(),
            },
            NodeError::RanDuringMove { job, target } => Reason::RanDuringMove {
                job: *job,
                target: *target,
            },
            NodeError::ChangedDuringMove { job, root, target } => Reason::ChangedDuringMove {
                job: *job,
                root: root.clone(),
                target: *target,
            },
            NodeError::StaleWorkspace { job, root, epoch } => Reason::StaleWorkspace {
                job: *job,
                root: root.clone(),
                left_at: *epoch,
            },
            NodeError::NoWorkspace(job) => Reason::NoWorkspace { job: *job },
            NodeError::Unfinished { job, step } => Reason::Unfinished {
                job: *job,
                step: *step,
            },
            NodeError::NoSnapshot(job) => Reason::NoSnapshot { job: *job },
            NodeError::WorkspaceOccupied { root } => {
                Reason::WorkspaceOccupied { root: root.clone() }
            }
            NodeError::BadAddress { text, .. } => Reason::BadAddress { text: text.clone() },
            NodeError::Peer { peer, .. } => Reason::Peer { peer: *peer },
            NodeError::PeerFact { fact, .. } | NodeError::LedgerFact { fact, .. } => {
                (**fact).clone()
            }
            NodeError::Paths(PathsError::SyncedRoot { root, folder, .. }) => Reason::SyncedRoot {
                root: root.clone(),
                folder: folder.clone(),
            },
            NodeError::Harness(HarnessError::SecretInArguments { program })
            | NodeError::HarnessConfig(HarnessConfigError::Adapter(
                HarnessError::SecretInArguments { program },
            )) => Reason::SecretInArguments {
                program: program.clone(),
            },
            NodeError::Fetch { snapshot, peer, .. } => Reason::Fetch {
                snapshot: *snapshot,
                peer: *peer,
            },
            NodeError::Io { .. }
            | NodeError::AlreadyRunning(_)
            | NodeError::SocketPathTooLong(_)
            | NodeError::Ledger(_)
            | NodeError::LedgerFile(_)
            | NodeError::Local(_)
            | NodeError::Paths(_)
            | NodeError::Store(_)
            | NodeError::Policy(_)
            | NodeError::Detect(_)
            | NodeError::Capture(_)
            | NodeError::Restore(_)
            | NodeError::Harness(_)
            | NodeError::HarnessConfig(_)
            | NodeError::Transport(_)
            | NodeError::Identity(_)
            | NodeError::PeerBook(_)
            | NodeError::Wire(_)
            | NodeError::Fault(_)
            | NodeError::Task(_) => Reason::Internal,
        }
    }

    /// A peer's answer that names `fact`, with `reason`, what it said.
    pub(crate) fn peer_fact(peer: NodeId, reason: String, fact: Reason) -> Self {
        NodeError::PeerFact {
            peer,
            reason,
            fact: Box::new(fact),
        }
    }

    /// A ledger rule's refusal `error`, which names `fact`.
    pub(crate) fn ledger_fact(error: LedgerError, fact: Reason) -> Self {
        NodeError::LedgerFact {
            error,
            fact: Box::new(fact),
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

/// What an unfinished step is, in the words of its refusal.
fn unfinished_what(step: Step) -> &'static str {
    match step {
        Step::Suspend => "a suspend",
        Step::Restore => "a restore",
    }
}

/// The command that finishes an unfinished step.
fn unfinished_command(step: Step) -> &'static str {
    match step {
        Step::Suspend => "suspend",
        Step::Restore => "resume",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use fabric_client::api::RefusalKind;

    use super::*;
    use crate::paths::{SyncedFolders, check_root};

    /// Review finding on PR #13: a root in a synced folder was refused as
    /// `internal`; the refusal names the root and the folder.
    #[test]
    fn a_root_in_a_synced_folder_is_refused_with_the_root_and_the_folder() {
        let home = Path::new("/Users/x");
        let icloud = home.join("Library/Mobile Documents");
        let root = icloud.join("w");
        let refused =
            NodeError::from(check_root(&root, home, SyncedFolders::default()).unwrap_err())
                .to_refusal();
        assert_eq!(
            refused.reason(),
            &Reason::SyncedRoot {
                root,
                folder: icloud,
            }
        );
        assert_eq!(refused.kind(), RefusalKind::Failed);
    }

    /// The same secret found by the adapter, when a suspend captures the
    /// state of an ACP harness a job already records, names the program as
    /// the refusal of a request that carries it does.
    #[test]
    fn a_secret_the_adapter_finds_in_acp_arguments_is_refused_with_the_program() {
        let refused = NodeError::from(HarnessError::SecretInArguments {
            program: "opencode".to_owned(),
        })
        .to_refusal();
        assert_eq!(
            refused.reason(),
            &Reason::SecretInArguments {
                program: "opencode".to_owned()
            }
        );
    }
}
