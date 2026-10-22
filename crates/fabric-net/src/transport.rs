//! The transport boundary (ADR 0005).
//!
//! Invariant: everything above this module sees a peer only as a `NodeId`
//! and a set of ordered byte streams. Which wire carried them, QUIC or TCP
//! with TLS, is a fact for logs and the ledger, never a branch in protocol
//! code. A `Connection` exists only after the pinned handshake of the `tls`
//! module succeeded.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::tls::TlsError;

/// A connection that has read nothing for this long is closed, on either wire.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often an idle connection sends a keep-alive on either wire, so NAT bindings stay open.
pub const KEEP_ALIVE: Duration = Duration::from_secs(10);

/// How long a dial may take by default on either wire, connect and handshake together.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Which wire carries a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportKind {
    /// QUIC over UDP: multiplexed, migrates across address changes.
    Quic,
    /// TLS over TCP with a stream multiplexer: the fallback when UDP is blocked.
    TcpTls,
}

impl fmt::Display for TransportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TransportKind::Quic => "quic",
            TransportKind::TcpTls => "tcp+tls",
        })
    }
}

/// Why a connection or a stream could not be made or used.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// A socket operation failed.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// This side refused the peer inside the handshake, or the TLS setup failed.
    ///
    /// A refusal means the node at the address is not the one dialed, so a
    /// dial does not retry it on the other wire.
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// The handshake with `addr` did not complete.
    #[error("handshake with {addr} failed: {source}")]
    Handshake {
        /// The address dialed.
        addr: SocketAddr,
        /// What the wire reported.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The connection is closed; the source is what the wire reported.
    #[error("connection closed: {0}")]
    Closed(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The local endpoint is closed, so nothing can be accepted.
    #[error("the endpoint is closed")]
    EndpointClosed,
    /// The wire's configuration was refused.
    #[error("transport configuration: {0}")]
    Config(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// An operation did not complete within its limit.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// No address is known or resolvable for the peer.
    #[error("no address to dial for {0}")]
    NoAddress(NodeId),
}

/// One ordered, bidirectional byte stream inside a connection.
///
/// Shutting down the write side tells the peer this side is done writing;
/// dropping the stream does the same and stops reading. Dropping it while
/// a write waits on flow control, as a timed-out write does, cuts the
/// stream off instead: the bytes not yet sent are dropped, and the peer's
/// read fails on QUIC or ends short on TCP.
pub struct Stream {
    send: Box<dyn AsyncWrite + Send + Unpin>,
    recv: Box<dyn AsyncRead + Send + Unpin>,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Stream")
    }
}

impl Stream {
    /// Joins a write half and a read half into one stream.
    pub fn new(
        send: impl AsyncWrite + Send + Unpin + 'static,
        recv: impl AsyncRead + Send + Unpin + 'static,
    ) -> Self {
        Self {
            send: Box::new(send),
            recv: Box::new(recv),
        }
    }

    /// Splits the stream so one task can write while another reads.
    #[must_use]
    pub fn split(
        self,
    ) -> (
        Box<dyn AsyncWrite + Send + Unpin>,
        Box<dyn AsyncRead + Send + Unpin>,
    ) {
        (self.send, self.recv)
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.send).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_shutdown(cx)
    }
}

/// An authenticated connection to one peer.
pub trait Connection: Send + Sync {
    /// The peer's node id, proved by the handshake.
    fn peer(&self) -> NodeId;
    /// The wire that carries this connection.
    fn kind(&self) -> TransportKind;
    /// The peer's address as last seen.
    fn remote_addr(&self) -> SocketAddr;
    /// Opens a new stream; the peer sees it once the first byte arrives.
    fn open(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;
    /// Waits for the next stream the peer opened.
    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;
    /// Closes the connection at once; every stream on it fails from now on.
    ///
    /// Bytes written but not yet read by the peer are discarded. Dropping
    /// the handle instead keeps the connection open, on both wires, while
    /// any stream on it is alive, and closes it when the last one goes. On
    /// QUIC that close can still discard bytes the peer has not
    /// acknowledged, so the side that asked for the data closes, and the
    /// side that served it waits for that.
    fn close(&self);
}

/// A wire that dials and accepts connections.
pub trait Transport: Send + Sync {
    /// The wire this transport speaks.
    fn kind(&self) -> TransportKind;
    /// The local address this transport is bound to.
    fn local_addr(&self) -> Result<SocketAddr, TransportError>;
    /// Dials `addr` and accepts the connection only if the peer proves it is `peer`.
    ///
    /// Each wire bounds the dial with its own dial timeout, `DIAL_TIMEOUT`
    /// unless set, and fails with `TransportError::Timeout` past it.
    fn connect(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>>;
    /// Waits for the next connection from a paired peer; strangers are refused inside the handshake and never surface here.
    fn accept(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>>;
}
