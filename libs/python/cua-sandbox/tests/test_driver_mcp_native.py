"""Real generated Driver binding over a synthetic MCP/Fleet carrier."""

import asyncio

import pytest

from .test_driver_mcp import McpTransport, sandbox
from .test_typed_driver_native import sdk as _sdk_fixture

sdk = _sdk_fixture


@pytest.mark.parametrize("sse", [False, True])
async def test_actual_generated_driver_and_typed_records_over_mcp(sdk, sse):
    transport = McpTransport(sse=sse)
    transport.response_data["result"] = {
        "content": [{"type": "text", "text": "typed-mcp-fixture"}],
        "isError": False,
    }
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            assert type(driver) is sdk.CuaDriver
            result = await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert result.text == "typed-mcp-fixture"
            request = transport.events[-1][3]
            assert request["method"] == "cua/driver/v1/exchange"
            assert request["params"]["envelope"]["name"] == "get_screen_size"
            assert request["params"]["envelope"]["arguments"] == {}
            session = sb.driver.session_name(driver)
            await driver.get_agent_cursor_state(sdk.GetAgentCursorStateInput(session=session))
            assert transport.events[-1][3]["params"]["envelope"]["arguments"] == {
                "session": session
            }
            before = len(transport.events)
            with pytest.raises(TypeError):
                await driver.get_screen_size(sdk.GetScreenSizeInput(session=123))
            assert len(transport.events) == before
        assert not transport.sessions
        before = len(transport.events)
        with pytest.raises(sdk.DriverError.Remote, match="closed"):
            await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
        assert len(transport.events) == before
    finally:
        await sb.disconnect()


async def test_actual_driver_refuses_noncanonical_metadata(sdk):
    transport = McpTransport()
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            # Metadata still uses the canonical envelope instead of disguising
            # an ordinary MCP tools/list result as generated Driver metadata.
            transport.response_data["result"] = {"not": "driver metadata"}
            with pytest.raises(sdk.DriverError.Protocol, match="invalid metadata"):
                await driver.metadata()
    finally:
        await sb.disconnect()


async def test_actual_driver_does_not_gain_trusted_session_binding(sdk):
    transport = McpTransport()
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            options = sdk.TrustedSessionOptions(
                public_session="another-session",
                mode=sdk.SessionPermissionMode.STANDARD,
                ttl_seconds=60,
                idle_ttl_seconds=30,
                capability_manifest_path=None,
                bounded_manifest_path=None,
            )
            before = len(transport.events)
            with pytest.raises(sdk.DriverError.Remote, match="rebinding is unsupported"):
                await sdk.create_remote_trusted_session(driver, options)
            assert len(transport.events) == before
    finally:
        await sb.disconnect()


async def test_actual_native_future_cancellation_reaches_mcp_receiver(sdk):
    transport = McpTransport()
    transport.exchange_wait = asyncio.Event()
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            task = asyncio.create_task(driver.get_screen_size(sdk.GetScreenSizeInput(session=None)))
            await asyncio.wait_for(transport.exchange_started.wait(), 2)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task

            async def cancellation_received():
                while True:
                    methods = [event[3] for event in transport.events if event[3]]
                    cancel = next(
                        (m for m in methods if m["method"] == "cua/driver/v1/cancel"), None
                    )
                    if cancel:
                        exchange = next(
                            m for m in methods if m["method"] == "cua/driver/v1/exchange"
                        )
                        assert (
                            cancel["params"]["request_id"]
                            == exchange["params"]["envelope"]["request_id"]
                        )
                        return
                    await asyncio.sleep(0.01)

            await asyncio.wait_for(cancellation_received(), 2)
        assert not transport.sessions
        assert not transport.exchange_aborted
        assert transport.exchange_finished.is_set()
    finally:
        transport.exchange_wait.set()
        await sb.disconnect()


@pytest.mark.parametrize("finish", ["cancel", "close"])
async def test_shared_native_teardown_drains_pending_response_before_session_close(sdk, finish):
    transport = McpTransport()
    transport.exchange_wait = asyncio.Event()
    transport.cancel_releases_exchange = False
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            task = asyncio.create_task(driver.get_screen_size(sdk.GetScreenSizeInput(session=None)))
            await asyncio.wait_for(transport.exchange_started.wait(), 2)
            if finish == "cancel":
                task.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await task
                closing = asyncio.create_task(sb.driver.close())
            else:
                closing = asyncio.create_task(sb.driver.close())
            await asyncio.wait_for(transport.cancel_received.wait(), 2)
            assert not transport.exchange_aborted
            assert transport.sessions
            assert not any(
                (event[3] or {}).get("method") == "cua/driver/v1/close"
                for event in transport.events
            )
            transport.exchange_wait.set()
            await asyncio.wait_for(closing, 5)
            if finish == "close":
                with pytest.raises(sdk.DriverError.ActionInterrupted) as error:
                    await task
                assert error.value.completion == sdk.ActionCompletion.UNKNOWN
            assert transport.exchange_finished.is_set()
            assert not transport.exchange_aborted
            assert not transport.sessions
    finally:
        transport.exchange_wait.set()
        await sb.disconnect()


async def test_shared_native_sandbox_rejects_use_from_another_event_loop(sdk):
    transport = McpTransport()
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:

            def other_loop():
                async def get_session():
                    return sb.driver.session_name(driver)

                return asyncio.run(get_session())

            from cua_sandbox.interfaces.driver import DriverConnectionError

            with pytest.raises(DriverConnectionError, match="different event loop"):
                await asyncio.to_thread(other_loop)
    finally:
        await sb.disconnect()


async def test_shared_native_drain_timeout_is_bounded_and_does_not_abort(sdk, caplog):
    transport = McpTransport()
    transport.exchange_wait = asyncio.Event()
    transport.cancel_releases_exchange = False
    sb = await sandbox(transport)
    try:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            task = asyncio.create_task(driver.get_screen_size(sdk.GetScreenSizeInput(session=None)))
            await asyncio.wait_for(transport.exchange_started.wait(), 2)
            await asyncio.wait_for(sb.driver.close(), 5)
            assert not transport.exchange_aborted
            assert transport.sessions
            assert "cleanup is unconfirmed" in caplog.text
            assert not any(
                (event[3] or {}).get("method") == "cua/driver/v1/close"
                for event in transport.events
            )
            transport.exchange_wait.set()
            with pytest.raises(sdk.DriverError.ActionInterrupted) as error:
                await task
            assert error.value.completion == sdk.ActionCompletion.UNKNOWN
    finally:
        transport.exchange_wait.set()
        await sb.disconnect()
