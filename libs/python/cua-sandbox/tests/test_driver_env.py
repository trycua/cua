"""``sb.driver`` over cua-spacesd: the typed Driver rides the spacesd's
``/mcp`` (typed-envelope MCP extension) through the cua SDK's ``SpacesdClient.http``.

Wire/ownership tests; not proof of a guest desktop or native effects.
"""

from types import SimpleNamespace

import pytest
from cua_sandbox import _sdk
from cua_sandbox.interfaces.driver import DriverConnectionError
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.env import EnvTransport
from cua_sandbox.transport.fleet import FleetTransport

from .test_driver_mcp import McpTransport
from .test_driver_mcp_shared import shared as _shared_fixture
from .test_typed_driver import native as _native_fixture
from .test_typed_driver_native import sdk as _sdk_fixture

native = _native_fixture
shared = _shared_fixture
sdk = _sdk_fixture


class EnvHttp:
    """A stand-in ``cua.SpacesdClient`` whose ``http`` is served by ``McpTransport``'s
    synthetic MCP receiver (or a canned reply)."""

    def __init__(self, mcp=None, reply=None):
        self.mcp = mcp
        self.reply = reply
        self.requests = []

    async def http(self, request):
        self.requests.append(request)
        if self.reply is not None:
            return self.reply
        response = await self.mcp.request_service(
            "mcp",
            method=request.method,
            path=request.path,
            body=bytes(request.body),
            headers=[(h.name, h.value) for h in request.headers],
            timeout=None if request.timeout_ms is None else request.timeout_ms / 1000,
            max_response_bytes=request.max_response_bytes,
        )
        return SimpleNamespace(
            status=response.status_code,
            headers=[SimpleNamespace(name=k, value=v) for k, v in response.headers.multi_items()],
            body=response.content,
        )


@pytest.fixture
def records(monkeypatch):
    """The SDK's record types, without the native library."""
    types = SimpleNamespace(SpacesdHttpRequest=SimpleNamespace, SpacesdHttpHeader=SimpleNamespace)
    monkeypatch.setattr(_sdk, "native", lambda: types)
    return types


async def env_sandbox(env):
    async def factory():
        return env

    sb = Sandbox(EnvTransport(env_factory=factory), _telemetry_enabled=False)
    await sb._connect()
    return sb


class FleetEnvTransport(FleetTransport):
    """A bound Fleet claim whose image runs cua-spacesd (service ``env``)."""

    def __init__(self, env, services=("env",)):
        super().__init__(sdk=None, bound=SimpleNamespace(services=list(services), name="sb"))
        self._env = env
        self.named = []

    async def spacesd(self):
        return self._env

    async def request_service(self, name, **request):
        self.named.append((name, request))
        raise AssertionError("the spacesd carrier must not use Fleet named services")


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


async def test_fleet_env_claim_uses_the_env_carrier_not_named_services(shared, records):
    _, channels = shared
    env = EnvHttp(reply=SimpleNamespace(status=200, headers=[], body=b"{}"))
    transport = FleetEnvTransport(env)
    sb = Sandbox(transport, _telemetry_enabled=False)
    await sb._connect()
    async with sb.driver.connect():
        request = SimpleNamespace(method="POST", path="/mcp", body=b"", headers=[], timeout_ms=5)
        await channels[0].transport.send(request)
    assert len(env.requests) == 1 and not transport.named
    await sb.disconnect()


async def test_fleet_claim_without_env_service_fails_before_loading_native(monkeypatch):
    import sys

    monkeypatch.setitem(sys.modules, "cua_driver", None)
    transport = FleetEnvTransport(EnvHttp(), services=("server",))
    sb = Sandbox(transport, _telemetry_enabled=False)
    await sb._connect()
    with pytest.raises(DriverConnectionError, match="expose"):
        async with sb.driver.connect():
            pass
    await sb.disconnect()


async def test_spacesd_carries_mcp_only(native):
    sb = await env_sandbox(EnvHttp())
    with pytest.raises(DriverConnectionError, match="MCP only"):
        async with sb.driver.connect(service="env", transport="envelope"):
            pass
    with pytest.raises(DriverConnectionError, match="Fleet transport"):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pass
    await sb.disconnect()


async def test_actual_generated_driver_over_spacesd_mcp(sdk, records):
    """The real generated binding negotiates the envelope extension and
    exchanges typed records over the env carrier."""
    mcp = McpTransport()
    await mcp.connect()
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
        await mcp.disconnect()
