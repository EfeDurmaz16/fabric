//! Request and response over per-request streams (ADR 0005).
//!
//! Invariant: a session speaks for the application only after both nodes
//! exchanged `Hello` and agreed on the protocol version and on who the peer
//! is; every call carries a deadline that fires on the caller whatever the
//! peer does; and the caller trusts a chunk only after its bytes hash to
//! the id it asked for. One stream carries one request and its answer, so
//! a slow answer never holds up another.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::{ChunkId, NodeId};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::frame::{FrameError, MAX_BULK_FRAME, MAX_CONTROL_FRAME, read_frame, write_frame};
use crate::transport::{Connection, Stream, TransportError, TransportKind};
use crate::wire::{
    ChunkBatch, Fault, FaultText, Hello, PROTOCOL_VERSION, Request, RequestBody, RequestId,
    Response, ResponseBody, WireError, decode, encode,
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
}

/// What a node does with the requests a peer sends; `Hello` and `WantChunks` never reach it.
pub trait Service: Send + Sync {
    /// Answers one request from `peer`.
    fn call(&self, peer: NodeId, request: RequestBody) -> BoxFuture<'_, ResponseBody>;
    /// Returns the bytes of one chunk for `peer`, or `None` if this node does not hold it.
    fn chunk(&self, peer: NodeId, id: ChunkId) -> BoxFuture<'_, Option<Vec<u8>>>;
}

/// An authenticated connection after the `Hello` exchange.
pub struct Session {
    connection: Arc<dyn Connection>,
    local: Hello,
    remote: Hello,
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
        ResponseBody::ManifestChunks { .. } => "manifest chunks",
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
    pub async fn connect(
        connection: Box<dyn Connection>,
        local: Hello,
        deadline: Duration,
    ) -> Result<Session, RpcError> {
        let connection: Arc<dyn Connection> = Arc::from(connection);
        let request = Request {
            id: RequestId(0),
            budget_ms: budget(deadline),
            body: RequestBody::Hello(local.clone()),
        };
        let exchange = async {
            let mut recv = send_request(connection.as_ref(), &request).await?;
            match read_response(recv.as_mut(), RequestId(0), MAX_CONTROL_FRAME).await? {
                ResponseBody::Hello(remote) => Ok(remote),
                ResponseBody::Fault(fault) => Err(RpcError::Fault(fault)),
                other => Err(RpcError::UnexpectedResponse {
                    expected: "hello",
                    got: kind_of(&other),
                }),
            }
        };
        let remote = timeout(deadline, exchange)
            .await
            .map_err(|_| RpcError::Deadline(deadline))??;
        Self::finish(connection, local, remote)
    }

    /// Runs the acceptor's half of the handshake: expects the peer's `Hello` first, answers with `local`.
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
            remote.ok_or(RpcError::NotAHello)
        };
        let remote = timeout(deadline, exchange)
            .await
            .map_err(|_| RpcError::Deadline(deadline))??;
        Self::finish(connection, local, remote)
    }

    fn finish(
        connection: Arc<dyn Connection>,
        local: Hello,
        remote: Hello,
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
        let session = Session {
            connection,
            local,
            remote,
            next_id: AtomicU64::new(1),
        };
        let skew = session.clock_skew_ms();
        if skew.unsigned_abs() > CLOCK_SKEW_WARN_MS {
            tracing::warn!(peer = %session.peer(), skew_ms = skew, "peer clock differs");
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

    /// The peer's clock minus this node's, in milliseconds, as read at `Hello`.
    #[must_use]
    pub fn clock_skew_ms(&self) -> i64 {
        let remote = i64::try_from(self.remote.clock_ms).unwrap_or(i64::MAX);
        let local = i64::try_from(self.local.clock_ms).unwrap_or(i64::MAX);
        remote.saturating_sub(local)
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
    pub async fn call(
        &self,
        body: RequestBody,
        deadline: Duration,
    ) -> Result<ResponseBody, RpcError> {
        let id = self.next_request_id();
        let request = Request {
            id,
            budget_ms: budget(deadline),
            body,
        };
        let exchange = async {
            let mut recv = send_request(self.connection.as_ref(), &request).await?;
            read_response(recv.as_mut(), id, MAX_CONTROL_FRAME).await
        };
        timeout(deadline, exchange)
            .await
            .map_err(|_| RpcError::Deadline(deadline))?
    }

    /// Sends `Ping` and waits at most `deadline` for `Pong`.
    pub async fn ping(&self, deadline: Duration) -> Result<(), RpcError> {
        match self.call(RequestBody::Ping, deadline).await? {
            ResponseBody::Pong => Ok(()),
            ResponseBody::Fault(fault) => Err(RpcError::Fault(fault)),
            other => Err(RpcError::UnexpectedResponse {
                expected: "pong",
                got: kind_of(&other),
            }),
        }
    }

    /// Asks for a batch of chunks; each one arrives through the returned stream, verified.
    ///
    /// `idle` bounds the wait for each chunk, not the whole batch. On any
    /// error the caller keeps what arrived, asks `ChunkStream::missing` what
    /// did not, and sends a new batch, on this session or a new one.
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
        Ok(ChunkStream {
            recv,
            id,
            wanted,
            idle,
        })
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

/// The chunks of one `WantChunks` request as they arrive.
pub struct ChunkStream {
    recv: Box<dyn AsyncRead + Send + Unpin>,
    id: RequestId,
    wanted: BTreeSet<ChunkId>,
    idle: Duration,
}

impl std::fmt::Debug for ChunkStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkStream")
            .field("request", &self.id)
            .field("missing", &self.wanted.len())
            .finish_non_exhaustive()
    }
}

impl ChunkStream {
    /// Returns the next verified chunk, or `None` once the peer sent every asked chunk.
    pub async fn next(&mut self) -> Result<Option<(ChunkId, Vec<u8>)>, RpcError> {
        let body = timeout(
            self.idle,
            read_response(self.recv.as_mut(), self.id, MAX_BULK_FRAME),
        )
        .await
        .map_err(|_| RpcError::Deadline(self.idle))??;
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
            other => Err(RpcError::UnexpectedResponse {
                expected: "chunk",
                got: kind_of(&other),
            }),
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
    match request.body {
        RequestBody::Hello(_) => {
            let fault = Fault::BadRequest(FaultText::truncated("hello after the handshake"));
            reply(
                send.as_mut(),
                id,
                ResponseBody::Fault(fault),
                MAX_CONTROL_FRAME,
            )
            .await?;
        }
        RequestBody::WantChunks(batch) => {
            for chunk in batch.ids() {
                let body = match timeout(budget, service.chunk(peer, *chunk)).await {
                    Ok(Some(bytes)) => ResponseBody::Chunk { id: *chunk, bytes },
                    Ok(None) => ResponseBody::Fault(Fault::UnknownChunk(*chunk)),
                    Err(_) => ResponseBody::Fault(Fault::Deadline),
                };
                let stop = matches!(body, ResponseBody::Fault(_));
                reply(send.as_mut(), id, body, MAX_BULK_FRAME).await?;
                if stop {
                    return Ok(());
                }
            }
            reply(
                send.as_mut(),
                id,
                ResponseBody::ChunksDone,
                MAX_CONTROL_FRAME,
            )
            .await?;
        }
        other => {
            let body = match timeout(budget, service.call(peer, other)).await {
                Ok(body) => body,
                Err(_) => ResponseBody::Fault(Fault::Deadline),
            };
            reply(send.as_mut(), id, body, MAX_CONTROL_FRAME).await?;
        }
    }
    send.shutdown().await.map_err(FrameError::Io)?;
    Ok(())
}
