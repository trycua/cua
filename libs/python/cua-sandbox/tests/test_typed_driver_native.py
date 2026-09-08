"""Optional integration with the matching generated cua-driver native library."""

import importlib
import os

import httpx
import pytest
from cua_sandbox.sandbox import Sandbox

from .test_typed_driver import Transport


async def test_real_canonical_driver_uses_fleet_callback_and_disconnects():
    required = os.environ.get("CUA_SANDBOX_REQUIRE_NATIVE_DRIVER") == "1"
    sdk = importlib.import_module("cua_driver") if required else pytest.importorskip("cua_driver")
    if not hasattr(sdk, "connect_remote_channel"):
        if required:
            pytest.fail("Required cua-driver remote channel bridge is missing")
        pytest.skip("cua-driver does not provide the remote channel bridge")

    class NativeTransport(Transport):
        async def request_service(self, name, *, method, path, json_body=None, headers=None):
            response = await super().request_service(
                name, method=method, path=path, json_body=json_body, headers=headers
            )
            if path.endswith("/exchange"):
                return httpx.Response(
                    200,
                    json=dict(
                        envelope_version=1,
                        request_id=json_body["request_id"],
                        ok=True,
                        result={
                            "content": [{"type": "text", "text": "typed-fleet-fixture"}],
                            "isError": False,
                        },
                        completion_known=True,
                    ),
                )
            return response

    transport = NativeTransport()
    sandbox = Sandbox(transport, _telemetry_enabled=False)
    await sandbox._connect()
    try:
        async with sandbox.driver.connect() as driver:
            assert isinstance(driver, sdk.CuaDriver)
            result = await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert result.text == "typed-fleet-fixture"
            request = transport.events[-1][3]
            assert request["name"] == "get_screen_size"
            assert request["arguments"] == {}
            assert request["deadline_unix_ms"] > 0
            session = sandbox.driver.session_name(driver)
            await driver.get_agent_cursor_state(sdk.GetAgentCursorStateInput(session=session))
            assert transport.events[-1][3]["arguments"] == {"session": "session-1"}
            await sandbox.disconnect()
            assert transport.events[-2][1] == "DELETE"
            assert transport.events[-1] == "disconnect"
            before = len(transport.events)
            with pytest.raises(sdk.DriverError.Remote, match="closed"):
                await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert len(transport.events) == before
    finally:
        await sandbox.disconnect()
