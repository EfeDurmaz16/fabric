//! A minimal Agent Client Protocol client over stdio JSON-RPC.
//!
//! Invariant: the client speaks only the baseline of ACP schema v1
//! (`initialize`, `session/new`, `session/load`, `session/resume`,
//! `session/prompt`, `session/cancel`, `session/update`), answers every
//! agent request so the agent never blocks, and grants a permission only
//! when the policy the caller chose says so. Deny is the default.
//!
//! Field names follow `schema/v1/schema.json` of
//! `agentclientprotocol/agent-client-protocol` at schema v1.26.0
//! (2026-10-16), read on 2026-10-19.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::session::SessionId;

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u16 = 1;

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
    /// `agentInfo.name`, when sent.
    pub name: Option<String>,
    /// `agentInfo.version`, when sent.
    pub version: Option<String>,
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
    pub session: String,
    /// The `update` object as the agent sent it.
    pub update: Value,
}

/// How the client answers `session/request_permission`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Pick a reject option; cancel when the agent offers none.
    #[default]
    Deny,
    /// Pick an `allow_once` option, else `allow_always`, else reject.
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
        let picked = match self {
            PermissionPolicy::AllowOnce => find("allow_once")
                .or_else(|| find("allow_always"))
                .or_else(|| find("reject_once"))
                .or_else(|| find("reject_always")),
            PermissionPolicy::Deny => find("reject_once").or_else(|| find("reject_always")),
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
type SharedStdin = Arc<tokio::sync::Mutex<ChildStdin>>;

struct Inner {
    stdin: SharedStdin,
    pending: Pending,
    next_id: AtomicU64,
    updates: tokio::sync::Mutex<mpsc::Receiver<SessionUpdate>>,
    capabilities: Mutex<Option<AgentCapabilities>>,
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

/// How many updates may queue before new ones are dropped with a warning.
const UPDATE_QUEUE: usize = 1024;

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

async fn write_line(stdin: &SharedStdin, value: &Value) -> Result<(), AcpError> {
    let mut text = serde_json::to_string(value).map_err(|e| AcpError::Protocol(e.to_string()))?;
    text.push('\n');
    let mut guard = stdin.lock().await;
    guard.write_all(text.as_bytes()).await?;
    guard.flush().await?;
    Ok(())
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

async fn read_loop(
    stdout: ChildStdout,
    stdin: SharedStdin,
    pending: Pending,
    updates: mpsc::Sender<SessionUpdate>,
    policy: PermissionPolicy,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "acp: read failed");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Incoming = match serde_json::from_str(&line) {
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
                if let Err(e) = write_line(&stdin, &response).await {
                    warn!(error = %e, "acp: write failed");
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
                    let session = message
                        .params
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let update = message.params.get("update").cloned().unwrap_or(Value::Null);
                    if updates.try_send(SessionUpdate { session, update }).is_err() {
                        warn!("acp: update queue full or closed; update dropped");
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
        let stdin: SharedStdin = Arc::new(tokio::sync::Mutex::new(stdin));
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (tx, rx) = mpsc::channel(UPDATE_QUEUE);
        let reader = tokio::spawn(read_loop(
            stdout,
            Arc::clone(&stdin),
            Arc::clone(&pending),
            tx,
            policy,
        ));
        AcpClient {
            inner: Arc::new(Inner {
                stdin,
                pending,
                next_id: AtomicU64::new(1),
                updates: tokio::sync::Mutex::new(rx),
                capabilities: Mutex::new(None),
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
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(e) = write_line(&self.inner.stdin, &message).await {
            take_waiter(&self.inner.pending, id);
            return Err(e);
        }
        rx.await.map_err(|_| AcpError::Closed)?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), AcpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        write_line(&self.inner.stdin, &message).await
    }

    /// Sends `initialize` and records what the agent can do.
    pub async fn initialize(&self) -> Result<AgentCapabilities, AcpError> {
        let result = self
            .request(
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
            name: result
                .pointer("/agentInfo/name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            version: result
                .pointer("/agentInfo/version")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        if let Ok(mut guard) = self.inner.capabilities.lock() {
            *guard = Some(capabilities.clone());
        }
        Ok(capabilities)
    }

    /// What `initialize` returned, if it has run.
    #[must_use]
    pub fn capabilities(&self) -> Option<AgentCapabilities> {
        self.inner.capabilities.lock().ok().and_then(|g| g.clone())
    }

    /// Sends `session/new` for `cwd` and returns the new session id.
    pub async fn new_session(&self, cwd: &str) -> Result<SessionId, AcpError> {
        let result = self
            .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await?;
        let id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpError::Protocol("session/new result has no sessionId".to_owned()))?;
        SessionId::parse(id).map_err(|e| AcpError::Protocol(format!("sessionId {id:?}: {e}")))
    }

    /// Sends `session/load`; the agent replays history as updates.
    pub async fn load_session(&self, session: &SessionId, cwd: &str) -> Result<(), AcpError> {
        self.request(
            "session/load",
            json!({ "sessionId": session.as_str(), "cwd": cwd, "mcpServers": [] }),
        )
        .await?;
        Ok(())
    }

    /// Sends `session/resume`; no replay.
    pub async fn resume_session(&self, session: &SessionId, cwd: &str) -> Result<(), AcpError> {
        self.request(
            "session/resume",
            json!({ "sessionId": session.as_str(), "cwd": cwd, "mcpServers": [] }),
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

    /// Sends `session/cancel`.
    pub async fn cancel(&self, session: &SessionId) -> Result<(), AcpError> {
        self.notify("session/cancel", json!({ "sessionId": session.as_str() }))
            .await
    }

    /// Waits for the next `session/update`; `None` once the agent has closed.
    pub async fn next_update(&self) -> Option<SessionUpdate> {
        self.inner.updates.lock().await.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_policy_never_picks_an_allow_option() {
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
        let only_allow = vec![json!({ "optionId": "a", "name": "Allow", "kind": "allow_always" })];
        assert_eq!(
            PermissionPolicy::Deny.choose(&only_allow),
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
}
