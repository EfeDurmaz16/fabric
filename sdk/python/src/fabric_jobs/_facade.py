"""The six managed-job Facade v1 operations."""

from __future__ import annotations

import math
import os
import re
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from ._https import HttpsEndpoint
from ._wire import InputError, NODE_ID, FabricError, JsonObject, JsonValue, ProtocolError, exchange

FACADE_VERSION = 1
MAX_WAIT_MS = 25_000
WATCH_WINDOW = 256
_ID = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}")
_KEY = re.compile(r"[A-Za-z0-9._:-]{1,128}")
_TOKEN = re.compile(r"[0-9a-f]{64}")
_COUNTER = re.compile(r"[1-9][0-9]{0,19}")
_U64_MAX = (1 << 64) - 1
_COUNTER_FIELDS = {"revision", "expected_revision", "current_revision", "generation", "seq", "next", "after"}
_REFUSAL_CODES = {
    "unsupported_version", "bad_request", "unauthorized", "scope", "unknown_job",
    "conflict", "revision_mismatch", "refused", "unavailable",
}
Fidelity = Literal["native_session", "new_native_session_with_package", "workspace_only"]


class FacadeRefusedError(FabricError):
    """An authority refusal with machine-readable code and reason."""

    def __init__(self, code: str, reason: str, message: str, current_revision: str | None) -> None:
        super().__init__(f"{code}/{reason}")
        self.code = code
        self.reason = reason
        self.message = message
        self.current_revision = current_revision


@dataclass(frozen=True)
class Accepted:
    operation: str
    key: str
    job: str
    revision: str
    replayed: bool


@dataclass(frozen=True)
class Inspection:
    job_id: str
    revision: str
    state: str
    data: JsonObject


@dataclass(frozen=True)
class Event:
    seq: str
    revision: str
    at_ms: int
    kind: str
    data: JsonObject


@dataclass(frozen=True)
class WatchPage:
    job: str
    events: tuple[Event, ...]
    next: str | None
    lagged: bool
    closed: str | None


def _matches(value: object, pattern: re.Pattern[str]) -> bool:
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def _is_counter(value: object) -> bool:
    return isinstance(value, str) and _COUNTER.fullmatch(value) is not None and int(value) <= _U64_MAX


def _optional_counter(value: str | None, field: str) -> None:
    if value is not None and not _is_counter(value):
        raise InputError(f"{field} must be a canonical decimal counter or None")


def _check_counters(value: JsonValue) -> None:
    if isinstance(value, list):
        for item in value:
            _check_counters(item)
    if isinstance(value, dict):
        for name, item in value.items():
            if name in _COUNTER_FIELDS and item is not None and not isinstance(item, str):
                raise ProtocolError("bad_integer", f"{name} must travel as a decimal string")
            _check_counters(item)


def _text(data: JsonObject, field: str) -> str:
    value = data.get(field)
    if not isinstance(value, str):
        raise ProtocolError("missing_field" if field not in data else "bad_string", field)
    return value


def _id(data: JsonObject, field: str) -> str:
    value = _text(data, field)
    if not _ID.fullmatch(value):
        raise ProtocolError("bad_string", f"{field} is not a ULID")
    return value


def _counter(data: JsonObject, field: str) -> str:
    value = data.get(field)
    if not isinstance(value, str) or not _is_counter(value):
        raise ProtocolError("bad_integer", f"{field} is not a canonical decimal counter")
    return value


def _nullable_text(data: JsonObject, field: str) -> str | None:
    if field in data and data[field] is None:
        return None
    return _text(data, field)


def _boolean(data: JsonObject, field: str) -> bool:
    value = data.get(field)
    if not isinstance(value, bool):
        raise ProtocolError("bad_string", f"{field} is not a boolean")
    return value


def _event(data: JsonValue) -> Event:
    if not isinstance(data, dict):
        raise ProtocolError("bad_string", "event envelope must be an object")
    at_ms = data.get("at_ms")
    event = data.get("event")
    if type(at_ms) is not int or not 0 <= at_ms <= (1 << 53) - 1:
        raise ProtocolError("bad_integer", "at_ms must be an exact nonnegative integer")
    if not isinstance(event, dict):
        raise ProtocolError("bad_string", "event must be an object")
    return Event(_counter(data, "seq"), _counter(data, "revision"), at_ms, _text(event, "kind"), event)


def _read_page(data: JsonObject) -> WatchPage:
    values = data.get("events")
    if not isinstance(values, list) or len(values) > WATCH_WINDOW:
        raise ProtocolError("bad_string", f"events must be an array of at most {WATCH_WINDOW} items")
    cursor = _nullable_text(data, "next")
    if cursor is not None:
        _counter(data, "next")
    return WatchPage(
        _id(data, "job"), tuple(_event(value) for value in values), cursor,
        _boolean(data, "lagged"), _nullable_text(data, "closed"),
    )


def _read_inspection(data: JsonObject) -> Inspection:
    job = data.get("job")
    if not isinstance(job, dict):
        raise ProtocolError("bad_string", "inspection job must be an object")
    _counter(job, "generation")
    _text(job, "goal")
    _id(job, "primary_task")
    if not isinstance(job.get("harness"), dict):
        raise ProtocolError("bad_string", "inspection harness must be an object")
    for field in ("tasks", "attempts"):
        if not isinstance(job.get(field), list):
            raise ProtocolError("bad_string", f"inspection job {field} must be an array")
    _boolean(data, "parked")
    for field in ("placements", "continuations", "notes", "provider_operations", "blockers", "residuals"):
        if not isinstance(data.get(field), list):
            raise ProtocolError("bad_string", f"inspection {field} must be an array")
    return Inspection(_id(job, "job"), _counter(job, "revision"), _text(job, "state"), data)


def _read_answer(data: JsonObject, token: str) -> Accepted | Inspection | WatchPage:
    kind = _text(data, "type")
    if kind not in ("accepted", "inspection", "events", "refused"):
        raise ProtocolError("unknown_variant", "unrecognized Facade answer type")
    version = data.get("facade")
    if type(version) is not int or version < 0:
        raise ProtocolError("bad_integer", "facade")
    if version != FACADE_VERSION:
        raise ProtocolError("unknown_variant", "unsupported Facade answer version")
    try:
        _check_counters(data)
    except RecursionError:
        raise ProtocolError("bad_object", "Facade answer nests too deeply") from None
    if kind == "accepted":
        key = _text(data, "key")
        if not _KEY.fullmatch(key):
            raise ProtocolError("bad_string", "operation key is invalid")
        return Accepted(
            _id(data, "operation"), key, _id(data, "job"), _counter(data, "revision"), _boolean(data, "replayed"),
        )
    if kind == "inspection":
        return _read_inspection(data)
    if kind == "events":
        return _read_page(data)

    code = _text(data, "code")
    if code not in _REFUSAL_CODES:
        raise ProtocolError("unknown_variant", "unrecognized Facade refusal code")
    reason = _text(data, "reason").replace(token, "[redacted]")
    message = data.get("message")
    current = None if data.get("current_revision") is None else _counter(data, "current_revision")
    raise FacadeRefusedError(
        code, reason, message.replace(token, "[redacted]") if isinstance(message, str) else f"{code}: {reason}", current,
    )


class FabricClient:
    """Synchronous Facade v1 client. Every call opens a new connection."""

    def __init__(
        self,
        socket_path: str | os.PathLike[str],
        authority: str,
        token: str,
        *,
        timeout: float = 35.0,
    ) -> None:
        if not _matches(authority, NODE_ID):
            raise InputError("authority must be a canonical Fabric node ID")
        path = Path(os.path.abspath(socket_path))

        def send(message: JsonObject, bearer: str, budget: float) -> JsonObject:
            return exchange(path, authority, {**message, "token": bearer}, budget)

        self._configure(send, token, timeout)

    @classmethod
    def from_https(
        cls,
        endpoint: str,
        token: str,
        *,
        ca_file: str | os.PathLike[str] | None = None,
        timeout: float = 35.0,
    ) -> FabricClient:
        """Use the authority configured by a Fabric HTTPS gateway."""
        client = cls.__new__(cls)
        client._configure(HttpsEndpoint(endpoint, ca_file), token, timeout)
        return client

    def _configure(
        self, send: Callable[[JsonObject, str, float], JsonObject], token: str, timeout: float,
    ) -> None:
        if not _matches(token, _TOKEN):
            raise InputError("token must contain 64 lowercase hexadecimal characters")
        if isinstance(timeout, bool) or not math.isfinite(timeout) or timeout <= 0:
            raise InputError("timeout must be finite and positive")
        self._send = send
        self.__token = token
        self._timeout = timeout

    def _call(self, operation: str, fields: JsonObject) -> Accepted | Inspection | WatchPage:
        message: JsonObject = {"type": operation, "facade": FACADE_VERSION, **fields}
        return _read_answer(self._send(message, self.__token, self._timeout), self.__token)

    def _command(self, operation: str, key: str, fields: JsonObject, job: str | None = None) -> Accepted:
        if not _matches(key, _KEY):
            raise InputError("operation key must have 1 to 128 characters from A-Z, a-z, 0-9, . _ : -")
        result = self._call(operation, {"key": key, **fields})
        if not isinstance(result, Accepted) or result.key != key or (job is not None and result.job != job):
            raise ProtocolError("unexpected_answer", "expected acceptance for this operation key and job")
        return result

    @staticmethod
    def _job_fields(job: str, expected_revision: str | None) -> JsonObject:
        if not _matches(job, _ID):
            raise InputError("job must be a canonical ULID")
        _optional_counter(expected_revision, "expected_revision")
        return {"job": job, "expected_revision": expected_revision}

    def submit(
        self,
        key: str,
        *,
        goal: str,
        harness: Mapping[str, JsonValue],
        source: Mapping[str, JsonValue],
        acceptance: Sequence[str] = (),
        policy: Mapping[str, JsonValue] | None = None,
    ) -> Accepted:
        """Submit a portable job. Reuse key and request when retrying."""
        fields: JsonObject = {
            "intent": {"goal": goal, "harness": dict(harness), "acceptance": list(acceptance)},
            "source": dict(source),
        }
        if policy is not None:
            fields["policy"] = {"pin": None, **policy}
        return self._command("submit", key, fields)

    def inspect(self, job: str) -> Inspection:
        """Read the authority's current managed-job projection."""
        if not _matches(job, _ID):
            raise InputError("job must be a canonical ULID")
        result = self._call("inspect", {"job": job})
        if not isinstance(result, Inspection) or result.job_id != job:
            raise ProtocolError("unexpected_answer", "expected inspection for this job")
        return result

    def watch(self, job: str, *, after: str | None = None, wait_ms: int | None = None) -> WatchPage:
        """Read one bounded event page. Pass next as after on the following call."""
        if not _matches(job, _ID):
            raise InputError("job must be a canonical ULID")
        _optional_counter(after, "after")
        fields: JsonObject = {"job": job, "after": after}
        if wait_ms is not None:
            if type(wait_ms) is not int or not 0 <= wait_ms <= MAX_WAIT_MS:
                raise InputError(f"wait_ms must be an integer from 0 to {MAX_WAIT_MS}")
            fields["wait_ms"] = wait_ms
        result = self._call("watch", fields)
        if not isinstance(result, WatchPage) or result.job != job:
            raise ProtocolError("unexpected_answer", "expected events for this job")

        last = 0 if after is None else int(after)
        for event in result.events:
            if int(event.seq) <= last:
                raise ProtocolError("bad_cursor", "event sequence did not advance")
            last = int(event.seq)
        if result.events and result.next != result.events[-1].seq:
            raise ProtocolError("bad_cursor", "cursor does not match the last event")
        if not result.events and result.next != after:
            raise ProtocolError("bad_cursor", "empty page must preserve the requested cursor")
        return result

    def cancel(
        self,
        job: str,
        key: str,
        *,
        reason: str,
        expected_revision: str | None = None,
        task: str | None = None,
    ) -> Accepted:
        """Request cancellation of a whole job or an explicitly named task."""
        if task is not None and not _matches(task, _ID):
            raise InputError("task must be a canonical ULID or None")
        fields: JsonObject = {**self._job_fields(job, expected_revision), "reason": reason, "task": task}
        return self._command("cancel", key, fields, job)

    def checkpoint(self, job: str, key: str, *, expected_revision: str | None = None) -> Accepted:
        """Request a checkpoint. Acceptance does not mean bytes were acknowledged."""
        return self._command("checkpoint", key, self._job_fields(job, expected_revision), job)

    def move(
        self,
        job: str,
        key: str,
        *,
        expected_revision: str | None = None,
        fidelity: Sequence[Fidelity] | None = None,
        pin: str | None = None,
    ) -> Accepted:
        """Request continuation using Facade's continue operation."""
        if pin is not None and not _matches(pin, NODE_ID):
            raise InputError("pin must be a canonical Fabric node ID or None")
        fields: JsonObject = {**self._job_fields(job, expected_revision), "pin": pin}
        if fidelity is not None:
            fields["fidelity"] = list(fidelity)
        return self._command("continue", key, fields, job)
