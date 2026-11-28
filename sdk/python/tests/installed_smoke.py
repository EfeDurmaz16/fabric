"""Run the installed wheel against disposable local and paired authorities.

Usage: /path/to/venv/bin/python -I installed_smoke.py /path/to/fabric
"""

import argparse
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from importlib.metadata import version
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import time

import fabric_jobs
from fabric_jobs import FabricClient, FacadeRefusedError


@dataclass(frozen=True)
class Node:
    binary: Path
    state: Path
    env: dict[str, str]
    id: str
    address: str

    def cli(self, *args):
        # The mint reply contains a bearer. Capture it and report failures
        # without echoing any command output.
        result = subprocess.run(
            [self.binary, "--json", *args], env=self.env, capture_output=True, text=True, timeout=20,
        )
        if result.returncode:
            raise AssertionError(f"Fabric {' '.join(args[:2])} failed with exit {result.returncode}")
        return json.loads(result.stdout)

    def client(self, authority, bearer):
        return FabricClient(self.state / "fabric.sock", authority, bearer)


@contextmanager
def serve(binary, directory):
    home = directory / "home"
    state = directory / "state"
    home.mkdir(mode=0o700, parents=True)
    state.mkdir(mode=0o700)
    env = {"HOME": str(home), "FABRIC_HOME": str(state), "PATH": "/usr/bin:/bin"}
    process = subprocess.Popen(
        [binary, "--json", "serve", "--listen", "127.0.0.1:0", "--wire", "tcp-tls"],
        env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True,
    )
    try:
        with selectors.DefaultSelector() as ready:
            ready.register(process.stdout, selectors.EVENT_READ)
            if not ready.select(15):
                raise AssertionError("Fabric daemon did not announce its address")
            line = process.stdout.readline()
        if not line:
            raise AssertionError("Fabric daemon exited before announcing its address")
        meta = json.loads(line)["node"]
        yield Node(binary, state, env, meta["node"], meta["listen"])
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        process.stdout.close()


def refused(call, code, reason=None):
    try:
        call()
    except FacadeRefusedError as error:
        assert error.code == code, error
        if reason is not None:
            assert error.reason == reason, error
        return error
    raise AssertionError(f"expected Facade refusal {code}/{reason}")


def main(binary, legacy_authority):
    package_path = Path(fabric_jobs.__file__).resolve()
    assert package_path.is_relative_to(Path(sys.prefix).resolve()), package_path
    assert sys.prefix != sys.base_prefix, "run the wheel smoke test from a virtual environment"
    scoped_operations = "--allow" in subprocess.run(
        [binary, "facade", "mint", "--help"], capture_output=True, text=True, check=True,
    ).stdout
    if not scoped_operations and not legacy_authority:
        raise AssertionError("Fabric daemon must support explicit Facade operation grants")

    with tempfile.TemporaryDirectory(prefix="fpw-", dir="/tmp") as directory, ExitStack() as stack:
        root = Path(directory).resolve()
        authority = stack.enter_context(serve(binary, root / "a"))
        peer = stack.enter_context(serve(binary, root / "b"))
        authority.cli("node", "add", peer.id, peer.address)
        peer.cli("node", "add", authority.id, authority.address)
        peer.cli("node", "ping", authority.id)

        def mint(actor, allow, *scope_args):
            permissions = [arg for permission in allow for arg in ("--allow", permission)] if scoped_operations else []
            answer = authority.cli("facade", "mint", "--actor", actor, "--ttl-ms", "60000", *permissions, *scope_args)
            return answer["token"]

        writer = mint("wheel-submit", ["submit"], "--node", peer.id)
        local = authority.client(authority.id, writer["bearer"])
        remote_submit = peer.client(authority.id, writer["bearer"])

        def submit(client, goal="Python wheel fixture", pin=peer.id):
            return client.submit(
                "wheel-submit-1", goal=goal, harness={"kind": "codex"}, source={"kind": "empty"},
                policy={"pin": pin},
            )

        refused(lambda: submit(local, pin=None), "scope", "resource")
        created = submit(local)
        assert created.revision == "1" and not created.replayed
        replay = submit(remote_submit)
        assert replay.replayed and replay.operation == created.operation and replay.job == created.job
        refused(lambda: submit(remote_submit, goal="different request"), "conflict")

        read_grant = mint("wheel-read", [], "--job", created.job, "--node", peer.id)
        local_read = authority.client(authority.id, read_grant["bearer"])
        remote_read = peer.client(authority.id, read_grant["bearer"])
        assert local_read.inspect(created.job).job_id == remote_read.inspect(created.job).job_id == created.job
        refused(lambda: remote_read.inspect("7ZZZZZZZZZZZZZZZZZZZZZZZZZ"), "scope", "job")

        first = local_read.watch(created.job, wait_ms=0)
        assert first.events and first.events[0].kind == "command_accepted", first
        assert first.next == first.events[-1].seq
        cursor = first.next
        # Placement can commit a parked explanation after submit. Drain live
        # updates from the peer before checking the bounded empty wait.
        for _ in range(12):
            started = time.monotonic()
            page = remote_read.watch(created.job, after=cursor, wait_ms=100)
            elapsed = time.monotonic() - started
            cursor = page.next
            if not page.events:
                assert 0.075 <= elapsed < 3, elapsed
                break
        else:
            raise AssertionError("watch did not settle to an empty page")

        if scoped_operations:
            refused(lambda: remote_submit.inspect(created.job), "scope", "operation")
            refused(lambda: remote_read.cancel(created.job, "blocked-cancel", reason="test"), "scope", "operation")
            refused(lambda: remote_read.checkpoint(created.job, "blocked-checkpoint"), "scope", "operation")
            refused(lambda: remote_read.move(created.job, "blocked-move", pin=peer.id), "scope", "operation")
            refused(lambda: remote_read.submit(
                "blocked-submit", goal="test", harness={"kind": "codex"}, source={"kind": "empty"},
            ), "scope", "operation")

        command_grant = mint(
            "wheel-command", ["checkpoint", "continue", "cancel"], "--job", created.job, "--node", peer.id,
        )
        control = peer.client(authority.id, command_grant["bearer"])
        refused(lambda: control.move(created.job, "wheel-move-wrong-pin", pin=authority.id), "scope", "resource")
        refused(lambda: control.checkpoint(created.job, "wheel-checkpoint"), "refused", "not_live")
        stale = refused(lambda: control.move(
            created.job, "wheel-stale-move", expected_revision="999999", pin=peer.id,
        ), "revision_mismatch")
        assert stale.current_revision is not None

        cancelled = control.cancel(created.job, "wheel-cancel", reason="clean up fixture")
        assert cancelled.job == created.job and not cancelled.replayed
        assert control.cancel(created.job, "wheel-cancel", reason="clean up fixture").replayed
        restarted_client = peer.client(authority.id, read_grant["bearer"])
        after_cancel = restarted_client.watch(created.job, after=cursor, wait_ms=100)
        assert after_cancel.events
        assert all(int(event.seq) > int(cursor) for event in after_cancel.events)
        assert restarted_client.inspect(created.job).state == "cancelled"
        closed = restarted_client.watch(created.job, after=after_cancel.next, wait_ms=100)
        assert closed.closed == "finished" and not closed.events
        authority.cli("facade", "revoke", read_grant["token_id"])
        refused(lambda: restarted_client.inspect(created.job), "unauthorized", "revoked")

        print(json.dumps({
            "installed_version": version("fabric-jobs"),
            "python": sys.version.split()[0],
            "paired_peer": True,
            "same_job": created.job,
            "duplicate_submit_replayed": replay.replayed,
            "initial_cursor": first.next,
            "reconnect_cursor": after_cancel.next,
            "finished_watch": closed.closed,
            "operation_scope_checks": scoped_operations,
        }))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fabric", type=Path)
    parser.add_argument("--legacy-authority", action="store_true", help="exercise the older token contract")
    args = parser.parse_args()
    main(args.fabric.resolve(strict=True), args.legacy_authority)
