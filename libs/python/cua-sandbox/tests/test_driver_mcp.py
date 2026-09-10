"""MCP session/wire tests; not proof of a guest desktop or native effects."""

import asyncio
import json

import httpx
import pytest
from cua_sandbox.interfaces.driver import DriverConnectionError
from cua_sandbox.sandbox import Sandbox

from .test_typed_driver import Transport
from .test_typed_driver_native import sdk as _sdk_fixture

sdk = _sdk_fixture


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
        self.cancel_releases_exchange = True
        self.exchange_aborted = False
        self.exchange_finished = asyncio.Event()
        self.cancel_received = asyncio.Event()

    async def request_service(
        self, name, *, method, path, body=None, json_body=None, headers=None, timeout=None
    ):
        if body:
            json_body = json.loads(body)
        headers = httpx.Headers(headers)
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
                        try:
                            await self.exchange_wait.wait()
                        except asyncio.CancelledError:
                            self.exchange_aborted = True
                            raise
                    if self.fail_exchange:
                        raise RuntimeError("sensitive transport diagnostic")
                    request = json_body["params"]["envelope"]
                    result = dict(self.response_data, request_id=request["request_id"])
                    self.exchange_finished.set()
                else:
                    assert rpc in ("cua/driver/v1/cancel", "cua/driver/v1/close")
                    if rpc == "cua/driver/v1/cancel":
                        self.cancel_received.set()
                        if self.cancel_releases_exchange and self.exchange_wait is not None:
                            self.exchange_wait.set()
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
async def test_explicit_mcp_preserves_canonical_driver_and_computer_server(sdk, sse):
    transport = McpTransport(sse=sse)
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            assert type(driver) is sdk.CuaDriver
            assert all(interface._t is transport for interface in (sb.shell, sb.files, sb.mouse))
            assert transport._service_name == "server"
            assert sb.driver.session_name(driver) == "session-1"
        assert not transport.sessions
        assert transport.events[-2][3]["method"] == "cua/driver/v1/close"
        assert transport.events[-1][1] == "DELETE"
        assert transport._connected
    finally:
        await sb.disconnect()


async def test_old_tools_only_endpoint_fails_before_open_and_cleans_http_session(sdk):
    transport = McpTransport(supported=False)
    sb = await sandbox(transport)
    with pytest.raises(sdk.DriverError.Remote):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pytest.fail("must not yield a facade")
    assert [event[3]["method"] for event in transport.events if event[3]] == ["initialize"]
    assert not transport.sessions
    await sb.disconnect()


async def test_two_channels_keep_distinct_mcp_sessions_and_close_independently(sdk):
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as first:
        async with sb.driver.connect(service="mcp", transport="mcp") as second:
            assert first is not second
            assert len(transport.sessions) == 2
        assert len(transport.sessions) == 1
    assert not transport.sessions
    await sb.disconnect()


async def test_unknown_transport_rejected_before_any_service_request(sdk):
    transport = McpTransport()
    sb = await sandbox(transport)
    with pytest.raises(DriverConnectionError, match="transport must"):
        async with sb.driver.connect(service="mcp", transport="guess"):
            pass
    assert not transport.events
    await sb.disconnect()


async def test_lost_exchange_never_retries_or_reinitializes(sdk, caplog):
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        transport.fail_exchange = True
        with pytest.raises(sdk.DriverError.ActionInterrupted) as interrupted:
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert interrupted.value.completion == sdk.ActionCompletion.UNKNOWN
        with pytest.raises(sdk.DriverError.Remote, match="closed"):
            sb.driver.session_name(driver)
        before = len(transport.events)
        with pytest.raises(sdk.DriverError.Remote):
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert not any(
            (event[3] or {}).get("method") == "cua/driver/v1/exchange"
            for event in transport.events[before:]
        )
    methods = [event[3]["method"] for event in transport.events if event[3]]
    assert methods.count("initialize") == 1
    assert methods.count("cua/driver/v1/exchange") == 1
    assert not transport.sessions
    assert "sensitive" not in caplog.text
    await sb.disconnect()


@pytest.mark.parametrize("mutation", ["wrong-id", "bad-version", "rpc-error", "wrong-inner-id"])
async def test_malformed_or_failed_response_closes_without_replay(sdk, mutation):
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
        with pytest.raises(sdk.DriverError.ActionInterrupted) as error:
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert error.value.completion == sdk.ActionCompletion.UNKNOWN
        assert "private" not in str(error.value)
    assert not transport.sessions
    await sb.disconnect()


async def test_malformed_initialize_still_deletes_allocated_session(sdk):
    transport = McpTransport()
    transport.mutate = lambda method, response: dict(response, id="wrong")
    sb = await sandbox(transport)
    with pytest.raises(sdk.DriverError.Remote):
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pass
    assert not transport.sessions
    assert transport.events[-1][1] == "DELETE"
    await sb.disconnect()


async def test_disconnect_during_initialization_cleans_late_session(sdk, monkeypatch):
    transport = McpTransport()
    entered, released = asyncio.Event(), asyncio.Event()
    original = transport.request_service

    async def delayed(*args, **kwargs):
        if json.loads(kwargs.get("body") or b"{}").get("method") == "initialize":
            entered.set()
            await released.wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(transport, "request_service", delayed)
    sb = await sandbox(transport)

    async def connect():
        async with sb.driver.connect(service="mcp", transport="mcp"):
            pytest.fail("disconnected accessor must not yield")

    opening = asyncio.create_task(connect())
    await asyncio.wait_for(entered.wait(), 2)
    closing = asyncio.create_task(sb.driver.close())
    await asyncio.sleep(0)
    released.set()
    await closing
    with pytest.raises((sdk.DriverError.Remote, DriverConnectionError)):
        await opening
    assert not transport.sessions
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
async def test_shared_decoder_refuses_ambiguous_incomplete_or_non_json_responses(
    sdk, monkeypatch, body, media
):
    transport = McpTransport()
    original = transport.request_service

    async def malformed(*args, **kwargs):
        request = json.loads(kwargs.get("body") or b"{}")
        response = await original(*args, **kwargs)
        if request.get("method") == "cua/driver/v1/exchange":
            return httpx.Response(
                200,
                content=body.replace("expected", request["id"]),
                headers={"Content-Type": media},
            )
        return response

    monkeypatch.setattr(transport, "request_service", malformed)
    sb = await sandbox(transport)
    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        with pytest.raises(sdk.DriverError.ActionInterrupted) as error:
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert error.value.completion == sdk.ActionCompletion.UNKNOWN
    assert not transport.sessions
    await sb.disconnect()
