# Fabric jobs for Python

This preview connects a Python 3.10+ application to a running Fabric node
through its Unix control socket. It speaks client API 2 and managed-job
Facade v1. The same client reaches a paired authority by naming that
authority's node ID and using the local node's socket. There are no runtime
dependencies.

## Install the wheel

From the Fabric repository root, build to a directory outside the checkout:

```sh
uv build --wheel --out-dir /path/to/artifacts sdk/python
python -m pip install /path/to/artifacts/fabric_jobs-0.1.0-py3-none-any.whl
```

The wheel is a preview distributed as a local build artifact.

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
        cursor = page.next
        if page.closed or (not page.events and not page.lagged):
            break
except FacadeRefusedError as error:
    print(error.code, error.reason)
```

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
