"""The Fabric node's owner-only UNIX socket and client API frame."""

from __future__ import annotations

import json
import os
import re
import socket
import stat
import struct
import time
from pathlib import Path
from typing import Union

JsonValue = Union[None, bool, int, float, str, list["JsonValue"], dict[str, "JsonValue"]]
JsonObject = dict[str, JsonValue]
API_VERSION = 2
SDK_VERSION = "0.1.0"
NODE_ID = re.compile(r"fab1[a-z2-7]{51}[aq]")
MAX_REQUEST = 1 << 20
MAX_FRAME = 16 << 20
_HEADER = struct.Struct("<I")
_SURROGATE = re.compile(r"[\ud800-\udfff]")


class InputError(ValueError):
    """Invalid client input. Messages name the field without including its value."""


class FabricError(Exception):
    """A Fabric transport, protocol, or application error."""


class NoDaemonError(FabricError):
    """The local daemon could not be reached at its socket."""


class UntrustedSocketError(FabricError):
    """The socket or a directory above it failed ownership or mode checks."""


class TransportError(FabricError):
    """The exchange ended early, timed out, or encountered a socket error."""


class ProtocolError(FabricError):
    """A peer's frame or message did not follow the wire contract."""

    def __init__(self, reason: str, detail: str) -> None:
        super().__init__(f"{reason}: {detail}")
        self.reason = reason


class IncompatibleError(FabricError):
    """The local daemon speaks another client API version."""

    def __init__(self, api: int, node_version: str) -> None:
        super().__init__(f"client API {API_VERSION}, daemon API {api}")
        self.api = api
        self.node_version = node_version


class UnsupportedError(FabricError):
    """The daemon did not advertise the Facade client API operation."""


class DaemonRefusedError(FabricError):
    """The outer client API refused the exchange before a Facade answer."""

    def __init__(self, kind: str, code: str) -> None:
        super().__init__(f"daemon refused: {kind}/{code}")
        self.kind = kind
        self.code = code


def _stat(path: Path, shown: Path) -> os.stat_result:
    try:
        return path.lstat()
    except FileNotFoundError:
        raise NoDaemonError(f"no Fabric daemon at {shown}") from None
    except OSError as error:
        raise TransportError(f"cannot check the Fabric socket at {shown}: {error.strerror}") from None


def _private_dir(path: Path, shown: Path, uid: int) -> None:
    info = _stat(path, shown)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != uid or info.st_mode & 0o077:
        raise UntrustedSocketError(f"Fabric state directory is not an owner-private directory: {path}")


def trusted_socket(path: Path) -> Path:
    # A symlink above the state directory (macOS /tmp) can be resolved once.
    # Dial only the canonical path whose ancestors and leaf we checked.
    if not hasattr(os, "geteuid") or not hasattr(socket, "AF_UNIX"):
        raise UntrustedSocketError("a Unix socket with verifiable ownership is required")
    uid = os.geteuid()
    _private_dir(path.parent, path, uid)
    try:
        state = path.parent.resolve(strict=True)
    except FileNotFoundError:
        raise NoDaemonError(f"no Fabric daemon at {path}") from None
    except (OSError, RuntimeError):
        raise UntrustedSocketError(f"cannot resolve the Fabric state directory: {path.parent}") from None
    _private_dir(state, path, uid)
    for parent in state.parents:
        info = _stat(parent, path)
        if (
            not stat.S_ISDIR(info.st_mode)
            or info.st_uid not in (0, uid)
            or (info.st_mode & 0o022 and not info.st_mode & stat.S_ISVTX)
        ):
            raise UntrustedSocketError(f"Fabric socket ancestor can be replaced by another user: {parent}")
    resolved = state / path.name
    info = _stat(resolved, path)
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid != uid:
        raise UntrustedSocketError(f"Fabric socket is not a socket owned by this user: {resolved}")
    return resolved


def encode_json(value: JsonObject) -> bytes:
    try:
        body = json.dumps(value, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode("utf-8")
    except (ValueError, TypeError, UnicodeError, RecursionError):
        raise ValueError("Fabric requests must contain valid JSON and Unicode") from None
    if not 1 <= len(body) <= MAX_REQUEST:
        raise ValueError("Fabric request exceeds the one MiB node limit")
    return body


def encode_frame(value: JsonObject) -> bytes:
    body = encode_json(value)
    return _HEADER.pack(len(body)) + body


def _reject_constant(_: str) -> None:
    raise ValueError("non-JSON constant")


def _check_unicode(value: JsonValue) -> None:
    if isinstance(value, str) and _SURROGATE.search(value):
        raise ValueError("non-Unicode text")
    if isinstance(value, list):
        for item in value:
            _check_unicode(item)
    if isinstance(value, dict):
        for key, item in value.items():
            _check_unicode(key)
            _check_unicode(item)


def _remaining(connection: socket.socket, deadline: float) -> None:
    timeout = deadline - time.monotonic()
    if timeout <= 0:
        raise TransportError("Fabric exchange timed out")
    connection.settimeout(timeout)


def _read_exact(connection: socket.socket, size: int, deadline: float) -> bytearray:
    data = bytearray(size)
    view = memoryview(data)
    while view:
        _remaining(connection, deadline)
        count = connection.recv_into(view)
        if count == 0:
            raise TransportError("Fabric socket closed before the complete response")
        view = view[count:]
    return data


def read_frame(connection: socket.socket, deadline: float) -> JsonObject:
    size, = _HEADER.unpack(_read_exact(connection, _HEADER.size, deadline))
    if not 1 <= size <= MAX_FRAME:
        raise ProtocolError("bad_length", "frame length must be between one byte and 16 MiB")
    return decode_json(_read_exact(connection, size, deadline))


def decode_json(body: bytes | bytearray) -> JsonObject:
    try:
        value = json.loads(body.decode("utf-8"), parse_constant=_reject_constant)
        _check_unicode(value)
    except (UnicodeError, ValueError, RecursionError):
        raise ProtocolError("bad_json", "response is not valid UTF-8 JSON") from None
    if not isinstance(value, dict):
        raise ProtocolError("bad_object", "response must be a JSON object")
    return value


def _welcome(answer: JsonObject) -> None:
    kind = answer.get("type")
    api = answer.get("api")
    version = answer.get("node_version")
    if kind not in ("welcome", "incompatible") or type(api) is not int or not 0 <= api <= 0xFFFFFFFF:
        raise ProtocolError("bad_welcome", "invalid client API welcome")
    if not isinstance(version, str):
        raise ProtocolError("bad_welcome", "daemon version must be text")
    if kind == "incompatible" or api != API_VERSION:
        raise IncompatibleError(api, version)
    node = answer.get("node")
    capabilities = answer.get("capabilities")
    if not isinstance(node, str) or not NODE_ID.fullmatch(node):
        raise ProtocolError("bad_welcome", "daemon node identity is invalid")
    if not isinstance(capabilities, list) or any(not isinstance(item, str) for item in capabilities):
        raise ProtocolError("bad_welcome", "daemon capabilities must be a list of strings")
    if "facade_call" not in capabilities:
        raise UnsupportedError("daemon does not advertise facade_call")


def _response(connection: socket.socket, deadline: float) -> JsonObject:
    while True:
        answer = read_frame(connection, deadline)
        kind = answer.get("type")
        if kind == "event" and isinstance(answer.get("event"), dict):
            continue
        if kind == "done":
            reply = answer.get("reply")
            if isinstance(reply, dict) and reply.get("kind") == "facade_call":
                message = reply.get("message")
                if isinstance(message, dict):
                    return message
        if kind == "refused":
            refusal = answer.get("refusal")
            if isinstance(refusal, dict) and isinstance(refusal.get("reason"), dict):
                name = refusal.get("kind")
                code = refusal["reason"].get("code")
                if isinstance(name, str) and isinstance(code, str):
                    raise DaemonRefusedError(name, code)
        raise ProtocolError("bad_reply", "expected a Facade reply from the client API")


def exchange(path: Path, authority: str, message: JsonObject, timeout: float) -> JsonObject:
    request = encode_frame({"op": "facade_call", "authority": authority, "message": message})
    resolved = trusted_socket(path)
    deadline = time.monotonic() + timeout
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            _remaining(connection, deadline)
            try:
                connection.connect(os.fspath(resolved))
            except (FileNotFoundError, ConnectionRefusedError):
                raise NoDaemonError(f"no Fabric daemon at {path}") from None
            _remaining(connection, deadline)
            connection.sendall(encode_frame({"api": API_VERSION, "client": f"fabric-python/{SDK_VERSION}"}))
            _welcome(read_frame(connection, deadline))
            _remaining(connection, deadline)
            connection.sendall(request)
            return _response(connection, deadline)
    except TimeoutError:
        raise TransportError("Fabric exchange timed out") from None
    except OSError as error:
        raise TransportError(f"Fabric socket exchange failed: {error.strerror}") from None
