//! The network layer of Fabric (ADR 0005): node identity, pinned TLS, two
//! transports, framing, typed RPC and the peer book.
//!
//! Invariant: an application byte is read from or written to a peer only
//! after a TLS 1.3 handshake in which the peer proved it holds the private
//! key of a node id this node expects: the id it dialed on the client side,
//! an id it paired with on the server side. No CA, no name and no date take
//! part in that decision. The node id is the public key, and pairing is the
//! trust. Everything above this layer is fenced by the ledger epoch.
//!
//! Nothing here needs an account, a relay or a coordination server. Two
//! nodes that can reach each other's address can work, over a LAN, a public
//! address or a tailnet alike; `docs/net.md` says how to run each.

mod accept;
mod checkpoint;
#[cfg(test)]
mod crash_tests;
mod durable;
pub mod endpoint;
pub mod frame;
pub mod identity;
mod idle;
pub mod mux;
pub mod peers;
mod pipe;
pub mod quic;
pub mod rpc;
pub mod tcp;
pub mod tls;
pub mod transport;
pub mod wire;
