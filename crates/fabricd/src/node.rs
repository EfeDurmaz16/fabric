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
use std::fs::{self, File};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fabric_client::api::{Answer, ClientEnv, Event, Request};
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

use crate::error::NodeError;
use crate::harnesses::{HarnessConfig, HarnessEnv};
use crate::job::Job;
use crate::paths::StateDir;
use crate::tool_env::ToolEnv;

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
    /// The environment the daemon was started with, for work no client
    /// asked for and for a request that sends none, until a local client
    /// sends one.
    pub env: ToolEnv,
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
    client_env: Mutex<Option<ClientEnv>>,
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
        fs::create_dir_all(root).map_err(|e| NodeError::io(root, e))?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|e| NodeError::io(root, e))?;
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
        let client_env = crate::local::read(&cfg.state.client_env()).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "the environment a client last sent does not read; the node uses its own until a client sends one");
            None
        });
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
            client_env: Mutex::new(client_env),
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
                        tokio::spawn(Box::pin(crate::control::serve(node, stream)));
                    }
                    Err(e) => tracing::warn!(error = %e, "control accept failed"),
                },
            }
        }
    }

    /// Runs one client request and answers with its reply or its refusal;
    /// the events go to `out` as the command makes them.
    pub(crate) async fn handle(self: &Arc<Self>, request: Request, mut out: Out) -> Answer {
        match Box::pin(self.dispatch(request, &mut out)).await {
            Ok(reply) => Answer::Done { reply },
            Err(e) => Answer::Refused {
                refusal: e.to_refusal(),
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

    /// The node's environment, for work no client asked for, such as a
    /// peer's pull or a peer's preflight, and for a request that sends
    /// none: the one the last local client sent, kept across restarts, or
    /// the daemon's own until a client sends one (ADR 0018, The client's
    /// environment). The daemon's own is rarely the user's shell: measured
    /// with it, a peer and this node's CLI compared the shell's
    /// `CARGO_HOME` with the daemon's and rebuilt what they could carry
    /// (review finding on PR #13).
    pub(crate) fn node_env(&self) -> ToolEnv {
        lock(&self.client_env)
            .as_ref()
            .map_or_else(|| self.cfg.env.clone(), ToolEnv::from)
    }

    /// Keeps `env`, which a local client sent, as the node's environment,
    /// on disk too when it changed. A failure to write it is logged: the
    /// request still runs in its own environment.
    pub(crate) fn remember_env(&self, env: &ClientEnv) {
        let mut kept = lock(&self.client_env);
        if kept.as_ref() == Some(env) {
            return;
        }
        if let Err(e) = crate::local::write(&self.cfg.state.client_env(), env) {
            tracing::warn!(error = %e, "could not keep the environment a client sent");
        }
        *kept = Some(env.clone());
    }

    /// The adapters' environment for a request made in `env`.
    pub(crate) fn harness_env(&self, env: &ToolEnv) -> Result<HarnessEnv, NodeError> {
        Ok(HarnessEnv::new(
            &self.cfg.home,
            env.path_var().to_owned(),
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

/// What a command reports as it runs, one typed event per step or choice
/// (ADR 0018, Progress as typed events).
///
/// This is the one place a node makes events. A command run for a client
/// streams each one to that client the moment it is made; work no client
/// asked for, such as recovery or a peer's request, keeps them for its log.
/// A live stream of every job's events (C2) is this sink made a broadcast.
#[derive(Debug, Default)]
pub(crate) struct Out {
    live: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
    /// The events kept when no client streams them.
    pub(crate) events: Vec<Event>,
}

impl Out {
    /// A sink that sends each event to a client's connection as it is made.
    pub(crate) fn streaming(live: tokio::sync::mpsc::UnboundedSender<Event>) -> Self {
        Self {
            live: Some(live),
            events: Vec::new(),
        }
    }

    /// Reports an event.
    pub(crate) fn emit(&mut self, event: Event) {
        match &self.live {
            // A client that left gets no more events; the command runs on.
            Some(live) => {
                let _ = live.send(event);
            }
            None => self.events.push(event),
        }
    }

    /// The kept events as the lines the CLI prints for them, for the
    /// node's log: an operator reading the log of `fabric serve` reads the
    /// same sentences as before the client API, not the events' fields.
    pub(crate) fn lines(&self) -> impl Iterator<Item = String> + '_ {
        self.events.iter().flat_map(fabric_client::sentences::event)
    }
}
