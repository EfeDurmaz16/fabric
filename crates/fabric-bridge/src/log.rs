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
//! own, [`MemoryLog`], which only touches memory, records in place, and a
//! [`Hand`] reaches a log another component already owns on a thread of
//! its own, such as the node's managed authority, one call per turn. A
//! call is queued whole when it is made, after room for it was found, so
//! a caller that stops waiting either queued nothing or queued a call
//! that runs to its end.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::managed::{OperationId, OperationKey};
use fabric_journal::{
    CleanupRefusal, CommitError, EffectClass, Journal, LocatorRecord, ProviderAnswer,
    ProviderDispatch, ProviderError, ProviderStatus, Purpose,
};
use tokio::sync::{mpsc, oneshot};

use crate::wire::Scope;

/// Why the log refused a change. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    /// The key or the id is already in the log: never send it again.
    #[error("already in the journal: {0}")]
    Used(String),
    /// The attempt is not the job's current live grant, or the job is
    /// closed; a create of the job is unknown; or a cleanup was refused.
    #[error("fenced: {0}")]
    Fenced(String),
    /// The log has no such operation, or it cannot move to that status.
    #[error("{0}")]
    Transition(String),
    /// The log failed.
    #[error("journal: {0}")]
    Storage(String),
}

/// A provider operation as a read or a cleanup needs it: its identity,
/// what it was dispatched as, its status and the last locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The job, task, attempt and generation it was dispatched for.
    pub scope: Scope,
    /// Its key.
    pub key: OperationKey,
    /// The profile it was dispatched under.
    pub profile: String,
    /// The bridge action it was dispatched as.
    pub action: String,
    /// The action's effect in that profile.
    pub effect: EffectClass,
    /// Why it was dispatched.
    pub purpose: Purpose,
    /// The locator it was sent to; `None` for a create.
    pub target: Option<LocatorRecord>,
    /// Where it is.
    pub status: ProviderStatus,
    /// The last locator the worker reported.
    pub locator: Option<LocatorRecord>,
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
        locator: Option<&LocatorRecord>,
    ) -> Result<(), LogError>;

    /// Records an `absent` answer to an attach of an `unknown` operation
    /// and the scope it searched; the status stays `unknown`.
    fn searched(
        &mut self,
        operation: OperationId,
        answer: &ProviderAnswer,
        searched: &str,
    ) -> Result<(), LogError>;

    /// Records an `absent` answer to an observe of a destroy's resource
    /// and the scope it searched: the destroy settles `succeeded`.
    /// Anything but a destroy that is not terminal is a `Transition`.
    fn absent(
        &mut self,
        operation: OperationId,
        answer: &ProviderAnswer,
        searched: &str,
    ) -> Result<(), LogError>;

    /// Records a locator reported before the answer; the status stays.
    fn locate(&mut self, operation: OperationId, locator: &LocatorRecord) -> Result<(), LogError>;

    /// The operation as the log holds it, or `None`.
    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError>;

    /// The scope and the locator a cleanup of the create `creating` acts
    /// on, or why no cleanup of it may be dispatched now (the journal's
    /// rules, read before anything is written; `provider_begin` checks
    /// them again in its transaction).
    fn cleanup_target(
        &self,
        creating: OperationId,
    ) -> Result<Result<(Scope, LocatorRecord), CleanupRefusal>, LogError>;
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
    /// Room on the queue of a thread something else owns the log on.
    Hand(Box<dyn Hand<L> + 'a>),
}

impl<L> std::fmt::Debug for Place<'_, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Place::Here(_) => "Here",
            Place::Queue(_) => "Queue",
            Place::Hand(_) => "Hand",
        })
    }
}

/// A call handed to the thread that owns a log, given the log or why it
/// cannot be reached.
pub type Handed<L> = Box<dyn FnOnce(Result<&mut L, LogError>) + Send>;

/// Room for one call on the queue of a thread that owns a log and serves
/// other work too: the node's managed authority, the one writer of the
/// journal (ADR 0020). The room is taken before the slot is made, so
/// [`Hand::hand`] queues the call whole and never waits. Each call is one
/// turn of that thread: between two calls, while the worker acts, the
/// thread serves its other work.
pub trait Hand<L>: Send {
    /// Queues `call`; the owner runs it once, with the log or the reason
    /// it has none. An owner that stopped drops it unrun.
    fn hand(self: Box<Self>, call: Handed<L>);
}

impl<'a, L> Slot<'a, L> {
    /// A slot on the queue `hand` holds room on, for a [`Recorder`] whose
    /// log another component owns on its own thread.
    #[must_use]
    pub fn handed(hand: Box<dyn Hand<L> + 'a>) -> Slot<'a, L> {
        Slot(Place::Hand(hand))
    }
}

/// What a slot's call waits on.
enum Waiting<T> {
    Ready(T),
    Queue(oneshot::Receiver<T>),
    Hand(oneshot::Receiver<Result<T, LogError>>),
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
            Place::Here(log) => Waiting::Ready(call(log)),
            Place::Queue(permit) => {
                let (reply, result) = oneshot::channel();
                permit.send(Message::Run(Box::new(move |log, served| {
                    let value = call(log);
                    // Counted before the caller can hear the result, so
                    // a count read once a call returned takes it in.
                    served.fetch_add(1, Ordering::Relaxed);
                    // A caller that stopped waiting has nothing to hear.
                    let _ = reply.send(value);
                })));
                Waiting::Queue(result)
            }
            Place::Hand(hand) => {
                let (reply, result) = oneshot::channel();
                hand.hand(Box::new(move |log| {
                    // A caller that stopped waiting has nothing to hear.
                    let _ = reply.send(log.map(call));
                }));
                Waiting::Hand(result)
            }
        };
        async move {
            match queued {
                Waiting::Ready(value) => Ok(value),
                Waiting::Queue(result) => result.await.map_err(|_| stopped()),
                Waiting::Hand(result) => result.await.map_err(|_| stopped())?,
            }
        }
    }
}

fn stopped() -> LogError {
    LogError::Storage("the log thread stopped".into())
}

/// A queued call, given the log and the count of calls run, which it
/// adds itself to before it answers its caller.
type Job<L> = Box<dyn FnOnce(&mut L, &AtomicU64) + Send>;

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
                        Message::Run(job) => job(&mut log, &count),
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

    /// How many calls the thread has run: each is two thread hops. A call
    /// is counted before its caller can hear its result, so a count read
    /// once a call returned takes that call in.
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

/// A log already on a thread of its own that blocks, reached in place.
///
/// The managed authority owns the journal on its thread and is the one
/// writer of it (ADR 0020); a cleanup it runs there awaits the bridge with
/// the runtime's handle and records through this, so the journal never
/// gets a second writer and never runs on the async runtime. Not for a
/// log used from async code: that is what [`LogThread`] is for.
#[derive(Debug)]
pub struct InPlace<'a, L>(pub &'a mut L);

impl<L: OperationLog + Send + 'static> Recorder for InPlace<'_, L> {
    type Log = L;

    fn reserve(&mut self) -> impl Future<Output = Result<Slot<'_, L>, LogError>> + Send {
        std::future::ready(Ok(Slot(Place::Here(&mut *self.0))))
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
            CommitError::Domain(_)
            | CommitError::Provider(
                ProviderError::CreateUnresolved { .. } | ProviderError::Cleanup { .. },
            ) => LogError::Fenced(e.to_string()),
            CommitError::Provider(_) => LogError::Transition(e.to_string()),
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
        locator: Option<&LocatorRecord>,
    ) -> Result<(), LogError> {
        Ok(self.provider_record(operation, status, answer, locator, now_ms())?)
    }

    fn searched(
        &mut self,
        operation: OperationId,
        answer: &ProviderAnswer,
        searched: &str,
    ) -> Result<(), LogError> {
        Ok(self.provider_searched(operation, answer, searched, now_ms())?)
    }

    fn absent(
        &mut self,
        operation: OperationId,
        answer: &ProviderAnswer,
        searched: &str,
    ) -> Result<(), LogError> {
        Ok(self.provider_absent(operation, answer, searched, now_ms())?)
    }

    fn locate(&mut self, operation: OperationId, locator: &LocatorRecord) -> Result<(), LogError> {
        Ok(self.provider_locate(operation, locator, now_ms())?)
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        Ok(self.provider_operation(operation)?.map(|op| Entry {
            scope: scope_of(&op.dispatch),
            key: op.dispatch.key,
            profile: op.dispatch.profile,
            action: op.dispatch.action,
            effect: op.dispatch.effect,
            purpose: op.dispatch.purpose,
            target: op.dispatch.target,
            status: op.status,
            locator: op.locator,
        }))
    }

    fn cleanup_target(
        &self,
        creating: OperationId,
    ) -> Result<Result<(Scope, LocatorRecord), CleanupRefusal>, LogError> {
        let Some(create) = self.provider_operation(creating)? else {
            return Ok(Err(CleanupRefusal::NotACreate));
        };
        let destroys = match &create.locator {
            Some(locator) => self.provider_destroys(locator.key)?,
            None => Vec::new(),
        };
        Ok(cleanup_rule(&entry_of(create), &destroys))
    }
}

fn entry_of(op: fabric_journal::ProviderOperation) -> Entry {
    Entry {
        scope: scope_of(&op.dispatch),
        key: op.dispatch.key,
        profile: op.dispatch.profile,
        action: op.dispatch.action,
        effect: op.dispatch.effect,
        purpose: op.dispatch.purpose,
        target: op.dispatch.target,
        status: op.status,
        locator: op.locator,
    }
}

/// The cleanup rules of docs/managed-jobs.md (step 3) over a create's
/// entry and the destroys of its resource, by operation and status.
fn cleanup_rule(
    create: &Entry,
    destroys: &[(OperationId, ProviderStatus)],
) -> Result<(Scope, LocatorRecord), CleanupRefusal> {
    if create.effect != EffectClass::Create {
        return Err(CleanupRefusal::NotACreate);
    }
    let Some(locator) = &create.locator else {
        return Err(CleanupRefusal::Unlocated);
    };
    if create.status == ProviderStatus::Dispatched {
        return Err(CleanupRefusal::StillDispatched);
    }
    if destroys
        .iter()
        .any(|(_, s)| *s == ProviderStatus::Succeeded)
    {
        return Err(CleanupRefusal::AlreadyDestroyed);
    }
    if let Some((operation, _)) = destroys.iter().find(|(_, s)| {
        matches!(
            s,
            ProviderStatus::Dispatched | ProviderStatus::Unknown | ProviderStatus::Observed
        )
    }) {
        return Err(CleanupRefusal::InFlight {
            operation: *operation,
        });
    }
    Ok((create.scope, locator.clone()))
}

/// An in-memory log with the journal's key, id and status rules but no
/// fencing, no create guard, no cleanup checks at `begin` and no
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
                key: dispatch.key.clone(),
                profile: dispatch.profile.clone(),
                action: dispatch.action.clone(),
                effect: dispatch.effect,
                purpose: dispatch.purpose,
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
        locator: Option<&LocatorRecord>,
    ) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        let unnamed = matches!(status, ProviderStatus::Attached | ProviderStatus::Observed)
            && entry.locator.is_none()
            && entry.target.is_none()
            && locator.is_none();
        if entry.status.is_terminal()
            || (entry.status != status && !entry.status.may_become(status))
            || unnamed
        {
            return Err(LogError::Transition(format!(
                "{operation} cannot go from {} to {}",
                entry.status.name(),
                status.name()
            )));
        }
        entry.status = status;
        if let Some(locator) = locator {
            entry.locator = Some(locator.clone());
        }
        Ok(())
    }

    fn searched(
        &mut self,
        operation: OperationId,
        _answer: &ProviderAnswer,
        _searched: &str,
    ) -> Result<(), LogError> {
        match self.entries.get(&operation) {
            Some(entry) if entry.status == ProviderStatus::Unknown => Ok(()),
            Some(entry) => Err(LogError::Transition(format!(
                "{operation} is {}, not unknown",
                entry.status.name()
            ))),
            None => Err(LogError::Transition(format!(
                "no provider operation {operation}"
            ))),
        }
    }

    fn absent(
        &mut self,
        operation: OperationId,
        _answer: &ProviderAnswer,
        _searched: &str,
    ) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        if entry.effect != EffectClass::Destroy || entry.status.is_terminal() {
            return Err(LogError::Transition(format!(
                "{operation} is {} {}, which absence does not settle",
                entry.status.name(),
                entry.action
            )));
        }
        entry.status = ProviderStatus::Succeeded;
        Ok(())
    }

    fn locate(&mut self, operation: OperationId, locator: &LocatorRecord) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        if !entry.status.is_terminal() {
            entry.locator = Some(locator.clone());
        }
        Ok(())
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        Ok(self.entries.get(&operation).cloned())
    }

    fn cleanup_target(
        &self,
        creating: OperationId,
    ) -> Result<Result<(Scope, LocatorRecord), CleanupRefusal>, LogError> {
        let Some(create) = self.entries.get(&creating) else {
            return Ok(Err(CleanupRefusal::NotACreate));
        };
        let key = create.locator.as_ref().map(|l| l.key);
        let destroys: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| {
                e.effect == EffectClass::Destroy
                    && key.is_some()
                    && e.target.as_ref().map(|t| t.key) == key
            })
            .map(|(id, e)| (*id, e.status))
            .collect();
        Ok(cleanup_rule(create, &destroys))
    }
}
