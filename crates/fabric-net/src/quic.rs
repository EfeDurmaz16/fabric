//! The QUIC transport: `quinn` over the pinned TLS of the `tls` module.
//!
//! Invariant: a connection surfaces from `accept` or `connect` only after
//! quinn completed the TLS 1.3 handshake under the pinned verifiers, and the
//! node id it reports is read from the certificate that handshake checked.
//! 0-RTT is never enabled, so no application byte is replayable. Incoming
//! handshakes run in the background (the `accept` module), so a peer that
//! stalls one does not hold up the next, and only after the peer's address
//! answered a Retry, so a spoofed source takes no handshake slot.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{IdleTimeout, VarInt};
use rustls::pki_types::CertificateDer;

use crate::accept::{Handshakes, Inbox};
use crate::identity::NodeIdentity;
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

fn transport_config() -> Result<quinn::TransportConfig, TransportError> {
    let mut config = quinn::TransportConfig::default();
    let idle =
        IdleTimeout::try_from(IDLE_TIMEOUT).map_err(|err| TransportError::Config(Box::new(err)))?;
    config.max_idle_timeout(Some(idle));
    config.keep_alive_interval(Some(KEEP_ALIVE));
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
            Ok(Stream::new(send, recv))
        })
    }

    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            let (send, recv) = self.inner.accept_bi().await.map_err(closed)?;
            Ok(Stream::new(send, recv))
        })
    }

    fn close(&self) {
        self.inner.close(VarInt::from_u32(0), b"close");
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

    /// A paired peer cannot open a unidirectional stream: nothing would
    /// read it, so it would hold buffered bytes with no deadline.
    #[tokio::test]
    async fn a_peer_gets_no_unidirectional_stream() {
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
        let _accepted = acceptor.accept().await.unwrap();

        let opened = tokio::time::timeout(Duration::from_millis(500), connection.open_uni()).await;
        assert!(
            opened.is_err(),
            "the acceptor gave credit for a unidirectional stream"
        );
        assert!(connection.open_bi().await.is_ok());
    }
}
