# PR 12: spans of a managed job through two fabricd processes

Ran on one host, 2026-11-09: two `fabricd` daemons, each with its own state root and home, paired over a TCP and TLS node link on 127.0.0.1. Not two machines.

## The Jaeger run: blocked

The planned run sends the spans to `jaegertracing/all-in-one` and queries its API. Docker (OrbStack) started, and the one pull of the image failed inside the Docker daemon:

```text
$ docker pull jaegertracing/all-in-one:latest
latest: Pulling from jaegertracing/all-in-one
failed to prepare extraction snapshot "extract-522255668-Yfcd sha256:326ce65caeaae9a7d740c654a301940ef7ee971b3172492843a42c6f5bee239f": failed to stat parent: stat /var/lib/docker/containerd/daemon/io.containerd.snapshotter.v1.overlayfs/snapshots/14844/fs: no such file or directory
```

A second pull after the review, with `docker info` answering, failed the same way on the same parent snapshot (`snapshots/14844/fs`), so the Docker daemon's own snapshot store is broken, not the network or the image. The image was not on the host before, so no Jaeger run happened and no Jaeger trace is claimed here. The commands for the run, once the image pulls:

```sh
docker run --rm -d --name jaeger -p 16686:16686 -p 4318:4318 jaegertracing/all-in-one:latest
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 fabric serve   # on each node
# after a managed job ran:
curl -s 'http://127.0.0.1:16686/api/traces?service=fabricd&limit=20'
```

## The same spans, received by the test's OTLP receiver

`spans-fake-receiver.json` is the span list of `observability::e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret`, written by:

```sh
FABRIC_E2E_SPANS_OUT=$PWD/docs/evidence/pr12 cargo test -p fabric --locked --test e2e observability
```

The test runs a plain HTTP listener that answers each OTLP post 200 and keeps its body; it is not a collector and checks nothing a collector would. The run: the job submitted to `a`'s facade and pinned to `b`, launched on `b`, a build task registered, a checkpoint, a continue pinned to `a` that granted generation 2 there. 21 spans: 13 from `a`, 8 from `b`. `b`'s `fabric peer.launch` is the root of the first attempt's trace, and `b`'s three `fabric report` spans and `a`'s `observe.started` and `observe.checkpointed` are in that trace. The continue's grant names `fabric.generation.supersedes = 1`. The test fails if the job's secret value, its goal, the build's argv, an operation key, a home path or the test root appears in any posted body or in the audit export.

## What these spans do not show yet

- A provider operation has no Fabric span. The Capsule bridge worker links each of its spans to `sha256("fabric.operation:" + operation_id)` of the provider operation it was given, but on main no node operation dispatches a bridge worker or writes `provider_operations`, so nothing in this run fills that context. The joined Fabric and Capsule trace of the roadmap row waits for that call site; the pinned vector test shows only that both sides derive the same ids.
- `fabric.approval.ref_digest` is in the span struct and the docs, and no node operation sets it yet.
- A replayed command and a re-sent launch each get a span id of their own, a child of the operation's or the attempt's span (`otel::tests`); this run has neither.
