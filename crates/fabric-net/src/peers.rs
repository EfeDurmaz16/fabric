//! The peer book: which nodes this node trusts and where it last found them.
//!
//! Invariant: a node id enters the book only through pairing, which the user
//! does with an id read from the other machine's screen. The book is the
//! only input to the accept-side trust decision, it is written whole and
//! atomically, and its text form is one line per peer so a user can read it.
//! Two processes share one book file: `fabric node pair` runs by hand while
//! the daemon serves. Every change therefore reloads the file under an
//! exclusive lock before it applies, so no writer overwrites another's
//! pairing, and `refresh`, which the accept path runs before every
//! handshake, re-reads the file and parses it again when its bytes changed,
//! so a pairing made after the daemon started is honoured at the next
//! handshake. The bytes are the stamp: a length or a modified time can stay
//! the same across an edit made in place, the bytes cannot, so the cache
//! never holds a book under content it was not parsed from. The check
//! inside the handshake reads only the book in memory.
//!
//! Pairing v0 is out of band: `fabric node pair <id>@<host>:<port>` on each
//! side. A short pairing code, where one side shows six words and the other
//! types them to fetch and confirm the id over the connection, is the
//! documented follow-up; it needs a key exchange that this phase does not.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use fabric_core::id::{NodeId, ParseIdError};

use crate::checkpoint;
use crate::durable::{sync_parent, temp_sibling};
use crate::tls::Trust;

/// The most addresses kept per peer; the newest address is first.
pub const MAX_ADDRS: usize = 8;

/// Why a pasted spec, an address or one line of the book does not parse.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The node id did not parse.
    #[error("node id: {0}")]
    NodeId(#[from] ParseIdError),
    /// The host is neither an address nor a DNS name.
    #[error("{0:?} is not a DNS name")]
    DnsName(String),
    /// The text is not `<host>:<port>`.
    #[error("{0:?} is not <host>:<port>")]
    HostPort(String),
    /// A spec names no address.
    #[error("a peer spec is <node-id>@<host>:<port>")]
    Spec,
    /// A book line lacks a field, has one too many, or has one of the wrong kind.
    #[error("a book line is <node-id> <addresses> <last-seen>: {0}")]
    Fields(&'static str),
    /// A book line lists more than `MAX_ADDRS` addresses.
    #[error("{0} addresses, more than the {MAX_ADDRS} kept")]
    TooManyAddrs(usize),
    /// A book line lists one address twice.
    #[error("{0} is listed twice")]
    DuplicateAddr(HostPort),
}

/// Why the peer book could not be read or written.
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
    /// A line of the book does not parse.
    #[error("line {line}: {source}")]
    Line {
        /// The one-based line number.
        line: usize,
        /// What was wrong.
        #[source]
        source: SpecError,
    },
    /// A node is listed on two lines.
    #[error("line {line}: {node} is already on line {first}")]
    DuplicateNode {
        /// The one-based line number of the second listing.
        line: usize,
        /// The line that listed it first.
        first: usize,
        /// The node.
        node: NodeId,
    },
}

/// A DNS name as the user pasted it: 1 to 253 bytes of letters, digits, dots and dashes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DnsName(String);

impl DnsName {
    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DnsName {
    type Err = SpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ok = !s.is_empty()
            && s.len() <= 253
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(SpecError::DnsName(s.to_owned()))
        }
    }
}

impl fmt::Display for DnsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a peer listens: an address, or a name resolved at dial time.
///
/// A parsed value is valid by construction: an address is a `SocketAddr`,
/// scope id included, and a name is a `DnsName`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostPort {
    /// An IPv4 or IPv6 address with its port.
    Addr(SocketAddr),
    /// A DNS name, such as a tailnet name, with its port.
    Name(DnsName, u16),
}

impl HostPort {
    /// Returns the address when no name has to be resolved.
    #[must_use]
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        match self {
            HostPort::Addr(addr) => Some(*addr),
            HostPort::Name(..) => None,
        }
    }

    /// Returns the port.
    #[must_use]
    pub fn port(&self) -> u16 {
        match self {
            HostPort::Addr(addr) => addr.port(),
            HostPort::Name(_, port) => *port,
        }
    }
}

impl From<SocketAddr> for HostPort {
    /// Keeps the scope id of an IPv6 address and drops its flow info, which has no text form.
    fn from(mut addr: SocketAddr) -> Self {
        if let SocketAddr::V6(v6) = &mut addr {
            v6.set_flowinfo(0);
        }
        Self::Addr(addr)
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostPort::Addr(addr) => write!(f, "{addr}"),
            HostPort::Name(name, port) => write!(f, "{name}:{port}"),
        }
    }
}

impl FromStr for HostPort {
    type Err = SpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(Self::Addr(addr));
        }
        let bad = || SpecError::HostPort(s.to_owned());
        let (host, port) = s.rsplit_once(':').ok_or_else(bad)?;
        let port: u16 = port.parse().map_err(|_| bad())?;
        Ok(Self::Name(host.parse()?, port))
    }
}

/// What the book knows about one peer.
///
/// It holds at most `MAX_ADDRS` distinct addresses, newest first, so a dial
/// tries a bounded list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEntry {
    node: NodeId,
    addrs: Vec<HostPort>,
    last_seen_ms: Option<u64>,
}

impl PeerEntry {
    /// The peer's node id.
    #[must_use]
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// Where it was last reachable, newest first; empty when no address is known, and silent on which side dials (ADR 0013).
    #[must_use]
    pub fn addrs(&self) -> &[HostPort] {
        &self.addrs
    }

    /// When a connection with it last completed, in milliseconds since the Unix epoch.
    #[must_use]
    pub fn last_seen_ms(&self) -> Option<u64> {
        self.last_seen_ms
    }
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
    type Err = SpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut fields = s.split_ascii_whitespace();
        let node: NodeId = fields
            .next()
            .ok_or(SpecError::Fields("the line is empty"))?
            .parse()?;
        let addrs = fields
            .next()
            .ok_or(SpecError::Fields("the addresses are missing"))?;
        let addrs = if addrs == "-" {
            Vec::new()
        } else {
            addrs
                .split(',')
                .map(str::parse)
                .collect::<Result<Vec<HostPort>, _>>()?
        };
        if addrs.len() > MAX_ADDRS {
            return Err(SpecError::TooManyAddrs(addrs.len()));
        }
        for (i, addr) in addrs.iter().enumerate() {
            if addrs[..i].contains(addr) {
                return Err(SpecError::DuplicateAddr(addr.clone()));
            }
        }
        let last_seen_ms = match fields
            .next()
            .ok_or(SpecError::Fields("the last seen time is missing"))?
        {
            "-" => None,
            ms => Some(
                ms.parse()
                    .map_err(|_| SpecError::Fields("the last seen time is not a number"))?,
            ),
        };
        if fields.next().is_some() {
            return Err(SpecError::Fields("a field follows the last seen time"));
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
    type Err = SpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (node, addr) = s.split_once('@').ok_or(SpecError::Spec)?;
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
    /// Parses the text form: one `PeerEntry` per line, each node on one line only.
    pub fn parse(text: &str) -> Result<Self, PeerBookError> {
        let mut peers = BTreeMap::new();
        let mut first_line = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let line_no = index + 1;
            if line.trim().is_empty() {
                continue;
            }
            let entry: PeerEntry = line.parse().map_err(|source| PeerBookError::Line {
                line: line_no,
                source,
            })?;
            if let Some(first) = first_line.insert(entry.node, line_no) {
                return Err(PeerBookError::DuplicateNode {
                    line: line_no,
                    first,
                    node: entry.node,
                });
            }
            peers.insert(entry.node, entry);
        }
        Ok(Self { peers })
    }

    /// Reads the book at `path`; a missing file is an empty book.
    pub fn load(path: &Path) -> Result<Self, PeerBookError> {
        let bytes = read_bytes(path).map_err(|source| PeerBookError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse_bytes(path, &bytes)
    }

    /// Parses the bytes of the book file at `path`, which must be UTF-8.
    fn parse_bytes(path: &Path, bytes: &[u8]) -> Result<Self, PeerBookError> {
        let text = std::str::from_utf8(bytes).map_err(|err| PeerBookError::Io {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidData, err),
        })?;
        Self::parse(text)
    }

    /// Writes the book to `path` whole: a temporary file only this call names, synced, renamed over, directory synced.
    pub fn save(&self, path: &Path) -> Result<(), PeerBookError> {
        let io = |source| PeerBookError::Io {
            path: path.to_path_buf(),
            source,
        };
        let tmp = temp_sibling(path);
        let written = (|| {
            let mut file = File::options().write(true).create_new(true).open(&tmp)?;
            file.write_all(self.to_string().as_bytes())?;
            file.sync_all()?;
            checkpoint::reached("peers.after_tmp");
            fs::rename(&tmp, path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written.map_err(io)?;
        checkpoint::reached("peers.after_rename");
        sync_parent(path).map_err(io)
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

/// Reads the bytes of the book at `path`; a missing file is an empty one.
fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    match fs::read(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        other => other,
    }
}

/// An exclusive advisory lock on the sibling `<book>.lock` file, held until dropped.
///
/// The lock lives on a sibling because a save renames a new file over the
/// book, so a lock on the book file itself would not cover the next writer.
/// It is an `flock`, which two descriptors exclude even within one process.
struct LockFile(File);

impl LockFile {
    fn exclusive(path: &Path) -> io::Result<Self> {
        let mut name = path
            .file_name()
            .map_or_else(Default::default, std::ffi::OsStr::to_os_string);
        name.push(".lock");
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_file_name(name))?;
        file.lock()?;
        Ok(Self(file))
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        // Closing the file releases the lock too; unlocking first keeps the
        // release explicit and independent of descriptor sharing.
        let _ = self.0.unlock();
    }
}

/// The book as last parsed and the file bytes it was parsed from.
#[derive(Debug)]
struct Cached {
    book: Arc<PeerBook>,
    /// `None` when the file could not be read, so the next read always parses.
    bytes: Option<Vec<u8>>,
}

/// The peer book behind a file that other processes write too.
///
/// A change reloads the file under an exclusive lock, applies, saves and
/// swaps the cache. `refresh` re-reads the file without a lock, since a
/// save renames a whole file into place, and parses it only when its bytes
/// differ from the cached ones. `is_paired` reads only the cache, and the
/// cache lock is held only to swap or clone, never across file IO, so a
/// handshake's trust check never waits on a save.
#[derive(Debug)]
pub struct Peers {
    path: PathBuf,
    cache: Mutex<Cached>,
}

impl Peers {
    /// Opens the book at `path`; a missing file is an empty book.
    pub fn open(path: impl Into<PathBuf>) -> Result<Arc<Self>, PeerBookError> {
        let path = path.into();
        let bytes = read_bytes(&path).map_err(|source| PeerBookError::Io {
            path: path.clone(),
            source,
        })?;
        let book = PeerBook::parse_bytes(&path, &bytes)?;
        Ok(Arc::new(Self {
            path,
            cache: Mutex::new(Cached {
                book: Arc::new(book),
                bytes: Some(bytes),
            }),
        }))
    }

    /// Returns a copy of the book as the file holds it now.
    #[must_use]
    pub fn snapshot(&self) -> PeerBook {
        self.reload();
        PeerBook::clone(&self.book())
    }

    /// Reloads the file under a lock, applies `change` to that fresh book and saves it.
    ///
    /// The change is kept only if the save succeeds. Another process's
    /// pairing made since the last read is in the book the change sees.
    pub fn update(&self, change: impl FnOnce(&mut PeerBook)) -> Result<(), PeerBookError> {
        let io = |source| PeerBookError::Io {
            path: self.path.clone(),
            source,
        };
        let _lock = LockFile::exclusive(&self.path).map_err(io)?;
        let mut bytes = read_bytes(&self.path).map_err(io)?;
        let current = PeerBook::parse_bytes(&self.path, &bytes)?;
        let mut next = current.clone();
        change(&mut next);
        if next != current {
            next.save(&self.path)?;
            bytes = next.to_string().into_bytes();
        }
        *lock(&self.cache) = Cached {
            book: Arc::new(next),
            bytes: Some(bytes),
        };
        Ok(())
    }

    /// Reloads the book if the file's bytes changed since it was last parsed; it blocks on file IO.
    ///
    /// Trust fails closed: a file that cannot be read or parsed pairs no
    /// one until it can, so a hand edit that revokes a node and slips in a
    /// typo never leaves that node trusted. A file that is gone empties the
    /// book too. A file caught halfway through an edit made in place pairs
    /// at most the nodes its first lines name, and the next refresh reads
    /// the finished edit.
    fn reload(&self) {
        let bytes = match read_bytes(&self.path) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!(path = %self.path.display(), error = %err, "peer book not readable; trusting no peer until it is");
                self.distrust(None);
                return;
            }
        };
        if lock(&self.cache).bytes.as_deref() == Some(bytes.as_slice()) {
            return;
        }
        match PeerBook::parse_bytes(&self.path, &bytes) {
            Ok(book) => {
                *lock(&self.cache) = Cached {
                    book: Arc::new(book),
                    bytes: Some(bytes),
                };
            }
            Err(err) => {
                tracing::error!(path = %self.path.display(), error = %err, "peer book does not parse; trusting no peer until it does");
                self.distrust(Some(bytes));
            }
        }
    }

    /// Pairs no one until the file's bytes differ from `bytes`.
    fn distrust(&self, bytes: Option<Vec<u8>>) {
        *lock(&self.cache) = Cached {
            book: Arc::new(PeerBook::default()),
            bytes,
        };
    }

    fn book(&self) -> Arc<PeerBook> {
        Arc::clone(&lock(&self.cache).book)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The guarded values are whole at every lock boundary.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Trust for Peers {
    fn is_paired(&self, node: NodeId) -> bool {
        self.book().is_paired(node)
    }

    fn refresh(&self) {
        self.reload();
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
        assert!(
            matches!(&spec.addr, HostPort::Name(name, 7777) if name.as_str() == "air.tail1234.ts.net")
        );
        assert_eq!(spec.addr.socket_addr(), None);
        let spec: PeerSpec = format!("{id}@[fd7a::1]:7777").parse().unwrap();
        assert_eq!(spec.addr.to_string(), "[fd7a::1]:7777");
        let spec: PeerSpec = format!("{id}@[fe80::1%5]:7777").parse().unwrap();
        assert_eq!(
            spec.addr.to_string(),
            "[fe80::1%5]:7777",
            "the scope id is kept"
        );
        assert!(matches!(
            format!("{id}@bad host:7777").parse::<PeerSpec>(),
            Err(SpecError::DnsName(_))
        ));
        assert!(matches!(
            format!("{id}@no-port").parse::<PeerSpec>(),
            Err(SpecError::HostPort(_))
        ));
        assert!(matches!("x".parse::<PeerSpec>(), Err(SpecError::Spec)));
        assert!(matches!(
            "fab1xyz@1.2.3.4:5".parse::<PeerSpec>(),
            Err(SpecError::NodeId(_))
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
        assert_eq!(entry.addrs()[0].to_string(), "10.0.0.9:7777");
        assert_eq!(entry.addrs().len(), 2);
        assert_eq!(entry.last_seen_ms(), Some(1_760_000_000_000));
        assert!(reloaded.get(node(3)).unwrap().addrs().is_empty());

        let text = format!("{reloaded}\nnot a peer\n");
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::Line {
                line: 4,
                source: SpecError::NodeId(_)
            })
        ));
        let text = format!("{} - - extra\n", node(5));
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::Line {
                line: 1,
                source: SpecError::Fields(_)
            })
        ));
    }

    /// A hand-edited book holds the same bounds as one the code wrote: at
    /// most `MAX_ADDRS` distinct addresses per peer, and one line per peer.
    #[test]
    fn a_book_line_past_its_bounds_is_refused() {
        let addrs: Vec<String> = (0..=MAX_ADDRS).map(|i| format!("10.0.0.{i}:1")).collect();
        let text = format!("{} {} -", node(1), addrs.join(","));
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::Line {
                line: 1,
                source: SpecError::TooManyAddrs(9)
            })
        ));
        let text = format!("{} 10.0.0.1:1,10.0.0.1:1 -", node(1));
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::Line {
                source: SpecError::DuplicateAddr(_),
                ..
            })
        ));
        let text = format!("{0} - -\n{1} - -\n{0} 10.0.0.1:1 -\n", node(1), node(2));
        assert!(matches!(
            PeerBook::parse(&text),
            Err(PeerBookError::DuplicateNode { line: 3, first: 1, node: n }) if n == node(1)
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

    /// The CLI pairs while the daemon serves: two handles on one file.
    #[test]
    fn two_handles_on_one_file_keep_each_others_pairings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers");
        let daemon = Peers::open(&path).unwrap();
        let cli = Peers::open(&path).unwrap();
        daemon.update(|b| b.pair(node(1), None)).unwrap();
        cli.update(|b| b.pair(node(2), None)).unwrap();
        daemon
            .update(|b| b.seen(node(1), "10.0.0.1:7777".parse().unwrap(), 5))
            .unwrap();

        let on_disk = PeerBook::load(&path).unwrap();
        assert!(on_disk.is_paired(node(1)) && on_disk.is_paired(node(2)));
        daemon.refresh();
        assert!(
            daemon.is_paired(node(2)),
            "the daemon honours the CLI's pairing"
        );
        assert_eq!(cli.snapshot().get(node(1)).unwrap().last_seen_ms, Some(5));

        fs::remove_file(&path).unwrap();
        daemon.refresh();
        assert!(!daemon.is_paired(node(1)), "a removed book pairs no one");
    }

    /// The CLI unpairs a node while the daemon, which read the book before,
    /// records a connection: the revocation sticks on disk and in the daemon.
    #[test]
    fn an_unpair_by_hand_survives_the_daemons_next_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers");
        let daemon = Peers::open(&path).unwrap();
        daemon.update(|b| b.pair(node(1), None)).unwrap();
        let cli = Peers::open(&path).unwrap();
        cli.update(|b| {
            b.unpair(node(1));
        })
        .unwrap();
        daemon
            .update(|b| b.seen(node(1), "10.0.0.1:7777".parse().unwrap(), 5))
            .unwrap();
        assert!(!PeerBook::load(&path).unwrap().is_paired(node(1)));
        assert!(!daemon.is_paired(node(1)));
    }

    /// A hand edit that revokes a node and breaks a line must not leave the
    /// node trusted: a book that does not parse pairs no one until it does.
    #[test]
    fn a_book_that_does_not_parse_pairs_no_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers");
        let daemon = Peers::open(&path).unwrap();
        daemon.update(|b| b.pair(node(1), None)).unwrap();
        assert!(daemon.is_paired(node(1)));

        fs::write(&path, format!("{} - -\nnot a peer line\n", node(2))).unwrap();
        daemon.refresh();
        assert!(!daemon.is_paired(node(1)) && !daemon.is_paired(node(2)));

        fs::write(&path, format!("{} - -\n", node(2))).unwrap();
        daemon.refresh();
        assert!(daemon.is_paired(node(2)), "a fixed book is read again");
    }

    /// An edit made in place can keep the file's length and modified time,
    /// as one node id replaced by another within one clock tick does. The
    /// next refresh still reads the new book, so a revoked node is not
    /// kept trusted.
    #[test]
    fn an_edit_in_place_that_keeps_length_and_time_is_read_at_the_next_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers");
        let daemon = Peers::open(&path).unwrap();
        daemon.update(|b| b.pair(node(1), None)).unwrap();
        let before = fs::metadata(&path).unwrap();

        let mut file = File::options().write(true).open(&path).unwrap();
        file.write_all(format!("{} - -\n", node(2)).as_bytes())
            .unwrap();
        file.set_modified(before.modified().unwrap()).unwrap();
        drop(file);
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());

        daemon.refresh();
        assert!(!daemon.is_paired(node(1)), "the revoked node stays trusted");
        assert!(daemon.is_paired(node(2)));
    }

    proptest! {
        #[test]
        fn entries_round_trip_as_text(
            bytes in any::<[u8; 32]>(),
            addrs in proptest::collection::btree_set(any::<SocketAddr>(), 0..3),
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
