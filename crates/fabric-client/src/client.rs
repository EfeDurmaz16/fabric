//! A client of one local node.
//!
//! Invariant: nothing is sent before the state directory and the socket
//! prove to be this user's alone, because a run plan read from the socket
//! decides what the caller launches (ADR 0016). Every call is one
//! connection: a hello, the node's welcome, one request, the node's events
//! as they happen and one final answer, whose kind must be the request's op.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, NodeId};
use fabric_core::session::SessionId;
use tokio::net::UnixStream;

use crate::api::{
    API_VERSION, Answer, Decision, DoctorReport, Event, Hello, HelloAnswer, JobCreated, JobList,
    JobLog, JobStatus, Lease, NodeIdentity, Op, Paired, PeerList, Pong, Refusal, Registered, Reply,
    Request, RunPlan, SessionRecorded, Suspended, Welcome,
};
use crate::frame::{self, FrameError};

/// Why a call did not get its reply.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No node listens on the socket.
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
    /// A frame could not be written or read.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The node speaks another api version.
    #[error(
        "the node speaks client api {node} (fabric {node_version}) and this client speaks {client}; use a client and a node from the same release"
    )]
    Incompatible {
        /// The client's api version.
        client: u32,
        /// The node's api version.
        node: u32,
        /// The node's software version.
        node_version: String,
    },
    /// The node does not serve the op.
    #[error("the node does not serve `{op}`; it runs an older fabric")]
    Unsupported {
        /// The op.
        op: Op,
    },
    /// The node closed the connection before it welcomed the client.
    #[error(
        "the node at {path} closed the connection before it answered the hello; it may run a fabric older than the client API"
    )]
    ClosedBeforeWelcome {
        /// The socket.
        path: PathBuf,
    },
    /// The node closed the connection before the final answer.
    #[error("the node closed the connection before it answered `{op}`")]
    ClosedBeforeAnswer {
        /// The op asked.
        op: Op,
    },
    /// The node answered a request with the reply of another op.
    #[error("the node answered `{op}` with the reply of `{got}`")]
    Unexpected {
        /// The op asked.
        op: Op,
        /// The op the reply belongs to.
        got: Op,
    },
    /// The node refused or failed the command.
    #[error("{0}")]
    Refused(Refusal),
}

/// A client of the node whose control socket is at a path.
#[derive(Debug, Clone)]
pub struct Client {
    socket: PathBuf,
    name: String,
}

impl Client {
    /// A client of the node listening at `socket`, `<state>/fabric.sock`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            name: format!("fabric-client/{}", env!("CARGO_PKG_VERSION")),
        }
    }

    /// The same client, naming itself `name` in its hello, for the node's log.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// The socket this client calls.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Connects, checks the api version and returns what the node says about itself.
    pub async fn hello(&self) -> Result<Welcome, ClientError> {
        Ok(self.connect().await?.1)
    }

    /// Sends `request` and returns its reply, handing each event to `on_event` as it arrives.
    ///
    /// An event of a kind this client does not know is skipped.
    pub async fn call<F: FnMut(Event)>(
        &self,
        request: &Request,
        mut on_event: F,
    ) -> Result<Reply, ClientError> {
        let op = request.op();
        let (mut stream, welcome) = self.connect().await?;
        if !welcome.serves(op) {
            return Err(ClientError::Unsupported { op });
        }
        frame::write(&mut stream, request).await?;
        loop {
            match frame::read::<_, Answer>(&mut stream).await? {
                None => return Err(ClientError::ClosedBeforeAnswer { op }),
                Some(Answer::Event {
                    event: Event::Unrecognized,
                }) => {}
                Some(Answer::Event { event }) => on_event(event),
                Some(Answer::Done { reply }) if reply.op() == op => return Ok(reply),
                Some(Answer::Done { reply }) => {
                    return Err(ClientError::Unexpected {
                        op,
                        got: reply.op(),
                    });
                }
                Some(Answer::Refused { refusal }) => return Err(ClientError::Refused(refusal)),
            }
        }
    }

    async fn connect(&self) -> Result<(UnixStream, Welcome), ClientError> {
        if let Some(dir) = self.socket.parent() {
            check_private(dir)?;
        }
        check_private(&self.socket)?;
        let mut stream =
            UnixStream::connect(&self.socket)
                .await
                .map_err(|source| ClientError::NoDaemon {
                    path: self.socket.clone(),
                    source,
                })?;
        let hello = Hello {
            api: API_VERSION,
            client: self.name.clone(),
        };
        frame::write(&mut stream, &hello).await?;
        let answer = match frame::read::<_, HelloAnswer>(&mut stream).await {
            Ok(Some(answer)) => answer,
            // An older node reads the hello as a postcard request it cannot
            // decode and closes, sometimes before it reads all of it.
            Ok(None) | Err(FrameError::Io(_)) => {
                return Err(ClientError::ClosedBeforeWelcome {
                    path: self.socket.clone(),
                });
            }
            Err(e) => return Err(e.into()),
        };
        match answer {
            HelloAnswer::Welcome(welcome) if welcome.api == API_VERSION => Ok((stream, welcome)),
            HelloAnswer::Welcome(Welcome {
                api, node_version, ..
            })
            | HelloAnswer::Incompatible { api, node_version } => Err(ClientError::Incompatible {
                client: API_VERSION,
                node: api,
                node_version,
            }),
        }
    }

    /// This node's id and listening address.
    pub async fn node_id(&self, on_event: impl FnMut(Event)) -> Result<NodeIdentity, ClientError> {
        match self.call(&Request::NodeId, on_event).await? {
            Reply::NodeId(reply) => Ok(reply),
            other => Err(unexpected(Op::NodeId, &other)),
        }
    }

    /// Pairs a peer, at `addr` (`host:port`) when this node can dial it.
    pub async fn node_add(
        &self,
        node: NodeId,
        addr: Option<String>,
        on_event: impl FnMut(Event),
    ) -> Result<Paired, ClientError> {
        match self
            .call(&Request::NodeAdd { node, addr }, on_event)
            .await?
        {
            Reply::NodeAdd(reply) => Ok(reply),
            other => Err(unexpected(Op::NodeAdd, &other)),
        }
    }

    /// The paired peers.
    pub async fn node_list(&self, on_event: impl FnMut(Event)) -> Result<PeerList, ClientError> {
        match self.call(&Request::NodeList, on_event).await? {
            Reply::NodeList(reply) => Ok(reply),
            other => Err(unexpected(Op::NodeList, &other)),
        }
    }

    /// Dials a peer and pings it.
    pub async fn node_ping(
        &self,
        node: NodeId,
        on_event: impl FnMut(Event),
    ) -> Result<Pong, ClientError> {
        match self.call(&Request::NodePing { node }, on_event).await? {
            Reply::NodePing(reply) => Ok(reply),
            other => Err(unexpected(Op::NodePing, &other)),
        }
    }

    /// Makes a job from the repository at `repo`, an absolute path on the node.
    pub async fn job_new(
        &self,
        repo: PathBuf,
        goal: String,
        harness: HarnessKind,
        on_event: impl FnMut(Event),
    ) -> Result<JobCreated, ClientError> {
        let request = Request::JobNew {
            repo,
            goal,
            harness,
        };
        match self.call(&request, on_event).await? {
            Reply::JobNew(reply) => Ok(reply),
            other => Err(unexpected(Op::JobNew, &other)),
        }
    }

    /// Every job the node knows.
    pub async fn job_list(&self, on_event: impl FnMut(Event)) -> Result<JobList, ClientError> {
        match self.call(&Request::JobList, on_event).await? {
            Reply::JobList(reply) => Ok(reply),
            other => Err(unexpected(Op::JobList, &other)),
        }
    }

    /// One job's state on the node.
    pub async fn job_status(
        &self,
        job: JobId,
        on_event: impl FnMut(Event),
    ) -> Result<JobStatus, ClientError> {
        match self.call(&Request::JobStatus { job }, on_event).await? {
            Reply::JobStatus(reply) => Ok(reply),
            other => Err(unexpected(Op::JobStatus, &other)),
        }
    }

    /// One job's ledger.
    pub async fn job_log(
        &self,
        job: JobId,
        on_event: impl FnMut(Event),
    ) -> Result<JobLog, ClientError> {
        match self.call(&Request::JobLog { job }, on_event).await? {
            Reply::JobLog(reply) => Ok(reply),
            other => Err(unexpected(Op::JobLog, &other)),
        }
    }

    /// Checks the job may run on the node and returns what to launch.
    pub async fn job_run(
        &self,
        job: JobId,
        on_event: impl FnMut(Event),
    ) -> Result<RunPlan, ClientError> {
        match self.call(&Request::JobRun { job }, on_event).await? {
            Reply::JobRun(plan) => Ok(plan),
            other => Err(unexpected(Op::JobRun, &other)),
        }
    }

    /// Pulls the job if another node owns it, restores it, and returns what to launch.
    pub async fn job_resume(
        &self,
        resume: Resume,
        on_event: impl FnMut(Event),
    ) -> Result<RunPlan, ClientError> {
        let Resume {
            job,
            on,
            harness,
            force,
            path,
        } = resume;
        let request = Request::JobResume {
            job,
            on,
            harness,
            force,
            path,
        };
        match self.call(&request, on_event).await? {
            Reply::JobResume(plan) => Ok(plan),
            other => Err(unexpected(Op::JobResume, &other)),
        }
    }

    /// Stops the harness, snapshots the workspace and records the suspend.
    pub async fn job_suspend(
        &self,
        job: JobId,
        path: Option<String>,
        on_event: impl FnMut(Event),
    ) -> Result<Suspended, ClientError> {
        match self
            .call(&Request::JobSuspend { job, path }, on_event)
            .await?
        {
            Reply::JobSuspend(reply) => Ok(reply),
            other => Err(unexpected(Op::JobSuspend, &other)),
        }
    }

    /// Moves the job to the peer `to` and returns the lease once the move is decided.
    pub async fn job_move(
        &self,
        job: JobId,
        to: NodeId,
        force: bool,
        path: Option<String>,
        on_event: impl FnMut(Event),
    ) -> Result<Lease, ClientError> {
        let request = Request::JobMove {
            job,
            to,
            force,
            path,
        };
        match self.call(&request, on_event).await? {
            Reply::JobMove(reply) => Ok(reply),
            other => Err(unexpected(Op::JobMove, &other)),
        }
    }

    /// Takes the job without its owner's consent; without `force` the node refuses with the cost.
    pub async fn job_steal(
        &self,
        job: JobId,
        reason: String,
        force: bool,
        on_event: impl FnMut(Event),
    ) -> Result<Lease, ClientError> {
        let request = Request::JobSteal { job, reason, force };
        match self.call(&request, on_event).await? {
            Reply::JobSteal(reply) => Ok(reply),
            other => Err(unexpected(Op::JobSteal, &other)),
        }
    }

    /// Records an approval or a rejection, with the user's note, in the job's ledger.
    pub async fn job_decide(
        &self,
        job: JobId,
        decision: Decision,
        note: String,
        on_event: impl FnMut(Event),
    ) -> Result<String, ClientError> {
        let request = Request::JobDecide {
            job,
            decision,
            note,
        };
        match self.call(&request, on_event).await? {
            Reply::JobDecide(reply) => Ok(reply.text),
            other => Err(unexpected(Op::JobDecide, &other)),
        }
    }

    /// The node's facts, or, with `peer`, their comparison with the peer's.
    pub async fn doctor(
        &self,
        peer: Option<NodeId>,
        job: Option<JobId>,
        path: Option<String>,
        on_event: impl FnMut(Event),
    ) -> Result<DoctorReport, ClientError> {
        match self
            .call(&Request::Doctor { peer, job, path }, on_event)
            .await?
        {
            Reply::Doctor { report } => Ok(report),
            other => Err(unexpected(Op::Doctor, &other)),
        }
    }

    /// Tells the node the planned harness runs as `launch.pid`, continuing
    /// or having created `session` when it is known, so the node records it
    /// and appends `Resumed`.
    pub async fn harness_started(
        &self,
        launch: Launched,
        session: Option<SessionId>,
        on_event: impl FnMut(Event),
    ) -> Result<Registered, ClientError> {
        let Launched {
            job,
            epoch,
            pid,
            harness,
        } = launch;
        let request = Request::HarnessStarted {
            job,
            epoch,
            pid,
            harness,
            session,
        };
        match self.call(&request, on_event).await? {
            Reply::HarnessStarted(reply) => Ok(reply),
            other => Err(unexpected(Op::HarnessStarted, &other)),
        }
    }

    /// Tells the node the running harness named `session`.
    pub async fn harness_session(
        &self,
        launch: Launched,
        session: SessionId,
        on_event: impl FnMut(Event),
    ) -> Result<SessionRecorded, ClientError> {
        let request = Request::HarnessSession {
            job: launch.job,
            epoch: launch.epoch,
            pid: launch.pid,
            harness: launch.harness,
            session,
        };
        match self.call(&request, on_event).await? {
            Reply::HarnessSession(reply) => Ok(reply),
            other => Err(unexpected(Op::HarnessSession, &other)),
        }
    }

    /// Tells the node the harness `pid` of `job` exited.
    pub async fn harness_exited(
        &self,
        job: JobId,
        pid: std::num::NonZeroU32,
        on_event: impl FnMut(Event),
    ) -> Result<(), ClientError> {
        match self
            .call(&Request::HarnessExited { job, pid }, on_event)
            .await?
        {
            Reply::HarnessExited => Ok(()),
            other => Err(unexpected(Op::HarnessExited, &other)),
        }
    }
}

/// The arguments of [`Client::job_resume`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    /// The job.
    pub job: JobId,
    /// The node that holds the job, when this node has never seen it.
    pub on: Option<NodeId>,
    /// A harness to use instead of the job's.
    pub harness: Option<HarnessKind>,
    /// Waive the preflight refusals that can be waived.
    pub force: bool,
    /// The `PATH` of the caller's shell; the node's own when `None`.
    pub path: Option<String>,
}

/// A harness the caller launched from a plan, for [`Client::harness_started`]
/// and [`Client::harness_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    /// The job.
    pub job: JobId,
    /// The epoch the plan was made under.
    pub epoch: Epoch,
    /// The harness process, which leads its own process group.
    pub pid: std::num::NonZeroU32,
    /// The harness that runs.
    pub harness: HarnessKind,
}

fn unexpected(op: Op, reply: &Reply) -> ClientError {
    ClientError::Unexpected {
        op,
        got: reply.op(),
    }
}

/// Refuses `path` unless this user owns it and, for a directory, no one else may enter it.
///
/// A missing path passes: the connect that follows reports it as no daemon.
fn check_private(path: &Path) -> Result<(), ClientError> {
    let untrusted = |reason: String| ClientError::Untrusted {
        path: path.to_owned(),
        reason,
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(FrameError::Io(e).into()),
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
            Err(ClientError::Untrusted { .. })
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
            Err(ClientError::Untrusted { .. })
        ));
    }
}
