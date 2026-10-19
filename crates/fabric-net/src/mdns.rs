//! LAN discovery with mDNS, used only to refresh the address of a paired node.
//!
//! Invariant: discovery never adds trust. A node seen on the LAN is reported
//! with the id it claims; the caller looks that id up in the peer book, and
//! the handshake proves it before any byte is exchanged. A node that is not
//! paired is listed as unpaired and never dialed.

use std::net::SocketAddr;

use fabric_core::id::NodeId;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

/// The DNS-SD service type Fabric advertises.
pub const SERVICE_TYPE: &str = "_fabric._udp.local.";

/// The TXT key that carries the node id.
pub const TXT_NODE: &str = "node";

/// Why mDNS could not start or run.
#[derive(Debug, thiserror::Error)]
pub enum MdnsError {
    /// The mDNS daemon refused.
    #[error("mdns: {0}")]
    Daemon(#[source] mdns_sd::Error),
}

/// Advertises this node on the LAN until dropped.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl std::fmt::Debug for Advertiser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertiser")
            .field("fullname", &self.fullname)
            .finish_non_exhaustive()
    }
}

impl Advertiser {
    /// Starts advertising `node` on `port` with the addresses of every interface.
    pub fn start(node: NodeId, port: u16) -> Result<Self, MdnsError> {
        let daemon = ServiceDaemon::new().map_err(MdnsError::Daemon)?;
        let text = node.to_string();
        let host = format!("{}.local.", &text[..16]);
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &text,
            &host,
            "",
            port,
            &[(TXT_NODE, text.as_str())][..],
        )
        .map_err(MdnsError::Daemon)?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info).map_err(MdnsError::Daemon)?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// A node seen on the LAN: the id it claims and where it says it listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The claimed id; proved only by a handshake.
    pub node: NodeId,
    /// Its advertised addresses, loopback left out.
    pub addrs: Vec<SocketAddr>,
}

/// Watches the LAN for Fabric nodes until dropped.
pub struct Browser {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Browser")
    }
}

impl Browser {
    /// Starts browsing for Fabric nodes.
    pub fn start() -> Result<Self, MdnsError> {
        let daemon = ServiceDaemon::new().map_err(MdnsError::Daemon)?;
        let events = daemon.browse(SERVICE_TYPE).map_err(MdnsError::Daemon)?;
        Ok(Self { daemon, events })
    }

    /// Waits for the next node that resolves; `None` once the daemon stops.
    pub async fn next(&self) -> Option<Seen> {
        loop {
            let event = self.events.recv_async().await.ok()?;
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            let Some(node) = info
                .txt_properties
                .get_property_val_str(TXT_NODE)
                .and_then(|text| text.parse::<NodeId>().ok())
            else {
                tracing::debug!(fullname = %info.fullname, "mdns record without a node id");
                continue;
            };
            let mut addrs: Vec<SocketAddr> = info
                .addresses
                .iter()
                .filter(|ip| !ip.is_loopback())
                .map(|ip| SocketAddr::new(ip.to_ip_addr(), info.port))
                .collect();
            addrs.sort();
            return Some(Seen { node, addrs });
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    #[ignore = "needs multicast on a real interface; run by hand on a LAN"]
    async fn an_advertised_node_is_seen_with_its_id() {
        let node = NodeId::from_bytes([0x42; 32]);
        let _advertiser = Advertiser::start(node, 7777).unwrap();
        let browser = Browser::start().unwrap();
        let seen = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match browser.next().await {
                    Some(seen) if seen.node == node => return seen,
                    Some(_) => {}
                    None => panic!("browser stopped"),
                }
            }
        })
        .await
        .unwrap();
        assert!(seen.addrs.iter().all(|a| a.port() == 7777));
    }
}
