"""MCP session/wire tests; not proof of a guest desktop or native effects."""

import asyncio
import json

import httpx
import pytest
from cua_sandbox.interfaces._driver_mcp import McpCarrierError, _response
from cua_sandbox.interfaces.driver import DriverConnectionError
from cua_sandbox.sandbox import Sandbox

from .test_typed_driver import ChannelError, Transport, envelope
from .test_typed_driver import native as _native_fixture

native = _native_fixture


class McpTransport(Transport):
    def __init__(self, *, sse=False, supported=True):
        super().__init__()
        self._bound.services.append("mcp")
        self.sse = sse
        self.supported = supported
        self.sessions = set()
        self.sequence = 0
        self.mutate = None
        self.fail_exchange = False
        self.exchange_started = asyncio.Event()
        self.exchange_wait = None

    async def request_service(
        self, name, *, method, path, json_body=None, headers=None, timeout=None
    ):
        self.events.append((name, method, path, json_body, headers))
        assert name == "mcp" and path == "/mcp"
        assert headers["Accept"] == "application/json, text/event-stream"
        rpc = json_body.get("method") if json_body else None
        response_headers = {}
        if rpc == "initialize":
            assert "Mcp-Session-Id" not in headers
            self.sequence += 1
            session = f"http-session-{self.sequence}"
            self.sessions.add(session)
            response_headers["Mcp-Session-Id"] = session
            result = {
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "experimental": (
                        {"ai.cua.driver.envelopes": {"version": 1}} if self.supported else {}
                    )
                },
            }
        else:
            session = headers["Mcp-Session-Id"]
            assert headers["MCP-Protocol-Version"] == "2025-06-18"
            if session not in self.sessions:
                return httpx.Response(404)
            if method == "DELETE":
                self.sessions.remove(session)
                return httpx.Response(200)
            if rpc == "notifications/initialized":
                assert "id" not in json_body
                return httpx.Response(202)
            if rpc == "cua/driver/v1/open":
                result = dict(self.open_data, connection_id=session, generation=f"gen-{session}")
            else:
                assert json_body["params"]["connection_id"] == session
                assert json_body["params"]["generation"] == f"gen-{session}"
                if rpc == "cua/driver/v1/exchange":
                    self.exchange_started.set()
                    if self.exchange_wait is not None:
                        await self.exchange_wait.wait()
                    if self.fail_exchange:
                        raise RuntimeError("sensitive transport diagnostic")
                    request = json_body["params"]["envelope"]
                    result = dict(self.response_data, request_id=request["request_id"])
                else:
                    assert rpc in ("cua/driver/v1/cancel", "cua/driver/v1/close")
                    result = {"ok": True}
        response = {"jsonrpc": "2.0", "id": json_body["id"], "result": result}
        if self.mutate:
            response = self.mutate(rpc, response)
        if self.sse:
            response_headers["Content-Type"] = "text/event-stream"
            return httpx.Response(
                200,
                content=("event: message\r\ndata: " + json.dumps(response) + "\r\n\r\n"),
                headers=response_headers,
            )
        return httpx.Response(200, json=response, headers=response_headers)


async def sandbox(transport):
    sb = Sandbox(transport, _telemetry_enabled=False)
    await sb._connect()
    return sb


@pytest.mark.parametrize("sse", [False, True])
async def test_explicit_mcp_preserves_canonical_driver_and_computer_server(native, sse):
    transport = McpTransport(sse=sse)
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            assert isinstance(driver, native.CuaDriver)
            assert all(interface._t is transport for interface in (sb.shell, sb.files, sb.mouse))
            assert transport._service_name == "server"
            channel = driver.channel
            result = await channel.exchange(envelope())
            assert json.loads(result.result_json) == {"width": 1280}
            assert channel.identity().connection_generation == "gen-http-session-1"
            assert (await channel.negotiate()).supports_cancellation is True
            assert sb.driver.session_name(driver) == "session-1"
        assert channel.cleanup_confirmed
        assert driver.shutdowns == 1
        assert not transport.sessions
        assert transport.events[-2][3]["method"] == "cua/driver/v1/close"
        assert transport.events[-1][1] == "DELETE"
        assert transport._connected
    finally:
        await sb.disconnect()


async def test_old_tools_only_endpoint_fails_before_open_and_cleans_http_session(native):
    transport = McpTransport(supported=False)
    sb = await sandbox(transport)
    with pytest.raises(ChannelError, match="does not support typed"):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pytest.fail("must not yield a facade")
    assert [event[3]["method"] for event in transport.events if event[3]] == ["initialize"]
    assert not transport.sessions
    await sb.disconnect()


async def test_two_channels_keep_distinct_mcp_sessions_and_close_independently(native):
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as first:
        async with sb.driver.connect(service="mcp", transport="mcp") as second:
            assert first.channel.generation != second.channel.generation
            assert len(transport.sessions) == 2
        assert len(transport.sessions) == 1
        assert (await first.channel.exchange(envelope())).ok
    assert not transport.sessions
    await sb.disconnect()


async def test_unknown_transport_rejected_before_any_service_request(native):
    transport = McpTransport()
    sb = await sandbox(transport)
    with pytest.raises(DriverConnectionError, match="transport must"):
        async with sb.driver.connect(service="mcp", transport="guess"):
            pass
    assert not transport.events
    await sb.disconnect()


async def test_lost_exchange_never_retries_or_reinitializes(native, caplog):
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        transport.fail_exchange = True
        with pytest.raises(ChannelError, match="completion is unknown"):
            await driver.channel.exchange(envelope())
        with pytest.raises(ChannelError, match="closed"):
            await driver.channel.exchange(envelope(request_id="second"))
        assert driver.channel.closed
        assert not driver.channel.cleanup_confirmed
    methods = [event[3]["method"] for event in transport.events if event[3]]
    assert methods.count("initialize") == 1
    assert methods.count("cua/driver/v1/exchange") == 1
    assert not transport.sessions
    assert "sensitive" not in caplog.text
    await sb.disconnect()


@pytest.mark.parametrize("mutation", ["wrong-id", "bad-version", "rpc-error", "wrong-inner-id"])
async def test_malformed_or_failed_response_closes_without_replay(native, mutation):
    transport = McpTransport()
    sb = await sandbox(transport)

    def mutate(method, response):
        if method != "cua/driver/v1/exchange":
            return response
        if mutation == "wrong-id":
            response["id"] = "other"
        elif mutation == "bad-version":
            response["jsonrpc"] = "1.0"
        elif mutation == "rpc-error":
            response.pop("result")
            response["error"] = {"code": -32409, "message": "private diagnostic"}
        else:
            response["result"]["request_id"] = "wrong-inner-id"
        return response

    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        transport.mutate = mutate
        with pytest.raises(ChannelError) as error:
            await driver.channel.exchange(envelope())
        assert "private" not in str(error.value)
    assert not transport.sessions
    await sb.disconnect()


async def test_cancel_can_overtake_exchange_and_prevents_late_result(native):
    transport = McpTransport()
    transport.exchange_wait = asyncio.Event()
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        task = asyncio.create_task(driver.channel.exchange(envelope()))
        await asyncio.wait_for(transport.exchange_started.wait(), 1)
        await asyncio.wait_for(driver.channel.cancel("request-1"), 1)
        transport.exchange_wait.set()
        with pytest.raises(ChannelError, match="after close or cancellation"):
            await task
        assert not transport.sessions
    methods = [event[3]["method"] for event in transport.events if event[3]]
    assert methods.index("cua/driver/v1/cancel") < methods.index("cua/driver/v1/close")
    await sb.disconnect()


@pytest.mark.parametrize(
    "body,media",
    [
        ('{"jsonrpc":"2.0","id":"expected","id":"expected","result":{}}', "application/json"),
        ('{"jsonrpc":"2.0","id":"expected","result":NaN}', "application/json"),
        ('data: {"jsonrpc":"2.0","id":"expected","result":{}}\n', "text/event-stream"),
        (
            'data: {"jsonrpc":"2.0","id":"expected","result":{}}\n\ndata: {}\n\n',
            "text/event-stream",
        ),
        ("{}", "text/html"),
        ('{"jsonrpc":"2.0","id":true,"result":{}}', "application/json"),
    ],
)
def test_decoder_refuses_ambiguous_incomplete_or_non_json_responses(body, media):
    with pytest.raises(McpCarrierError, match="malformed"):
        _response(httpx.Response(200, content=body, headers={"Content-Type": media}), "expected")


async def test_malformed_initialize_still_deletes_allocated_session(native):
    transport = McpTransport()
    transport.mutate = lambda method, response: dict(response, id="wrong")
    sb = await sandbox(transport)
    with pytest.raises(ChannelError, match="malformed"):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pass
    assert not transport.sessions
    assert transport.events[-1][1] == "DELETE"
    await sb.disconnect()


async def test_disconnect_during_initialization_cleans_late_session(native, monkeypatch):
    transport = McpTransport()
    entered, released = asyncio.Event(), asyncio.Event()
    original = transport.request_service

    async def delayed(*args, **kwargs):
        if (kwargs.get("json_body") or {}).get("method") == "initialize":
            entered.set()
            await released.wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(transport, "request_service", delayed)
    sb = await sandbox(transport)

    async def connect():
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pytest.fail("disconnected accessor must not yield")

    opening = asyncio.create_task(connect())
    await entered.wait()
    closing = asyncio.create_task(sb.driver.close())
    await asyncio.sleep(0)
    released.set()
    await closing
    with pytest.raises(ChannelError, match="closed during"):
        await opening
    assert not transport.sessions
    assert all((event[3] or {}).get("method") != "cua/driver/v1/open" for event in transport.events)
    await sb.disconnect()
