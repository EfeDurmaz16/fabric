"""HTTPS request shape, transport limits, and typed response handling."""

import http.client
import json
import socket
import ssl
import time
import unittest
from email.message import Message
from unittest.mock import MagicMock, patch

from fabric_jobs import (
    FabricClient, FacadeRefusedError, HttpStatusError, ProtocolError, TlsVerificationError, TransportError,
)
from fabric_jobs._https import _DeadlineConnection

ENDPOINT = "https://fabric.example/v1/facade"
TOKEN = "1" * 64
JOB = "01K8W3A00000000000000000J1"


def response(status=200, payload=b"", length=None, content_type="application/json"):
    result = MagicMock(spec=http.client.HTTPResponse)
    result.__enter__.return_value = result
    result.status = status
    headers = Message()
    headers["Content-Type"] = content_type
    result.headers = headers
    result.getheader.side_effect = headers.get
    result.read.return_value = payload
    result.length = length
    return result


class HttpsFacadeTests(unittest.TestCase):
    def test_refuses_invalid_endpoint_before_connecting(self):
        endpoints = (
            "http://fabric.example/v1/facade",
            "https://user:pass@fabric.example/v1/facade",
            "https://fabric.example/v1/facade?",
            "https://fabric.example/v1/facade#foo",
            "https://fabric.example/v1/facade/elsewhere",
            "https://fabric.example:0/v1/facade",
            "https://fab\nric.example/v1/facade",
            "https://fabric.example%2f.evil/v1/facade",
        )
        for endpoint in endpoints:
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                FabricClient.from_https(endpoint, TOKEN)

    @patch("fabric_jobs._https._DeadlineConnection")
    def test_sends_bearer_only_as_a_header_and_reads_typed_refusal(self, factory):
        conn = factory.return_value
        payload = json.dumps({
            "type": "refused", "facade": 1, "code": "scope", "reason": "job", "message": "scope: job",
        }).encode()
        conn.getresponse.return_value = response(payload=payload)
        client = FabricClient.from_https(ENDPOINT, TOKEN)
        with self.assertRaises(FacadeRefusedError) as caught:
            client.inspect(JOB)
        self.assertEqual((caught.exception.code, caught.exception.reason), ("scope", "job"))
        self.assertEqual(conn.request.call_args.args, ("POST", "/v1/facade"))
        self.assertEqual(json.loads(conn.request.call_args.kwargs["body"]), {
            "type": "inspect", "facade": 1, "job": JOB,
        })
        self.assertEqual(conn.request.call_args.kwargs["headers"]["Authorization"], f"Bearer {TOKEN}")
        conn.close.assert_called_once()

    @patch("fabric_jobs._https._DeadlineConnection")
    def test_redirect_is_a_status_error_without_following_location(self, factory):
        conn = factory.return_value
        conn.getresponse.return_value = redirect = response(status=307)
        redirect.headers["Location"] = "https://second.example/v1/facade"
        with self.assertRaises(HttpStatusError) as caught:
            FabricClient.from_https(ENDPOINT, TOKEN).watch(JOB)
        self.assertEqual(caught.exception.status, 307)
        self.assertNotIn(TOKEN, str(caught.exception))
        factory.assert_called_once()
        conn.request.assert_called_once()
        redirect.read.assert_not_called()

    @patch("fabric_jobs._https._DeadlineConnection")
    def test_declared_and_streaming_response_bounds(self, factory):
        conn = factory.return_value
        client = FabricClient.from_https(ENDPOINT, TOKEN)
        with patch("fabric_jobs._https.MAX_FRAME", 8):
            conn.getresponse.return_value = declared = response(length=9)
            with self.assertRaises(ProtocolError) as caught:
                client.inspect(JOB)
            self.assertEqual(caught.exception.reason, "bad_length")
            declared.read.assert_not_called()
            conn.getresponse.return_value = streamed = response(payload=b"a" * 9)
            with self.assertRaises(ProtocolError) as caught:
                client.inspect(JOB)
            self.assertEqual(caught.exception.reason, "bad_length")
            streamed.read.assert_called_once_with(9)

    @patch("fabric_jobs._https._DeadlineConnection")
    def test_tls_failure_happens_before_bearer_is_sent(self, factory):
        conn = factory.return_value
        conn.connect.side_effect = ssl.SSLCertVerificationError()
        with self.assertRaises(TlsVerificationError):
            FabricClient.from_https(ENDPOINT, TOKEN).inspect(JOB)
        context = factory.call_args.args[2]
        self.assertTrue(context.check_hostname)
        self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)
        conn.request.assert_not_called()

    def test_response_headers_use_the_remaining_exchange_deadline(self):
        left, right = socket.socketpair()
        connection = _DeadlineConnection("fabric.example", 443, ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT),
                                         time.monotonic() + 0.050)
        with left, right, connection.response_class(right, method="POST") as parser:
            left.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n")
            with self.assertRaises(TimeoutError):
                parser.begin()

    def test_expired_deadline_prevents_request_write(self):
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        connection = _DeadlineConnection("fabric.example", 443, context, time.monotonic() - 1)
        connection.sock = MagicMock(spec=socket.socket)
        with self.assertRaises(TransportError):
            connection.send(b"request")
        connection.sock.sendall.assert_not_called()


if __name__ == "__main__":
    unittest.main()
