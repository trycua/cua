"""Optional integration with the matching generated cua-driver native library."""

import importlib
import json
import os
from pathlib import Path

import pytest

from ._driver_env_support import McpTransport
from ._driver_env_support import sandbox as mcp_sandbox
from ._driver_env_support import records as _records_fixture

records = _records_fixture


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


def native_transport(result=None):
    """A synthetic MCP receiver whose ``exchange`` answers with ``result``."""
    transport = McpTransport()
    transport.response_data["result"] = (
        result
        if result is not None
        else {"content": [{"type": "text", "text": "typed-env-fixture"}], "isError": False}
    )
    return transport


def envelope_of(transport):
    """The typed envelope of the last JSON-RPC request the receiver saw."""
    return transport.events[-1][3]["params"]["envelope"]


async def test_real_canonical_driver_uses_the_env_callback_and_disconnects(sdk, records):
    transport = native_transport()
    sb = await mcp_sandbox(transport)
    try:
        async with sb.driver.connect() as driver:
            assert isinstance(driver, sdk.CuaDriver)
            result = await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert result.text == "typed-env-fixture"
            request = envelope_of(transport)
            assert request["name"] == "get_screen_size"
            assert request["arguments"] == {}
            assert request["deadline_unix_ms"] > 0
            session = sb.driver.session_name(driver)
            await driver.get_agent_cursor_state(sdk.GetAgentCursorStateInput(session=session))
            assert envelope_of(transport)["arguments"] == {"session": "session-1"}
            await sb.disconnect()
            assert not transport.sessions
            assert transport.events[-1][1] == "DELETE"
            before = len(transport.events)
            with pytest.raises(sdk.DriverError.Remote, match="closed"):
                await driver.get_screen_size(sdk.GetScreenSizeInput(session=None))
            assert len(transport.events) == before
    finally:
        await sb.disconnect()


@pytest.mark.parametrize(
    "variant,fields,expected",
    [
        ("WINDOW", {"pid": 42, "window_id": 123}, {"kind": "window", "pid": 42, "window_id": 123}),
        ("DESKTOP", {"display_id": "primary"}, {"kind": "desktop", "display_id": "primary"}),
    ],
)
async def test_real_canonical_action_target_serializes_over_the_env_carrier(
    sdk, records, refusal, variant, fields, expected
):
    transport = native_transport(result=refusal)
    sandbox = await mcp_sandbox(transport)
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
            request = envelope_of(transport)
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


async def test_real_canonical_tool_result_preserves_structured_refusal(sdk, records, refusal):
    transport = native_transport(result=refusal)
    sandbox = await mcp_sandbox(transport)
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
                    value_equals="typed-env-fixture",
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


async def test_real_canonical_verify_state_serializes_over_the_env_carrier(
    records, refusal, verification_input
):
    transport = native_transport(result=refusal)
    sandbox = await mcp_sandbox(transport)
    try:
        async with sandbox.driver.connect() as driver:
            verification_input.session = sandbox.driver.session_name(driver)
            await driver.verify_state(verification_input)
            name, method, path, rpc, headers = transport.events[-1]
            assert (name, method, path) == ("mcp", "POST", "/mcp")
            assert rpc["method"] == "cua/driver/v1/exchange"
            request = rpc["params"]["envelope"]
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
                            "value_equals": "typed-env-fixture",
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
            # The binding bounds the exchange by the request deadline (120 s by
            # default) plus its own margin.
            assert 0 < transport.exchange_timeouts[0] <= 125
    finally:
        await sandbox.disconnect()


@pytest.mark.parametrize("status", ["satisfied", "unsatisfied", "unknown"])
async def test_real_canonical_verify_state_preserves_structured_outcome(
    sdk, records, verification_input, status
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
    transport = native_transport(result=wire_result)
    sandbox = await mcp_sandbox(transport)
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
    sdk, records, verification_input
):
    transport = native_transport()
    sandbox = await mcp_sandbox(transport)
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
