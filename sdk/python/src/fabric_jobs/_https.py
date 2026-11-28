"""Direct HTTPS transport for a gateway configured with one authority."""

from __future__ import annotations

import http.client
import io
import os
import socket
import ssl
import time
from urllib.parse import urlsplit

from ._wire import (
    MAX_FRAME,
    FabricError,
    JsonObject,
    ProtocolError,
    TransportError,
    _remaining,
    decode_json,
    encode_json,
)

_ROUTE = "/v1/facade"


class HttpStatusError(FabricError):
    """The gateway returned a status other than 200."""

    def __init__(self, status: int) -> None:
        super().__init__(f"Fabric HTTPS gateway returned HTTP {status}")
        self.status = status


class TlsVerificationError(TransportError):
    """The gateway certificate or hostname could not be verified."""


class _DeadlineReader(io.RawIOBase):
    """Give the standard HTTP parser a stream with one read deadline."""

    def __init__(self, connection: socket.socket, deadline: float) -> None:
        super().__init__()
        self._connection = connection
        self._deadline = deadline
        self._raw = connection.makefile("rb", buffering=0)

    def readable(self) -> bool:
        return True

    def readinto(self, buffer) -> int | None:
        _remaining(self._connection, self._deadline)
        return self._raw.readinto(buffer)

    def close(self) -> None:
        try:
            self._raw.close()
        finally:
            super().close()


class _DeadlineConnection(http.client.HTTPConnection):
    default_port = 443

    def __init__(self, host: str, port: int, context: ssl.SSLContext, deadline: float) -> None:
        super().__init__(host, port)
        self._context = context
        self._deadline = deadline

    def connect(self) -> None:
        # Resolve using the platform resolver, then share one budget across
        # every address, TLS negotiation, and the HTTP exchange.
        last_error: OSError | None = None
        for family, kind, protocol, _, address in socket.getaddrinfo(
            self.host, self.port, type=socket.SOCK_STREAM,
        ):
            with socket.socket(family, kind, protocol) as raw:
                _remaining(raw, self._deadline)
                try:
                    raw.connect(address)
                except OSError as error:
                    last_error = error
                    continue
                _remaining(raw, self._deadline)
                self.sock = self._context.wrap_socket(raw, server_hostname=self.host)
                return
        raise last_error or OSError("no address for the Fabric gateway")

    def send(self, data) -> None:
        if self.sock is not None:
            _remaining(self.sock, self._deadline)
        super().send(data)

    def response_class(self, connection: socket.socket, *, method: str) -> http.client.HTTPResponse:
        response = http.client.HTTPResponse(connection, method=method)
        response.fp.close()
        response.fp = io.BufferedReader(_DeadlineReader(connection, self._deadline))
        return response


class HttpsEndpoint:
    """An HTTPS Facade exchange that sends credentials only to its exact origin."""

    def __init__(self, endpoint: str, ca_file: str | os.PathLike[str] | None) -> None:
        if not isinstance(endpoint, str) or any(ord(char) <= 32 or ord(char) == 127 for char in endpoint):
            raise ValueError("endpoint must be an HTTPS URL without whitespace or control characters")
        try:
            url = urlsplit(endpoint)
            port = url.port if url.port is not None else 443
        except ValueError:
            raise ValueError("invalid Fabric HTTPS endpoint") from None
        if (
            url.scheme != "https"
            or url.hostname is None
            or url.username is not None
            or url.password is not None
            or not 1 <= port <= 65_535
            or url.path != _ROUTE
            or url.netloc.endswith(":")
            or any(char in endpoint for char in ("?", "#", "\\", "%"))
        ):
            raise ValueError(f"endpoint must be an HTTPS URL ending in {_ROUTE}, with no credentials, query, or fragment")

        # A default client SSL context enables peer and hostname validation.
        # Use an explicit context so ambient SSLKEYLOGFILE cannot turn on key logging.
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.set_alpn_protocols(["http/1.1"])
        if ca_file is None:
            context.load_default_certs(ssl.Purpose.SERVER_AUTH)
        else:
            context.load_verify_locations(cafile=os.fspath(ca_file))

        self._host = url.hostname
        self._port = port
        self._context = context

    def __call__(self, message: JsonObject, token: str, timeout: float) -> JsonObject:
        body = encode_json(message)
        deadline = time.monotonic() + timeout
        connection = _DeadlineConnection(self._host, self._port, self._context, deadline)
        try:
            connection.connect()
            connection.request("POST", _ROUTE, body=body, headers={
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json",
                "Accept": "application/json",
                "Accept-Encoding": "identity",
                "Connection": "close",
            })
            with connection.getresponse() as response:
                # http.client never follows redirects or ambient proxy settings.
                if response.status != 200:
                    raise HttpStatusError(response.status)
                if (
                    response.headers.get_content_type() != "application/json"
                    or response.getheader("Content-Encoding", "identity").lower() != "identity"
                ):
                    raise ProtocolError("bad_content_type", "expected an uncompressed JSON Facade response")
                length = response.length
                if length is not None and length > MAX_FRAME:
                    raise ProtocolError("bad_length", "Fabric HTTPS response exceeds 16 MiB")
                payload = response.read(MAX_FRAME + 1)
                if len(payload) > MAX_FRAME:
                    raise ProtocolError("bad_length", "Fabric HTTPS response exceeds 16 MiB")
                if length is not None and len(payload) != length:
                    raise TransportError("Fabric HTTPS response ended before its declared length")
                return decode_json(payload)
        except ssl.SSLCertVerificationError:
            raise TlsVerificationError("Fabric HTTPS gateway certificate verification failed") from None
        except TimeoutError:
            raise TransportError("Fabric HTTPS exchange timed out") from None
        except (OSError, http.client.HTTPException):
            raise TransportError("Fabric HTTPS exchange failed") from None
        finally:
            connection.close()
