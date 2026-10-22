//! The control socket between the CLI and the daemon on one node.
//!
//! Invariant: the socket lives in the state directory, which only its owner
//! can enter (mode 0700), so a request on it comes from the user who runs
//! the daemon. Neither side trusts that alone: the daemon refuses a peer
//! whose user id is not its own, and the CLI refuses a state directory or a
//! socket that another user owns or that others can enter, because the run
//! plan it reads there decides what it launches. One connection carries one request and one response, each a
//! `u32` little-endian length and that many postcard bytes, the length
//! checked against `MAX_CONTROL_MESSAGE` before anything is allocated. The daemon answers every request, refusals
//! included, with the lines it printed on the way, so every automatic
//! choice reaches the user with its reason (ADR 0008). A refusal carries
//! its kind next to its text, so the CLI picks its exit code from the kind
//! and never parses the text.

use std::ffi::OsString;
use std::num::NonZeroU32;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, NodeId, RecordHash};
use fabric_harness::SessionId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::harnesses::HarnessConfig;

/// The largest control message, either way.
pub const MAX_CONTROL_MESSAGE: usize = 4 << 20;

/// What the CLI asks the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// This node's id and listening address.
    NodeId,
    /// Pairs a peer, at an address (`host:port`) when this node can dial it.
    NodeAdd {
        /// The peer.
        node: NodeId,
        /// Where it listens; `None` for a peer behind a NAT that only dials in.
        addr: Option<String>,
    },
    /// The paired peers.
    NodeList,
    /// Dials a peer and pings it.
    NodePing {
        /// The peer.
        node: NodeId,
    },
    /// Makes a job from a repository, rooted at `~/fabric/jobs/<id>/work`.
    JobNew {
        /// The repository to clone.
        repo: PathBuf,
        /// The goal in the user's words.
        goal: String,
        /// The harness the job starts under.
        harness: HarnessKind,
    },
    /// Every job this node knows.
    JobList,
    /// One job's state on this node.
    JobStatus {
        /// The job.
        job: JobId,
    },
    /// One job's ledger.
    JobLog {
        /// The job.
        job: JobId,
    },
    /// Checks the job may run here and returns what to launch.
    JobRun {
        /// The job.
        job: JobId,
    },
    /// Pulls the job if another node owns it, restores it if needed, and returns what to launch.
    JobResume {
        /// The job.
        job: JobId,
        /// The node that holds the job, when this node has never seen it.
        on: Option<NodeId>,
        /// A harness to use instead of the job's.
        harness: Option<HarnessKind>,
        /// Waive the preflight refusals that can be waived.
        force: bool,
        /// The `PATH` of the user's shell.
        path_var: OsString,
    },
    /// Before any setup step, under the tree lock: whether the plan made at
    /// `epoch` still holds, so a launch whose plan another launch overtook
    /// refuses before it writes to the workspace.
    LaunchCheck {
        /// The job.
        job: JobId,
        /// The epoch the plan was made under.
        epoch: Epoch,
        /// The ledger head the plan was made at.
        head: RecordHash,
    },
    /// The CLI launched the harness; the daemon records it and appends `Resumed`.
    HarnessStarted {
        /// The job.
        job: JobId,
        /// The epoch the plan was made under.
        epoch: Epoch,
        /// The harness process, which leads its own group.
        pid: NonZeroU32,
        /// The harness that runs.
        harness: HarnessKind,
        /// The session it continues or created, when known.
        session: Option<SessionId>,
    },
    /// The harness exited.
    HarnessExited {
        /// The job.
        job: JobId,
        /// The process that exited.
        pid: NonZeroU32,
    },
    /// Stops the harness, snapshots the workspace and records the suspend.
    JobSuspend {
        /// The job.
        job: JobId,
        /// The `PATH` of the user's shell.
        path_var: OsString,
    },
    /// Moves the job to a peer by the cooperative move protocol.
    JobMove {
        /// The job.
        job: JobId,
        /// The target.
        to: NodeId,
        /// Waive the preflight refusals that can be waived.
        force: bool,
        /// The `PATH` of the user's shell.
        path_var: OsString,
    },
    /// Takes the job without the owner's consent (ADR 0003, The steal).
    JobSteal {
        /// The job.
        job: JobId,
        /// The user's reason.
        reason: String,
        /// Without it, the steal only prints what it would do.
        force: bool,
    },
    /// Records a decision in the ledger.
    JobDecide {
        /// The job.
        job: JobId,
        /// What the user decided.
        decision: Decision,
        /// The user's words, if any.
        note: String,
    },
    /// The preflight: this node's facts, and the comparison with a peer's.
    Doctor {
        /// The peer to compare with.
        peer: Option<NodeId>,
        /// The job a move would carry, for the rows that depend on it.
        job: Option<JobId>,
        /// The `PATH` of the user's shell.
        path_var: OsString,
    },
}

/// A user's decision on a job's work (ADR 0016, Decisions as ledger events).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// `fabric job approve`.
    Approve,
    /// `fabric job reject`.
    Reject,
}

/// What to launch, as the daemon decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlan {
    /// The job.
    pub job: JobId,
    /// The epoch this node owns the job at; the launch is fenced on it.
    pub epoch: Epoch,
    /// The ledger head the plan was made at; a launch whose job moved past
    /// it, by another launch for one, is refused before setup.
    pub head: RecordHash,
    /// The workspace root.
    pub root: PathBuf,
    /// The harness to launch.
    pub harness: HarnessKind,
    /// The session to continue; a new one when `None`.
    pub session: Option<SessionId>,
    /// The job's goal, the first message of a new session.
    pub goal: String,
    /// The node's harness configuration, which the launch must use.
    pub harness_config: HarnessConfig,
    /// The home the node captures and restores harness state under; a
    /// launch under another home would keep the session where the node
    /// never looks.
    pub home: PathBuf,
}

/// The daemon's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    /// Done; print the lines.
    Done {
        /// What happened, one line per step or choice.
        lines: Vec<String>,
    },
    /// Launch this; print the lines first.
    Run {
        /// What happened, one line per step or choice.
        lines: Vec<String>,
        /// What to launch.
        plan: RunPlan,
    },
    /// Refused or failed; print the lines, then the error.
    Failed {
        /// What happened before the failure.
        lines: Vec<String>,
        /// Why it stopped, in words.
        error: String,
        /// What kind of refusal it is.
        refusal: Refusal,
    },
}

/// The kind of a refusal or failure, which decides the CLI's exit code (ADR 0016, Exit codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// Another node owns the job, or this node does not know it.
    NotOwner,
    /// A move this node started is open, or the job is frozen after a fork.
    Frozen,
    /// A peer could not be reached, or gave no answer in time.
    Unreachable,
    /// The preflight refused, or a steal was asked without `--force`.
    NeedsForce,
    /// The job is not in a state the command acts on: a suspend or a
    /// restore did not finish, a harness runs, or there is no snapshot.
    JobState,
    /// Anything else: a file, the store, a harness adapter, a peer's answer.
    Failed,
}

/// The exit code of a command that found no daemon to answer it.
pub const NO_DAEMON_EXIT: u8 = 80;

impl Refusal {
    /// The exit code the CLI ends with for this refusal.
    #[must_use]
    pub fn exit_code(self) -> u8 {
        match self {
            Refusal::NotOwner => 81,
            Refusal::Frozen => 82,
            Refusal::Unreachable => 83,
            Refusal::NeedsForce => 84,
            Refusal::JobState => 85,
            Refusal::Failed => 1,
        }
    }
}

/// Why the CLI could not talk to the daemon.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// No daemon listens on the socket.
    #[error("no fabric daemon at {path} ({source}); start one with `fabric serve`")]
    NoDaemon {
        /// The socket.
        path: PathBuf,
        /// Why the connect failed.
        #[source]
        source: std::io::Error,
    },
    /// The state directory or the socket is not this user's alone.
    #[error("{path} {reason}, so its answers are not trusted; it must be yours and mode 0700")]
    Untrusted {
        /// The directory or socket.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// The exchange failed midway.
    #[error("control socket: {0}")]
    Io(#[from] std::io::Error),
    /// A message declares more bytes than `MAX_CONTROL_MESSAGE`.
    #[error("control message of {0} bytes exceeds the cap")]
    TooLarge(usize),
    /// A message did not encode or decode.
    #[error("control socket: {0}")]
    Codec(#[from] postcard::Error),
    /// A message decoded with bytes left over.
    #[error("control message has {0} trailing bytes")]
    Trailing(usize),
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ControlError> {
    Ok(postcard::to_stdvec(value)?)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ControlError> {
    let (value, rest) = postcard::take_from_bytes::<T>(bytes)?;
    if !rest.is_empty() {
        return Err(ControlError::Trailing(rest.len()));
    }
    Ok(value)
}

async fn write_message(stream: &mut UnixStream, payload: &[u8]) -> Result<(), ControlError> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|len| *len as usize <= MAX_CONTROL_MESSAGE)
        .ok_or(ControlError::TooLarge(payload.len()))?;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(payload).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn read_message(stream: &mut UnixStream) -> Result<Vec<u8>, ControlError> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_CONTROL_MESSAGE {
        return Err(ControlError::TooLarge(len));
    }
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

/// Refuses `path` unless this user owns it and, for a directory, no one else may enter it.
fn check_private(path: &Path) -> Result<(), ControlError> {
    let untrusted = |reason: String| ControlError::Untrusted {
        path: path.to_owned(),
        reason,
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(ControlError::Io(e)),
    };
    let me = rustix::process::geteuid().as_raw();
    if meta.uid() != me {
        return Err(untrusted(format!(
            "belongs to user {}, not to you ({me})",
            meta.uid()
        )));
    }
    if meta.file_type().is_symlink() {
        return Err(untrusted("is a symlink".to_owned()));
    }
    if meta.is_dir() && meta.mode() & 0o077 != 0 {
        return Err(untrusted(format!(
            "has mode {:o}, which lets others in",
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

/// Sends one request to the daemon at `socket` and waits for its answer.
///
/// The socket's directory and the socket must be this user's alone; a
/// missing one is the daemon not running, which the connect reports.
pub async fn call(socket: &Path, request: &Request) -> Result<Response, ControlError> {
    if let Some(dir) = socket.parent() {
        check_private(dir)?;
    }
    check_private(socket)?;
    let mut stream =
        UnixStream::connect(socket)
            .await
            .map_err(|source| ControlError::NoDaemon {
                path: socket.to_owned(),
                source,
            })?;
    write_message(&mut stream, &encode(request)?).await?;
    decode(&read_message(&mut stream).await?)
}

/// Reads one request from a connection the daemon accepted.
pub(crate) async fn read_request(stream: &mut UnixStream) -> Result<Request, ControlError> {
    decode(&read_message(stream).await?)
}

/// Writes the answer to a connection the daemon accepted.
pub(crate) async fn write_response(
    stream: &mut UnixStream,
    response: &Response,
) -> Result<(), ControlError> {
    write_message(stream, &encode(response)?).await
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_state_directory_others_can_enter_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            check_private(dir.path()),
            Err(ControlError::Untrusted { .. })
        ));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        check_private(dir.path()).unwrap();
    }

    /// A socket path that is a symlink could lead anywhere, such as to a
    /// socket another user serves run plans on.
    #[test]
    fn a_socket_that_is_a_symlink_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("fabric.sock");
        std::os::unix::fs::symlink(dir.path().join("elsewhere.sock"), &socket).unwrap();
        assert!(matches!(
            check_private(&socket),
            Err(ControlError::Untrusted { .. })
        ));
    }
}
