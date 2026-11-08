//! Facade protocol v1 (ADR 0027, docs/facade-protocol.md): the messages a
//! client sends a managed job's authority through any registered node, and
//! the answers it gets back.
//!
//! Invariant: the bytes a client writes are the bytes the authority reads;
//! no node in between parses or rewrites them. Both ends parse here. The
//! authority side ([`ClientMessage::from_json`]) refuses in the order the
//! protocol fixes (schema, then version; the token and the scope are the
//! authority's own checks) and names each refusal with the `code` and
//! `reason` of the `refused` answer, so the fixtures in
//! `protocol/facade/v1` pin every outcome. The client side
//! ([`FacadeAnswer::from_json`]) skips fields it does not know, so an
//! authority may add fields within v1, and refuses an unknown message type
//! or refusal code as a protocol violation, never as an answer it did not
//! understand. Counters cross as canonical decimal strings; a JSON number
//! where a counter belongs is a `bad_integer` on either side.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::{EventSeq, OperationId, OperationKey, TaskId, WorkRevision};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api::{
    FidelityName, HarnessDef, Inspection, ManagedPolicy, ManagedSource, SecretRefWire,
};

/// The protocol version this module speaks; every message carries it as `facade`.
pub const FACADE_VERSION: u64 = 1;
/// The longest a watch may wait for a first event, in milliseconds.
pub const MAX_WAIT_MS: u32 = 25_000;
/// How long a watch waits when the client gave no `wait_ms`.
pub const DEFAULT_WAIT_MS: u32 = 10_000;
/// The most events one `events` answer carries, and the authority's broadcast window per job.
pub const WATCH_WINDOW: usize = 256;
/// The most bytes one free-text field may hold.
pub const TEXT_MAX_BYTES: usize = 16 * 1024;
/// The most bytes a facade message may take on the node link.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// The most bytes a facade answer may take on the node link.
pub const MAX_ANSWER_BYTES: usize = 16 * 1024 * 1024;
/// The bytes of a bearer secret; it travels as twice as many lowercase hex characters.
pub const BEARER_BYTES: usize = 32;
/// The most bytes a git URI may hold on the wire.
const URI_MAX_BYTES: usize = 2048;

/// A bearer secret as a message carries it. It prints redacted and never
/// serializes by accident: [`Bearer::to_hex`] is the one way out.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Bearer([u8; BEARER_BYTES]);

impl Bearer {
    /// Wraps the bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; BEARER_BYTES]) -> Bearer {
        Bearer(bytes)
    }

    /// Reads the 64 lowercase hex characters of a bearer.
    pub fn parse_hex(text: &str) -> Result<Bearer, BearerError> {
        if text.len() != BEARER_BYTES * 2
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(BearerError);
        }
        let mut out = [0u8; BEARER_BYTES];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| BearerError)?;
        }
        Ok(Bearer(out))
    }

    /// The bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; BEARER_BYTES] {
        &self.0
    }

    /// The 64 lowercase hex characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
}

impl fmt::Debug for Bearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bearer(<redacted>)")
    }
}

/// The text was not 64 lowercase hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a bearer is 64 lowercase hex characters")]
pub struct BearerError;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The `code` of a `refused` answer: what a client switches on first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    /// The `facade` field is not 1.
    UnsupportedVersion,
    /// The message does not parse under docs/facade-protocol.md.
    BadRequest,
    /// The token is unknown, revoked or expired.
    Unauthorized,
    /// The token does not cover the job or the resource.
    Scope,
    /// The job is not in this authority's journal.
    UnknownJob,
    /// The key was used for another request.
    Conflict,
    /// `expected_revision` is not the job's current one.
    RevisionMismatch,
    /// The domain refused the command.
    Refused,
    /// The authority is not open or the link to it failed.
    Unavailable,
}

impl RefusalCode {
    /// Every code, as docs/facade-protocol.md lists them.
    pub const ALL: [RefusalCode; 9] = [
        RefusalCode::UnsupportedVersion,
        RefusalCode::BadRequest,
        RefusalCode::Unauthorized,
        RefusalCode::Scope,
        RefusalCode::UnknownJob,
        RefusalCode::Conflict,
        RefusalCode::RevisionMismatch,
        RefusalCode::Refused,
        RefusalCode::Unavailable,
    ];

    /// The name on the wire.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            RefusalCode::UnsupportedVersion => "unsupported_version",
            RefusalCode::BadRequest => "bad_request",
            RefusalCode::Unauthorized => "unauthorized",
            RefusalCode::Scope => "scope",
            RefusalCode::UnknownJob => "unknown_job",
            RefusalCode::Conflict => "conflict",
            RefusalCode::RevisionMismatch => "revision_mismatch",
            RefusalCode::Refused => "refused",
            RefusalCode::Unavailable => "unavailable",
        }
    }

    fn parse(name: &str) -> Option<RefusalCode> {
        RefusalCode::ALL
            .into_iter()
            .find(|code| code.name() == name)
    }
}

impl fmt::Display for RefusalCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A `refused` answer: the code and reason a client switches on, a message
/// for a person, and the current revision after a `revision_mismatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The code.
    pub code: RefusalCode,
    /// The reason under the code.
    pub reason: String,
    /// For a person; never compared.
    pub message: String,
    /// The job's revision, present exactly for `revision_mismatch`.
    pub current_revision: Option<WorkRevision>,
}

impl Refusal {
    /// A refusal with the message `"{code}: {reason}"`.
    #[must_use]
    pub fn new(code: RefusalCode, reason: impl Into<String>) -> Refusal {
        let reason = reason.into();
        Refusal {
            message: format!("{code}: {reason}"),
            code,
            reason,
            current_revision: None,
        }
    }

    /// `bad_request` with `reason`.
    #[must_use]
    pub fn bad_request(reason: &'static str) -> Refusal {
        Refusal::new(RefusalCode::BadRequest, reason)
    }

    /// `revision_mismatch` naming the job's revision.
    #[must_use]
    pub fn revision_mismatch(current: WorkRevision) -> Refusal {
        Refusal {
            current_revision: Some(current),
            ..Refusal::new(RefusalCode::RevisionMismatch, "revision")
        }
    }

    /// The same refusal with another message for a person.
    #[must_use]
    pub fn saying(mut self, message: impl Into<String>) -> Refusal {
        self.message = message.into();
        self
    }

    /// The answer as JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("type".into(), json!("refused"));
        map.insert("facade".into(), json!(FACADE_VERSION));
        map.insert("code".into(), json!(self.code.name()));
        map.insert("reason".into(), json!(self.reason));
        map.insert("message".into(), json!(self.message));
        if let Some(current) = self.current_revision {
            map.insert("current_revision".into(), json!(current.to_string()));
        }
        Value::Object(map)
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why a client did not read an answer: the frame was not an answer of
/// this protocol. `reason` is the name the fixtures use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("protocol violation ({reason}): {detail}")]
pub struct ProtocolViolation {
    /// `unknown_variant`, `bad_integer`, `missing_field`, `bad_id` or `bad_string`.
    pub reason: &'static str,
    /// What was found.
    pub detail: String,
}

impl ProtocolViolation {
    fn new(reason: &'static str, detail: impl Into<String>) -> ProtocolViolation {
        ProtocolViolation {
            reason,
            detail: detail.into(),
        }
    }
}

/// What a `submit` asks for, which the local socket's `managed_submit` shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submit {
    /// The goal in the user's words.
    pub goal: String,
    /// The harness the primary task runs under.
    pub harness: HarnessKind,
    /// What the work must satisfy.
    pub acceptance: Vec<String>,
    /// Where the workspace comes from; over the facade never `repository` or `directory`.
    pub source: ManagedSource,
    /// The policy, when the client set any field.
    pub policy: Option<ManagedPolicy>,
}

/// A message from a client to the authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// Makes a managed job; the authority places or parks it after the commit.
    Submit {
        /// The bearer.
        token: Bearer,
        /// The caller's key for the command.
        key: OperationKey,
        /// What to make.
        submit: Submit,
    },
    /// Everything the journal holds about a job.
    Inspect {
        /// The bearer.
        token: Bearer,
        /// The job.
        job: JobId,
    },
    /// A bounded page of events after a cursor.
    Watch {
        /// The bearer.
        token: Bearer,
        /// The job.
        job: JobId,
        /// The last sequence the client has; `None` for the first event.
        after: Option<EventSeq>,
        /// How long to wait for a first event; the default when absent.
        wait_ms: Option<u32>,
    },
    /// Cancels the job or one task.
    Cancel {
        /// The bearer.
        token: Bearer,
        /// The caller's key.
        key: OperationKey,
        /// The job.
        job: JobId,
        /// The revision expected, or `None` for any.
        expected_revision: Option<WorkRevision>,
        /// The task; `None` cancels the job.
        task: Option<TaskId>,
        /// Why.
        reason: String,
    },
    /// A checkpoint cut of the live attempt.
    Checkpoint {
        /// The bearer.
        token: Bearer,
        /// The caller's key.
        key: OperationKey,
        /// The job.
        job: JobId,
        /// The revision expected, or `None` for any.
        expected_revision: Option<WorkRevision>,
    },
    /// The planned handoff.
    Continue {
        /// The bearer.
        token: Bearer,
        /// The caller's key.
        key: OperationKey,
        /// The job.
        job: JobId,
        /// The revision expected, or `None` for any.
        expected_revision: Option<WorkRevision>,
        /// The fidelities allowed; the job's allowance when `None`.
        fidelity: Option<Vec<FidelityName>>,
        /// The one node the new attempt may run on.
        pin: Option<NodeId>,
    },
}

/// The message types a client sends, in the protocol's order.
pub const CLIENT_TYPES: [&str; 6] = [
    "submit",
    "inspect",
    "watch",
    "cancel",
    "checkpoint",
    "continue",
];

impl ClientMessage {
    /// The message's type on the wire.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            ClientMessage::Submit { .. } => "submit",
            ClientMessage::Inspect { .. } => "inspect",
            ClientMessage::Watch { .. } => "watch",
            ClientMessage::Cancel { .. } => "cancel",
            ClientMessage::Checkpoint { .. } => "checkpoint",
            ClientMessage::Continue { .. } => "continue",
        }
    }

    /// The bearer the message carries.
    #[must_use]
    pub fn token(&self) -> &Bearer {
        match self {
            ClientMessage::Submit { token, .. }
            | ClientMessage::Inspect { token, .. }
            | ClientMessage::Watch { token, .. }
            | ClientMessage::Cancel { token, .. }
            | ClientMessage::Checkpoint { token, .. }
            | ClientMessage::Continue { token, .. } => token,
        }
    }

    /// The job the message names; `None` for `submit`.
    #[must_use]
    pub fn job(&self) -> Option<JobId> {
        match self {
            ClientMessage::Submit { .. } => None,
            ClientMessage::Inspect { job, .. }
            | ClientMessage::Watch { job, .. }
            | ClientMessage::Cancel { job, .. }
            | ClientMessage::Checkpoint { job, .. }
            | ClientMessage::Continue { job, .. } => Some(*job),
        }
    }

    /// The caller's key; `None` for a read.
    #[must_use]
    pub fn key(&self) -> Option<&OperationKey> {
        match self {
            ClientMessage::Submit { key, .. }
            | ClientMessage::Cancel { key, .. }
            | ClientMessage::Checkpoint { key, .. }
            | ClientMessage::Continue { key, .. } => Some(key),
            ClientMessage::Inspect { .. } | ClientMessage::Watch { .. } => None,
        }
    }

    /// What the message asks of the token's job scope: a submit creates a
    /// job, every other message names one.
    #[must_use]
    pub fn job_ask(&self) -> fabric_core::managed::facade::JobAsk {
        use fabric_core::managed::facade::JobAsk;
        match self {
            ClientMessage::Submit { .. } => JobAsk::Create,
            other => other.job().map_or(JobAsk::Nothing, JobAsk::Existing),
        }
    }

    /// What the message asks of the token's resource scope: a submit or a
    /// continue places work, on its pin or wherever the authority chooses.
    #[must_use]
    pub fn resource_ask(&self) -> fabric_core::managed::facade::ResourceAsk {
        use fabric_core::managed::facade::ResourceAsk;
        match self {
            ClientMessage::Submit { .. } | ClientMessage::Continue { .. } => self
                .pin()
                .map_or(ResourceAsk::Anywhere, ResourceAsk::Pinned),
            _ => ResourceAsk::Nothing,
        }
    }

    /// The node the message names as a resource: a submit's or a continue's pin.
    #[must_use]
    pub fn pin(&self) -> Option<NodeId> {
        match self {
            ClientMessage::Submit { submit, .. } => submit.policy.as_ref().and_then(|p| p.pin),
            ClientMessage::Continue { pin, .. } => *pin,
            _ => None,
        }
    }

    /// Reads one message as the authority does: schema first, then the
    /// version. The token and the scope are checked by the authority
    /// after this. A refusal here is the `refused` answer to send.
    pub fn from_json(value: &Value) -> Result<ClientMessage, Refusal> {
        let mut obj = Obj::new(value, Refusal::bad_request)?;
        let kind = obj.str("type")?.to_owned();
        if !CLIENT_TYPES.contains(&kind.as_str()) {
            return Err(Refusal::bad_request("unknown_variant").saying(format!(
                "{kind} is not a facade message; the facade takes {}",
                CLIENT_TYPES.join(", ")
            )));
        }
        let version = obj.u64("facade")?;
        let token = Bearer::parse_hex(obj.str("token")?)
            .map_err(|_| Refusal::bad_request("bad_string").saying("token: 64 lowercase hex"))?;
        let message = match kind.as_str() {
            "submit" => ClientMessage::Submit {
                token,
                key: obj.key()?,
                submit: parse_submit(&mut obj)?,
            },
            "inspect" => ClientMessage::Inspect {
                token,
                job: obj.id("job")?,
            },
            "watch" => {
                let job = obj.id("job")?;
                let after = obj.counter_nullable("after")?;
                let wait_ms = match obj.optional("wait_ms") {
                    None => None,
                    Some(wait) => {
                        let wait = integer(wait, "wait_ms", Refusal::bad_request)?;
                        let wait = u32::try_from(wait)
                            .ok()
                            .filter(|wait| *wait <= MAX_WAIT_MS)
                            .ok_or_else(|| {
                                Refusal::bad_request("wait_above_limit")
                                    .saying(format!("wait_ms is at most {MAX_WAIT_MS}"))
                            })?;
                        Some(wait)
                    }
                };
                ClientMessage::Watch {
                    token,
                    job,
                    after,
                    wait_ms,
                }
            }
            "cancel" => ClientMessage::Cancel {
                token,
                key: obj.key()?,
                job: obj.id("job")?,
                expected_revision: obj.counter_nullable("expected_revision")?,
                task: obj.id_nullable("task")?,
                reason: obj.text("reason")?,
            },
            "checkpoint" => ClientMessage::Checkpoint {
                token,
                key: obj.key()?,
                job: obj.id("job")?,
                expected_revision: obj.counter_nullable("expected_revision")?,
            },
            _ => ClientMessage::Continue {
                token,
                key: obj.key()?,
                job: obj.id("job")?,
                expected_revision: obj.counter_nullable("expected_revision")?,
                fidelity: match obj.optional("fidelity") {
                    None => None,
                    Some(list) => Some(fidelities(list, Refusal::bad_request)?),
                },
                pin: obj.id_nullable("pin")?,
            },
        };
        obj.finish()?;
        if version != FACADE_VERSION {
            return Err(
                Refusal::new(RefusalCode::UnsupportedVersion, "facade").saying(format!(
                    "this authority speaks facade {FACADE_VERSION}, the message says {version}"
                )),
            );
        }
        Ok(message)
    }

    /// The message as JSON, the bytes a client sends.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("type".into(), json!(self.kind()));
        map.insert("facade".into(), json!(FACADE_VERSION));
        map.insert("token".into(), json!(self.token().to_hex()));
        if let Some(key) = self.key() {
            map.insert("key".into(), json!(key.as_str()));
        }
        match self {
            ClientMessage::Submit { submit, .. } => {
                map.insert("intent".into(), intent_json(submit));
                map.insert("source".into(), to_value(&submit.source));
                if let Some(policy) = &submit.policy {
                    map.insert("policy".into(), to_value(policy));
                }
            }
            ClientMessage::Inspect { job, .. } => {
                map.insert("job".into(), json!(job.to_string()));
            }
            ClientMessage::Watch {
                job,
                after,
                wait_ms,
                ..
            } => {
                map.insert("job".into(), json!(job.to_string()));
                map.insert("after".into(), counter_json(*after));
                if let Some(wait) = wait_ms {
                    map.insert("wait_ms".into(), json!(wait));
                }
            }
            ClientMessage::Cancel {
                job,
                expected_revision,
                task,
                reason,
                ..
            } => {
                map.insert("job".into(), json!(job.to_string()));
                map.insert("expected_revision".into(), counter_json(*expected_revision));
                map.insert(
                    "task".into(),
                    task.map_or(Value::Null, |t| json!(t.to_string())),
                );
                map.insert("reason".into(), json!(reason));
            }
            ClientMessage::Checkpoint {
                job,
                expected_revision,
                ..
            } => {
                map.insert("job".into(), json!(job.to_string()));
                map.insert("expected_revision".into(), counter_json(*expected_revision));
            }
            ClientMessage::Continue {
                job,
                expected_revision,
                fidelity,
                pin,
                ..
            } => {
                map.insert("job".into(), json!(job.to_string()));
                map.insert("expected_revision".into(), counter_json(*expected_revision));
                if let Some(fidelity) = fidelity {
                    map.insert("fidelity".into(), to_value(fidelity));
                }
                map.insert(
                    "pin".into(),
                    pin.map_or(Value::Null, |n| json!(n.to_string())),
                );
            }
        }
        Value::Object(map)
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    // Every wire type here serializes without failing: strings, numbers,
    // lists and maps of them.
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn counter_json<T: fmt::Display>(counter: Option<T>) -> Value {
    counter.map_or(Value::Null, |c| json!(c.to_string()))
}

fn intent_json(submit: &Submit) -> Value {
    let mut map = Map::new();
    map.insert("goal".into(), json!(submit.goal));
    map.insert("harness".into(), harness_json(&submit.harness));
    map.insert("acceptance".into(), json!(submit.acceptance));
    Value::Object(map)
}

/// A harness in the client API's form.
#[must_use]
pub fn harness_json(harness: &HarnessKind) -> Value {
    #[derive(Serialize)]
    struct Wire<'a>(#[serde(with = "HarnessDef")] &'a HarnessKind);
    to_value(&Wire(harness))
}

fn parse_submit(obj: &mut Obj<'_>) -> Result<Submit, Refusal> {
    let mut intent = obj.object("intent")?;
    let goal = intent.text("goal")?;
    let harness = {
        #[derive(Deserialize)]
        struct Wire(#[serde(with = "HarnessDef")] HarnessKind);
        let value = intent.required("harness")?;
        Wire::deserialize(value)
            .map(|wire| wire.0)
            .map_err(|e| Refusal::bad_request(serde_reason(&e)).saying(format!("harness: {e}")))?
    };
    let acceptance = match intent.optional("acceptance") {
        None => Vec::new(),
        Some(list) => list
            .as_array()
            .ok_or_else(|| Refusal::bad_request("bad_string").saying("acceptance: a list"))?
            .iter()
            .map(|item| text_value(item, "acceptance", Refusal::bad_request))
            .collect::<Result<Vec<String>, Refusal>>()?,
    };
    intent.finish()?;

    let mut source = obj.object("source")?;
    let source = match source.str("kind")? {
        "git" => {
            let uri = source.str("uri")?.to_owned();
            if uri.len() > URI_MAX_BYTES || uri.bytes().any(|b| b < 0x20 || b == 0x7f) {
                return Err(Refusal::bad_request("bad_string").saying("source.uri"));
            }
            let commit = source.str("commit")?.to_owned();
            source.finish()?;
            ManagedSource::Git { uri, commit }
        }
        "snapshot" => {
            let snapshot = source.id("snapshot")?;
            source.finish()?;
            ManagedSource::Snapshot { snapshot }
        }
        "empty" => {
            source.finish()?;
            ManagedSource::Empty
        }
        "repository" | "directory" => {
            return Err(Refusal::bad_request("source_local").saying(
                "a repository or directory is a path on one node; the facade takes git, snapshot or empty",
            ));
        }
        other => {
            return Err(Refusal::bad_request("unknown_variant")
                .saying(format!("source.kind {other}: git, snapshot or empty")));
        }
    };

    let policy = match obj.optional("policy") {
        None => None,
        Some(value) => Some(parse_policy(value)?),
    };
    Ok(Submit {
        goal,
        harness,
        acceptance,
        source,
        policy,
    })
}

fn parse_policy(value: &Value) -> Result<ManagedPolicy, Refusal> {
    let mut obj = Obj::new(value, Refusal::bad_request)?;
    let drain_budget_ms = match obj.optional("drain_budget_ms") {
        None => None,
        Some(v) => Some(
            u32::try_from(integer(v, "drain_budget_ms", Refusal::bad_request)?)
                .map_err(|_| Refusal::bad_request("bad_integer").saying("drain_budget_ms"))?,
        ),
    };
    let allowed_fidelity = match obj.optional("allowed_fidelity") {
        None => None,
        Some(list) => Some(fidelities(list, Refusal::bad_request)?),
    };
    let secrets = match obj.optional("secrets") {
        None => None,
        Some(list) => {
            let items = list
                .as_array()
                .ok_or_else(|| Refusal::bad_request("bad_string").saying("secrets: a list"))?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let mut secret = Obj::new(item, Refusal::bad_request)?;
                let name = secret.str("name")?.to_owned();
                if name.is_empty()
                    || name.len() > 64
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                {
                    return Err(Refusal::bad_request("bad_string")
                        .saying("a secret name is 1 to 64 characters of [A-Z0-9_]"));
                }
                let attempt = secret.str("attempt")?.to_owned();
                if attempt != "any" && attempt != "current" {
                    return Err(Refusal::bad_request("unknown_variant")
                        .saying(format!("secret attempt scope {attempt}: any or current")));
                }
                let kinds = secret.required("resource_kinds")?;
                let kinds = kinds
                    .as_array()
                    .ok_or_else(|| Refusal::bad_request("bad_string").saying("resource_kinds"))?;
                let mut resource_kinds = Vec::with_capacity(kinds.len());
                for kind in kinds {
                    match kind.as_str() {
                        Some("node") => resource_kinds.push("node".to_owned()),
                        Some(other) => {
                            return Err(Refusal::bad_request("unknown_variant")
                                .saying(format!("resource kind {other}: node")));
                        }
                        None => {
                            return Err(Refusal::bad_request("bad_string").saying("resource_kinds"));
                        }
                    }
                }
                secret.finish()?;
                out.push(SecretRefWire {
                    name,
                    attempt,
                    resource_kinds,
                });
            }
            Some(out)
        }
    };
    let export_allowlist = match obj.optional("export_allowlist") {
        None => None,
        Some(list) => Some(
            list.as_array()
                .ok_or_else(|| Refusal::bad_request("bad_string").saying("export_allowlist"))?
                .iter()
                .map(|item| text_value(item, "export_allowlist", Refusal::bad_request))
                .collect::<Result<Vec<String>, Refusal>>()?,
        ),
    };
    let disk_bytes = match obj.optional("disk_bytes") {
        None => None,
        Some(v) => Some(integer(v, "disk_bytes", Refusal::bad_request)?),
    };
    let pin = obj.id_nullable("pin")?;
    let recovery = match obj.optional("recovery") {
        None => None,
        Some(v) => Some(match v.as_str() {
            Some("block") => crate::api::RecoveryName::Block,
            Some("restart_from_acknowledged") => crate::api::RecoveryName::RestartFromAcknowledged,
            _ => return Err(Refusal::bad_request("bad_policy").saying("recovery")),
        }),
    };
    obj.finish()?;
    Ok(ManagedPolicy {
        drain_budget_ms,
        allowed_fidelity,
        secrets,
        export_allowlist,
        disk_bytes,
        pin,
        recovery,
    })
}

fn fidelities<E>(list: &Value, make: fn(&'static str) -> E) -> Result<Vec<FidelityName>, E> {
    let items = list.as_array().ok_or_else(|| make("bad_string"))?;
    if items.is_empty() {
        return Err(make("empty_fidelity"));
    }
    items
        .iter()
        .map(|item| {
            let name = item.as_str().ok_or_else(|| make("bad_string"))?;
            FidelityName::ALL
                .into_iter()
                .find(|f| f.name() == name)
                .ok_or_else(|| make("unknown_variant"))
        })
        .collect()
}

/// The protocol's name for a serde error, from its text.
fn serde_reason(e: &serde_json::Error) -> &'static str {
    let text = e.to_string();
    if text.starts_with("unknown variant") {
        "unknown_variant"
    } else if text.starts_with("unknown field") {
        "unknown_field"
    } else if text.starts_with("missing field") {
        "missing_field"
    } else {
        "bad_string"
    }
}

/// An integer of the wire: a JSON integer in `0..=2^53-1`.
fn integer<E>(value: &Value, field: &str, make: fn(&'static str) -> E) -> Result<u64, E> {
    let _ = field;
    value
        .as_u64()
        .filter(|n| *n < (1_u64 << 53))
        .ok_or_else(|| make("bad_integer"))
}

/// A canonical decimal string as a counter.
fn counter<T: FromStr, E>(value: &Value, make: fn(&'static str) -> E) -> Result<T, E> {
    value
        .as_str()
        .and_then(|text| text.parse::<T>().ok())
        .ok_or_else(|| make("bad_integer"))
}

/// Free text: no control character but `\n` and `\t`, at most `TEXT_MAX_BYTES`.
fn text_value<E>(value: &Value, field: &str, make: fn(&'static str) -> E) -> Result<String, E> {
    let _ = field;
    let text = value.as_str().ok_or_else(|| make("bad_string"))?;
    if text.len() > TEXT_MAX_BYTES
        || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(make("bad_string"));
    }
    Ok(text.to_owned())
}

/// One JSON object being read field by field; what is left at the end is unknown.
struct Obj<'a> {
    map: &'a Map<String, Value>,
    seen: BTreeSet<&'a str>,
    make: fn(&'static str) -> Refusal,
}

impl<'a> Obj<'a> {
    fn new(value: &'a Value, make: fn(&'static str) -> Refusal) -> Result<Self, Refusal> {
        let map = value
            .as_object()
            .ok_or_else(|| make("bad_string").saying("a facade message is a JSON object"))?;
        Ok(Obj {
            map,
            seen: BTreeSet::new(),
            make,
        })
    }

    fn optional(&mut self, name: &'static str) -> Option<&'a Value> {
        self.seen.insert(name);
        self.map.get(name)
    }

    fn required(&mut self, name: &'static str) -> Result<&'a Value, Refusal> {
        self.optional(name)
            .ok_or_else(|| (self.make)("missing_field").saying(format!("{name} is required")))
    }

    fn str(&mut self, name: &'static str) -> Result<&'a str, Refusal> {
        let value = self.required(name)?;
        value
            .as_str()
            .ok_or_else(|| (self.make)("bad_string").saying(format!("{name}: a string")))
    }

    fn text(&mut self, name: &'static str) -> Result<String, Refusal> {
        let value = self.required(name)?;
        text_value(value, name, self.make).map_err(|e| e.saying(format!("{name}: free text")))
    }

    fn u64(&mut self, name: &'static str) -> Result<u64, Refusal> {
        let value = self.required(name)?;
        integer(value, name, self.make).map_err(|e| e.saying(format!("{name}: an integer")))
    }

    fn id<T: FromStr>(&mut self, name: &'static str) -> Result<T, Refusal> {
        let text = self.str(name)?;
        text.parse::<T>()
            .map_err(|_| (self.make)("bad_id").saying(format!("{name}: not an id")))
    }

    fn id_nullable<T: FromStr>(&mut self, name: &'static str) -> Result<Option<T>, Refusal> {
        match self.required(name)? {
            Value::Null => Ok(None),
            Value::String(text) => text
                .parse::<T>()
                .map(Some)
                .map_err(|_| (self.make)("bad_id").saying(format!("{name}: not an id"))),
            _ => Err((self.make)("bad_id").saying(format!("{name}: an id or null"))),
        }
    }

    fn key(&mut self) -> Result<OperationKey, Refusal> {
        let text = self.str("key")?;
        OperationKey::parse(text).map_err(|e| (self.make)("bad_string").saying(format!("key: {e}")))
    }

    fn counter_nullable<T: FromStr>(&mut self, name: &'static str) -> Result<Option<T>, Refusal> {
        match self.required(name)? {
            Value::Null => Ok(None),
            value => counter(value, self.make)
                .map(Some)
                .map_err(|e| e.saying(format!("{name}: a canonical decimal string or null"))),
        }
    }

    fn object(&mut self, name: &'static str) -> Result<Obj<'a>, Refusal> {
        let value = self.required(name)?;
        Obj::new(value, self.make).map_err(|e| e.saying(format!("{name}: an object")))
    }

    fn finish(self) -> Result<(), Refusal> {
        match self.map.keys().find(|k| !self.seen.contains(k.as_str())) {
            Some(unknown) => Err((self.make)("unknown_field")
                .saying(format!("{unknown} is not a field of this message"))),
            None => Ok(()),
        }
    }
}

/// One event in an `events` answer: the journal's sequence and the
/// event as the client API writes it. The event keeps its JSON, with its
/// `kind` read, so a client follows a job whose newer events it does not
/// know by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventWire {
    /// The sequence, the cursor to ask after.
    #[serde(with = "crate::text")]
    pub seq: EventSeq,
    /// The job's revision the event was recorded at.
    #[serde(with = "crate::text")]
    pub revision: WorkRevision,
    /// When it was recorded, Unix milliseconds.
    pub at_ms: u64,
    /// The event, with a `kind`.
    pub event: Map<String, Value>,
}

impl EventWire {
    /// The event's kind.
    #[must_use]
    pub fn kind(&self) -> &str {
        self.event.get("kind").and_then(Value::as_str).unwrap_or("")
    }
}

/// An answer from the authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacadeAnswer {
    /// A command is durable.
    Accepted {
        /// The operation.
        operation: OperationId,
        /// The key it was accepted under.
        key: OperationKey,
        /// Whether this is the stored answer of a key seen before.
        replayed: bool,
        /// The job.
        job: JobId,
        /// The job's revision after the command.
        revision: WorkRevision,
    },
    /// The answer to `inspect`.
    Inspection(Box<Inspection>),
    /// The answer to `watch`.
    Events {
        /// The job.
        job: JobId,
        /// At most `WATCH_WINDOW` events after the cursor, in order.
        events: Vec<EventWire>,
        /// The cursor to ask after next; `None` only when the job has no event yet.
        next: Option<EventSeq>,
        /// Whether the reader fell behind the window; `next` is still right.
        lagged: bool,
        /// `revoked` or `finished` when no event will follow under this watch.
        closed: Option<String>,
    },
    /// The authority refused the message.
    Refused(Refusal),
}

/// The answer types, in the protocol's order.
pub const ANSWER_TYPES: [&str; 4] = ["accepted", "inspection", "events", "refused"];

/// The fields whose value is a counter: a canonical decimal string, never a number.
const COUNTER_FIELDS: [&str; 7] = [
    "revision",
    "expected_revision",
    "current_revision",
    "generation",
    "seq",
    "next",
    "after",
];

fn counters_are_strings(value: &Value) -> Result<(), ProtocolViolation> {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if COUNTER_FIELDS.contains(&key.as_str()) && !(inner.is_null() || inner.is_string())
                {
                    return Err(ProtocolViolation::new(
                        "bad_integer",
                        format!("{key} = {inner} is a counter and travels as a decimal string"),
                    ));
                }
                counters_are_strings(inner)?;
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(counters_are_strings),
        _ => Ok(()),
    }
}

impl FacadeAnswer {
    /// The answer's type on the wire.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            FacadeAnswer::Accepted { .. } => "accepted",
            FacadeAnswer::Inspection(_) => "inspection",
            FacadeAnswer::Events { .. } => "events",
            FacadeAnswer::Refused(_) => "refused",
        }
    }

    /// Reads an answer as a client does: an unknown type or refusal code
    /// is a protocol violation, a counter sent as a number is one, and a
    /// field this client does not know is skipped.
    pub fn from_json(value: &Value) -> Result<FacadeAnswer, ProtocolViolation> {
        let map = value
            .as_object()
            .ok_or_else(|| ProtocolViolation::new("bad_string", "an answer is a JSON object"))?;
        let kind = map
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| ProtocolViolation::new("missing_field", "type"))?;
        if !ANSWER_TYPES.contains(&kind) {
            return Err(ProtocolViolation::new(
                "unknown_variant",
                format!("{kind} is not a facade answer"),
            ));
        }
        match map.get("facade").and_then(Value::as_u64) {
            Some(FACADE_VERSION) => {}
            Some(other) => {
                return Err(ProtocolViolation::new(
                    "unknown_variant",
                    format!("facade {other}; this client speaks {FACADE_VERSION}"),
                ));
            }
            None => return Err(ProtocolViolation::new("bad_integer", "facade")),
        }
        counters_are_strings(value)?;
        let detail = |e: serde_json::Error| {
            let reason = serde_reason(&e);
            ProtocolViolation::new(
                if reason == "unknown_field" {
                    "bad_string"
                } else {
                    reason
                },
                e.to_string(),
            )
        };
        match kind {
            "accepted" => {
                #[derive(Deserialize)]
                struct Wire {
                    #[serde(with = "crate::text")]
                    operation: OperationId,
                    key: OperationKey,
                    replayed: bool,
                    #[serde(with = "crate::text")]
                    job: JobId,
                    #[serde(with = "crate::text")]
                    revision: WorkRevision,
                }
                let wire = Wire::deserialize(value).map_err(detail)?;
                Ok(FacadeAnswer::Accepted {
                    operation: wire.operation,
                    key: wire.key,
                    replayed: wire.replayed,
                    job: wire.job,
                    revision: wire.revision,
                })
            }
            "inspection" => Ok(FacadeAnswer::Inspection(Box::new(
                Inspection::deserialize(value).map_err(detail)?,
            ))),
            "events" => {
                #[derive(Deserialize)]
                struct Wire {
                    #[serde(with = "crate::text")]
                    job: JobId,
                    events: Vec<EventWire>,
                    #[serde(with = "crate::text::option")]
                    next: Option<EventSeq>,
                    lagged: bool,
                    closed: Option<String>,
                }
                let wire = Wire::deserialize(value).map_err(detail)?;
                if let Some(event) = wire.events.iter().find(|e| e.kind().is_empty()) {
                    return Err(ProtocolViolation::new(
                        "missing_field",
                        format!("event {} has no kind", event.seq),
                    ));
                }
                Ok(FacadeAnswer::Events {
                    job: wire.job,
                    events: wire.events,
                    next: wire.next,
                    lagged: wire.lagged,
                    closed: wire.closed,
                })
            }
            _ => Ok(FacadeAnswer::Refused(parse_refused(map)?)),
        }
    }

    /// The answer as JSON, the bytes the authority sends.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            FacadeAnswer::Accepted {
                operation,
                key,
                replayed,
                job,
                revision,
            } => json!({
                "type": "accepted",
                "facade": FACADE_VERSION,
                "operation": operation.to_string(),
                "key": key.as_str(),
                "replayed": replayed,
                "job": job.to_string(),
                "revision": revision.to_string(),
            }),
            FacadeAnswer::Inspection(inspection) => {
                let mut map = Map::new();
                map.insert("type".into(), json!("inspection"));
                map.insert("facade".into(), json!(FACADE_VERSION));
                if let Value::Object(fields) = to_value(inspection) {
                    map.extend(fields);
                }
                Value::Object(map)
            }
            FacadeAnswer::Events {
                job,
                events,
                next,
                lagged,
                closed,
            } => json!({
                "type": "events",
                "facade": FACADE_VERSION,
                "job": job.to_string(),
                "events": to_value(events),
                "next": counter_json(*next),
                "lagged": lagged,
                "closed": closed,
            }),
            FacadeAnswer::Refused(refusal) => refusal.to_json(),
        }
    }
}

fn parse_refused(map: &Map<String, Value>) -> Result<Refusal, ProtocolViolation> {
    let code = map
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolViolation::new("missing_field", "code"))?;
    let code = RefusalCode::parse(code)
        .ok_or_else(|| ProtocolViolation::new("unknown_variant", format!("refusal code {code}")))?;
    let reason = map
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolViolation::new("missing_field", "reason"))?
        .to_owned();
    let message = map
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| format!("{code}: {reason}"), str::to_owned);
    let current_revision = match map.get("current_revision") {
        None | Some(Value::Null) => None,
        Some(v) => Some(counter(v, |reason| {
            ProtocolViolation::new(reason, "current_revision")
        })?),
    };
    Ok(Refusal {
        code,
        reason,
        message,
        current_revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bearer_reads_its_hex_and_prints_redacted() {
        let bearer = Bearer::parse_hex(&"ab".repeat(32)).unwrap();
        assert_eq!(bearer.as_bytes(), &[0xab; 32]);
        assert_eq!(bearer.to_hex(), "ab".repeat(32));
        assert_eq!(format!("{bearer:?}"), "Bearer(<redacted>)");
        assert!(Bearer::parse_hex(&"AB".repeat(32)).is_err());
        assert!(Bearer::parse_hex(&"ab".repeat(31)).is_err());
    }

    #[test]
    fn a_message_writes_back_what_it_read() {
        let value = json!({
            "type": "cancel", "facade": 1, "token": "1".repeat(64), "key": "k",
            "job": "01K8W3A00000000000000000J1", "expected_revision": "4", "task": null, "reason": "stop"
        });
        let message = ClientMessage::from_json(&value).unwrap();
        assert_eq!(message.to_json(), value);
        let refused =
            ClientMessage::from_json(&json!({"type": "cancel", "facade": 1})).unwrap_err();
        assert_eq!(refused.code, RefusalCode::BadRequest);
        assert_eq!(refused.reason, "missing_field");
    }

    #[test]
    fn the_generations_of_the_event_body_stay_strings() {
        let value = json!({
            "type": "events", "facade": 1, "job": "01K8W3A00000000000000000J1",
            "events": [{"seq": "7", "revision": "3", "at_ms": 1, "event": {"kind": "x", "generation": 1}}],
            "next": "7", "lagged": false, "closed": null
        });
        let violation = FacadeAnswer::from_json(&value).unwrap_err();
        assert_eq!(violation.reason, "bad_integer");
    }
}
