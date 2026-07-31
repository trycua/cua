from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path
from typing import Any

import pytest

DRIVER_ROOT = Path(__file__).parents[2]
NATIVE_LIBRARY = DRIVER_ROOT / "python/src/cua_driver/libcua_driver_sdk.dylib"
HARNESS_APP = DRIVER_ROOT / "rust/test-apps/harness-appkit/CuaTestHarness.AppKit.app"

pytestmark = [
    pytest.mark.skipif(sys.platform != "darwin", reason="macOS AppKit integration"),
    pytest.mark.skipif(
        os.environ.get("CUA_DRIVER_RUN_MACOS_INTEGRATION") != "1",
        reason="set CUA_DRIVER_RUN_MACOS_INTEGRATION=1 to run",
    ),
]


def test_real_appkit_click_moves_to_secondary_and_keeps_frontmost_app(
    tmp_path: Path,
) -> None:
    if not NATIVE_LIBRARY.exists():
        pytest.skip("host-native Python library is not staged")
    if not HARNESS_APP.exists():
        pytest.skip("AppKit test app is not built")

    from cua_driver.testing import CuaTestSession

    async def scenario() -> None:
        session = CuaTestSession.create(
            artifacts_dir=tmp_path,
        )
        target_pid: int | None = None
        try:
            frontmost_before_launch = await session._require_active_pid()
            app = await session.launch(
                bundle_id="com.trycua.harness.appkit",
                window_title="CuaTestHarness AppKit",
                timeout=10,
            )
            target_pid = app.pid
            assert await session._require_active_pid() == frontmost_before_launch
            initial_placement = app.launch_metadata["initial_window_placement"]
            assert initial_placement["verified"] is True
            assert initial_placement["activated"] is False
            assert app.window.frame is not None
            assert await _frame_is_on_secondary_display(session, app.window.frame)
            frame_before_action = app.window.frame
            z_index_before_action = app.window.z_index

            await app.wait_for_text("counter=0")
            button = await app.buttons.by_id("btn-increment").wait_for_exists()
            assert button.identifier == "btn-increment"
            assert button.raw["identifier"] == "btn-increment"
            await app.buttons.by_id("btn-increment").tap()
            await app.wait_for_text("counter=1")

            field = app.text_fields.by_id("txt-input")
            await field.type_text("background input")
            await field.wait_for_value("background input")

            assert app.window.frame == frame_before_action
            assert app.window.z_index is not None
            assert z_index_before_action is not None
            await _assert_target_stays_below_active_app(session, app.window)
            assert await _frame_is_on_secondary_display(session, app.window.frame)
            assert await session._require_active_pid() == frontmost_before_launch
        finally:
            await session.close()
            if target_pid is not None:
                await _wait_for_process_exit(target_pid)

    async def run_with_timeout() -> None:
        async with asyncio.timeout(30):
            await scenario()

    asyncio.run(run_with_timeout())


async def _wait_for_process_exit(pid: int) -> None:
    for _ in range(50):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        await asyncio.sleep(0.02)
    raise AssertionError(f"test app pid {pid} was not terminated")


async def _frame_is_on_secondary_display(session: Any, frame: Any) -> bool:
    display = await session._secondary_display()
    if display is None:
        return False
    center_x = frame.x + frame.width / 2
    center_y = frame.y + frame.height / 2
    return (
        display.frame.x <= center_x <= display.frame.x + display.frame.width
        and display.frame.y <= center_y <= display.frame.y + display.frame.height
    )


async def _assert_target_stays_below_active_app(session: Any, target: Any) -> None:
    active_pid = await session._require_active_pid()
    windows = await session._list_windows()
    active_z_indexes = [
        window.z_index
        for window in windows
        if window.pid == active_pid
        and window.is_on_screen is not False
        and window.z_index is not None
    ]
    assert active_z_indexes
    assert target.z_index is not None
    assert target.z_index < max(active_z_indexes)
