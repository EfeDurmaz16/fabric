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

use std::collections::HashMap;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fabric_core::managed::{OperationId, RequestDigest};
use fabric_journal::{ProviderAnswer, ProviderDispatch, ProviderStatus};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::frames::{self, FrameFault};
use crate::log::{LogError, OperationLog};
use crate::wire::{
    Action, Cancel, CancelState, ErrorCode, ErrorMessage, Expect, Hello, Limits, Locator, Observe,
    OperationRef, Profile, Refusal, Request, RequestPolicy, ResultMessage, ResultState, Scope,
    ToFabric, ToWorker, Unresolved, Versions, fits_observe, fits_request,
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
    kind: Kind,
    tx: mpsc::UnboundedSender<Incoming>,
}

#[derive(Debug, Default)]
struct State {
    waiters: HashMap<String, Waiter>,
    lost: Option<String>,
    shutting_down: bool,
    late: u64,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks the worker gone and tells every waiter, once.
    fn lose(&self, why: &str) {
        let mut state = self.lock();
        if state.lost.is_none() {
            state.lost = Some(why.to_owned());
        }
        for (_, waiter) in state.waiters.drain() {
            let _ = waiter.tx.send(Incoming::Lost(why.to_owned()));
        }
    }
}

type Writer = Box<dyn AsyncWrite + Send + Unpin>;

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
    writer: Arc<tokio::sync::Mutex<Option<Writer>>>,
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
        let writer = Arc::new(tokio::sync::Mutex::new(Some(writer)));
        let profile = Arc::new(profile);
        let reader = tokio::spawn(read_loop(
            reader,
            Arc::clone(&shared),
            Arc::clone(&profile),
            cap_of(limits.max_frame_bytes),
            Arc::clone(&writer),
            process.clone(),
            config.kill_grace,
        ));
        Ok(Bridge {
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

    /// Answers that came after Fabric stopped waiting for them, dropped unread.
    #[must_use]
    pub fn late_answers(&self) -> u64 {
        self.shared.lock().late
    }

    /// Why the worker is gone, if it is.
    #[must_use]
    pub fn lost(&self) -> Option<String> {
        self.shared.lock().lost.clone()
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
    ) -> Result<mpsc::UnboundedReceiver<Incoming>, BridgeError> {
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
        let (tx, rx) = mpsc::unbounded_channel();
        state.waiters.insert(
            id.to_owned(),
            Waiter {
                operation,
                action: action.map(str::to_owned),
                kind,
                tx,
            },
        );
        Ok(rx)
    }

    fn unregister(&self, id: &str) {
        self.shared.lock().waiters.remove(id);
    }

    async fn send(&self, frame: &[u8]) -> Result<(), String> {
        let mut writer = self.writer.lock().await;
        let Some(writer) = writer.as_mut() else {
            return Err("the worker's input is closed".into());
        };
        frames::write(writer, frame)
            .await
            .map_err(|e| format!("writing to the worker: {e}"))
    }

    /// Sends one effect for a journal operation.
    ///
    /// Before anything is written: a request outside the negotiated
    /// profile or over one of its limits, or a frame over the request
    /// cap, is recorded `rejected` and refused as `RefusedLocally`. Then
    /// the operation is recorded `dispatched`; a key or id the log holds
    /// refuses with `Log(Used)` and nothing is sent. Then the frame is
    /// written. A worker that is gone, busy or shutting down refuses
    /// before the log is touched.
    pub async fn request<L: OperationLog>(
        &self,
        log: &mut L,
        effect: Effect,
    ) -> Result<InFlight, BridgeError> {
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
        if let Err(refusal) = fits_request(&request, &self.profile) {
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
        let rx = self.register(
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
        let mut flight = InFlight {
            request_id: request.request_id.clone(),
            operation: dispatch.operation,
            action: dispatch.action,
            kind: Kind::Request,
            rx,
            deadline,
            shared: Arc::clone(&self.shared),
            terminal_only: self
                .profile
                .action(request.action.name())
                .is_some_and(|spec| spec.terminal_only),
            done: false,
        };
        if let Err(why) = self.send(&frame).await {
            // Part of the frame may have reached the worker: unknown.
            self.shared.lose(&why);
            let outcome = flight.settle(log, Answer::Lost(why.clone()))?;
            flight.done = true;
            debug_assert_eq!(outcome.status, ProviderStatus::Unknown);
            return Err(BridgeError::Lost(why));
        }
        Ok(flight)
    }

    /// Reads the resource of an operation the log holds as `dispatched`
    /// or `unknown`, by its original id, waiting at most `wait_ms`. An
    /// action the profile does not observe, or a wait over its limit, is
    /// refused before sending and changes nothing in the log.
    pub async fn observe<L: OperationLog>(
        &self,
        log: &mut L,
        operation: OperationId,
        action: &str,
        resource: Locator,
        wait_ms: u64,
    ) -> Result<InFlight, BridgeError> {
        let observe = Observe {
            request_id: self.request_id(),
            operation_id: operation,
            action: action.to_owned(),
            resource,
            wait_ms,
        };
        fits_observe(&observe, &self.profile).map_err(BridgeError::RefusedLocally)?;
        let observable = log.entry(operation)?.is_some_and(|e| {
            matches!(
                e.status,
                ProviderStatus::Dispatched | ProviderStatus::Unknown
            )
        });
        if !observable {
            return Err(BridgeError::NotObservable(operation));
        }
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
        let rx = self.register(
            &observe.request_id,
            Some(operation),
            Some(&observe.action),
            Kind::Observe,
        )?;
        let mut flight = InFlight {
            request_id: observe.request_id,
            operation,
            action: observe.action,
            kind: Kind::Observe,
            rx,
            deadline: Instant::now() + Duration::from_millis(wait_ms) + self.config.grace,
            shared: Arc::clone(&self.shared),
            terminal_only: false,
            done: false,
        };
        if let Err(why) = self.send(&frame).await {
            self.shared.lose(&why);
            flight.settle(log, Answer::Lost(why.clone()))?;
            flight.done = true;
            return Err(BridgeError::Lost(why));
        }
        Ok(flight)
    }

    /// Asks the worker to stop waiting on the outstanding request
    /// `request_id`. A request that is not outstanding is `NotFound`
    /// without a frame; an action the profile does not cancel is
    /// `Unsupported` without a frame. The request still answers on its
    /// own: a cancel acknowledgement is not termination.
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
        let rx = self.register(&cancel.request_id, Some(operation), None, Kind::Control)?;
        if let Err(why) = self.send(&frame).await {
            self.shared.lose(&why);
            return Err(BridgeError::Lost(why));
        }
        Ok(Control::Sent(Pending {
            request_id: cancel.request_id,
            rx,
            deadline: Instant::now() + self.config.grace,
            shared: Arc::clone(&self.shared),
            _answer: std::marker::PhantomData,
        }))
    }

    /// Asks the worker to drain for at most `drain_ms` and exit. New
    /// requests are refused from now on, before the log is touched. The
    /// outstanding requests still answer on their own.
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
        let rx = self.register(&request_id, None, None, Kind::Control)?;
        if let Err(why) = self.send(&frame).await {
            self.shared.lose(&why);
            return Err(BridgeError::Lost(why));
        }
        Ok(Pending {
            request_id,
            rx,
            deadline: Instant::now() + Duration::from_millis(drain_ms) + self.config.grace,
            shared: Arc::clone(&self.shared),
            _answer: std::marker::PhantomData,
        })
    }

    /// Closes the worker's input, which the protocol reads as a shutdown
    /// with no drain, and waits up to the kill grace for it to exit before
    /// killing it. `None` when the bridge runs no process.
    pub async fn close(self) -> Option<ExitStatus> {
        self.writer.lock().await.take();
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
    rx: mpsc::UnboundedReceiver<Incoming>,
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
    action: String,
    kind: Kind,
    rx: mpsc::UnboundedReceiver<Incoming>,
    deadline: Instant,
    shared: Arc<Shared>,
    terminal_only: bool,
    done: bool,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if !self.done {
            self.shared.lock().waiters.remove(&self.request_id);
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
    /// `unknown`; a later answer is dropped unread.
    pub async fn next<L: OperationLog>(&mut self, log: &mut L) -> Result<Event, BridgeError> {
        if self.done {
            return Err(BridgeError::Lost("the request was already answered".into()));
        }
        let incoming = tokio::time::timeout_at(self.deadline, self.rx.recv()).await;
        let answer = match incoming {
            Err(_) => {
                self.shared.lock().waiters.remove(&self.request_id);
                Answer::TimedOut
            }
            Ok(None) => Answer::Lost("the bridge reader stopped".into()),
            Ok(Some(Incoming::Lost(why))) => Answer::Lost(why),
            Ok(Some(Incoming::Progress(locator))) => {
                log.locate(self.operation, &locator_text(&locator))?;
                return Ok(Event::Progress(locator));
            }
            Ok(Some(Incoming::Answer(message))) => match *message {
                ToFabric::Result(r) => {
                    if self.terminal_only
                        && matches!(r.state, ResultState::Accepted | ResultState::Running)
                    {
                        // A terminal-only action answered before a terminal
                        // state: the worker broke the protocol.
                        let why = format!(
                            "{} is terminal-only and the worker answered {:?}",
                            self.action, r.state
                        );
                        self.shared.lose(&why);
                        Answer::Lost(why)
                    } else {
                        Answer::Result(r)
                    }
                }
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

fn cap_of(bytes: u64) -> usize {
    usize::try_from(bytes).unwrap_or(usize::MAX)
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
    writer: Arc<tokio::sync::Mutex<Option<Writer>>>,
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
            writer.lock().await.take();
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
    let Some(waiter) = state.waiters.get(&id) else {
        state.late += 1;
        return Ok(());
    };
    if operation.is_some() && waiter.kind != Kind::Control && operation != waiter.operation {
        return Err(format!("an answer to {id} names another operation"));
    }
    let incoming = match message {
        ToFabric::Progress { resource, .. } => Incoming::Progress(resource),
        other => Incoming::Answer(Box::new(other)),
    };
    let _ = waiter.tx.send(incoming);
    if last {
        state.waiters.remove(&id);
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
