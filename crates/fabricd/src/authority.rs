//! The managed authority (ADR 0020): the one writer of the managed
//! journal, `<state>/managed/journal.sqlite`, inside the daemon.
//!
//! Invariant: one thread owns the journal and applies calls one at a time
//! from a bounded queue, so commands are serialized without a lock and no
//! other code in the daemon opens the file. The journal is made by the
//! first `CreateJob`, never by a start; a state directory without one
//! behaves as before managed jobs existed and the start pays one `stat`.
//! When a journal exists, the thread opens it, runs `verify` and restart
//! recovery before it answers any call, off the daemon's start path: a
//! dispatched operation becomes unknown and nothing is replayed or
//! granted. A journal that fails `verify` stops the authority, never the
//! daemon: every managed call is refused with the reason and standalone
//! jobs run as before. A managed job never takes an id a standalone job
//! has, and a managed command never acts on a standalone job.

use std::fs;
use std::path::PathBuf;

use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::{
    AttemptId, Command, DomainError, Envelope, ManagedState, NewIds, Observation, OperationId,
    OperationMark, TaskId, Transition,
};
use fabric_journal::{Answer, CommitError, FILE_NAME, Journal, Liveness, OpenError};
use tokio::sync::{mpsc, oneshot};
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
    Job(JobId, Reply<Option<ManagedState>>),
    List(Reply<Vec<ManagedState>>),
}

/// The handle the daemon's tasks call the authority through.
#[derive(Debug, Clone)]
pub(crate) struct Authority {
    calls: mpsc::Sender<Call>,
}

impl Authority {
    /// Starts the authority's thread for `state`, on node `node`. Returns
    /// at once; recovery of an existing journal runs on the thread.
    pub(crate) fn start(state: &StateDir, node: NodeId) -> std::io::Result<Authority> {
        let (calls, queue) = mpsc::channel(QUEUE);
        let place = Place {
            dir: managed_dir(state),
            jobs: state.jobs(),
            node,
        };
        std::thread::Builder::new()
            .name("fabric-authority".into())
            .spawn(move || serve(&place, queue))?;
        Ok(Authority { calls })
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Call) -> Result<T, AuthorityError> {
        let (reply, answer) = oneshot::channel();
        self.calls
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

    /// Records what an executor reports about an attempt or an operation.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the executor wiring of M3 reports through it")
    )]
    pub(crate) async fn observe(
        &self,
        job: JobId,
        observation: Observation,
    ) -> Result<Transition, AuthorityError> {
        self.call(|reply| Call::Observe(job, observation, reply))
            .await
    }

    /// Marks an operation, `Dispatched` right before its effect is asked for.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the executor wiring of M3 dispatches through it")
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
        self.call(|reply| Call::Job(job, reply)).await
    }

    /// Every managed job, oldest first.
    pub(crate) async fn list(&self) -> Result<Vec<ManagedState>, AuthorityError> {
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
fn open_existing(place: &Place) -> Holder {
    let path = place.journal();
    if !path.exists() {
        return Holder::Absent;
    }
    let started = std::time::Instant::now();
    let mut journal = match Journal::open(&path) {
        Ok(journal) => journal,
        Err(e) => return stopped(&e),
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
fn serve(place: &Place, mut queue: mpsc::Receiver<Call>) {
    let mut holder = open_existing(place);
    while let Some(call) = queue.blocking_recv() {
        match call {
            Call::Submit(envelope, reply) => {
                let _ = reply.send(submit(place, &mut holder, &envelope));
            }
            Call::Observe(job, observation, reply) => {
                let _ = reply.send(
                    open(&mut holder, job)
                        .and_then(|j| Ok(j.observe(job, &observation, now_ms())?)),
                );
            }
            Call::Mark(job, operation, mark, reply) => {
                let _ = reply.send(
                    open(&mut holder, job)
                        .and_then(|j| Ok(j.mark(job, operation, mark, now_ms())?)),
                );
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
