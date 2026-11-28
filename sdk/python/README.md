# Fabric jobs for Python

This preview gives Python 3.10+ applications access to Fabric's managed-job
Facade v1. Use a Fabric HTTPS gateway or client API 2 over the local Unix
control socket. A local node can also forward calls to a paired authority.
There are no runtime dependencies.

[Run your first job over HTTPS](https://github.com/EfeDurmaz16/fabric/blob/3c7c776d/examples/first-https-job/README.md) walks through a clean install, authority startup, token grants, submission and a completed turn.

## Install the wheel

From the Fabric repository root, build to a directory outside the checkout:

```sh
uv build --wheel --out-dir /path/to/artifacts sdk/python
python -m pip install /path/to/artifacts/fabric_jobs-0.1.0-py3-none-any.whl
```

The wheel is a preview distributed as a local build artifact.

## HTTPS CLI

The wheel installs `fbrc`. Scripts and agents can also invoke `python -m fabric_jobs`.
Set `gateway` to the full HTTPS endpoint, including `/v1/facade`. Supply the bearer
as a single line on standard input, for example by redirecting a private credential
file. For a private CA, add `--ca-file ca.pem` before the command.

```sh
fbrc --endpoint "$gateway" inspect "$job" < "$token_file"
fbrc --endpoint "$gateway" watch "$job" --wait-ms 1000 < "$token_file"
fbrc --endpoint "$gateway" submit --key ci-run-1 --spec job.json < "$token_file"
```

A submit specification contains the same fields as the Python method.

```json
{"goal":"Review the fixture","harness":{"kind":"codex"},"source":{"kind":"empty"}}
```

Run `fbrc --help` for all six commands and `fbrc move --help` for placement
options. A successful call writes one JSON object to stdout (exit 0). Errors
write one JSON object to stderr (exit 1 for a refused or failed request, 2 for
invalid input). Persist the operation key before a mutation and the cursor after
processing an event page.


## Inspect and watch

Use the authority node ID from `fabric node id` and a bearer minted by the
authority owner. A new Facade token defaults to inspect and watch. The
owner can restrict it to the job with `fabric facade mint --actor report
--job JOB_ID --ttl-ms 300000`. Inject the bearer from a secrets manager
or another trusted source. Keep it out of command arguments, source files,
and logs.

```python
import os

from fabric_jobs import FabricClient, FacadeRefusedError

client = FabricClient(
    os.environ["FABRIC_SOCKET"],
    os.environ["FABRIC_AUTHORITY"],
    os.environ["FABRIC_FACADE_TOKEN"],
)
job = os.environ["FABRIC_JOB"]
try:
    inspection = client.inspect(job)
    print(inspection.job_id, inspection.revision, inspection.state)

    cursor = None
    while True:
        page = client.watch(job, after=cursor, wait_ms=1000)
        for event in page.events:
            print(event.seq, event.kind)
        cursor = page.next or cursor
        if page.closed:
            break
except FacadeRefusedError as error:
    print(error.code, error.reason)
```

An idle page may be followed by new events. The loop above observes the job until it closes. For a single turn, inspect the latest attempt status as in the first-job guide.

Persist the last fully handled cursor if the consumer needs to resume after
a process exit. Each watch is a bounded call with at most 256 events and an
authority wait of up to 25 seconds. Reuse `page.next` after timeouts or
network disconnects only once you have received and handled that page. If a
call failed before it returned a page, retry from the prior cursor.

## Commands

`submit`, `checkpoint`, `move`, and `cancel` take caller-supplied
operation keys. Persist an operation key with its exact request before
sending and reuse it when retrying an uncertain result. An `Accepted`
answer reports the committed operation and revision. Use inspection and
events to learn the outcome. `move` uses the Facade's `continue` operation.

```python
accepted = client.submit(
    "ci-run-2026-11-28-1",
    goal="Review the fixture",
    harness={"kind": "codex"},
    source={"kind": "empty"},
)
print(accepted.job, accepted.operation, accepted.revision)
```

The example requires a token with explicit `--allow submit` and a job
scope that permits creating jobs. Use separate grants for read access and
for commands. The authority checks operation, job and placement scopes
and rejects expired or revoked tokens. Older authorities and legacy
tokens may allow commands under broader scopes. Use a privately owned
socket and state directory. A socket on a paired node is
how this client reaches a remote authority.

## Connect over HTTPS

Use the operator-provided `https://host:port/v1/facade` endpoint. The
gateway selects the authority. All six job methods work with either
transport.

```python
import os
from fabric_jobs import FabricClient

client = FabricClient.from_https(
    os.environ["FABRIC_ENDPOINT"],
    os.environ["FABRIC_FACADE_TOKEN"],
)
print(client.inspect(os.environ["FABRIC_JOB"]).state)
```

The client checks the certificate and hostname against the system trust
store. For a private CA, pass its PEM bundle with `ca_file=`. The bearer
travels in the Authorization header. Redirects and non-200 responses raise
`HttpStatusError`. The gateway's HTTP 200 Facade refusals raise
`FacadeRefusedError` as they do over the Unix socket. The timeout covers
connection and response I/O, with DNS resolution handled by the platform.

## Verification

```sh
PYTHONPATH=sdk/python/src python -m unittest discover -s sdk/python/tests -p 'test_*.py'
```

The installed-wheel smoke test starts two disposable loopback Fabric nodes.
It requires a built `fabric` binary and permission to bind Unix and loopback
sockets. Use an authority binary supporting explicit operation grants.
Run it with a clean environment's Python after installing the wheel:

```sh
/path/to/venv/bin/python -I sdk/python/tests/installed_smoke.py /absolute/path/to/fabric
```
