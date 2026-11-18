"""MCP session/wire tests over the spacesd carrier; not proof of a guest desktop or native effects."""

import asyncio
import json

import httpx
import pytest
from cua_sandbox.interfaces.driver import DriverConnectionError

from ._driver_env_support import McpTransport
from ._driver_env_support import records as _records_fixture
from ._driver_env_support import sandbox
from .test_typed_driver_native import sdk as _sdk_fixture

records = _records_fixture
sdk = _sdk_fixture


@pytest.mark.parametrize("sse", [False, True])
async def test_default_carrier_preserves_canonical_driver_and_sandbox_interfaces(sdk, records, sse):
    transport = McpTransport(sse=sse)
    sb = await sandbox(transport)
    sandbox_transport = sb._transport
    try:
        async with sb.driver.connect() as driver:
            assert type(driver) is sdk.CuaDriver
            assert all(
                interface._t is sandbox_transport for interface in (sb.shell, sb.files, sb.mouse)
            )
            assert sb.driver.session_name(driver) == "session-1"
        assert not transport.sessions
        assert transport.events[-2][3]["method"] == "cua/driver/v1/close"
        assert transport.events[-1][1] == "DELETE"
        assert sandbox_transport._connected
    finally:
        await sb.disconnect()


async def test_old_tools_only_endpoint_fails_before_open_and_cleans_http_session(sdk, records):
    transport = McpTransport(supported=False)
    sb = await sandbox(transport)
    with pytest.raises(sdk.DriverError.Remote):
        async with sb.driver.connect():
            pytest.fail("must not yield a facade")
    assert [event[3]["method"] for event in transport.events if event[3]] == ["initialize"]
    assert not transport.sessions
    await sb.disconnect()


async def test_two_channels_keep_distinct_mcp_sessions_and_close_independently(sdk, records):
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect() as first:
        async with sb.driver.connect() as second:
            assert first is not second
            assert len(transport.sessions) == 2
        assert len(transport.sessions) == 1
    assert not transport.sessions
    await sb.disconnect()


async def test_unknown_transport_rejected_before_any_service_request(sdk, records):
    transport = McpTransport()
    sb = await sandbox(transport)
    with pytest.raises(DriverConnectionError, match="transport must"):
        async with sb.driver.connect(service="mcp", transport="guess"):
            pass
    assert not transport.events
    await sb.disconnect()


async def test_lost_exchange_never_retries_or_reinitializes(sdk, records, caplog):
    transport = McpTransport()
    sb = await sandbox(transport)
    async with sb.driver.connect() as driver:
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
async def test_malformed_or_failed_response_closes_without_replay(sdk, records, mutation):
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

    async with sb.driver.connect() as driver:
        transport.mutate = mutate
        with pytest.raises(sdk.DriverError.ActionInterrupted) as error:
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert error.value.completion == sdk.ActionCompletion.UNKNOWN
        assert "private" not in str(error.value)
    assert not transport.sessions
    await sb.disconnect()


async def test_malformed_initialize_still_deletes_allocated_session(sdk, records):
    transport = McpTransport()
    transport.mutate = lambda method, response: dict(response, id="wrong")
    sb = await sandbox(transport)
    with pytest.raises(sdk.DriverError.Remote):
        async with sb.driver.connect():
            pass
    assert not transport.sessions
    assert transport.events[-1][1] == "DELETE"
    await sb.disconnect()


async def test_disconnect_during_initialization_cleans_late_session(sdk, records, monkeypatch):
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
        async with sb.driver.connect():
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
    sdk, records, monkeypatch, body, media
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
    async with sb.driver.connect() as driver:
        with pytest.raises(sdk.DriverError.ActionInterrupted) as error:
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert error.value.completion == sdk.ActionCompletion.UNKNOWN
    assert not transport.sessions
    await sb.disconnect()
