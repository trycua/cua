#!/opt/venv/bin/python3
"""Verify the direct-driver XFCE POC from inside the running container."""

import json
import sys
import uuid

from cua_driver import (
    CaptureScope,
    CuaDriver,
    DesktopScope,
    EndSessionInput,
    GetCursorPositionInput,
    GetDesktopStateInput,
    GetScreenSizeInput,
    MoveCursorInput,
    StartSessionInput,
)


def require_success(name, result):
    if result.is_error:
        raise RuntimeError(f"{name} failed: {result.text or result.error_code}")
    return json.loads(result.structured_json or "{}")


def main() -> int:
    driver = CuaDriver.connect(None)
    if not driver.is_available():
        print("Cua Driver daemon is not available", file=sys.stderr)
        return 1

    session = f"xfce-sdk-smoke-{uuid.uuid4().hex[:8]}"
    try:
        started = driver.start_session(
            StartSessionInput(session=session, capture_scope=CaptureScope.DESKTOP)
        )
        if not started.active:
            raise RuntimeError("start_session did not activate the SDK session")
        size = require_success(
            "get_screen_size",
            driver.get_screen_size(GetScreenSizeInput(session=session)),
        )
        desktop = driver.get_desktop_state(
            GetDesktopStateInput(session=session, screenshot_out_file=None)
        )
        require_success("get_desktop_state", desktop)
        if not desktop.images:
            raise RuntimeError("get_desktop_state returned no image")
        cursor = require_success(
            "get_cursor_position",
            driver.get_cursor_position(GetCursorPositionInput(session=session)),
        )
        require_success(
            "move_cursor",
            driver.move_cursor(
                MoveCursorInput(
                    x=cursor["x"],
                    y=cursor["y"],
                    scope=DesktopScope.DESKTOP,
                    session=session,
                )
            ),
        )
        print(
            json.dumps(
                {
                    "ok": True,
                    "screen": {"width": size["width"], "height": size["height"]},
                    "screenshot_mime_type": desktop.images[0].mime_type,
                    "input_verified": True,
                },
                sort_keys=True,
            )
        )
        return 0
    finally:
        driver.end_session(EndSessionInput(session=session))


if __name__ == "__main__":
    raise SystemExit(main())
