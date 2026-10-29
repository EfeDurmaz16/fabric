//! A minimal Agent Client Protocol client over stdio JSON-RPC.
//!
//! Invariant: the client speaks only the baseline of ACP schema v1
//! (`initialize`, `session/new`, `session/load`, `session/resume`,
//! `session/prompt`, `session/cancel`, `session/update`), answers every
//! agent request so the agent never blocks, and grants a permission only
//! when the policy the caller chose says so, never more than it says. Deny
//! is the default. Once a turn is cancelled, every permission request for
//! its session is answered `cancelled` until the next prompt, as the
//! schema asks of `session/cancel`, so no tool runs after a cancel. At
//! most `UPDATE_BUFFER` `session/update` notifications, and at most
//! `UPDATE_BUFFER_BYTES` of the lines they came in, wait for
//! `next_update`; newer ones are dropped and counted in
//! `dropped_updates`, so the reader never waits on a caller who is not
//! draining updates, and an undrained client never holds more. A handshake request has a
//! deadline, so a hung agent cannot hang `launch`. Every message reaches
//! the agent as one whole line written by a single writer task, so a
//! caller that drops a request mid-way cannot leave half a line on the
//! agent's stdin, and a dropped request leaves no waiter behind. A line
//! from the agent longer than `MAX_LINE` closes the connection, and so
//! does an agent that stops reading its stdin while `OUTGOING_BUFFER`
//! lines wait for it.
//!
//! Field names follow `schema/v1/schema.json` of
//! `agentclientprotocol/agent-client-protocol` at schema v1.26.0
//! (2026-10-16), read on 2026-10-19.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u16 = 1;

/// How long `initialize` and the session handshake may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);

/// The longest line read from the agent, in bytes.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// How many lines may wait for the writer before the agent is taken to
/// have stopped reading its stdin and the connection is closed.
pub const OUTGOING_BUFFER: usize = 1024;

/// How many `session/update` notifications wait for `next_update` before
/// newer ones are dropped and counted.
pub const UPDATE_BUFFER: usize = 1024;

/// How many bytes of `session/update` lines, as the agent sent them, wait
/// for `next_update` before newer ones are dropped and counted. With
/// `UPDATE_BUFFER` alone, 1024 lines of up to `MAX_LINE` each could wait,
/// 16 GiB. It equals `MAX_LINE`, so one update of the longest line still
/// fits a queue that waits on nothing. A parsed update takes a small
/// multiple of its line in memory, which this bounds as well.
pub const UPDATE_BUFFER_BYTES: usize = MAX_LINE;

/// Why an ACP exchange failed.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The pipe to the agent failed.
    #[error("agent pipe: {0}")]
    Io(#[from] std::io::Error),
    /// The agent answered with a JSON-RPC error.
    #[error("agent error {code}: {message}")]
    Rpc {
        /// The JSON-RPC error code.
        code: i64,
        /// The agent's message.
        message: String,
    },
    /// The agent sent something that is not JSON-RPC, or a result without a required field.
    #[error("protocol: {0}")]
    Protocol(String),
    /// The agent closed its stdout.
    #[error("agent closed the connection")]
    Closed,
    /// `OUTGOING_BUFFER` lines already waited for an agent that is not
    /// reading its stdin.
    #[error("the agent stopped reading: {OUTGOING_BUFFER} lines wait unwritten")]
    Saturated,
    /// A handshake request got no answer within `HANDSHAKE_TIMEOUT`.
    #[error("{method} got no answer within {HANDSHAKE_TIMEOUT:?}")]
    Timeout {
        /// The request that timed out.
        method: &'static str,
    },
    /// The agent answered `initialize` with a version this client does not speak.
    #[error("agent speaks protocol version {agent}, this client speaks {PROTOCOL_VERSION}")]
    Version {
        /// The version the agent reported.
        agent: u16,
    },
}

/// What the agent said it can do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentCapabilities {
    /// `agentCapabilities.loadSession`.
    pub load_session: bool,
    /// `agentCapabilities.sessionCapabilities.resume` is present.
    pub resume: bool,
}

/// Why a prompt turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// `end_turn`.
    EndTurn,
    /// `max_tokens`.
    MaxTokens,
    /// `max_turn_requests`.
    MaxTurnRequests,
    /// `refusal`.
    Refusal,
    /// `cancelled`.
    Cancelled,
    /// A value this client does not know.
    Other(String),
}

impl StopReason {
    fn parse(text: &str) -> Self {
        match text {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "max_turn_requests" => StopReason::MaxTurnRequests,
            "refusal" => StopReason::Refusal,
            "cancelled" => StopReason::Cancelled,
            other => StopReason::Other(other.to_owned()),
        }
    }
}

/// One `session/update` notification, kept raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUpdate {
    /// The session the update is for.
    pub session: SessionId,
    /// The `update` object as the agent sent it.
    pub update: Value,
}

/// How the client answers `session/request_permission`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Pick a reject option; cancel when the agent offers none.
    #[default]
    Deny,
    /// Pick an `allow_once` option; when the agent offers none, reject.
    /// A permanent grant is never picked by this policy.
    AllowOnce,
}

impl PermissionPolicy {
    fn choose(self, options: &[Value]) -> Value {
        let find = |kind: &str| {
            options
                .iter()
                .find(|o| o.get("kind").and_then(Value::as_str) == Some(kind))
                .and_then(|o| o.get("optionId").cloned())
        };
        let reject = || find("reject_once").or_else(|| find("reject_always"));
        let picked = match self {
            PermissionPolicy::AllowOnce => find("allow_once").or_else(reject),
            PermissionPolicy::Deny => reject(),
        };
        match picked {
            Some(option_id) => {
                json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
            }
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        }
    }
}

type Waiter = oneshot::Sender<Result<Value, AcpError>>;
type Pending = Arc<Mutex<Option<HashMap<u64, Waiter>>>>;
type Outgoing = mpsc::Sender<String>;
/// The sessions whose turn was cancelled and has not been prompted since.
type Cancelled = Arc<Mutex<HashSet<String>>>;

struct Inner {
    outgoing: Outgoing,
    pending: Pending,
    cancelled: Cancelled,
    next_id: AtomicU64,
    updates: tokio::sync::Mutex<mpsc::Receiver<Queued>>,
    /// The bytes of the lines of the updates that wait.
    queued_bytes: Arc<AtomicUsize>,
    dropped_updates: Arc<AtomicU64>,
    reader: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// A connection to one agent process; clones share it.
#[derive(Clone)]
pub struct AcpClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpClient").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Incoming {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcErrorBody>,
}

#[derive(Deserialize)]
struct RpcErrorBody {
    code: i64,
    message: String,
}

/// Queues one message for the writer task as a whole line.
fn write_line(outgoing: &Outgoing, value: &Value) -> Result<(), AcpError> {
    let mut text = serde_json::to_string(value).map_err(|e| AcpError::Protocol(e.to_string()))?;
    text.push('\n');
    outgoing.try_send(text).map_err(|e| match e {
        mpsc::error::TrySendError::Full(_) => AcpError::Saturated,
        mpsc::error::TrySendError::Closed(_) => AcpError::Closed,
    })
}

/// Writes queued lines to the agent until every sender is gone or a write
/// fails; then the agent's stdin is closed. A failed write fails every
/// waiting request: its line, or a later one, never reached the agent, and
/// an agent that keeps its stdout open would otherwise leave a request
/// with no deadline, such as a prompt, waiting forever.
///
/// With `go`, nothing is written before it fires; then the empty line the
/// agent's held trampoline waits for goes first (`process::spawn`). A
/// `go` dropped unfired closes the agent's stdin unwritten, so the
/// trampoline exits without its exec.
async fn write_loop(
    mut stdin: ChildStdin,
    mut lines: mpsc::Receiver<String>,
    pending: Pending,
    go: Option<oneshot::Receiver<()>>,
) {
    if let Some(go) = go {
        let released = async {
            go.await
                .map_err(|_| std::io::Error::other("the hold was dropped unreleased"))?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        };
        if let Err(e) = released.await {
            debug!(error = %e, "acp: the agent was never released");
            lines.close();
            fail_pending(&pending);
            return;
        }
    }
    while let Some(line) = lines.recv().await {
        let written = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        };
        if let Err(e) = written.await {
            warn!(error = %e, "acp: write failed");
            lines.close();
            fail_pending(&pending);
            break;
        }
    }
}

/// Removes a request's waiter when the request is dropped before its answer.
struct WaiterGuard<'a> {
    pending: &'a Pending,
    id: u64,
}

impl Drop for WaiterGuard<'_> {
    fn drop(&mut self) {
        take_waiter(self.pending, self.id);
    }
}

fn fail_pending(pending: &Pending) {
    if let Ok(mut guard) = pending.lock()
        && let Some(map) = guard.take()
    {
        for (_, sender) in map {
            let _ = sender.send(Err(AcpError::Closed));
        }
    }
}

fn take_waiter(pending: &Pending, id: u64) -> Option<Waiter> {
    pending
        .lock()
        .ok()
        .and_then(|mut guard| guard.as_mut().and_then(|map| map.remove(&id)))
}

fn parse_update(params: &Value) -> Option<SessionUpdate> {
    let raw = params.get("sessionId").and_then(Value::as_str)?;
    let session = match SessionId::parse(raw) {
        Ok(session) => session,
        Err(e) => {
            warn!(session = raw, error = %e, "acp: update names an invalid session; dropped");
            return None;
        }
    };
    let update = params.get("update").cloned().unwrap_or(Value::Null);
    Some(SessionUpdate { session, update })
}

/// An update waiting for `next_update`, with the bytes of the line it
/// came in.
struct Queued {
    update: SessionUpdate,
    bytes: usize,
}

/// Where the reader puts `session/update` notifications. The queue is
/// bounded by count and by the bytes of the lines the updates came in,
/// and a full queue drops the newest update and counts it, rather than
/// making the reader wait: a reader that waited on a caller who is not
/// draining updates would never deliver the response that caller is
/// waiting for. `session/load` replays a whole history as updates, so the
/// bound is what keeps an undrained client from growing without limit.
struct UpdateSink {
    tx: mpsc::Sender<Queued>,
    /// The bytes waiting, which `next_update` lowers as it takes them.
    queued_bytes: Arc<AtomicUsize>,
    budget: usize,
    dropped: Arc<AtomicU64>,
}

/// The receiving half of an `UpdateSink`: the queue, the bytes it holds
/// and the count of updates dropped.
type UpdateQueue = (mpsc::Receiver<Queued>, Arc<AtomicUsize>, Arc<AtomicU64>);

impl UpdateSink {
    fn new(capacity: usize, budget: usize) -> (Self, UpdateQueue) {
        let (tx, rx) = mpsc::channel(capacity);
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = UpdateSink {
            tx,
            queued_bytes: Arc::clone(&queued_bytes),
            budget,
            dropped: Arc::clone(&dropped),
        };
        (sink, (rx, queued_bytes, dropped))
    }

    /// Queues `update`, which came in a line of `bytes`, or drops it when
    /// the queue holds `capacity` updates or `budget` bytes would be
    /// passed. The reader is the only one that adds, so a check and then
    /// an add never let two updates past the budget together.
    fn offer(&self, update: SessionUpdate, bytes: usize) {
        if self
            .queued_bytes
            .load(Ordering::Acquire)
            .saturating_add(bytes)
            > self.budget
        {
            self.drop_newest();
            return;
        }
        self.queued_bytes.fetch_add(bytes, Ordering::AcqRel);
        match self.tx.try_send(Queued { update, bytes }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                self.drop_newest();
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                debug!("acp: update receiver dropped");
            }
        }
    }

    fn drop_newest(&self) {
        if self.dropped.fetch_add(1, Ordering::Relaxed) == 0 {
            warn!(
                buffer = UPDATE_BUFFER,
                bytes = self.budget,
                "acp: session updates are not being read; dropping the newest"
            );
        }
    }
}

/// The answer to a `session/request_permission` with `params`: cancelled
/// for a session whose turn was cancelled, else what `policy` picks.
fn permission_answer(policy: PermissionPolicy, cancelled: &Cancelled, params: &Value) -> Value {
    let session = params.get("sessionId").and_then(Value::as_str);
    // A poisoned set cannot say the session is live; answer cancelled.
    let is_cancelled = cancelled
        .lock()
        .map_or(true, |set| session.is_some_and(|s| set.contains(s)));
    if is_cancelled {
        return json!({ "outcome": { "outcome": "cancelled" } });
    }
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    policy.choose(&options)
}

async fn read_loop(
    stdout: ChildStdout,
    outgoing: Outgoing,
    pending: Pending,
    cancelled: Cancelled,
    updates: UpdateSink,
    policy: PermissionPolicy,
) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        let limit = u64::try_from(MAX_LINE)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        match (&mut reader).take(limit).read_until(b'\n', &mut line).await {
            Ok(0) => break,
            Ok(_) if line.len() > MAX_LINE => {
                warn!(
                    limit = MAX_LINE,
                    "acp: line from the agent is too long; closing"
                );
                break;
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "acp: read failed");
                break;
            }
        }
        if line.trim_ascii().is_empty() {
            continue;
        }
        let message: Incoming = match serde_json::from_slice(line.trim_ascii()) {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "acp: line is not JSON-RPC; ignored");
                continue;
            }
        };
        match (message.id, message.method) {
            (Some(id), Some(method)) => {
                let response = if method == "session/request_permission" {
                    let answer = permission_answer(policy, &cancelled, &message.params);
                    json!({ "jsonrpc": "2.0", "id": id, "result": answer })
                } else {
                    debug!(%method, "acp: agent request not supported; answering method not found");
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("method not supported by this client: {method}") }
                    })
                };
                if let Err(e) = write_line(&outgoing, &response) {
                    warn!(error = %e, "acp: cannot answer the agent; closing");
                    break;
                }
            }
            (Some(id), None) => {
                let Some(id) = id.as_u64() else {
                    warn!("acp: response with a non-numeric id; ignored");
                    continue;
                };
                let Some(sender) = take_waiter(&pending, id) else {
                    warn!(id, "acp: response to an unknown request; ignored");
                    continue;
                };
                let outcome = match (message.result, message.error) {
                    (_, Some(err)) => Err(AcpError::Rpc {
                        code: err.code,
                        message: err.message,
                    }),
                    (Some(result), None) => Ok(result),
                    (None, None) => Ok(Value::Null),
                };
                let _ = sender.send(outcome);
            }
            (None, Some(method)) => {
                if method == "session/update" {
                    if let Some(update) = parse_update(&message.params) {
                        updates.offer(update, line.len());
                    }
                } else {
                    debug!(%method, "acp: notification ignored");
                }
            }
            (None, None) => warn!("acp: message with neither id nor method; ignored"),
        }
    }
    fail_pending(&pending);
}

impl AcpClient {
    /// Takes over the agent's pipes and starts reading.
    #[must_use]
    pub fn attach(stdin: ChildStdin, stdout: ChildStdout, policy: PermissionPolicy) -> Self {
        Self::attach_with(stdin, stdout, policy, None)
    }

    /// `attach` for an agent `process::spawn` holds: nothing reaches its
    /// stdin before `go` fires, and then the line its trampoline waits
    /// for comes first.
    pub(crate) fn attach_held(
        stdin: ChildStdin,
        stdout: ChildStdout,
        policy: PermissionPolicy,
        go: oneshot::Receiver<()>,
    ) -> Self {
        Self::attach_with(stdin, stdout, policy, Some(go))
    }

    fn attach_with(
        stdin: ChildStdin,
        stdout: ChildStdout,
        policy: PermissionPolicy,
        go: Option<oneshot::Receiver<()>>,
    ) -> Self {
        let (outgoing, lines) = mpsc::channel(OUTGOING_BUFFER);
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let cancelled: Cancelled = Arc::new(Mutex::new(HashSet::new()));
        tokio::spawn(write_loop(stdin, lines, Arc::clone(&pending), go));
        let (sink, (rx, queued_bytes, dropped_updates)) =
            UpdateSink::new(UPDATE_BUFFER, UPDATE_BUFFER_BYTES);
        let reader = tokio::spawn(read_loop(
            stdout,
            outgoing.clone(),
            Arc::clone(&pending),
            Arc::clone(&cancelled),
            sink,
            policy,
        ));
        AcpClient {
            inner: Arc::new(Inner {
                outgoing,
                pending,
                cancelled,
                next_id: AtomicU64::new(1),
                updates: tokio::sync::Mutex::new(rx),
                queued_bytes,
                dropped_updates,
                reader,
            }),
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self
                .inner
                .pending
                .lock()
                .map_err(|_| AcpError::Protocol("pending table poisoned".to_owned()))?;
            match guard.as_mut() {
                Some(map) => {
                    map.insert(id, tx);
                }
                None => return Err(AcpError::Closed),
            }
        }
        let _guard = WaiterGuard {
            pending: &self.inner.pending,
            id,
        };
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        write_line(&self.inner.outgoing, &message)?;
        rx.await.map_err(|_| AcpError::Closed)?
    }

    /// A request that must be answered within `HANDSHAKE_TIMEOUT`.
    async fn handshake(&self, method: &'static str, params: Value) -> Result<Value, AcpError> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, self.request(method, params))
            .await
            .map_err(|_| AcpError::Timeout { method })?
    }

    fn notify(&self, method: &str, params: &Value) -> Result<(), AcpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        write_line(&self.inner.outgoing, &message)
    }

    /// Sends `initialize` and returns what the agent can do.
    pub async fn initialize(&self) -> Result<AgentCapabilities, AcpError> {
        let result = self
            .handshake(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": false, "writeTextFile": false },
                        "terminal": false
                    },
                    "clientInfo": { "name": "fabric", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
            .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                AcpError::Protocol("initialize result has no protocolVersion".to_owned())
            })?;
        if version != u64::from(PROTOCOL_VERSION) {
            return Err(AcpError::Version {
                agent: u16::try_from(version).unwrap_or(u16::MAX),
            });
        }
        let caps = result.get("agentCapabilities");
        let capabilities = AgentCapabilities {
            load_session: caps
                .and_then(|c| c.get("loadSession"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            resume: caps
                .and_then(|c| c.get("sessionCapabilities"))
                .and_then(|s| s.get("resume"))
                .is_some_and(|r| !r.is_null()),
        };
        Ok(capabilities)
    }

    /// Sends `session/new` for `cwd` and returns the new session id.
    pub async fn new_session(&self, cwd: &AbsPath) -> Result<SessionId, AcpError> {
        let result = self
            .handshake(
                "session/new",
                json!({ "cwd": cwd.as_str(), "mcpServers": [] }),
            )
            .await?;
        let id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpError::Protocol("session/new result has no sessionId".to_owned()))?;
        SessionId::parse(id).map_err(|e| AcpError::Protocol(format!("sessionId {id:?}: {e}")))
    }

    /// Sends `session/load`; the agent replays history as updates.
    pub async fn load_session(&self, session: &SessionId, cwd: &AbsPath) -> Result<(), AcpError> {
        self.handshake(
            "session/load",
            json!({ "sessionId": session.as_str(), "cwd": cwd.as_str(), "mcpServers": [] }),
        )
        .await?;
        Ok(())
    }

    /// Sends `session/resume`; no replay.
    pub async fn resume_session(&self, session: &SessionId, cwd: &AbsPath) -> Result<(), AcpError> {
        self.handshake(
            "session/resume",
            json!({ "sessionId": session.as_str(), "cwd": cwd.as_str(), "mcpServers": [] }),
        )
        .await?;
        Ok(())
    }

    /// Sends one text prompt and waits for the turn to end. A new turn
    /// lifts an earlier cancel of the session.
    pub async fn prompt(&self, session: &SessionId, text: &str) -> Result<StopReason, AcpError> {
        self.inner
            .cancelled
            .lock()
            .map_err(|_| AcpError::Protocol("cancelled set poisoned".to_owned()))?
            .remove(session.as_str());
        let result = self
            .request(
                "session/prompt",
                json!({
                    "sessionId": session.as_str(),
                    "prompt": [{ "type": "text", "text": text }]
                }),
            )
            .await?;
        let reason = result
            .get("stopReason")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AcpError::Protocol("session/prompt result has no stopReason".to_owned())
            })?;
        Ok(StopReason::parse(reason))
    }

    /// Queues `session/cancel`. From now until the next `prompt` of the
    /// session, its permission requests are answered `cancelled`.
    pub fn cancel(&self, session: &SessionId) -> Result<(), AcpError> {
        self.inner
            .cancelled
            .lock()
            .map_err(|_| AcpError::Protocol("cancelled set poisoned".to_owned()))?
            .insert(session.as_str().to_owned());
        self.notify("session/cancel", &json!({ "sessionId": session.as_str() }))
    }

    /// Waits for the next `session/update`; `None` once the agent has closed.
    /// At most `UPDATE_BUFFER` updates, and `UPDATE_BUFFER_BYTES` of their
    /// lines, wait here; see `dropped_updates`.
    pub async fn next_update(&self) -> Option<SessionUpdate> {
        let queued = self.inner.updates.lock().await.recv().await?;
        self.inner
            .queued_bytes
            .fetch_sub(queued.bytes, Ordering::AcqRel);
        Some(queued.update)
    }

    /// How many updates were dropped because `UPDATE_BUFFER` updates, or
    /// `UPDATE_BUFFER_BYTES` of their lines, were already waiting unread.
    #[must_use]
    pub fn dropped_updates(&self) -> u64 {
        self.inner.dropped_updates.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_never_grant_more_than_they_say() {
        let options = vec![
            json!({ "optionId": "a", "name": "Allow", "kind": "allow_once" }),
            json!({ "optionId": "r", "name": "Reject", "kind": "reject_once" }),
        ];
        assert_eq!(
            PermissionPolicy::Deny.choose(&options),
            json!({ "outcome": { "outcome": "selected", "optionId": "r" } })
        );
        assert_eq!(
            PermissionPolicy::AllowOnce.choose(&options),
            json!({ "outcome": { "outcome": "selected", "optionId": "a" } })
        );
        let only_always = vec![
            json!({ "optionId": "A", "name": "Always", "kind": "allow_always" }),
            json!({ "optionId": "r", "name": "Reject", "kind": "reject_once" }),
        ];
        assert_eq!(
            PermissionPolicy::AllowOnce.choose(&only_always),
            json!({ "outcome": { "outcome": "selected", "optionId": "r" } }),
            "once never becomes always"
        );
        let no_reject = vec![json!({ "optionId": "A", "name": "Always", "kind": "allow_always" })];
        assert_eq!(
            PermissionPolicy::Deny.choose(&no_reject),
            json!({ "outcome": { "outcome": "cancelled" } })
        );
        assert_eq!(
            PermissionPolicy::AllowOnce.choose(&no_reject),
            json!({ "outcome": { "outcome": "cancelled" } })
        );
    }

    #[test]
    fn a_cancelled_session_gets_no_permission_until_its_next_prompt() {
        let cancelled: Cancelled = Arc::new(Mutex::new(HashSet::new()));
        let params = json!({
            "sessionId": "s1",
            "options": [{ "optionId": "a", "name": "Allow", "kind": "allow_once" }]
        });
        let allowed = json!({ "outcome": { "outcome": "selected", "optionId": "a" } });
        let refused = json!({ "outcome": { "outcome": "cancelled" } });
        assert_eq!(
            permission_answer(PermissionPolicy::AllowOnce, &cancelled, &params),
            allowed
        );
        cancelled.lock().unwrap().insert("s1".to_owned());
        assert_eq!(
            permission_answer(PermissionPolicy::AllowOnce, &cancelled, &params),
            refused
        );
        let other = json!({ "sessionId": "s2", "options": params["options"].clone() });
        assert_eq!(
            permission_answer(PermissionPolicy::AllowOnce, &cancelled, &other),
            allowed
        );
        cancelled.lock().unwrap().remove("s1");
        assert_eq!(
            permission_answer(PermissionPolicy::AllowOnce, &cancelled, &params),
            allowed
        );
    }

    #[test]
    fn stop_reason_keeps_unknown_values() {
        assert_eq!(StopReason::parse("end_turn"), StopReason::EndTurn);
        assert_eq!(StopReason::parse("cancelled"), StopReason::Cancelled);
        assert_eq!(
            StopReason::parse("new_thing"),
            StopReason::Other("new_thing".to_owned())
        );
    }

    fn update(n: u64) -> SessionUpdate {
        SessionUpdate {
            session: SessionId::parse("s1").unwrap(),
            update: json!(n),
        }
    }

    #[test]
    fn a_full_update_queue_drops_the_newest_and_counts_it() {
        let (sink, (mut rx, bytes, dropped)) = UpdateSink::new(2, 1000);
        for n in 0..5 {
            sink.offer(update(n), 10);
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 3);
        assert_eq!(bytes.load(Ordering::Relaxed), 20);
        assert_eq!(rx.try_recv().unwrap().update.update, json!(0));
        assert_eq!(rx.try_recv().unwrap().update.update, json!(1));
        assert!(rx.try_recv().is_err());
    }

    /// A queue whose bytes would pass the budget drops the newest update
    /// and counts it; a smaller one that still fits is queued, and the
    /// longest line fits a queue that holds nothing.
    #[test]
    fn a_queue_full_of_bytes_drops_the_newest_and_counts_it() {
        let (sink, (mut rx, bytes, dropped)) = UpdateSink::new(100, 100);
        sink.offer(update(0), 60);
        sink.offer(update(1), 50);
        sink.offer(update(2), 40);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(bytes.load(Ordering::Relaxed), 100);
        let first = rx.try_recv().unwrap();
        assert_eq!((first.update.update, first.bytes), (json!(0), 60));
        assert_eq!(rx.try_recv().unwrap().update.update, json!(2));
        assert!(rx.try_recv().is_err());

        let (sink, (mut rx, _, dropped)) = UpdateSink::new(100, 100);
        sink.offer(update(3), 100);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(rx.try_recv().unwrap().update.update, json!(3));
    }

    #[tokio::test]
    async fn a_failed_write_fails_a_waiting_prompt() {
        // The agent closes its stdin but keeps its stdout open.
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exec 0<&-; sleep 30")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let client = AcpClient::attach(stdin, stdout, PermissionPolicy::Deny);
        let session = SessionId::parse("s1").unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(10), client.prompt(&session, "hi"))
            .await
            .expect("the prompt must fail, not wait for an answer that cannot come");
        assert!(answer.is_err());
        child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn an_agent_that_stops_reading_is_disconnected_not_buffered() {
        // The agent floods permission requests and never reads an answer.
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{"options":[]}}"#;
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("while :; do echo '{request}'; done"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let client = AcpClient::attach(stdin, stdout, PermissionPolicy::Deny);
        let session = SessionId::parse("s1").unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(20), client.prompt(&session, "hi"))
            .await
            .expect("the prompt must fail once the queue is full");
        assert!(answer.is_err());
        child.kill().await.unwrap();
    }

    /// M2-23: updates were bounded by count alone, 1024 of lines up to
    /// 16 MiB each, so an agent no one drained could make the client hold
    /// 16 GiB. Forty updates of 1 MiB each, none read, must leave at most
    /// `UPDATE_BUFFER_BYTES` of them queued and count the rest as dropped.
    #[tokio::test]
    async fn updates_are_bounded_by_bytes_as_well_as_by_count() {
        const UPDATES: usize = 40;
        let dir = tempfile::tempdir().unwrap();
        let text = "a".repeat(1024 * 1024);
        let mut line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text }
                }
            }
        }))
        .unwrap();
        line.push('\n');
        let update = dir.path().join("update.jsonl");
        std::fs::write(&update, &line).unwrap();
        // After the updates, a request: the agent reads the client's
        // answer, so every update before it has been offered by then.
        let request = r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"s1","options":[]}}"#;
        let script = format!(
            "i=0; while [ $i -lt {UPDATES} ]; do cat \"$0\"; i=$((i+1)); done; echo '{request}'; read -r answer"
        );
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg(&update)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let client = AcpClient::attach(stdin, stdout, PermissionPolicy::Deny);
        let status = tokio::time::timeout(Duration::from_mins(1), child.wait())
            .await
            .expect("the agent got no answer to its request")
            .unwrap();
        assert!(status.success(), "{status}");
        let dropped = usize::try_from(client.dropped_updates()).unwrap();
        let mut queued = 0;
        while let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_secs(5), client.next_update()).await
        {
            queued += 1;
        }
        assert_eq!(queued + dropped, UPDATES);
        assert!(
            queued * line.len() <= UPDATE_BUFFER_BYTES,
            "{queued} updates of {} bytes waited unread",
            line.len()
        );
        assert!(queued > 0, "an update that fits was dropped");
    }

    #[test]
    fn an_update_with_a_bad_session_id_is_dropped() {
        assert!(parse_update(&json!({ "sessionId": "../x", "update": {} })).is_none());
        assert!(parse_update(&json!({ "update": {} })).is_none());
        let update = parse_update(&json!({ "sessionId": "s1", "update": { "k": 1 } })).unwrap();
        assert_eq!(update.session.as_str(), "s1");
    }
}
