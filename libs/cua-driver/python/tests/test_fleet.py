"""Thin Fleet adapter contracts without requiring a native library."""

import asyncio
import importlib.util
import json
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest


@pytest.fixture
def adapter(monkeypatch):
    channels = []

    def create(transport, principal):
        channel = SimpleNamespace(transport=transport, principal=principal)
        channels.append(channel)
        return channel

    sdk = SimpleNamespace(
        DriverServiceHeader=SimpleNamespace,
        DriverServiceResponse=SimpleNamespace,
        DriverServiceTransport=object,
        DriverServiceTransportError=SimpleNamespace(Failed=RuntimeError),
        open_mcp_driver_channel=create,
    )
    monkeypatch.setitem(sys.modules, "cua_driver", sdk)
    path = Path(__file__).parents[1] / "src/cua_driver/fleet.py"
    spec = importlib.util.spec_from_file_location("cua_driver.fleet", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    class Builder:
        def __init__(self):
            self.values = {}

        def __getattr__(self, name):
            def field(value):
                self.values[name] = value
                return self

            return field

        def build(self):
            return SimpleNamespace(**self.values)

    monkeypatch.setitem(
        sys.modules,
        "fleet_sdk",
        SimpleNamespace(HttpHeader=SimpleNamespace, HttpRequestBuilder=Builder),
    )
    return module, channels


def test_byte_mapping_and_caller_ownership(adapter):
    async def check():
        module, channels = adapter
        seen = []

        async def request(*args):
            seen.append(args)
            return SimpleNamespace(
                status=207,
                body=b"\x00\xff",
                headers=[
                    SimpleNamespace(name="x-test", value="a"),
                    SimpleNamespace(name="x-test", value="b"),
                ],
            )

        client = SimpleNamespace(service_request=request)
        sandbox = SimpleNamespace(services=["mcp"])
        channel = module.open_fleet_mcp_driver_channel(client, sandbox)
        result = await channel.transport.send(
            SimpleNamespace(
                method="POST",
                path="/mcp",
                body=b"\xff\x00",
                timeout_ms=1234,
                headers=[SimpleNamespace(name="x-test", value="c")],
            )
        )
        assert seen[0][:3] == (sandbox, "mcp", "/mcp")
        sent = seen[0][3]
        assert sent.body == b"\xff\x00" and sent.timeout_secs == 2
        assert sent.url == "https://service.invalid/mcp"
        assert sent.headers[0].value == "c"
        assert result.body == b"\x00\xff" and result.status == 207
        assert [h.value for h in result.headers] == ["a", "b"]
        other = module.open_fleet_mcp_driver_channel(client, sandbox)
        assert channel.principal != other.principal
        assert channels == [channel, other]

    asyncio.run(check())


def test_target_validation_precedes_open(adapter):
    module, channels = adapter
    with pytest.raises(ValueError, match="does not expose"):
        module.open_fleet_mcp_driver_channel(object(), SimpleNamespace(services=[]))
    with pytest.raises(TypeError, match="service_request"):
        module.open_fleet_mcp_driver_channel(object(), SimpleNamespace(services=["mcp"]))
    assert not channels


def test_transport_errors_are_sanitized_and_cancellation_propagates(adapter):
    async def check():
        module, _ = adapter

        failure = RuntimeError("private diagnostic")

        async def fail(*args):
            raise failure

        client = SimpleNamespace(service_request=fail)
        channel = module.open_fleet_mcp_driver_channel(client, SimpleNamespace(services=["mcp"]))
        request = SimpleNamespace(method="POST", path="/mcp", body=b"", timeout_ms=1, headers=[])
        with pytest.raises(RuntimeError, match="completion is unknown") as error:
            await channel.transport.send(request)
        assert "private" not in str(error.value)

        failure = asyncio.CancelledError()
        with pytest.raises(asyncio.CancelledError):
            await channel.transport.send(request)

    asyncio.run(check())


def test_real_generated_channel_uses_fleet_records_and_frozen_target():
    import cua_driver as sdk
    from cua_driver.fleet import open_fleet_mcp_driver_channel
    from fleet_sdk import HttpHeader, HttpResponse, Sandbox

    async def check():
        events = []
        target = Sandbox(namespace="fixture", claim="claim", name="guest", services=["mcp"])

        async def service_request(bound, service, path, request):
            assert bound.name == "guest" and bound.services == ["mcp"]
            assert service == "mcp" and path == "/mcp"
            rpc = json.loads(request.body) if request.body else {}
            method = rpc.get("method")
            events.append(method or request.method)
            headers = [HttpHeader(name="content-type", value="application/json")]
            if method == "initialize":
                headers.append(HttpHeader(name="Mcp-Session-Id", value="fleet-session"))
                result = {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"experimental": {"ai.cua.driver.envelopes": {"version": 1}}},
                }
            elif method == "notifications/initialized" or request.method == "DELETE":
                return HttpResponse(status=202, headers=[], body=b"")
            elif method == "cua/driver/v1/open":
                result = {
                    "connection_id": "receiver",
                    "generation": "generation",
                    "public_session": "host-session",
                    "capabilities": {
                        "minimum_envelope_version": 1,
                        "maximum_envelope_version": 1,
                        "supports_cancellation": True,
                    },
                }
            elif method == "cua/driver/v1/exchange":
                envelope = rpc["params"]["envelope"]
                result = {
                    "envelope_version": 1,
                    "request_id": envelope["request_id"],
                    "ok": True,
                    "completion_known": True,
                    "result": {
                        "content": [{"type": "text", "text": "fleet-native"}],
                        "isError": False,
                    },
                }
            else:
                assert method == "cua/driver/v1/close"
                result = {"ok": True}
            return HttpResponse(
                status=200,
                headers=headers,
                body=json.dumps({"jsonrpc": "2.0", "id": rpc["id"], "result": result}).encode(),
            )

        client = SimpleNamespace(service_request=service_request)
        channel = open_fleet_mcp_driver_channel(client, target)
        target.name = "different-guest"
        target.services.clear()
        client.service_request = None
        try:
            await channel.open()
            driver = channel.driver()
            assert type(driver) is sdk.CuaDriver
            assert channel.public_session() == "host-session"
            result = await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert result.text == "fleet-native"
        finally:
            await channel.close()
        assert events == [
            "initialize",
            "notifications/initialized",
            "cua/driver/v1/open",
            "cua/driver/v1/exchange",
            "cua/driver/v1/close",
            "DELETE",
        ]

    asyncio.run(check())
