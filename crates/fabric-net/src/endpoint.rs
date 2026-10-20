//! One node's endpoint: both transports on one port, QUIC first.
//!
//! Invariant: a dial tries QUIC first and falls back to TCP with TLS only
//! when QUIC did not complete within `quic_timeout`; the fallback is
//! reported with its reason, never silent, so a slow path is visible to
//! the user and to the ledger. Both transports listen on one port number,
//! so a user pastes one address at pairing.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::id::NodeId;

use crate::identity::NodeIdentity;
use crate::peers::HostPort;
use crate::quic::QuicTransport;
use crate::tcp::TcpTlsTransport;
use crate::tls::Trust;
use crate::transport::{Connection, Transport, TransportError};

/// How long QUIC gets before the dial falls back to TCP with TLS.
pub const QUIC_TIMEOUT: Duration = Duration::from_secs(3);

/// A connection and, when QUIC was not used, why.
pub struct Dialed {
    /// The connection.
    pub connection: Box<dyn Connection>,
    /// Why the dial fell back to TCP with TLS; `None` when QUIC served it.
    pub fallback_reason: Option<String>,
}

/// The transports of one node bound to one port.
#[derive(Debug)]
pub struct Endpoint {
    node: NodeId,
    quic: QuicTransport,
    tcp: TcpTlsTransport,
    quic_timeout: Duration,
}

impl Endpoint {
    /// Binds both transports at `addr`; with port 0 QUIC picks the port and TCP follows.
    ///
    /// Call it inside a tokio runtime.
    pub fn bind(
        identity: &NodeIdentity,
        trust: Arc<dyn Trust>,
        addr: SocketAddr,
    ) -> Result<Self, TransportError> {
        let quic = QuicTransport::bind(identity, Arc::clone(&trust), addr)?;
        let tcp = TcpTlsTransport::bind(identity, trust, quic.local_addr()?)?;
        Ok(Self {
            node: identity.node_id(),
            quic,
            tcp,
            quic_timeout: QUIC_TIMEOUT,
        })
    }

    /// Sets how long QUIC gets before the dial falls back.
    #[must_use]
    pub fn with_quic_timeout(mut self, timeout: Duration) -> Self {
        self.quic_timeout = timeout;
        self
    }

    /// This node's id.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.node
    }

    /// The address both transports listen on.
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.quic.local_addr()
    }

    /// Dials `peer` at `addr`: QUIC within `quic_timeout`, then TCP with TLS.
    pub async fn dial(&self, peer: NodeId, addr: SocketAddr) -> Result<Dialed, TransportError> {
        let quic_failure =
            match tokio::time::timeout(self.quic_timeout, self.quic.connect(peer, addr)).await {
                Ok(Ok(connection)) => {
                    return Ok(Dialed {
                        connection,
                        fallback_reason: None,
                    });
                }
                Ok(Err(TransportError::Tls(err))) => return Err(TransportError::Tls(err)),
                Ok(Err(err)) => err,
                Err(_) => TransportError::Timeout(self.quic_timeout),
            };
        let reason = format!("quic: {quic_failure}");
        tracing::info!(%peer, %addr, %reason, "falling back to tcp+tls");
        let connection = self.tcp.connect(peer, addr).await?;
        Ok(Dialed {
            connection,
            fallback_reason: Some(reason),
        })
    }

    /// Dials `peer` at the first of `addrs` that answers, resolving names as needed.
    pub async fn dial_any(
        &self,
        peer: NodeId,
        addrs: &[HostPort],
    ) -> Result<Dialed, TransportError> {
        let mut last = TransportError::NoAddress(peer);
        for host in addrs {
            let resolved: Vec<SocketAddr> = match host {
                HostPort::Addr(addr) => vec![*addr],
                HostPort::Name(name, port) => {
                    match tokio::net::lookup_host((name.as_str(), *port)).await {
                        Ok(iter) => iter.collect(),
                        Err(err) => {
                            tracing::debug!(%host, error = %err, "name did not resolve");
                            last = TransportError::Io(err);
                            continue;
                        }
                    }
                }
            };
            for addr in resolved {
                match self.dial(peer, addr).await {
                    Ok(dialed) => return Ok(dialed),
                    Err(TransportError::Tls(err)) => return Err(TransportError::Tls(err)),
                    Err(err) => {
                        tracing::debug!(%peer, %addr, error = %err, "dial failed");
                        last = err;
                    }
                }
            }
        }
        Err(last)
    }

    /// Waits for the next connection from a paired peer on either transport.
    ///
    /// Both transports finish handshakes in the background and only queue
    /// finished connections, so the transport that loses the race keeps
    /// its connection for the next call.
    pub async fn accept(&self) -> Result<Box<dyn Connection>, TransportError> {
        tokio::select! {
            quic = self.quic.accept() => quic,
            tcp = self.tcp.accept() => tcp,
        }
    }
}
