"""Gestures-only harness: exercises every cua-driver input primitive in
controlled ways. Targets a single known app (Paint by default since it
gives a forgiving canvas surface for drag/scroll/click).

Goal: shake out per-gesture bugs that the apps × gestures matrix misses.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from overnight_harness import cua_call, shell, kill_app, find_window, APPS  # type: ignore


def setup(app: str = "paint") -> dict:
    kill_app(app)
    time.sleep(1)
    shell(f'start "" /B {APPS[app]["launch"]}', timeout=10)
    time.sleep(4)
    w = find_window(APPS[app]["title"], timeout_s=15)
    return w


def test_clicks(w: dict) -> dict:
    """left, right, middle, double, triple, click_at coords."""
    results = {}
    # Element-based click on the first button
    state = cua_call("get_window_state", json.dumps({"pid": w["pid"], "window_id": w["window_id"]}), timeout=30)
    # element_index=0 is usually the title; pick a clickable one
    if "tree_markdown" in state:
        # try to click at element_index=5 (probably some button)
        for idx in [5, 3, 1]:
            r = cua_call("click", json.dumps({"pid": w["pid"], "window_id": w["window_id"], "element_index": idx}), timeout=15)
            results[f"click_element_{idx}"] = r
    # x,y click at canvas
    cx, cy = w["bounds"]["x"] + 400, w["bounds"]["y"] + 400
    r = cua_call("click", json.dumps({"pid": w["pid"], "x": cx, "y": cy}), timeout=15)
    results[f"click_xy_{cx}_{cy}"] = r

    # Right click
    r = cua_call("click", json.dumps({"pid": w["pid"], "x": cx, "y": cy, "button": "right"}), timeout=15)
    results["right_click"] = r

    # Middle click
    r = cua_call("click", json.dumps({"pid": w["pid"], "x": cx, "y": cy, "button": "middle"}), timeout=15)
    results["middle_click"] = r

    # Double click
    r = cua_call("click", json.dumps({"pid": w["pid"], "x": cx, "y": cy, "count": 2}), timeout=15)
    results["double_click"] = r

    # Triple click
    r = cua_call("click", json.dumps({"pid": w["pid"], "x": cx, "y": cy, "count": 3}), timeout=15)
    results["triple_click"] = r

    return results


def test_drags(w: dict) -> dict:
    """drag, drag_to with various paths."""
    results = {}
    x0, y0 = w["bounds"]["x"] + 200, w["bounds"]["y"] + 300
    x1, y1 = w["bounds"]["x"] + 600, w["bounds"]["y"] + 500

    # Straight drag
    r = cua_call("drag", json.dumps({
        "pid": w["pid"], "from_x": x0, "from_y": y0, "to_x": x1, "to_y": y1
    }), timeout=20)
    results["drag_straight"] = r

    # Drag with explicit duration
    r = cua_call("drag", json.dumps({
        "pid": w["pid"], "from_x": x0, "from_y": y0, "to_x": x1, "to_y": y1, "duration_ms": 500
    }), timeout=20)
    results["drag_500ms"] = r

    # Drag with right button
    r = cua_call("drag", json.dumps({
        "pid": w["pid"], "from_x": x0, "from_y": y0, "to_x": x1, "to_y": y1, "button": "right"
    }), timeout=20)
    results["drag_right_button"] = r

    return results


def test_scrolls(w: dict) -> dict:
    """scroll up/down/horizontal — Windows uses WM_VSCROLL/WM_HSCROLL.

    Schema (per platform-windows scroll tool): {pid, direction, by?, amount?,
    window_id?, element_index?}. No dx/dy — those don't exist in cua-driver.
    """
    results = {}
    for direction in ["up", "down", "left", "right"]:
        for by in ["line", "page"]:
            for amount in [1, 5]:
                r = cua_call("scroll", json.dumps({
                    "pid": w["pid"],
                    "window_id": w["window_id"],
                    "direction": direction,
                    "by": by,
                    "amount": amount,
                }), timeout=15)
                results[f"scroll_{direction}_{by}_{amount}"] = r

    return results


def test_hotkeys(w: dict) -> dict:
    """various modifier combos and special keys."""
    results = {}
    for key in [
        "ctrl+z",
        "ctrl+y",
        "ctrl+a",
        "ctrl+c",
        "ctrl+v",
        "ctrl+shift+s",
        "f1",
        "f5",
        "f12",
        "alt+tab",
        "escape",
        "tab",
        "enter",
        "shift+tab",
        "ctrl+home",
        "ctrl+end",
        "page_down",
        "page_up",
        "arrow_down",
        "arrow_up",
        "arrow_left",
        "arrow_right",
    ]:
        r = cua_call("hotkey", json.dumps({"pid": w["pid"], "key": key}), timeout=15)
        results[f"hotkey_{key.replace('+','_')}"] = r

    return results


def test_type_text(w: dict) -> dict:
    """type_text variations: ASCII, Unicode, multiline, special chars."""
    results = {}
    for label, text in [
        ("ascii_simple", "hello"),
        ("unicode_emoji", "café ☕"),
        ("unicode_japanese", "こんにちは"),
        ("multiline", "line one\nline two\nline three"),
        ("special_chars", "!@#$%^&*()_+-={}[]"),
        ("long_500", "x" * 500),
        ("empty", ""),
    ]:
        r = cua_call("type_text", json.dumps({"pid": w["pid"], "text": text}), timeout=30)
        # Truncate the call response if any
        if "_text" in r:
            r["_text"] = r["_text"][:200]
        results[f"type_{label}"] = r

    return results


def test_press_key(w: dict) -> dict:
    """press_key — Swift/Linux equivalent of cross-platform single-key press.

    On Windows there's no separate mouse_down/mouse_up tool exposed via MCP
    (per cua-driver-rs/crates/platform-windows/src/tools/impl_.rs). Tested
    other primitives via hotkey above; press_key is the explicit
    single-press primitive on macOS+Linux.
    """
    results = {}
    for key in ["space", "backspace", "delete", "tab", "enter"]:
        r = cua_call("press_key", json.dumps({"pid": w["pid"], "key": key}), timeout=10)
        results[f"press_{key}"] = r
    return results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--app", default="paint")
    ap.add_argument("--test", action="append", default=[], help="Tests to run (clicks/drags/scrolls/hotkeys/type_text/mouse). Default = all.")
    args = ap.parse_args()

    w = setup(args.app)
    if not w:
        print(f"ERROR: {args.app} window not found", file=sys.stderr)
        sys.exit(1)
    print(f"Target: {args.app}  pid={w['pid']}  window_id={w['window_id']}  bounds={w['bounds']}", flush=True)

    SUITES = {
        "clicks": test_clicks,
        "drags": test_drags,
        "scrolls": test_scrolls,
        "hotkeys": test_hotkeys,
        "type_text": test_type_text,
        "press_key": test_press_key,
    }
    suites = args.test if args.test else list(SUITES.keys())

    all_results = {"app": args.app, "window": w, "suites": {}}
    for s in suites:
        print(f"  → running {s}…", flush=True)
        try:
            all_results["suites"][s] = SUITES[s](w)
        except Exception as e:
            all_results["suites"][s] = {"_runner_error": str(e)}

    out = Path(__file__).parent / "overnight-results" / f"gestures-{args.app}-{int(time.time())}.json"
    out.write_text(json.dumps(all_results, indent=2))
    print(f"\n→ saved {out}", flush=True)


if __name__ == "__main__":
    main()
