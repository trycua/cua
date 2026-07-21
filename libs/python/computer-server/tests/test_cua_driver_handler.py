import base64
import json
from io import BytesIO
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from PIL import Image

from computer_server.cli import parse_args
from computer_server.handlers.cua_driver import CuaDriverAutomationHandler


class FakeDriver:
    def __init__(self) -> None:
        self.calls = []
        image = Image.new("RGB", (2, 1), "red")
        output = BytesIO()
        image.save(output, format="PNG")
        self.png_base64 = base64.b64encode(output.getvalue()).decode("ascii")

    def call_tool(self, name: str, arguments_json: str):
        arguments = json.loads(arguments_json)
        self.calls.append((name, arguments))
        structured = {
            "start_session": {"session": arguments["session"], "active": True},
            "get_cursor_position": {"available": True, "x": 12, "y": 34},
            "get_screen_size": {"width": 1024, "height": 768, "scale_factor": 1},
        }.get(name, {"verified": True})
        images = (
            [SimpleNamespace(mime_type="image/png", data_base64=self.png_base64)]
            if name == "get_desktop_state"
            else []
        )
        return SimpleNamespace(
            structured_json=json.dumps(structured),
            images=images,
            is_error=False,
            text="",
            error_code=None,
        )


def fake_fallback():
    return SimpleNamespace(
        mouse_down=AsyncMock(return_value={"success": True, "source": "fallback"}),
        mouse_up=AsyncMock(return_value={"success": True, "source": "fallback"}),
        key_down=AsyncMock(return_value={"success": True, "source": "fallback"}),
        key_up=AsyncMock(return_value={"success": True, "source": "fallback"}),
        copy_to_clipboard=AsyncMock(
            return_value={"success": True, "source": "fallback", "content": "copied"}
        ),
        set_clipboard=AsyncMock(return_value={"success": True, "source": "fallback"}),
        run_command=AsyncMock(return_value={"success": True, "source": "fallback"}),
    )


def test_cli_accepts_cua_driver_backend():
    assert parse_args(["--backend", "cua-driver"]).backend == "cua-driver"


@pytest.mark.asyncio
async def test_declares_one_desktop_session_and_translates_pointer_calls():
    driver = FakeDriver()
    handler = CuaDriverAutomationHandler(fake_fallback(), driver=driver, session_id="compat-test")

    assert await handler.left_click(100, 200) == {
        "success": True,
        "verified": True,
    }
    assert await handler.move_cursor(300, 400) == {
        "success": True,
        "verified": True,
    }

    assert driver.calls == [
        (
            "start_session",
            {"session": "compat-test", "capture_scope": "desktop"},
        ),
        (
            "click",
            {
                "x": 100,
                "y": 200,
                "scope": "desktop",
                "button": "left",
                "count": 1,
                "session": "compat-test",
            },
        ),
        (
            "move_cursor",
            {"x": 300, "y": 400, "scope": "desktop", "session": "compat-test"},
        ),
    ]


@pytest.mark.asyncio
async def test_uses_cursor_for_implicit_click_and_scroll_coordinates():
    driver = FakeDriver()
    handler = CuaDriverAutomationHandler(fake_fallback(), driver=driver)

    await handler.right_click()
    await handler.scroll(0, -3)

    assert [name for name, _ in driver.calls] == [
        "start_session",
        "get_cursor_position",
        "click",
        "get_cursor_position",
        "scroll",
    ]
    assert driver.calls[2][1] == {
        "x": 12,
        "y": 34,
        "scope": "desktop",
        "button": "right",
        "count": 1,
        "session": "computer-server-compat",
    }
    assert driver.calls[4][1] == {
        "x": 12,
        "y": 34,
        "direction": "down",
        "scope": "desktop",
        "by": "line",
        "amount": 3,
        "session": "computer-server-compat",
    }


@pytest.mark.asyncio
async def test_normalizes_legacy_hotkey_and_drag_values_to_driver_contract():
    driver = FakeDriver()
    handler = CuaDriverAutomationHandler(fake_fallback(), driver=driver)

    assert (await handler.hotkey(["enter"]))["success"] is True
    assert (await handler.drag([(index, index) for index in range(250)], duration=20))["success"]

    assert driver.calls[1] == (
        "press_key",
        {
            "key": "enter",
            "scope": "desktop",
            "modifiers": [],
            "session": "computer-server-compat",
        },
    )
    assert driver.calls[2][0] == "drag"
    assert driver.calls[2][1]["duration_ms"] == 10000
    assert driver.calls[2][1]["steps"] == 200


@pytest.mark.asyncio
async def test_preserves_computer_server_screenshot_shapes():
    driver = FakeDriver()
    handler = CuaDriverAutomationHandler(fake_fallback(), driver=driver)

    png = await handler.screenshot()
    jpeg = await handler.screenshot(format="jpeg", quality=80)

    assert png == {"success": True, "image_data": driver.png_base64, "format": "png"}
    assert jpeg["success"] is True
    assert jpeg["format"] == "jpeg"
    with Image.open(BytesIO(base64.b64decode(jpeg["image_data"]))) as image:
        assert image.format == "JPEG"
        assert image.size == (2, 1)


@pytest.mark.asyncio
async def test_preserves_non_driver_operations_on_legacy_fallback():
    fallback = fake_fallback()
    handler = CuaDriverAutomationHandler(fallback, driver=FakeDriver())

    assert (await handler.mouse_down(1, 2))["source"] == "fallback"
    assert (await handler.key_down("shift"))["source"] == "fallback"
    assert (await handler.copy_to_clipboard())["content"] == "copied"
    assert (await handler.run_command("true", 1))["source"] == "fallback"

    fallback.mouse_down.assert_awaited_once_with(1, 2, "left")
    fallback.key_down.assert_awaited_once_with("shift")
    fallback.run_command.assert_awaited_once_with("true", 1)


@pytest.mark.asyncio
async def test_returns_legacy_error_shape_when_driver_rejects_call():
    driver = FakeDriver()

    def reject(name: str, arguments_json: str):
        if name == "start_session":
            return FakeDriver.call_tool(driver, name, arguments_json)
        return SimpleNamespace(
            structured_json=None,
            images=[],
            is_error=True,
            text="not allowed",
            error_code="permission_denied",
        )

    driver.call_tool = reject
    handler = CuaDriverAutomationHandler(fake_fallback(), driver=driver)

    assert await handler.type_text("hello") == {
        "success": False,
        "error": "not allowed (permission_denied)",
    }
