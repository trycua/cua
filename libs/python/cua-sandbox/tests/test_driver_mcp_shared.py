"""Sandbox ownership and byte mapping; protocol safety uses real-binding tests."""

import asyncio
from types import SimpleNamespace

import httpx
import pytest
from cua_sandbox.interfaces._driver_mcp import shared_channel
from cua_sandbox.interfaces.driver import DriverConnectionError

from ._driver_env_support import EnvHttp, env_sandbox
from ._driver_env_support import native as _native_fixture
from ._driver_env_support import records as _records_fixture
from ._driver_env_support import shared as _shared_fixture

native = _native_fixture
records = _records_fixture
shared = _shared_fixture


async def test_shared_channel_is_canonical_and_closes_before_the_transport(shared, records):
    sdk, channels = shared
    env = EnvHttp()
    sb = await env_sandbox(env)
    async with sb.driver.connect() as driver:
        assert driver is channels[0].canonical
        assert sb.driver.session_name(driver) == "host-selected"
        assert channels[0].principal == sb.driver._principal
        await sb.disconnect()
        assert channels[0].closes >= 1
        assert not sb._transport._connected
        with pytest.raises(DriverConnectionError, match="inactive"):
            sb.driver.session_name(driver)
    assert driver.shutdowns == 0
    assert not env.requests


async def test_missing_shared_api_never_falls_back_to_envelope(native, records):
    env = EnvHttp()
    sb = await env_sandbox(env)
    with pytest.raises(DriverConnectionError, match="open_mcp_driver_channel"):
        async with sb.driver.connect():
            pytest.fail("old SDK must not silently fall back")
    assert not env.requests
    await sb.disconnect()


async def test_callback_preserves_bytes_headers_and_timeout(shared):
    sdk, channels = shared
    seen = []

    async def request_service(service, **request):
        seen.append((service, request))
        return httpx.Response(207, content=b"\x00\xff", headers=[("x-one", "a"), ("x-one", "b")])

    transport = SimpleNamespace(_connected=True, request_service=request_service)
    connection = shared_channel(sdk, transport, "mcp", "principal")
    request = SimpleNamespace(
        method="POST",
        path="/mcp",
        body=b"\xff\x00",
        headers=[SimpleNamespace(name="x-test", value="v")],
        timeout_ms=1234,
    )
    response = await channels[0].transport.send(request)
    assert seen == [
        (
            "mcp",
            dict(
                method="POST",
                path="/mcp",
                body=b"\xff\x00",
                headers=[("x-test", "v")],
                timeout=1.234,
                max_response_bytes=16 * 1024 * 1024,
            ),
        )
    ]
    assert response.status == 207 and response.body == b"\x00\xff"
    assert [(h.name, h.value) for h in response.headers if h.name == "x-one"] == [
        ("x-one", "a"),
        ("x-one", "b"),
    ]
    await connection.close()


async def test_callback_accepts_response_exactly_at_limit(shared):
    sdk, channels = shared
    body = bytes(16 * 1024 * 1024)

    async def request_service(service, **request):
        assert service == "mcp"
        assert request["max_response_bytes"] == len(body)
        return httpx.Response(200, content=body)

    transport = SimpleNamespace(_connected=True, request_service=request_service)
    shared_channel(sdk, transport, "mcp", "principal")
    request = SimpleNamespace(method="POST", path="/mcp", body=b"", headers=[], timeout_ms=1)

    response = await channels[0].transport.send(request)

    assert response.body == body


async def test_callback_sanitizes_oversized_response(shared):
    sdk, channels = shared
    secret = b"private-response-fragment"

    async def request_service(service, **request):
        assert service == "mcp"
        limit = request["max_response_bytes"]
        return httpx.Response(200, content=secret + bytes(limit + 1 - len(secret)))

    transport = SimpleNamespace(_connected=True, request_service=request_service)
    shared_channel(sdk, transport, "mcp", "principal")
    request = SimpleNamespace(method="POST", path="/mcp", body=b"", headers=[], timeout_ms=1)

    with pytest.raises(RuntimeError, match="completion is unknown") as error:
        await channels[0].transport.send(request)

    assert secret.decode() not in str(error.value)


async def test_callback_sanitizes_failure_and_preserves_cancellation(shared):
    sdk, channels = shared

    async def fail(*args, **kwargs):
        raise RuntimeError("private diagnostic")

    transport = SimpleNamespace(_connected=True, request_service=fail)
    shared_channel(sdk, transport, "mcp", "principal")
    request = SimpleNamespace(method="POST", path="/mcp", body=b"", headers=[], timeout_ms=1)
    with pytest.raises(RuntimeError, match="completion is unknown") as error:
        await channels[0].transport.send(request)
    assert "private" not in str(error.value)

    async def cancel(*args, **kwargs):
        raise asyncio.CancelledError

    transport.request_service = cancel
    with pytest.raises(asyncio.CancelledError):
        await channels[0].transport.send(request)
