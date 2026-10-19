//! The peer book: which nodes this node trusts and where it last found them.
//!
//! Invariant: a node id enters the book only through pairing, which the user
//! does with an id read from the other machine's screen. The book is the
//! only input to the accept-side trust decision, it is written whole and
//! atomically, and its text form is one line per peer so a user can read it.
//!
//! Pairing v0 is out of band: `fabric node pair <id>@<host>:<port>` on each
//! side. A short pairing code, where one side shows six words and the other
//! types them to fetch and confirm the id over the connection, is the
//! documented follow-up; it needs a key exchange that this phase does not.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use fabric_core::id::{NodeId, ParseIdError};

use crate::tls::Trust;

/// The most addresses kept per peer; the newest address is first.
pub const MAX_ADDRS: usize = 8;

/// Why the peer book or a peer spec could not be read.
#[derive(Debug, thiserror::Error)]
pub enum PeerBookError {
    /// The book file could not be read or written.
    #[error("peer book {path}: {source}")]
    Io {
        /// The book file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// A line of the book, or a pasted spec, does not parse.
    #[error("line {line}: {reason}")]
    Parse {
        /// The one-based line number, or 1 for a pasted spec.
        line: usize,
        /// What was wrong.
        reason: String,
    },
    /// The node id did not parse.
    #[error("node id: {0}")]
    NodeId(#[from] ParseIdError),
    /// A spec names no address.
    #[error("a peer spec is <node-id>@<host>:<port>")]
    Spec,
}

/// A host and port as the user pasted it: an IP address or a name resolved at dial time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostPort {
    /// The host: an IPv4 or IPv6 address, or a DNS name such as a tailnet name.
    pub host: String,
    /// The port.
    pub port: u16,
}

impl HostPort {
    /// Returns the address directly when the host is an IP literal.
    #[must_use]
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.to_string().parse().ok()
    }
}

impl From<SocketAddr> for HostPort {
    fn from(addr: SocketAddr) -> Self {
        Self {
            host: addr.ip().to_string(),
            port: addr.port(),
        }
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

impl FromStr for HostPort {
    type Err = PeerBookError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || PeerBookError::Parse {
            line: 1,
            reason: format!("{s:?} is not <host>:<port>"),
        };
        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(addr.into());
        }
        let (host, port) = s.rsplit_once(':').ok_or_else(bad)?;
        let port: u16 = port.parse().map_err(|_| bad())?;
        let host_ok = !host.is_empty()
            && host.len() <= 253
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !host_ok {
            return Err(bad());
        }
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}

/// What the book knows about one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEntry {
    /// The peer's node id.
    pub node: NodeId,
    /// Where it was last reachable, newest first; empty for a peer that only dials in.
    pub addrs: Vec<HostPort>,
    /// When a connection with it last completed, in milliseconds since the Unix epoch.
    pub last_seen_ms: Option<u64>,
}

impl fmt::Display for PeerEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.node)?;
        let addrs: Vec<String> = self.addrs.iter().map(ToString::to_string).collect();
        write!(
            f,
            " {}",
            if addrs.is_empty() {
                "-".to_owned()
            } else {
                addrs.join(",")
            }
        )?;
        match self.last_seen_ms {
            Some(ms) => write!(f, " {ms}"),
            None => f.write_str(" -"),
        }
    }
}

impl FromStr for PeerEntry {
    type Err = PeerBookError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut fields = s.split_ascii_whitespace();
        let bad = |reason: &str| PeerBookError::Parse {
            line: 1,
            reason: reason.to_owned(),
        };
        let node: NodeId = fields.next().ok_or_else(|| bad("empty line"))?.parse()?;
        let addrs = fields.next().ok_or_else(|| bad("missing addresses"))?;
        let addrs = if addrs == "-" {
            Vec::new()
        } else {
            addrs
                .split(',')
                .map(str::parse)
                .collect::<Result<Vec<HostPort>, _>>()?
        };
        let last_seen_ms = match fields.next().ok_or_else(|| bad("missing last seen"))? {
            "-" => None,
            ms => Some(ms.parse().map_err(|_| bad("last seen is not a number"))?),
        };
        if fields.next().is_some() {
            return Err(bad("extra fields"));
        }
        Ok(Self {
            node,
            addrs,
            last_seen_ms,
        })
    }
}

/// A pasted pairing spec: `<node-id>@<host>:<port>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSpec {
    /// The peer's node id.
    pub node: NodeId,
    /// Where to dial it.
    pub addr: HostPort,
}

impl FromStr for PeerSpec {
    type Err = PeerBookError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (node, addr) = s.split_once('@').ok_or(PeerBookError::Spec)?;
        Ok(Self {
            node: node.parse()?,
            addr: addr.parse()?,
        })
    }
}

impl fmt::Display for PeerSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.node, self.addr)
    }
}

/// The paired peers, as a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerBook {
    peers: BTreeMap<NodeId, PeerEntry>,
}

impl PeerBook {
    /// Parses the text form: one `PeerEntry` per line.
    pub fn parse(text: &str) -> Result<Self, PeerBookError> {
        let mut peers = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: PeerEntry = line.parse().map_err(|err| match err {
                PeerBookError::Parse { reason, .. } => PeerBookError::Parse {
                    line: index + 1,
                    reason,
                },
                other => other,
            })?;
            peers.insert(entry.node, entry);
        }
        Ok(Self { peers })
    }

    /// Reads the book at `path`; a missing file is an empty book.
    pub fn load(path: &Path) -> Result<Self, PeerBookError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(PeerBookError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Writes the book to `path` whole: a sibling temporary file, synced, then renamed over.
    pub fn save(&self, path: &Path) -> Result<(), PeerBookError> {
        let io = |source| PeerBookError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut name = path
            .file_name()
            .map_or_else(Default::default, std::ffi::OsStr::to_os_string);
        name.push(".tmp");
        let tmp = path.with_file_name(name);
        let written = (|| {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(self.to_string().as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp, path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written.map_err(io)
    }

    /// Pairs `node`, adding `addr` in front of the addresses already known.
    pub fn pair(&mut self, node: NodeId, addr: Option<HostPort>) {
        let entry = self.peers.entry(node).or_insert(PeerEntry {
            node,
            addrs: Vec::new(),
            last_seen_ms: None,
        });
        if let Some(addr) = addr {
            promote(&mut entry.addrs, addr);
        }
    }

    /// Forgets `node`; returns whether it was paired.
    pub fn unpair(&mut self, node: NodeId) -> bool {
        self.peers.remove(&node).is_some()
    }

    /// Records that a connection with `node` completed at `addr`; ignored for an unpaired node.
    pub fn seen(&mut self, node: NodeId, addr: SocketAddr, now_ms: u64) {
        if let Some(entry) = self.peers.get_mut(&node) {
            promote(&mut entry.addrs, addr.into());
            entry.last_seen_ms = Some(now_ms);
        }
    }

    /// Returns the entry for `node`.
    #[must_use]
    pub fn get(&self, node: NodeId) -> Option<&PeerEntry> {
        self.peers.get(&node)
    }

    /// Returns true when `node` is paired.
    #[must_use]
    pub fn is_paired(&self, node: NodeId) -> bool {
        self.peers.contains_key(&node)
    }

    /// Returns every peer, ordered by node id.
    pub fn iter(&self) -> impl Iterator<Item = &PeerEntry> {
        self.peers.values()
    }
}

fn promote(addrs: &mut Vec<HostPort>, addr: HostPort) {
    addrs.retain(|a| *a != addr);
    addrs.insert(0, addr);
    addrs.truncate(MAX_ADDRS);
}

impl fmt::Display for PeerBook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for entry in self.peers.values() {
            writeln!(f, "{entry}")?;
        }
        Ok(())
    }
}

/// The peer book behind a lock and a file: every change is saved before it is visible.
#[derive(Debug)]
pub struct Peers {
    path: PathBuf,
    book: Mutex<PeerBook>,
}

impl Peers {
    /// Opens the book at `path`, creating an empty one in memory if the file is missing.
    pub fn open(path: impl Into<PathBuf>) -> Result<Arc<Self>, PeerBookError> {
        let path = path.into();
        let book = PeerBook::load(&path)?;
        Ok(Arc::new(Self {
            path,
            book: Mutex::new(book),
        }))
    }

    /// Returns a copy of the book.
    #[must_use]
    pub fn snapshot(&self) -> PeerBook {
        self.lock().clone()
    }

    /// Applies `change` to the book and saves it; the change is kept only if the save succeeds.
    pub fn update(&self, change: impl FnOnce(&mut PeerBook)) -> Result<(), PeerBookError> {
        let mut book = self.lock();
        let mut next = book.clone();
        change(&mut next);
        if next != *book {
            next.save(&self.path)?;
            *book = next;
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PeerBook> {
        // The book is a plain value; a poisoned lock still holds a whole book.
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Trust for Peers {
    fn is_paired(&self, node: NodeId) -> bool {
        self.lock().is_paired(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    #[test]
    fn a_spec_parses_an_ip_or_a_name_and_nothing_else() {
        let id = node(1);
        let spec: PeerSpec = format!("{id}@192.168.1.20:7777").parse().unwrap();
        assert_eq!(
            spec.addr.socket_addr(),
            Some("192.168.1.20:7777".parse().unwrap())
        );
        let spec: PeerSpec = format!("{id}@air.tail1234.ts.net:7777").parse().unwrap();
        assert_eq!(spec.addr.host, "air.tail1234.ts.net");
        assert_eq!(spec.addr.socket_addr(), None);
        let spec: PeerSpec = format!("{id}@[fd7a::1]:7777").parse().unwrap();
        assert_eq!(spec.addr.to_string(), "[fd7a::1]:7777");
        assert!(matches!(
            format!("{id}@no-port").parse::<PeerSpec>(),
            Err(PeerBookError::Parse { .. })
        ));
        assert!(matches!("x".parse::<PeerSpec>(), Err(PeerBookError::Spec)));
        assert!(matches!(
            "fab1xyz@1.2.3.4:5".parse::<PeerSpec>(),
            Err(PeerBookError::NodeId(_))
        ));
    }

    #[test]
    fn the_book_round_trips_through_its_file_and_a_bad_line_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers");
        let peers = Peers::open(&path).unwrap();
        peers
            .update(|b| b.pair(node(2), Some("10.0.0.2:7777".parse().unwrap())))
            .unwrap();
        peers
            .update(|b| b.seen(node(2), "10.0.0.9:7777".parse().unwrap(), 1_760_000_000_000))
            .unwrap();
        peers.update(|b| b.pair(node(3), None)).unwrap();
        assert!(peers.is_paired(node(2)));
        assert!(!peers.is_paired(node(4)));

        let reloaded = PeerBook::load(&path).unwrap();
        assert_eq!(reloaded, peers.snapshot());
        let entry = reloaded.get(node(2)).unwrap();
        assert_eq!(entry.addrs[0].to_string(), "10.0.0.9:7777");
        assert_eq!(entry.addrs.len(), 2);
        assert_eq!(entry.last_seen_ms, Some(1_760_000_000_000));
        assert!(reloaded.get(node(3)).unwrap().addrs.is_empty());

        let text = format!("{reloaded}\nnot a peer\n");
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::NodeId(_))
        ));
        let text = format!("{} extra\n", node(5));
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::Parse { line: 1, .. })
        ));
    }

    #[test]
    fn an_unpaired_node_is_not_recorded_as_seen() {
        let mut book = PeerBook::default();
        book.seen(node(7), "10.0.0.7:1".parse().unwrap(), 5);
        assert!(!book.is_paired(node(7)));
        book.pair(node(7), None);
        assert!(book.unpair(node(7)));
        assert!(!book.unpair(node(7)));
    }

    #[test]
    fn a_failed_save_keeps_the_old_book() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-dir").join("peers");
        let peers = Peers::open(&path).unwrap();
        assert!(peers.update(|b| b.pair(node(1), None)).is_err());
        assert!(!peers.is_paired(node(1)));
    }

    proptest! {
        #[test]
        fn entries_round_trip_as_text(
            bytes in any::<[u8; 32]>(),
            addrs in proptest::collection::vec(any::<SocketAddr>(), 0..3),
            seen in proptest::option::of(any::<u64>()),
        ) {
            let entry = PeerEntry {
                node: NodeId::from_bytes(bytes),
                addrs: addrs.into_iter().map(HostPort::from).collect(),
                last_seen_ms: seen,
            };
            prop_assert_eq!(entry.to_string().parse::<PeerEntry>().unwrap(), entry);
        }
    }
}
