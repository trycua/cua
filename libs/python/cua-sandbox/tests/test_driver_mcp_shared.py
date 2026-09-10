"""Sandbox ownership and byte mapping; protocol safety uses real-binding tests."""

import asyncio
from types import SimpleNamespace

import httpx
import pytest
from cua_sandbox.interfaces._driver_mcp import shared_channel
from cua_sandbox.interfaces.driver import DriverConnectionError

from .test_driver_mcp import McpTransport, sandbox
from .test_typed_driver import native as _native_fixture

native = _native_fixture


@pytest.fixture
def shared(native):
    channels = []

    class Channel:
        def __init__(self, transport, principal):
            self.transport, self.principal = transport, principal
            self.opened, self.closes = False, 0
            self.canonical = native.CuaDriver(self)
            channels.append(self)

        async def open(self):
            self.opened = True

        def driver(self):
            assert self.opened
            return self.canonical

        def public_session(self):
            return "host-selected"

        async def close(self):
            self.closes += 1

    native.DriverServiceTransport = object
    native.DriverServiceHeader = SimpleNamespace
    native.DriverServiceResponse = SimpleNamespace
    native.DriverServiceTransportError = SimpleNamespace(Failed=RuntimeError)
    native.open_mcp_driver_channel = Channel
    return native, channels


async def test_shared_channel_is_canonical_and_closes_before_fleet(shared):
    sdk, channels = shared
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        assert driver is channels[0].canonical
        assert sb.driver.session_name(driver) == "host-selected"
        assert channels[0].principal == sb.driver._principal
        await sb.disconnect()
        assert channels[0].closes >= 1
        assert not transport._connected
        with pytest.raises(DriverConnectionError, match="inactive"):
            sb.driver.session_name(driver)
    assert driver.shutdowns == 0
    assert not transport.sessions


async def test_missing_shared_api_never_falls_back_to_envelope(native):
    transport = McpTransport()
    sb = await sandbox(transport)
    with pytest.raises(DriverConnectionError, match="open_mcp_driver_channel"):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pytest.fail("old SDK must not silently fall back")
    assert not transport.events
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
            ),
        )
    ]
    assert response.status == 207 and response.body == b"\x00\xff"
    assert [(h.name, h.value) for h in response.headers if h.name == "x-one"] == [
        ("x-one", "a"),
        ("x-one", "b"),
    ]
    await connection.close()


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
