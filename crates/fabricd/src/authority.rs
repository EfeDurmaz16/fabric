//! The managed authority (ADR 0020): the one writer of the managed
//! journal, `<state>/managed/journal.sqlite`, inside the daemon.
//!
//! Invariant: one thread owns the journal and applies calls one at a time
//! from a bounded queue, so commands are serialized without a lock and no
//! other code in the daemon opens the file. The journal is made by the
//! first `CreateJob`, never by a start; a state directory without one
//! behaves as before managed jobs existed: the start pays one `stat` and
//! starts no thread. The thread starts with the daemon when a journal
//! exists, and otherwise with the first managed command, so a daemon that
//! never runs a managed job has neither the thread nor its queue. Until
//! then a read (a job, the list, whether an id is managed) answers from
//! the start's look, which found no journal, and a command that starts
//! the thread does so under the same lock, so no read misses a job a
//! command made. When a journal exists, the thread opens it, runs
//! `verify` and restart recovery before it answers any call, off the
//! daemon's start path: a dispatched operation becomes unknown and
//! nothing is replayed or granted. A journal that fails `verify` stops
//! the authority, never the daemon: every managed call is refused with
//! the reason and standalone jobs run as before. A managed job never
//! takes an id a standalone job has, and a managed command never acts on
//! a standalone job.
//!
//! The ids the journal holds are also published outside the queue, read
//! once the file opens and before `verify` and recovery run, so a
//! standalone lookup of an id this node does not know (a peer's first
//! push of a job, for one) never waits behind recovery or a list. The
//! ids stay published when `verify` then fails; only a journal that does
//! not open leaves them unknown, and a lookup then fails closed.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::{
    AttemptId, Command, CommandKind, DomainError, Envelope, ManagedState, NewIds, Observation,
    OperationId, OperationKey, OperationMark, OperationStatus, StoredResult, TaskId, Transition,
};
use fabric_journal::{Answer, CommitError, FILE_NAME, Journal, KeyedRequest, Liveness, OpenError};
use tokio::sync::{mpsc, oneshot, watch};
use ulid::Ulid;

use crate::clock::now_ms;
use crate::paths::StateDir;

/// How many calls may wait for the authority before a caller waits to queue one.
const QUEUE: usize = 64;

/// How many fresh job ids the authority draws before it gives up on one
/// no standalone job has; a ULID collision is already out of reach.
const ID_DRAWS: usize = 8;

/// The directory of the managed journal inside a state directory.
pub(crate) fn managed_dir(state: &StateDir) -> PathBuf {
    state.root().join("managed")
}

/// The managed journal's path inside a state directory.
#[cfg(test)]
pub(crate) fn journal_path(state: &StateDir) -> PathBuf {
    managed_dir(state).join(FILE_NAME)
}

/// Why the authority did not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AuthorityError {
    /// The authority stopped on a journal it could not trust, or is gone.
    #[error("the managed authority is stopped: {0}")]
    Unavailable(String),
    /// The id names a standalone job; a managed command never acts on one (ADR 0020).
    #[error("job {0} is a standalone job; managed commands do not act on it")]
    Standalone(JobId),
    /// The journal could not be made.
    #[error(transparent)]
    Open(#[from] OpenError),
    /// The journal refused or failed the change.
    #[error(transparent)]
    Commit(#[from] CommitError),
}

impl AuthorityError {
    /// The domain's refusal, when that is what this is.
    #[cfg(test)]
    pub(crate) fn domain(&self) -> Option<&DomainError> {
        match self {
            AuthorityError::Commit(CommitError::Domain(e)) => Some(e),
            _ => None,
        }
    }
}

type Reply<T> = oneshot::Sender<Result<T, AuthorityError>>;

enum Call {
    Submit(Envelope<Command>, Reply<Answer>),
    Observe(JobId, Observation, Reply<Transition>),
    Mark(JobId, OperationId, OperationMark, Reply<Transition>),
    Job(JobId, Reply<Option<ManagedState>>),
    List(Reply<Vec<ManagedState>>),
    Pending(Reply<Vec<PendingEffect>>),
    /// Work on the open journal by the facade and the inspect read (owner
    /// B, M5): the closure answers its own caller. `create` makes the
    /// journal when there is none yet, as a first `CreateJob` does, for a
    /// token minted before any job. `writes` says whether the work may
    /// change the journal; only then is the commit counter moved, so a
    /// read wakes no watcher (one tick per call that changed the journal).
    With {
        create: bool,
        writes: bool,
        work: JournalWork,
    },
    /// A command whose body is derived from the job's state, deduplicated
    /// by the caller's request (`Authority::submit_derived`).
    Derived(DerivedWork),
}

/// An accepted operation whose effect the node has not asked for yet: a
/// grant to launch, a create to place, or an M5 command to dispatch. The
/// request an M5 command was asked with is in the job's state
/// (`ManagedState::request`). Read anew from the journal every time, so a
/// restart and a running daemon see the same list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingEffect {
    /// The job.
    pub job: JobId,
    /// The operation.
    pub operation: OperationId,
    /// Its kind.
    pub kind: CommandKind,
}

/// Work the facade or a read runs inside the authority's thread; it
/// answers its own caller.
type JournalWork = Box<dyn FnOnce(Result<&mut Journal, AuthorityError>) + Send>;

type DerivedWork = Box<dyn FnOnce(&Place, &mut Holder) + Send>;

/// The digest of a caller's request for [`Authority::submit_derived`]:
/// blake3 under its own domain over each part, length first, so two
/// requests that differ in any part differ here.
pub(crate) fn request_digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fabric.managed.request.v1\0");
    for part in parts {
        hasher.update(&(part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

/// The managed ids as the standalone paths read them, without the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ids {
    /// The thread has not read the journal's ids yet.
    Loading,
    /// Every id the journal holds; empty when there is no journal.
    Known(HashSet<JobId>),
    /// The journal did not open, so its ids are unknown.
    Unreadable(String),
}

/// The handle the daemon's tasks call the authority through.
#[derive(Debug, Clone)]
pub(crate) struct Authority {
    writer: Arc<Mutex<Writer>>,
    /// The commit counter once the thread runs, `None` before: what the
    /// node's dispatch waits on without starting the thread itself.
    started: Arc<watch::Sender<Option<watch::Receiver<u64>>>>,
    /// How many threads this authority started, for the tests.
    #[cfg(test)]
    spawned: Arc<std::sync::atomic::AtomicUsize>,
}

/// The authority's thread, or where it would work.
#[derive(Debug)]
enum Writer {
    /// No journal at start and no managed command since: no thread, no
    /// queue and no published ids, since there are none. Holds the state
    /// directory the thread will work in.
    Idle(StateDir, NodeId),
    /// The thread runs.
    Running {
        /// The queue it takes calls from. Declared before `thread`, so the
        /// last handle's drop closes the queue, which ends the thread,
        /// before it joins it.
        calls: mpsc::Sender<Call>,
        /// The ids it publishes outside the queue.
        ids: watch::Receiver<Ids>,
        /// How many calls have changed the journal; a watcher of the
        /// facade waits on it (M5).
        commits: watch::Receiver<u64>,
        /// The thread, joined when the last handle drops, so a dropped
        /// authority has closed its journal: `SQLite` unlinks the WAL and
        /// shm beside the journal on the last close, and a caller that
        /// looks at the state directory after the drop must not race it.
        _thread: JoinOnDrop,
    },
}

/// A thread joined when dropped.
#[derive(Debug)]
struct JoinOnDrop(Option<std::thread::JoinHandle<()>>);

impl Drop for JoinOnDrop {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            // A panic in the thread already failed the call it served.
            let _ = thread.join();
        }
    }
}

impl Authority {
    /// The authority for `state`, on node `node`. Returns at once: with a
    /// journal, its thread starts now and recovers it before any call;
    /// without one, no thread starts until a managed command.
    pub(crate) fn start(state: &StateDir, node: NodeId) -> std::io::Result<Authority> {
        // A journal whose presence cannot be read is left to the thread,
        // which fails closed on it.
        let journal = !matches!(managed_dir(state).join(FILE_NAME).try_exists(), Ok(false));
        let authority = Authority {
            writer: Arc::new(Mutex::new(Writer::Idle(state.clone(), node))),
            started: Arc::new(watch::channel(None).0),
            #[cfg(test)]
            spawned: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        if journal {
            authority.running()?;
        }
        Ok(authority)
    }

    /// How many threads this authority started.
    #[cfg(test)]
    pub(crate) fn threads(&self) -> usize {
        self.spawned.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The thread's queue and ids, starting the thread when none runs.
    fn running(&self) -> std::io::Result<(mpsc::Sender<Call>, watch::Receiver<Ids>)> {
        let mut writer = crate::node::lock(&self.writer);
        let place = match &*writer {
            Writer::Running { calls, ids, .. } => return Ok((calls.clone(), ids.clone())),
            Writer::Idle(state, node) => Place {
                dir: managed_dir(state),
                jobs: state.jobs(),
                node: *node,
            },
        };
        let (calls, queue) = mpsc::channel(QUEUE);
        let (publish, ids) = watch::channel(Ids::Loading);
        let (committed, commits) = watch::channel(0_u64);
        let thread = std::thread::Builder::new()
            .name("fabric-authority".into())
            .spawn(move || serve(&place, queue, &publish, &committed))?;
        #[cfg(test)]
        self.spawned
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.send_replace(Some(commits.clone()));
        *writer = Writer::Running {
            calls: calls.clone(),
            ids: ids.clone(),
            commits,
            _thread: JoinOnDrop(Some(thread)),
        };
        Ok((calls, ids))
    }

    /// A counter of the calls that changed the journal, for a facade watch
    /// that waits for the next event (M5): it starts the thread when none
    /// runs, so a watch on an empty authority waits on a live counter.
    pub(crate) fn commits(&self) -> Result<watch::Receiver<u64>, AuthorityError> {
        self.running()
            .map_err(|e| AuthorityError::Unavailable(format!("its thread did not start: {e}")))?;
        match &*crate::node::lock(&self.writer) {
            Writer::Running { commits, .. } => Ok(commits.clone()),
            Writer::Idle(..) => Err(AuthorityError::Unavailable(
                "its thread did not start".into(),
            )),
        }
    }

    /// The commit counter, once the thread runs: waits without starting
    /// it, so a node with no managed job starts no thread for its
    /// dispatch. `None` when the authority is dropped before it starts.
    pub(crate) async fn started(&self) -> Option<watch::Receiver<u64>> {
        let mut started = self.started.subscribe();
        started.wait_for(Option::is_some).await.ok()?.clone()
    }

    /// Runs `work` on the journal inside the authority's thread, after
    /// the calls before it (M5, owner B: the facade's token rows and
    /// reads, the inspect read). With `create`, a journal is made when
    /// there is none, as a first `CreateJob` makes it; without it, an
    /// absent journal is `UnknownJob` for the job the work names, or
    /// unavailable for work that names none.
    pub(crate) async fn with_journal<T: Send + 'static>(
        &self,
        create: bool,
        work: impl FnOnce(&mut Journal) -> Result<T, AuthorityError> + Send + 'static,
    ) -> Result<T, AuthorityError> {
        self.journal_call(create, false, work).await
    }

    /// As [`Authority::with_journal`], for work that writes (a token
    /// minted or revoked): the commit counter moves after it, so a watch
    /// sees the write.
    pub(crate) async fn write_journal<T: Send + 'static>(
        &self,
        create: bool,
        work: impl FnOnce(&mut Journal) -> Result<T, AuthorityError> + Send + 'static,
    ) -> Result<T, AuthorityError> {
        self.journal_call(create, true, work).await
    }

    async fn journal_call<T: Send + 'static>(
        &self,
        create: bool,
        writes: bool,
        work: impl FnOnce(&mut Journal) -> Result<T, AuthorityError> + Send + 'static,
    ) -> Result<T, AuthorityError> {
        self.call(|reply: Reply<T>| Call::With {
            create,
            writes,
            work: Box::new(move |journal| {
                let _ = reply.send(journal.and_then(work));
            }),
        })
        .await
    }

    /// Whether no thread runs: no journal at start and no command since,
    /// so the authority holds no job. Read under the lock a command
    /// starts the thread under.
    fn idle(&self) -> bool {
        matches!(*crate::node::lock(&self.writer), Writer::Idle(..))
    }

    /// Whether the journal holds `job`, without waiting behind the queue:
    /// only the open of the journal's file is waited for. An error when
    /// the journal did not open, so the caller fails closed.
    pub(crate) async fn holds(&self, job: JobId) -> Result<bool, AuthorityError> {
        let mut ids = match &*crate::node::lock(&self.writer) {
            Writer::Idle(..) => return Ok(false),
            Writer::Running { ids, .. } => ids.clone(),
        };
        let ids = ids
            .wait_for(|ids| !matches!(ids, Ids::Loading))
            .await
            .map_err(|_| AuthorityError::Unavailable("its thread ended".into()))?;
        match &*ids {
            Ids::Known(set) => Ok(set.contains(&job)),
            Ids::Unreadable(reason) => Err(AuthorityError::Unavailable(reason.clone())),
            Ids::Loading => Err(AuthorityError::Unavailable(
                "the journal's ids are not read yet".into(),
            )),
        }
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Call) -> Result<T, AuthorityError> {
        let (calls, _) = self
            .running()
            .map_err(|e| AuthorityError::Unavailable(format!("its thread did not start: {e}")))?;
        let (reply, answer) = oneshot::channel();
        calls
            .send(make(reply))
            .await
            .map_err(|_| AuthorityError::Unavailable("its thread ended".into()))?;
        answer
            .await
            .map_err(|_| AuthorityError::Unavailable("its thread ended".into()))?
    }

    /// Applies a command once (ADR 0019): the same key and command get
    /// the same answer, before and after a restart.
    pub(crate) async fn submit(
        &self,
        envelope: Envelope<Command>,
    ) -> Result<Answer, AuthorityError> {
        self.call(|reply| Call::Submit(envelope, reply)).await
    }

    /// Applies a command whose body `derive` makes from `job`'s state (a
    /// checkpoint names the live attempt, a continue its task) once per
    /// request: `request` is the digest of what the caller asked
    /// ([`request_digest`]), and it, not the derived body, decides
    /// sameness under `key`. The same request again answers the stored
    /// answer as a replay without deriving, however the job moved since;
    /// another request under the key is `OperationConflict`. The check,
    /// the derivation and the commit run as one call on the authority's
    /// thread, so nothing interleaves.
    pub(crate) async fn submit_derived<E: Send + 'static>(
        &self,
        key: OperationKey,
        request: [u8; 32],
        job: JobId,
        derive: impl FnOnce(&ManagedState) -> Result<Envelope<Command>, E> + Send + 'static,
    ) -> Result<Result<Answer, E>, AuthorityError> {
        self.call(|reply: Reply<Result<Answer, E>>| {
            Call::Derived(Box::new(move |place, holder| {
                let _ = reply.send(derived(place, holder, &key, &request, job, derive));
            }))
        })
        .await
    }

    /// Records what an executor reports about an attempt or an operation.
    pub(crate) async fn observe(
        &self,
        job: JobId,
        observation: Observation,
    ) -> Result<Transition, AuthorityError> {
        self.call(|reply| Call::Observe(job, observation, reply))
            .await
    }

    /// Marks an operation, `Dispatched` right before its effect is asked for.
    #[allow(
        dead_code,
        reason = "a launch's dispatch marks through it; no launch is dispatched by the node in M5, so only the tests call it"
    )]
    pub(crate) async fn mark(
        &self,
        job: JobId,
        operation: OperationId,
        mark: OperationMark,
    ) -> Result<Transition, AuthorityError> {
        self.call(|reply| Call::Mark(job, operation, mark, reply))
            .await
    }

    /// A managed job, or `None` when the authority has no such job.
    pub(crate) async fn job(&self, job: JobId) -> Result<Option<ManagedState>, AuthorityError> {
        if self.idle() {
            return Ok(None);
        }
        self.call(|reply| Call::Job(job, reply)).await
    }

    /// Every managed job, oldest first.
    pub(crate) async fn list(&self) -> Result<Vec<ManagedState>, AuthorityError> {
        if self.idle() {
            return Ok(Vec::new());
        }
        self.call(Call::List).await
    }

    /// Every accepted operation whose effect was not asked for yet, oldest
    /// job first: what the node's dispatch loop takes next, after a
    /// restart as during a run. Each is dispatched once, after
    /// `mark(Dispatched)` or, for a create or a continue, by its `Placed`.
    pub(crate) async fn pending(&self) -> Result<Vec<PendingEffect>, AuthorityError> {
        if self.idle() {
            return Ok(Vec::new());
        }
        self.call(Call::Pending).await
    }
}

/// Where the authority keeps its journal and where standalone jobs live.
struct Place {
    dir: PathBuf,
    jobs: PathBuf,
    node: NodeId,
}

impl Place {
    fn journal(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Whether `job` is a standalone job's directory on this node.
    fn standalone(&self, job: JobId) -> bool {
        self.jobs.join(job.to_string()).exists()
    }
}

/// The journal, once there is one.
enum Holder {
    Absent,
    Open(Journal),
    Stopped(String),
}

/// Opens and recovers an existing journal; nothing when there is none.
/// The journal's ids are published once it opens, before `verify`.
fn open_existing(place: &Place, ids: &watch::Sender<Ids>) -> Holder {
    let path = place.journal();
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            ids.send_replace(Ids::Known(HashSet::new()));
            return Holder::Absent;
        }
        Err(e) => {
            let e = format!("{}: {e}", path.display());
            ids.send_replace(Ids::Unreadable(e.clone()));
            return stopped(&e);
        }
    }
    let started = std::time::Instant::now();
    let opened = Journal::open(&path)
        .map_err(|e| e.to_string())
        .and_then(|journal| match journal.jobs() {
            Ok(jobs) => Ok((journal, jobs)),
            Err(e) => Err(e.to_string()),
        });
    let mut journal = match opened {
        Ok((journal, jobs)) => {
            ids.send_replace(Ids::Known(jobs.into_iter().collect()));
            journal
        }
        Err(e) => {
            ids.send_replace(Ids::Unreadable(e.clone()));
            return stopped(&e);
        }
    };
    match journal.verify() {
        Ok(report) => tracing::info!(
            jobs = report.jobs,
            events = report.events,
            "managed journal verified"
        ),
        Err(e) => return stopped(&e),
    }
    // No attempt records a process group yet (an executor reports one in
    // M3), so nothing can be verified gone or running: attempts stay as
    // they are, and only dispatched operations change, to unknown.
    match journal.recover(now_ms(), |_| Liveness::CannotTell) {
        Ok(recovery) => {
            for (job, operation) in &recovery.unknown {
                tracing::warn!(%job, %operation, "a dispatched operation's outcome is unknown after the restart");
            }
            for (job, operation) in &recovery.to_dispatch {
                tracing::info!(%job, %operation, "an accepted operation waits for its dispatch");
            }
        }
        Err(e) => return stopped(&e),
    }
    tracing::info!(
        took_ms = started.elapsed().as_millis(),
        "managed authority recovered"
    );
    Holder::Open(journal)
}

fn stopped(e: &dyn std::fmt::Display) -> Holder {
    tracing::error!(error = %e, "the managed authority stopped; managed commands are refused until the journal is repaired");
    Holder::Stopped(e.to_string())
}

/// The authority's thread: recovery first, then one call at a time.
fn serve(
    place: &Place,
    mut queue: mpsc::Receiver<Call>,
    ids: &watch::Sender<Ids>,
    committed: &watch::Sender<u64>,
) {
    let mut holder = open_existing(place, ids);
    let tick = || committed.send_modify(|n| *n = n.wrapping_add(1));
    while let Some(call) = queue.blocking_recv() {
        match call {
            Call::With {
                create,
                writes,
                work,
            } => {
                if create && matches!(holder, Holder::Absent) {
                    match self::create(place) {
                        Ok(journal) => holder = Holder::Open(journal),
                        Err(e) => {
                            work(Err(e));
                            continue;
                        }
                    }
                }
                work(match &mut holder {
                    Holder::Open(journal) => Ok(journal),
                    Holder::Absent => {
                        Err(AuthorityError::Unavailable("no managed journal yet".into()))
                    }
                    Holder::Stopped(reason) => Err(AuthorityError::Unavailable(reason.clone())),
                });
                if writes {
                    tick();
                }
            }
            Call::Derived(work) => {
                work(place, &mut holder);
                tick();
            }
            Call::Submit(envelope, reply) => {
                let answer = submit(place, &mut holder, &envelope);
                tick();
                // Published before the answer, so no caller that saw the
                // job made can look its id up and miss it.
                if let Ok(Answer {
                    result: StoredResult::Accepted(accepted),
                    ..
                }) = &answer
                {
                    ids.send_modify(|ids| {
                        if let Ids::Known(set) = ids {
                            set.insert(accepted.job);
                        }
                    });
                }
                let _ = reply.send(answer);
            }
            Call::Observe(job, observation, reply) => {
                let _ = reply.send(
                    open(&mut holder, job)
                        .and_then(|j| Ok(j.observe(job, &observation, now_ms())?)),
                );
                tick();
            }
            Call::Mark(job, operation, mark, reply) => {
                let _ = reply.send(
                    open(&mut holder, job)
                        .and_then(|j| Ok(j.mark(job, operation, mark, now_ms())?)),
                );
                tick();
            }
            Call::Job(job, reply) => {
                let _ = reply.send(match &holder {
                    Holder::Absent => Ok(None),
                    Holder::Open(j) => j.job(job).map_err(AuthorityError::from),
                    Holder::Stopped(reason) => Err(AuthorityError::Unavailable(reason.clone())),
                });
            }
            Call::List(reply) => {
                let _ = reply.send(list(&holder));
            }
            Call::Pending(reply) => {
                let _ = reply.send(list(&holder).map(|jobs| pending(&jobs)));
            }
        }
    }
}

/// The accepted effects of `jobs`: grants that may still be dispatched
/// and M5 commands still `accepted`.
fn pending(jobs: &[ManagedState]) -> Vec<PendingEffect> {
    let mut out = Vec::new();
    for state in jobs {
        for op in &state.operations {
            let waits = match op.kind {
                CommandKind::StartAttempt => state.grant_may_dispatch(op.id),
                _ => op.status == OperationStatus::Accepted,
            };
            if waits {
                out.push(PendingEffect {
                    job: state.job.id,
                    operation: op.id,
                    kind: op.kind,
                });
            }
        }
    }
    out
}

/// The open journal; with none yet, `job` is a job the authority does not have.
fn open(holder: &mut Holder, job: JobId) -> Result<&mut Journal, AuthorityError> {
    match holder {
        Holder::Open(journal) => Ok(journal),
        Holder::Absent => Err(CommitError::Domain(DomainError::UnknownJob { job }).into()),
        Holder::Stopped(reason) => Err(AuthorityError::Unavailable(reason.clone())),
    }
}

fn list(holder: &Holder) -> Result<Vec<ManagedState>, AuthorityError> {
    match holder {
        Holder::Absent => Ok(Vec::new()),
        Holder::Open(journal) => {
            let mut out = Vec::new();
            for job in journal.jobs()? {
                if let Some(state) = journal.job(job)? {
                    out.push(state);
                }
            }
            Ok(out)
        }
        Holder::Stopped(reason) => Err(AuthorityError::Unavailable(reason.clone())),
    }
}

fn ulid() -> [u8; 16] {
    Ulid::generate().to_bytes()
}

/// The body of [`Authority::submit_derived`] on the authority's thread.
fn derived<E>(
    place: &Place,
    holder: &mut Holder,
    key: &OperationKey,
    request: &[u8; 32],
    job: JobId,
    derive: impl FnOnce(&ManagedState) -> Result<Envelope<Command>, E>,
) -> Result<Result<Answer, E>, AuthorityError> {
    let unknown = || -> AuthorityError {
        if place.standalone(job) {
            AuthorityError::Standalone(job)
        } else {
            CommitError::Domain(DomainError::UnknownJob { job }).into()
        }
    };
    let journal = match holder {
        Holder::Open(journal) => journal,
        Holder::Absent => return Err(unknown()),
        Holder::Stopped(reason) => return Err(AuthorityError::Unavailable(reason.clone())),
    };
    match journal.keyed_request(key, request)? {
        KeyedRequest::Other => {
            return Err(
                CommitError::Domain(DomainError::OperationConflict { key: key.clone() }).into(),
            );
        }
        KeyedRequest::Same => {
            if let Some(answer) = journal.stored_answer(key)? {
                return Ok(Ok(answer));
            }
        }
        KeyedRequest::New => {}
    }
    let Some(state) = journal.job(job)? else {
        return Err(unknown());
    };
    let envelope = match derive(&state) {
        Ok(envelope) => envelope,
        Err(e) => return Ok(Err(e)),
    };
    if envelope.key != *key || envelope.job != Some(job) {
        return Err(CommitError::Domain(DomainError::BadEnvelope).into());
    }
    journal.record_request(key, request)?;
    Ok(Ok(submit(place, holder, &envelope)?))
}

fn submit(
    place: &Place,
    holder: &mut Holder,
    envelope: &Envelope<Command>,
) -> Result<Answer, AuthorityError> {
    if let Holder::Stopped(reason) = holder {
        return Err(AuthorityError::Unavailable(reason.clone()));
    }
    if let Some(job) = envelope.job
        && place.standalone(job)
        && !matches!(holder, Holder::Open(j) if j.contains(job).unwrap_or(false))
    {
        return Err(AuthorityError::Standalone(job));
    }
    if matches!(holder, Holder::Absent) {
        if !matches!(envelope.body, Command::CreateJob { .. }) {
            // No journal, so no managed job and no stored key: refused
            // without a record, as any command for an unknown job would be.
            return Err(CommitError::Domain(match envelope.job {
                Some(job) => DomainError::UnknownJob { job },
                None => DomainError::BadEnvelope,
            })
            .into());
        }
        *holder = Holder::Open(create(place)?);
    }
    let Holder::Open(journal) = holder else {
        return Err(AuthorityError::Unavailable(
            "the journal did not open".into(),
        ));
    };
    let ids = NewIds {
        job: fresh_job(place, journal)?,
        task: TaskId::from_bytes(ulid()),
        attempt: AttemptId::from_bytes(ulid()),
        operation: OperationId::from_bytes(ulid()),
        at_ms: now_ms(),
    };
    Ok(journal.submit(envelope, ids)?)
}

/// Makes the journal and its private directory for the first managed command.
fn create(place: &Place) -> Result<Journal, AuthorityError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match fs::DirBuilder::new().mode(0o700).create(&place.dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(source) => {
            return Err(OpenError::Io {
                path: place.dir.clone(),
                source,
            }
            .into());
        }
    }
    fs::set_permissions(&place.dir, fs::Permissions::from_mode(0o700)).map_err(|source| {
        OpenError::Io {
            path: place.dir.clone(),
            source,
        }
    })?;
    tracing::info!(path = %place.journal().display(), "made the managed journal");
    Ok(Journal::create(&place.journal(), place.node, now_ms())?)
}

/// A job id neither a standalone job nor a managed one has.
fn fresh_job(place: &Place, journal: &Journal) -> Result<JobId, AuthorityError> {
    for _ in 0..ID_DRAWS {
        let job = JobId::from_bytes(ulid());
        if !place.standalone(job) && !journal.contains(job)? {
            return Ok(job);
        }
    }
    Err(AuthorityError::Unavailable(
        "no fresh job id after repeated draws".into(),
    ))
}

#[cfg(test)]
mod tests;
