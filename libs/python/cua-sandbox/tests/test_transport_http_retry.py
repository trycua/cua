"""Unit tests for HTTPTransport._cmd retry-on-5xx behavior.

The computer-server can briefly return 5xx (Traefik dropping the pod
from its endpoint list, grpc fork hiccups). The transport retries
up to 3 times with exponential backoff for 5xx responses. 4xx
errors are raised immediately. httpx transport exceptions (read
timeout, connection reset) are not retried because the command may
have already started on the server.
"""

from __future__ import annotations

import json
from itertools import cycle

import httpx
import pytest

from cua_sandbox.transport.http import HTTPTransport

pytestmark = pytest.mark.asyncio


def _sse_body(payload: dict) -> str:
    return f"data: {json.dumps(payload)}\n\n"


async def _make_transport(handler) -> HTTPTransport:
    t = HTTPTransport("http://server:8000", timeout=5.0)
    # Patch the async client after connect so the mock transport is in use.
    await t.connect()
    assert t._client is not None
    await t._client.aclose()
    t._client = httpx.AsyncClient(
        base_url="http://server:8000",
        transport=httpx.MockTransport(handler),
        timeout=5.0,
    )
    return t


async def test_cmd_success_no_retry():
    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request.url.path)
        return httpx.Response(200, text=_sse_body({"width": 1, "height": 2}))

    t = await _make_transport(handler)
    result = await t._cmd("get_screen_size")
    assert result == {"width": 1, "height": 2}
    assert len(calls) == 1, "success should not retry"


async def test_cmd_retries_5xx_then_succeeds(monkeypatch):
    # Skip sleeps so the test runs instantly.
    import cua_sandbox.transport.http as http_mod

    async def _no_sleep(_seconds):
        pass

    monkeypatch.setattr(http_mod.asyncio, "sleep", _no_sleep)

    responses = iter([
        httpx.Response(503, text="service unavailable"),
        httpx.Response(502, text="bad gateway"),
        httpx.Response(200, text=_sse_body({"ok": True})),
    ])
    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request.url.path)
        return next(responses)

    t = await _make_transport(handler)
    result = await t._cmd("screenshot")
    assert result == {"ok": True}
    assert len(calls) == 3, "should retry twice before succeeding"


async def test_cmd_gives_up_after_max_retries(monkeypatch):
    import cua_sandbox.transport.http as http_mod

    async def _no_sleep(_seconds):
        pass

    monkeypatch.setattr(http_mod.asyncio, "sleep", _no_sleep)

    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request.url.path)
        return httpx.Response(503, text="still unavailable")

    t = await _make_transport(handler)
    with pytest.raises(httpx.HTTPStatusError) as exc_info:
        await t._cmd("screenshot")
    assert exc_info.value.response.status_code == 503
    # _CMD_MAX_RETRIES (3) attempts total, then raise.
    assert len(calls) == 3


async def test_cmd_does_not_retry_4xx():
    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request.url.path)
        return httpx.Response(401, text="unauthorized")

    t = await _make_transport(handler)
    with pytest.raises(httpx.HTTPStatusError) as exc_info:
        await t._cmd("screenshot")
    assert exc_info.value.response.status_code == 401
    assert len(calls) == 1, "4xx should not retry"


async def test_cmd_does_not_retry_read_timeout():
    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request.url.path)
        raise httpx.ReadTimeout("server too slow", request=request)

    t = await _make_transport(handler)
    with pytest.raises(httpx.ReadTimeout):
        await t._cmd("run_command", params={"command": "slow"})
    # ReadTimeout means the request reached the server — don't retry
    # or we risk double-executing a non-idempotent command.
    assert len(calls) == 1
