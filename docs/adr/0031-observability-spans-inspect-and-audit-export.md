# ADR 0031: Operation spans, inspect and the audit export

Status: proposed 2026-11-09 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). PR 12 of the 2026-11-08 roadmap, Fabric side; the Capsule side is capsule#322. Number 0030 is left to PR 11, which may land first; if it does not, this ADR can be renumbered at the merge.

## Context

Platform teams ask two things of a runtime before they run it: spans in the tracing backend they already have, and an export of what happened for an audit. Capsule ships both (capsule#322): one span per operation through a tracer the caller gives it, a span link from each Fabric-driven worker operation to a context derived from the Fabric operation id alone, and `capsule audit export` as JSONL. Fabric's journal already holds the facts, hash-chained; nothing gets them out except `fabric managed inspect`, which shows part of one job.

The hard part is what a span must never carry. `fabricd` writes 27 `error =` and 5 `path =` fields to its `tracing` log. A `tracing` layer that forwards fields to OpenTelemetry would send each of them, and the next log line added would send more: the rule "no env value, no secret, no argv, no path, no message text" would hold only by review. The second hard part is identity: the Capsule worker links to `sha256("fabric.operation:" + operation_id)`, and Fabric must emit its span with exactly those ids.

## Decision

### An in-house OTLP/HTTP JSON exporter, always compiled, off by default

`fabric serve --otlp-endpoint http://host:port` (or `OTEL_EXPORTER_OTLP_ENDPOINT`) starts one task that posts batches as OTLP/HTTP JSON to `<endpoint>/v1/traces` over a plain tokio `TcpStream`: a queue of 1024 spans, a batch at 256 spans or 2 seconds, a 5 second post deadline, no retry, drop and count on failure, a 3 second flush when the node stops. Without an endpoint no span is built and no task runs. No new crate enters the workspace: `ring`, already in the graph through `rustls`, becomes a direct `fabricd` dependency for SHA-256.

Selected over the `opentelemetry` crates with a `tracing-opentelemetry` layer because (from the research run for this row) their 0.31, 0.32 and 0.33 releases each broke public API in 2026, 0.32 removed the span builder's trace and span id setters (so Fabric could only link to the derived context, not be it), and the OTLP exporter adds 42 crates with its hyper client or 72 with reqwest, about 1.5 MB to the release binary. Selected over a JSON log plus a collector sidecar because the row's acceptance is a run in Jaeger, and a log layer has the same field problem as the tracing layer. `https` is refused: a TLS client to a collector is a dependency decision of its own, and a local collector terminates TLS today.

### A closed span type

A span is `OpSpan`: one typed field per attribute (ids, counters, a node id, a snapshot id, SHA-256 digests, a code that holds only `[A-Za-z0-9_]`), and the OTLP JSON is built from these fields alone. There is no field for text, so a goal, an argv, a path, an env value, a raw key or an error message cannot be exported. An error is its type name (the leading identifier of its `Debug` form, which for an enum is the variant), a refusal its snake-case code. A unit test posts to a receiver it runs and fails if a secret, a raw key, an argv, a path or a message reaches a span; it was proven red by giving a span the raw key.

### Derived trace identity

An operation's span has trace id and span id `sha256("fabric.operation:" + id)[0..16]` and `[16..24]`, the context the Capsule worker links to; the vector is computed from Capsule's `trace.ts` and pinned in a test. An attempt's trace is the same over `"fabric.attempt:" + id`: its root is the launch span on the node that runs it, and the reports, the instructions and the authority's observations of it are its children, so one attempt across two daemons is one trace, and the grant links to it. Every process an attempt spawns gets that context as `TRACEPARENT` while the node exports, and a bridge worker gets the `TRACEPARENT` its `Config` names, never one it inherited. Selected over carrying trace context on the wire because neither the launch protocol nor the bridge needs a field for it.

### Spans for operations, not reads

The authority's commands and observations, the standalone client requests that change state, the fenced peer requests and the launch path get a span; reads and heartbeats do not, as Capsule's observe and attach do not. Bridge operations have no `fabricd` call site on `main` (the bridge crate is not linked into the node yet), so no bridge span is emitted; the worker side links to the derived context already.

### Inspect and the audit export are client API ops

`inspect` answers a job's whole managed state (the managed inspection plus attempts, generations with what granted them and what they superseded, the newest 1000 operations with refusal codes and the newest 1000 evidence rows, each with a count of the older rows left out, the last report) with `schema: 1`, which only gains fields. `audit_export` answers pages of the journal's operations, events, evidence and provider operations made in `[since, until)` in the total order `(at_ms, table, row id)`, after a cursor, under half a frame. An audit row carries ids, codes, counters, times and digests only: no event body, no evidence ref (its SHA-256), no provider locator, target, searched scope or reason, no idempotency key (its SHA-256). Both are additive ops of api 1 with fixtures, so `fabric inspect` and `fabric audit export` exit 80 without a node, as every client op does. A date-only bound is midnight UTC, as in `capsule audit export`, so a date-only `--until` leaves that day out.

## Consequences

- The release binary grows by 557,760 bytes (+2.9%, the exporter, the two ops, their wire types and the CLI together; the research estimate for the exporter alone was 60 to 120 KB), measured on aarch64 macOS with the release profile: 18,941,648 bytes at the base (origin/main ad744de4) and 19,499,408 with this PR. CI reports the size on every pull request.
- The exporter speaks HTTP/1.1 with `Connection: close` and reads only the status line. A collector that needs gzip, TLS or headers sits behind a local collector.
- A span is lost on a full queue, a failed post and a node killed with `SIGKILL`. The journal and the audit export are the record.
- The Jaeger run of the acceptance did not happen in this PR: the Docker daemon failed the image pull (docs/evidence/pr12). The same two-daemon run against the test's own OTLP receiver is in the evidence and the end-to-end suite.
