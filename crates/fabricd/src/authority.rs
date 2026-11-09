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
use fabric_core::managed::launch::{Heartbeat, HeartbeatAnswer};
use fabric_core::managed::{
    AttemptId, AttemptStatus, CheckpointAck, Command, CommandKind, CoveredPath, DomainError,
    Envelope, FenceVerdict, GrantToken, HEARTBEAT_INTERVAL_MS, ManagedEvent, ManagedState, NewIds,
    Observation, OperationId, OperationKey, OperationMark, OperationStatus, OutcomePolicy,
    RecoveryBlock, RecoveryPolicy, ResourceRef, StoredResult, TaskId, Transition, fence,
    recovery_decision,
};
use fabric_journal::{
    Answer, Blocker, CommitError, FILE_NAME, Journal, KeyedRequest, Liveness, OpenError,
};
use tokio::sync::{mpsc, oneshot, watch};
use ulid::Ulid;

use crate::clock::{mono_ms, now_ms};
use crate::paths::StateDir;

mod presence;

pub(crate) use presence::Overdue;
use presence::Presence;

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

/// What the fence said for a covered write: the verdict, or why it could
/// not be run.
type Fenced = Result<Result<(), FenceVerdict>, AuthorityError>;

/// The write behind a fence, run on the authority's thread with what the
/// fence said; it answers the caller itself.
type Write = Box<dyn FnOnce(Fenced) + Send>;

enum Call {
    Submit(Envelope<Command>, Reply<Answer>),
    Observe(JobId, Observation, Reply<Transition>),
    Mark(JobId, OperationId, OperationMark, Reply<Transition>),
    /// An executor's report behind the fence (ADR 0028).
    Report(
        GrantToken,
        Option<NodeId>,
        Observation,
        Reply<Result<Transition, FenceVerdict>>,
    ),
    /// A covered write other than a journal operation: the fence, then the
    /// write itself, on the authority's thread, so nothing commits between
    /// the two. The closure gets the fence's answer and does the write for
    /// `Ok(Ok(()))`, and answers the caller itself.
    Fenced(GrantToken, CoveredPath, Option<NodeId>, Write),
    /// The authority's acceptance of a checkpoint it fetched and verified.
    Acknowledge(JobId, CheckpointAck, Reply<Transition>),
    /// A heartbeat from an executor node: never a journal write by itself.
    Heartbeat(NodeId, Heartbeat, Reply<HeartbeatAnswer>),
    /// One tick of the authority's clock: overdue attempts become
    /// unavailable, and the recovery policy runs for each.
    Tick(u64, Reply<Vec<Overdue>>),
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
    /// A read of the journal that also runs when there is none yet
    /// (`Authority::read_journal`): `gc` reads its live set and its
    /// residuals in one such call, so both are of one state (ADR 0030).
    Read(ReadWork),
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

type DerivedWork = Box<dyn FnOnce(&Place, &mut Holder, &mut Presence) + Send>;

/// A read for [`Authority::read_journal`]: the journal, `None` when there
/// is none, or why the authority stopped.
type ReadWork = Box<dyn FnOnce(Result<Option<&Journal>, AuthorityError>) + Send>;

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
    /// Where its command and observation spans go (ADR 0031).
    trace: crate::otel::Tracer,
}

/// The authority's thread, or where it would work.
#[derive(Debug)]
enum Writer {
    /// No journal at start and no managed command since: no thread, no
    /// queue and no published ids, since there are none. Holds the state
    /// directory the thread will work in and the node.
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
    /// [`Authority::start_traced`] with no spans, for the tests.
    #[cfg(test)]
    pub(crate) fn start(state: &StateDir, node: NodeId) -> std::io::Result<Authority> {
        Authority::start_traced(state, node, crate::otel::Tracer::off())
    }

    /// The authority for `state`, on node `node`, its spans going to
    /// `trace`. Returns at once: with a journal, its thread starts now and
    /// recovers it before any call; without one, no thread starts until a
    /// managed command. The thread holds `trace` too, for what the
    /// authority records on its own (a tick's observations, its `Recover`,
    /// a returned attempt), so it is given before the thread can start.
    /// The authority's own automatic `Recover` takes the grant; the
    /// dispatch places its failover with the M5 placement
    /// (`dispatch::pass`).
    pub(crate) fn start_traced(
        state: &StateDir,
        node: NodeId,
        trace: crate::otel::Tracer,
    ) -> std::io::Result<Authority> {
        // A journal whose presence cannot be read is left to the thread,
        // which fails closed on it.
        let journal = !matches!(managed_dir(state).join(FILE_NAME).try_exists(), Ok(false));
        let authority = Authority {
            writer: Arc::new(Mutex::new(Writer::Idle(state.clone(), node))),
            started: Arc::new(watch::channel(None).0),
            #[cfg(test)]
            spawned: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            trace,
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
                trace: self.trace.clone(),
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

    /// Runs `work` on the journal inside the authority's thread, after
    /// the calls before it, as one read: everything it reads is of one
    /// state of the journal. With no journal it runs with `None` and
    /// starts no thread. It never writes.
    pub(crate) async fn read_journal<T: Send + 'static>(
        &self,
        work: impl FnOnce(Option<&Journal>) -> Result<T, AuthorityError> + Send + 'static,
    ) -> Result<T, AuthorityError> {
        if self.idle() {
            return work(None);
        }
        self.call(|reply: Reply<T>| {
            Call::Read(Box::new(move |journal| {
                let _ = reply.send(journal.and_then(work));
            }))
        })
        .await
    }

    /// The journal as the bridge client records into it (ADR 0030): each
    /// call the client makes is one turn of the authority's thread, so
    /// the thread serves every other call while a worker acts, and the
    /// journal keeps its one writer.
    pub(crate) fn recorder(&self) -> Recording {
        Recording(self.clone())
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
        let span = self
            .trace
            .on()
            .then(|| crate::otel::hooks::command_begin(&envelope));
        let answer = self.call(|reply| Call::Submit(envelope, reply)).await;
        if let Some(mut span) = span {
            crate::otel::hooks::command_end(&mut span, answer.as_ref());
            self.trace.emit(span);
        }
        answer
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
        let span = self
            .trace
            .on()
            .then(|| crate::otel::hooks::derived_begin(&key, job));
        let answer = self
            .call(|reply: Reply<Result<Answer, E>>| {
                Call::Derived(Box::new(move |place, holder, presence| {
                    let _ = reply.send(derived(
                        place, holder, presence, &key, &request, job, derive,
                    ));
                }))
            })
            .await;
        if let Some(mut span) = span {
            // A derivation the caller refused wrote nothing: no span.
            match &answer {
                Ok(Ok(answer)) => {
                    crate::otel::hooks::command_end(&mut span, Ok(answer));
                    self.trace.emit(span);
                }
                Ok(Err(_)) => {}
                Err(error) => {
                    crate::otel::hooks::command_end(&mut span, Err(error));
                    self.trace.emit(span);
                }
            }
        }
        answer
    }

    /// Records the authority's own observation of an attempt or an
    /// operation, or what an executor on this node reports: no runner
    /// presents a grant, so no fence runs (ADR 0028).
    pub(crate) async fn observe_authority(
        &self,
        job: JobId,
        observation: Observation,
    ) -> Result<Transition, AuthorityError> {
        let span = self
            .trace
            .on()
            .then(|| crate::otel::hooks::observation_begin(job, &observation));
        let recorded = self
            .call(|reply| Call::Observe(job, observation, reply))
            .await;
        if let Some(mut span) = span {
            crate::otel::hooks::observation_end(&mut span, recorded.as_ref());
            self.trace.emit(span);
        }
        recorded
    }

    /// Marks an operation on the authority's own behalf, `Dispatched`
    /// right before its effect is asked for, then its outcome when the
    /// answer settles it (the dispatch). No fence runs: no runner
    /// presents a grant.
    pub(crate) async fn mark_authority(
        &self,
        job: JobId,
        operation: OperationId,
        mark: OperationMark,
    ) -> Result<Transition, AuthorityError> {
        self.call(|reply| Call::Mark(job, operation, mark, reply))
            .await
    }

    /// Records an executor's report behind the fence: `token` is the grant
    /// it acts under and `from` the peer it came from, `None` for a report
    /// made on this node, which the fence reads as this node (ADR 0028). A
    /// report from the node an `unknown` attempt was granted to is
    /// presence: the attempt is observed reachable first. With no managed
    /// journal the verdict is `UnknownJob`, as a heartbeat gets.
    pub(crate) async fn report(
        &self,
        token: GrantToken,
        from: Option<NodeId>,
        observation: Observation,
    ) -> Result<Result<Transition, FenceVerdict>, AuthorityError> {
        if self.idle() {
            return Ok(Err(FenceVerdict::UnknownJob));
        }
        let span = self
            .trace
            .on()
            .then(|| crate::otel::hooks::observation_begin(token.job, &observation));
        let recorded = self
            .call(|reply| Call::Report(token, from, observation, reply))
            .await;
        if let Some(mut span) = span {
            crate::otel::hooks::report_end(&mut span, from, recorded.as_ref().map(Result::as_ref));
            self.trace.emit(span);
        }
        recorded
    }

    /// A covered write other than a journal operation (a ledger append, a
    /// bundle publish, a facade outcome), behind the fence (ADR 0028):
    /// `write` runs on the authority's thread right after the fence
    /// answered `Live`, before any other call, so no `Recover`, tick or
    /// command can commit between the check and the write it guards. A
    /// refusal is recorded, returned, and `write` never runs. `from` is
    /// the peer the write came from, `None` for a write made on this node.
    /// With no managed journal the verdict is `UnknownJob`.
    pub(crate) async fn fenced_write<T: Send + 'static>(
        &self,
        token: GrantToken,
        path: CoveredPath,
        from: Option<NodeId>,
        write: impl FnOnce() -> T + Send + 'static,
    ) -> Result<Result<T, FenceVerdict>, AuthorityError> {
        if self.idle() {
            return Ok(Err(FenceVerdict::UnknownJob));
        }
        let (calls, _) = self
            .running()
            .map_err(|e| AuthorityError::Unavailable(format!("its thread did not start: {e}")))?;
        let (reply, answer) = oneshot::channel();
        let call = Call::Fenced(
            token,
            path,
            from,
            Box::new(move |fenced| {
                let out = fenced.map(|fenced| fenced.map(|()| write()));
                let _ = reply.send(out);
            }),
        );
        calls
            .send(call)
            .await
            .map_err(|_| AuthorityError::Unavailable("its thread ended".into()))?;
        answer
            .await
            .map_err(|_| AuthorityError::Unavailable("its thread ended".into()))?
    }

    /// Commits a checkpoint the authority fetched and verified (ADR 0028).
    pub(crate) async fn acknowledge(
        &self,
        job: JobId,
        ack: CheckpointAck,
    ) -> Result<Transition, AuthorityError> {
        let span = self
            .trace
            .on()
            .then(|| crate::otel::hooks::acknowledge_begin(job, &ack));
        let recorded = self.call(|reply| Call::Acknowledge(job, ack, reply)).await;
        if let Some(mut span) = span {
            crate::otel::hooks::observation_end(&mut span, recorded.as_ref());
            self.trace.emit(span);
        }
        recorded
    }

    /// Answers a heartbeat from `from` with the verdict of each grant it
    /// names (ADR 0028). A heartbeat is never a journal write; an attempt
    /// it brings back from `unknown` is one observation.
    pub(crate) async fn heartbeat(
        &self,
        from: NodeId,
        beat: Heartbeat,
    ) -> Result<HeartbeatAnswer, AuthorityError> {
        if self.idle() {
            let verdicts = beat
                .attempts
                .iter()
                .map(|t| (t.attempt, FenceVerdict::UnknownJob))
                .collect();
            return Ok(HeartbeatAnswer { verdicts });
        }
        self.call(|reply| Call::Heartbeat(from, beat, reply)).await
    }

    /// One tick of the authority's monotonic clock at `now` (`mono_ms`):
    /// every tracked attempt silent for `UNAVAILABLE_AFTER_MS` becomes
    /// `unknown`, and the job's recovery policy runs for each attempt that
    /// has been unavailable for `UNAVAILABLE_AFTER_MS`. Returns what became
    /// unavailable on this tick.
    pub(crate) async fn tick(&self, now: u64) -> Result<Vec<Overdue>, AuthorityError> {
        if self.idle() {
            return Ok(Vec::new());
        }
        self.call(|reply| Call::Tick(now, reply)).await
    }

    /// Ticks every `HEARTBEAT_INTERVAL_MS` until the authority is gone.
    /// Nothing runs while no thread does: no journal, no attempts.
    pub(crate) async fn run_ticker(self) {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match self.tick(mono_ms()).await {
                Ok(overdue) => {
                    for o in overdue {
                        tracing::warn!(job = %o.job, attempt = %o.attempt, "an attempt's resource is unavailable: no heartbeat");
                    }
                }
                Err(AuthorityError::Unavailable(_)) => return,
                Err(e) => tracing::warn!(error = %e, "the authority's tick failed"),
            }
        }
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

/// The journal as the bridge client reaches it through the authority
/// ([`Authority::recorder`]).
#[derive(Debug)]
pub(crate) struct Recording(Authority);

impl fabric_bridge::Recorder for Recording {
    type Log = Journal;

    async fn reserve(
        &mut self,
    ) -> Result<fabric_bridge::Slot<'_, Journal>, fabric_bridge::LogError> {
        let ended = || fabric_bridge::LogError::Storage("the managed authority stopped".into());
        let (calls, _) = self.0.running().map_err(|_| ended())?;
        let permit = calls.reserve_owned().await.map_err(|_| ended())?;
        Ok(fabric_bridge::Slot::handed(Box::new(Turn(permit))))
    }
}

/// Room for one call on the authority's queue.
struct Turn(mpsc::OwnedPermit<Call>);

impl fabric_bridge::Hand<Journal> for Turn {
    fn hand(self: Box<Self>, call: fabric_bridge::Handed<Journal>) {
        self.0.send(Call::With {
            create: false,
            writes: true,
            work: Box::new(move |journal| {
                call(journal.map_err(|e| fabric_bridge::LogError::Storage(e.to_string())));
            }),
        });
    }
}

/// Where the authority keeps its journal and where standalone jobs live.
struct Place {
    dir: PathBuf,
    jobs: PathBuf,
    node: NodeId,
    /// Where the spans of what the thread records on its own go.
    trace: crate::otel::Tracer,
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
#[allow(
    clippy::too_many_lines,
    reason = "one arm per call reads better than a helper per arm"
)]
fn serve(
    place: &Place,
    mut queue: mpsc::Receiver<Call>,
    ids: &watch::Sender<Ids>,
    committed: &watch::Sender<u64>,
) {
    let mut holder = open_existing(place, ids);
    let bump = || committed.send_modify(|n| *n = n.wrapping_add(1));
    let mut presence = Presence::default();
    if let Holder::Open(journal) = &holder {
        track_live(place, journal, &mut presence, mono_ms());
    }
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
                    bump();
                }
            }
            Call::Derived(work) => {
                work(place, &mut holder, &mut presence);
                bump();
            }
            Call::Read(work) => work(match &holder {
                Holder::Absent => Ok(None),
                Holder::Open(journal) => Ok(Some(journal)),
                Holder::Stopped(reason) => Err(AuthorityError::Unavailable(reason.clone())),
            }),
            Call::Submit(envelope, reply) => {
                let answer = submit(place, &mut holder, &envelope);
                bump();
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
                    note(
                        place,
                        &mut presence,
                        accepted.job,
                        &accepted.events,
                        mono_ms(),
                    );
                }
                let _ = reply.send(answer);
            }
            Call::Observe(job, observation, reply) => {
                let answer = open(&mut holder, job)
                    .and_then(|j| Ok(j.observe_authority(job, &observation, now_ms())?));
                if let Ok(t) = &answer {
                    note(place, &mut presence, job, &t.events, mono_ms());
                }
                bump();
                let _ = reply.send(answer);
            }
            Call::Mark(job, operation, mark, reply) => {
                let answer = open(&mut holder, job)
                    .and_then(|j| Ok(j.mark_authority(job, operation, mark, now_ms())?));
                bump();
                let _ = reply.send(answer);
            }
            Call::Report(token, from, observation, reply) => {
                let from = from.unwrap_or(place.node);
                let answer = match &mut holder {
                    Holder::Absent => Ok(Err(FenceVerdict::UnknownJob)),
                    _ => open(&mut holder, token.job).and_then(|j| {
                        presence_of(place, j, &mut presence, from, &token);
                        Ok(j.observe(&token, Some(from), &observation, now_ms())?)
                    }),
                };
                if let Ok(Ok(t)) = &answer {
                    note(place, &mut presence, token.job, &t.events, mono_ms());
                }
                bump();
                let _ = reply.send(answer);
            }
            Call::Fenced(token, path, from, write) => {
                let from = from.unwrap_or(place.node);
                let fenced = match &mut holder {
                    Holder::Absent => Ok(Err(FenceVerdict::UnknownJob)),
                    _ => open(&mut holder, token.job).and_then(|j| {
                        presence_of(place, j, &mut presence, from, &token);
                        Ok(j.fence(&token, path, Some(from), now_ms())?)
                    }),
                };
                // The write runs here, before the next call is taken.
                write(fenced);
                bump();
            }
            Call::Acknowledge(job, ack, reply) => {
                let answer =
                    open(&mut holder, job).and_then(|j| Ok(j.acknowledge(job, &ack, now_ms())?));
                bump();
                let _ = reply.send(answer);
            }
            // A heartbeat or a tick moves the counter only when it wrote,
            // so an idle authority wakes no facade watch (ADR 0028). Each
            // call moves it before it answers, so a caller that saw its
            // answer sees the counter moved.
            Call::Heartbeat(from, beat, reply) => {
                let mut wrote = false;
                let answer = heartbeat(place, &mut holder, &mut presence, from, &beat, &mut wrote);
                if wrote {
                    bump();
                }
                let _ = reply.send(answer);
            }
            Call::Tick(now, reply) => {
                let mut wrote = false;
                let answer = tick(place, &mut holder, &mut presence, now, &mut wrote);
                if wrote {
                    bump();
                }
                let _ = reply.send(answer);
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

/// Tracks every live attempt of a remote node the journal holds, at the
/// authority's start (`now` on the monotonic clock): each is overdue
/// `UNAVAILABLE_AFTER_MS` from now unless a heartbeat arrives first, and
/// one the journal says is `unknown` for want of heartbeats is tracked as
/// unavailable, so its heartbeat brings it back and its recovery policy
/// still runs after the restart (ADR 0028).
fn track_live(place: &Place, journal: &Journal, presence: &mut Presence, now: u64) {
    let Ok(jobs) = journal.jobs() else {
        return;
    };
    for job in jobs {
        let Ok(Some(state)) = journal.job(job) else {
            continue;
        };
        for attempt in state.attempts.iter().filter(|a| a.status.may_run()) {
            let ResourceRef::Node { node } = attempt.resource;
            if node == place.node {
                continue;
            }
            if attempt.status == AttemptStatus::Unknown && attempt.unavailable_since_ms.is_some() {
                presence.track_unavailable(job, attempt.id, attempt.generation, node, now);
            } else {
                presence.track(job, attempt.id, attempt.generation, node, now);
            }
        }
    }
}

/// Keeps the presence in step with what a committed transition did to the
/// job's attempts: a grant to a remote node is tracked from `now` (on the
/// monotonic clock); an attempt that ended or lost its grant is forgotten.
fn note(place: &Place, presence: &mut Presence, job: JobId, events: &[ManagedEvent], now: u64) {
    for event in events {
        match event {
            ManagedEvent::AttemptGranted {
                attempt,
                resource: ResourceRef::Node { node },
                generation,
                ..
            } if *node != place.node => {
                presence.track(job, *attempt, *generation, *node, now);
            }
            ManagedEvent::AttemptObserved {
                attempt, status, ..
            } if !status.may_run() => presence.forget(*attempt),
            ManagedEvent::GrantSuperseded { attempt, .. } => presence.forget(*attempt),
            _ => {}
        }
    }
}

/// A heartbeat token or a covered write from `from` for `token` is
/// presence (ADR 0028). An attempt the authority made `unknown` for want
/// of heartbeats, granted to `from` at the job's generation, is observed
/// `ResourceReachable` first, so the write or the fence that follows meets
/// a `running` attempt: `unknown` means the authority could not tell, and
/// the node it granted to is telling it now. A reachable tracked attempt
/// has its presence refreshed without a read; a live grant the start scan
/// did not see (one made while the journal was stopped) is tracked from
/// here. A write from this node is never presence: nothing on this node
/// heartbeats. Writes nothing else; a failed observation is logged and
/// the fence then answers as the journal stands. Returns whether it
/// committed the observation.
fn presence_of(
    place: &Place,
    journal: &mut Journal,
    presence: &mut Presence,
    from: NodeId,
    token: &GrantToken,
) -> bool {
    if from == place.node {
        return false;
    }
    let (now, wall) = (mono_ms(), now_ms());
    if presence.seen(from, token, now, wall) {
        return false;
    }
    // Not tracked, or unavailable: the journal decides.
    let state = match journal.job(token.job) {
        Ok(Some(state)) => state,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(job = %token.job, error = %e, "a write's presence was not read");
            return false;
        }
    };
    let Some(attempt) = state.attempt(token.attempt) else {
        return false;
    };
    let ResourceRef::Node { node } = attempt.resource;
    if node != from
        || attempt.generation != token.generation
        || attempt.generation != state.job.generation
    {
        return false;
    }
    let mut wrote = false;
    match attempt.status {
        AttemptStatus::Unknown if attempt.unavailable_since_ms.is_some() => {
            let back = Observation::ResourceReachable {
                attempt: token.attempt,
                generation: token.generation,
            };
            let recorded = journal.observe_authority(token.job, &back, wall);
            observed_span(place, token.job, &back, &recorded);
            match recorded {
                Ok(t) => wrote = !t.events.is_empty(),
                Err(e) => {
                    tracing::warn!(job = %token.job, attempt = %token.attempt, error = %e, "a returned attempt was not recorded");
                    return false;
                }
            }
        }
        AttemptStatus::Granted | AttemptStatus::Running => {}
        _ => return false,
    }
    presence.track(token.job, token.attempt, token.generation, from, now);
    presence.returned(token.attempt, now, wall);
    wrote
}

/// Answers a heartbeat: each token of a running heartbeat from the node
/// its attempt was granted to is presence (`presence_of`), then every
/// token gets its fence verdict from the projection. No other write.
fn heartbeat(
    place: &Place,
    holder: &mut Holder,
    presence: &mut Presence,
    from: NodeId,
    beat: &Heartbeat,
    wrote: &mut bool,
) -> Result<HeartbeatAnswer, AuthorityError> {
    let journal = match holder {
        Holder::Open(journal) => journal,
        Holder::Absent => {
            let verdicts = beat
                .attempts
                .iter()
                .map(|t| (t.attempt, FenceVerdict::UnknownJob))
                .collect();
            return Ok(HeartbeatAnswer { verdicts });
        }
        Holder::Stopped(reason) => return Err(AuthorityError::Unavailable(reason.clone())),
    };
    let mut verdicts = Vec::with_capacity(beat.attempts.len());
    for token in &beat.attempts {
        // A presentation after a restart names grants the node no longer
        // runs: it is answered, never taken as presence.
        if beat.running {
            *wrote |= presence_of(place, journal, presence, from, token);
        }
        let state = journal.job(token.job)?;
        verdicts.push((token.attempt, fence(state.as_ref(), token, Some(from))));
    }
    Ok(HeartbeatAnswer { verdicts })
}

/// One tick at `now` on the monotonic clock: every overdue attempt is
/// observed `ResourceUnavailable` (the record carries the wall clock), and
/// the recovery policy runs for each attempt that has been unavailable for
/// `UNAVAILABLE_AFTER_MS`. `wrote` says whether anything was committed.
fn tick(
    place: &Place,
    holder: &mut Holder,
    presence: &mut Presence,
    now: u64,
    wrote: &mut bool,
) -> Result<Vec<Overdue>, AuthorityError> {
    let journal = match holder {
        Holder::Open(journal) => journal,
        Holder::Absent => return Ok(Vec::new()),
        Holder::Stopped(reason) => return Err(AuthorityError::Unavailable(reason.clone())),
    };
    let wall = now_ms();
    let mut out = Vec::new();
    for o in presence.overdue(now) {
        let unavailable = Observation::ResourceUnavailable {
            attempt: o.attempt,
            generation: o.generation,
            last_heartbeat_ms: o.last_heartbeat_ms,
            at_ms: wall,
        };
        let recorded = journal.observe_authority(o.job, &unavailable, wall);
        observed_span(place, o.job, &unavailable, &recorded);
        match recorded {
            Ok(t) => {
                *wrote |= !t.events.is_empty();
                presence.mark_unavailable(o.attempt, now);
                out.push(o);
            }
            Err(CommitError::Domain(_)) => presence.forget(o.attempt),
            Err(e) => return Err(e.into()),
        }
    }
    for o in presence.unavailable(now) {
        recover(place, journal, presence, &o, now, wall, wrote)?;
    }
    Ok(out)
}

/// The key of the authority's own `Recover` for an attempt, derived from
/// the attempt and its generation, so a restart after a crash replays the
/// same operation and never grants twice.
fn recover_key(
    job: JobId,
    attempt: AttemptId,
    generation: fabric_core::managed::Generation,
) -> OperationKey {
    OperationKey::parse(&format!("recover.{job}.{attempt}.{}", generation.get()))
        .unwrap_or_else(|_| OperationKey::parse("recover").unwrap_or_else(|_| unreachable_key()))
}

fn unreachable_key() -> OperationKey {
    // "recover" is within the key grammar; this arm never runs.
    OperationKey::parse("r").unwrap_or_else(|_| std::process::abort())
}

/// Runs the recovery policy for one attempt that has been unavailable for
/// the threshold (ADR 0028): under `Block` nothing; under
/// `RestartFromAcknowledged` the pure decision, judged by the attempt's
/// own task, then the `Recover` it names under its derived key, or the
/// reason it did not restart, recorded once per silence and reason. The
/// `Recover` takes the grant; its failover is placed by the dispatch with
/// the M5 placement, the unavailable node excluded, and granted like a
/// continue (`dispatch::place_failover`). `now` is the monotonic clock, `wall` the time the records carry.
fn recover(
    place: &Place,
    journal: &mut Journal,
    presence: &mut Presence,
    o: &Overdue,
    now: u64,
    wall: u64,
    wrote: &mut bool,
) -> Result<(), AuthorityError> {
    let Some(state) = journal.job(o.job)? else {
        presence.forget(o.attempt);
        return Ok(());
    };
    let Some(attempt) = state.attempt(o.attempt) else {
        presence.forget(o.attempt);
        return Ok(());
    };
    if attempt.status != AttemptStatus::Unknown {
        presence.forget(o.attempt);
        return Ok(());
    }
    if state.job.policy.recovery != RecoveryPolicy::RestartFromAcknowledged {
        return Ok(());
    }
    let Some(task) = state.task(attempt.task) else {
        presence.forget(o.attempt);
        return Ok(());
    };
    let outcome = OutcomePolicy::of_task(task);
    let blockers: Vec<OperationId> = journal
        .replacement_blockers(o.job)?
        .iter()
        .map(Blocker::operation)
        .collect();
    let decided = recovery_decision(&state, o.attempt, outcome, &blockers);
    let reason = match decided {
        Ok(command) => {
            let envelope = Envelope {
                key: recover_key(o.job, o.attempt, o.generation),
                job: Some(o.job),
                expected: None,
                body: command,
            };
            let ids = NewIds {
                job: o.job,
                task: TaskId::from_bytes(ulid()),
                attempt: AttemptId::from_bytes(ulid()),
                operation: OperationId::from_bytes(ulid()),
                at_ms: wall,
            };
            let span = place
                .trace
                .on()
                .then(|| crate::otel::hooks::command_begin(&envelope));
            let answer = journal.submit(&envelope, ids).map_err(AuthorityError::from);
            if let Some(mut span) = span {
                crate::otel::hooks::command_end(&mut span, answer.as_ref());
                place.trace.emit(span);
            }
            let answer = answer?;
            *wrote |= !answer.replayed;
            match answer.result {
                StoredResult::Accepted(accepted) => {
                    tracing::info!(job = %o.job, attempt = %o.attempt, operation = %accepted.operation, "recovered an unavailable attempt from its acknowledged checkpoint");
                    presence.forget(o.attempt);
                    note(place, presence, o.job, &accepted.events, now);
                    return Ok(());
                }
                StoredResult::Rejected(DomainError::ReplacementBlocked { operation }) => {
                    RecoveryBlock::ReplacementBlocked {
                        blockers: vec![operation],
                    }
                }
                StoredResult::Rejected(other) => {
                    // Stored under the recover's derived key, so this
                    // silence never recovers by itself: recorded as the
                    // reason, then no longer judged.
                    tracing::warn!(job = %o.job, attempt = %o.attempt, refusal = %other, "the authority's recover was refused");
                    let reason = RecoveryBlock::Refused {
                        refusal: other.to_string(),
                    };
                    if presence.block_changed(o.attempt, &reason) {
                        blocked(place, journal, o, reason, wall)?;
                        *wrote = true;
                    }
                    presence.forget(o.attempt);
                    return Ok(());
                }
            }
        }
        Err(reason) => reason,
    };
    if presence.block_changed(o.attempt, &reason) {
        blocked(place, journal, o, reason, wall)?;
        *wrote = true;
    }
    Ok(())
}

/// Records why the policy did not recover `o`, with its span.
fn blocked(
    place: &Place,
    journal: &mut Journal,
    o: &Overdue,
    reason: RecoveryBlock,
    wall: u64,
) -> Result<(), AuthorityError> {
    let span_reason = place.trace.on().then(|| reason.clone());
    let recorded = journal
        .recovery_blocked(o.job, o.attempt, reason, wall)
        .map_err(AuthorityError::from);
    if let Some(reason) = span_reason {
        place.trace.emit(crate::otel::hooks::recovery_blocked(
            o.job,
            o.attempt,
            &reason,
            recorded.as_ref(),
        ));
    }
    recorded.map(|_| ())
}

/// Emits the span of an observation the authority made on its own
/// thread (ADR 0028): a tick's `ResourceUnavailable`, a returned
/// attempt's `ResourceReachable`.
fn observed_span(
    place: &Place,
    job: JobId,
    observation: &Observation,
    recorded: &Result<Transition, CommitError>,
) {
    if place.trace.on() {
        let mut span = crate::otel::hooks::observation_begin(job, observation);
        crate::otel::hooks::observation_committed(&mut span, recorded.as_ref());
        place.trace.emit(span);
    }
}

/// The body of [`Authority::submit_derived`] on the authority's thread.
fn derived<E>(
    place: &Place,
    holder: &mut Holder,
    presence: &mut Presence,
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
    let answer = submit(place, holder, &envelope)?;
    // A continue grants the new attempt: it is tracked like any grant, so
    // its silence is seen (ADR 0028).
    if let StoredResult::Accepted(accepted) = &answer.result {
        note(place, presence, job, &accepted.events, mono_ms());
    }
    Ok(Ok(answer))
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
