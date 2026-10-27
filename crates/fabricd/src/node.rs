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
//! ask the other for chunks over the connection one of them dialed. A peer
//! holds at most one live session per direction: a new connection dialed
//! the same way as a live one closes the older, so the per-session work
//! caps bound what a paired peer can run here, and dials to one peer are
//! made one at a time, so this node never displaces a session it is using.

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
use fabric_net::rpc::{RpcError, ServeLimits, Service, Session};
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
    /// The caps every session with one peer shares, so a peer that connects again gets no fresh ones.
    serve_limits: ServeLimits,
    /// The live session per peer and direction; a newer one closes the older.
    opened: Mutex<HashMap<(NodeId, Side), Arc<Session>>>,
    /// One gate per peer that a dial to it holds.
    dialing: Mutex<HashMap<NodeId, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) harness_config: HarnessConfig,
    control: Mutex<Option<UnixListener>>,
    pub(crate) recovered: tokio::sync::watch::Sender<bool>,
    /// Peers being caught up now, each with whether a session with it
    /// began since, so its catch-up runs once more when it ends.
    pub(crate) catching_up: Mutex<BTreeMap<NodeId, bool>>,
    /// Peers whose session began while start-up recovery ran; recovery
    /// catches up with each of them once it is done.
    pub(crate) waiting_catch_up: Mutex<BTreeSet<NodeId>>,
    /// The environment the last local client sent, which the node uses
    /// for work no client asks for (ADR 0018, The client's environment).
    client_env: Mutex<Option<ClientEnv>>,
    /// The managed authority, the one writer of the managed journal (ADR 0020).
    pub(crate) authority: crate::authority::Authority,
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
        let lock_file = open_lock(&lock_path)?;
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
        // One stat on the start path: the authority's thread opens and
        // recovers an existing journal on its own, and makes none.
        let authority = crate::authority::Authority::start(&cfg.state, identity.node_id())
            .map_err(|e| NodeError::io(crate::authority::managed_dir(&cfg.state), e))?;
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
            serve_limits: ServeLimits::new(),
            opened: Mutex::new(HashMap::new()),
            dialing: Mutex::new(HashMap::new()),
            harness_config,
            control: Mutex::new(Some(control)),
            recovered: tokio::sync::watch::Sender::new(false),
            catching_up: Mutex::new(BTreeMap::new()),
            waiting_catch_up: Mutex::new(BTreeSet::new()),
            client_env: Mutex::new(client_env),
            authority,
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
                self.register(Arc::new(session), Side::Accepted);
            }
            Err(e) => tracing::info!(error = %e, "an incoming session failed its hello"),
        }
    }

    /// Keeps `session` for calls to its peer, serves the peer's requests on
    /// it until it ends, and catches up with the peer, which may have
    /// written records while the two were apart.
    ///
    /// A live session the same side dialed is closed: its serving task
    /// would otherwise run on with semaphores of its own, so a peer that
    /// keeps connecting could run any number of requests here at once. A
    /// session the other side dialed stays, so two nodes that dial each
    /// other at once keep one session each way and neither closes both.
    fn register(self: &Arc<Self>, session: Arc<Session>, side: Side) {
        let peer = session.peer();
        let displaced = lock(&self.opened).insert((peer, side), Arc::clone(&session));
        lock(&self.sessions).insert(peer, Arc::clone(&session));
        if let Some(old) = displaced {
            tracing::info!(%peer, ?side, "a newer session replaced a live one; closing the older");
            old.close();
        }
        self.spawn_catch_up(peer);
        let node = Arc::clone(self);
        tokio::spawn(async move {
            let service: Arc<dyn Service> = node.clone();
            let trust: Arc<dyn Trust> = node.peers.clone();
            let ended = session.serve(service, trust, &node.serve_limits).await;
            tracing::info!(%peer, reason = %ended, "session ended");
            let mut opened = lock(&node.opened);
            if opened
                .get(&(peer, side))
                .is_some_and(|kept| Arc::ptr_eq(kept, &session))
            {
                opened.remove(&(peer, side));
            }
            node.keep_another(&opened, peer, &session);
        });
    }

    /// Drops `session` as the one kept for calls to `peer`, and keeps in its
    /// place the session the other side opened, when one is live.
    ///
    /// Two nodes that dialed each other at once hold one session each way,
    /// and only the one registered last is kept for calls. Without this, the
    /// end of that one would leave the other live but out of reach: the
    /// next call would dial again, and that dial would close the hidden one
    /// a call may still be using. The caller holds `opened`, so a session
    /// that ends meanwhile is never kept.
    fn keep_another(
        &self,
        opened: &HashMap<(NodeId, Side), Arc<Session>>,
        peer: NodeId,
        session: &Arc<Session>,
    ) {
        let mut sessions = lock(&self.sessions);
        if !sessions
            .get(&peer)
            .is_some_and(|kept| Arc::ptr_eq(kept, session))
        {
            return;
        }
        let other = [Side::Dialed, Side::Accepted]
            .into_iter()
            .filter_map(|side| opened.get(&(peer, side)))
            .find(|other| !Arc::ptr_eq(other, session));
        match other {
            Some(other) => {
                sessions.insert(peer, Arc::clone(other));
            }
            None => {
                sessions.remove(&peer);
            }
        }
    }

    /// A live session with `peer`: the one already open, or a new dial.
    ///
    /// Dials to one peer wait for each other, and each looks for a session
    /// again once it may go: a second dial would replace the first one's
    /// session, and with it close a call another task is making on it.
    pub(crate) async fn session(self: &Arc<Self>, peer: NodeId) -> Result<Arc<Session>, NodeError> {
        if let Some(session) = self.open_session(peer) {
            return Ok(session);
        }
        let gate = Arc::clone(lock(&self.dialing).entry(peer).or_default());
        let _dialing = gate.lock().await;
        if let Some(session) = self.open_session(peer) {
            return Ok(session);
        }
        self.dial(peer).await
    }

    /// The open session with `peer`, if any, without dialing.
    pub(crate) fn open_session(&self, peer: NodeId) -> Option<Arc<Session>> {
        lock(&self.sessions).get(&peer).cloned()
    }

    /// Drops the kept session with `peer` after a call on it failed at the
    /// transport, and keeps the session the other side opened, if any.
    pub(crate) fn forget(&self, peer: NodeId, session: &Arc<Session>) {
        let opened = lock(&self.opened);
        self.keep_another(&opened, peer, session);
        drop(opened);
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
        self.register(Arc::clone(&session), Side::Dialed);
        Ok(session)
    }

    /// The job's lock, if this node knows the job.
    ///
    /// A job only the managed authority holds is refused as a managed job,
    /// so a standalone command never reads it as unknown and never acts on
    /// it (ADR 0020); the authority is asked only on that path.
    pub(crate) async fn job(&self, id: JobId) -> Result<Arc<tokio::sync::Mutex<Job>>, NodeError> {
        let found = self.jobs.lock().await.get(&id).cloned();
        match found {
            Some(job) => Ok(job),
            None => Err(self
                .managed_only(id)
                .await
                .unwrap_or(NodeError::UnknownJob(id))),
        }
    }

    /// The node's environment, for work no client asked for, such as a
    /// peer's pull or a peer's preflight, and for a request that sends
    /// none: the one the last local client sent, kept across restarts, or
    /// the daemon's own until a client sends one (ADR 0018, The client's
    /// environment). The daemon's own is rarely the user's shell: measured
    /// with it, a peer and this node's CLI compared the shell's
    /// `CARGO_HOME` with the daemon's and rebuilt what they could carry
    /// (review finding on PR #13).
    ///
    /// A kept environment loses every directory in a job's tree, checked
    /// each time, since a workspace changes: the shell that sent it may
    /// have had a job's virtualenv on its `PATH`, and the node would run
    /// that workspace's programs for whichever peer asked next (review
    /// finding on PR #13).
    pub(crate) fn node_env(&self) -> ToolEnv {
        let kept = lock(&self.client_env).clone();
        kept.map_or_else(
            || self.cfg.env.clone(),
            |env| {
                let trees = [
                    crate::paths::home_jobs(&self.cfg.home),
                    self.cfg.state.jobs(),
                ];
                ToolEnv::from(&crate::tool_env::outside(env, &trees))
            },
        )
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

/// Which side opened a session's connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    /// This node dialed it.
    Dialed,
    /// The peer dialed it.
    Accepted,
}

/// Treats a transport failure as a reason to drop the kept session and dial again.
pub(crate) fn is_transport(err: &RpcError) -> bool {
    matches!(err, RpcError::Transport(_) | RpcError::Frame(_))
}

/// Opens the daemon's lock file at `path`, made with mode 0600 when it is missing.
///
/// A link at the name is refused, never followed, and so is anything there
/// but a regular file. A harness or any other process of this user can
/// replace the file with a link before a restart, and the state
/// directory's check passes a link this user made; opening through it
/// would act on the file it names. The file is opened without truncating
/// and never written, since the lock on it is all it is for, and the open
/// does not wait on a FIFO.
fn open_lock(path: &std::path::Path) -> Result<File, NodeError> {
    use rustix::fs::{FileType, Mode, OFlags};
    let refuse = |what: &str| NodeError::UnsafeStateDir {
        path: path.to_owned(),
        reason: format!(
            "is {what}, not the lock file Fabric writes; remove it once you have checked what it is"
        ),
    };
    let flags =
        OFlags::RDONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = match rustix::fs::open(path, flags, Mode::RUSR | Mode::WUSR) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::LOOP) => return Err(refuse("a symlink")),
        Err(e) => return Err(NodeError::io(path, e.into())),
    };
    let stat = rustix::fs::fstat(&fd).map_err(|e| NodeError::io(path, e.into()))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(refuse("not a regular file"));
    }
    Ok(File::from(fd))
}

fn open_store(dir: &std::path::Path) -> Result<Store, NodeError> {
    match Store::open(dir) {
        Ok(store) => Ok(store),
        Err(StoreError::NotAStore { .. }) => Ok(Store::create(dir, ChunkParams::DEFAULT)?),
        Err(e) => Err(e.into()),
    }
}

/// The names Fabric writes at the top of a state directory. `harness` is
/// where the adapters keep departure records and the Claude config lock
/// under `~/.fabric`, which is the state directory unless `FABRIC_HOME`
/// names another.
const STATE_ENTRIES: [&str; 12] = [
    "client-env",
    "daemon.lock",
    "fabric.sock",
    "harness",
    "harness.toml",
    "jobs",
    "locks",
    "managed",
    "node.key",
    "peers",
    "peers.lock",
    "store",
];

/// Makes the state directory private to this user without taking over one
/// it does not own.
///
/// A missing directory is created with mode 0700. An existing one must be
/// this user's and must be the directory itself, not a link to one: whoever
/// owns a link can point it elsewhere while the node runs, and the CLI
/// refuses a linked state directory anyway. Every entry in it must be this
/// user's too, whatever its mode: an entry another user made while others
/// could write there, such as a `peers` book that pairs their node, stays
/// theirs after the directory is narrowed, so a later start must not trust
/// it either. When the directory lets others in, it is narrowed only if it
/// holds nothing but what Fabric writes, so a `FABRIC_HOME` that names a
/// shared directory, such as `/tmp` or a project, is refused and left as it
/// was.
fn prepare_state_dir(root: &std::path::Path) -> Result<(), NodeError> {
    use std::os::unix::fs::MetadataExt;
    // The components drop a trailing `/` or `/.`, which would make the
    // look below follow a link at the end of the path.
    let root: std::path::PathBuf = root.components().collect();
    let root = root.as_path();
    let meta = match fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match make_state_dir(root) {
            Ok(()) => return Ok(()),
            // Something took the name after the look: judge it as found.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::symlink_metadata(root).map_err(|e| NodeError::io(root, e))?
            }
            Err(e) => return Err(NodeError::io(root, e)),
        },
        Err(e) => return Err(NodeError::io(root, e)),
    };
    let refuse = |reason: String| NodeError::UnsafeStateDir {
        path: root.to_owned(),
        reason,
    };
    let me = rustix::process::geteuid().as_raw();
    if meta.uid() != me {
        return Err(refuse(format!(
            "belongs to user {}, not to you ({me}); choose a dedicated FABRIC_HOME",
            meta.uid()
        )));
    }
    if meta.file_type().is_symlink() {
        return Err(refuse(
            "is a symlink; set FABRIC_HOME to the directory itself".to_owned(),
        ));
    }
    if !meta.is_dir() {
        return Err(refuse(
            "is not a directory; choose a dedicated FABRIC_HOME".to_owned(),
        ));
    }
    adopt_state_dir(root, &meta, me)
}

/// Makes `root` a new directory with mode 0700, and its missing parents
/// with it. `root` itself is made by a plain `mkdir`, which fails on any
/// name already there, a link included, so a link made after the caller
/// looked is never followed.
fn make_state_dir(root: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if let Some(parent) = root.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    fs::DirBuilder::new().mode(0o700).create(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))
}

/// Adopts `root`, a directory user `me` owns, whose metadata is `meta`.
///
/// One that others can write is narrowed before its entries are judged,
/// so none of them can add an entry after the check, and it gets its mode
/// back when it is refused. One that others can at most read is judged
/// first, since only its owner can add to it.
fn adopt_state_dir(root: &std::path::Path, meta: &fs::Metadata, me: u32) -> Result<(), NodeError> {
    use std::os::unix::fs::MetadataExt;
    let mode = meta.mode() & 0o777;
    let others = mode & 0o077;
    if others == 0 {
        return judge_entries(root, me, None);
    }
    let narrow = || {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|e| NodeError::io(root, e))
    };
    if others & 0o022 == 0 {
        judge_entries(root, me, Some(mode))?;
        return narrow();
    }
    narrow()?;
    judge_entries(root, me, Some(mode)).inspect_err(|_| {
        let was = fs::Permissions::from_mode(meta.mode() & 0o7777);
        if let Err(e) = fs::set_permissions(root, was) {
            tracing::warn!(path = %root.display(), error = %e, "the refused state directory keeps mode 700");
        }
    })
}

/// Refuses the first entry of `root` that is not user `me`'s, and, when
/// the directory let others in with the mode in `shared`, the first one
/// Fabric does not write.
fn judge_entries(root: &std::path::Path, me: u32, shared: Option<u32>) -> Result<(), NodeError> {
    use std::os::unix::fs::MetadataExt;
    let refuse = |reason: String| NodeError::UnsafeStateDir {
        path: root.to_owned(),
        reason,
    };
    for entry in fs::read_dir(root).map_err(|e| NodeError::io(root, e))? {
        let entry = entry.map_err(|e| NodeError::io(root, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // The entry itself, not where a link points: a link is its maker's.
        let owner = match entry.metadata() {
            Ok(meta) => meta.uid(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(NodeError::io(entry.path(), e)),
        };
        if owner != me {
            return Err(refuse(format!(
                "holds {name}, which belongs to user {owner}, not to you ({me}); remove it once you have checked what it holds, or choose a dedicated FABRIC_HOME"
            )));
        }
        if let Some(mode) = shared
            && !is_state_entry(&name)
        {
            return Err(refuse(format!(
                "has mode {mode:o} and holds {name}, which Fabric did not write; secure it (chmod 700) or choose a dedicated FABRIC_HOME"
            )));
        }
    }
    Ok(())
}

/// Whether `name` is one of `STATE_ENTRIES`, a temporary file Fabric
/// writes one of them through, or a log. Fabric writes no log itself, but
/// a user who sends the daemon's output into the state directory, from a
/// shell or a launchd job, makes the directory and the log before the
/// first start.
fn is_state_entry(name: &str) -> bool {
    if std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
    {
        return true;
    }
    STATE_ENTRIES.iter().any(|known| {
        name == *known
            || name
                .strip_prefix(known)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.strip_suffix("tmp"))
                .is_some_and(|middle| middle.ends_with('.'))
    })
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

    /// The kept events' lines, collected, for a test to read.
    #[cfg(test)]
    pub(crate) fn said(&self) -> Vec<String> {
        self.lines().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A paired peer can connect again while its first session lives. If
    /// the node kept serving both, each with work caps of its own, a peer
    /// that kept connecting could run any number of requests at once.
    /// The node closes the older session once the newer one is up.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_session_the_peer_dials_closes_its_first() {
        let (_dir_a, a) = crate::ops::tests::node();
        let (_dir_b, b) = crate::ops::tests::node();
        crate::ops::tests::paired(&a, &b).await;
        let addr: fabric_net::peers::HostPort =
            a.local_addr().unwrap().to_string().parse().unwrap();
        // Sessions b opens without keeping them, so only a can close them.
        let open = || async {
            let dialed = b
                .endpoint
                .dial_any(a.id, std::slice::from_ref(&addr))
                .await
                .unwrap();
            Session::connect(
                Arc::from(dialed.into_connection()),
                b.id,
                b.version.clone(),
                HELLO_DEADLINE,
            )
            .await
            .unwrap()
        };
        let deadline = Duration::from_secs(5);
        let first = open().await;
        first.ping(deadline).await.unwrap();
        let second = open().await;
        // a answers on a session only after it registered it.
        second.ping(deadline).await.unwrap();
        let pinged = first.ping(deadline).await;
        assert!(pinged.is_err(), "the first session still serves");
    }

    /// Two nodes that dial each other at once hold a session each way, and
    /// only the one registered last is kept for calls. When that one ends,
    /// the other must be kept in its place: left out of reach, the next
    /// call would dial again and close it under a call still using it.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_session_each_way_survives_the_other_ending() {
        let (_dir_a, a) = crate::ops::tests::node();
        let (_dir_b, b) = crate::ops::tests::node();
        crate::ops::tests::paired(&a, &b).await;
        let dialed = a.session(b.id).await.unwrap();
        let addr: fabric_net::peers::HostPort =
            a.local_addr().unwrap().to_string().parse().unwrap();
        // A session b opens without keeping it, so a holds one each way.
        let accepted = Session::connect(
            Arc::from(
                b.endpoint
                    .dial_any(a.id, std::slice::from_ref(&addr))
                    .await
                    .unwrap()
                    .into_connection(),
            ),
            b.id,
            b.version.clone(),
            HELLO_DEADLINE,
        )
        .await
        .unwrap();
        // a answers on a session only after it registered it.
        accepted.ping(Duration::from_secs(5)).await.unwrap();
        assert!(!Arc::ptr_eq(&a.open_session(b.id).unwrap(), &dialed));
        accepted.close();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while lock(&a.opened).contains_key(&(b.id, Side::Accepted)) {
            assert!(
                std::time::Instant::now() < deadline,
                "the session never ended"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let kept = a.open_session(b.id).expect("no session kept");
        assert!(Arc::ptr_eq(&kept, &dialed));
    }

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

    /// An entry another user made while they could write in the
    /// directory, such as a `peers` book that pairs their node, is still
    /// theirs once the directory is narrowed, so it is refused at every
    /// start, not only at the one that narrowed it, and a directory others
    /// can write keeps its mode. No test can make a file another user
    /// owns, so the node is told it runs as the next user id.
    #[test]
    fn an_entry_another_user_owns_is_refused_whatever_the_mode() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        for was in [0o700, 0o750, 0o775, 0o777] {
            let state = dir.path().join(format!("{was:o}"));
            fs::create_dir(&state).unwrap();
            fs::write(state.join("peers"), "").unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(was)).unwrap();
            let meta = fs::metadata(&state).unwrap();
            let other = meta.uid().wrapping_add(1);
            let refused = adopt_state_dir(&state, &meta, other);
            assert!(
                matches!(&refused, Err(NodeError::UnsafeStateDir { reason, .. }) if reason.contains("holds peers, which belongs to user")),
                "mode {was:o}: {refused:?}"
            );
            assert_eq!(mode(&state), was);
        }
    }

    /// A directory others can write is narrowed first and then judged, so
    /// one that holds only this user's own entries under Fabric's names is
    /// adopted, as a `~/.fabric` made with umask 002 that holds the
    /// `harness.toml` the user wrote before the first start.
    #[test]
    fn a_directory_others_can_write_is_adopted_when_every_entry_is_yours() {
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join("team");
        fs::create_dir(&team).unwrap();
        fs::write(team.join("harness.toml"), "").unwrap();
        fs::create_dir(team.join("harness")).unwrap();
        fs::set_permissions(&team, fs::Permissions::from_mode(0o775)).unwrap();
        prepare_state_dir(&team).unwrap();
        assert_eq!(mode(&team), 0o700);
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        fs::set_permissions(&empty, fs::Permissions::from_mode(0o777)).unwrap();
        prepare_state_dir(&empty).unwrap();
        assert_eq!(mode(&empty), 0o700);
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("notes.txt"), "mine").unwrap();
        fs::set_permissions(&project, fs::Permissions::from_mode(0o777)).unwrap();
        let refused = prepare_state_dir(&project);
        assert!(
            matches!(refused, Err(NodeError::UnsafeStateDir { .. })),
            "{refused:?}"
        );
        assert_eq!(mode(&project), 0o777);
    }

    /// A user who keeps the daemon's output in the state directory, as
    /// `fabric serve >>~/.fabric/daemon.log` or a launchd job whose log
    /// path is there, makes the directory by hand before the first start,
    /// so it holds the log, made by the shell or by launchd, and the
    /// default mode.
    #[test]
    fn a_directory_that_holds_the_users_log_is_made_private() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("fabric");
        fs::create_dir(&home).unwrap();
        fs::write(home.join("harness.toml"), "").unwrap();
        fs::write(home.join("daemon.log"), "").unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_state_dir(&home).unwrap();
        assert_eq!(mode(&home), 0o700);
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
        // The adapters write here under `~/.fabric` whether or not a node
        // ever ran with it as its state directory.
        fs::create_dir_all(made.join("harness/locks")).unwrap();
        fs::write(made.join("harness/locks/claude-config.lock"), "").unwrap();
        fs::set_permissions(&made, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_state_dir(&made).unwrap();
        assert_eq!(mode(&made), 0o700);
    }

    /// A `FABRIC_HOME` that is a link is refused however it is spelled,
    /// and the directory it names keeps its mode: whoever owns the link,
    /// such as another user who planted it under `/tmp`, could point it
    /// elsewhere while the node runs, and the CLI refuses a linked state
    /// directory anyway.
    #[test]
    fn a_fabric_home_that_is_a_link_is_refused_and_its_target_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("harness.toml"), "").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("link")).unwrap();
        for spelled in ["link", "link/", "link/."] {
            let refused = prepare_state_dir(&dir.path().join(spelled));
            assert!(
                matches!(&refused, Err(NodeError::UnsafeStateDir { reason, .. }) if reason.contains("is a symlink")),
                "{spelled}: {refused:?}"
            );
            assert_eq!(mode(&target), 0o755, "{spelled}");
        }
    }

    /// A harness or another process of this user can replace `daemon.lock`
    /// with a link before a restart. Taking the lock must not follow it:
    /// creating the lock file through the link truncated the file it names
    /// (Codex review of #8, issue #23).
    #[test]
    fn a_lock_file_that_is_a_link_is_refused_and_its_target_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "keep").unwrap();
        let lock = dir.path().join("daemon.lock");
        std::os::unix::fs::symlink(&victim, &lock).unwrap();
        let refused = open_lock(&lock);
        assert!(
            matches!(&refused, Err(NodeError::UnsafeStateDir { reason, .. }) if reason.contains("is a symlink")),
            "{refused:?}"
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    }

    /// Anything else at the lock's name, a FIFO here, is not the file
    /// Fabric writes either: it is refused, and the start does not wait on
    /// it for a writer.
    #[test]
    fn a_lock_file_that_is_not_a_regular_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("daemon.lock");
        let made = std::process::Command::new("mkfifo")
            .arg(&lock)
            .status()
            .unwrap();
        assert!(made.success());
        let refused = open_lock(&lock);
        assert!(
            matches!(&refused, Err(NodeError::UnsafeStateDir { reason, .. }) if reason.contains("not a regular file")),
            "{refused:?}"
        );
    }

    /// A link that takes a missing state directory's name after the start
    /// looked, as one another user plants under `/tmp`, makes the `mkdir`
    /// fail instead of being followed, so the directory it names keeps
    /// its mode.
    #[test]
    fn a_link_made_after_the_look_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let made = make_state_dir(&link).map_err(|e| e.kind());
        assert_eq!(made, Err(std::io::ErrorKind::AlreadyExists));
        assert_eq!(mode(&target), 0o755);
    }

    /// An entry that is a link is judged by who made the link, not by who
    /// owns what it names, so a `peers` link another user planted that
    /// names a file of this user's is still theirs. No test can make a
    /// link another user owns, so the link names `/`, which root owns, and
    /// the node is told it runs as root. Run as root, the test gives the
    /// link to the next user id.
    #[test]
    fn a_state_entry_that_is_a_link_is_judged_as_itself() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let target = std::path::Path::new("/");
        let me = fs::metadata(target).unwrap().uid();
        let peers = state.join("peers");
        std::os::unix::fs::symlink(target, &peers).unwrap();
        if fs::symlink_metadata(&peers).unwrap().uid() == me {
            std::os::unix::fs::lchown(&peers, Some(me + 1), None).unwrap();
        }
        let meta = fs::metadata(&state).unwrap();
        let refused = adopt_state_dir(&state, &meta, me);
        assert!(
            matches!(&refused, Err(NodeError::UnsafeStateDir { reason, .. }) if reason.contains("holds peers, which belongs to user")),
            "{refused:?}"
        );
    }
}
