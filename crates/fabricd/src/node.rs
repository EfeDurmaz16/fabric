//! The node process: one per state directory, holding the store, the job
//! ledgers, the network endpoint and the control socket.
//!
//! Invariant: exactly one process serves a state directory. It takes an
//! exclusive lock on `daemon.lock` before it opens anything else and keeps
//! it for its whole life, so the store's single writer, the ledger files'
//! single appender and the supervisor of every harness this node launched
//! are the same process (ADR 0003, Who supervises the harness). A session
//! with a peer exists only after the pinned TLS handshake and the hello
//! (ADR 0013), and every session both calls and serves, so either node can
//! ask the other for chunks over the connection one of them dialed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs::{self, File};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fabric_core::id::{JobId, NodeId};
use fabric_net::endpoint::{DialOrder, Endpoint};
use fabric_net::identity::NodeIdentity;
use fabric_net::peers::Peers;
use fabric_net::rpc::{RpcError, Service, Session};
use fabric_net::tls::Trust;
use fabric_net::transport::{Connection, TransportError};
use fabric_net::wire::VersionText;
use fabric_store::objects::ChunkParams;
use fabric_store::store::{Store, StoreError};
use tokio::net::UnixListener;

use crate::control::{self, Request, Response};
use crate::error::NodeError;
use crate::harnesses::{HarnessConfig, HarnessEnv};
use crate::job::Job;
use crate::paths::StateDir;

/// The longest Unix socket path every supported system binds: macOS allows 103 bytes.
pub const MAX_SOCKET_PATH: usize = 104;

/// How long the hello may take on a new connection.
pub const HELLO_DEADLINE: Duration = Duration::from_secs(10);

/// What a node is started with.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The state directory.
    pub state: StateDir,
    /// The user's home, resolved to its physical path.
    pub home: PathBuf,
    /// Where both transports listen.
    pub listen: SocketAddr,
    /// Which wires a dial tries.
    pub dial: DialOrder,
    /// The `PATH` the daemon was started with, for work no CLI asked for.
    pub path_var: OsString,
    /// `CARGO_HOME`, when set.
    pub cargo_home: Option<PathBuf>,
}

/// A running node.
pub struct Node {
    pub(crate) cfg: NodeConfig,
    pub(crate) id: NodeId,
    pub(crate) version: VersionText,
    pub(crate) peers: Arc<Peers>,
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) jobs: tokio::sync::Mutex<BTreeMap<JobId, Arc<tokio::sync::Mutex<Job>>>>,
    pub(crate) broken: Mutex<BTreeMap<JobId, String>>,
    endpoint: Endpoint,
    sessions: Mutex<HashMap<NodeId, Arc<Session>>>,
    pub(crate) harness_config: HarnessConfig,
    control: Mutex<Option<UnixListener>>,
    pub(crate) recovered: tokio::sync::watch::Sender<bool>,
    pub(crate) catching_up: Mutex<BTreeSet<NodeId>>,
    _lock: File,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("state", &self.cfg.state)
            .finish_non_exhaustive()
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Node {
    /// Takes the state directory, opens everything in it, binds the network and the control socket.
    ///
    /// Call it inside a tokio runtime: the transports and the socket register with it.
    pub fn start(cfg: NodeConfig) -> Result<Arc<Self>, NodeError> {
        let root = cfg.state.root();
        prepare_state_dir(root)?;
        let lock_path = cfg.state.lock();
        let lock_file = File::create(&lock_path).map_err(|e| NodeError::io(&lock_path, e))?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(NodeError::AlreadyRunning(root.to_owned()));
            }
            Err(fs::TryLockError::Error(e)) => return Err(NodeError::io(&lock_path, e)),
        }

        crate::faults::arm()?;
        let identity = NodeIdentity::load_or_generate(&cfg.state.node_key())?;
        let peers = Peers::open(cfg.state.peers())?;
        let store = open_store(&cfg.state.store())?;
        let harness_config = HarnessConfig::load(&cfg.state.harness_config())?;
        let (jobs, broken) = load_jobs(&cfg.state)?;
        let trust: Arc<dyn Trust> = peers.clone();
        let endpoint = Endpoint::bind(&identity, &trust, cfg.listen)?.with_dial_order(cfg.dial);

        let socket = cfg.state.socket();
        if socket.as_os_str().len() >= MAX_SOCKET_PATH {
            return Err(NodeError::SocketPathTooLong(socket));
        }
        // The lock proves no other daemon serves this directory, so a socket
        // file left here belongs to a daemon that died.
        match fs::remove_file(&socket) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(NodeError::io(&socket, e)),
        }
        let control = UnixListener::bind(&socket).map_err(|e| NodeError::io(&socket, e))?;

        let version = VersionText::new(env!("CARGO_PKG_VERSION"))?;
        let node = Arc::new(Self {
            id: identity.node_id(),
            cfg,
            version,
            peers,
            store: Arc::new(Mutex::new(store)),
            jobs: tokio::sync::Mutex::new(jobs),
            broken: Mutex::new(broken),
            endpoint,
            sessions: Mutex::new(HashMap::new()),
            harness_config,
            control: Mutex::new(Some(control)),
            recovered: tokio::sync::watch::Sender::new(false),
            catching_up: Mutex::new(BTreeSet::new()),
            _lock: lock_file,
        });
        tracing::info!(node = %node.id, addr = ?node.local_addr().ok(), "node started");
        Ok(node)
    }

    /// This node's id.
    #[must_use]
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The address both transports listen on.
    pub fn local_addr(&self) -> Result<SocketAddr, NodeError> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Serves peers and the control socket until the endpoint closes.
    pub async fn run(self: Arc<Self>) -> Result<(), NodeError> {
        let control = lock(&self.control)
            .take()
            .ok_or_else(|| NodeError::Task("the node is already running".to_owned()))?;
        tokio::spawn(Arc::clone(&self).recover());
        loop {
            tokio::select! {
                accepted = self.endpoint.accept() => match accepted {
                    Ok(connection) => {
                        let node = Arc::clone(&self);
                        tokio::spawn(async move { node.accept_session(connection).await });
                    }
                    Err(TransportError::EndpointClosed) => return Err(TransportError::EndpointClosed.into()),
                    Err(e) => tracing::warn!(error = %e, "accept failed"),
                },
                accepted = control.accept() => match accepted {
                    Ok((stream, _)) => {
                        let node = Arc::clone(&self);
                        tokio::spawn(Box::pin(node.serve_control(stream)));
                    }
                    Err(e) => tracing::warn!(error = %e, "control accept failed"),
                },
            }
        }
    }

    async fn serve_control(self: Arc<Self>, mut stream: tokio::net::UnixStream) {
        let me = rustix::process::geteuid().as_raw();
        match stream.peer_cred() {
            Ok(peer) if peer.uid() == me => {}
            Ok(peer) => {
                tracing::warn!(
                    uid = peer.uid(),
                    "refused a control connection from another user"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "refused a control connection whose user is unknown");
                return;
            }
        }
        let request = match control::read_request(&mut stream).await {
            Ok(request) => request,
            Err(e) => {
                tracing::warn!(error = %e, "unreadable control request");
                return;
            }
        };
        let response = Box::pin(self.handle(request)).await;
        if let Err(e) = control::write_response(&mut stream, &response).await {
            tracing::warn!(error = %e, "control response not delivered");
        }
    }

    async fn handle(self: &Arc<Self>, request: Request) -> Response {
        let mut out = Out::default();
        let result = Box::pin(self.dispatch(request, &mut out)).await;
        match result {
            Ok(None) => Response::Done { lines: out.lines },
            Ok(Some(plan)) => Response::Run {
                lines: out.lines,
                plan,
            },
            Err(e) => Response::Failed {
                lines: out.lines,
                error: e.to_string(),
                refusal: e.refusal(),
            },
        }
    }

    async fn accept_session(self: Arc<Self>, connection: Box<dyn Connection>) {
        match Session::accept(
            Arc::from(connection),
            self.id,
            self.version.clone(),
            HELLO_DEADLINE,
        )
        .await
        {
            Ok(session) => {
                self.register(Arc::new(session));
            }
            Err(e) => tracing::info!(error = %e, "an incoming session failed its hello"),
        }
    }

    /// Keeps `session` for calls to its peer, serves the peer's requests on
    /// it until it ends, and catches up with the peer, which may have
    /// written records while the two were apart.
    fn register(self: &Arc<Self>, session: Arc<Session>) {
        let peer = session.peer();
        lock(&self.sessions).insert(peer, Arc::clone(&session));
        self.spawn_catch_up(peer);
        let node = Arc::clone(self);
        tokio::spawn(async move {
            let service: Arc<dyn Service> = node.clone();
            let trust: Arc<dyn Trust> = node.peers.clone();
            let ended = session.serve(service, trust).await;
            tracing::info!(%peer, reason = %ended, "session ended");
            let mut sessions = lock(&node.sessions);
            if sessions
                .get(&peer)
                .is_some_and(|kept| Arc::ptr_eq(kept, &session))
            {
                sessions.remove(&peer);
            }
        });
    }

    /// A live session with `peer`: the one already open, or a new dial.
    pub(crate) async fn session(self: &Arc<Self>, peer: NodeId) -> Result<Arc<Session>, NodeError> {
        if let Some(session) = lock(&self.sessions).get(&peer).cloned() {
            return Ok(session);
        }
        self.dial(peer).await
    }

    /// The open session with `peer`, if any, without dialing.
    pub(crate) fn open_session(&self, peer: NodeId) -> Option<Arc<Session>> {
        lock(&self.sessions).get(&peer).cloned()
    }

    /// Drops the kept session with `peer` after a call on it failed at the transport.
    pub(crate) fn forget(&self, peer: NodeId, session: &Arc<Session>) {
        let mut sessions = lock(&self.sessions);
        if sessions
            .get(&peer)
            .is_some_and(|kept| Arc::ptr_eq(kept, session))
        {
            sessions.remove(&peer);
        }
        session.close();
    }

    /// Dials `peer` at the addresses the peer book holds for it.
    pub(crate) async fn dial(self: &Arc<Self>, peer: NodeId) -> Result<Arc<Session>, NodeError> {
        let book = self.peers.snapshot();
        let addrs = book
            .get(peer)
            .map(|entry| entry.addrs().to_vec())
            .filter(|addrs| !addrs.is_empty())
            .ok_or(NodeError::UnknownPeer(peer))?;
        let unreachable = |reason: String| NodeError::Unreachable { peer, reason };
        let dialed = self
            .endpoint
            .dial_any(peer, &addrs)
            .await
            .map_err(|e| unreachable(e.to_string()))?;
        if let Some(quic) = dialed.quic_failure() {
            tracing::info!(%peer, reason = %quic, "QUIC failed; the session runs on TCP with TLS");
        }
        let session = Session::connect(
            Arc::from(dialed.into_connection()),
            self.id,
            self.version.clone(),
            HELLO_DEADLINE,
        )
        .await
        .map_err(|e| unreachable(e.to_string()))?;
        let session = Arc::new(session);
        let peers = Arc::clone(&self.peers);
        let now = crate::clock::now_ms();
        // Only an address this node dialed is recorded: the address an
        // accepted connection came from may sit behind a NAT (ADR 0013).
        let _ =
            tokio::task::spawn_blocking(move || peers.update(|book| book.seen(peer, now))).await;
        self.register(Arc::clone(&session));
        Ok(session)
    }

    /// The job's lock, if this node knows the job.
    pub(crate) async fn job(&self, id: JobId) -> Result<Arc<tokio::sync::Mutex<Job>>, NodeError> {
        self.jobs
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or(NodeError::UnknownJob(id))
    }

    /// The adapters' environment for a request made with `path_var`.
    pub(crate) fn harness_env(&self, path_var: &OsString) -> Result<HarnessEnv, NodeError> {
        Ok(HarnessEnv::new(
            &self.cfg.home,
            path_var.clone(),
            self.harness_config.clone(),
        )?)
    }

    /// Waits until the start-up recovery has run, so no job command acts on
    /// a ledger that has not yet learned what the peers wrote while this
    /// node was away.
    pub(crate) async fn recovered(&self) {
        let mut done = self.recovered.subscribe();
        let _ = done.wait_for(|done| *done).await;
    }
}

/// Treats a transport failure as a reason to drop the kept session and dial again.
pub(crate) fn is_transport(err: &RpcError) -> bool {
    matches!(err, RpcError::Transport(_) | RpcError::Frame(_))
}

fn open_store(dir: &std::path::Path) -> Result<Store, NodeError> {
    match Store::open(dir) {
        Ok(store) => Ok(store),
        Err(StoreError::NotAStore { .. }) => Ok(Store::create(dir, ChunkParams::DEFAULT)?),
        Err(e) => Err(e.into()),
    }
}

/// The names Fabric writes at the top of a state directory.
const STATE_ENTRIES: [&str; 9] = [
    "daemon.lock",
    "fabric.sock",
    "harness.toml",
    "jobs",
    "locks",
    "node.key",
    "peers",
    "peers.lock",
    "store",
];

/// Makes the state directory private to this user without taking over one
/// it does not own.
///
/// A missing directory is created with mode 0700. An existing one must be
/// this user's; when it lets others in, its mode is narrowed only if it
/// holds nothing but what Fabric writes, so a `FABRIC_HOME` that names a
/// shared directory, such as `/tmp` or a project, is refused, not changed.
fn prepare_state_dir(root: &std::path::Path) -> Result<(), NodeError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let unsafe_dir = |reason: String| NodeError::UnsafeStateDir {
        path: root.to_owned(),
        reason,
    };
    let meta = match fs::metadata(root) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root)
                .map_err(|e| NodeError::io(root, e))?;
            return fs::set_permissions(root, fs::Permissions::from_mode(0o700))
                .map_err(|e| NodeError::io(root, e));
        }
        Err(e) => return Err(NodeError::io(root, e)),
    };
    if !meta.is_dir() {
        return Err(unsafe_dir("is not a directory".to_owned()));
    }
    let me = rustix::process::geteuid().as_raw();
    if meta.uid() != me {
        return Err(unsafe_dir(format!(
            "belongs to user {}, not to you ({me})",
            meta.uid()
        )));
    }
    let others = meta.mode() & 0o077;
    if others == 0 {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|e| NodeError::io(root, e))? {
        let name = entry.map_err(|e| NodeError::io(root, e))?.file_name();
        let name = name.to_string_lossy();
        let ours = STATE_ENTRIES.iter().any(|known| {
            *name == **known
                || name
                    .strip_prefix(known)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .and_then(|rest| rest.strip_suffix("tmp"))
                    .is_some_and(|middle| middle.ends_with('.'))
        });
        if !ours {
            return Err(unsafe_dir(format!(
                "has mode {:o} and holds {name}, which Fabric did not write",
                meta.mode() & 0o777
            )));
        }
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|e| NodeError::io(root, e))
}

type Jobs = BTreeMap<JobId, Arc<tokio::sync::Mutex<Job>>>;

fn load_jobs(state: &StateDir) -> Result<(Jobs, BTreeMap<JobId, String>), NodeError> {
    let dir = state.jobs();
    fs::create_dir_all(&dir).map_err(|e| NodeError::io(&dir, e))?;
    let mut jobs = BTreeMap::new();
    let mut broken = BTreeMap::new();
    for entry in fs::read_dir(&dir).map_err(|e| NodeError::io(&dir, e))? {
        let entry = entry.map_err(|e| NodeError::io(&dir, e))?;
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<JobId>().ok())
        else {
            continue;
        };
        if !state.job(id).ledger().exists() {
            continue;
        }
        match Job::open(id, state.job(id)) {
            Ok(job) => {
                jobs.insert(id, Arc::new(tokio::sync::Mutex::new(job)));
            }
            Err(e) => {
                tracing::error!(job = %id, error = %e, "job left closed");
                broken.insert(id, e.to_string());
            }
        }
    }
    Ok((jobs, broken))
}

/// The lines a command prints, one per step or choice.
#[derive(Debug, Default)]
pub(crate) struct Out {
    pub(crate) lines: Vec<String>,
}

impl Out {
    /// Adds a line.
    pub(crate) fn say(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A `FABRIC_HOME` typed at a shared directory must be refused, not
    /// narrowed to 0700 under everyone who uses it.
    #[test]
    fn a_shared_directory_is_refused_and_left_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("project");
        fs::create_dir(&shared).unwrap();
        fs::write(shared.join("notes.txt"), "mine").unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
        let refused = prepare_state_dir(&shared);
        assert!(
            matches!(refused, Err(NodeError::UnsafeStateDir { .. })),
            "{refused:?}"
        );
        assert_eq!(mode(&shared), 0o755);
    }

    #[test]
    fn a_new_or_fabric_only_directory_is_made_private() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("a/b/state");
        prepare_state_dir(&fresh).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        let made = dir.path().join("made");
        fs::create_dir(&made).unwrap();
        fs::write(made.join("harness.toml"), "").unwrap();
        fs::write(made.join("peers.42.tmp"), "").unwrap();
        fs::set_permissions(&made, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_state_dir(&made).unwrap();
        assert_eq!(mode(&made), 0o700);
    }
}
