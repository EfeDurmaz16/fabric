//! Request and response over per-request streams (ADR 0005).
//!
//! Invariant: a session speaks for the application only after both nodes
//! exchanged `Hello` and agreed on the protocol version and on who the peer
//! is; every call carries a deadline that fires on the caller whatever the
//! peer does; and the caller trusts a chunk only after its bytes hash to
//! the id it asked for. One stream carries one request and its answer, so
//! a slow answer never holds up another. A serving session answers only a
//! peer that is still paired: it checks the trust decision before each
//! request and asks it again every `TRUST_RECHECK`, and closes the
//! connection once the peer is unpaired, however long the connection lives.
//!
//! The wire types `RequestBody`, `ResponseBody` and `JobResponse` are built
//! and matched here only. A caller asks through one typed method per
//! request, per-job requests included, and a `Service` answers through one
//! typed method per request, each with its own answer type, so neither can
//! pair a request with an answer of the wrong kind: a service cannot build
//! one, and a caller refuses one as `RpcError::UnexpectedResponse`. `Hello`
//! and `Ping` belong to this layer and never reach a service.

use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::{ChunkId, Epoch, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::FencingToken;
use fabric_core::moves::MoveOffer;
use fabric_core::snapshot::MAX_CHUNK_LEN;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::accept::refresh;
use crate::frame::{FrameError, MAX_BULK_FRAME, MAX_CONTROL_FRAME, read_frame, write_frame};
use crate::idle::IdleReader;
use crate::tls::Trust;
use crate::transport::{Connection, Stream, TransportError, TransportKind};
use crate::wire::{
    Appended, ChunkBatch, Fault, FaultText, Fenced, Hello, JobAnswer, JobRequest, JobResponse,
    ManifestChunks, MoveRequested, MoveState, Offered, PROTOCOL_VERSION, Pulled, RecordBytes,
    RecordPage, Request, RequestBody, RequestId, Response, ResponseBody, VersionText, WireError,
    decode, encode, wall_clock_ms,
};

/// A clock difference between two nodes above this is logged at `Hello`.
pub const CLOCK_SKEW_WARN_MS: u64 = 10_000;

/// How often a serving session re-reads the trust decision, so an unpair ends a connection that stays open.
pub const TRUST_RECHECK: Duration = Duration::from_secs(5);

/// How long a peer may take to send one whole request on a stream it opened.
///
/// A request is at most one control frame, so this allows a link of about
/// 35 KiB/s, and a peer that sends part of a frame and stops gives its
/// stream back.
pub const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a call failed.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// The connection or stream failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A frame could not be read or written.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// A message did not decode or encode.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// No answer arrived within the deadline.
    #[error("no answer within {0:?}")]
    Deadline(Duration),
    /// The peer speaks another protocol version.
    #[error("peer speaks protocol {theirs}, this node speaks {ours}")]
    ProtocolVersion {
        /// This node's version.
        ours: u16,
        /// The peer's version.
        theirs: u16,
    },
    /// The peer's `Hello` names a node other than the one the handshake proved.
    #[error("the hello claims {claimed} but the handshake proved {proved}")]
    HelloMismatch {
        /// The id the TLS handshake proved.
        proved: NodeId,
        /// The id the `Hello` claims.
        claimed: NodeId,
    },
    /// The first request of a session was something other than `Hello`.
    #[error("the first request of the session was not a hello")]
    NotAHello,
    /// The response carries another request number than the one sent.
    #[error("response {got} answers another request than {sent}")]
    WrongRequestId {
        /// The number sent.
        sent: RequestId,
        /// The number received.
        got: RequestId,
    },
    /// The peer answered with a body of the wrong kind.
    #[error("expected {expected}, got {got}")]
    UnexpectedResponse {
        /// The kind expected.
        expected: &'static str,
        /// The kind received.
        got: &'static str,
    },
    /// The chunk does not decompress, within `MAX_CHUNK_LEN`, to bytes that hash to its id; it is dropped.
    #[error("chunk {id} did not decompress to bytes that hash to its id")]
    ChunkMismatch {
        /// The id claimed.
        id: ChunkId,
    },
    /// The peer sent a chunk that was not asked for, or sent one twice.
    #[error("chunk {id} was not asked for or already arrived")]
    UnrequestedChunk {
        /// The id sent.
        id: ChunkId,
    },
    /// The peer said it was done while chunks were still missing.
    #[error("the peer stopped with {missing} chunks still missing")]
    Incomplete {
        /// How many asked chunks never arrived.
        missing: usize,
    },
    /// The peer understood the request and could not serve it.
    #[error("the peer could not serve the request: {0}")]
    Fault(Fault),
    /// An earlier error ended this chunk stream; ask for `missing` in a new request.
    #[error("the chunk stream already failed; ask for the missing chunks again")]
    Interrupted,
    /// The peer is no longer paired, so the serving side closed the connection.
    #[error("{0} is no longer paired")]
    Unpaired(NodeId),
}

/// A per-job answer from a service, or why it could not serve.
pub type JobFuture<'a, T> = BoxFuture<'a, Result<Fenced<T>, Fault>>;

/// What a node serves to its peers: per-job messages, manifests and chunks.
///
/// Every per-job method answers a message from `peer` sent under the
/// fencing `token`, and runs the ADR 0003 epoch check before it reads the
/// rest. A per-job message can write ledger records, so its future always
/// runs to its end, even after the caller's budget ran out and the caller
/// was told `Fault::Deadline`. The caller then treats the outcome as
/// unknown and asks again, with `MoveStatus` for a move.
pub trait Service: Send + Sync {
    /// Answers `Pull`: the records after `after`, or from the first record when it is `None`, one page at a time.
    fn pull(
        &self,
        peer: NodeId,
        token: FencingToken,
        after: Option<RecordHash>,
    ) -> JobFuture<'_, Pulled>;
    /// Answers `Append`: links `records` to this node's ledger, or says where its head is.
    fn append(
        &self,
        peer: NodeId,
        token: FencingToken,
        records: RecordPage,
    ) -> JobFuture<'_, Appended>;
    /// Answers `MoveRequest`: starts a move of the job toward `peer`, or says why not.
    fn move_request(&self, peer: NodeId, token: FencingToken) -> JobFuture<'_, MoveRequested>;
    /// Answers `MoveOffer`: `offer` names the job the token names.
    fn offer(&self, peer: NodeId, token: FencingToken, offer: MoveOffer) -> JobFuture<'_, Offered>;
    /// Answers `MoveStatus`: this node's decision on `move_id`, if it made one.
    fn move_status(
        &self,
        peer: NodeId,
        token: FencingToken,
        move_id: MoveId,
    ) -> JobFuture<'_, MoveState>;
    /// Answers `MoveAbort`: decides against `move_id` unless already decided, and returns the decision record.
    fn move_abort(
        &self,
        peer: NodeId,
        token: FencingToken,
        move_id: MoveId,
    ) -> JobFuture<'_, RecordBytes>;
    /// Returns the length and chunk list of `snapshot`'s manifest for `peer`.
    fn manifest(
        &self,
        peer: NodeId,
        snapshot: SnapshotId,
    ) -> BoxFuture<'_, Result<ManifestChunks, Fault>>;
    /// Returns one chunk for `peer` as the store keeps it: one zstd frame of its bytes (ADR 0004).
    ///
    /// It goes out as is; the receiver decompresses and checks it.
    /// `Fault::UnknownChunk` says this node does not hold it; a chunk it
    /// holds and could not read is `Fault::Internal`, so the peer never
    /// mistakes a failed read for a missing chunk.
    fn chunk(&self, peer: NodeId, id: ChunkId) -> BoxFuture<'_, Result<Vec<u8>, Fault>>;
}

/// An authenticated connection after the `Hello` exchange.
pub struct Session {
    connection: Arc<dyn Connection>,
    local: Hello,
    remote: Hello,
    clock_skew_ms: i64,
    next_id: AtomicU64,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("peer", &self.remote.node)
            .field("kind", &self.connection.kind())
            .finish_non_exhaustive()
    }
}

fn budget(deadline: Duration) -> u32 {
    u32::try_from(deadline.as_millis()).unwrap_or(u32::MAX)
}

fn unexpected(expected: &'static str, got: &ResponseBody) -> RpcError {
    RpcError::UnexpectedResponse {
        expected,
        got: kind_of(got),
    }
}

fn check_id(sent: RequestId, got: RequestId) -> Result<(), RpcError> {
    if sent == got {
        Ok(())
    } else {
        Err(RpcError::WrongRequestId { sent, got })
    }
}

fn kind_of(body: &ResponseBody) -> &'static str {
    match body {
        ResponseBody::Hello(_) => "hello",
        ResponseBody::Pong => "pong",
        ResponseBody::Job(_) => "job",
        ResponseBody::ManifestChunks(_) => "manifest chunks",
        ResponseBody::Chunk { .. } => "chunk",
        ResponseBody::ChunksDone => "chunks done",
        ResponseBody::Fault(_) => "fault",
    }
}

async fn send_request(
    connection: &dyn Connection,
    request: &Request,
) -> Result<Box<dyn AsyncRead + Send + Unpin>, RpcError> {
    let (mut send, recv) = connection.open().await?.split();
    write_frame(&mut send, &encode(request)?, MAX_CONTROL_FRAME).await?;
    send.shutdown().await.map_err(FrameError::Io)?;
    Ok(recv)
}

async fn read_response(
    recv: &mut (dyn AsyncRead + Send + Unpin),
    id: RequestId,
    max: usize,
) -> Result<ResponseBody, RpcError> {
    let bytes = read_frame(recv, max).await?;
    let response: Response = decode(&bytes)?;
    check_id(id, response.id)?;
    Ok(response.body)
}

/// This node's `Hello` for this protocol version, with the clock read now.
fn hello(local: NodeId, fabric_version: VersionText) -> Hello {
    Hello {
        node: local,
        protocol_version: PROTOCOL_VERSION,
        fabric_version,
        clock_ms: wall_clock_ms(),
    }
}

impl Session {
    /// Runs the dialer's half of the handshake: sends this node's `Hello`, expects the peer's.
    ///
    /// `local` is this node's id; the hello's clock is read as it is sent.
    pub async fn connect(
        connection: Arc<dyn Connection>,
        local: NodeId,
        fabric_version: VersionText,
        deadline: Duration,
    ) -> Result<Session, RpcError> {
        let local = hello(local, fabric_version);
        let request = Request {
            id: RequestId(0),
            budget_ms: budget(deadline),
            body: RequestBody::Hello(local.clone()),
        };
        let exchange = async {
            let mut recv = send_request(connection.as_ref(), &request).await?;
            match read_response(recv.as_mut(), RequestId(0), MAX_CONTROL_FRAME).await? {
                ResponseBody::Hello(remote) => Ok((remote, wall_clock_ms())),
                ResponseBody::Fault(fault) => Err(RpcError::Fault(fault)),
                other => Err(unexpected("hello", &other)),
            }
        };
        let (remote, received_ms) = timeout(deadline, exchange)
            .await
            .map_err(|_| RpcError::Deadline(deadline))??;
        Self::finish(connection, local, remote, received_ms)
    }

    /// Runs the acceptor's half of the handshake: expects the peer's `Hello` first, answers with this node's.
    ///
    /// `local` is this node's id; the hello's clock is read as it is sent.
    pub async fn accept(
        connection: Arc<dyn Connection>,
        local: NodeId,
        fabric_version: VersionText,
        deadline: Duration,
    ) -> Result<Session, RpcError> {
        let exchange = async {
            let (mut send, mut recv) = connection.next().await?.split();
            let bytes = read_frame(&mut recv, MAX_CONTROL_FRAME).await?;
            let request: Request = decode(&bytes)?;
            let local = hello(local, fabric_version);
            let received_ms = local.clock_ms;
            let (body, remote) = match request.body {
                RequestBody::Hello(remote) => (ResponseBody::Hello(local.clone()), Some(remote)),
                _ => (
                    ResponseBody::Fault(Fault::BadRequest(FaultText::truncated(
                        "the first request must be a hello",
                    ))),
                    None,
                ),
            };
            reply(send.as_mut(), request.id, body, MAX_CONTROL_FRAME).await?;
            send.shutdown().await.map_err(FrameError::Io)?;
            let remote = remote.ok_or(RpcError::NotAHello)?;
            Ok::<_, RpcError>((local, remote, received_ms))
        };
        let (local, remote, received_ms) = timeout(deadline, exchange)
            .await
            .map_err(|_| RpcError::Deadline(deadline))??;
        Self::finish(connection, local, remote, received_ms)
    }

    /// Checks the peer's `Hello` against the handshake; the skew is its clock against ours when it arrived.
    fn finish(
        connection: Arc<dyn Connection>,
        local: Hello,
        remote: Hello,
        received_ms: u64,
    ) -> Result<Session, RpcError> {
        let proved = connection.peer();
        if remote.node != proved {
            connection.close();
            return Err(RpcError::HelloMismatch {
                proved,
                claimed: remote.node,
            });
        }
        if remote.protocol_version != PROTOCOL_VERSION {
            connection.close();
            return Err(RpcError::ProtocolVersion {
                ours: PROTOCOL_VERSION,
                theirs: remote.protocol_version,
            });
        }
        let theirs = i64::try_from(remote.clock_ms).unwrap_or(i64::MAX);
        let ours = i64::try_from(received_ms).unwrap_or(i64::MAX);
        let session = Session {
            connection,
            local,
            remote,
            clock_skew_ms: theirs.saturating_sub(ours),
            next_id: AtomicU64::new(1),
        };
        if session.clock_skew_ms.unsigned_abs() > CLOCK_SKEW_WARN_MS {
            tracing::warn!(peer = %session.peer(), skew_ms = session.clock_skew_ms, "peer clock differs");
        }
        tracing::info!(peer = %session.peer(), kind = %session.kind(), "session open");
        Ok(session)
    }

    /// The peer's node id, proved by the handshake and confirmed by its `Hello`.
    #[must_use]
    pub fn peer(&self) -> NodeId {
        self.remote.node
    }

    /// The peer's `Hello`.
    #[must_use]
    pub fn remote(&self) -> &Hello {
        &self.remote
    }

    /// This node's `Hello` as sent.
    #[must_use]
    pub fn local(&self) -> &Hello {
        &self.local
    }

    /// The wire under the session.
    #[must_use]
    pub fn kind(&self) -> TransportKind {
        self.connection.kind()
    }

    /// The peer's address as last seen.
    #[must_use]
    pub fn remote_addr(&self) -> std::net::SocketAddr {
        self.connection.remote_addr()
    }

    /// The peer's clock when it sent its `Hello` minus this node's clock when it arrived, in milliseconds.
    #[must_use]
    pub fn clock_skew_ms(&self) -> i64 {
        self.clock_skew_ms
    }

    /// Closes the connection under the session.
    ///
    /// Answers in flight are lost; the caller closes, the server lets
    /// `serve` end with the connection instead.
    pub fn close(&self) {
        self.connection.close();
    }

    fn next_request_id(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Sends one request and waits at most `deadline` for its answer.
    ///
    /// The peer stops work when the budget it was given runs out and says
    /// so with `Fault::Deadline`. That is the same outcome as this side's
    /// timer firing first, so both are `RpcError::Deadline`.
    async fn call(&self, body: RequestBody, deadline: Duration) -> Result<ResponseBody, RpcError> {
        let id = self.next_request_id();
        let request = Request {
            id,
            budget_ms: budget(deadline),
            body,
        };
        let exchange = async {
            let mut recv = send_request(self.connection.as_ref(), &request).await?;
            match read_response(recv.as_mut(), id, MAX_CONTROL_FRAME).await? {
                ResponseBody::Fault(Fault::Deadline) => Err(RpcError::Deadline(deadline)),
                ResponseBody::Fault(fault) => Err(RpcError::Fault(fault)),
                body => Ok(body),
            }
        };
        timeout(deadline, exchange)
            .await
            .map_err(|_| RpcError::Deadline(deadline))?
    }

    /// Sends `Ping` and waits at most `deadline` for `Pong`.
    pub async fn ping(&self, deadline: Duration) -> Result<(), RpcError> {
        match self.call(RequestBody::Ping, deadline).await? {
            ResponseBody::Pong => Ok(()),
            other => Err(unexpected("pong", &other)),
        }
    }

    /// Sends one per-job message and reads the answer as the kind the request takes.
    async fn job<T: JobAnswer>(
        &self,
        token: FencingToken,
        request: JobRequest,
        deadline: Duration,
    ) -> Result<Fenced<T>, RpcError> {
        let body = RequestBody::Job {
            token,
            body: request,
        };
        match self.call(body, deadline).await? {
            ResponseBody::Job(response) => {
                Fenced::from_response(response).map_err(|other| RpcError::UnexpectedResponse {
                    expected: T::KIND,
                    got: other.kind(),
                })
            }
            other => Err(unexpected(T::KIND, &other)),
        }
    }

    /// Asks for the records after `after`, or from the first record when it is `None`, under `token`, waiting at most `deadline`.
    pub async fn pull(
        &self,
        token: FencingToken,
        after: Option<RecordHash>,
        deadline: Duration,
    ) -> Result<Fenced<Pulled>, RpcError> {
        self.job(token, JobRequest::Pull { after }, deadline).await
    }

    /// Replicates one page of records under `token`, waiting at most `deadline`.
    pub async fn append(
        &self,
        token: FencingToken,
        records: RecordPage,
        deadline: Duration,
    ) -> Result<Fenced<Appended>, RpcError> {
        self.job(token, JobRequest::Append { records }, deadline)
            .await
    }

    /// Asks the owner to start a move toward this node under `token`, waiting at most `deadline`.
    pub async fn move_request(
        &self,
        token: FencingToken,
        deadline: Duration,
    ) -> Result<Fenced<MoveRequested>, RpcError> {
        self.job(token, JobRequest::MoveRequest, deadline).await
    }

    /// Sends the owner's `offer` at `epoch`, waiting at most `deadline`.
    ///
    /// The token is built from the offer's job, so the two cannot name different jobs.
    pub async fn offer(
        &self,
        epoch: Epoch,
        offer: MoveOffer,
        deadline: Duration,
    ) -> Result<Fenced<Offered>, RpcError> {
        let token = FencingToken {
            job_id: offer.job_id,
            epoch,
        };
        self.job(token, JobRequest::MoveOffer(offer.into()), deadline)
            .await
    }

    /// Asks the target for its decision on `move_id` again under `token`, waiting at most `deadline`.
    pub async fn move_status(
        &self,
        token: FencingToken,
        move_id: MoveId,
        deadline: Duration,
    ) -> Result<Fenced<MoveState>, RpcError> {
        self.job(token, JobRequest::MoveStatus { move_id }, deadline)
            .await
    }

    /// Asks the target to decide against `move_id` under `token`, waiting at most `deadline`; returns its decision record.
    pub async fn move_abort(
        &self,
        token: FencingToken,
        move_id: MoveId,
        deadline: Duration,
    ) -> Result<Fenced<RecordBytes>, RpcError> {
        self.job(token, JobRequest::MoveAbort { move_id }, deadline)
            .await
    }

    /// Asks for the length and chunk list of `snapshot`'s manifest, waiting at most `deadline`.
    pub async fn want_snapshot(
        &self,
        snapshot: SnapshotId,
        deadline: Duration,
    ) -> Result<ManifestChunks, RpcError> {
        match self
            .call(RequestBody::WantSnapshot { snapshot }, deadline)
            .await?
        {
            ResponseBody::ManifestChunks(manifest) => Ok(manifest),
            other => Err(unexpected("manifest chunks", &other)),
        }
    }

    /// Asks for a batch of chunks; each one arrives through the returned stream, verified.
    ///
    /// `idle` bounds the time without any byte arriving, not the time for a
    /// chunk or the batch, so a slow link that keeps moving is never cut. On
    /// any error the caller keeps what arrived, asks `ChunkStream::missing`
    /// what did not, and sends a new batch, on this session or a new one.
    pub async fn want_chunks(
        &self,
        batch: ChunkBatch,
        idle: Duration,
    ) -> Result<ChunkStream, RpcError> {
        let id = self.next_request_id();
        let wanted: BTreeSet<ChunkId> = batch.ids().iter().copied().collect();
        let request = Request {
            id,
            budget_ms: budget(idle),
            body: RequestBody::WantChunks(batch),
        };
        let recv = timeout(idle, send_request(self.connection.as_ref(), &request))
            .await
            .map_err(|_| RpcError::Deadline(idle))??;
        Ok(ChunkStream::new(recv, id, wanted, idle))
    }

    /// Answers the peer's requests with `service` while `trust` pairs the peer, and returns why it stopped.
    ///
    /// A connection either side closed comes back as
    /// `RpcError::Transport(TransportError::Closed(_))`. A peer that `trust`
    /// no longer pairs, seen before a request or at a recheck every
    /// `TRUST_RECHECK`, has its connection closed and comes back as
    /// `RpcError::Unpaired`; requests in flight then fail with it.
    pub async fn serve(&self, service: Arc<dyn Service>, trust: Arc<dyn Trust>) -> RpcError {
        let peer = self.peer();
        let requests = async {
            loop {
                let stream = match self.connection.next().await {
                    Ok(stream) => stream,
                    Err(err) => return err.into(),
                };
                if !trust.is_paired(peer) {
                    return RpcError::Unpaired(peer);
                }
                let service = service.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle(peer, stream, service).await {
                        tracing::debug!(%peer, error = %err, "request ended early");
                    }
                });
            }
        };
        let recheck = async {
            loop {
                tokio::time::sleep(TRUST_RECHECK).await;
                refresh(&trust).await;
                if !trust.is_paired(peer) {
                    return RpcError::Unpaired(peer);
                }
            }
        };
        let ended = tokio::select! {
            ended = requests => ended,
            ended = recheck => ended,
        };
        if let RpcError::Unpaired(_) = ended {
            tracing::info!(%peer, "peer unpaired; closing its connection");
            self.connection.close();
        }
        ended
    }
}

/// One chunk that arrived and was checked: its zstd frame decompresses to at
/// most `MAX_CHUNK_LEN` bytes, and those bytes hash to its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedChunk {
    id: ChunkId,
    stored: Vec<u8>,
    bytes: Vec<u8>,
}

impl VerifiedChunk {
    /// Decompresses `stored` within the chunk bound and keeps it only if the bytes hash to `id`.
    ///
    /// The size the frame declares is checked before anything is
    /// decompressed, and a frame that declares none is decompressed into at
    /// most `MAX_CHUNK_LEN`, so a small frame cannot make this node
    /// allocate more than one chunk.
    fn check(id: ChunkId, stored: Vec<u8>) -> Option<Self> {
        let capacity = match zstd::zstd_safe::get_frame_content_size(&stored) {
            Ok(Some(declared)) => usize::try_from(declared)
                .ok()
                .filter(|n| *n <= MAX_CHUNK_LEN)?,
            Ok(None) => MAX_CHUNK_LEN,
            Err(_) => return None,
        };
        let bytes = zstd::bulk::decompress(&stored, capacity).ok()?;
        (ChunkId::of(&bytes) == id).then_some(Self { id, stored, bytes })
    }

    /// The chunk's id: blake3 of its bytes.
    #[must_use]
    pub fn id(&self) -> ChunkId {
        self.id
    }

    /// The chunk's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The zstd frame as it arrived, which a store can keep as is.
    #[must_use]
    pub fn stored(&self) -> &[u8] {
        &self.stored
    }

    /// Returns the id and the zstd frame, which a store keeps as is.
    #[must_use]
    pub fn into_stored(self) -> (ChunkId, Vec<u8>) {
        (self.id, self.stored)
    }

    /// Returns the id and the decompressed bytes.
    #[must_use]
    pub fn into_bytes(self) -> (ChunkId, Vec<u8>) {
        (self.id, self.bytes)
    }
}

/// Where a chunk stream is: reading, done, or ended by an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    Open,
    Done,
    Broken,
}

/// The chunks of one `WantChunks` request as they arrive.
///
/// The first error ends the stream: it may have stopped inside a frame, so
/// every later `next` returns `Interrupted` instead of reading payload
/// bytes as a frame header.
pub struct ChunkStream {
    recv: IdleReader<Box<dyn AsyncRead + Send + Unpin>>,
    id: RequestId,
    wanted: BTreeSet<ChunkId>,
    idle: Duration,
    progress: Progress,
}

impl std::fmt::Debug for ChunkStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkStream")
            .field("request", &self.id)
            .field("missing", &self.wanted.len())
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

impl ChunkStream {
    fn new(
        recv: Box<dyn AsyncRead + Send + Unpin>,
        id: RequestId,
        wanted: BTreeSet<ChunkId>,
        idle: Duration,
    ) -> Self {
        Self {
            recv: IdleReader::new(recv, idle),
            id,
            wanted,
            idle,
            progress: Progress::Open,
        }
    }

    /// Returns the next chunk, verified, or `None` once the peer sent every asked chunk.
    ///
    /// Cancelling the call ends the stream, as an error does: it may have
    /// stopped inside a frame, so the next call returns `Interrupted`.
    pub async fn next(&mut self) -> Result<Option<VerifiedChunk>, RpcError> {
        match self.progress {
            Progress::Open => {}
            Progress::Done => return Ok(None),
            Progress::Broken => return Err(RpcError::Interrupted),
        }
        // Broken until a whole frame is read, so a cancelled call leaves it broken.
        self.progress = Progress::Broken;
        let next = self.read_one().await;
        self.progress = match &next {
            Ok(Some(_)) => Progress::Open,
            Ok(None) => Progress::Done,
            Err(_) => Progress::Broken,
        };
        next
    }

    async fn read_one(&mut self) -> Result<Option<VerifiedChunk>, RpcError> {
        let body = match read_response(&mut self.recv, self.id, MAX_BULK_FRAME).await {
            Err(RpcError::Frame(FrameError::Io(err))) if err.kind() == io::ErrorKind::TimedOut => {
                return Err(RpcError::Deadline(self.idle));
            }
            other => other?,
        };
        match body {
            ResponseBody::Chunk { id, zstd } => {
                if !self.wanted.remove(&id) {
                    return Err(RpcError::UnrequestedChunk { id });
                }
                let Some(chunk) = VerifiedChunk::check(id, zstd) else {
                    self.wanted.insert(id);
                    return Err(RpcError::ChunkMismatch { id });
                };
                Ok(Some(chunk))
            }
            ResponseBody::ChunksDone => {
                if self.wanted.is_empty() {
                    Ok(None)
                } else {
                    Err(RpcError::Incomplete {
                        missing: self.wanted.len(),
                    })
                }
            }
            // The peer's budget ran out finding a chunk: the same outcome as `call` reports.
            ResponseBody::Fault(Fault::Deadline) => Err(RpcError::Deadline(self.idle)),
            ResponseBody::Fault(fault) => Err(RpcError::Fault(fault)),
            other => Err(unexpected("chunk", &other)),
        }
    }

    /// The asked chunks that have not arrived verified.
    pub fn missing(&self) -> impl Iterator<Item = ChunkId> + '_ {
        self.wanted.iter().copied()
    }
}

async fn reply(
    send: &mut (dyn AsyncWrite + Send + Unpin),
    id: RequestId,
    body: ResponseBody,
    max: usize,
) -> Result<(), RpcError> {
    let frame = encode(&Response { id, body })?;
    write_frame(send, &frame, max).await?;
    Ok(())
}

async fn handle(peer: NodeId, stream: Stream, service: Arc<dyn Service>) -> Result<(), RpcError> {
    let (mut send, mut recv) = stream.split();
    let bytes = timeout(
        REQUEST_READ_TIMEOUT,
        read_frame(&mut recv, MAX_CONTROL_FRAME),
    )
    .await
    .map_err(|_| RpcError::Deadline(REQUEST_READ_TIMEOUT))??;
    let request: Request = decode(&bytes)?;
    let budget = Duration::from_millis(u64::from(request.budget_ms));
    let id = request.id;
    let body = match request.body {
        RequestBody::Hello(_) => ResponseBody::Fault(Fault::BadRequest(FaultText::truncated(
            "hello after the handshake",
        ))),
        RequestBody::Ping => ResponseBody::Pong,
        RequestBody::Job { token, body } => {
            // Only the answer races the budget; the work is never dropped halfway.
            let service = Arc::clone(&service);
            let work =
                tokio::spawn(async move { answer_job(service.as_ref(), peer, token, body).await });
            match timeout(budget, work).await {
                Ok(Ok(answer)) => answer.map_or_else(ResponseBody::Fault, ResponseBody::Job),
                Ok(Err(_)) => ResponseBody::Fault(Fault::Internal(FaultText::truncated(
                    "the job request failed",
                ))),
                Err(_) => ResponseBody::Fault(Fault::Deadline),
            }
        }
        RequestBody::WantSnapshot { snapshot } => timeout(budget, service.manifest(peer, snapshot))
            .await
            .unwrap_or(Err(Fault::Deadline))
            .map_or_else(ResponseBody::Fault, ResponseBody::ManifestChunks),
        RequestBody::WantChunks(batch) => {
            send_chunks(peer, id, batch, budget, send.as_mut(), service.as_ref()).await?;
            send.shutdown().await.map_err(FrameError::Io)?;
            return Ok(());
        }
    };
    reply(send.as_mut(), id, body, MAX_CONTROL_FRAME).await?;
    send.shutdown().await.map_err(FrameError::Io)?;
    Ok(())
}

/// Hands one per-job message to the service method for its kind and puts the answer on the wire.
async fn answer_job(
    service: &dyn Service,
    peer: NodeId,
    token: FencingToken,
    request: JobRequest,
) -> Result<JobResponse, Fault> {
    Ok(match request {
        JobRequest::Pull { after } => service.pull(peer, token, after).await?.into_response(),
        JobRequest::Append { records } => {
            service.append(peer, token, records).await?.into_response()
        }
        JobRequest::MoveRequest => service.move_request(peer, token).await?.into_response(),
        JobRequest::MoveOffer(offer) => service
            .offer(peer, token, offer.for_job(token.job_id))
            .await?
            .into_response(),
        JobRequest::MoveStatus { move_id } => service
            .move_status(peer, token, move_id)
            .await?
            .into_response(),
        JobRequest::MoveAbort { move_id } => service
            .move_abort(peer, token, move_id)
            .await?
            .into_response(),
    })
}

/// Streams the chunks of one batch, each found within `budget`, then `ChunksDone`; the first fault ends the run.
async fn send_chunks(
    peer: NodeId,
    id: RequestId,
    batch: ChunkBatch,
    budget: Duration,
    send: &mut (dyn AsyncWrite + Send + Unpin),
    service: &dyn Service,
) -> Result<(), RpcError> {
    for chunk in batch.ids() {
        let body = match timeout(budget, service.chunk(peer, *chunk)).await {
            Ok(Ok(zstd)) => ResponseBody::Chunk { id: *chunk, zstd },
            Ok(Err(fault)) => ResponseBody::Fault(fault),
            Err(_) => ResponseBody::Fault(Fault::Deadline),
        };
        let fault = matches!(body, ResponseBody::Fault(_));
        reply(send, id, body, MAX_BULK_FRAME).await?;
        if fault {
            return Ok(());
        }
    }
    reply(send, id, ResponseBody::ChunksDone, MAX_CONTROL_FRAME).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Duration = Duration::from_millis(150);

    fn frame(body: ResponseBody) -> Vec<u8> {
        let payload = encode(&Response {
            id: RequestId(1),
            body,
        })
        .unwrap();
        let mut bytes = u32::try_from(payload.len() + 1)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        bytes.push(crate::frame::FRAME_VERSION);
        bytes.extend_from_slice(&payload);
        bytes
    }

    /// A chunk response carrying `bytes` as zstd stores them.
    fn chunk(id: ChunkId, bytes: &[u8]) -> ResponseBody {
        ResponseBody::Chunk {
            id,
            zstd: zstd::bulk::compress(bytes, 3).unwrap(),
        }
    }

    fn stream_over(recv: tokio::io::DuplexStream, ids: &[ChunkId]) -> ChunkStream {
        ChunkStream::new(
            Box::new(recv),
            RequestId(1),
            ids.iter().copied().collect(),
            IDLE,
        )
    }

    /// `idle` is the time without a byte, so a chunk that takes many idle
    /// periods to arrive over a slow but moving link is not cut.
    #[tokio::test]
    async fn a_slow_chunk_that_keeps_moving_arrives() {
        let bytes = vec![7u8; 256 << 10];
        let id = ChunkId::of(&bytes);
        let (mut send, recv) = tokio::io::duplex(1 << 20);
        let mut stream = stream_over(recv, &[id]);
        let wire = [frame(chunk(id, &bytes)), frame(ResponseBody::ChunksDone)].concat();
        tokio::spawn(async move {
            for piece in wire.chunks(wire.len() / 16) {
                tokio::time::sleep(IDLE / 3).await;
                send.write_all(piece).await.unwrap();
            }
        });
        let started = tokio::time::Instant::now();
        assert_eq!(stream.next().await.unwrap().unwrap().bytes(), bytes);
        assert!(started.elapsed() > IDLE * 4, "the link was not slow");
        assert!(stream.next().await.unwrap().is_none());
    }

    /// A call cancelled inside a frame, by the caller's own timeout or
    /// `select!`, ends the stream too: the rest of the frame is never read
    /// as a header.
    #[tokio::test]
    async fn a_call_cancelled_inside_a_frame_ends_the_stream() {
        let id = ChunkId::of(b"x");
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[id]);
        let whole = frame(chunk(id, b"x"));
        send.write_all(&whole[..3]).await.unwrap();
        let cancelled = tokio::time::timeout(IDLE / 5, stream.next()).await;
        assert!(cancelled.is_err(), "the call must still be waiting");
        send.write_all(&whole[3..]).await.unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::Interrupted)));
    }

    /// A peer whose budget ran out while it looked for a chunk reports a
    /// deadline, which the caller sees as from any other call.
    #[tokio::test]
    async fn a_peer_deadline_on_a_chunk_is_the_callers_deadline() {
        let id = ChunkId::of(b"x");
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[id]);
        send.write_all(&frame(ResponseBody::Fault(Fault::Deadline)))
            .await
            .unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::Deadline(d)) if d == IDLE));
    }

    /// A chunk is kept only once its bytes hash to the id it came under;
    /// one that does not is refused and stays missing.
    #[tokio::test]
    async fn a_chunk_whose_bytes_do_not_hash_to_its_id_is_refused() {
        let id = ChunkId::of(b"x");
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[id]);
        send.write_all(&frame(chunk(id, b"y"))).await.unwrap();
        assert!(
            matches!(stream.next().await, Err(RpcError::ChunkMismatch { id: bad }) if bad == id)
        );
        assert_eq!(stream.missing().collect::<Vec<_>>(), [id]);
    }

    /// A small zstd frame that decompresses past the chunk bound is refused
    /// even when the bytes would hash to the id, so a peer cannot make this
    /// node allocate more than one chunk.
    #[tokio::test]
    async fn a_chunk_that_decompresses_past_the_bound_is_refused() {
        let bomb = vec![0u8; MAX_CHUNK_LEN + 1];
        let id = ChunkId::of(&bomb);
        let (mut send, recv) = tokio::io::duplex(1 << 20);
        let mut stream = stream_over(recv, &[id]);
        send.write_all(&frame(chunk(id, &bomb))).await.unwrap();
        assert!(
            matches!(stream.next().await, Err(RpcError::ChunkMismatch { id: bad }) if bad == id)
        );
    }

    /// The missing set stays exact: a chunk nobody asked for, or one that
    /// already arrived, is a protocol error and not a second copy.
    #[tokio::test]
    async fn a_chunk_not_asked_for_or_sent_twice_is_refused() {
        let (x, y) = (ChunkId::of(b"x"), ChunkId::of(b"y"));
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[x]);
        send.write_all(&frame(chunk(y, b"y"))).await.unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::UnrequestedChunk { id }) if id == y));

        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[x]);
        let once = frame(chunk(x, b"x"));
        send.write_all(&[once.clone(), once].concat())
            .await
            .unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap().id(), x);
        assert!(matches!(stream.next().await, Err(RpcError::UnrequestedChunk { id }) if id == x));
    }

    /// A peer that says it is done while asked chunks never came is not done.
    #[tokio::test]
    async fn done_with_chunks_still_missing_is_incomplete() {
        let (x, y) = (ChunkId::of(b"x"), ChunkId::of(b"y"));
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[x, y]);
        let wire = [frame(chunk(x, b"x")), frame(ResponseBody::ChunksDone)].concat();
        send.write_all(&wire).await.unwrap();
        assert!(stream.next().await.unwrap().is_some());
        assert!(matches!(
            stream.next().await,
            Err(RpcError::Incomplete { missing: 1 })
        ));
        assert_eq!(stream.missing().collect::<Vec<_>>(), [y]);
    }

    /// Two nodes on different protocol versions refuse each other at the
    /// hello instead of misreading each other's requests.
    #[tokio::test]
    async fn a_hello_from_another_protocol_version_is_refused() {
        use crate::mux::{Mux, Side};

        let node = NodeId::from_bytes([3; 32]);
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        let (a, b) = tokio::io::duplex(1 << 16);
        let dialer = Mux::spawn(a, Side::Dialer, node, addr);
        let acceptor: Arc<dyn Connection> = Arc::new(Mux::spawn(b, Side::Acceptor, node, addr));
        let accepting = tokio::spawn(Session::accept(
            acceptor,
            node,
            VersionText::truncated("this"),
            IDLE * 10,
        ));
        let newer = Request {
            id: RequestId(0),
            budget_ms: 1_000,
            body: RequestBody::Hello(Hello {
                node,
                protocol_version: PROTOCOL_VERSION + 1,
                fabric_version: VersionText::truncated("next"),
                clock_ms: wall_clock_ms(),
            }),
        };
        let _answer = send_request(&dialer, &newer).await.unwrap();
        assert!(matches!(
            accepting.await.unwrap(),
            Err(RpcError::ProtocolVersion { ours: PROTOCOL_VERSION, theirs })
                if theirs == PROTOCOL_VERSION + 1
        ));
    }

    /// A trust decision the test flips.
    #[derive(Debug)]
    struct Revocable(std::sync::atomic::AtomicBool);

    impl Trust for Revocable {
        fn is_paired(&self, _node: NodeId) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A service that serves nothing; the sessions here only ping.
    struct Nothing;

    fn no_job<T>() -> JobFuture<'static, T> {
        Box::pin(async { Err(Fault::UnknownJob) })
    }

    impl Service for Nothing {
        fn pull(&self, _: NodeId, _: FencingToken, _: Option<RecordHash>) -> JobFuture<'_, Pulled> {
            no_job()
        }

        fn append(&self, _: NodeId, _: FencingToken, _: RecordPage) -> JobFuture<'_, Appended> {
            no_job()
        }

        fn move_request(&self, _: NodeId, _: FencingToken) -> JobFuture<'_, MoveRequested> {
            no_job()
        }

        fn offer(&self, _: NodeId, _: FencingToken, _: MoveOffer) -> JobFuture<'_, Offered> {
            no_job()
        }

        fn move_status(&self, _: NodeId, _: FencingToken, _: MoveId) -> JobFuture<'_, MoveState> {
            no_job()
        }

        fn move_abort(&self, _: NodeId, _: FencingToken, _: MoveId) -> JobFuture<'_, RecordBytes> {
            no_job()
        }

        fn manifest(
            &self,
            _peer: NodeId,
            _snapshot: SnapshotId,
        ) -> BoxFuture<'_, Result<ManifestChunks, Fault>> {
            Box::pin(async { Err(Fault::UnknownSnapshot) })
        }

        fn chunk(&self, _peer: NodeId, id: ChunkId) -> BoxFuture<'_, Result<Vec<u8>, Fault>> {
            Box::pin(async move { Err(Fault::UnknownChunk(id)) })
        }
    }

    /// A dialer's session and an acceptor's session that serves under `trust`, over the multiplexer in memory.
    async fn serving_pair(trust: Arc<Revocable>) -> (Session, tokio::task::JoinHandle<RpcError>) {
        use crate::mux::{Mux, Side};

        let (dialer_id, acceptor_id) = (NodeId::from_bytes([1; 32]), NodeId::from_bytes([2; 32]));
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        let (a, b) = tokio::io::duplex(1 << 16);
        let dialer: Arc<dyn Connection> = Arc::new(Mux::spawn(a, Side::Dialer, acceptor_id, addr));
        let acceptor: Arc<dyn Connection> =
            Arc::new(Mux::spawn(b, Side::Acceptor, dialer_id, addr));
        let version = VersionText::truncated("test");
        let (dialer_session, acceptor_session) = tokio::join!(
            Session::connect(dialer, dialer_id, version.clone(), IDLE * 10),
            Session::accept(acceptor, acceptor_id, version, IDLE * 10),
        );
        let accepted = acceptor_session.unwrap();
        let serving = tokio::spawn(async move { accepted.serve(Arc::new(Nothing), trust).await });
        (dialer_session.unwrap(), serving)
    }

    /// A peer unpaired while its connection is idle loses the connection
    /// at the next recheck; keep-alives do not keep its access.
    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_ends_once_its_peer_is_unpaired() {
        let trust = Arc::new(Revocable(true.into()));
        let (session, serving) = serving_pair(Arc::clone(&trust)).await;
        session.ping(IDLE * 10).await.unwrap();
        trust.0.store(false, Ordering::SeqCst);
        let unpaired = tokio::time::Instant::now();
        assert!(matches!(serving.await.unwrap(), RpcError::Unpaired(_)));
        assert!(unpaired.elapsed() <= TRUST_RECHECK);
        assert!(session.ping(IDLE * 10).await.is_err());
    }

    /// A request that arrives after the trust decision dropped the peer is
    /// not served, even before the next recheck.
    #[tokio::test(start_paused = true)]
    async fn a_request_after_an_unpair_is_not_served() {
        let trust = Arc::new(Revocable(true.into()));
        let (session, serving) = serving_pair(Arc::clone(&trust)).await;
        trust.0.store(false, Ordering::SeqCst);
        let started = tokio::time::Instant::now();
        assert!(session.ping(IDLE * 10).await.is_err());
        assert!(matches!(serving.await.unwrap(), RpcError::Unpaired(_)));
        assert!(started.elapsed() < TRUST_RECHECK);
    }

    /// A paired peer that sends part of a request and stops gets its stream
    /// ended after `REQUEST_READ_TIMEOUT`; it does not hold the stream and
    /// its task for as long as keep-alives hold the connection.
    #[tokio::test(start_paused = true)]
    async fn a_request_that_stops_halfway_loses_its_stream() {
        let (session, _serving) = serving_pair(Arc::new(Revocable(true.into()))).await;
        let mut stream = session.connection.open().await.unwrap();
        stream.write_all(&[64, 0, 0]).await.unwrap();
        let started = tokio::time::Instant::now();
        let mut answer = Vec::new();
        let ended = timeout(
            REQUEST_READ_TIMEOUT * 2,
            tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut answer),
        )
        .await;
        assert!(ended.is_ok(), "the server kept the stream");
        assert!(answer.is_empty());
        assert!(started.elapsed() >= REQUEST_READ_TIMEOUT);
    }

    /// A stream that failed inside a frame never reads again.
    #[tokio::test]
    async fn a_stalled_stream_fails_once_and_then_stays_failed() {
        let id = ChunkId::of(b"x");
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[id]);
        let whole = frame(chunk(id, b"x"));
        send.write_all(&whole[..3]).await.unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::Deadline(d)) if d == IDLE));
        send.write_all(&whole[3..]).await.unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::Interrupted)));
        assert_eq!(stream.missing().collect::<Vec<_>>(), [id]);
    }
}
