//! The control socket between the CLI and the daemon on one node.
//!
//! Invariant: the socket lives in the state directory, which only its owner
//! can enter (mode 0700), so a request on it comes from the user who runs
//! the daemon. One connection carries one request and one response, each a
//! `u32` little-endian length and that many postcard bytes, the length
//! checked against `MAX_CONTROL_MESSAGE` before anything is allocated. The daemon answers every request, refusals
//! included, with the lines it printed on the way, so every automatic
//! choice reaches the user with its reason (ADR 0008).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, NodeId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

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
        /// A harness to use instead of the job's.
        harness: Option<HarnessKind>,
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
    /// The CLI launched the harness; the daemon records it and appends `Resumed`.
    HarnessStarted {
        /// The job.
        job: JobId,
        /// The epoch the plan was made under.
        epoch: Epoch,
        /// The harness process, which leads its own group.
        pid: u32,
        /// The harness that runs.
        harness: HarnessKind,
        /// The session it continues or created, when known.
        session: Option<String>,
    },
    /// The harness exited.
    HarnessExited {
        /// The job.
        job: JobId,
        /// The process that exited.
        pid: u32,
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
        /// Approve, or reject.
        approve: bool,
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

/// What to launch, as the daemon decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlan {
    /// The job.
    pub job: JobId,
    /// The epoch this node owns the job at; the launch is fenced on it.
    pub epoch: Epoch,
    /// The workspace root.
    pub root: PathBuf,
    /// The harness to launch.
    pub harness: HarnessKind,
    /// The session to continue; a new one when `None`.
    pub session: Option<String>,
    /// The job's goal, the first message of a new session.
    pub goal: String,
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
        /// Why it stopped.
        error: String,
    },
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

/// Sends one request to the daemon at `socket` and waits for its answer.
pub async fn call(socket: &Path, request: &Request) -> Result<Response, ControlError> {
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
