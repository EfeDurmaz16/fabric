# PR 12: spans of a managed job through two fabricd processes

Ran on one host, 2026-11-09: two `fabricd` daemons, each with its own state root and home, paired over a TCP and TLS node link on 127.0.0.1. Not two machines.

## The Jaeger run

[jaeger.md](jaeger.md) is the primary evidence: both daemons ran with `fabric serve --otlp-endpoint http://127.0.0.1:4318` against `jaegertracing/all-in-one:1.73.0`, and Jaeger's own query API returned the spans, saved as [spans-jaeger.json](spans-jaeger.json). 21 spans in 8 traces, 13 from `a` and 8 from `b`; trace `5e36d7b8ce59258e0a78955f788115b5` holds `b`'s launch, instructions and reports and `a`'s observations of them. No secret value, goal, argv, operation key or path is in the JSON.

The first two tries pulled `jaegertracing/all-in-one:latest`, and both failed inside the Docker daemon on the same broken parent snapshot (`snapshots/14844/fs`), before any span was sent. The run above used the pinned `1.73.0` image, pulled after that.

## The deterministic run: the test's OTLP receiver

`spans-fake-receiver.json` is the span list of `observability::e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret`, written by:

```sh
FABRIC_E2E_SPANS_OUT=$PWD/docs/evidence/pr12 cargo test -p fabric --locked --test e2e observability
```

The test runs a plain HTTP listener that answers each OTLP post 200 and keeps its body; it is not a collector and checks nothing a collector would. The run: the job submitted to `a`'s facade and pinned to `b`, launched on `b`, a build task registered, a checkpoint, a continue pinned to `a` that granted generation 2 there. 21 spans: 13 from `a`, 8 from `b`. `b`'s `fabric peer.launch` is the root of the first attempt's trace, and `b`'s three `fabric report` spans and `a`'s `observe.started` and `observe.checkpointed` are in that trace. The continue's grant names `fabric.generation.supersedes = 1`. The test fails if the job's secret value, its goal, the build's argv, an operation key, a home path or the test root appears in any posted body or in the audit export. It is the run that repeats on every test run; the Jaeger run is a one-off on this host.

## What these spans do not show yet

- A provider operation has no Fabric span. The Capsule bridge worker links each of its spans to `sha256("fabric.operation:" + operation_id)` of the provider operation it was given, but on main no node operation dispatches a bridge worker or writes `provider_operations`, so nothing in this run fills that context. The joined Fabric and Capsule trace of the roadmap row waits for that call site; the pinned vector test shows only that both sides derive the same ids.
- `fabric.approval.ref_digest` is in the span struct and the docs, and no node operation sets it yet.
- A replayed command and a re-sent launch each get a span id of their own, a child of the operation's or the attempt's span (`otel::tests`); this run has neither.
