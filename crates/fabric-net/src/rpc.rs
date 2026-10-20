//! Request and response over per-request streams (ADR 0005).
//!
//! Invariant: a session speaks for the application only after both nodes
//! exchanged `Hello` and agreed on the protocol version and on who the peer
//! is; every call carries a deadline that fires on the caller whatever the
//! peer does; and the caller trusts a chunk only after its bytes hash to
//! the id it asked for. One stream carries one request and its answer, so
//! a slow answer never holds up another.
//!
//! The wire types `RequestBody` and `ResponseBody` are built and matched
//! here only. A caller asks through one typed method per request, and a
//! `Service` answers through one typed method per request, so neither can
//! pair a request with an answer of the wrong kind. `Hello` and `Ping`
//! belong to this layer and never reach a service.

use std::collections::BTreeSet;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::{ChunkId, NodeId, SnapshotId};
use fabric_core::lease::FencingToken;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::time::{Instant, Sleep, timeout};

use crate::frame::{FrameError, MAX_BULK_FRAME, MAX_CONTROL_FRAME, read_frame, write_frame};
use crate::transport::{Connection, Stream, TransportError, TransportKind};
use crate::wire::{
    ChunkBatch, Fault, FaultText, Hello, JobRequest, JobResponse, ManifestChunks, PROTOCOL_VERSION,
    Request, RequestBody, RequestId, Response, ResponseBody, WireError, decode, encode,
    wall_clock_ms,
};

/// A clock difference between two nodes above this is logged at `Hello`.
pub const CLOCK_SKEW_WARN_MS: u64 = 10_000;

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
    /// The chunk's bytes do not hash to its id; it is dropped.
    #[error("chunk {id} did not hash to its id")]
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
}

/// What a node serves to its peers: per-job messages, manifests and chunks.
pub trait Service: Send + Sync {
    /// Answers one per-job message from `peer`, sent under the fencing `token`.
    fn job(
        &self,
        peer: NodeId,
        token: FencingToken,
        request: JobRequest,
    ) -> BoxFuture<'_, Result<JobResponse, Fault>>;
    /// Returns the length and chunk list of `snapshot`'s manifest for `peer`.
    fn manifest(
        &self,
        peer: NodeId,
        snapshot: SnapshotId,
    ) -> BoxFuture<'_, Result<ManifestChunks, Fault>>;
    /// Returns the bytes of one chunk for `peer`, or `None` if this node does not hold it.
    fn chunk(&self, peer: NodeId, id: ChunkId) -> BoxFuture<'_, Option<Vec<u8>>>;
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

impl Session {
    /// Runs the dialer's half of the handshake: sends `local`, expects the peer's `Hello`.
    ///
    /// `local`'s clock is read again as it is sent.
    pub async fn connect(
        connection: Box<dyn Connection>,
        local: Hello,
        deadline: Duration,
    ) -> Result<Session, RpcError> {
        let connection: Arc<dyn Connection> = Arc::from(connection);
        let local = Hello {
            clock_ms: wall_clock_ms(),
            ..local
        };
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

    /// Runs the acceptor's half of the handshake: expects the peer's `Hello` first, answers with `local`.
    ///
    /// `local`'s clock is read again as it is sent.
    pub async fn accept(
        connection: Box<dyn Connection>,
        local: Hello,
        deadline: Duration,
    ) -> Result<Session, RpcError> {
        let connection: Arc<dyn Connection> = Arc::from(connection);
        let exchange = async {
            let (mut send, mut recv) = connection.next().await?.split();
            let bytes = read_frame(&mut recv, MAX_CONTROL_FRAME).await?;
            let request: Request = decode(&bytes)?;
            let received_ms = wall_clock_ms();
            let local = Hello {
                clock_ms: received_ms,
                ..local
            };
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

    /// The connection under the session, for bulk streams outside the request vocabulary.
    #[must_use]
    pub fn connection(&self) -> &dyn Connection {
        self.connection.as_ref()
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

    /// Sends one per-job message under the fencing `token` and waits at most `deadline` for the answer.
    pub async fn job(
        &self,
        token: FencingToken,
        request: JobRequest,
        deadline: Duration,
    ) -> Result<JobResponse, RpcError> {
        let body = RequestBody::Job {
            token,
            body: request,
        };
        match self.call(body, deadline).await? {
            ResponseBody::Job(response) => Ok(response),
            other => Err(unexpected("job", &other)),
        }
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

    /// Answers the peer's requests with `service` until the connection ends.
    pub async fn serve(&self, service: Arc<dyn Service>) -> Result<(), RpcError> {
        loop {
            let stream = self.connection.next().await?;
            let peer = self.peer();
            let service = service.clone();
            tokio::spawn(async move {
                if let Err(err) = handle(peer, stream, service.as_ref()).await {
                    tracing::debug!(%peer, error = %err, "request ended early");
                }
            });
        }
    }
}

/// A reader that fails with `TimedOut` once `idle` passes with no byte arriving.
struct IdleReader {
    inner: Box<dyn AsyncRead + Send + Unpin>,
    limit: Duration,
    timer: Pin<Box<Sleep>>,
}

impl IdleReader {
    fn new(inner: Box<dyn AsyncRead + Send + Unpin>, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            timer: Box::pin(tokio::time::sleep(limit)),
        }
    }
}

impl AsyncRead for IdleReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    let next = Instant::now() + self.limit;
                    self.timer.as_mut().reset(next);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => match self.timer.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
                Poll::Pending => Poll::Pending,
            },
        }
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
    recv: IdleReader,
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

    /// Returns the next verified chunk, or `None` once the peer sent every asked chunk.
    pub async fn next(&mut self) -> Result<Option<(ChunkId, Vec<u8>)>, RpcError> {
        match self.progress {
            Progress::Open => {}
            Progress::Done => return Ok(None),
            Progress::Broken => return Err(RpcError::Interrupted),
        }
        let next = self.read_one().await;
        self.progress = match &next {
            Ok(Some(_)) => Progress::Open,
            Ok(None) => Progress::Done,
            Err(_) => Progress::Broken,
        };
        next
    }

    async fn read_one(&mut self) -> Result<Option<(ChunkId, Vec<u8>)>, RpcError> {
        let body = match read_response(&mut self.recv, self.id, MAX_BULK_FRAME).await {
            Err(RpcError::Frame(FrameError::Io(err))) if err.kind() == io::ErrorKind::TimedOut => {
                return Err(RpcError::Deadline(self.idle));
            }
            other => other?,
        };
        match body {
            ResponseBody::Chunk { id, bytes } => {
                if !self.wanted.remove(&id) {
                    return Err(RpcError::UnrequestedChunk { id });
                }
                if ChunkId::of(&bytes) != id {
                    self.wanted.insert(id);
                    return Err(RpcError::ChunkMismatch { id });
                }
                Ok(Some((id, bytes)))
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

async fn handle(peer: NodeId, stream: Stream, service: &dyn Service) -> Result<(), RpcError> {
    let (mut send, mut recv) = stream.split();
    let bytes = read_frame(&mut recv, MAX_CONTROL_FRAME).await?;
    let request: Request = decode(&bytes)?;
    let budget = Duration::from_millis(u64::from(request.budget_ms));
    let id = request.id;
    let body = match request.body {
        RequestBody::Hello(_) => ResponseBody::Fault(Fault::BadRequest(FaultText::truncated(
            "hello after the handshake",
        ))),
        RequestBody::Ping => ResponseBody::Pong,
        RequestBody::Job { token, body } => timeout(budget, service.job(peer, token, body))
            .await
            .unwrap_or(Err(Fault::Deadline))
            .map_or_else(ResponseBody::Fault, ResponseBody::Job),
        RequestBody::WantSnapshot { snapshot } => timeout(budget, service.manifest(peer, snapshot))
            .await
            .unwrap_or(Err(Fault::Deadline))
            .map_or_else(ResponseBody::Fault, ResponseBody::ManifestChunks),
        RequestBody::WantChunks(batch) => {
            send_chunks(peer, id, batch, budget, send.as_mut(), service).await?;
            send.shutdown().await.map_err(FrameError::Io)?;
            return Ok(());
        }
    };
    reply(send.as_mut(), id, body, MAX_CONTROL_FRAME).await?;
    send.shutdown().await.map_err(FrameError::Io)?;
    Ok(())
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
            Ok(Some(bytes)) => ResponseBody::Chunk { id: *chunk, bytes },
            Ok(None) => ResponseBody::Fault(Fault::UnknownChunk(*chunk)),
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
        let wire = [
            frame(ResponseBody::Chunk {
                id,
                bytes: bytes.clone(),
            }),
            frame(ResponseBody::ChunksDone),
        ]
        .concat();
        tokio::spawn(async move {
            for piece in wire.chunks(wire.len() / 16) {
                tokio::time::sleep(IDLE / 3).await;
                send.write_all(piece).await.unwrap();
            }
        });
        let started = Instant::now();
        assert_eq!(stream.next().await.unwrap(), Some((id, bytes)));
        assert!(started.elapsed() > IDLE * 4, "the link was not slow");
        assert!(stream.next().await.unwrap().is_none());
    }

    /// A stream that failed inside a frame never reads again.
    #[tokio::test]
    async fn a_stalled_stream_fails_once_and_then_stays_failed() {
        let id = ChunkId::of(b"x");
        let (mut send, recv) = tokio::io::duplex(1 << 10);
        let mut stream = stream_over(recv, &[id]);
        let whole = frame(ResponseBody::Chunk {
            id,
            bytes: b"x".to_vec(),
        });
        send.write_all(&whole[..3]).await.unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::Deadline(d)) if d == IDLE));
        send.write_all(&whole[3..]).await.unwrap();
        assert!(matches!(stream.next().await, Err(RpcError::Interrupted)));
        assert_eq!(stream.missing().collect::<Vec<_>>(), [id]);
    }
}
