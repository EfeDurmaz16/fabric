"""Agent-friendly HTTPS commands for the managed-job Facade."""

import argparse
import json
import sys
from dataclasses import asdict
from pathlib import Path

from . import FabricClient, FabricError, FacadeRefusedError, HttpStatusError
from ._wire import InputError, MAX_FRAME, decode_json


class UsageError(InputError):
    pass


class Parser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        # argparse can echo arbitrary argv, including accidentally supplied credentials.
        names = {name for action in self._actions for name in (*action.option_strings, action.dest)}
        required = "the following arguments are required: "
        if message.startswith(required):
            missing = message[len(required):].split(", ")
            if all(field in names for field in missing):
                raise UsageError("required: " + ", ".join(missing))
        field = message.partition(":")[0].removeprefix("argument ")
        if field in names:
            raise UsageError(f"invalid or missing value for {field}; see --help")
        raise UsageError("invalid arguments; see --help")


def _parser() -> Parser:
    parser = Parser(prog="fbrc", description="Control Fabric jobs over HTTPS. Read one bearer from standard input.")
    parser.add_argument("--endpoint", required=True, help="Full HTTPS gateway URL ending in /v1/facade")
    parser.add_argument("--ca-file", help="PEM certificate authority file (default: system trust)")
    commands = parser.add_subparsers(dest="command", required=True)
    submit = commands.add_parser("submit", help="Create a job from a JSON specification")
    submit.add_argument("--spec", required=True, type=Path, help="File containing goal, harness, source and optional acceptance and policy")
    submit.add_argument("--key", required=True, help="Stable idempotency key")
    for name, help_text in (
        ("inspect", "Read the current job state"),
        ("watch", "Read one bounded event page"),
        ("checkpoint", "Request a checkpoint"),
        ("move", "Request continuation on a worker"),
        ("cancel", "Request cancellation"),
    ):
        sub = commands.add_parser(name, help=help_text)
        sub.add_argument("job", help="Canonical job ULID")
        if name in ("checkpoint", "move", "cancel"):
            sub.add_argument("--key", required=True, help="Stable idempotency key")
            sub.add_argument("--expected-revision", help="Only accept if the revision still matches")
        if name == "watch":
            sub.add_argument("--after", help="Resume after this event cursor")
            sub.add_argument("--wait-ms", type=int, help="Authority wait in milliseconds, 0 to 25000")
        if name == "cancel":
            sub.add_argument("--reason", required=True)
            sub.add_argument("--task", help="Restrict cancellation to this task ULID")
        if name == "move":
            sub.add_argument("--pin", help="Destination Fabric node ID")
            sub.add_argument("--fidelity", action="append", choices=("native_session", "new_native_session_with_package", "workspace_only"))
    return parser


def _submit_spec(path: Path) -> dict:
    try:
        with path.open("rb") as source:
            body = source.read(MAX_FRAME + 1)
    except OSError as error:
        raise UsageError("cannot read --spec file") from error
    if len(body) > MAX_FRAME:
        raise UsageError("spec exceeds frame limit")
    try:
        spec = decode_json(body)
    except FabricError as error:
        raise UsageError("spec must contain a JSON object") from error
    required = {"goal": str, "harness": dict, "source": dict}
    if any(not isinstance(spec.get(key), kind) for key, kind in required.items()):
        raise UsageError("spec requires a goal string and harness and source objects")
    if spec.keys() - (required.keys() | {"acceptance", "policy"}):
        raise UsageError("unknown spec field")
    acceptance = spec.get("acceptance", [])
    if not isinstance(acceptance, list) or not all(isinstance(item, str) for item in acceptance):
        raise UsageError("acceptance must be an array of strings")
    if spec.get("policy") is not None and not isinstance(spec["policy"], dict):
        raise UsageError("policy must be an object")
    return spec


def _run(client: FabricClient, args: argparse.Namespace):
    if args.command == "submit":
        return client.submit(args.key, **_submit_spec(args.spec))
    if args.command == "inspect":
        return client.inspect(args.job)
    if args.command == "watch":
        return client.watch(args.job, after=args.after, wait_ms=args.wait_ms)
    if args.command == "checkpoint":
        return client.checkpoint(args.job, args.key, expected_revision=args.expected_revision)
    if args.command == "move":
        return client.move(args.job, args.key, expected_revision=args.expected_revision, pin=args.pin, fidelity=args.fidelity)
    return client.cancel(args.job, args.key, expected_revision=args.expected_revision, reason=args.reason, task=args.task)


def main() -> int:
    try:
        args = _parser().parse_args()
        if sys.stdin.isatty():
            raise UsageError("pipe the bearer on standard input")
        token = sys.stdin.readline(67).rstrip("\r\n")
        try:
            client = FabricClient.from_https(args.endpoint, token, ca_file=args.ca_file)
        except OSError as error:
            raise UsageError("cannot load TLS trust roots; check --ca-file") from error
        print(json.dumps(asdict(_run(client, args)), separators=(",", ":")))
        return 0
    except FacadeRefusedError as exc:
        error = {"error": "refused", "code": exc.code, "reason": exc.reason, "current_revision": exc.current_revision}
    except HttpStatusError as exc:
        error = {"error": "http_status", "status": exc.status}
    except FabricError as exc:
        error = {"error": type(exc).__name__}
    except (ValueError, OSError) as exc:
        print(json.dumps({"error": "invalid_input", "message": str(exc) if isinstance(exc, InputError) else "check input and file paths"}), file=sys.stderr)
        return 2
    print(json.dumps(error), file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
