"""Optional integration with the matching generated cua-driver native library."""

import importlib
import json
import os
from pathlib import Path

import httpx
import pytest
from cua_sandbox.sandbox import Sandbox

from .test_typed_driver import Transport


@pytest.fixture
def sdk():
    required = os.environ.get("CUA_SANDBOX_REQUIRE_NATIVE_DRIVER") == "1"
    sdk = importlib.import_module("cua_driver") if required else pytest.importorskip("cua_driver")
    if not hasattr(sdk, "connect_remote_channel"):
        if required:
            pytest.fail("Required cua-driver remote channel bridge is missing")
        pytest.skip("cua-driver does not provide the remote channel bridge")
    return sdk


@pytest.fixture
def refusal():
    fixture = Path(__file__).resolve().parents[3] / "cua-driver/contract/fixtures/tool-refusal.json"
    return json.loads(fixture.read_text())["result"]


class NativeTransport(Transport):
    def __init__(self, result=None):
        super().__init__()
        self.result = (
            result
            if result is not None
            else {
                "content": [{"type": "text", "text": "typed-fleet-fixture"}],
                "isError": False,
            }
        )

    async def request_service(
        self, name, *, method, path, json_body=None, headers=None, timeout=None
    ):
        response = await super().request_service(
            name, method=method, path=path, json_body=json_body, headers=headers, timeout=timeout
        )
        if path.endswith("/exchange"):
            return httpx.Response(
                200,
                json=dict(
                    envelope_version=1,
                    request_id=json_body["request_id"],
                    ok=True,
                    result=self.result,
                    completion_known=True,
                ),
            )
        return response


async def test_real_canonical_driver_uses_fleet_callback_and_disconnects(sdk):

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


@pytest.mark.parametrize(
    "variant,fields,expected",
    [
        ("WINDOW", {"pid": 42, "window_id": 123}, {"kind": "window", "pid": 42, "window_id": 123}),
        ("DESKTOP", {"display_id": "primary"}, {"kind": "desktop", "display_id": "primary"}),
    ],
)
async def test_real_canonical_action_target_serializes_over_fleet(
    sdk, refusal, variant, fields, expected
):
    transport = NativeTransport(result=refusal)
    sandbox = Sandbox(transport, _telemetry_enabled=False)
    await sandbox._connect()
    try:
        async with sandbox.driver.connect() as driver:
            await driver.click(
                sdk.ClickInput(
                    x=12.5,
                    y=34.5,
                    target=getattr(sdk.ActionTarget, variant)(**fields),
                    scope=None,
                    session=sandbox.driver.session_name(driver),
                    button=None,
                    count=None,
                )
            )
            request = transport.events[-1][3]
            assert request["name"] == "click"
            assert request["arguments"] == {
                "x": 12.5,
                "y": 34.5,
                "target": expected,
                "session": "session-1",
            }
    finally:
        await sandbox.disconnect()


async def test_real_canonical_tool_result_preserves_structured_refusal(sdk, refusal):
    transport = NativeTransport(result=refusal)
    sandbox = Sandbox(transport, _telemetry_enabled=False)
    await sandbox._connect()
    try:
        async with sandbox.driver.connect() as driver:
            result = await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert isinstance(result, sdk.ToolResult)
            assert result.is_error is True
            assert result.text == refusal["content"][0]["text"]
            assert result.error_code == "foreground_required"
            assert json.loads(result.structured_json) == refusal["structuredContent"]
            assert json.loads(result.raw_json) == refusal
    finally:
        await sandbox.disconnect()
