//! The TCP fallback: TLS over TCP with the stream multiplexer of the `mux` module.
//!
//! Invariant: the same pinned verifiers as QUIC decide the handshake, so a
//! peer that QUIC would refuse is refused here too, and a connection
//! surfaces only after that handshake. The ledger records which transport
//! served a resume, so this slower path is visible.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use rustls::pki_types::ServerName;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::accept::{Handshakes, Inbox};
use crate::identity::NodeIdentity;
use crate::mux::{Mux, Side};
use crate::tls::{Credentials, Refusal, TlsError, Trust, peer_of};
use crate::transport::{Connection, Transport, TransportError, TransportKind};

/// How long a dial may take by default, TCP connect and TLS handshake together.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// A TCP listener that dials and accepts under one node identity.
pub struct TcpTlsTransport {
    local_addr: SocketAddr,
    credentials: Arc<Credentials>,
    dial_timeout: Duration,
    inbox: Inbox,
}

impl std::fmt::Debug for TcpTlsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpTlsTransport")
            .field("local_addr", &self.local_addr)
            .finish_non_exhaustive()
    }
}

impl TcpTlsTransport {
    /// Binds a TCP listener at `addr` that accepts paired peers and dials as `identity`.
    ///
    /// Call it inside a tokio runtime.
    pub fn bind(
        identity: &NodeIdentity,
        trust: Arc<dyn Trust>,
        addr: SocketAddr,
    ) -> Result<Self, TransportError> {
        let credentials = Arc::new(Credentials::new(identity)?);
        let acceptor = TlsAcceptor::from(Arc::new(credentials.server_config(Arc::clone(&trust))?));
        let std_listener = std::net::TcpListener::bind(addr)?;
        std_listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(std_listener)?;
        let local_addr = listener.local_addr()?;
        let inbox = Inbox::spawn(trust, |handshakes| {
            accept_loop(listener, acceptor, handshakes)
        });
        Ok(Self {
            local_addr,
            credentials,
            dial_timeout: DIAL_TIMEOUT,
            inbox,
        })
    }

    /// Sets how long a dial may take, TCP connect and TLS handshake together.
    #[must_use]
    pub fn with_dial_timeout(mut self, timeout: Duration) -> Self {
        self.dial_timeout = timeout;
        self
    }

    /// Dials within the dial timeout: an address that drops SYNs costs that
    /// long, not the operating system's connect timeout of a minute or more.
    async fn dial(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> Result<Box<dyn Connection>, TransportError> {
        tokio::time::timeout(self.dial_timeout, self.connect_and_handshake(peer, addr))
            .await
            .map_err(|_| TransportError::Timeout(self.dial_timeout))?
    }

    async fn connect_and_handshake(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> Result<Box<dyn Connection>, TransportError> {
        let handshake = |err: &dyn std::fmt::Display| TransportError::Handshake {
            addr,
            reason: err.to_string(),
        };
        let tcp = TcpStream::connect(addr).await?;
        tcp.set_nodelay(true)?;
        let refusal = Arc::new(Refusal::default());
        let connector =
            TlsConnector::from(Arc::new(self.credentials.client_config(peer, &refusal)?));
        let name = ServerName::IpAddress(addr.ip().into());
        let tls = connector
            .connect(name, tcp)
            .await
            .map_err(|err| refusal.take().map_or_else(|| handshake(&err), Into::into))?;
        let actual = peer_of(tls.get_ref().1.peer_certificates())?;
        if actual != peer {
            return Err(TlsError::PeerMismatch {
                expected: peer,
                actual,
            }
            .into());
        }
        Ok(Box::new(Mux::spawn(
            TlsStream::Client(tls),
            Side::Dialer,
            peer,
            addr,
        )))
    }
}

/// How long the accept loop waits after the listener fails, for example out of file descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// How long an accepted socket may stay silent before its first handshake byte.
///
/// A dialer sends its `ClientHello` right after connecting, so this is far
/// shorter than `HANDSHAKE_TIMEOUT`: a host that connects and says nothing
/// gives its handshake slot back quickly.
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(2);

/// Hands every accepted socket to `handshakes` until the transport is dropped.
async fn accept_loop(listener: TcpListener, acceptor: TlsAcceptor, handshakes: Handshakes) {
    loop {
        let (tcp, remote) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                tracing::warn!(error = %err, "tcp accept failed");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        if let Err(err) = tcp.set_nodelay(true) {
            tracing::debug!(%remote, error = %err, "tcp nodelay refused");
        }
        let acceptor = acceptor.clone();
        let handshake = async move {
            let mut first = [0u8; 1];
            match tokio::time::timeout(FIRST_BYTE_TIMEOUT, tcp.peek(&mut first)).await {
                Ok(Ok(0)) => {
                    return Err(TransportError::Closed(
                        "closed before its handshake".to_owned(),
                    ));
                }
                Ok(Ok(_)) => {}
                Ok(Err(err)) => return Err(err.into()),
                Err(_) => return Err(TransportError::Timeout(FIRST_BYTE_TIMEOUT)),
            }
            let tls = acceptor.accept(tcp).await?;
            let peer = peer_of(tls.get_ref().1.peer_certificates())?;
            Ok(Box::new(Mux::spawn(
                TlsStream::Server(tls),
                Side::Acceptor,
                peer,
                remote,
            )) as Box<dyn Connection>)
        };
        if !handshakes.run(remote, handshake).await {
            return;
        }
    }
}

impl Transport for TcpTlsTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::TcpTls
    }

    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.local_addr)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[derive(Debug)]
    struct NoOne;

    impl Trust for NoOne {
        fn is_paired(&self, _node: NodeId) -> bool {
            false
        }
    }

    /// A host that connects and sends nothing loses its socket after
    /// `FIRST_BYTE_TIMEOUT`, not the whole handshake timeout, so silent
    /// sockets give their handshake slots back quickly.
    #[tokio::test]
    async fn a_socket_silent_past_the_first_byte_timeout_is_closed() {
        let identity = NodeIdentity::generate().unwrap();
        let transport =
            TcpTlsTransport::bind(&identity, Arc::new(NoOne), ([127, 0, 0, 1], 0).into()).unwrap();
        let mut silent = TcpStream::connect(transport.local_addr).await.unwrap();
        let started = tokio::time::Instant::now();
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(FIRST_BYTE_TIMEOUT * 2, silent.read(&mut byte)).await;
        assert!(
            matches!(read, Ok(Ok(0) | Err(_))),
            "the server must close the silent socket"
        );
        assert!(started.elapsed() + Duration::from_millis(100) >= FIRST_BYTE_TIMEOUT);
    }
}
