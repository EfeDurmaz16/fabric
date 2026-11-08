# Observability

A node sends one OpenTelemetry span per operation that changes state, as OTLP/HTTP JSON, to a collector you name. `fabric inspect` reads a managed job's whole state, and `fabric audit export` writes the journal as JSON lines for an audit. The decision and its reasons are [ADR 0031](adr/0031-observability-spans-inspect-and-audit-export.md).

## What a span will never carry

- An environment value or a secret value, a job's goal, a harness or task argv, a repository, workspace, home or socket path, a user name, a locator's text or a native id, an error message, or any process's output.
- A raw idempotency key or a raw approval ref. A key appears as SHA-256 hex in `fabric.operation.key_digest`, so a span joins to the journal row without carrying the value. `fabric.approval.ref_digest` is reserved for an approval ref the same way; no node operation takes an approval ref yet, so no span carries it today.
- An error's text. A failed span's status message, and `fabric.error.type`, are the error's type name only, such as `Io` or `Unreachable`. A refusal is its snake-case code in `fabric.refusal.reason`.

This holds by type, not by convention: a span is a closed struct with one typed field per attribute below, and the exporter builds the JSON from those fields alone. No `tracing` event or field reaches the exporter.

## Setup

Run a collector that takes OTLP over HTTP on port 4318. Jaeger's all-in-one image does:

```sh
docker run --rm -d --name jaeger -e COLLECTOR_OTLP_ENABLED=true -p 16686:16686 -p 4318:4318 jaegertracing/all-in-one:1.73.0
fabric serve --otlp-endpoint http://localhost:4318
```

Without `--otlp-endpoint`, `fabric serve` reads `OTEL_EXPORTER_OTLP_ENDPOINT`, as OpenTelemetry's exporters do. Without both, the node exports nothing and builds no span.

- The node posts to `<endpoint>/v1/traces` with `Content-Type: application/json`. Only `http://` is supported. `--otlp-endpoint https://...` is a usage error (exit 2). An `https` value in the variable is reported once on standard error and nothing is exported, so a variable set for other tools never stops a node.
- Spans wait in a queue of 1024. A batch goes when it holds 256 spans, 2 seconds after its first span, and when the node stops, which waits 3 seconds at most. A post that fails or answers anything but 2xx drops its batch. A full queue drops the new span. Either way the spans are counted, and `fabric serve` logs a warning at most once a minute. A span is never retried: the journal is the record, not the span.
- Only `fabric serve` exports. No client command sends a span, so `--json` output is unchanged.

## Trace identity

An operation's trace id and span id are the first 32 and the next 16 hex of `sha256("fabric.operation:" + operation_id)`, with the operation id as its ULID text. This is the context the Capsule bridge worker links each of its spans to (`fabricOperationLink` in its `trace.ts`), so a backend joins both sides without a protocol change. Only the first accepted answer of an operation carries these ids; a replay of it is a child span with an id of its own, so no two spans share them.

What has a span in that context today: managed commands and placements (`observe.placed`). A provider operation, the one a Capsule worker names in its link, has none yet: on main no node operation dispatches a bridge worker or writes `provider_operations`, so a worker's link points at a context no Fabric span fills, and the joined Fabric and Capsule trace waits for that call site.

An attempt's trace is `sha256("fabric.attempt:" + attempt_id)` the same way. The node that runs the attempt makes its root span (`fabric peer.launch`) when it accepts the launch; a launch it refuses (the plan sent again after a restart, answered `already_launched`) is a child of that root with its own id. Its reports and instructions, and the authority's observations of them, are children of it. So the spans of both nodes of one attempt are in one trace. A grant links to the attempt's trace.

Each process an attempt spawns (the harness, a build or test task) gets `TRACEPARENT=00-<attempt trace>-<attempt span>-01` while the node exports, so a Capsule worker started inside the attempt links to it. `fabric-bridge` gives a worker the `TRACEPARENT` its `Config::traceparent` names, and never one it inherited; no node operation spawns a bridge worker yet, so this is the library's rule, not a path a node runs today.

## Span attributes

The resource names `service.name` `fabricd`, `service.version` and `service.instance.id`, the node id. A span is `fabric <op>`. Its kind is `INTERNAL`, `SERVER` for a peer request this node serves, and `CLIENT` for a report it sends.

| Op (`fabric.op`) | When |
| --- | --- |
| `managed.create_job`, `managed.start_attempt`, `managed.register_task`, `managed.finish_task`, `managed.cancel_job`, `managed.cancel_task`, `managed.record_note`, `managed.checkpoint`, `managed.continue` | The authority accepted, replayed or refused a command. |
| `managed.command` | A checkpoint or continue the journal refused before its kind was derived. |
| `observe.started`, `observe.checkpointed`, `observe.receipts`, `observe.handoff_blocked`, `observe.placed`, `observe.continued`, `observe.exited`, `observe.lost`, `observe.unknown` | The authority recorded an observation: a report, a placement, a recovery. |
| `peer.launch`, `peer.instruct`, `report` | The launch path on the node that runs the attempt. |
| `peer.append`, `peer.move_request`, `peer.offer`, `peer.move_abort` | A fenced standalone request a peer sent. |
| `node.add`, `job.new`, `job.run`, `job.resume`, `job.suspend`, `job.move`, `job.steal`, `job.decide`, `job.repair`, `harness.started`, `harness.session`, `harness.exited` | A client request that changed standalone state. |

Reads (status, list, log, inspect, audit export, pull, manifest, chunk, facts, ping) and heartbeats, which write nothing, get no span.

| Attribute | Value |
| --- | --- |
| `fabric.op`, `fabric.node`, `fabric.result` | Always. The result is `done`, `replayed` (the answer stored under the key, given again), `refused`, `failed` or `unknown`. |
| `fabric.job`, `fabric.operation.id`, `fabric.task`, `fabric.attempt` | ULIDs, when the operation has them. |
| `fabric.operation.key_digest` | SHA-256 hex of the idempotency key. |
| `fabric.generation`, `fabric.revision`, `fabric.epoch` | Integers. |
| `fabric.generation.supersedes` | On a grant at generation n > 1 (a regrant, or a continue's new attempt): n - 1. |
| `fabric.peer`, `fabric.target` | Node ids: the other side, and a move's destination. |
| `fabric.snapshot`, `fabric.checkpoint` | A snapshot id, and a cut's digest. |
| `fabric.refusal.reason`, `fabric.error.type` | A snake-case code, and a type name. |

The status is `OK` for done and replayed, `UNSET` for refused, and `ERROR` with the type name for failed and unknown. An unknown outcome adds the event `fabric.outcome.unknown`.

## Inspect

```sh
fabric inspect <JOB> --json
fabric inspect <JOB> --attempt <ATTEMPT> --json
```

`--json` prints `{"inspection": {...}}` with this schema, version 1 (`schema`). A later version only adds fields. Every list is oldest first. Counters inside `job`, `placements` and `notes` are decimal strings, as `managed_inspect` sends them; the rows `inspect` adds use JSON numbers, all below 2^53.

| Field | What |
| --- | --- |
| `schema` | `1`. |
| `attempt` | The attempt asked for, when `--attempt` was given. It narrows `attempts`, `generations`, `evidence`, `provider_operations` and `last_report` to that attempt. |
| `job`, `parked`, `placements`, `continuations`, `notes`, `provider_operations`, `blockers`, `residuals` | As `fabric managed inspect` shows them: the job with its tasks, their classes and last receipts, every placement decision with its candidates, every continuation with the fidelity achieved. |
| `attempts[]` | `attempt`, `task`, `generation`, `status`, `node`, `granted_by` (the operation), `initial_checkpoint` and `last_checkpoint` (64 hex), `granted_ms`, `ended_ms`. |
| `generations[]` | `generation`, `attempt`, `granted_by`, `granted_by_kind` (`create_job`, `start_attempt` or `continue`), `supersedes`. |
| `operations[]` | The newest 1000 operations: `operation`, `operation_key`, `kind`, `status`, `revision` (absent for a rejected one), `refusal` (its code), `accepted_ms`, `updated_ms`. |
| `operations_omitted` | How many older operations the reply leaves out; absent when none. |
| `evidence[]` | The newest 1000 evidence rows: `seq`, `kind` (`checkpoint` or `exit`), `attempt`, `revision`, `ref`, `digest`, `at_ms`. |
| `evidence_omitted` | How many older evidence rows the reply leaves out; absent when none. |
| `last_report` | The newest event a report recorded: `seq`, `kind`, `attempt`, `at_ms`. |

A job that checkpoints for weeks holds tens of thousands of rows, more than one reply frame holds, so `inspect` carries the newest of each and counts the rest; `fabric audit export` pages every row. `inspect` is local to the node's owner, so it shows operation keys and evidence refs as the journal holds them. A job the authority does not hold is refused as `unknown_job`; an attempt not in the job is `bad_request`.

## Audit export

```sh
fabric audit export --since 2026-11-01 --until 2026-11-08 > audit.jsonl
fabric audit export --json
```

It writes one JSON object per line for every row the authority's journal made in `[since, until)`, oldest first, and exits 0, also for an empty window. `--json` writes the same rows as one document, `{"records":[...]}`. Both bounds are optional. Each is an RFC 3339 time (`2026-11-08T09:30:00Z`, `2026-11-08T12:30:00+03:00`, fractions allowed) or a date, which is that day's midnight UTC. So a date-only `--until 2026-11-08` leaves 2026-11-08 out: to include a day, give the next day. A bound that does not read, or an `--until` not later than `--since`, is a usage error (exit 2). Without a running node it exits 80 (`no_daemon`).

The order is `at_ms`, then the type in the order below, then the row's id (an event's `seq`), so it is total and the same on every run. The node answers in pages under half a frame and the CLI follows the cursor.

| `type` | Fields |
| --- | --- |
| `operation` | `at_ms` (accepted or rejected), `operation`, `job`, `kind`, `decision` (`accept` or `reject`), `status`, `refusal` (the code of a rejected one), `key_digest` (SHA-256 of the idempotency key). |
| `event` | `at_ms`, `seq`, `job`, `revision`, `operation`, `kind`, `digest`, `prev_digest`. No body. The digests are the journal's hash chain, so a reader checks that each event links to the one before it in its job. |
| `evidence` | `at_ms` (its event's), `seq`, `job`, `attempt`, `kind`, `ref_digest` (SHA-256 of the ref), `digest`. |
| `provider_operation` | `at_ms` (dispatched), `operation`, `job`, `task`, `attempt`, `generation`, `profile`, `action`, `effect_class`, `purpose`, `status`, `code` (only when it is a plain code), `effect`, `locator_key` (the journal's digest of the locator). No locator, target, searched scope or reason text. |

Times are Unix milliseconds and digests 64 lowercase hex.

## Evidence

[evidence/pr12](evidence/pr12/README.md) holds a managed job run through two `fabricd` processes on one host into Jaeger, with the trace JSON its query API returned, and the same run against the test's own receiver. The end-to-end test `observability::e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret` runs both daemons against an OTLP receiver of its own and fails if the job's secret, its goal, a task argv, an operation key or a home path reaches a span or the audit export.
