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
    AttemptId, AttemptStatus, CheckpointAck, Command, CoveredPath, DomainError, Envelope,
    FenceVerdict, GrantToken, HEARTBEAT_INTERVAL_MS, ManagedEvent, ManagedState, NewIds,
    Observation, OperationId, OperationKey, OperationMark, OutcomePolicy, RecoveryBlock,
    RecoveryPolicy, ResourceRef, StoredResult, TaskId, Transition, fence, recovery_decision,
};
use fabric_journal::{Answer, Blocker, CommitError, FILE_NAME, Journal, Liveness, OpenError};
use fabric_net::wire::{Heartbeat, HeartbeatAnswer};
use tokio::sync::{mpsc, oneshot, watch};
use ulid::Ulid;

use crate::clock::now_ms;
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

enum Call {
    Submit(Envelope<Command>, Reply<Answer>),
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the executor wiring of M3 reports through it")
    )]
    Observe(JobId, Observation, Reply<Transition>),
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the executor wiring of M3 dispatches through it")
    )]
    Mark(JobId, OperationId, OperationMark, Reply<Transition>),
    /// An executor's report behind the fence (ADR 0028).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the M5 executor reports through it at the restack"
        )
    )]
    Report(
        GrantToken,
        Option<NodeId>,
        Observation,
        Reply<Result<Transition, FenceVerdict>>,
    ),
    /// The fence alone, for a covered write other than a journal operation.
    Fence(
        GrantToken,
        CoveredPath,
        Option<NodeId>,
        Reply<Result<(), FenceVerdict>>,
    ),
    /// The authority's acceptance of a checkpoint it fetched and verified.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the verifier commits through it once M5 reports Captured"
        )
    )]
    Acknowledge(JobId, CheckpointAck, Reply<Transition>),
    /// A heartbeat from an executor node: never a journal write by itself.
    Heartbeat(NodeId, Heartbeat, Reply<HeartbeatAnswer>),
    /// One tick of the authority's clock: overdue attempts become
    /// unavailable, and the recovery policy runs for each.
    Tick(u64, Reply<Vec<Overdue>>),
    Job(JobId, Reply<Option<ManagedState>>),
    List(Reply<Vec<ManagedState>>),
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
        /// The queue it takes calls from.
        calls: mpsc::Sender<Call>,
        /// The ids it publishes outside the queue.
        ids: watch::Receiver<Ids>,
    },
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
            Writer::Running { calls, ids } => return Ok((calls.clone(), ids.clone())),
            Writer::Idle(state, node) => Place {
                dir: managed_dir(state),
                jobs: state.jobs(),
                node: *node,
            },
        };
        let (calls, queue) = mpsc::channel(QUEUE);
        let (publish, ids) = watch::channel(Ids::Loading);
        std::thread::Builder::new()
            .name("fabric-authority".into())
            .spawn(move || serve(&place, queue, &publish))?;
        #[cfg(test)]
        self.spawned
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *writer = Writer::Running {
            calls: calls.clone(),
            ids: ids.clone(),
        };
        Ok((calls, ids))
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

    /// Records the authority's own observation of an attempt or an
    /// operation: no runner presents a grant, so no fence runs.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the executor wiring of M3 reports through it")
    )]
    pub(crate) async fn observe_authority(
        &self,
        job: JobId,
        observation: Observation,
    ) -> Result<Transition, AuthorityError> {
        self.call(|reply| Call::Observe(job, observation, reply))
            .await
    }

    /// Marks an operation on the authority's own behalf, `Dispatched`
    /// right before its effect is asked for.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the executor wiring of M3 dispatches through it")
    )]
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
    /// it acts under and `from` the peer it came from (ADR 0028).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the M5 executor reports through it at the restack"
        )
    )]
    pub(crate) async fn report(
        &self,
        token: GrantToken,
        from: Option<NodeId>,
        observation: Observation,
    ) -> Result<Result<Transition, FenceVerdict>, AuthorityError> {
        self.call(|reply| Call::Report(token, from, observation, reply))
            .await
    }

    /// Runs the fence for a covered write other than a journal operation;
    /// a refusal is recorded and returned (ADR 0028).
    pub(crate) async fn fence(
        &self,
        token: GrantToken,
        path: CoveredPath,
        from: Option<NodeId>,
    ) -> Result<Result<(), FenceVerdict>, AuthorityError> {
        self.call(|reply| Call::Fence(token, path, from, reply))
            .await
    }

    /// Commits a checkpoint the authority fetched and verified (ADR 0028).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the verifier commits through it once M5 reports Captured"
        )
    )]
    pub(crate) async fn acknowledge(
        &self,
        job: JobId,
        ack: CheckpointAck,
    ) -> Result<Transition, AuthorityError> {
        self.call(|reply| Call::Acknowledge(job, ack, reply)).await
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
                .attempts()
                .iter()
                .map(|t| (t.attempt, FenceVerdict::UnknownJob))
                .collect();
            return Ok(HeartbeatAnswer { verdicts });
        }
        self.call(|reply| Call::Heartbeat(from, beat, reply)).await
    }

    /// One tick of the authority's clock at `now_ms`: every tracked attempt
    /// silent for `UNAVAILABLE_AFTER_MS` becomes `unknown`, and the job's
    /// recovery policy runs for each unavailable attempt. Returns what
    /// became unavailable on this tick.
    pub(crate) async fn tick(&self, now_ms: u64) -> Result<Vec<Overdue>, AuthorityError> {
        if self.idle() {
            return Ok(Vec::new());
        }
        self.call(|reply| Call::Tick(now_ms, reply)).await
    }

    /// Ticks every `HEARTBEAT_INTERVAL_MS` until the authority is gone.
    /// Nothing runs while no thread does: no journal, no attempts.
    pub(crate) async fn run_ticker(self) {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match self.tick(now_ms()).await {
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
#[allow(
    clippy::too_many_lines,
    reason = "one arm per call reads better than a helper per arm"
)]
fn serve(place: &Place, mut queue: mpsc::Receiver<Call>, ids: &watch::Sender<Ids>) {
    let mut holder = open_existing(place, ids);
    let mut presence = Presence::default();
    if let Holder::Open(journal) = &holder {
        track_live(place, journal, &mut presence, now_ms());
    }
    while let Some(call) = queue.blocking_recv() {
        match call {
            Call::Submit(envelope, reply) => {
                let answer = submit(place, &mut holder, &envelope);
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
                        now_ms(),
                    );
                }
                let _ = reply.send(answer);
            }
            Call::Observe(job, observation, reply) => {
                let answer = open(&mut holder, job)
                    .and_then(|j| Ok(j.observe_authority(job, &observation, now_ms())?));
                if let Ok(t) = &answer {
                    note(place, &mut presence, job, &t.events, now_ms());
                }
                let _ = reply.send(answer);
            }
            Call::Mark(job, operation, mark, reply) => {
                let _ = reply.send(
                    open(&mut holder, job)
                        .and_then(|j| Ok(j.mark_authority(job, operation, mark, now_ms())?)),
                );
            }
            Call::Report(token, from, observation, reply) => {
                let answer = open(&mut holder, token.job)
                    .and_then(|j| Ok(j.observe(&token, from, &observation, now_ms())?));
                if let Ok(Ok(t)) = &answer {
                    note(place, &mut presence, token.job, &t.events, now_ms());
                }
                let _ = reply.send(answer);
            }
            Call::Fence(token, path, from, reply) => {
                let _ = reply.send(
                    open(&mut holder, token.job)
                        .and_then(|j| Ok(j.fence(&token, path, from, now_ms())?)),
                );
            }
            Call::Acknowledge(job, ack, reply) => {
                let _ = reply.send(
                    open(&mut holder, job).and_then(|j| Ok(j.acknowledge(job, &ack, now_ms())?)),
                );
            }
            Call::Heartbeat(from, beat, reply) => {
                let _ = reply.send(heartbeat(&mut holder, &mut presence, from, &beat));
            }
            Call::Tick(now, reply) => {
                let _ = reply.send(tick(place, &mut holder, &mut presence, now));
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
        }
    }
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
/// authority's start: each is overdue `UNAVAILABLE_AFTER_MS` from now
/// unless a heartbeat arrives first (ADR 0028).
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
            if node != place.node {
                presence.track(job, attempt.id, attempt.generation, node, now);
            }
        }
    }
}

/// Keeps the presence in step with what a committed transition did to the
/// job's attempts: a grant to a remote node is tracked from now; an
/// attempt that ended or lost its grant is forgotten.
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

/// Answers a heartbeat: the attempts it brings back from `unknown` are
/// observed `ResourceReachable`, then every token gets its fence verdict
/// from the projection. No other write.
fn heartbeat(
    holder: &mut Holder,
    presence: &mut Presence,
    from: NodeId,
    beat: &Heartbeat,
) -> Result<HeartbeatAnswer, AuthorityError> {
    let now = now_ms();
    let journal = match holder {
        Holder::Open(journal) => journal,
        Holder::Absent => {
            let verdicts = beat
                .attempts()
                .iter()
                .map(|t| (t.attempt, FenceVerdict::UnknownJob))
                .collect();
            return Ok(HeartbeatAnswer { verdicts });
        }
        Holder::Stopped(reason) => return Err(AuthorityError::Unavailable(reason.clone())),
    };
    for (job, attempt, generation) in presence.seen(from, beat, now) {
        let back = Observation::ResourceReachable {
            attempt,
            generation,
        };
        if let Err(e) = journal.observe_authority(job, &back, now) {
            tracing::warn!(%job, %attempt, error = %e, "a returned heartbeat was not recorded");
        }
    }
    let mut verdicts = Vec::with_capacity(beat.attempts().len());
    for token in beat.attempts() {
        let state = journal.job(token.job)?;
        let verdict = fence(state.as_ref(), token, Some(from));
        if verdict.allows() && !presence.tracks(token.attempt) {
            // A grant the start scan did not see, such as one made while
            // the journal was stopped: tracked from this heartbeat on.
            presence.track(token.job, token.attempt, token.generation, from, now);
            let _ = presence.seen(from, beat, now);
        }
        verdicts.push((token.attempt, verdict));
    }
    Ok(HeartbeatAnswer { verdicts })
}

/// One tick: every overdue attempt is observed `ResourceUnavailable`, and
/// the recovery policy of each unavailable attempt's job runs.
fn tick(
    place: &Place,
    holder: &mut Holder,
    presence: &mut Presence,
    now: u64,
) -> Result<Vec<Overdue>, AuthorityError> {
    let journal = match holder {
        Holder::Open(journal) => journal,
        Holder::Absent => return Ok(Vec::new()),
        Holder::Stopped(reason) => return Err(AuthorityError::Unavailable(reason.clone())),
    };
    let mut out = Vec::new();
    for o in presence.overdue(now) {
        let unavailable = Observation::ResourceUnavailable {
            attempt: o.attempt,
            generation: o.generation,
            last_heartbeat_ms: o.last_heartbeat_ms,
            at_ms: now,
        };
        match journal.observe_authority(o.job, &unavailable, now) {
            Ok(_) => {
                presence.mark_unavailable(o.attempt);
                out.push(o);
            }
            Err(CommitError::Domain(_)) => presence.forget(o.attempt),
            Err(e) => return Err(e.into()),
        }
    }
    for o in presence.unavailable() {
        recover(place, journal, presence, &o, now)?;
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

/// Runs the recovery policy for one unavailable attempt (ADR 0028): under
/// `Block` nothing; under `RestartFromAcknowledged` the pure decision,
/// then the `Recover` it names under its derived key, or the reason it
/// did not restart, recorded once per silence and reason. Placement is
/// this node until the M5 placement lands at the restack.
fn recover(
    place: &Place,
    journal: &mut Journal,
    presence: &mut Presence,
    o: &Overdue,
    now: u64,
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
    let outcome = OutcomePolicy::of_primary(&state.job.intent);
    let blockers: Vec<OperationId> = journal
        .replacement_blockers(o.job)?
        .iter()
        .map(Blocker::operation)
        .collect();
    let placement = Ok(ResourceRef::Node { node: place.node });
    let decided = recovery_decision(&state, o.attempt, outcome, &blockers, placement);
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
                at_ms: now,
            };
            match journal.submit(&envelope, ids)?.result {
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
                    tracing::warn!(job = %o.job, attempt = %o.attempt, refusal = %other, "the authority's recover was refused");
                    presence.forget(o.attempt);
                    return Ok(());
                }
            }
        }
        Err(reason) => reason,
    };
    if presence.block_changed(o.attempt, &reason) {
        journal.recovery_blocked(o.job, o.attempt, reason, now)?;
    }
    Ok(())
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
