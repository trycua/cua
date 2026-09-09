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
        self.exchange_timeouts = []
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
            self.exchange_timeouts.append(timeout)
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
            with pytest.raises(sdk.DriverError.Tool) as error:
                await driver.click(
                    sdk.ClickInput(
                        position=sdk.ClickPosition.COORDINATES(x=12.5, y=34.5),
                        target=getattr(sdk.ActionTarget, variant)(**fields),
                        delivery_mode=sdk.InputDeliveryMode.FOREGROUND,
                        session=sandbox.driver.session_name(driver),
                        button=None,
                        count=None,
                    )
                )
            assert error.value.tool == "click"
            assert error.value.error_code == "foreground_required"
            request = transport.events[-1][3]
            assert request["name"] == "click"
            assert request["arguments"] == {
                "x": 12.5,
                "y": 34.5,
                "target": expected,
                "delivery_mode": "foreground",
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


@pytest.fixture
def verification_input(sdk):
    return sdk.VerifyStateInput(
        pid=42,
        window_id=123,
        expect=[
            sdk.StatePredicate(window=sdk.WindowPredicate(exists=True, bounds=None), element=None),
            sdk.StatePredicate(
                window=None,
                element=sdk.ElementPredicate(
                    selector=sdk.ElementSelector(role="textbox", label_contains="Message"),
                    exists=True,
                    value_equals="typed-fleet-fixture",
                    enabled=True,
                    selected=False,
                ),
            ),
        ],
        session="session-1",
        timeout_ms=250,
        stable_samples=2,
        include_screenshot=False,
    )


async def test_real_canonical_verify_state_serializes_over_fleet(refusal, verification_input):
    transport = NativeTransport(result=refusal)
    sandbox = Sandbox(transport, _telemetry_enabled=False)
    await sandbox._connect()
    try:
        async with sandbox.driver.connect() as driver:
            verification_input.session = sandbox.driver.session_name(driver)
            await driver.verify_state(verification_input)
            name, method, path, request, headers = transport.events[-1]
            assert (name, method, path) == (
                "driver",
                "POST",
                "/v1/connections/connection-1/exchange",
            )
            assert headers == {"X-Cua-Driver-Generation": "generation-1"}
            assert request["name"] == "verify_state"
            assert request["arguments"] == {
                "pid": 42,
                "window_id": 123,
                "expect": [
                    {"window": {"exists": True}},
                    {
                        "element": {
                            "selector": {"role": "textbox", "label_contains": "Message"},
                            "exists": True,
                            "value_equals": "typed-fleet-fixture",
                            "enabled": True,
                            "selected": False,
                        }
                    },
                ],
                "session": "session-1",
                "timeout_ms": 250,
                "stable_samples": 2,
                "include_screenshot": False,
            }
            assert request["deadline_unix_ms"] > 0
            assert len(transport.exchange_timeouts) == 1
            assert 0 < transport.exchange_timeouts[0] <= 120
    finally:
        await sandbox.disconnect()


@pytest.mark.parametrize("status", ["satisfied", "unsatisfied", "unknown"])
async def test_real_canonical_verify_state_preserves_structured_outcome(
    sdk, verification_input, status
):
    # Synthetic service results qualify the bridge, not live AX observations.
    stable = status == "satisfied"
    reason = "untrusted_source" if status == "unknown" else None
    observed = None if status == "unknown" else '{"value":"observed fixture"}'
    structured = {
        "status": status,
        "stable": stable,
        "elapsed_ms": 250,
        "samples": 2,
        "predicates": [
            {
                "index": index,
                "status": status,
                "unknown_reason": reason,
                "observed_json": observed,
            }
            for index in range(2)
        ],
    }
    wire_result = {
        "content": [{"type": "text", "text": status}],
        "structuredContent": structured,
        "isError": False,
    }
    transport = NativeTransport(result=wire_result)
    sandbox = Sandbox(transport, _telemetry_enabled=False)
    await sandbox._connect()
    try:
        async with sandbox.driver.connect() as driver:
            result = await driver.verify_state(verification_input)
            assert isinstance(result, sdk.ToolResult)
            assert result.text == status
            assert result.is_error is False
            assert result.error_code is None
            assert result.action is None
            assert json.loads(result.structured_json) == structured
            assert json.loads(result.raw_json) == wire_result
            expected_status = getattr(sdk.VerificationStatus, status.upper())
            assert result.verification == sdk.VerifyStateOutput(
                status=expected_status,
                stable=stable,
                elapsed_ms=250,
                samples=2,
                predicates=[
                    sdk.PredicateOutcome(
                        index=index,
                        status=expected_status,
                        unknown_reason=sdk.UnknownReason.UNTRUSTED_SOURCE if reason else None,
                        observed_json=observed,
                    )
                    for index in range(2)
                ],
            )
            if status == "unknown":
                # A successful tool exchange does not make Unknown a satisfied predicate.
                assert result.verification.status != sdk.VerificationStatus.SATISFIED
                assert result.verification.stable is False
    finally:
        await sandbox.disconnect()


async def test_real_canonical_verify_state_after_disconnect_does_not_exchange(
    sdk, verification_input
):
    transport = NativeTransport()
    sandbox = Sandbox(transport, _telemetry_enabled=False)
    await sandbox._connect()
    try:
        async with sandbox.driver.connect() as driver:
            await sandbox.disconnect()
            before = list(transport.events)
            with pytest.raises(sdk.DriverError.Remote, match="closed"):
                await driver.verify_state(verification_input)
            assert transport.events == before
            assert transport.exchange_timeouts == []
    finally:
        await sandbox.disconnect()
