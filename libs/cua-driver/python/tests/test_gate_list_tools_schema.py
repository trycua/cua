"""Regression tests for the HAI-facing native inventory gate."""

from __future__ import annotations

import copy
import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))

from gate_list_tools_schema import (  # noqa: E402
    REQUIRED_TOOLS,
    WheelGateError,
    validate_inventory,
)


def valid_linux_inventory() -> dict[str, object]:
    tools = [
        {"name": name, "inputSchema": {"type": "object", "properties": {}}}
        for name in sorted(REQUIRED_TOOLS)
    ]
    by_name = {tool["name"]: tool for tool in tools}
    by_name["drag"]["inputSchema"]["properties"]["input_route"] = {"type": "string"}
    by_name["set_value"]["inputSchema"]["properties"]["delivery_mode"] = {"type": "string"}
    return {"tools": tools}


def test_required_linux_inventory_passes() -> None:
    inventory = validate_inventory(valid_linux_inventory(), "linux")

    assert {tool.name for tool in inventory.tools} >= REQUIRED_TOOLS


def test_macos_inventory_does_not_require_linux_routing_fields() -> None:
    inventory = valid_linux_inventory()
    tools = inventory["tools"]
    assert isinstance(tools, list)
    for tool_name, property_name in (
        ("drag", "input_route"),
        ("set_value", "delivery_mode"),
    ):
        tool = next(tool for tool in tools if tool["name"] == tool_name)
        del tool["inputSchema"]["properties"][property_name]

    validated = validate_inventory(inventory, "darwin")

    assert {tool.name for tool in validated.tools} >= REQUIRED_TOOLS


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("select_text", "missing required tools: select_text"),
        ("drag.input_route", "drag is missing required input properties: input_route"),
        (
            "set_value.delivery_mode",
            "set_value is missing required input properties: delivery_mode",
        ),
    ],
)
def test_missing_hai_contract_surface_is_rejected(mutation: str, message: str) -> None:
    inventory = copy.deepcopy(valid_linux_inventory())
    tools = inventory["tools"]
    assert isinstance(tools, list)
    if mutation == "select_text":
        inventory["tools"] = [tool for tool in tools if tool["name"] != "select_text"]
    else:
        tool_name, property_name = mutation.split(".", 1)
        tool = next(tool for tool in tools if tool["name"] == tool_name)
        del tool["inputSchema"]["properties"][property_name]

    with pytest.raises(WheelGateError, match=message):
        validate_inventory(inventory, "linux")


def test_unknown_platform_is_rejected() -> None:
    with pytest.raises(WheelGateError, match="unsupported inventory platform"):
        validate_inventory(valid_linux_inventory(), "win32")
