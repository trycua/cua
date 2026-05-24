"""Integration test: launch_app opens a visible window without stealing focus.

Root cause being tested
-----------------------
`launch_app` must open an on-screen window in the background without
stealing focus from the current foreground app.

For apps already running with windows on a different Space (Finder, Safari),
a home-directory URL fallback via `application(_:open:)` creates a new
window on the current Space without activation.

Run:
    pytest Tests/integration/test_launch_app_visible.py -v
"""

from __future__ import annotations

import subprocess
import time

import pytest

TEXTEDIT_BUNDLE = "com.apple.TextEdit"
CALCULATOR_BUNDLE = "com.apple.calculator"
FINDER_BUNDLE = "com.apple.finder"


def _on_screen_windows(driver, pid: int) -> list[dict]:
    result = driver.call_tool("list_windows", {"pid": pid})
    windows = result.get("structuredContent", {}).get("windows", [])
    return [
        w for w in windows
        if w.get("is_on_screen")
        and (w.get("bounds", {}).get("width", 0) or 0) > 50
        and (w.get("bounds", {}).get("height", 0) or 0) > 50
    ]


def _pid_from_result(result: dict) -> int:
    """Extract pid from a launch_app result (structuredContent or text fallback)."""
    import re
    pid = result.get("structuredContent", {}).get("pid", 0)
    if pid:
        return pid
    for item in result.get("content", []):
        text = item.get("text", "")
        m = re.search(r'\bpid\s+(\d+)', text)
        if m:
            return int(m.group(1))
    return 0


# ── module-scoped setup ───────────────────────────────────────────────────────

@pytest.fixture(scope="module", autouse=True)
def _kill_test_apps():
    """Kill test apps before the module so each test cold-launches cleanly."""
    subprocess.run(["pkill", "-x", "TextEdit"], check=False)
    subprocess.run(["pkill", "-x", "Calculator"], check=False)
    subprocess.run(["pkill", "-x", "Finder"], check=False)
    time.sleep(1.5)
    yield
    subprocess.run(["pkill", "-x", "TextEdit"], check=False)
    subprocess.run(["pkill", "-x", "Calculator"], check=False)


# ── tests ─────────────────────────────────────────────────────────────────────

def test_textedit_cold_launch_opens_visible_window(driver, focus_monitor, ux_guard):
    """launch_app(TextEdit) must open an on-screen window with 0 focus losses."""
    result = driver.call_tool("launch_app", {"bundle_id": TEXTEDIT_BUNDLE})
    pid = _pid_from_result(result)
    assert pid > 0, f"launch_app did not return a pid: {result}"

    time.sleep(2.0)
    on_screen = _on_screen_windows(driver, pid)
    assert len(on_screen) > 0, (
        f"TextEdit (pid={pid}) has no on-screen windows after launch_app. "
        f"All windows: {driver.call_tool('list_windows', {'pid': pid})}"
    )
    print(f"\n  ✓ TextEdit pid={pid} has {len(on_screen)} on-screen window(s)")
    # ux_guard.assert_clean() fires in teardown — verifies no UX violations


def test_calculator_cold_launch_focus_steal_suppressed(driver, focus_monitor, ux_guard):
    """launch_app(Calculator) must open a window and not steal focus permanently.

    Calculator calls NSApp.activate in applicationDidFinishLaunching.
    The focus-steal suppressor must detect and undo this, leaving
    FocusMonitorApp as frontmost.
    """
    subprocess.run(["pkill", "-x", "Calculator"], check=False)
    time.sleep(0.5)

    result = driver.call_tool("launch_app", {"bundle_id": CALCULATOR_BUNDLE})
    pid = _pid_from_result(result)
    assert pid > 0

    time.sleep(2.5)
    on_screen = _on_screen_windows(driver, pid)
    assert len(on_screen) > 0, (
        f"Calculator (pid={pid}) has no on-screen windows after launch_app. "
        f"All windows: {driver.call_tool('list_windows', {'pid': pid})}"
    )
    print(f"\n  ✓ Calculator pid={pid} has {len(on_screen)} on-screen window(s)")
    # ux_guard checks frontmost is still FocusMonitorApp in teardown


def test_finder_no_url_opens_visible_window(driver, focus_monitor, ux_guard):
    """launch_app(Finder) without urls must produce an on-screen window.

    When Finder is already running with windows on a different Space,
    launch_app must automatically open a home-directory window on the
    current Space via application(_:open:) — no explicit url required.
    """
    result = driver.call_tool("launch_app", {"bundle_id": FINDER_BUNDLE})
    pid = _pid_from_result(result)
    assert pid > 0

    time.sleep(2.0)
    on_screen = _on_screen_windows(driver, pid)
    assert len(on_screen) > 0, (
        f"Finder (pid={pid}) has no on-screen windows after launch_app (no url). "
        f"All windows: {driver.call_tool('list_windows', {'pid': pid})}"
    )
    print(f"\n  ✓ Finder pid={pid} has {len(on_screen)} on-screen window(s)")
