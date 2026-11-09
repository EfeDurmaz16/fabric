# PR 12: one managed job's spans in Jaeger

Ran on one host, 2026-11-09: two `fabricd` daemons as two processes on one host, each with its own state root and home, paired over a TCP and TLS node link on 127.0.0.1, both exporting to one local Jaeger. Not two machines.

## The collector

Image `jaegertracing/all-in-one:1.73.0`, digest `sha256:fe884668a2bf8d586b6fb7697d48a6a19a02f9e78a1eddf09c5b0f08c27d63b9`. Its log shows the OTLP receiver starting its HTTP server on `:4318`.

```sh
docker run --rm -d --name fabric-p12-jaeger -e COLLECTOR_OTLP_ENABLED=true \
  -p 127.0.0.1:16686:16686 -p 127.0.0.1:4318:4318 jaegertracing/all-in-one:1.73.0
```

## The run

The scenario of `observability::e2e_spans_of_two_daemons_share_a_trace_and_the_audit_export_holds_no_secret`, by hand with the debug `fabric` binary of this branch. `$ROOT` is a scratch directory, deleted after the run. Each daemon runs with `HOME=$ROOT/home-<n>`, `FABRIC_HOME=$ROOT/<n>`, a fake `codex` first on the `PATH` that answers `--version`, writes a session file and holds (`FAKE_CODEX_HOLD=1`), and `FABRIC_SECRET_DEPLOY_KEY=s3cr3t-value-7c1e-only-in-the-daemon-env`. The CLI commands run with `HOME=$ROOT/home` and the node's `FABRIC_HOME`.

```sh
fabric serve --listen 127.0.0.1:27411 --wire tcp-tls --otlp-endpoint http://127.0.0.1:4318   # a
fabric serve --listen 127.0.0.1:27412 --wire tcp-tls --otlp-endpoint http://127.0.0.1:4318   # b
fabric node add <A> 127.0.0.1:27411                                   # on b
fabric node add <B>                                                   # on a
fabric facade mint --actor e2e --ttl-ms 600000                        # on a
fabric facade submit --authority <A> --token <bearer> --empty \
  --goal 'trace the launch path 4b9d' --harness codex --secret DEPLOY_KEY \
  --drain-budget 1000 --pin <B> --operation-key submit-otel-1         # on b
# once a's inspect shows the attempt running on b:
fabric managed register <JOB> --parent <PRIMARY> --label build --class build \
  --operation-key register-otel-1 -- sh -c 'echo argv-marker-e51f && sleep 30'
fabric managed checkpoint <JOB> --operation-key checkpoint-otel-1
# once the primary task's receipt is checkpointed:
fabric managed continue <JOB> --pin <A> --operation-key continue-otel-1
# once a second attempt runs on a: SIGTERM to both daemons, wait for both to exit, then
curl -s http://127.0.0.1:16686/api/services
curl -s 'http://127.0.0.1:16686/api/traces?service=fabricd&limit=20' > spans-jaeger.json
```

The job ran to the end: attempt 1 at generation 1 on `b`, checkpointed, and attempt 2 at generation 2 running on `a`.

## What Jaeger holds

`/api/services` answered `["jaeger-all-in-one","fabricd"]`. `/api/traces?service=fabricd&limit=20` answered 8 traces with 21 spans and no error, saved as returned, pretty-printed, in [spans-jaeger.json](spans-jaeger.json).

| Node | Spans | Ops |
| --- | --- | --- |
| `a` (authority) | 13 | `node.add`, `managed.create_job`, `managed.register_task`, `managed.checkpoint`, `managed.continue`, `observe.placed` (2), `observe.started` (2), `observe.checkpointed` (2), `observe.continued`, `peer.launch` |
| `b` | 8 | `node.add`, `peer.launch`, `peer.instruct` (3), `report` (3) |

Trace `5e36d7b8ce59258e0a78955f788115b5`, the first attempt's, holds spans of both nodes: `b`'s `fabric peer.launch` (`4e1388b00a05e452`) is its root, and its 9 other spans are children of it: `b`'s three `fabric peer.instruct` and three `fabric report`, and `a`'s `fabric observe.started` and two `fabric observe.checkpointed`. That is 7 spans from `b` and 3 from `a`, the same shape the test's receiver saw ([spans-fake-receiver.json](spans-fake-receiver.json)). The regrant's `fabric observe.placed` on `a` carries `fabric.generation.supersedes = 1`. Every span's `fabric.result` is `done`.

Jaeger marks 12 spans with `clock skew adjustment disabled; not applying calculated delta`. Those are children that start after their parent span ended, such as a report made seconds after the launch it belongs to; a span is the moment a node accepted an operation, not the attempt's lifetime, and Jaeger shows them as recorded.

## What the JSON does not hold

None of these appears in `spans-jaeger.json`, checked with `grep -c` on the file as Jaeger returned it, before anything was written to the repository: the secret value, its variable and secret name (`FABRIC_SECRET`, `DEPLOY_KEY`), the goal, the build's argv (`argv-marker-e51f`, `sleep 30`), the four operation keys, the bearer, the fake harness's name and thread id, `TRACEPARENT`, `/tmp`, `/Users`, the user name and the home directories. The attribute keys present are the `fabric.*` keys of [observability](../../observability.md), `otel.scope.name`, `otel.scope.version`, `otel.status_code`, `span.kind`, and the resource's `service.instance.id` and `service.version`. Nothing was redacted.
