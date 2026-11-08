//! OpenTelemetry spans out of the node (ADR 0031, docs/observability.md):
//! one span per operation that changes state, sent as OTLP/HTTP JSON to
//! `<endpoint>/v1/traces` when `fabric serve --otlp-endpoint` (or
//! `OTEL_EXPORTER_OTLP_ENDPOINT`) names one, and nothing at all when not.
//!
//! Invariant: a span carries only what [`OpSpan`] has a field for, and it
//! has none for an env value, a secret, an argv, a goal, a path, a raw
//! operation key, a raw approval ref, an error message or any process
//! output. An idempotency key and an approval ref travel as SHA-256 hex,
//! an error as its type name, a refusal as its snake-case code. A span is
//! not evidence (the journal is), so the exporter never retries and never
//! blocks the operation it describes: a full queue or a failed post drops
//! spans and counts them.
//!
//! Trace identity is derived, so another process joins without a
//! protocol change: an operation's trace and span id are the first 32 and
//! the next 16 hex of `sha256("fabric.operation:" + operation_id)`, as the
//! Capsule bridge worker links to; an attempt's are the same over
//! `"fabric.attempt:" + attempt_id`, so the authority's spans of an
//! attempt and the spans of the node that runs it share one trace.

mod export;
pub(crate) mod hooks;
mod span;

#[cfg(test)]
mod tests;

/// How long a stopping node waits for its last spans to post.
pub(crate) const FLUSH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) use export::Tracer;
pub use export::{OtlpEndpoint, OtlpEndpointError};
pub(crate) use span::{
    Code, OpSpan, SpanKind, SpanOp, SpanResult, attempt_context, standalone_context, traceparent,
    type_name,
};
