"""Read fixtures shared by the Rust and TypeScript Facade clients."""

import copy
import json
import socket
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from fabric_jobs import (
    Accepted,
    FabricClient,
    FacadeRefusedError,
    IncompatibleError,
    Inspection,
    ProtocolError,
    TransportError,
    UnsupportedError,
    UntrustedSocketError,
    WatchPage,
)
from fabric_jobs._facade import _read_answer
from fabric_jobs._wire import API_VERSION, MAX_FRAME, _welcome, encode_frame, read_frame, trusted_socket

ROOT = Path(__file__).resolve().parents[3]
FACADES = ROOT / "protocol" / "facade" / "v1"
CASES = FACADES / "cases"
AUTHORITY = "fab1" + "a" * 52


def fixture(name):
    return json.loads((CASES / f"{name}.json").read_text(encoding="utf-8"))


class FacadeContract(unittest.TestCase):
    def test_shared_reply_fixtures(self):
        for path in CASES.glob("*.json"):
            case = fixture(path.stem)
            if case["direction"] != "authority_to_client":
                continue
            with self.subTest(case=path.stem):
                message = case["message"]
                expect = case["expect"]
                if expect["outcome"] != "accept":
                    with self.assertRaises(ProtocolError) as caught:
                        _read_answer(message, "fixture-token")
                    self.assertEqual(caught.exception.reason, expect["reason"])
                elif message["type"] == "refused":
                    with self.assertRaises(FacadeRefusedError) as caught:
                        _read_answer(message, "fixture-token")
                    self.assertEqual(caught.exception.code, message["code"])
                    self.assertEqual(caught.exception.reason, message["reason"])
                    self.assertEqual(caught.exception.current_revision, message.get("current_revision"))
                else:
                    decoded = _read_answer(message, "fixture-token")
                    if isinstance(decoded, Accepted):
                        self.assertEqual(decoded.operation, message["operation"])
                        self.assertEqual(decoded.job, message["job"])
                        self.assertEqual(decoded.revision, message["revision"])
                        self.assertEqual(decoded.replayed, message["replayed"])
                    elif isinstance(decoded, Inspection):
                        self.assertEqual(decoded.job_id, message["job"]["job"])
                        self.assertEqual(decoded.revision, message["job"]["revision"])
                        self.assertEqual(decoded.data, message)
                    else:
                        self.assertIsInstance(decoded, WatchPage)
                        self.assertEqual(decoded.next, message["next"])
                        self.assertEqual(decoded.lagged, message["lagged"])
                        self.assertEqual(decoded.closed, message["closed"])
                        self.assertEqual([event.seq for event in decoded.events], [e["seq"] for e in message["events"]])

    def test_shared_request_fixtures(self):
        requests = [
            "submit_valid", "submit_duplicate_key", "submit_empty_valid", "submit_snapshot_valid",
            "inspect_valid", "watch_valid", "watch_from_cursor", "cancel_valid", "cancel_task_valid",
            "checkpoint_valid", "continue_valid", "continue_pinned_valid",
        ]
        for name in requests:
            with self.subTest(case=name):
                message = fixture(name)["message"]
                client = FabricClient("/no/fabric.sock", AUTHORITY, message["token"])
                args = {key: value for key, value in message.items() if key not in {"type", "facade", "token"}}
                operation = message["type"]
                if operation == "submit":
                    args.update(args.pop("intent"))
                if operation == "continue":
                    operation = "move"
                with patch("fabric_jobs._facade.exchange", return_value=fixture("refused_valid")["message"]) as wire:
                    with self.assertRaises(FacadeRefusedError):
                        getattr(client, operation)(**args)
                _, authority, sent, _ = wire.call_args.args
                self.assertEqual(authority, AUTHORITY)
                self.assertEqual(sent, message)

    def test_watch_rejects_replayed_events_and_invalid_cursors(self):
        data = fixture("events_valid")["message"]
        client = FabricClient("/no/fabric.sock", AUTHORITY, "1" * 64)
        with self.assertRaises(ValueError):
            client.watch(data["job"], after="01")
        with self.assertRaises(ValueError):
            client.watch(data["job"], wait_ms=25_001)
        with patch("fabric_jobs._facade.exchange", return_value=data):
            with self.assertRaises(ProtocolError) as caught:
                client.watch(data["job"], after=data["events"][0]["seq"])
            self.assertEqual(caught.exception.reason, "bad_cursor")

    def test_shared_watch_cursors(self):
        client = FabricClient("/no/fabric.sock", AUTHORITY, "1" * 64)
        for case in json.loads((ROOT / "sdk/watch-cursors.json").read_text()):
            page = fixture("events_valid")["message"]
            event = page["events"][0]
            page.update(events=[{**event, "seq": seq} for seq in case["sequences"]], next=case["next"])
            with self.subTest(case=case), patch("fabric_jobs._facade.exchange", return_value=page):
                if case["valid"]:
                    self.assertEqual(client.watch(page["job"], after=case["after"]).next, case["next"])
                else:
                    with self.assertRaises(ProtocolError) as caught:
                        client.watch(page["job"], after=case["after"])
                    self.assertEqual(caught.exception.reason, "bad_cursor")

    def test_refusal_redacts_the_bearer(self):
        token = "1" * 64
        data = copy.deepcopy(fixture("refused_valid")["message"])
        data["message"] = f"echo {token}"
        data["reason"] = f"echo {token}"
        with self.assertRaises(FacadeRefusedError) as caught:
            _read_answer(data, token)
        self.assertNotIn(token, str(caught.exception))
        self.assertNotIn(token, caught.exception.message)


class ClientWireContract(unittest.TestCase):
    def test_shared_frame_bytes(self):
        data = json.loads((ROOT / "protocol" / "fixtures" / "frame" / "hello.json").read_text(encoding="utf-8"))
        self.assertEqual(encode_frame(data["value"]).hex(), data["hex"])

    def test_welcome_requires_matching_version_and_facade_capability(self):
        base = ROOT / "protocol" / "fixtures" / "hello_answer"
        welcome = json.loads((base / "welcome_with_placement_and_facade.json").read_text(encoding="utf-8"))
        _welcome({**welcome, "api": API_VERSION})
        with self.assertRaises(IncompatibleError):
            _welcome({**welcome, "api": API_VERSION + 1})
        with self.assertRaises(UnsupportedError):
            _welcome({**welcome, "api": API_VERSION, "capabilities": []})

    def test_rejects_invalid_lengths_before_body(self):
        for size in (0, MAX_FRAME + 1):
            with self.subTest(size=size):
                self.check_wire_error(size.to_bytes(4, "little"), ProtocolError)

    def test_truncation_invalid_utf8_and_non_unicode_json(self):
        payloads = (
            (b"\x05\0\0\0{}", TransportError),
            (b"\x01\0\0\0\xff", ProtocolError),
            (b'{"a":"\\ud800"}', ProtocolError),
        )
        for wire, error in payloads:
            if wire.startswith(b"{"):
                wire = len(wire).to_bytes(4, "little") + wire
            with self.subTest(error=error):
                self.check_wire_error(wire, error)

    def check_wire_error(self, wire, error):
        left, right = socket.socketpair()
        with left, right:
            left.sendall(wire)
            left.shutdown(socket.SHUT_WR)
            with self.assertRaises(error):
                read_frame(right, time.monotonic() + 2)

    def test_rejects_untrusted_socket_inputs(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)
            public = path / "public"
            public.mkdir(mode=0o755)
            with self.assertRaises(UntrustedSocketError):
                trusted_socket(public / "fabric.sock")
            private = path / "private"
            private.mkdir(mode=0o700)
            alias = path / "alias"
            alias.symlink_to(private, target_is_directory=True)
            with self.assertRaises(UntrustedSocketError):
                trusted_socket(alias / "fabric.sock")
            leaf = private / "fabric.sock"
            leaf.symlink_to(private / "elsewhere")
            with self.assertRaises(UntrustedSocketError):
                trusted_socket(leaf)
            writable = path / "writable"
            writable.mkdir(mode=0o777)
            writable.chmod(0o777)
            child = writable / "state"
            child.mkdir(mode=0o700)
            with self.assertRaises(UntrustedSocketError):
                trusted_socket(child / "fabric.sock")
