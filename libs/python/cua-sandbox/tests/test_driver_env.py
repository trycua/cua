"""``sb.driver`` over cua-spacesd: the typed Driver rides the spacesd's
``/mcp`` (typed-envelope MCP extension) through the cua SDK's ``SpacesdClient.http``.

Wire/ownership tests; not proof of a guest desktop or native effects.
"""

import asyncio
import sys
from types import SimpleNamespace

import pytest
from cua_sandbox.interfaces.driver import DriverConnectionError
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.base import Transport as BaseTransport

from ._driver_env_support import EnvHttp, McpTransport, env_sandbox
from ._driver_env_support import native as _native_fixture
from ._driver_env_support import records as _records_fixture
from ._driver_env_support import shared as _shared_fixture
from .test_typed_driver_native import sdk as _sdk_fixture

native = _native_fixture
records = _records_fixture
shared = _shared_fixture
sdk = _sdk_fixture


class PlainTransport(BaseTransport):
    """A transport with no cua-spacesd (e.g. QMP/VNC)."""

    async def connect(self):
        pass

    async def disconnect(self):
        pass

    async def send(self, action, **params):
        raise NotImplementedError

    async def screenshot(self, format="png", quality=95):
        raise NotImplementedError

    async def get_screen_size(self):
        raise NotImplementedError

    async def get_environment(self):
        return "linux"


async def test_default_carrier_is_spacesd_mcp_through_the_sdk(shared, records):
    sdk_module, channels = shared
    reply = SimpleNamespace(
        status=207,
        headers=[
            SimpleNamespace(name="x-one", value="a"),
            SimpleNamespace(name="x-one", value="b"),
        ],
        body=b"\x00\xff",
    )
    env = EnvHttp(reply=reply)
    sb = await env_sandbox(env)
    async with sb.driver.connect() as driver:
        assert driver is channels[0].canonical
        request = SimpleNamespace(
            method="POST",
            path="/mcp",
            body=b"\xff\x00",
            headers=[SimpleNamespace(name="accept", value="application/json")],
            timeout_ms=1234,
        )
        response = await channels[0].transport.send(request)
    sent = env.requests[0]
    assert (sent.method, sent.path, sent.body) == ("POST", "/mcp", b"\xff\x00")
    assert [(h.name, h.value) for h in sent.headers] == [("accept", "application/json")]
    assert sent.timeout_ms == 1234
    assert sent.max_response_bytes == 16 * 1024 * 1024
    assert response.status == 207 and response.body == b"\x00\xff"
    assert [(h.name, h.value) for h in response.headers if h.name == "x-one"] == [
        ("x-one", "a"),
        ("x-one", "b"),
    ]
    assert channels[0].closes >= 1
    await sb.disconnect()


async def test_a_transport_without_spacesd_does_not_load_native(monkeypatch):
    monkeypatch.setitem(sys.modules, "cua_driver", None)
    sb = Sandbox(PlainTransport(), _telemetry_enabled=False)
    with pytest.raises(DriverConnectionError, match="cua-spacesd"):
        async with sb.driver.connect():
            pass


async def test_spacesd_carries_mcp_only(native):
    sb = await env_sandbox(EnvHttp())
    with pytest.raises(DriverConnectionError, match="MCP only"):
        async with sb.driver.connect(service="env", transport="envelope"):
            pass
    with pytest.raises(DriverConnectionError, match="MCP only"):
        async with sb.driver.connect(transport="envelope"):
            pass
    with pytest.raises(DriverConnectionError, match="Named Driver services"):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pass
    with pytest.raises(DriverConnectionError, match="Named Driver services"):
        async with sb.driver.connect(service="driver"):
            pass
    with pytest.raises(DriverConnectionError, match="transport must"):
        async with sb.driver.connect(transport="guess"):
            pass
    await sb.disconnect()


async def test_actual_generated_driver_over_spacesd_mcp(sdk, records):
    """The real generated binding negotiates the envelope extension and
    exchanges typed records over the env carrier."""
    mcp = McpTransport()
    mcp.response_data["result"] = {
        "content": [{"type": "text", "text": "typed-env-fixture"}],
        "isError": False,
    }
    env = EnvHttp(mcp=mcp)
    sb = await env_sandbox(env)
    try:
        async with sb.driver.connect() as driver:
            assert type(driver) is sdk.CuaDriver
            result = await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert result.text == "typed-env-fixture"
            request = mcp.events[-1][3]
            assert request["method"] == "cua/driver/v1/exchange"
            assert request["params"]["envelope"]["name"] == "get_screen_size"
        assert not mcp.sessions
        assert env.requests and all(r.path == "/mcp" for r in env.requests)
    finally:
        await sb.disconnect()


async def test_accessor_rejects_cleanup_from_another_event_loop(shared, records):
    sb = await env_sandbox(EnvHttp())
    async with sb.driver.connect():

        def wrong_loop():
            with pytest.raises(DriverConnectionError, match="different event loop"):
                asyncio.run(sb.driver.close())

        await asyncio.to_thread(wrong_loop)
    await sb.disconnect()


async def test_session_name_only_for_active_owned_driver(shared, records):
    sb = await env_sandbox(EnvHttp())
    async with sb.driver.connect() as driver:
        assert sb.driver.session_name(driver) == "host-selected"
        with pytest.raises(DriverConnectionError, match="another sandbox"):
            sb.driver.session_name(object())
    with pytest.raises(DriverConnectionError, match="inactive"):
        sb.driver.session_name(driver)
    await sb.disconnect()


async def test_context_cancellation_closes_the_channel(shared, records):
    _, channels = shared
    sb = await env_sandbox(EnvHttp())
    opened = asyncio.Event()

    async def run():
        async with sb.driver.connect():
            opened.set()
            await asyncio.Event().wait()

    task = asyncio.create_task(run())
    await opened.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert channels[0].closes >= 1
    assert not sb.driver._connections
    await sb.disconnect()
