"""Integration test: pixel-addressed click routes through AX hit-test.

The pixel path (`click({pid, x, y})`) is supposed to try AX first and
fall back to CGEvent only when the pixel hits a non-AX surface. This
test clicks Calculator's "9" button by pixel and asserts the driver
returns `dispatch == "ax_hit_test"` with `role == "AXButton"` in the
structured content — same behavioral contract agents can rely on when
deciding whether to retry after a pixel-click failure.

Calculator is used because it's always installed, the "9" button has a
stable AXIdentifier ("Nine"), and its digit keypad is a clean AX-press
target (no modifier / no double-click interactions needed).
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from driver_client import (
    DriverClient,
    default_binary_path,
    reset_calculator,
    resolve_window_id,
)


def _osascript(script: str) -> str:
    """Run an AppleScript one-liner and return stdout (stripped)."""
    proc = subprocess.run(
        ["osascript", "-e", script],
        capture_output=True,
        text=True,
        check=False,
    )
    return proc.stdout.strip()


def _nine_button_rect() -> tuple[float, float, float, float] | None:
    """Return (x, y, w, h) in screen points for Calculator's "9" button.

    Walks the keypad group looking for AXIdentifier == "Nine".
    """
    script = (
        'tell application "System Events"\n'
        '  tell process "Calculator"\n'
        "    set btns to every button of group 1 of group 1 of splitter group 1 of group 1 of window 1\n"
        "    repeat with b in btns\n"
        "      try\n"
        '        if (value of attribute "AXIdentifier" of b) is "Nine" then\n'
        "          set pos to position of b\n"
        "          set sz to size of b\n"
        '          return (item 1 of pos as text) & "," & (item 2 of pos as text) & "," & (item 1 of sz as text) & "," & (item 2 of sz as text)\n'
        "        end if\n"
        "      end try\n"
        "    end repeat\n"
        '    return "NOT_FOUND"\n'
        "  end tell\n"
        "end tell"
    )
    out = _osascript(script)
    if not out or out == "NOT_FOUND":
        return None
    parts = out.split(",")
    if len(parts) != 4:
        return None
    return tuple(float(p) for p in parts)  # type: ignore[return-value]


def _window_origin() -> tuple[float, float] | None:
    script = (
        'tell application "System Events"\n'
        '  tell process "Calculator"\n'
        "    try\n"
        "      set pos to position of window 1\n"
        '      return (item 1 of pos as text) & "," & (item 2 of pos as text)\n'
        "    on error\n"
        '      return "NOT_FOUND"\n'
        "    end try\n"
        "  end tell\n"
        "end tell"
    )
    out = _osascript(script)
    if not out or out == "NOT_FOUND":
        return None
    parts = out.split(",")
    if len(parts) != 2:
        return None
    return (float(parts[0]), float(parts[1]))


class ClickPixelAxHitTestTests(unittest.TestCase):
    def setUp(self) -> None:
        self._client_cm = DriverClient(default_binary_path())
        self.client = self._client_cm.__enter__()
        reset_calculator()

    def tearDown(self) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        self._client_cm.__exit__(None, None, None)

    def test_pixel_click_dispatches_via_ax_hit_test(self) -> None:
        # The AX hit-test path is gated on `capture_mode != vision` —
        # in `vision` mode pixel clicks intentionally go straight to
        # CGEvent / SkyLight because no AX walk has populated the
        # hit-test cache. Force `som` (tree + screenshot, the shipped
        # default) so this assertion doesn't flap based on whatever
        # capture_mode the previous session left on disk.
        set_mode = self.client.call_tool(
            "set_config", {"key": "capture_mode", "value": "som"}
        )
        self.assertIsNone(set_mode.get("isError"), msg=set_mode)

        launch = self.client.call_tool(
            "launch_app", {"bundle_id": "com.apple.calculator"}
        )
        self.assertIsNone(launch.get("isError"), msg=launch)
        pid = launch["structuredContent"]["pid"]

        # AppKit needs a beat after launch_app's hidden-launch to finish
        # window setup before the AX rect is queryable.
        time.sleep(1.0)

        # Populate the snapshot cache so element_index paths work if
        # the test ever needs them; harmless for the pixel assertion.
        window_id = resolve_window_id(self.client, pid)
        snap = self.client.call_tool(
            "get_window_state", {"pid": pid, "window_id": window_id}
        )
        self.assertIsNone(snap.get("isError"), msg=snap)
        scale = snap["structuredContent"].get("screenshot_scale_factor") or 1.0

        rect = _nine_button_rect()
        if rect is None:
            self.skipTest(
                "Could not resolve Calculator's '9' button via osascript "
                "(layout changed? AX permission missing?)."
            )
        bx, by, bw, bh = rect
        cx_screen = bx + bw / 2.0
        cy_screen = by + bh / 2.0

        origin = _window_origin()
        if origin is None:
            self.skipTest("Could not resolve Calculator window origin.")
        wx0, wy0 = origin

        image_x = int(round((cx_screen - wx0) * scale))
        image_y = int(round((cy_screen - wy0) * scale))

        result = self.client.call_tool(
            "click",
            {"pid": pid, "window_id": window_id, "x": image_x, "y": image_y},
        )
        self.assertIsNone(result.get("isError"), msg=result)
        structured = result["structuredContent"]
        self.assertEqual(
            structured.get("dispatch"),
            "ax_hit_test",
            msg=f"expected AX hit-test dispatch, got: {structured}",
        )
        self.assertEqual(
            structured.get("role"),
            "AXButton",
            msg=f"expected role=AXButton, got: {structured}",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
