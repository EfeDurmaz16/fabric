import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

from fabric_jobs import Accepted, FacadeRefusedError
from fabric_jobs.__main__ import main


class CliTests(unittest.TestCase):
    endpoint = "https://fabric.example/v1/facade"
    token = "a" * 64

    def invoke(self, args):
        stdout, stderr = io.StringIO(), io.StringIO()
        with (
            patch("sys.argv", ["fbrc", "--endpoint", self.endpoint, *args]),
            patch("sys.stdin", io.StringIO(self.token + "\n")),
            redirect_stdout(stdout),
            redirect_stderr(stderr),
        ):
            code = main()
        return code, stdout.getvalue(), stderr.getvalue()

    @patch("fabric_jobs.__main__.FabricClient.from_https")
    def test_submit_and_read_one_bearer(self, connect):
        spec = {"goal": "demo", "harness": {"kind": "codex"}, "source": {"kind": "empty"}}
        connect.return_value.submit.return_value = Accepted("op", "ci-run", "job", "1", False)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "job.json"
            path.write_text(json.dumps(spec))
            code, stdout, stderr = self.invoke(["submit", "--spec", str(path), "--key", "ci-run"])
        self.assertEqual((code, stderr), (0, ""))
        self.assertEqual(json.loads(stdout)["revision"], "1")
        connect.assert_called_once_with(self.endpoint, self.token, ca_file=None)
        connect.return_value.submit.assert_called_once_with("ci-run", **spec)

    @patch("fabric_jobs.__main__.FabricClient.from_https")
    def test_conflict_keeps_revision_and_cursor_arguments(self, connect):
        connect.return_value.watch.side_effect = FacadeRefusedError("refused", "cursor", "detail", "2")
        code, stdout, stderr = self.invoke(["watch", "job", "--after", "1", "--wait-ms", "0"])
        self.assertEqual((code, stdout), (1, ""))
        self.assertEqual(json.loads(stderr), {"error": "refused", "code": "refused", "reason": "cursor", "current_revision": "2"})
        connect.return_value.watch.assert_called_once_with("job", after="1", wait_ms=0)

    def test_invalid_args_are_json_and_never_echo_argv(self):
        code, stdout, stderr = self.invoke([self.token])
        self.assertEqual((code, stdout), (2, ""))
        self.assertEqual(json.loads(stderr)["error"], "invalid_input")
        self.assertNotIn(self.token, stderr)

    @patch("fabric_jobs.__main__.FabricClient.from_https")
    def test_malformed_spec_never_submits(self, connect):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "job.json"
            path.write_text('{"goal":"g","harness":{},"source":{},"acceptance":"abc"}')
            code, stdout, stderr = self.invoke(["submit", "--spec", str(path), "--key", "ci-run"])
        self.assertEqual((code, stdout), (2, ""))
        self.assertIn("acceptance", json.loads(stderr)["message"])
        connect.return_value.submit.assert_not_called()

    def test_argument_errors_name_the_field_without_echoing_values(self):
        cases = [
            (["watch", "job", "--wait-ms", self.token], "--wait-ms"),
            (["checkpoint", "job"], "--key"),
            (["submit", "--spec", "missing", "--key", "k"], "--spec"),
            (["--endpoint", "http://localhost/v1/facade", "inspect", "job"], "endpoint"),
            (["inspect", self.token], "job"),
            (["--ca-file", "/missing/" + self.token, "inspect", "job"], "--ca-file"),
        ]
        for args, field in cases:
            with self.subTest(field=field):
                code, stdout, stderr = self.invoke(args)
                self.assertEqual((code, stdout), (2, ""))
                self.assertIn(field, json.loads(stderr)["message"])
                self.assertNotIn(self.token, stderr)
