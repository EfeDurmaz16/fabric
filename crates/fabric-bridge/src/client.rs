//! The bridge client: runs one provider worker over stdio, negotiates the
//! handshake, and sends effects that the operation log recorded first.
//!
//! Invariants, each pinned by a test:
//! - An effect is recorded `dispatched` before its frame is written, and
//!   a key or id the log holds is never sent again.
//! - A request Fabric can tell is outside the negotiated profile, over a
//!   limit, or too large is recorded `rejected` and never sent: refusal
//!   before effect.
//! - A deadline that passes, a worker that exits, closes stdout or breaks
//!   the protocol makes each outstanding operation `unknown`: never
//!   cancelled, never failed, never sent again. Only `observe` by the
//!   original operation moves it on.
//! - Every frame the worker sends is read under the negotiated cap,
//!   checked before the body is read; the handshake answer under 1 MiB.
//! - What waits for a caller is bounded: at most 16 `progress` frames per
//!   request, `seq` from 1 rising by 1, then its one answer. Any other
//!   `progress` breaks the protocol and ends the worker.
//! - Frames are written whole by one writer task, in order, each by its
//!   deadline. A caller that stops waiting cannot leave part of a frame
//!   behind, and a worker that stops reading its input is lost and
//!   terminated at the deadline instead of holding the bridge forever.

use std::collections::{HashMap, VecDeque};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fabric_core::managed::{OperationId, RequestDigest};
use fabric_journal::{ProviderAnswer, ProviderDispatch, ProviderStatus};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;

use crate::frames::{self, FrameFault};
use crate::log::{Entry, LogError, OperationLog};
use crate::wire::{
    Action, Cancel, CancelState, ErrorCode, ErrorMessage, Expect, Hello, Limits, Locator,
    MAX_PROGRESS, Observe, OperationRef, Phase, Profile, Refusal, Request, RequestPolicy,
    ResultMessage, ResultState, Scope, ToFabric, ToWorker, Unresolved, Versions,
};

/// How Fabric runs a worker.
#[derive(Debug, Clone)]
pub struct Config {
    /// Fabric's name and version, sent in `hello`.
    pub fabric: String,
    /// The capability profile Fabric asks for.
    pub profile: String,
    /// The protocol majors Fabric speaks.
    pub versions: Versions,
    /// Fabric's limits; the worker may only lower them.
    pub limits: Limits,
    /// How long the worker has to answer `hello` (10 s by the protocol).
    pub handshake_timeout: Duration,
    /// How long past a request's own deadline Fabric waits before it
    /// records `unknown`.
    pub grace: Duration,
    /// How long a worker has between SIGTERM and SIGKILL.
    pub kill_grace: Duration,
}

impl Config {
    /// Protocol v1 defaults for `profile`.
    #[must_use]
    pub fn new(profile: &str) -> Config {
        Config {
            fabric: concat!("fabric/", env!("CARGO_PKG_VERSION")).to_owned(),
            profile: profile.to_owned(),
            versions: Versions::V1,
            limits: Limits::DEFAULT,
            handshake_timeout: Duration::from_secs(10),
            grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(2),
        }
    }

    fn hello(&self) -> Hello {
        Hello {
            versions: self.versions,
            fabric: self.fabric.clone(),
            profile: self.profile.clone(),
            limits: self.limits,
        }
    }
}

/// Why the bridge could not do what was asked.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The worker could not be started.
    #[error("starting the worker: {0}")]
    Spawn(std::io::Error),
    /// A frame from the worker was refused; the worker is gone.
    #[error("worker frame refused: {0}")]
    Frame(FrameFault),
    /// The worker broke the protocol; it is gone.
    #[error("worker broke the protocol: {0}")]
    Violation(Refusal),
    /// The worker refused the handshake.
    #[error("worker refused the handshake: {code} ({message}); it speaks {}..={}", versions.min, versions.max)]
    Refused {
        /// The worker's range.
        versions: Versions,
        /// Why.
        code: String,
        /// Its diagnostic.
        message: String,
    },
    /// The worker did not answer `hello` in time.
    #[error("the worker did not answer the handshake in time")]
    HandshakeTimeout,
    /// The worker is gone: exited, closed its output or was killed.
    #[error("the worker is gone: {0}")]
    Lost(String),
    /// Fabric refused the request before sending it, and recorded it `rejected`.
    #[error("refused before sending: {0}")]
    RefusedLocally(Refusal),
    /// The log refused; nothing was sent.
    #[error(transparent)]
    Log(#[from] LogError),
    /// As many requests as the worker takes are outstanding; nothing was
    /// recorded or sent, and the operation may be sent later.
    #[error("the worker has {0} requests outstanding, its limit")]
    Busy(u64),
    /// The bridge is shutting down; nothing was recorded or sent.
    #[error("the bridge is shutting down")]
    ShuttingDown,
    /// The operation is not one the log holds as `dispatched` or `unknown`.
    #[error("operation {0} has nothing to observe")]
    NotObservable(OperationId),
    /// The observe names another action or another resource than the
    /// operation's own in the log, or the log holds no locator to observe
    /// it by. Nothing was sent and the log is unchanged.
    #[error("operation {operation} is not observed so: {why}")]
    ObserveMismatch {
        /// The operation.
        operation: OperationId,
        /// What does not match.
        why: String,
    },
}

/// What happened to a provider operation, and the status the log now holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The status the log holds after this answer.
    pub status: ProviderStatus,
    /// What came back.
    pub answer: Answer,
}

/// What came back for one request or observe.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// A `result`.
    Result(ResultMessage),
    /// An `error`.
    Error(ErrorMessage),
    /// Fabric's own deadline passed with no answer.
    TimedOut,
    /// The worker went away before answering.
    Lost(String),
}

/// One step of an outstanding request.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "one answer per request, moved once to its caller; its size depends on serde_json's features across the workspace, so the lint fires in some builds only"
)]
pub enum Event {
    /// The worker reported the locator before the answer; it is recorded.
    Progress(Locator),
    /// The answer, recorded.
    Done(Outcome),
}

#[derive(Debug)]
enum Incoming {
    Progress(Locator),
    Answer(Box<ToFabric>),
    Lost(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Request,
    Observe,
    Control,
}

#[derive(Debug)]
struct Waiter {
    operation: Option<OperationId>,
    action: Option<String>,
    terminal_only: bool,
    kind: Kind,
    /// The `seq` of the last `progress` it got; 0 before the first.
    progress: u64,
    tx: mpsc::Sender<Incoming>,
}

/// Room for every frame one waiter can get: its `progress` frames, then
/// its answer or the worker's loss.
const QUEUE: usize = 17;
const _: () = assert!(QUEUE as u64 == MAX_PROGRESS + 1);

#[derive(Debug, Default)]
struct State {
    waiters: HashMap<String, Waiter>,
    /// Requests Fabric stopped waiting on, oldest first, at most
    /// `EXPIRED`: a late answer to one still names its resource.
    expired: VecDeque<Expired>,
    /// The locators late answers named, by operation, not yet in the log.
    found: HashMap<OperationId, Locator>,
    lost: Option<String>,
    shutting_down: bool,
    late: u64,
}

/// A request whose deadline passed, or whose caller dropped it, unanswered.
#[derive(Debug)]
struct Expired {
    request_id: String,
    operation: OperationId,
    /// The `seq` of the last `progress` it got.
    progress: u64,
}

/// How many unanswered requests Fabric remembers past their deadline.
const EXPIRED: usize = 1024;

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops waiting on `id`. A request is remembered, so that the
    /// locator its late answer names is still recorded.
    fn expire(&self, id: &str) {
        let mut state = self.lock();
        let Some(waiter) = state.waiters.remove(id) else {
            return;
        };
        let (Kind::Request, Some(operation)) = (waiter.kind, waiter.operation) else {
            return;
        };
        if state.expired.len() == EXPIRED {
            state.expired.pop_front();
        }
        state.expired.push_back(Expired {
            request_id: id.to_owned(),
            operation,
            progress: waiter.progress,
        });
    }

    /// Records in `log` each locator a late answer named. One for an
    /// operation `log` does not hold is dropped; when the log fails, what
    /// is left is kept for the next call.
    fn record_found<L: OperationLog>(&self, log: &mut L) -> Result<(), LogError> {
        let found = std::mem::take(&mut self.lock().found);
        let mut rest = found.into_iter();
        while let Some((operation, locator)) = rest.next() {
            match log.locate(operation, &locator_text(&locator)) {
                Ok(()) | Err(LogError::Transition(_)) => {}
                Err(e) => {
                    let mut state = self.lock();
                    for (operation, locator) in std::iter::once((operation, locator)).chain(rest) {
                        state.found.entry(operation).or_insert(locator);
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Marks the worker gone and tells every waiter, once.
    fn lose(&self, why: &str) {
        let mut state = self.lock();
        if state.lost.is_none() {
            state.lost = Some(why.to_owned());
        }
        for (_, waiter) in state.waiters.drain() {
            let _ = waiter.tx.try_send(Incoming::Lost(why.to_owned()));
        }
    }
}

type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// One frame for the writer task, and the instant by which the worker
/// must have read it.
struct Write {
    frame: Vec<u8>,
    deadline: Instant,
}

/// The worker process, when the bridge runs one.
struct Process {
    child: tokio::sync::Mutex<Child>,
    pid: Option<i32>,
}

impl Process {
    /// SIGTERM, then SIGKILL after `grace` unless it exited.
    async fn terminate(&self, grace: Duration) {
        let mut child = self.child.lock().await;
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Some(pid) = self.pid.and_then(rustix::process::Pid::from_raw) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        if tokio::time::timeout(grace, child.wait()).await.is_err() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

/// A connected bridge to one worker.
pub struct Bridge {
    /// The writer task's queue; `None` once `close` closed it.
    writes: Option<mpsc::Sender<Write>>,
    writer: JoinHandle<()>,
    shared: Arc<Shared>,
    reader: JoinHandle<()>,
    process: Option<Arc<Process>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    protocol: u64,
    worker: String,
    profile: Arc<Profile>,
    limits: Limits,
    config: Config,
    next_id: AtomicU64,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("worker", &self.worker)
            .field("profile", &self.profile.profile)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

impl Bridge {
    /// Starts `command` as the worker, with piped stdio, killed when the
    /// bridge is dropped, and runs the handshake. A worker that refuses,
    /// breaks the handshake or does not answer in time is terminated.
    pub async fn spawn(mut command: Command, config: Config) -> Result<Bridge, BridgeError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(BridgeError::Spawn)?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(BridgeError::Spawn(std::io::Error::other(
                "the worker's stdio is not piped",
            )));
        };
        let pid = child.id().and_then(|id| i32::try_from(id).ok());
        let process = Arc::new(Process {
            child: tokio::sync::Mutex::new(child),
            pid,
        });
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap = usize::try_from(config.limits.max_stderr_bytes).unwrap_or(usize::MAX);
        tokio::spawn(drain_stderr(stderr, Arc::clone(&captured), cap));
        let grace = config.kill_grace;
        match Bridge::handshake(stdout, Box::new(stdin), config, Some(Arc::clone(&process))).await {
            Ok(mut bridge) => {
                bridge.stderr = captured;
                Ok(bridge)
            }
            Err(e) => {
                process.terminate(grace).await;
                Err(e)
            }
        }
    }

    /// Runs the handshake over `reader` and `writer`, which carry no
    /// process: an in-process worker in a test.
    pub async fn connect<R>(
        reader: R,
        writer: Writer,
        config: Config,
    ) -> Result<Bridge, BridgeError>
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        Bridge::handshake(reader, writer, config, None).await
    }

    async fn handshake<R>(
        mut reader: R,
        mut writer: Writer,
        config: Config,
        process: Option<Arc<Process>>,
    ) -> Result<Bridge, BridgeError>
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        let hello = config.hello();
        let hello_cap = usize::try_from(config.limits.max_request_bytes).unwrap_or(usize::MAX);
        let frame = frames::encode(&ToWorker::Hello(hello.clone()).to_value(), hello_cap)
            .map_err(BridgeError::Frame)?;
        let exchange = async {
            frames::write(&mut writer, &frame)
                .await
                .map_err(|e| BridgeError::Lost(format!("writing hello: {e}")))?;
            match frames::read(&mut reader, cap_of(crate::wire::MAX_REQUEST_BYTES)).await {
                Ok(Some(value)) => Ok(value),
                Ok(None) => Err(BridgeError::Lost(
                    "the worker closed its output before answering hello".into(),
                )),
                Err(fault) => Err(BridgeError::Frame(fault)),
            }
        };
        let value = tokio::time::timeout(config.handshake_timeout, exchange)
            .await
            .map_err(|_| BridgeError::HandshakeTimeout)??;
        let (protocol, worker, profile, limits) =
            match ToFabric::parse(&value, Expect::Handshake(&hello))
                .map_err(BridgeError::Violation)?
            {
                ToFabric::HelloAck {
                    protocol,
                    worker,
                    profile,
                    limits,
                } => (protocol, worker, profile, limits),
                ToFabric::HelloRefused {
                    versions,
                    code,
                    message,
                } => {
                    return Err(BridgeError::Refused {
                        versions,
                        code,
                        message,
                    });
                }
                _ => {
                    return Err(BridgeError::Violation(Refusal {
                        code: "protocol_violation",
                        reason: "unexpected",
                        detail: "a session message before hello_ack".into(),
                    }));
                }
            };
        let shared = Arc::new(Shared::default());
        // One frame per outstanding request or observe, one cancel for
        // each, and a shutdown.
        let room = cap_of(limits.max_in_flight)
            .saturating_mul(2)
            .saturating_add(1);
        let (outbox, queue) = mpsc::channel(room);
        let writer = tokio::spawn(write_loop(
            writer,
            queue,
            Arc::clone(&shared),
            process.clone(),
            config.kill_grace,
        ));
        let profile = Arc::new(profile);
        let reader = tokio::spawn(read_loop(
            reader,
            Arc::clone(&shared),
            Arc::clone(&profile),
            cap_of(limits.max_frame_bytes),
            writer.abort_handle(),
            process.clone(),
            config.kill_grace,
        ));
        Ok(Bridge {
            writes: Some(outbox),
            writer,
            shared,
            reader,
            process,
            stderr: Arc::new(Mutex::new(Vec::new())),
            protocol,
            worker,
            profile,
            limits,
            config,
            next_id: AtomicU64::new(1),
        })
    }

    /// The protocol version the handshake picked.
    #[must_use]
    pub fn protocol(&self) -> u64 {
        self.protocol
    }

    /// The worker's name and version.
    #[must_use]
    pub fn worker(&self) -> &str {
        &self.worker
    }

    /// The capability profile the worker enforces.
    #[must_use]
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The negotiated limits.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// What the worker wrote to stderr, at most `max_stderr_bytes`.
    #[must_use]
    pub fn stderr(&self) -> Vec<u8> {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Answers that came after Fabric stopped waiting for them. Their
    /// status is not recorded; the locator a late answer to a request
    /// names is, by `record_late` or the next call that is handed the log.
    #[must_use]
    pub fn late_answers(&self) -> u64 {
        self.shared.lock().late
    }

    /// Records in `log` the locators that answers to requests Fabric
    /// stopped waiting on named since the last call; the status each
    /// operation holds stays. `request`, `observe` and `InFlight::next` do
    /// this first, so a caller only needs it while it makes none of those.
    pub fn record_late<L: OperationLog>(&self, log: &mut L) -> Result<(), BridgeError> {
        Ok(self.shared.record_found(log)?)
    }

    /// Why the worker is gone, if it is.
    #[must_use]
    pub fn lost(&self) -> Option<String> {
        self.shared.lock().lost.clone()
    }

    /// Refuses `message` as the worker would on reading it under the
    /// negotiated profile: an action it does not offer, a limit over its
    /// own, a target of another shape, and every encoding rule, from a NUL
    /// in an argument to a zero deadline. What the worker would refuse
    /// before effect, Fabric refuses before sending.
    fn as_worker_reads(&self, message: &ToWorker) -> Result<(), Refusal> {
        ToWorker::parse_as_worker(
            &message.to_value(),
            Phase::Session,
            Versions::V1,
            std::slice::from_ref(&self.profile),
            &self.profile,
        )
        .map(|_| ())
    }

    fn request_id(&self) -> String {
        format!("r-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Registers a waiter under `id`, unless the worker is gone, the
    /// bridge is shutting down, or (for a request or an observe) the
    /// worker's in-flight limit is reached.
    fn register(
        &self,
        id: &str,
        operation: Option<OperationId>,
        action: Option<&str>,
        kind: Kind,
    ) -> Result<mpsc::Receiver<Incoming>, BridgeError> {
        let mut state = self.shared.lock();
        if let Some(why) = &state.lost {
            return Err(BridgeError::Lost(why.clone()));
        }
        if state.shutting_down && kind != Kind::Control {
            return Err(BridgeError::ShuttingDown);
        }
        if kind != Kind::Control {
            let outstanding = state
                .waiters
                .values()
                .filter(|w| w.kind != Kind::Control)
                .count();
            let outstanding = u64::try_from(outstanding).unwrap_or(u64::MAX);
            if outstanding >= self.limits.max_in_flight {
                return Err(BridgeError::Busy(outstanding));
            }
        }
        let (tx, rx) = mpsc::channel(QUEUE);
        state.waiters.insert(
            id.to_owned(),
            Waiter {
                operation,
                action: action.map(str::to_owned),
                terminal_only: kind == Kind::Request
                    && action
                        .and_then(|a| self.profile.action(a))
                        .is_some_and(|spec| spec.terminal_only),
                kind,
                progress: 0,
                tx,
            },
        );
        Ok(rx)
    }

    fn unregister(&self, id: &str) {
        self.shared.lock().waiters.remove(id);
    }

    /// A place in the writer's queue, taken before anything is recorded,
    /// so a frame that is recorded is always queued.
    fn reserve(&self) -> Result<mpsc::Permit<'_, Write>, BridgeError> {
        let Some(writes) = &self.writes else {
            return Err(BridgeError::ShuttingDown);
        };
        writes.try_reserve().map_err(|e| match e {
            mpsc::error::TrySendError::Full(()) => {
                BridgeError::Busy(u64::try_from(writes.max_capacity()).unwrap_or(u64::MAX))
            }
            mpsc::error::TrySendError::Closed(()) => BridgeError::Lost(
                self.lost()
                    .unwrap_or_else(|| "the worker's input is closed".into()),
            ),
        })
    }

    /// `register` and `reserve` together; nothing is left registered when
    /// the queue refuses.
    fn enlist(
        &self,
        id: &str,
        operation: Option<OperationId>,
        action: Option<&str>,
        kind: Kind,
    ) -> Result<(mpsc::Receiver<Incoming>, mpsc::Permit<'_, Write>), BridgeError> {
        let rx = self.register(id, operation, action, kind)?;
        match self.reserve() {
            Ok(permit) => Ok((rx, permit)),
            Err(e) => {
                self.unregister(id);
                Err(e)
            }
        }
    }

    /// Sends one effect for a journal operation.
    ///
    /// Before anything is written: a request outside the negotiated
    /// profile or over one of its limits, or a frame over the request
    /// cap, is recorded `rejected` and refused as `RefusedLocally`. Then
    /// the operation is recorded `dispatched`; a key or id the log holds
    /// refuses with `Log(Used)` and nothing is sent. Then the frame is
    /// queued for the writer, which writes it whole by the request's
    /// deadline. A worker that is gone, busy or shutting down refuses
    /// before the log is touched.
    ///
    /// It never waits on the worker, so a caller that stops waiting on it
    /// leaves either nothing recorded or the operation recorded and its
    /// frame queued whole.
    #[allow(
        clippy::unused_async,
        reason = "async like the rest of the client; it records and queues, so it completes without waiting"
    )]
    pub async fn request<L: OperationLog>(
        &self,
        log: &mut L,
        effect: Effect,
    ) -> Result<InFlight, BridgeError> {
        self.shared.record_found(log)?;
        let request = Request {
            request_id: self.request_id(),
            operation: effect.operation,
            scope: effect.scope,
            action: effect.action,
            approval: effect.approval,
            policy: effect.policy,
            deadline_ms: effect.deadline_ms,
        };
        let dispatch = ProviderDispatch {
            operation: request.operation.id,
            key: request.operation.key.clone(),
            digest: RequestDigest::from_bytes(request.operation.digest.0),
            job: request.scope.job,
            task: request.scope.task,
            attempt: request.scope.attempt,
            generation: request.scope.generation,
            action: request.action.name().to_owned(),
            target: request.action.target().map(locator_text),
        };
        let refuse = |log: &mut L, refusal: Refusal| -> BridgeError {
            let answer = ProviderAnswer {
                code: Some(refusal.code.to_owned()),
                reason: Some(refusal.reason.to_owned()),
                effect: Some("none".to_owned()),
            };
            match log.begin(&dispatch, Some(&answer)) {
                Ok(()) => BridgeError::RefusedLocally(refusal),
                Err(e) => BridgeError::Log(e),
            }
        };
        if let Err(refusal) = self.as_worker_reads(&ToWorker::Request(Box::new(request.clone()))) {
            return Err(refuse(log, refusal));
        }
        let frame = match frames::encode(&request.to_value(), cap_of(self.limits.max_request_bytes))
        {
            Ok(frame) => frame,
            Err(fault) => {
                return Err(refuse(
                    log,
                    Refusal {
                        code: "invalid",
                        reason: "limit",
                        detail: fault.to_string(),
                    },
                ));
            }
        };
        let (rx, permit) = self.enlist(
            &request.request_id,
            Some(dispatch.operation),
            Some(&dispatch.action),
            Kind::Request,
        )?;
        if let Err(e) = log.begin(&dispatch, None) {
            self.unregister(&request.request_id);
            return Err(e.into());
        }
        let deadline =
            Instant::now() + Duration::from_millis(request.deadline_ms) + self.config.grace;
        permit.send(Write { frame, deadline });
        Ok(InFlight {
            request_id: request.request_id,
            operation: dispatch.operation,
            kind: Kind::Request,
            rx,
            deadline,
            shared: Arc::clone(&self.shared),
            done: false,
        })
    }

    /// Reads the resource of an operation the log holds as `dispatched`
    /// or `unknown`, by its original id, waiting at most `wait_ms`. An
    /// action the profile does not observe, or a wait over its limit, is
    /// refused before sending and changes nothing in the log.
    ///
    /// `action` and `resource` must be the operation's own as the log
    /// holds them: its action, and the target it was sent to or, for an
    /// action without one such as a create, the locator its worker
    /// reported. Anything else is `ObserveMismatch` before sending, since
    /// the answer would settle this operation from another resource's
    /// state: a destroy recorded done while its target still runs.
    #[allow(
        clippy::unused_async,
        reason = "async like the rest of the client; it queues its frame, so it completes without waiting"
    )]
    pub async fn observe<L: OperationLog>(
        &self,
        log: &mut L,
        operation: OperationId,
        action: &str,
        resource: Locator,
        wait_ms: u64,
    ) -> Result<InFlight, BridgeError> {
        // A late answer may have named the resource this observe reads.
        self.shared.record_found(log)?;
        let observe = Observe {
            request_id: self.request_id(),
            operation_id: operation,
            action: action.to_owned(),
            resource,
            wait_ms,
        };
        self.as_worker_reads(&ToWorker::Observe(observe.clone()))
            .map_err(BridgeError::RefusedLocally)?;
        let entry = log.entry(operation)?.filter(|e| {
            matches!(
                e.status,
                ProviderStatus::Dispatched | ProviderStatus::Unknown
            )
        });
        let Some(entry) = entry else {
            return Err(BridgeError::NotObservable(operation));
        };
        own_resource(&entry, &observe.action, &observe.resource)
            .map_err(|why| BridgeError::ObserveMismatch { operation, why })?;
        let frame = frames::encode(
            &ToWorker::Observe(observe.clone()).to_value(),
            cap_of(self.limits.max_request_bytes),
        )
        .map_err(|fault| {
            BridgeError::RefusedLocally(Refusal {
                code: "invalid",
                reason: "limit",
                detail: fault.to_string(),
            })
        })?;
        let (rx, permit) = self.enlist(
            &observe.request_id,
            Some(operation),
            Some(&observe.action),
            Kind::Observe,
        )?;
        let deadline = Instant::now() + Duration::from_millis(wait_ms) + self.config.grace;
        permit.send(Write { frame, deadline });
        Ok(InFlight {
            request_id: observe.request_id,
            operation,
            kind: Kind::Observe,
            rx,
            deadline,
            shared: Arc::clone(&self.shared),
            done: false,
        })
    }

    /// Asks the worker to stop waiting on the outstanding request
    /// `request_id`. A request that is not outstanding is `NotFound`
    /// without a frame; an action the profile does not cancel is
    /// `Unsupported` without a frame. The request still answers on its
    /// own: a cancel acknowledgement is not termination.
    #[allow(
        clippy::unused_async,
        reason = "async like the rest of the client; it queues its frame, so it completes without waiting"
    )]
    pub async fn cancel(&self, request_id: &str) -> Result<Control<CancelState>, BridgeError> {
        let found = {
            let state = self.shared.lock();
            state
                .waiters
                .get(request_id)
                .filter(|w| w.kind == Kind::Request)
                .and_then(|w| Some((w.operation?, w.action.clone()?)))
        };
        let Some((operation, action)) = found else {
            return Ok(Control::Local(CancelState::NotFound));
        };
        if !self.profile.action(&action).is_some_and(|spec| spec.cancel) {
            return Ok(Control::Local(CancelState::Unsupported));
        }
        let cancels = request_id.to_owned();
        let cancel = Cancel {
            request_id: self.request_id(),
            cancels,
            operation_id: operation,
        };
        let frame = frames::encode(
            &ToWorker::Cancel(cancel.clone()).to_value(),
            cap_of(self.limits.max_request_bytes),
        )
        .map_err(BridgeError::Frame)?;
        let (rx, permit) = self.enlist(&cancel.request_id, Some(operation), None, Kind::Control)?;
        let deadline = Instant::now() + self.config.grace;
        permit.send(Write { frame, deadline });
        Ok(Control::Sent(Pending {
            request_id: cancel.request_id,
            rx,
            deadline,
            shared: Arc::clone(&self.shared),
            _answer: std::marker::PhantomData,
        }))
    }

    /// Asks the worker to drain for at most `drain_ms` and exit. New
    /// requests are refused from now on, before the log is touched. The
    /// outstanding requests still answer on their own.
    #[allow(
        clippy::unused_async,
        reason = "async like the rest of the client; it queues its frame, so it completes without waiting"
    )]
    pub async fn shutdown(&self, drain_ms: u64) -> Result<Pending<Vec<Unresolved>>, BridgeError> {
        self.shared.lock().shutting_down = true;
        let request_id = self.request_id();
        let frame = frames::encode(
            &ToWorker::Shutdown {
                request_id: request_id.clone(),
                drain_ms,
            }
            .to_value(),
            cap_of(self.limits.max_request_bytes),
        )
        .map_err(BridgeError::Frame)?;
        let (rx, permit) = self.enlist(&request_id, None, None, Kind::Control)?;
        let deadline = Instant::now() + Duration::from_millis(drain_ms) + self.config.grace;
        permit.send(Write { frame, deadline });
        Ok(Pending {
            request_id,
            rx,
            deadline,
            shared: Arc::clone(&self.shared),
            _answer: std::marker::PhantomData,
        })
    }

    /// Closes the worker's input once the frames already queued are
    /// written, which the protocol reads as a shutdown with no drain, and
    /// waits up to the kill grace for it to exit before killing it. A
    /// writer held up by a worker that does not read is stopped at the
    /// kill grace, which closes the input as well. `None` when the bridge
    /// runs no process.
    pub async fn close(mut self) -> Option<ExitStatus> {
        self.writes.take();
        if tokio::time::timeout(self.config.kill_grace, &mut self.writer)
            .await
            .is_err()
        {
            self.writer.abort();
            let _ = (&mut self.writer).await;
        }
        let process = self.process.clone()?;
        let mut child = process.child.lock().await;
        if let Ok(Ok(status)) = tokio::time::timeout(self.config.kill_grace, child.wait()).await {
            return Some(status);
        }
        let _ = child.start_kill();
        child.wait().await.ok()
    }

    /// Kills the worker at once, as a crash would.
    pub async fn kill(&self) {
        if let Some(process) = &self.process {
            let mut child = process.child.lock().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

/// One effect to send: a journal operation and what it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    /// The journal operation: its id, key and Fabric's digest.
    pub operation: OperationRef,
    /// The managed identities it belongs to.
    pub scope: Scope,
    /// The action, its target and its parameters.
    pub action: Action,
    /// The fresh approval ref, if any.
    pub approval: Option<String>,
    /// Fabric's narrowing of policy.
    pub policy: Option<RequestPolicy>,
    /// Milliseconds from when the worker reads the frame.
    pub deadline_ms: u64,
}

/// A control message's answer: settled locally, or sent and pending.
#[derive(Debug)]
pub enum Control<T> {
    /// Fabric answered without a frame.
    Local(T),
    /// Sent; the worker's answer is pending.
    Sent(Pending<T>),
}

/// The pending answer to a `cancel` or a `shutdown`.
#[derive(Debug)]
pub struct Pending<T> {
    request_id: String,
    rx: mpsc::Receiver<Incoming>,
    deadline: Instant,
    shared: Arc<Shared>,
    _answer: std::marker::PhantomData<T>,
}

impl<T> Pending<T> {
    /// The transport id the message was sent under.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    async fn next(&mut self) -> Result<ToFabric, BridgeError> {
        let incoming = tokio::time::timeout_at(self.deadline, self.rx.recv()).await;
        self.shared.lock().waiters.remove(&self.request_id);
        match incoming {
            Ok(Some(Incoming::Answer(message))) => Ok(*message),
            Ok(Some(Incoming::Lost(why))) => Err(BridgeError::Lost(why)),
            Ok(Some(Incoming::Progress(_)) | None) => {
                Err(BridgeError::Lost("the bridge reader stopped".into()))
            }
            Err(_) => Err(BridgeError::Lost(
                "the worker did not answer in time".into(),
            )),
        }
    }
}

impl Pending<CancelState> {
    /// Waits for `cancel_ack`.
    pub async fn wait(mut self) -> Result<CancelState, BridgeError> {
        match self.next().await? {
            ToFabric::CancelAck { state, .. } => Ok(state),
            other => Err(unexpected(&other)),
        }
    }
}

impl Pending<Vec<Unresolved>> {
    /// Waits for `shutdown_ack` and returns what the worker left open.
    pub async fn wait(mut self) -> Result<Vec<Unresolved>, BridgeError> {
        match self.next().await? {
            ToFabric::ShutdownAck { unresolved, .. } => Ok(unresolved),
            other => Err(unexpected(&other)),
        }
    }
}

fn unexpected(message: &ToFabric) -> BridgeError {
    BridgeError::Violation(Refusal {
        code: "protocol_violation",
        reason: "unexpected",
        detail: format!("{message:?}"),
    })
}

/// An outstanding request or observe. Each answer is recorded in the log
/// before it is returned.
#[derive(Debug)]
#[must_use = "an outstanding request is recorded only when it is finished"]
pub struct InFlight {
    request_id: String,
    operation: OperationId,
    kind: Kind,
    rx: mpsc::Receiver<Incoming>,
    deadline: Instant,
    shared: Arc<Shared>,
    done: bool,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if !self.done {
            self.shared.expire(&self.request_id);
        }
    }
}

impl InFlight {
    /// The transport id it was sent under.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Its operation.
    #[must_use]
    pub fn operation(&self) -> OperationId {
        self.operation
    }

    /// The next step: a locator reported early, or the answer. Past
    /// Fabric's deadline the answer is `TimedOut` and the operation
    /// `unknown`; a later answer leaves the status as it is and only the
    /// locator it names is recorded (`Bridge::record_late`).
    pub async fn next<L: OperationLog>(&mut self, log: &mut L) -> Result<Event, BridgeError> {
        if self.done {
            return Err(BridgeError::Lost("the request was already answered".into()));
        }
        self.shared.record_found(log)?;
        let incoming = tokio::time::timeout_at(self.deadline, self.rx.recv()).await;
        let answer = match incoming {
            Err(_) => {
                self.shared.expire(&self.request_id);
                Answer::TimedOut
            }
            Ok(None) => Answer::Lost("the bridge reader stopped".into()),
            Ok(Some(Incoming::Lost(why))) => Answer::Lost(why),
            Ok(Some(Incoming::Progress(locator))) => {
                log.locate(self.operation, &locator_text(&locator))?;
                return Ok(Event::Progress(locator));
            }
            Ok(Some(Incoming::Answer(message))) => match *message {
                ToFabric::Result(r) => Answer::Result(r),
                ToFabric::Error(e) => Answer::Error(e),
                other => {
                    let why = format!("an unexpected answer: {other:?}");
                    self.shared.lose(&why);
                    Answer::Lost(why)
                }
            },
        };
        let outcome = self.settle(log, answer)?;
        self.done = true;
        Ok(Event::Done(outcome))
    }

    /// Waits for the answer, recording any locator reported before it.
    pub async fn finish<L: OperationLog>(mut self, log: &mut L) -> Result<Outcome, BridgeError> {
        loop {
            if let Event::Done(outcome) = self.next(log).await? {
                return Ok(outcome);
            }
        }
    }

    /// Records `answer` and returns the status the log holds after it.
    fn settle<L: OperationLog>(&self, log: &mut L, answer: Answer) -> Result<Outcome, BridgeError> {
        let before = log
            .entry(self.operation)?
            .map_or(ProviderStatus::Dispatched, |e| e.status);
        let (status, record, locator) = match (&answer, self.kind) {
            (Answer::Result(r), _) => match r.state {
                ResultState::Completed => (
                    ProviderStatus::Succeeded,
                    ProviderAnswer::default(),
                    Some(&r.resource),
                ),
                ResultState::Cancelled => (
                    ProviderStatus::Cancelled,
                    ProviderAnswer::default(),
                    Some(&r.resource),
                ),
                ResultState::Accepted | ResultState::Running => {
                    (before, ProviderAnswer::default(), Some(&r.resource))
                }
            },
            (Answer::Error(e), kind) => {
                let body = &e.error;
                let record = ProviderAnswer {
                    code: Some(body.code.name().to_owned()),
                    reason: Some(body.reason.clone()),
                    effect: Some(body.effect.name().to_owned()),
                };
                let status = match (body.code, kind) {
                    (ErrorCode::Pending, _) => before,
                    (ErrorCode::Unknown, _) => ProviderStatus::Unknown,
                    (code, Kind::Request) if code.is_refusal() => ProviderStatus::Rejected,
                    (ErrorCode::ProviderFailed, Kind::Request) => ProviderStatus::Failed,
                    // A refused or failed observe says nothing new about the
                    // operation it read.
                    _ => {
                        return Ok(Outcome {
                            status: before,
                            answer,
                        });
                    }
                };
                (status, record, body.resource.as_ref())
            }
            (Answer::TimedOut | Answer::Lost(_), _) => {
                let status = if before == ProviderStatus::Dispatched {
                    ProviderStatus::Unknown
                } else {
                    before
                };
                (status, ProviderAnswer::default(), None)
            }
        };
        if before.is_terminal() {
            return Ok(Outcome {
                status: before,
                answer,
            });
        }
        let locator = locator.map(locator_text);
        log.record(self.operation, status, &record, locator.as_deref())?;
        Ok(Outcome { status, answer })
    }
}

fn locator_text(locator: &Locator) -> String {
    locator.to_value().to_string()
}

/// Whether `action` on `resource` is the operation's own as `entry` holds
/// it: its action, and its target, or for an action without one the
/// locator its worker reported. A row that names no locator has nothing
/// to be observed by.
fn own_resource(entry: &Entry, action: &str, resource: &Locator) -> Result<(), String> {
    if entry.action != action {
        return Err(format!(
            "it was dispatched as {}, not {action}",
            entry.action
        ));
    }
    let Some(text) = entry.target.as_deref().or(entry.locator.as_deref()) else {
        return Err("the log holds no locator for it".into());
    };
    let own = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| Locator::from_value(&value).ok());
    if own.as_ref() != Some(resource) {
        return Err(format!("{text} is its resource, not the one named"));
    }
    Ok(())
}

fn cap_of(bytes: u64) -> usize {
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// Writes each queued frame whole, in order, by its deadline. A write
/// that fails or misses its deadline may have left part of a frame on the
/// worker's input, which cannot be resynchronized: the worker is lost and
/// terminated, as for a framing error from it. When the queue closes, the
/// worker's input is closed.
async fn write_loop(
    mut writer: Writer,
    mut queue: mpsc::Receiver<Write>,
    shared: Arc<Shared>,
    process: Option<Arc<Process>>,
    grace: Duration,
) {
    while let Some(write) = queue.recv().await {
        let why = match write_by(&mut writer, &write.frame, write.deadline).await {
            Ok(Ok(())) => continue,
            Ok(Err(e)) => format!("writing to the worker: {e}"),
            Err(_) => "the worker did not read a frame by its deadline".to_owned(),
        };
        shared.lose(&why);
        drop(writer);
        drop(queue);
        if let Some(process) = process {
            process.terminate(grace).await;
        }
        return;
    }
}

/// Writes `frame` by `deadline`. The deadline's timer is armed only when
/// the write does not finish at once, so a worker that keeps reading costs
/// the write alone: each timer takes the runtime's timer lock when dropped.
async fn write_by(
    writer: &mut Writer,
    frame: &[u8],
    deadline: Instant,
) -> Result<std::io::Result<()>, tokio::time::error::Elapsed> {
    let mut write = std::pin::pin!(frames::write(writer, frame));
    match std::future::poll_fn(|cx| std::task::Poll::Ready(write.as_mut().poll(cx))).await {
        std::task::Poll::Ready(written) => Ok(written),
        std::task::Poll::Pending => tokio::time::timeout_at(deadline, write).await,
    }
}

/// Reads the worker's frames until it is gone, routing each answer to its
/// waiter. A frame or a message that breaks the protocol, a
/// `protocol_error`, or the end of the stream makes the worker lost; on a
/// break Fabric closes its input and terminates it.
async fn read_loop<R: AsyncRead + Unpin>(
    mut reader: R,
    shared: Arc<Shared>,
    profile: Arc<Profile>,
    cap: usize,
    writer: AbortHandle,
    process: Option<Arc<Process>>,
    grace: Duration,
) {
    let why = loop {
        let value = match frames::read(&mut reader, cap).await {
            Ok(Some(value)) => value,
            Ok(None) => break None,
            Err(fault) => break Some(format!("frame refused ({}): {fault}", fault.code())),
        };
        let message = match ToFabric::parse(&value, Expect::Session(&profile)) {
            Ok(message) => message,
            Err(refusal) => break Some(format!("{refusal}")),
        };
        if let ToFabric::ProtocolError { code, message } = &message {
            break Some(format!("the worker reported {code}: {message}"));
        }
        if let Err(why) = route(&shared, message) {
            break Some(why);
        }
    };
    match why {
        None => shared.lose("the worker closed its output"),
        Some(why) => {
            shared.lose(&why);
            // Stopping the writer drops the worker's input, even mid-frame.
            writer.abort();
            if let Some(process) = process {
                process.terminate(grace).await;
            }
        }
    }
}

/// Hands `message` to its waiter. An answer whose operation is not the
/// waiter's breaks the protocol; one for no waiter came late and is dropped.
fn route(shared: &Shared, message: ToFabric) -> Result<(), String> {
    let (id, operation, last) = match &message {
        ToFabric::Result(r) => (r.request_id.clone(), Some(r.operation_id), true),
        ToFabric::Error(e) => (e.request_id.clone(), Some(e.operation_id), true),
        ToFabric::Progress {
            request_id,
            operation_id,
            ..
        } => (request_id.clone(), Some(*operation_id), false),
        ToFabric::CancelAck { request_id, .. } | ToFabric::ShutdownAck { request_id, .. } => {
            (request_id.clone(), None, true)
        }
        other => return Err(format!("a {other:?} in the session")),
    };
    let mut state = shared.lock();
    let Some(waiter) = state.waiters.get_mut(&id) else {
        state.late += 1;
        return late(&mut state, &id, &message);
    };
    if operation.is_some() && waiter.kind != Kind::Control && operation != waiter.operation {
        return Err(format!("an answer to {id} names another operation"));
    }
    let fits = match (&message, waiter.kind) {
        // A terminal-only action answers a result only once terminal.
        (ToFabric::Result(r), Kind::Request) => {
            !(waiter.terminal_only
                && matches!(r.state, ResultState::Accepted | ResultState::Running))
        }
        (ToFabric::Result(_) | ToFabric::Error(_) | ToFabric::Progress { .. }, kind) => {
            kind != Kind::Control
        }
        (ToFabric::CancelAck { .. }, Kind::Control) => waiter.operation.is_some(),
        (ToFabric::ShutdownAck { .. }, Kind::Control) => waiter.operation.is_none(),
        _ => false,
    };
    if !fits {
        return Err(format!(
            "{id} was answered with a message that does not answer it: {message:?}"
        ));
    }
    let incoming = match message {
        ToFabric::Progress { seq, resource, .. } => {
            // At most 16 per request, in order: what waits is bounded.
            if seq != waiter.progress + 1 {
                return Err(format!(
                    "progress {seq} for {id} after progress {}",
                    waiter.progress
                ));
            }
            waiter.progress = seq;
            Incoming::Progress(resource)
        }
        other => Incoming::Answer(Box::new(other)),
    };
    if let Err(mpsc::error::TrySendError::Full(_)) = waiter.tx.try_send(incoming) {
        return Err(format!("{id} got more frames than it can wait for"));
    }
    if last {
        state.waiters.remove(&id);
    }
    Ok(())
}

/// An answer or a `progress` for a request Fabric stopped waiting on.
/// The operation keeps the status its deadline left; the locator it
/// names is kept for the log, since the bridge contract records a locator
/// as soon as it is observed and only observe by it moves the operation
/// on. One naming another operation, or a `progress` out of order, breaks
/// the protocol as it would in time. One for a request id never sent, or
/// forgotten past `EXPIRED`, is only counted.
fn late(state: &mut State, id: &str, message: &ToFabric) -> Result<(), String> {
    let Some(at) = state.expired.iter().position(|e| e.request_id == id) else {
        return Ok(());
    };
    let (operation, resource, seq) = match message {
        ToFabric::Result(r) => (r.operation_id, Some(&r.resource), None),
        ToFabric::Error(e) => (e.operation_id, e.error.resource.as_ref(), None),
        ToFabric::Progress {
            operation_id,
            seq,
            resource,
            ..
        } => (*operation_id, Some(resource), Some(*seq)),
        other => {
            return Err(format!(
                "{id} was answered with a message that does not answer it: {other:?}"
            ));
        }
    };
    let expired = &mut state.expired[at];
    if operation != expired.operation {
        return Err(format!("an answer to {id} names another operation"));
    }
    match seq {
        Some(seq) if seq != expired.progress + 1 => {
            return Err(format!(
                "progress {seq} for {id} after progress {}",
                expired.progress
            ));
        }
        Some(seq) => expired.progress = seq,
        None => {
            state.expired.remove(at);
        }
    }
    if let Some(resource) = resource
        && (state.found.len() < EXPIRED || state.found.contains_key(&operation))
    {
        state.found.insert(operation, resource.clone());
    }
    Ok(())
}

/// Keeps the first `cap` bytes the worker writes to stderr and drops the rest.
async fn drain_stderr<R: AsyncRead + Unpin>(mut stderr: R, keep: Arc<Mutex<Vec<u8>>>, cap: usize) {
    let mut buf = [0u8; 8192];
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut kept = keep.lock().unwrap_or_else(PoisonError::into_inner);
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}
