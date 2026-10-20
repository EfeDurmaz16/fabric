//! A minimal Agent Client Protocol client over stdio JSON-RPC.
//!
//! Invariant: the client speaks only the baseline of ACP schema v1
//! (`initialize`, `session/new`, `session/load`, `session/resume`,
//! `session/prompt`, `session/cancel`, `session/update`), answers every
//! agent request so the agent never blocks, and grants a permission only
//! when the policy the caller chose says so, never more than it says. Deny
//! is the default. At most `UPDATE_BUFFER` `session/update` notifications
//! wait for `next_update`; newer ones are dropped and counted in
//! `dropped_updates`, so the reader never waits on a caller who is not
//! draining updates. A handshake request has a
//! deadline, so a hung agent cannot hang `launch`. Every message reaches
//! the agent as one whole line written by a single writer task, so a
//! caller that drops a request mid-way cannot leave half a line on the
//! agent's stdin, and a dropped request leaves no waiter behind. A line
//! from the agent longer than `MAX_LINE` closes the connection.
//!
//! Field names follow `schema/v1/schema.json` of
//! `agentclientprotocol/agent-client-protocol` at schema v1.26.0
//! (2026-10-16), read on 2026-10-19.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fabric_core::path::AbsPath;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::session::SessionId;

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u16 = 1;

/// How long `initialize` and the session handshake may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);

/// The longest line read from the agent, in bytes.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// How many `session/update` notifications wait for `next_update` before
/// newer ones are dropped and counted.
pub const UPDATE_BUFFER: usize = 1024;

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
type Outgoing = mpsc::UnboundedSender<String>;

struct Inner {
    outgoing: Outgoing,
    pending: Pending,
    next_id: AtomicU64,
    updates: tokio::sync::Mutex<mpsc::Receiver<SessionUpdate>>,
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
    outgoing.send(text).map_err(|_| AcpError::Closed)
}

/// Writes queued lines to the agent until every sender is gone or a write
/// fails; then the agent's stdin is closed.
async fn write_loop(mut stdin: ChildStdin, mut lines: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = lines.recv().await {
        let written = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        };
        if let Err(e) = written.await {
            warn!(error = %e, "acp: write failed");
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

/// Where the reader puts `session/update` notifications. The queue is
/// bounded and a full queue drops the newest update and counts it, rather
/// than making the reader wait: a reader that waited on a caller who is
/// not draining updates would never deliver the response that caller is
/// waiting for. `session/load` replays a whole history as updates, so the
/// bound is what keeps an undrained client from growing without limit.
struct UpdateSink {
    tx: mpsc::Sender<SessionUpdate>,
    dropped: Arc<AtomicU64>,
}

impl UpdateSink {
    fn new(capacity: usize) -> (Self, mpsc::Receiver<SessionUpdate>, Arc<AtomicU64>) {
        let (tx, rx) = mpsc::channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = UpdateSink {
            tx,
            dropped: Arc::clone(&dropped),
        };
        (sink, rx, dropped)
    }

    fn offer(&self, update: SessionUpdate) {
        match self.tx.try_send(update) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                if self.dropped.fetch_add(1, Ordering::Relaxed) == 0 {
                    warn!(
                        buffer = UPDATE_BUFFER,
                        "acp: session updates are not being read; dropping the newest"
                    );
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => debug!("acp: update receiver dropped"),
        }
    }
}

async fn read_loop(
    stdout: ChildStdout,
    outgoing: Outgoing,
    pending: Pending,
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
                    let options = message
                        .params
                        .get("options")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    json!({ "jsonrpc": "2.0", "id": id, "result": policy.choose(&options) })
                } else {
                    debug!(%method, "acp: agent request not supported; answering method not found");
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("method not supported by this client: {method}") }
                    })
                };
                if write_line(&outgoing, &response).is_err() {
                    warn!("acp: writer closed; stopping");
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
                        updates.offer(update);
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
        let (outgoing, lines) = mpsc::unbounded_channel();
        tokio::spawn(write_loop(stdin, lines));
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (sink, rx, dropped_updates) = UpdateSink::new(UPDATE_BUFFER);
        let reader = tokio::spawn(read_loop(
            stdout,
            outgoing.clone(),
            Arc::clone(&pending),
            sink,
            policy,
        ));
        AcpClient {
            inner: Arc::new(Inner {
                outgoing,
                pending,
                next_id: AtomicU64::new(1),
                updates: tokio::sync::Mutex::new(rx),
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

    /// Sends one text prompt and waits for the turn to end.
    pub async fn prompt(&self, session: &SessionId, text: &str) -> Result<StopReason, AcpError> {
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

    /// Queues `session/cancel`.
    pub fn cancel(&self, session: &SessionId) -> Result<(), AcpError> {
        self.notify("session/cancel", &json!({ "sessionId": session.as_str() }))
    }

    /// Waits for the next `session/update`; `None` once the agent has closed.
    /// At most `UPDATE_BUFFER` updates wait here; see `dropped_updates`.
    pub async fn next_update(&self) -> Option<SessionUpdate> {
        self.inner.updates.lock().await.recv().await
    }

    /// How many updates were dropped because `UPDATE_BUFFER` were already
    /// waiting unread.
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
    fn stop_reason_keeps_unknown_values() {
        assert_eq!(StopReason::parse("end_turn"), StopReason::EndTurn);
        assert_eq!(StopReason::parse("cancelled"), StopReason::Cancelled);
        assert_eq!(
            StopReason::parse("new_thing"),
            StopReason::Other("new_thing".to_owned())
        );
    }

    #[test]
    fn a_full_update_queue_drops_the_newest_and_counts_it() {
        let (sink, mut rx, dropped) = UpdateSink::new(2);
        let update = |n: u64| SessionUpdate {
            session: SessionId::parse("s1").unwrap(),
            update: json!(n),
        };
        for n in 0..5 {
            sink.offer(update(n));
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 3);
        assert_eq!(rx.try_recv().unwrap().update, json!(0));
        assert_eq!(rx.try_recv().unwrap().update, json!(1));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn an_update_with_a_bad_session_id_is_dropped() {
        assert!(parse_update(&json!({ "sessionId": "../x", "update": {} })).is_none());
        assert!(parse_update(&json!({ "update": {} })).is_none());
        let update = parse_update(&json!({ "sessionId": "s1", "update": { "k": 1 } })).unwrap();
        assert_eq!(update.session.as_str(), "s1");
    }
}
