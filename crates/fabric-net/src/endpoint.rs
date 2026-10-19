//! One node's endpoint: every enabled transport on one port, QUIC first.
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
#[cfg(feature = "quic")]
use crate::quic::QuicTransport;
#[cfg(feature = "tcp_tls")]
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
    #[cfg(feature = "quic")]
    quic: QuicTransport,
    #[cfg(feature = "tcp_tls")]
    tcp: TcpTlsTransport,
    quic_timeout: Duration,
}

impl Endpoint {
    /// Binds every enabled transport at `addr`; with port 0 the first transport picks the port and the rest follow.
    ///
    /// Call it inside a tokio runtime.
    pub fn bind(
        identity: &NodeIdentity,
        trust: Arc<dyn Trust>,
        addr: SocketAddr,
    ) -> Result<Self, TransportError> {
        #[cfg(all(feature = "quic", feature = "tcp_tls"))]
        let quic_trust = Arc::clone(&trust);
        #[cfg(all(feature = "quic", not(feature = "tcp_tls")))]
        let quic_trust = trust;
        #[cfg(feature = "quic")]
        let quic = QuicTransport::bind(identity, quic_trust, addr)?;
        #[cfg(feature = "tcp_tls")]
        let tcp = {
            #[cfg(feature = "quic")]
            let addr = quic.local_addr()?;
            TcpTlsTransport::bind(identity, trust, addr)?
        };
        Ok(Self {
            node: identity.node_id(),
            #[cfg(feature = "quic")]
            quic,
            #[cfg(feature = "tcp_tls")]
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

    /// The address every transport listens on.
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        #[cfg(feature = "quic")]
        let addr = self.quic.local_addr();
        #[cfg(all(feature = "tcp_tls", not(feature = "quic")))]
        let addr = self.tcp.local_addr();
        addr
    }

    /// Dials `peer` at `addr`: QUIC within `quic_timeout`, then TCP with TLS.
    pub async fn dial(&self, peer: NodeId, addr: SocketAddr) -> Result<Dialed, TransportError> {
        #[cfg(feature = "quic")]
        let quic_failure = {
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
            }
        };
        #[cfg(not(feature = "quic"))]
        let quic_failure = TransportError::Config("quic is not built in".to_owned());
        #[cfg(feature = "tcp_tls")]
        {
            let reason = format!("quic: {quic_failure}");
            tracing::info!(%peer, %addr, %reason, "falling back to tcp+tls");
            let connection = self.tcp.connect(peer, addr).await?;
            Ok(Dialed {
                connection,
                fallback_reason: Some(reason),
            })
        }
        #[cfg(not(feature = "tcp_tls"))]
        {
            Err(quic_failure)
        }
    }

    /// Dials `peer` at the first of `addrs` that answers, resolving names as needed.
    pub async fn dial_any(
        &self,
        peer: NodeId,
        addrs: &[HostPort],
    ) -> Result<Dialed, TransportError> {
        let mut last = TransportError::NoAddress(peer);
        for host in addrs {
            let resolved: Vec<SocketAddr> = match host.socket_addr() {
                Some(addr) => vec![addr],
                None => match tokio::net::lookup_host(host.to_string()).await {
                    Ok(iter) => iter.collect(),
                    Err(err) => {
                        tracing::debug!(%host, error = %err, "name did not resolve");
                        last = TransportError::Io(err);
                        continue;
                    }
                },
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

    /// Waits for the next connection from a paired peer on any transport.
    pub async fn accept(&self) -> Result<Box<dyn Connection>, TransportError> {
        #[cfg(all(feature = "quic", feature = "tcp_tls"))]
        {
            tokio::select! {
                quic = self.quic.accept() => quic,
                tcp = self.tcp.accept() => tcp,
            }
        }
        #[cfg(all(feature = "quic", not(feature = "tcp_tls")))]
        {
            self.quic.accept().await
        }
        #[cfg(all(feature = "tcp_tls", not(feature = "quic")))]
        {
            self.tcp.accept().await
        }
    }
}
