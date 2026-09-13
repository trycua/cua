from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from cua_bench.computers.remote import RemoteDesktopSession


@pytest.mark.asyncio
async def test_remote_screen_rect_uses_native_window_origin() -> None:
    remote_calls = []

    def python_command(*, use_system_python):
        assert use_system_python is True

        def decorate(_function):
            async def call(*args):
                remote_calls.append(args)
                return {"x": 10, "y": 20, "width": 30, "height": 40}

            return call

        return decorate

    session = RemoteDesktopSession()
    session._computer = SimpleNamespace(python_command=python_command)
    session._ensure_computer = AsyncMock()
    session._get_process_client_origin = AsyncMock(return_value={"x": 385, "y": 285})

    rect = await session.get_element_rect(123, ".target", space="screen")

    assert rect == {"x": 395, "y": 305, "width": 30, "height": 40}
    assert remote_calls == [(123, ".target", "window")]
    session._get_process_client_origin.assert_awaited_once_with(123)


@pytest.mark.asyncio
async def test_remote_screen_rect_preserves_bench_ui_mapping_outside_linux() -> None:
    remote_calls = []

    def python_command(*, use_system_python):
        assert use_system_python is True

        def decorate(_function):
            async def call(*args):
                remote_calls.append(args)
                return {"x": 10, "y": 20, "width": 30, "height": 40}

            return call

        return decorate

    session = RemoteDesktopSession(os_type="windows")
    session._computer = SimpleNamespace(python_command=python_command)
    session._ensure_computer = AsyncMock()
    session._get_process_client_origin = AsyncMock()

    rect = await session.get_element_rect(123, ".target", space="screen")

    assert rect == {"x": 10, "y": 20, "width": 30, "height": 40}
    assert remote_calls == [(123, ".target", "screen")]
    session._get_process_client_origin.assert_not_awaited()


@pytest.mark.asyncio
async def test_remote_element_click_waits_until_the_dom_observes_it() -> None:
    session = RemoteDesktopSession()
    interface = SimpleNamespace(
        move_cursor=AsyncMock(),
        left_click=AsyncMock(),
    )
    session._computer = SimpleNamespace(interface=interface)
    session.get_element_rect = AsyncMock(return_value={"x": 10, "y": 20, "width": 30, "height": 40})
    session._activate_process_window = AsyncMock(return_value=True)
    session._arm_element_event = AsyncMock(return_value="event-token")
    session._wait_for_element_event = AsyncMock(return_value={"observed": True})
    session._clear_element_event = AsyncMock()

    await session.click_element(123, ".target")

    session._activate_process_window.assert_awaited_once_with(123)
    session._arm_element_event.assert_awaited_once_with(123, ".target", "click")
    interface.move_cursor.assert_awaited_once_with(25, 40)
    interface.left_click.assert_awaited_once_with()
    session._wait_for_element_event.assert_awaited_once_with(123, "event-token")
    session._clear_element_event.assert_awaited_once_with(123, "event-token")


@pytest.mark.asyncio
async def test_remote_right_click_flushes_the_target_event_loop() -> None:
    session = RemoteDesktopSession()
    interface = SimpleNamespace(move_cursor=AsyncMock(), right_click=AsyncMock())
    session._computer = SimpleNamespace(interface=interface)
    session.get_element_rect = AsyncMock(return_value={"x": 10, "y": 20, "width": 30, "height": 40})
    session._activate_process_window = AsyncMock(return_value=True)
    session.execute_javascript = AsyncMock(return_value=True)

    await session.right_click_element(123, ".target")

    interface.move_cursor.assert_awaited_once_with(25, 40)
    interface.right_click.assert_awaited_once_with()
    session.execute_javascript.assert_awaited_once_with(123, "true")


@pytest.mark.asyncio
async def test_remote_element_click_never_retries_when_telemetry_is_missing() -> None:
    session = RemoteDesktopSession()
    interface = SimpleNamespace(
        move_cursor=AsyncMock(),
        left_click=AsyncMock(),
    )
    session._computer = SimpleNamespace(interface=interface)
    session.get_element_rect = AsyncMock(return_value={"x": 10, "y": 20, "width": 30, "height": 40})
    session._activate_process_window = AsyncMock(return_value=True)
    session._arm_element_event = AsyncMock(return_value="event-token")
    session._wait_for_element_event = AsyncMock(return_value=None)
    session._clear_element_event = AsyncMock()

    await session.click_element(123, ".target")

    interface.move_cursor.assert_awaited_once_with(25, 40)
    interface.left_click.assert_awaited_once_with()
    session._wait_for_element_event.assert_awaited_once_with(123, "event-token")
    session._clear_element_event.assert_awaited_once_with(123, "event-token")


@pytest.mark.asyncio
async def test_remote_element_click_never_retries_after_navigation() -> None:
    session = RemoteDesktopSession()
    interface = SimpleNamespace(move_cursor=AsyncMock(), left_click=AsyncMock())
    session._computer = SimpleNamespace(interface=interface)
    session.get_element_rect = AsyncMock(return_value={"x": 10, "y": 20, "width": 30, "height": 40})
    session._activate_process_window = AsyncMock(return_value=True)
    session._arm_element_event = AsyncMock(return_value="event-token")
    session._wait_for_element_event = AsyncMock(side_effect=RuntimeError("window navigated"))
    session._clear_element_event = AsyncMock()

    await session.click_element(123, ".target")

    interface.left_click.assert_awaited_once_with()
    session._clear_element_event.assert_awaited_once_with(123, "event-token")


@pytest.mark.asyncio
async def test_remote_element_click_fails_loudly_when_it_lands_elsewhere() -> None:
    session = RemoteDesktopSession()
    interface = SimpleNamespace(move_cursor=AsyncMock(), left_click=AsyncMock())
    session._computer = SimpleNamespace(interface=interface)
    session.get_element_rect = AsyncMock(return_value={"x": 10, "y": 20, "width": 30, "height": 40})
    session._activate_process_window = AsyncMock(return_value=True)
    session._arm_element_event = AsyncMock(return_value="event-token")
    session._wait_for_element_event = AsyncMock(return_value={"observed": False, "target": "DIV"})
    session._clear_element_event = AsyncMock()
    session.execute_javascript = AsyncMock(return_value={"expectedX": 25, "expectedY": 40})

    with pytest.raises(RuntimeError, match="landed outside selector"):
        await session.click_element(123, ".target")

    interface.left_click.assert_awaited_once_with()


@pytest.mark.asyncio
async def test_remote_element_click_fails_before_input_when_window_cannot_activate() -> None:
    session = RemoteDesktopSession()
    interface = SimpleNamespace(move_cursor=AsyncMock(), left_click=AsyncMock())
    session._computer = SimpleNamespace(interface=interface)
    session.get_element_rect = AsyncMock(return_value={"x": 10, "y": 20, "width": 30, "height": 40})
    session._activate_process_window = AsyncMock(return_value=False)

    with pytest.raises(RuntimeError, match="Could not activate remote window"):
        await session.click_element(123, ".target")

    interface.move_cursor.assert_not_awaited()
    interface.left_click.assert_not_awaited()
