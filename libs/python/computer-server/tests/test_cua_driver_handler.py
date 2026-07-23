import json
from enum import Enum
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from computer_server.handlers.cua_driver import CuaDriverAutomationHandler
from computer_server.handlers.factory import HandlerFactory


class _Record:
    def __init__(self, **values):
        self.__dict__.update(values)


class _CaptureScope(Enum):
    AUTO = "auto"
    WINDOW = "window"
    DESKTOP = "desktop"


class _DesktopScope(Enum):
    DESKTOP = "desktop"


class _EffectiveScope(Enum):
    WINDOW = "window"
    DESKTOP = "desktop"


class _EscalationReason(Enum):
    AX_TREE_PIXEL_MISMATCH = "ax_tree_pixel_mismatch"
    BACKGROUND_DELIVERY_FAILED = "background_delivery_failed"
    FOREGROUND_INEFFECTIVE = "foreground_ineffective"
    NO_WINDOW_TARGET = "no_window_target"
    OTHER = "other"


class _ClickButton(Enum):
    LEFT = "left"
    RIGHT = "right"
    MIDDLE = "middle"


class _ScrollDirection(Enum):
    UP = "up"
    DOWN = "down"
    LEFT = "left"
    RIGHT = "right"


class _ScrollBy(Enum):
    LINE = "line"


class _Result:
    def __init__(self, structured=None, *, images=None, error=None):
        self.structured_json = json.dumps(structured or {})
        self.images = images or []
        self.is_error = error is not None
        self.text = error or "ok"
        self.error_code = "test_error" if error else None
        self.verified = True
        self.degraded = False


class _Driver:
    def __init__(self):
        self.calls = []
        self.ended = []
        self.shutdown_count = 0

    async def start_session(self, input):
        self.calls.append(("start_session", input))
        return SimpleNamespace(active=True)

    async def end_session(self, input):
        self.ended.append(input)
        return SimpleNamespace(active=False)

    async def get_session_state(self, input):
        self.calls.append(("get_session_state", input))
        return SimpleNamespace(
            session=input.session,
            capture_scope=_CaptureScope.AUTO,
            effective_scope=_EffectiveScope.WINDOW,
            desktop_unlocked=False,
            escalation_reason=None,
            escalation_detail=None,
        )

    async def escalate_session(self, input):
        self.calls.append(("escalate_session", input))
        return SimpleNamespace(
            session=input.session,
            capture_scope=_CaptureScope.AUTO,
            effective_scope=_EffectiveScope.DESKTOP,
            desktop_unlocked=True,
            escalation_reason=input.reason,
            escalation_detail=input.detail,
        )

    async def get_desktop_state(self, input):
        self.calls.append(("get_desktop_state", input))
        return _Result(
            {
                "screen_width": 1280,
                "screen_height": 720,
                "screenshot_width": 1280,
                "screenshot_height": 720,
                "scale_factor": 1.0,
            },
            images=[SimpleNamespace(mime_type="image/png", data_base64="cG5n")],
        )

    async def get_screen_size(self, input):
        self.calls.append(("get_screen_size", input))
        return _Result({"width": 1280, "height": 720, "scale_factor": 1.0})

    async def get_cursor_position(self, input):
        self.calls.append(("get_cursor_position", input))
        return _Result({"x": 12, "y": 34, "available": True})

    async def move_cursor(self, input):
        self.calls.append(("move_cursor", input))
        return _Result({})

    async def click(self, input):
        self.calls.append(("click", input))
        return _Result({})

    async def drag(self, input):
        self.calls.append(("drag", input))
        return _Result({})

    async def scroll(self, input):
        self.calls.append(("scroll", input))
        return _Result({})

    async def type_text(self, input):
        self.calls.append(("type_text", input))
        return _Result({})

    async def press_key(self, input):
        self.calls.append(("press_key", input))
        return _Result({})

    async def hotkey(self, input):
        self.calls.append(("hotkey", input))
        return _Result({})

    async def shutdown(self):
        self.shutdown_count += 1


@pytest.fixture
def sdk():
    return SimpleNamespace(
        CaptureScope=_CaptureScope,
        DesktopScope=_DesktopScope,
        EffectiveScope=_EffectiveScope,
        EscalationReason=_EscalationReason,
        ClickButton=_ClickButton,
        ScrollDirection=_ScrollDirection,
        ScrollBy=_ScrollBy,
        StartSessionInput=_Record,
        EndSessionInput=_Record,
        EscalateSessionInput=_Record,
        GetSessionStateInput=_Record,
        GetDesktopStateInput=_Record,
        GetScreenSizeInput=_Record,
        GetCursorPositionInput=_Record,
        MoveCursorInput=_Record,
        ClickInput=_Record,
        DragInput=_Record,
        ScrollInput=_Record,
        TypeTextInput=_Record,
        PressKeyInput=_Record,
        HotkeyInput=_Record,
    )


@pytest.fixture
def fallback():
    return SimpleNamespace(
        mouse_down=AsyncMock(return_value={"success": True}),
        mouse_up=AsyncMock(return_value={"success": True}),
        key_down=AsyncMock(return_value={"success": True}),
        key_up=AsyncMock(return_value={"success": True}),
        copy_to_clipboard=AsyncMock(return_value={"success": True, "content": "x"}),
        set_clipboard=AsyncMock(return_value={"success": True}),
        run_command=AsyncMock(return_value={"success": True, "stdout": ""}),
    )


@pytest.mark.asyncio
async def test_desktop_actions_share_one_typed_session(sdk, fallback):
    driver = _Driver()
    handler = CuaDriverAutomationHandler(
        fallback,
        driver=driver,
        sdk=sdk,
        session_id="server-a",
        capture_scope="desktop",
    )

    desktop = await handler.get_desktop_state()
    click = await handler.left_click()
    scroll = await handler.scroll_down(3)

    assert desktop["success"] is True
    assert desktop["image_data"] == "cG5n"
    assert desktop["screen_width"] == 1280
    assert click["success"] is True
    assert scroll["success"] is True
    assert [name for name, _ in driver.calls].count("start_session") == 1
    start = next(value for name, value in driver.calls if name == "start_session")
    assert start.session == "server-a"
    assert start.capture_scope is _CaptureScope.DESKTOP
    clicked = next(value for name, value in driver.calls if name == "click")
    assert (clicked.x, clicked.y) == (12.0, 34.0)
    assert clicked.scope is _DesktopScope.DESKTOP


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("capture_scope", "expected"),
    [("auto", _CaptureScope.AUTO), ("window", _CaptureScope.WINDOW)],
)
async def test_session_capture_scope_is_configurable(sdk, fallback, capture_scope, expected):
    driver = _Driver()
    handler = CuaDriverAutomationHandler(
        fallback,
        driver=driver,
        sdk=sdk,
        session_id="scoped",
        capture_scope=capture_scope,
    )

    await handler.get_screen_size()

    start = next(value for name, value in driver.calls if name == "start_session")
    assert start.capture_scope is expected


@pytest.mark.asyncio
async def test_adapter_uses_the_generated_python_contract(fallback):
    cua_driver = pytest.importorskip("cua_driver")
    driver = _Driver()
    handler = CuaDriverAutomationHandler(
        fallback,
        driver=driver,
        sdk=cua_driver,
        session_id="generated-contract",
        capture_scope="desktop",
    )

    result = await handler.get_screen_size()

    assert result == {"success": True, "size": {"width": 1280, "height": 720}}
    start = next(value for name, value in driver.calls if name == "start_session")
    assert isinstance(start, cua_driver.StartSessionInput)
    assert start.capture_scope is cua_driver.CaptureScope.DESKTOP
    request = next(value for name, value in driver.calls if name == "get_screen_size")
    assert isinstance(request, cua_driver.GetScreenSizeInput)


@pytest.mark.asyncio
async def test_auto_scope_escalation_is_explicit_and_typed(sdk, fallback):
    driver = _Driver()
    handler = CuaDriverAutomationHandler(
        fallback,
        driver=driver,
        sdk=sdk,
        session_id="auto-session",
        capture_scope="auto",
    )

    before = await handler.get_capture_scope_state()
    escalated = await handler.escalate_capture_scope(
        "no_window_target", "the window ladder found no target"
    )

    assert before["effective_scope"] == "window"
    assert before["desktop_unlocked"] is False
    assert escalated["effective_scope"] == "desktop"
    assert escalated["desktop_unlocked"] is True
    assert escalated["escalation_reason"] == "no_window_target"
    request = next(value for name, value in driver.calls if name == "escalate_session")
    assert request.reason is _EscalationReason.NO_WINDOW_TARGET
    assert request.detail == "the window ladder found no target"


@pytest.mark.asyncio
async def test_invalid_escalation_reason_is_rejected_without_calling_driver(sdk, fallback):
    driver = _Driver()
    handler = CuaDriverAutomationHandler(fallback, driver=driver, sdk=sdk)

    result = await handler.escalate_capture_scope("because")

    assert result["success"] is False
    assert not any(name == "escalate_session" for name, _ in driver.calls)


@pytest.mark.asyncio
async def test_close_ends_owned_session_and_shuts_down_once(sdk, fallback):
    driver = _Driver()
    handler = CuaDriverAutomationHandler(
        fallback, driver=driver, sdk=sdk, session_id="owned-session"
    )
    await handler.get_cursor_position()

    await handler.close()
    await handler.close()

    assert [value.session for value in driver.ended] == ["owned-session"]
    assert driver.shutdown_count == 1


@pytest.mark.asyncio
async def test_non_portable_actions_remain_on_legacy_handler(sdk, fallback):
    handler = CuaDriverAutomationHandler(fallback, driver=_Driver(), sdk=sdk)

    assert await handler.mouse_down(1, 2, "left") == {"success": True}
    assert await handler.key_down("shift") == {"success": True}
    assert await handler.copy_to_clipboard() == {"success": True, "content": "x"}

    fallback.mouse_down.assert_awaited_once_with(1, 2, "left")
    fallback.key_down.assert_awaited_once_with("shift")
    fallback.copy_to_clipboard.assert_awaited_once_with()


def test_invalid_driver_configuration_is_rejected(sdk, fallback):
    with pytest.raises(ValueError, match="CUA_DRIVER_MODE"):
        CuaDriverAutomationHandler(fallback, driver=_Driver(), sdk=sdk, mode="rpc")
    with pytest.raises(ValueError, match="CUA_DRIVER_CAPTURE_SCOPE"):
        CuaDriverAutomationHandler(fallback, driver=_Driver(), sdk=sdk, capture_scope="everything")


@pytest.mark.asyncio
async def test_factory_closes_and_clears_the_shared_runtime():
    automation = SimpleNamespace(close=AsyncMock())
    handlers = (object(), automation, object(), object(), object(), object())
    HandlerFactory._shared_handlers = handlers

    await HandlerFactory.close_handlers()
    await HandlerFactory.close_handlers()

    automation.close.assert_awaited_once_with()
    assert HandlerFactory._shared_handlers is None
