//! The QUIC transport: `quinn` over the pinned TLS of the `tls` module.
//!
//! Invariant: a connection surfaces from `accept` or `connect` only after
//! quinn completed the TLS 1.3 handshake under the pinned verifiers, and the
//! node id it reports is read from the certificate that handshake checked.
//! 0-RTT is never enabled, so no application byte is replayable. Incoming
//! handshakes run in the background (the `accept` module), so a peer that
//! stalls one does not hold up the next, and only after the peer's address
//! answered a Retry, so a spoofed source takes no handshake slot.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{IdleTimeout, VarInt};
use rustls::pki_types::CertificateDer;
use tokio::io::AsyncWrite;

use crate::accept::{Handshakes, Inbox};
use crate::frame::MAX_CONTROL_FRAME;
use crate::identity::NodeIdentity;
use crate::rpc::MAX_CHUNK_RUNS;
use crate::tls::{Credentials, Refusal, SERVER_NAME, TlsError, Trust, peer_of};
use crate::transport::{
    Connection, DIAL_TIMEOUT, IDLE_TIMEOUT, KEEP_ALIVE, Stream, Transport, TransportError,
    TransportKind,
};

/// A QUIC endpoint that dials and accepts under one node identity.
pub struct QuicTransport {
    endpoint: quinn::Endpoint,
    credentials: Arc<Credentials>,
    transport: Arc<quinn::TransportConfig>,
    dial_timeout: Duration,
    inbox: Inbox,
}

impl std::fmt::Debug for QuicTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicTransport")
            .field("local_addr", &self.endpoint.local_addr().ok())
            .finish_non_exhaustive()
    }
}

impl QuicTransport {
    /// Binds a UDP socket at `addr` that accepts paired peers and dials as `identity`.
    ///
    /// Call it inside a tokio runtime.
    pub fn bind(
        identity: &NodeIdentity,
        trust: Arc<dyn Trust>,
        addr: SocketAddr,
    ) -> Result<Self, TransportError> {
        let credentials = Arc::new(Credentials::new(identity)?);
        let transport = Arc::new(transport_config()?);
        let rustls_config = credentials.server_config(Arc::clone(&trust))?;
        let quic_config = QuicServerConfig::try_from(rustls_config)
            .map_err(|err| TransportError::Config(Box::new(err)))?;
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(quic_config));
        server.transport_config(transport.clone());
        let endpoint = quinn::Endpoint::server(server, addr)?;
        let inbox = Inbox::spawn(trust, |handshakes| {
            accept_loop(endpoint.clone(), handshakes)
        });
        Ok(Self {
            endpoint,
            credentials,
            transport,
            dial_timeout: DIAL_TIMEOUT,
            inbox,
        })
    }

    /// Sets how long a dial may take, handshake included.
    #[must_use]
    pub fn with_dial_timeout(mut self, timeout: Duration) -> Self {
        self.dial_timeout = timeout;
        self
    }

    /// Dials within the dial timeout: an address where nothing answers
    /// costs that long, not quinn's idle timeout.
    async fn dial(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> Result<Box<dyn Connection>, TransportError> {
        tokio::time::timeout(self.dial_timeout, self.handshake(peer, addr))
            .await
            .map_err(|_| TransportError::Timeout(self.dial_timeout))?
    }

    async fn handshake(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> Result<Box<dyn Connection>, TransportError> {
        let refusal = Arc::new(Refusal::default());
        let rustls_config = self.credentials.client_config(peer, &refusal)?;
        let quic_config = QuicClientConfig::try_from(rustls_config)
            .map_err(|err| TransportError::Config(Box::new(err)))?;
        let mut client = quinn::ClientConfig::new(Arc::new(quic_config));
        client.transport_config(self.transport.clone());
        let connection = self
            .endpoint
            .connect_with(client, addr, SERVER_NAME)
            .map_err(|err| TransportError::Handshake {
                addr,
                source: Box::new(err),
            })?
            .await
            .map_err(|err| {
                refusal.take().map_or_else(
                    || TransportError::Handshake {
                        addr,
                        source: Box::new(err),
                    },
                    Into::into,
                )
            })?;
        let actual = identity_of(&connection)?;
        if actual != peer {
            connection.close(VarInt::from_u32(1), b"peer mismatch");
            return Err(TlsError::PeerMismatch {
                expected: peer,
                actual,
            }
            .into());
        }
        Ok(Box::new(QuicConnection {
            inner: connection,
            peer,
        }))
    }
}

/// Hands every incoming connection to `handshakes` until the endpoint closes.
///
/// An Initial from an address that has not proved it can receive is
/// answered with a Retry first. A spoofed source never sees the Retry, so
/// it never reaches a handshake slot.
async fn accept_loop(endpoint: quinn::Endpoint, handshakes: Handshakes) {
    while let Some(incoming) = endpoint.accept().await {
        let remote = incoming.remote_address();
        if !incoming.remote_address_validated() {
            if let Err(err) = incoming.retry() {
                tracing::debug!(%remote, error = %err, "retry refused");
            }
            continue;
        }
        let handshake = async move {
            let connection = incoming.await.map_err(|err| TransportError::Handshake {
                addr: remote,
                source: Box::new(err),
            })?;
            match identity_of(&connection) {
                Ok(peer) => Ok(Box::new(QuicConnection {
                    inner: connection,
                    peer,
                }) as Box<dyn Connection>),
                Err(err) => {
                    connection.close(VarInt::from_u32(1), b"no node certificate");
                    Err(err.into())
                }
            }
        };
        if !handshakes.run(remote, handshake).await {
            return;
        }
    }
}

/// The bytes a peer may send on one stream before this node reads them:
/// quinn's default, named here so `CONNECTION_WINDOW` can count it.
const STREAM_WINDOW: u32 = 1_250_000;

/// The bytes a peer may send on all the streams of one connection before
/// this node reads them. quinn sets no such bound by default, so a paired
/// peer's requests waiting for room to be read (`rpc`) could hold a whole
/// `STREAM_WINDOW` on each of its 100 streams here, 125 MB a connection.
/// It holds the `MAX_CHUNK_RUNS` chunk streams a peer sends this node at
/// once at their whole window, and a request frame besides, so a fetch
/// never waits on it.
const CONNECTION_WINDOW: u64 =
    MAX_CHUNK_RUNS as u64 * STREAM_WINDOW as u64 + MAX_CONTROL_FRAME as u64;

fn transport_config() -> Result<quinn::TransportConfig, TransportError> {
    let mut config = quinn::TransportConfig::default();
    let idle =
        IdleTimeout::try_from(IDLE_TIMEOUT).map_err(|err| TransportError::Config(Box::new(err)))?;
    config.max_idle_timeout(Some(idle));
    config.keep_alive_interval(Some(KEEP_ALIVE));
    config.stream_receive_window(VarInt::from_u32(STREAM_WINDOW));
    config.receive_window(
        VarInt::from_u64(CONNECTION_WINDOW).map_err(|err| TransportError::Config(Box::new(err)))?,
    );
    // Requests ride bidirectional streams only. Nothing accepts a
    // unidirectional one, so a peer gets no credit to open any: one it
    // could open would sit buffered with no reader and no deadline.
    config.max_concurrent_uni_streams(VarInt::from_u32(0));
    Ok(config)
}

fn identity_of(connection: &quinn::Connection) -> Result<NodeId, TlsError> {
    let certs = connection
        .peer_identity()
        .and_then(|any| any.downcast::<Vec<CertificateDer<'static>>>().ok());
    peer_of(certs.as_deref().map(Vec::as_slice))
}

fn closed(err: quinn::ConnectionError) -> TransportError {
    TransportError::Closed(Box::new(err))
}

impl Transport for QuicTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Quic
    }

    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.endpoint.local_addr()?)
    }

    fn connect(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
        Box::pin(self.dial(peer, addr))
    }

    fn accept(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
        Box::pin(self.inbox.next())
    }
}

struct QuicConnection {
    inner: quinn::Connection,
    peer: NodeId,
}

impl Connection for QuicConnection {
    fn peer(&self) -> NodeId {
        self.peer
    }

    fn kind(&self) -> TransportKind {
        TransportKind::Quic
    }

    fn remote_addr(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    fn open(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            let (send, recv) = self.inner.open_bi().await.map_err(closed)?;
            Ok(Stream::new(SendHalf::new(send), recv))
        })
    }

    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            let (send, recv) = self.inner.accept_bi().await.map_err(closed)?;
            Ok(Stream::new(SendHalf::new(send), recv))
        })
    }

    fn close(&self) {
        self.inner.close(VarInt::from_u32(0), b"close");
    }
}

/// A stream's send half that resets the stream when dropped in the middle of a write.
///
/// quinn finishes a send stream that is dropped, so the bytes already
/// written are still sent and the stream stays open until the peer reads
/// them. A write cut off by its caller's timeout must not leave that, so
/// a drop while a write waits on flow control resets the stream instead:
/// its bytes are dropped and the peer's read fails.
struct SendHalf {
    inner: quinn::SendStream,
    mid_write: bool,
}

impl SendHalf {
    fn new(inner: quinn::SendStream) -> Self {
        Self {
            inner,
            mid_write: false,
        }
    }
}

impl AsyncWrite for SendHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let polled = AsyncWrite::poll_write(Pin::new(&mut self.inner), cx, data);
        self.mid_write = polled.is_pending();
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.inner), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.inner), cx)
    }
}

impl Drop for SendHalf {
    fn drop(&mut self) {
        if self.mid_write {
            let _ = self.inner.reset(VarInt::from_u32(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Everyone;

    impl Trust for Everyone {
        fn is_paired(&self, _node: NodeId) -> bool {
            true
        }
    }

    /// An acceptor transport and a raw quinn connection to it from a second node, both paired with everyone.
    async fn raw_pair() -> (QuicTransport, QuicTransport, quinn::Connection) {
        let loopback: SocketAddr = ([127, 0, 0, 1], 0).into();
        let server = NodeIdentity::generate().unwrap();
        let acceptor = QuicTransport::bind(&server, Arc::new(Everyone), loopback).unwrap();
        let dialer_id = NodeIdentity::generate().unwrap();
        let dialer = QuicTransport::bind(&dialer_id, Arc::new(Everyone), loopback).unwrap();
        let rustls_config = dialer
            .credentials
            .client_config(server.node_id(), &Arc::new(Refusal::default()))
            .unwrap();
        let mut client =
            quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(rustls_config).unwrap()));
        client.transport_config(Arc::clone(&dialer.transport));
        let addr = acceptor.local_addr().unwrap();
        let connection = dialer
            .endpoint
            .connect_with(client, addr, SERVER_NAME)
            .unwrap()
            .await
            .unwrap();
        (acceptor, dialer, connection)
    }

    /// A paired peer cannot open a unidirectional stream: nothing would
    /// read it, so it would hold buffered bytes with no deadline.
    #[tokio::test]
    async fn a_peer_gets_no_unidirectional_stream() {
        let (acceptor, _dialer, connection) = raw_pair().await;
        let _accepted = acceptor.accept().await.unwrap();

        let opened = tokio::time::timeout(Duration::from_millis(500), connection.open_uni()).await;
        assert!(
            opened.is_err(),
            "the acceptor gave credit for a unidirectional stream"
        );
        assert!(connection.open_bi().await.is_ok());
    }

    /// A send half dropped while its write waits on flow control, as a
    /// reply that stalled past its limit is, resets the stream: the peer's
    /// read fails instead of the stream staying open until it reads on.
    #[tokio::test]
    async fn a_send_half_dropped_mid_write_resets_its_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (acceptor, _dialer, connection) = raw_pair().await;
        let accepted = acceptor.accept().await.unwrap();
        let (mut ask, mut answer) = connection.open_bi().await.unwrap();
        ask.write_all(b"ask").await.unwrap();
        ask.finish().unwrap();
        let mut answering = accepted.next().await.unwrap();
        let mut asked = Vec::new();
        answering.read_to_end(&mut asked).await.unwrap();
        let stalled = tokio::time::timeout(
            Duration::from_millis(200),
            answering.write_all(&vec![0u8; 8 << 20]),
        )
        .await;
        assert!(
            stalled.is_err(),
            "the peer never ran out of flow control credit"
        );
        drop(answering);

        let read = tokio::time::timeout(Duration::from_secs(2), answer.read_to_end(usize::MAX))
            .await
            .expect("the read never ended");
        assert!(
            matches!(
                read,
                Err(quinn::ReadToEndError::Read(quinn::ReadError::Reset(_)))
            ),
            "{read:?}"
        );
    }

    /// Review of PR #31: quinn bounds no connection's unread bytes by
    /// default, so a peer's requests waiting for room to be read could
    /// hold a stream window on each of its streams here. Twelve streams of
    /// 1 MiB, none read, get at most `CONNECTION_WINDOW` of their bytes
    /// onto the wire, counted from the sender.
    #[tokio::test]
    async fn a_peer_can_leave_at_most_the_connection_window_unread() {
        const STREAMS: usize = 12;
        let (acceptor, _dialer, connection) = raw_pair().await;
        let _accepted = acceptor.accept().await.unwrap();
        let mut writers = Vec::new();
        for _ in 0..STREAMS {
            let (mut send, recv) = connection.open_bi().await.unwrap();
            writers.push(tokio::spawn(async move {
                let _held = recv;
                let _written = send.write_all(&vec![7u8; 1 << 20]).await;
                std::future::pending::<()>().await;
            }));
        }
        // Wait until the sender stops sending: the window is spent. A burst
        // over loopback loses datagrams, which are sent again, so what
        // arrived is what was sent less what was lost.
        let mut sent = 0;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let stats = connection.stats();
            let now = stats.udp_tx.bytes.saturating_sub(stats.path.lost_bytes);
            if now == sent {
                break;
            }
            sent = now;
        }
        // Packet headers, acks and the handshake take a few percent more.
        let bound = CONNECTION_WINDOW + CONNECTION_WINDOW / 10 + (64 << 10);
        assert!(
            sent <= bound,
            "{sent} bytes reached a connection that read none, past {CONNECTION_WINDOW}"
        );
        for writer in writers {
            writer.abort();
        }
    }
}
