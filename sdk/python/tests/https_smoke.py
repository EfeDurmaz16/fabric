"""Run installed Python and npm packages against a real TLS gateway and authority.

Run venv/bin/python -I https_smoke.py PATH_TO_FABRIC NPM_CONSUMER_DIRECTORY.
"""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile

import fabric_jobs
from fabric_jobs import FabricClient, TlsVerificationError

# Under -I only the test helpers need a source path. Both SDKs come from packages.
sys.path.append(str(Path(__file__).resolve().parent))
from installed_smoke import refused, serve


def issue_certificate(root):
    def openssl(*args):
        subprocess.run(["openssl", *args], cwd=root, capture_output=True, check=True, timeout=30)

    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=Fabric smoke CA", "-keyout", "ca.key", "-out", "ca.pem")
    openssl("req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
            "-keyout", "server.key", "-out", "server.csr")
    (root / "extensions").write_text("basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n"
                                     "subjectAltName=IP:127.0.0.1\n")
    openssl("x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
            "-set_serial", "1", "-days", "1", "-extfile", "extensions", "-out", "server.pem")


@contextmanager
def gateway(authority, root):
    with subprocess.Popen([
        authority.binary, "gateway", "--authority", authority.id, "--listen", "127.0.0.1:0",
        "--cert", root / "server.pem", "--key", root / "server.key",
    ], env=authority.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, text=True) as process:
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(process.stdout, selectors.EVENT_READ)
                if not ready.select(15):
                    raise AssertionError("HTTPS gateway did not announce its address")
                yield json.loads(process.stdout.readline())["endpoint"]
        finally:
            process.kill()


def main(binary, consumer):
    assert sys.prefix != sys.base_prefix
    assert Path(fabric_jobs.__file__).resolve().is_relative_to(Path(sys.prefix).resolve())
    assert (consumer / "node_modules/@fabric-jobs/client").is_dir()
    js = Path(__file__).with_name("https_consumer.mjs").read_text()
    with tempfile.TemporaryDirectory(prefix="fht-", dir="/tmp") as directory:
        root = Path(directory).resolve()
        issue_certificate(root)
        with serve(binary, root / "authority") as node, gateway(node, root) as endpoint:
            # An unregistered destination keeps the job parked without running any harness.
            with serve(binary, root / "destination") as destination:
                pin = destination.id
            ca = root / "ca.pem"
            consumer_env = {"PATH": os.environ["PATH"], "HOME": str(consumer)}

            def mint(actor, operations, *scope):
                permissions = [arg for op in operations for arg in ("--allow", op)]
                return node.cli("facade", "mint", "--actor", actor, "--ttl-ms", "120000",
                                "--node", pin, *permissions, *scope)["token"]["bearer"]

            def python(token):
                return FabricClient.from_https(endpoint, token, ca_file=ca)

            def javascript(action, token, **args):
                body = {"endpoint": endpoint, "token": token, "ca": str(ca), "action": action, **args}
                result = subprocess.run(
                    ["node", "--input-type=module", "--eval", js], input=json.dumps(body),
                    cwd=consumer, env=consumer_env, stdout=subprocess.PIPE, text=True, check=True, timeout=30,
                )
                return json.loads(result.stdout)

            writer = mint("https-writer", ["submit"])
            spec = {"goal": "HTTPS smoke", "source": {"kind": "empty"}, "harness": {"kind": "codex"},
                    "acceptance": [], "policy": {"pin": pin}}
            created = python(writer).submit("https-submit", **spec)
            job = created.job
            replay = javascript("retry", writer, spec=spec)
            assert replay["replayed"] and replay["operation"] == created.operation and replay["job"] == job

            reader = mint("https-reader", [], "--job", job)
            try:
                FabricClient.from_https(endpoint, reader).inspect(job)
            except TlsVerificationError:
                pass
            else:
                raise AssertionError("client trusted an unknown certificate")
            refused(lambda: python(reader).inspect("7ZZZZZZZZZZZZZZZZZZZZZZZZZ"), "scope", "job")
            cli = subprocess.run(
                [Path(sys.executable).with_name("fbrc"), "--endpoint", endpoint, "--ca-file", ca, "inspect", job],
                input=reader + "\n", cwd=consumer, env=consumer_env, capture_output=True, text=True, timeout=15,
            )
            assert cli.returncode == 0 and json.loads(cli.stdout)["job_id"] == job

            control = mint("https-control", ["checkpoint", "continue", "cancel"], "--job", job)
            moved = javascript("move", control, job=job, spec=spec)
            retry = python(control).move(job, "https-move", pin=pin)
            assert retry.replayed and retry.operation == moved["operation"]
            first = javascript("read", reader, job=job, cursor=None)
            assert first["events"] and first["next"] == first["events"][-1]["seq"]
            cursor = first["next"]
            cancelled = python(control).cancel(job, "https-cancel", reason="fixture cleanup")
            replay = javascript("cancel", control, job=job)
            assert replay["replayed"] and replay["operation"] == cancelled.operation
            resumed = javascript("read", reader, job=job, cursor=cursor)
            assert resumed["events"] and all(int(e["seq"]) > int(cursor) for e in resumed["events"])
            closed = python(reader).watch(job, after=resumed["next"], wait_ms=0)
            assert not closed.events and closed.closed == "finished"
            assert python(reader).inspect(job).state == "cancelled"
            print("Installed Python, fbrc and Node consumers passed HTTPS lifecycle and replay checks.")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve(strict=True), Path(sys.argv[2]).resolve(strict=True))
