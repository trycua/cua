"""Regression tests for PTY stdin encoding on the computer-server transports.

computer-server's ``POST /pty/{pid}/stdin`` route base64-decodes the ``data``
field (``base64.b64decode(body.get("data", ""))``) before writing to the PTY.
``Terminal.send_input(pid, "echo hello\\n")`` hands the text straight to
``Transport.pty_send``, so the transport is responsible for producing the
base64 the server expects.  Sending the raw text either fails to decode
(HTTP 500) or writes garbage bytes to the shell.

The fake servers below decode ``data`` exactly the way computer-server does
and record what would reach the PTY.
"""

from __future__ import annotations

import base64
import binascii
import json

import httpx
import pytest
from cua_sandbox.transport.fleet import FleetTransport
from cua_sandbox.transport.http import HTTPTransport
from fleet_sdk import HttpResponse, Sandbox

pytestmark = pytest.mark.asyncio

_INPUT = "echo hello\n"


def _decode_like_server(body: bytes) -> bytes | None:
    """Mirror computer_server.main.pty_stdin: base64-decode ``data``."""
    payload = json.loads(body)
    try:
        return base64.b64decode(payload.get("data", ""))
    except (binascii.Error, ValueError):
        return None


async def test_http_pty_send_base64_encodes_stdin():
    received: list[bytes] = []

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/pty/7/stdin"
        decoded = _decode_like_server(request.content)
        if decoded is None:
            return httpx.Response(500, text="Internal Server Error")
        received.append(decoded)
        return httpx.Response(200, json={"ok": True})

    transport = HTTPTransport("http://server:8000", timeout=5.0)
    await transport.connect()
    assert transport._client is not None
    await transport._client.aclose()
    transport._client = httpx.AsyncClient(
        base_url="http://server:8000",
        transport=httpx.MockTransport(handler),
        timeout=5.0,
    )

    await transport.pty_send(7, _INPUT)

    assert received == [_INPUT.encode("utf-8")]


class _FakeSDK:
    def __init__(self) -> None:
        self.bodies: list[bytes] = []

    async def service_request(self, sandbox, service, path, request):
        assert path == "/pty/7/stdin"
        self.bodies.append(request.body)
        if _decode_like_server(request.body) is None:
            return HttpResponse(status=500, headers=[], body=b"Internal Server Error")
        return HttpResponse(status=200, headers=[], body=b'{"ok": true}')


async def test_fleet_pty_send_base64_encodes_stdin():
    sdk = _FakeSDK()
    transport = FleetTransport(
        sdk=sdk,
        bound=Sandbox(namespace="demo", claim="claim-demo", name="sandbox-demo", services=["api"]),
    )
    await transport.connect()

    await transport.pty_send(7, _INPUT)

    assert [_decode_like_server(body) for body in sdk.bodies] == [_INPUT.encode("utf-8")]
