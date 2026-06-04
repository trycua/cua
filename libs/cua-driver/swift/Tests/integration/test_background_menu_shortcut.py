"""Integration test: hotkey with menu key equivalents on a backgrounded app.

Root cause being tested
-----------------------
`hotkey` without window_id uses the SkyLight auth-envelope path which
bypasses IOHIDPostEvent — NSMenu key equivalents are silently dropped.

Fix: when `window_id` is supplied, `hotkey` / `press_key` call
`FocusWithoutRaise.withMenuShortcutActivation` which:
  1. Saves prior frontmost PSN.
  2. Calls `SLPSSetFrontProcessWithOptions(kCPSNoWindows)` to make the
     target WindowServer-frontmost without raising windows.
  3. Posts the key WITHOUT the auth envelope (routes through IOHIDPostEvent
     so NSMenu.performKeyEquivalent: dispatches the shortcut).
  4. Immediately restores the prior frontmost — all in < 1 ms so the
     5 ms UXMonitor never observes the intermediate state.

Behavioral contract
-------------------
WITHOUT window_id: auth-envelope path, 0 UX violations.
WITH window_id:    NSMenu key equivalent fires (Cmd+N opens new document),
                   0 UX violations (activate+restore is sub-millisecond).

Run:
    pytest Tests/integration/test_background_menu_shortcut.py -v
"""

from __future__ import annotations

import subprocess
import time

import pytest

TEXTEDIT_BUNDLE = "com.apple.TextEdit"


def _on_screen_doc_windows(driver, pid: int) -> list[dict]:
    result = driver.call_tool("list_windows", {"pid": pid})
    windows = result.get("structuredContent", {}).get("windows", [])
    return [
        w for w in windows
        if w.get("is_on_screen")
        and (w.get("bounds", {}).get("width", 0) or 0) > 100
        and (w.get("bounds", {}).get("height", 0) or 0) > 100
    ]


# ── module-scoped driver (avoids scope-mismatch with module fixtures) ─────────

@pytest.fixture(scope="module")
def module_driver(binary):
    """Module-scoped driver used by setup fixtures (textedit_pid, textedit_wid).

    The function-scoped `driver` from conftest gives each test its own process;
    module-scoped fixtures must use this dedicated module_driver instead.
    """
    from harness.driver import Driver
    with Driver(binary) as d:
        yield d


# ── module fixtures ───────────────────────────────────────────────────────────

def _pid_from_result(result: dict) -> int:
    """Extract pid from a launch_app result (structuredContent or text fallback)."""
    import re
    pid = result.get("structuredContent", {}).get("pid", 0)
    if pid:
        return pid
    # Fall back: parse "pid N" from the text content
    for item in result.get("content", []):
        text = item.get("text", "")
        m = re.search(r'\bpid\s+(\d+)', text)
        if m:
            return int(m.group(1))
    return 0


@pytest.fixture(scope="module")
def textedit_pid(module_driver):
    """Launch TextEdit fresh and return its pid."""
    subprocess.run(["pkill", "-x", "TextEdit"], check=False)
    time.sleep(0.5)
    result = module_driver.call_tool("launch_app", {"bundle_id": TEXTEDIT_BUNDLE})
    pid = _pid_from_result(result)
    assert pid > 0, f"launch_app TextEdit failed: {result}"
    time.sleep(1.0)

    wins = _on_screen_doc_windows(module_driver, pid)
    assert wins, f"TextEdit has no on-screen windows after launch. pid={pid}"
    yield pid

    subprocess.run(["pkill", "-x", "TextEdit"], check=False)


@pytest.fixture(scope="module")
def textedit_wid(module_driver, textedit_pid):
    """Return the window_id of the largest TextEdit on-screen window."""
    wins = _on_screen_doc_windows(module_driver, textedit_pid)
    best = max(wins, key=lambda w: (
        w.get("bounds", {}).get("width", 0) * w.get("bounds", {}).get("height", 0)
    ))
    return best["window_id"]


# ── tests ─────────────────────────────────────────────────────────────────────

def test_hotkey_without_window_id_does_not_activate(
    driver, focus_monitor, activate_focus_monitor, ux_guard, textedit_pid
):
    """hotkey WITHOUT window_id must not violate any UX invariants.

    Uses the auth-envelope path which delivers directly to the target PID
    without calling activateForMenuShortcut — FocusMonitorApp keeps focus.
    """
    driver.call_tool("hotkey", {
        "pid": textedit_pid,
        "keys": ["cmd", "z"],
    })
    time.sleep(0.5)
    # ux_guard.assert_clean() fires in teardown


def test_hotkey_with_window_id_fires_nsmenu(
    driver, focus_monitor, activate_focus_monitor, ux_guard,
    textedit_pid, textedit_wid
):
    """hotkey WITH window_id fires NSMenu key equivalents with zero UX violations.

    Sends Cmd+N (New Document) to backgrounded TextEdit. The driver uses
    withMenuShortcutActivation: activate → post → restore in < 1 ms, so
    the 5 ms UXMonitor never observes the brief frontmost change. NSMenu
    still fires because the key is already enqueued in the target's run-loop
    before restoration; AppKit dispatches it regardless of current frontmost.
    """
    count_before = len(_on_screen_doc_windows(driver, textedit_pid))

    driver.call_tool("hotkey", {
        "pid": textedit_pid,
        "keys": ["cmd", "n"],
        "window_id": textedit_wid,
    })
    time.sleep(1.5)

    count_after = len(_on_screen_doc_windows(driver, textedit_pid))

    # Cleanup extra windows
    if count_after > count_before:
        wins = _on_screen_doc_windows(driver, textedit_pid)
        for w in wins[count_before:]:
            wid = w["window_id"]
            driver.call_tool("hotkey", {
                "pid": textedit_pid, "keys": ["cmd", "w"], "window_id": wid})
            time.sleep(0.4)
            driver.call_tool("press_key", {
                "pid": textedit_pid, "key": "delete", "window_id": wid})
            time.sleep(0.3)

    assert count_after > count_before, (
        f"Cmd+N WITH window_id should open a new TextEdit document. "
        f"windows before={count_before}, after={count_after}."
    )
    print(
        f"\n  ✓ Cmd+N (with window_id): window opened "
        f"({count_before}→{count_after}), 0 UX violations"
    )
    # ux_guard.assert_clean() fires in teardown — verifies no focus/cursor/overlay violations
