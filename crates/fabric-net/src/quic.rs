//! The QUIC transport: `quinn` over the pinned TLS of the `tls` module.
//!
//! Invariant: a connection surfaces from `accept` or `connect` only after
//! quinn completed the TLS 1.3 handshake under the pinned verifiers, and the
//! node id it reports is read from the certificate that handshake checked.
//! 0-RTT is never enabled, so no application byte is replayable. Incoming
//! handshakes run in the background (the `accept` module), so a peer that
//! stalls one does not hold up the next.

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
use crate::transport::{Connection, Stream, Transport, TransportError, TransportKind};

/// A connection with no traffic for this long is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often an idle connection sends a keep-alive, so NAT bindings stay open.
pub const KEEP_ALIVE: Duration = Duration::from_secs(10);

/// A QUIC endpoint that dials and accepts under one node identity.
pub struct QuicTransport {
    endpoint: quinn::Endpoint,
    credentials: Arc<Credentials>,
    transport: Arc<quinn::TransportConfig>,
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
            .map_err(|err| TransportError::Config(err.to_string()))?;
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
            inbox,
        })
    }

    async fn dial(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> Result<Box<dyn Connection>, TransportError> {
        let refusal = Arc::new(Refusal::default());
        let rustls_config = self.credentials.client_config(peer, &refusal)?;
        let quic_config = QuicClientConfig::try_from(rustls_config)
            .map_err(|err| TransportError::Config(err.to_string()))?;
        let mut client = quinn::ClientConfig::new(Arc::new(quic_config));
        client.transport_config(self.transport.clone());
        let handshake = |err: &dyn std::fmt::Display| TransportError::Handshake {
            addr,
            reason: err.to_string(),
        };
        let connection = self
            .endpoint
            .connect_with(client, addr, SERVER_NAME)
            .map_err(|err| handshake(&err))?
            .await
            .map_err(|err| refusal.take().map_or_else(|| handshake(&err), Into::into))?;
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
async fn accept_loop(endpoint: quinn::Endpoint, handshakes: Handshakes) {
    while let Some(incoming) = endpoint.accept().await {
        let remote = incoming.remote_address();
        let handshake = async move {
            let connection = incoming.await.map_err(|err| TransportError::Handshake {
                addr: remote,
                reason: err.to_string(),
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
    let idle = IdleTimeout::try_from(IDLE_TIMEOUT)
        .map_err(|err| TransportError::Config(err.to_string()))?;
    config.max_idle_timeout(Some(idle));
    config.keep_alive_interval(Some(KEEP_ALIVE));
    Ok(config)
}

fn identity_of(connection: &quinn::Connection) -> Result<NodeId, TlsError> {
    let certs = connection
        .peer_identity()
        .and_then(|any| any.downcast::<Vec<CertificateDer<'static>>>().ok());
    peer_of(certs.as_deref().map(Vec::as_slice))
}

fn closed(err: &quinn::ConnectionError) -> TransportError {
    TransportError::Closed(err.to_string())
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
            let (send, recv) = self.inner.open_bi().await.map_err(|e| closed(&e))?;
            Ok(Stream::new(send, recv))
        })
    }

    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            let (send, recv) = self.inner.accept_bi().await.map_err(|e| closed(&e))?;
            Ok(Stream::new(send, recv))
        })
    }

    fn close(&self) {
        self.inner.close(VarInt::from_u32(0), b"close");
    }
}
