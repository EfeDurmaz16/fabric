//! Where the bridge client records each provider operation: the managed
//! journal in a node, or an in-memory log in a test or a bench.
//!
//! Invariant: the client calls [`OperationLog::begin`] and gets `Ok`
//! before it writes a request frame, so an effect the worker may perform
//! is always in the log first; a key or id the log already holds is
//! [`LogError::Used`] and nothing is sent, so nothing is ever sent twice.
//!
//! The client reaches its log through a [`Recorder`], never by calling a
//! log from async code: [`LogThread`] runs a log that blocks, such as the
//! journal with its `SQLite` transactions and fsyncs, on a thread of its
//! own, and [`MemoryLog`], which only touches memory, records in place. A
//! call is queued whole when it is made, after room for it was found, so
//! a caller that stops waiting either queued nothing or queued a call
//! that runs to its end.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::managed::{OperationId, OperationKey};
use fabric_journal::{
    CommitError, Journal, ProviderAnswer, ProviderDispatch, ProviderError, ProviderStatus,
};
use tokio::sync::{mpsc, oneshot};

use crate::wire::Scope;

/// Why the log refused a change. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    /// The key or the id is already in the log: never send it again.
    #[error("already in the journal: {0}")]
    Used(String),
    /// The attempt is not the job's current live grant, or the job is closed.
    #[error("fenced: {0}")]
    Fenced(String),
    /// The log has no such operation, or it cannot move to that status.
    #[error("{0}")]
    Transition(String),
    /// The log failed.
    #[error("journal: {0}")]
    Storage(String),
}

/// A provider operation's scope, action, target, status and last locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The job, task, attempt and generation it was dispatched for.
    pub scope: Scope,
    /// The bridge action it was dispatched as.
    pub action: String,
    /// The locator it was sent to, as JSON text; `None` for a create.
    pub target: Option<String>,
    /// Where it is.
    pub status: ProviderStatus,
    /// The last locator the worker reported, as JSON text.
    pub locator: Option<String>,
}

/// What the bridge client writes to.
pub trait OperationLog {
    /// Records the operation as `dispatched`, or as `rejected` with
    /// `refusal` when Fabric refused it before sending.
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError>;

    /// Records an answer: the new status, why, and the locator it named.
    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError>;

    /// Records a locator reported before the answer; the status stays.
    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError>;

    /// The operation's status and locator, or `None`.
    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError>;
}

/// How the bridge client reaches an [`OperationLog`] from async code
/// without running it there: [`LogThread`] for a log that blocks, such as
/// the journal, and [`MemoryLog`] for one in memory.
pub trait Recorder: Send {
    /// The log the calls run against.
    type Log: OperationLog + Send + 'static;

    /// Waits for room for one call. Nothing is queued yet, so a caller
    /// that stops waiting here changes nothing.
    fn reserve(&mut self) -> impl Future<Output = Result<Slot<'_, Self::Log>, LogError>> + Send;
}

/// Room for one call on a log, from [`Recorder::reserve`].
#[derive(Debug)]
pub struct Slot<'a, L>(Place<'a, L>);

enum Place<'a, L> {
    /// The log itself: the call runs in place.
    Here(&'a mut L),
    /// A place in a log thread's queue.
    Queue(mpsc::Permit<'a, Message<L>>),
}

impl<L> std::fmt::Debug for Place<'_, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Place::Here(_) => "Here",
            Place::Queue(_) => "Queue",
        })
    }
}

impl<L: OperationLog + Send + 'static> Slot<'_, L> {
    /// Queues `call` now, whole, and returns what waits for its result. A
    /// caller that drops the wait leaves the call queued: it runs to its
    /// end. A log thread that stopped before it ran the call is
    /// [`LogError::Storage`].
    pub fn run<T, F>(self, call: F) -> impl Future<Output = Result<T, LogError>> + Send + 'static
    where
        T: Send + 'static,
        F: FnOnce(&mut L) -> T + Send + 'static,
    {
        let queued = match self.0 {
            Place::Here(log) => Ok(call(log)),
            Place::Queue(permit) => {
                let (reply, result) = oneshot::channel();
                permit.send(Message::Run(Box::new(move |log| {
                    // A caller that stopped waiting has nothing to hear.
                    let _ = reply.send(call(log));
                })));
                Err(result)
            }
        };
        async move {
            match queued {
                Ok(value) => Ok(value),
                Err(result) => result.await.map_err(|_| stopped()),
            }
        }
    }
}

fn stopped() -> LogError {
    LogError::Storage("the log thread stopped".into())
}

type Job<L> = Box<dyn FnOnce(&mut L) + Send>;

/// What a log thread is asked.
enum Message<L> {
    Run(Job<L>),
    Stop(oneshot::Sender<L>),
}

/// How many calls may wait for a log thread before a caller waits for
/// room. One client has one call in flight at a time; the room is for
/// calls whose callers stopped waiting, which still run.
const QUEUE: usize = 16;

/// An [`OperationLog`] on a thread of its own, so its blocking calls,
/// the journal's `SQLite` transactions and fsyncs, never run on the async
/// runtime. Calls run one at a time in the order they were queued; each
/// is one hop to the thread and one back. The thread ends when the
/// handle is dropped, closing the log there.
#[derive(Debug)]
pub struct LogThread<L> {
    calls: mpsc::Sender<Message<L>>,
    served: Arc<AtomicU64>,
}

impl<L: OperationLog + Send + 'static> LogThread<L> {
    /// Starts the thread that owns `log`.
    pub fn spawn(log: L) -> std::io::Result<LogThread<L>> {
        let (calls, mut queue) = mpsc::channel::<Message<L>>(QUEUE);
        let served = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&served);
        std::thread::Builder::new()
            .name("fabric-bridge-log".into())
            .spawn(move || {
                let mut log = log;
                while let Some(message) = queue.blocking_recv() {
                    match message {
                        Message::Run(job) => {
                            job(&mut log);
                            count.fetch_add(1, Ordering::Relaxed);
                        }
                        Message::Stop(back) => {
                            let _ = back.send(log);
                            return;
                        }
                    }
                }
            })?;
        Ok(LogThread { calls, served })
    }

    /// Runs `call` on the log's thread and returns its result.
    pub async fn call<T, F>(&self, call: F) -> Result<T, LogError>
    where
        T: Send + 'static,
        F: FnOnce(&mut L) -> T + Send + 'static,
    {
        let permit = self.calls.reserve().await.map_err(|_| stopped())?;
        Slot(Place::Queue(permit)).run(call).await
    }

    /// How many calls the thread has run: each is two thread hops.
    #[must_use]
    pub fn calls(&self) -> u64 {
        self.served.load(Ordering::Relaxed)
    }

    /// Stops the thread once the calls queued before this one ran, and
    /// hands the log back.
    pub async fn into_inner(self) -> Result<L, LogError> {
        let (back, log) = oneshot::channel();
        self.calls
            .send(Message::Stop(back))
            .await
            .map_err(|_| stopped())?;
        log.await.map_err(|_| stopped())
    }
}

impl<L: OperationLog + Send + 'static> Recorder for LogThread<L> {
    type Log = L;

    async fn reserve(&mut self) -> Result<Slot<'_, L>, LogError> {
        let permit = self.calls.reserve().await.map_err(|_| stopped())?;
        Ok(Slot(Place::Queue(permit)))
    }
}

/// The scope a dispatch was recorded for.
fn scope_of(dispatch: &ProviderDispatch) -> Scope {
    Scope {
        job: dispatch.job,
        task: dispatch.task,
        attempt: dispatch.attempt,
        generation: dispatch.generation,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl From<CommitError> for LogError {
    fn from(e: CommitError) -> Self {
        match e {
            CommitError::Provider(
                ProviderError::KeyUsed { .. } | ProviderError::OperationUsed { .. },
            ) => LogError::Used(e.to_string()),
            CommitError::Provider(_) => LogError::Transition(e.to_string()),
            CommitError::Domain(_) => LogError::Fenced(e.to_string()),
            other => LogError::Storage(other.to_string()),
        }
    }
}

/// The managed journal. Each call that writes is one `IMMEDIATE`
/// transaction, committed (and synced) before it returns, so it blocks:
/// hand the journal to a [`LogThread`], never to async code directly.
impl OperationLog for Journal {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        Ok(self.provider_begin(dispatch, refusal, now_ms())?)
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError> {
        Ok(self.provider_record(operation, status, answer, locator, now_ms())?)
    }

    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError> {
        Ok(self.provider_locate(operation, locator, now_ms())?)
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        Ok(self.provider_operation(operation)?.map(|op| Entry {
            scope: scope_of(&op.dispatch),
            action: op.dispatch.action,
            target: op.dispatch.target,
            status: op.status,
            locator: op.locator,
        }))
    }
}

/// An in-memory log with the journal's rules but no fencing and no
/// durability: for tests and the bench, never for a node. It touches only
/// memory, so it records in place.
#[derive(Debug, Default)]
pub struct MemoryLog {
    entries: HashMap<OperationId, Entry>,
    keys: HashSet<OperationKey>,
}

impl MemoryLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> MemoryLog {
        MemoryLog::default()
    }
}

impl Recorder for MemoryLog {
    type Log = MemoryLog;

    fn reserve(&mut self) -> impl Future<Output = Result<Slot<'_, MemoryLog>, LogError>> + Send {
        std::future::ready(Ok(Slot(Place::Here(self))))
    }
}

impl OperationLog for MemoryLog {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        if self.keys.contains(&dispatch.key) || self.entries.contains_key(&dispatch.operation) {
            return Err(LogError::Used(format!(
                "{} or {}",
                dispatch.key, dispatch.operation
            )));
        }
        self.keys.insert(dispatch.key.clone());
        self.entries.insert(
            dispatch.operation,
            Entry {
                scope: scope_of(dispatch),
                action: dispatch.action.clone(),
                target: dispatch.target.clone(),
                status: if refusal.is_some() {
                    ProviderStatus::Rejected
                } else {
                    ProviderStatus::Dispatched
                },
                locator: None,
            },
        );
        Ok(())
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        _answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        if entry.status.is_terminal()
            || (entry.status != status && !entry.status.may_become(status))
        {
            return Err(LogError::Transition(format!(
                "{operation} cannot go from {} to {}",
                entry.status.name(),
                status.name()
            )));
        }
        entry.status = status;
        if let Some(locator) = locator {
            entry.locator = Some(locator.to_owned());
        }
        Ok(())
    }

    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        if !entry.status.is_terminal() {
            entry.locator = Some(locator.to_owned());
        }
        Ok(())
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        Ok(self.entries.get(&operation).cloned())
    }
}
