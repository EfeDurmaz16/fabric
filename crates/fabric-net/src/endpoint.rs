//! One node's endpoint: both transports on one port, QUIC first.
//!
//! Invariant: by default a dial tries QUIC first and falls back to TCP with
//! TLS only when QUIC did not complete within `quic_timeout`; the fallback is
//! reported with QUIC's failure in `Dialed::quic_failure`, never silent, so
//! the caller can show the slow path. Both transports listen on one port number,
//! so a user pastes one address at pairing. A forged handshake ends a dial
//! at once; any other refusal, such as another node at a stale address,
//! moves on to the next address.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::id::NodeId;

use crate::identity::NodeIdentity;
use crate::peers::HostPort;
use crate::quic::QuicTransport;
use crate::tcp::TcpTlsTransport;
use crate::tls::{TlsError, Trust};
use crate::transport::{Connection, DIAL_TIMEOUT, Transport, TransportError};

/// How long QUIC gets before the dial falls back to TCP with TLS.
pub const QUIC_TIMEOUT: Duration = Duration::from_secs(3);

/// How many ports a bind to port 0 tries before it reports the TCP port taken.
pub const EPHEMERAL_TRIES: usize = 8;

/// Which wires a dial tries, and in which order.
///
/// Both wires always listen; this only shapes the dialing side, so two
/// nodes with different orders still reach each other on a wire they share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialOrder {
    /// QUIC within the QUIC timeout, then TCP with TLS.
    #[default]
    QuicThenTcp,
    /// QUIC only, for a network known to pass UDP.
    QuicOnly,
    /// TCP with TLS only, for a network that drops UDP.
    TcpOnly,
}

/// A connection and, when QUIC was not used, why.
///
/// Only `Endpoint::dial` builds one, so `quic_failure` is set exactly when
/// the dial fell back from QUIC to TCP with TLS. Under `DialOrder::TcpOnly`
/// the connection is TCP with TLS and `quic_failure` is `None`.
pub struct Dialed {
    connection: Box<dyn Connection>,
    quic_failure: Option<TransportError>,
}

impl Dialed {
    /// The connection.
    #[must_use]
    pub fn connection(&self) -> &dyn Connection {
        self.connection.as_ref()
    }

    /// Hands over the connection.
    #[must_use]
    pub fn into_connection(self) -> Box<dyn Connection> {
        self.connection
    }

    /// How QUIC failed before the dial fell back to TCP with TLS; `None` when QUIC served it.
    #[must_use]
    pub fn quic_failure(&self) -> Option<&TransportError> {
        self.quic_failure.as_ref()
    }
}

/// The transports of one node bound to one port.
#[derive(Debug)]
pub struct Endpoint {
    node: NodeId,
    quic: QuicTransport,
    tcp: TcpTlsTransport,
    quic_timeout: Duration,
    order: DialOrder,
}

impl Endpoint {
    /// Binds both transports at `addr`; with port 0 QUIC picks the port and TCP follows.
    ///
    /// The port QUIC picks is free for UDP but may be taken for TCP; with
    /// port 0 the bind then picks again, up to `EPHEMERAL_TRIES` times.
    /// Call it inside a tokio runtime.
    pub fn bind(
        identity: &NodeIdentity,
        trust: &Arc<dyn Trust>,
        addr: SocketAddr,
    ) -> Result<Self, TransportError> {
        let mut tries = if addr.port() == 0 { EPHEMERAL_TRIES } else { 1 };
        loop {
            let quic = QuicTransport::bind(identity, Arc::clone(trust), addr)?;
            match TcpTlsTransport::bind(identity, Arc::clone(trust), quic.local_addr()?) {
                Ok(tcp) => {
                    return Ok(Self {
                        node: identity.node_id(),
                        quic,
                        tcp,
                        quic_timeout: QUIC_TIMEOUT,
                        order: DialOrder::default(),
                    });
                }
                Err(TransportError::Io(err))
                    if err.kind() == std::io::ErrorKind::AddrInUse && tries > 1 =>
                {
                    tries -= 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Sets how long QUIC gets before the dial falls back.
    #[must_use]
    pub fn with_quic_timeout(mut self, timeout: Duration) -> Self {
        self.quic_timeout = timeout;
        self
    }

    /// Sets which wires a dial tries.
    #[must_use]
    pub fn with_dial_order(mut self, order: DialOrder) -> Self {
        self.order = order;
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

    /// Dials `peer` at `addr` on the wires the dial order names: QUIC
    /// within `quic_timeout`, then TCP with TLS, by default.
    pub async fn dial(&self, peer: NodeId, addr: SocketAddr) -> Result<Dialed, TransportError> {
        match self.order {
            DialOrder::QuicThenTcp => {}
            DialOrder::QuicOnly => {
                let connection =
                    tokio::time::timeout(self.quic_timeout, self.quic.connect(peer, addr))
                        .await
                        .map_err(|_| TransportError::Timeout(self.quic_timeout))??;
                return Ok(Dialed {
                    connection,
                    quic_failure: None,
                });
            }
            DialOrder::TcpOnly => {
                return Ok(Dialed {
                    connection: self.tcp.connect(peer, addr).await?,
                    quic_failure: None,
                });
            }
        }
        let quic_failure =
            match tokio::time::timeout(self.quic_timeout, self.quic.connect(peer, addr)).await {
                Ok(Ok(connection)) => {
                    return Ok(Dialed {
                        connection,
                        quic_failure: None,
                    });
                }
                Ok(Err(TransportError::Tls(err))) => return Err(TransportError::Tls(err)),
                Ok(Err(err)) => err,
                Err(_) => TransportError::Timeout(self.quic_timeout),
            };
        tracing::info!(%peer, %addr, quic = %quic_failure, "falling back to tcp+tls");
        let connection = self.tcp.connect(peer, addr).await?;
        Ok(Dialed {
            connection,
            quic_failure: Some(quic_failure),
        })
    }

    /// Dials `peer` at the first of `addrs` that answers, resolving names as needed.
    ///
    /// A name lookup gets `DIAL_TIMEOUT`, as a dial does, so a resolver
    /// that does not answer, such as a tailnet name while Tailscale is
    /// down, costs one bounded wait before the next address is tried.
    ///
    /// An address where another node answers, which a stale address can
    /// do after DHCP gave it away, is skipped like one that does not
    /// answer. A handshake signed with a key the certificate does not name
    /// is a forgery, and ends the dial.
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
                    let lookup = tokio::net::lookup_host((name.as_str(), *port));
                    match tokio::time::timeout(DIAL_TIMEOUT, lookup).await {
                        Ok(Ok(iter)) => iter.collect(),
                        Ok(Err(err)) => {
                            tracing::debug!(%host, error = %err, "name did not resolve");
                            last = TransportError::Io(err);
                            continue;
                        }
                        Err(_) => {
                            tracing::debug!(%host, "name lookup timed out");
                            last = TransportError::Timeout(DIAL_TIMEOUT);
                            continue;
                        }
                    }
                }
            };
            for addr in resolved {
                match self.dial(peer, addr).await {
                    Ok(dialed) => return Ok(dialed),
                    Err(TransportError::Tls(TlsError::BadSignature)) => {
                        return Err(TlsError::BadSignature.into());
                    }
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
