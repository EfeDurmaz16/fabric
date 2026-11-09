//! The closed span: every attribute a node span may carry is a typed
//! field here, and the OTLP JSON is made from these fields alone.

use std::fmt::{self, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{AttemptId, OperationId, TaskId};
use ring::digest::{SHA256, digest as sha256};
use serde_json::{Value, json};

/// A trace id: 16 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TraceId(pub(crate) [u8; 16]);

/// A span id: 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpanId(pub(crate) [u8; 8]);

/// A span context: the trace and the span in it. Fabric's are always sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpanContext {
    /// The trace.
    pub(crate) trace: TraceId,
    /// The span.
    pub(crate) span: SpanId,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

/// The context `sha256(domain + id)` names: trace id the first 16 bytes,
/// span id the next 8.
fn derive(domain: &str, id: &str) -> SpanContext {
    let d = sha256(&SHA256, format!("{domain}{id}").as_bytes());
    let d = d.as_ref();
    let mut trace = [0; 16];
    let mut span = [0; 8];
    trace.copy_from_slice(&d[..16]);
    span.copy_from_slice(&d[16..24]);
    SpanContext {
        trace: TraceId(trace),
        span: SpanId(span),
    }
}

/// The context of a managed or provider operation, as the Capsule bridge
/// worker derives it (`fabricOperationLink` in its `trace.ts`).
pub(crate) fn operation_context(operation: OperationId) -> SpanContext {
    derive("fabric.operation:", &operation.to_string())
}

/// The context of an attempt: the root of the trace its launch, its
/// reports and the authority's observations of it share.
pub(crate) fn attempt_context(attempt: AttemptId) -> SpanContext {
    derive("fabric.attempt:", &attempt.to_string())
}

/// A fresh context for an operation that has no id of its own (a
/// standalone job op, or a command the journal refused).
pub(crate) fn standalone_context() -> SpanContext {
    derive("fabric.op:", &ulid::Ulid::generate().to_string())
}

/// A fresh span id, for a span inside another's trace.
pub(crate) fn child_span() -> SpanId {
    standalone_context().span
}

/// The W3C `traceparent` of `context`, which a spawned worker reads as
/// `TRACEPARENT`.
pub(crate) fn traceparent(context: SpanContext) -> String {
    format!("00-{}-{}-01", context.trace, context.span)
}

/// SHA-256 of `value`: how a span names an idempotency key or an approval
/// ref without carrying it.
pub(crate) fn digest(value: &str) -> Digest {
    let mut out = [0; 32];
    out.copy_from_slice(sha256(&SHA256, value.as_bytes()).as_ref());
    Digest(out)
}

/// A 32-byte digest, shown as lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Digest(pub(crate) [u8; 32]);

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

/// A code: a refusal reason or an error type name. Only ASCII letters,
/// digits and `_`, at most 64 of them, so no message text fits in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Code(String);

impl Code {
    /// `text` as a code, `None` when it is not one.
    pub(crate) fn new(text: &str) -> Option<Code> {
        let ok = !text.is_empty()
            && text.len() <= 64
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        ok.then(|| Code(text.to_owned()))
    }

    /// A code this source names; one that is not a code is `Unnamed`.
    pub(crate) fn fixed(name: &'static str) -> Code {
        Code::new(name).unwrap_or_else(|| Code("Unnamed".to_owned()))
    }

    /// The code.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The type name of a value: the leading identifier of its `Debug` form,
/// which for an enum is the variant's name and never its data.
pub(crate) fn type_name(value: &impl fmt::Debug) -> Code {
    let text = format!("{value:?}");
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    Code::new(&text[..end.min(64)]).unwrap_or_else(|| Code::fixed("Unnamed"))
}

/// The snake-case form of a `CamelCase` name.
pub(crate) fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The operation a span describes: its name is `fabric <op>`. Only the
/// constants below exist, so no runtime text names a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpanOp(&'static str);

impl SpanOp {
    pub(crate) const NODE_ADD: SpanOp = SpanOp("node.add");
    pub(crate) const JOB_NEW: SpanOp = SpanOp("job.new");
    pub(crate) const JOB_RUN: SpanOp = SpanOp("job.run");
    pub(crate) const JOB_RESUME: SpanOp = SpanOp("job.resume");
    pub(crate) const JOB_SUSPEND: SpanOp = SpanOp("job.suspend");
    pub(crate) const JOB_MOVE: SpanOp = SpanOp("job.move");
    pub(crate) const JOB_STEAL: SpanOp = SpanOp("job.steal");
    pub(crate) const JOB_DECIDE: SpanOp = SpanOp("job.decide");
    pub(crate) const JOB_REPAIR: SpanOp = SpanOp("job.repair");
    pub(crate) const GC: SpanOp = SpanOp("gc");
    pub(crate) const HARNESS_STARTED: SpanOp = SpanOp("harness.started");
    pub(crate) const HARNESS_SESSION: SpanOp = SpanOp("harness.session");
    pub(crate) const HARNESS_EXITED: SpanOp = SpanOp("harness.exited");
    pub(crate) const MANAGED_COMMAND: SpanOp = SpanOp("managed.command");
    pub(crate) const PEER_LAUNCH: SpanOp = SpanOp("peer.launch");
    pub(crate) const PEER_INSTRUCT: SpanOp = SpanOp("peer.instruct");
    pub(crate) const REPORT: SpanOp = SpanOp("report");
    pub(crate) const PEER_APPEND: SpanOp = SpanOp("peer.append");
    pub(crate) const PEER_MOVE_REQUEST: SpanOp = SpanOp("peer.move_request");
    pub(crate) const PEER_OFFER: SpanOp = SpanOp("peer.offer");
    pub(crate) const PEER_MOVE_ABORT: SpanOp = SpanOp("peer.move_abort");

    /// The span of a managed command of `kind`.
    pub(crate) fn managed(kind: fabric_core::managed::CommandKind) -> SpanOp {
        use fabric_core::managed::CommandKind as K;
        SpanOp(match kind {
            K::CreateJob => "managed.create_job",
            K::StartAttempt => "managed.start_attempt",
            K::RegisterTask => "managed.register_task",
            K::FinishTask => "managed.finish_task",
            K::CancelJob => "managed.cancel_job",
            K::CancelTask => "managed.cancel_task",
            K::RecordNote => "managed.record_note",
            K::Checkpoint => "managed.checkpoint",
            K::Continue => "managed.continue",
        })
    }

    /// The span of an observation the authority recorded.
    pub(crate) fn observed(observation: &fabric_core::managed::Observation) -> SpanOp {
        use fabric_core::managed::Observation as O;
        SpanOp(match observation {
            O::Started { .. } => "observe.started",
            O::Checkpointed { .. } => "observe.checkpointed",
            O::Receipts { .. } => "observe.receipts",
            O::HandoffBlocked { .. } => "observe.handoff_blocked",
            O::Placed { .. } => "observe.placed",
            O::Continued { .. } => "observe.continued",
            O::Exited { .. } => "observe.exited",
            O::Lost { .. } => "observe.lost",
            O::Unknown { .. } => "observe.unknown",
        })
    }

    /// The op's name, the span's `fabric.op`.
    pub(crate) fn name(self) -> &'static str {
        self.0
    }
}

/// The span kind, as OTLP numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpanKind {
    /// Work inside this node.
    Internal = 1,
    /// A request this node serves for a peer.
    Server = 2,
    /// A request this node sends.
    Client = 3,
}

/// How the operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpanResult {
    /// It did what it was asked.
    Done,
    /// The answer stored under its key was given again.
    Replayed,
    /// It was refused before any effect.
    Refused,
    /// It failed.
    Failed,
    /// Whether it took effect cannot be told.
    Unknown,
}

impl SpanResult {
    fn name(self) -> &'static str {
        match self {
            SpanResult::Done => "done",
            SpanResult::Replayed => "replayed",
            SpanResult::Refused => "refused",
            SpanResult::Failed => "failed",
            SpanResult::Unknown => "unknown",
        }
    }
}

/// One operation of this node, as a span. Closed: there is no field for
/// free text, so none can be exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpSpan {
    /// What it was.
    pub(crate) op: SpanOp,
    /// Its kind.
    pub(crate) kind: SpanKind,
    /// Its own context.
    pub(crate) context: SpanContext,
    /// The span it is a child of, in the same trace.
    pub(crate) parent: Option<SpanId>,
    /// Spans in other traces it relates to: the attempt a grant made.
    pub(crate) links: Vec<SpanContext>,
    /// Unix nanoseconds.
    pub(crate) start_ns: u64,
    /// Unix nanoseconds.
    pub(crate) end_ns: u64,
    /// `fabric.job`.
    pub(crate) job: Option<JobId>,
    /// `fabric.operation.id`.
    pub(crate) operation: Option<OperationId>,
    /// `fabric.operation.key_digest`: SHA-256 of the idempotency key.
    pub(crate) key_digest: Option<Digest>,
    /// `fabric.approval.ref_digest`: SHA-256 of an approval ref.
    pub(crate) approval_digest: Option<Digest>,
    /// `fabric.task`.
    pub(crate) task: Option<TaskId>,
    /// `fabric.attempt`.
    pub(crate) attempt: Option<AttemptId>,
    /// `fabric.generation`.
    pub(crate) generation: Option<u64>,
    /// `fabric.generation.supersedes`: the generation a regrant ends.
    pub(crate) supersedes: Option<u64>,
    /// `fabric.revision`: the managed job's revision after the operation.
    pub(crate) revision: Option<u64>,
    /// `fabric.epoch`: a standalone job's lease epoch.
    pub(crate) epoch: Option<u64>,
    /// `fabric.peer`: the node on the other side.
    pub(crate) peer: Option<NodeId>,
    /// `fabric.target`: the node a move goes to.
    pub(crate) target: Option<NodeId>,
    /// `fabric.snapshot`.
    pub(crate) snapshot: Option<SnapshotId>,
    /// `fabric.checkpoint`: the cut's digest.
    pub(crate) checkpoint: Option<Digest>,
    /// `fabric.result`.
    pub(crate) result: SpanResult,
    /// `fabric.refusal.reason`.
    pub(crate) refusal: Option<Code>,
    /// `fabric.error.type`.
    pub(crate) error_type: Option<Code>,
}

/// Unix nanoseconds now.
pub(crate) fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

impl OpSpan {
    /// A span of `op` in `context`, started now, `done` until told otherwise.
    pub(crate) fn begin(op: SpanOp, kind: SpanKind, context: SpanContext) -> OpSpan {
        let now = now_ns();
        OpSpan {
            op,
            kind,
            context,
            parent: None,
            links: Vec::new(),
            start_ns: now,
            end_ns: now,
            job: None,
            operation: None,
            key_digest: None,
            approval_digest: None,
            task: None,
            attempt: None,
            generation: None,
            supersedes: None,
            revision: None,
            epoch: None,
            peer: None,
            target: None,
            snapshot: None,
            checkpoint: None,
            result: SpanResult::Done,
            refusal: None,
            error_type: None,
        }
    }

    /// Moves the span into `parent`'s trace as its child, with a fresh id.
    pub(crate) fn child_of(&mut self, parent: SpanContext) {
        self.context = SpanContext {
            trace: parent.trace,
            span: child_span(),
        };
        self.parent = Some(parent.span);
    }

    /// Ends it now with `result`.
    pub(crate) fn end(&mut self, result: SpanResult) {
        self.end_ns = now_ns().max(self.start_ns);
        self.result = result;
    }

    /// Ends it as refused with `code`.
    pub(crate) fn refused(&mut self, code: &str) {
        self.refusal = Code::new(code);
        self.end(SpanResult::Refused);
    }

    /// Ends it as failed (or unknown) with the error's type name.
    pub(crate) fn failed(&mut self, result: SpanResult, error_type: Code) {
        self.error_type = Some(error_type);
        self.end(result);
    }

    /// The OTLP JSON of the span (`Span` in `trace.proto`), with this
    /// node's id as `fabric.node`.
    pub(crate) fn to_otlp(&self, node: NodeId) -> Value {
        let mut attributes = vec![
            string("fabric.op", self.op.name()),
            string("fabric.node", &node.to_string()),
            string("fabric.result", self.result.name()),
        ];
        let mut text = |key: &str, value: Option<String>| {
            if let Some(value) = value {
                attributes.push(string(key, &value));
            }
        };
        text("fabric.job", self.job.map(|v| v.to_string()));
        text("fabric.operation.id", self.operation.map(|v| v.to_string()));
        text(
            "fabric.operation.key_digest",
            self.key_digest.map(|v| v.to_string()),
        );
        text(
            "fabric.approval.ref_digest",
            self.approval_digest.map(|v| v.to_string()),
        );
        text("fabric.task", self.task.map(|v| v.to_string()));
        text("fabric.attempt", self.attempt.map(|v| v.to_string()));
        text("fabric.peer", self.peer.map(|v| v.to_string()));
        text("fabric.target", self.target.map(|v| v.to_string()));
        text("fabric.snapshot", self.snapshot.map(|v| v.to_string()));
        text("fabric.checkpoint", self.checkpoint.map(|v| v.to_string()));
        text(
            "fabric.refusal.reason",
            self.refusal.as_ref().map(|c| c.as_str().to_owned()),
        );
        text(
            "fabric.error.type",
            self.error_type.as_ref().map(|c| c.as_str().to_owned()),
        );
        for (key, value) in [
            ("fabric.generation", self.generation),
            ("fabric.generation.supersedes", self.supersedes),
            ("fabric.revision", self.revision),
            ("fabric.epoch", self.epoch),
        ] {
            if let Some(value) = value {
                attributes.push(int(key, value));
            }
        }
        let status = match self.result {
            SpanResult::Done | SpanResult::Replayed => json!({ "code": 1 }),
            SpanResult::Refused => json!({ "code": 0 }),
            SpanResult::Failed | SpanResult::Unknown => json!({
                "code": 2,
                "message": self.error_type.as_ref().map_or("unknown", Code::as_str),
            }),
        };
        let mut span = json!({
            "traceId": self.context.trace.to_string(),
            "spanId": self.context.span.to_string(),
            "name": format!("fabric {}", self.op.name()),
            "kind": self.kind as u8,
            "startTimeUnixNano": self.start_ns.to_string(),
            "endTimeUnixNano": self.end_ns.to_string(),
            "attributes": attributes,
            "status": status,
        });
        if let Some(parent) = self.parent {
            span["parentSpanId"] = Value::String(parent.to_string());
        }
        if !self.links.is_empty() {
            span["links"] = Value::Array(
                self.links
                    .iter()
                    .map(
                        |l| json!({ "traceId": l.trace.to_string(), "spanId": l.span.to_string() }),
                    )
                    .collect(),
            );
        }
        if self.result == SpanResult::Unknown {
            span["events"] = json!([{ "name": "fabric.outcome.unknown", "timeUnixNano": self.end_ns.to_string() }]);
        }
        span
    }
}

fn string(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn int(key: &str, value: u64) -> Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

/// The OTLP JSON body (`ExportTraceServiceRequest`) for `spans` of `node`.
pub(crate) fn body(node: NodeId, spans: &[OpSpan]) -> Value {
    json!({
        "resourceSpans": [{
            "resource": { "attributes": [
                string("service.name", "fabricd"),
                string("service.version", env!("CARGO_PKG_VERSION")),
                string("service.instance.id", &node.to_string()),
            ] },
            "scopeSpans": [{
                "scope": { "name": "fabricd", "version": env!("CARGO_PKG_VERSION") },
                "spans": spans.iter().map(|s| s.to_otlp(node)).collect::<Vec<_>>(),
            }],
        }],
    })
}
