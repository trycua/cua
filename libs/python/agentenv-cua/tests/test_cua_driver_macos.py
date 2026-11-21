"""The cua-driver macOS driver's declarations and checks, offline: no daemon, no Mac needed."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest
from agentenv_protocol.computer import Unsupported
from agentenv_protocol.computer.platform import Platform

FOLDER = Path(__file__).parents[1] / "src" / "agentenv_cua" / "drivers" / "cua_driver_macos"
MANIFEST = {
    "instructions": "cua-driver: background GUI automation.",
    "tools": [
        {
            "name": "click",
            "description": "Click.",
            "inputSchema": {"type": "object"},
            "annotations": None,
            "outputSchema": None,
        },
        {
            "name": "check_permissions",
            "description": "TCC.",
            "inputSchema": {"type": "object"},
            "annotations": None,
            "outputSchema": None,
        },
    ],
    "resources": [
        {
            "uri": "skill://cua-driver/SKILL.md",
            "name": "SKILL.md",
            "description": "",
            "mimeType": None,
        }
    ],
}


@pytest.fixture(scope="module")
def driver(tmp_path_factory):
    spec = importlib.util.spec_from_file_location("cua_driver_macos_driver", FOLDER / "driver.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    manifest = tmp_path_factory.mktemp("pin") / "mcp.json"
    manifest.write_text(json.dumps(MANIFEST))
    module.MANIFEST = manifest
    module._manifest.cache_clear()
    return module


def test_it_declares_macos_on_apple_silicon_only(driver):
    assert driver.CuaDriverMacOS.supports == [{"os": {"type": "macos"}, "arch": ["arm64"]}]
    card = driver.CuaDriverMacOS.declaration(Platform(os="macos", arch="arm64"))
    assert card is not None


def test_modifiers_map_to_cua_drivers_macos_names(driver):
    assert driver._modifiers(["Meta", "Shift"]) == ["cmd", "shift"]
    assert driver._modifiers(["Alt", "Control"]) == ["option", "ctrl"]
    with pytest.raises(Unsupported):
        driver._modifiers(["a"])


def test_named_keys_use_macos_names(driver):
    assert driver.NAMED["Backspace"] == "delete" and driver.NAMED["Delete"] == "forwarddelete"
    assert driver.NAMED["Enter"] == "return"


def test_the_tool_list_check_names_every_tool_that_differs_from_the_pin(driver):
    pinned = MANIFEST["tools"]
    same = [{"name": t["name"], "input_schema": t["inputSchema"]} for t in pinned]
    assert driver.tool_list_check(same, pinned).ok
    changed = [same[0], {"name": "zoom", "input_schema": {}}]
    check = driver.tool_list_check(changed, pinned)
    assert not check.ok and "check_permissions" in check.detail and "zoom" in check.detail


@pytest.mark.parametrize(
    "state, ok",
    [
        (
            {"accessibility": True, "screen_recording": True, "screen_recording_capturable": True},
            True,
        ),
        (
            {"accessibility": True, "screen_recording": True, "screen_recording_capturable": False},
            False,
        ),
        (
            {"accessibility": False, "screen_recording": True, "screen_recording_capturable": True},
            False,
        ),
        ({"accessibility": True, "screen_recording": True, "screen_recording_capturable": None}, True),
        ({}, False),
    ],
)
def test_the_permissions_check_wants_both_grants_and_a_working_capture(driver, state, ok):
    check = driver.permissions_check(state)
    assert check.ok is ok
    if not ok:
        assert "not granted" in check.detail


def test_the_pinned_app_is_the_0_38_0_release(driver):
    assert driver.VERSION == "0.38.0"
    assert driver.EXECUTABLE == Path("/Applications/CuaDriver.app/Contents/MacOS/cua-driver")
    assert len(driver.EXECUTABLE_SHA256) == 64
    assert driver.SOCKET.parts[-3:] == ("Caches", "agentenv", "cua-driver.sock")


def test_forwarded_tools_are_the_pinned_ones(driver):
    tools = driver.CuaDriverMacOS().forwarded_tools()
    assert [t.tool.name for t in tools] == ["click", "check_permissions"]


@pytest.mark.parametrize("action", ["click", "drag", "scroll"])
async def test_pointer_actions_are_delivered_in_the_foreground(driver, monkeypatch, action):
    sent: list[tuple] = []

    async def record(name, **args):
        sent.append((name, args))

    monkeypatch.setattr(driver, "DRAG_SETTLE_S", 0)
    env = driver.CuaDriverMacOS()
    env._send = record
    if action == "click":
        await env.click_at(5, 6, "left", 1, [])
    elif action == "drag":
        await env.drag_along([(1, 2), (3, 4)], "left", 100, [])
    else:
        await env.scroll_at(5, 6, "down", 2, [])

    assert [name for name, _ in sent][-1] == action
    assert all(args["delivery_mode"] == "foreground" for _, args in sent)
    if action == "drag":
        assert sent[0] == ("move_cursor", {"x": 1, "y": 2, "delivery_mode": "foreground"})
        assert 20 <= sent[1][1]["steps"] <= 200
