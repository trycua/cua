#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Stdio MCP server of the `pi-raw-linux` bench arm: one tool, run_script, that runs Python against Linux
low-level APIs (AT-SPI through gi, libX11/libXtst/libXi through ctypes, xdotool) with no Cua Driver in the loop.

Each call: the background-use lint (rawlint.py) reads the script; a blocked script is returned unrun with the rule,
its source and a background-safe alternative. A script that passes runs in a fresh runner process (runner.py) with
restricted builtins and imports, under the call's time limit, while the runtime hook samples the real pointer, the
core focus, the active window and the top of the stacking order. The result carries the return value, printed
output, screenshots as image content, lint warnings, runtime refusals and every pointer/focus change the hook saw.

Every call is appended to RAW_LOG (JSONL) so the bench can count lint blocks and runtime catches per trial.

Environment: RAW_LOG (call log path), RAW_TMP (scratch directory for screenshots), PYTHON (interpreter for the
runner, default this one), RAW_TRIAL (the per-trial JSON the harness writes before each trial: the task's
foreground grant, the per-app AT-SPI coverage labels, and the Cua Driver socket, binary and session for raw.driver).
The trial file is read on every call, so one server process serves every trial of a run.
"""

from __future__ import annotations

import base64
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
TRIAL = Path(os.environ["RAW_TRIAL"]) if os.environ.get("RAW_TRIAL") else None
sys.path.insert(0, str(HERE))
import rawlint  # noqa: E402

RUNNER = HERE / "runner.py"
LOG = Path(os.environ["RAW_LOG"]) if os.environ.get("RAW_LOG") else None
TMP = Path(os.environ.get("RAW_TMP") or tempfile.mkdtemp(prefix="raw-linux-"))
TMP.mkdir(parents=True, exist_ok=True)
PYTHON = os.environ.get("PYTHON") or sys.executable
DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS = 30_000, 120_000

DESCRIPTION = """\
Run one Python script against the Linux desktop. The script body runs inside a function, so `return` a value \
(JSON-able) to see it; print() output is returned too. Each call is a fresh process: nothing persists between calls.

Two routes, in one script if you like:
1. Raw AT-SPI (cheap, background-safe): you find, act on and read back elements yourself.
2. Cua Driver, `raw.driver(tool, args)`: the fallback for apps whose AT-SPI tree does not reach what you need \
(spreadsheet cells, canvas, custom widgets). Same tools and arguments as the Cua Driver MCP tools; the result is \
returned to the script as a dict. GUI tools only: list_windows, list_apps, get_window_state, get_desktop_state, \
get_screen_size, get_cursor_position, zoom, parse_visual_regions, clipboard_read, verify_state, click, \
double_click, right_click, type_text, set_value, press_key, hotkey, scroll, drag, invoke_menu, move_cursor, \
mouse_button_down, mouse_drag, mouse_button_up, set_window_frame, bring_to_front. Typical: \
`s = raw.driver("get_window_state", {"pid": p, "window_id": w}); return s["tree_markdown"]`, then \
`raw.driver("click", {"pid": p, "window_id": w, "element_token": "<snapshot_id>:<row>"})` or \
`raw.driver("type_text", {"pid": p, "window_id": w, "element_token": t, "text": "..."})`. Screenshots are dropped \
from driver results unless you pass `image=True`. A failed driver call raises with the driver's message.

Which route: start with raw AT-SPI for an app labelled "full" below. For an app labelled "partial", use raw where \
its tree has the control, and raw.driver where it does not or where a raw action had no effect. Read back either way.
{coverage}

Raw API (global `raw`, plus `Atspi`):
- raw.Atspi / Atspi: gi.repository.Atspi (AT-SPI 2). Call interface methods in class form on the accessible \
itself (the get_*_iface() getters only tell you whether an interface exists): Atspi.Action.get_n_actions(acc), \
Atspi.Action.get_action_name(acc, i), Atspi.Action.do_action(acc, i); Atspi.EditableText.set_text_contents(acc, s); \
Atspi.Text.get_text(acc, 0, -1); Atspi.Value.get_current_value(acc), Atspi.Value.set_current_value(acc, v); \
Atspi.Component.get_extents(acc, Atspi.CoordType.SCREEN); Atspi.Selection.select_child(acc, i). Tree: \
Atspi.get_desktop(0), acc.get_role_name(), get_name(), get_child_count(), get_child_at_index(i), get_parent(), \
get_state_set().contains(Atspi.StateType.SHOWING).
- raw.app(name=None, pid=None) -> the AT-SPI application; raw.find(root, role=None, name=None, contains=None, \
showing=None) -> list of accessibles (role names as get_role_name() returns them, e.g. "button", "text", \
"spin button", "combo box", "check box", "label", "menu item", "list item", "table cell"; dump a tree to see them); \
raw.describe(acc) -> one line; raw.dump(root, depth=12) -> text tree of showing nodes.
- raw.windows() -> [{window_id, title, pid, frame:(x,y,w,h)}] bottom to top; raw.pointer(); raw.active_window().
- raw.screenshot(window_id=None, region=None) -> the PNG is returned to you as an image.
- raw.x11, raw.xtst, raw.xi: libX11 / libXtst / libXi functions (ctypes calls with int and bytes arguments), raw.dpy \
an open Display*, raw.root the root window. ctypes itself is not importable; use raw.pointer(), raw.windows() and \
raw.active_window() for reads that would need out-parameters. raw.xdotool(*args) runs one xdotool command (an \
argument equal to an xdotool command name counts as a new command, and chains are foreground). raw.sleep(s).

Background contract (checked before the script runs, and again while it runs):
- The user's real pointer, keyboard focus, active window and stacking must stay as they are. Foreground routes: \
XTest motion/buttons/keys, XWarpPointer, Atspi.generate_mouse_event / generate_keyboard_event, xdotool \
mousemove/click/key/type (without --window)/windowactivate/windowfocus/windowraise, XSetInputFocus, XRaiseWindow, \
_NET_ACTIVE_WINDOW, wmctrl, Wayland/portal input, and raw.driver with delivery_mode "foreground", scope "desktop" \
or bring_to_front. Grabs (XGrabKeyboard/Pointer/Key/Button/Server) are always blocked.
- {grant}
- Prefer AT-SPI: Action.do_action for buttons/checks/menu items/combo boxes, EditableText.set_text_contents for \
text, Value.set_current_value for spin buttons and sliders. These need no focus and no pointer.
- Combo boxes: opening one with its "press" action shows a popup that takes the keyboard grab from the user's \
window. Select an item without opening it: Atspi.Action.do_action on the (hidden) menu item, or \
Atspi.Selection.select_child(combo, i).
- Not available: files, processes, network, ctypes and other imports (GUI-only; json, re, math, time, itertools, \
collections, functools, string, textwrap are importable); eval/exec, private attributes, getattr with computed \
names.
- An action the API accepted is not proof it worked: read the element back (get_text, get_current_value, states, \
a screenshot, a driver get_window_state) before you report success.
"""
GRANTED = (
    "This task grants foreground: the routes above are allowed when no background route works (a real hover, a "
    "drag with no accessible action, a tooltip that only shows on hover). Each use is reported."
)
NOT_GRANTED = "This task does not grant foreground: the routes above are blocked. Use a background route."


def trial() -> dict:
    """The harness's per-trial settings (grant, coverage labels, driver), or the safe default: no grant, no driver."""
    if TRIAL is None or not TRIAL.is_file():
        return {"foreground_grant": False, "coverage": {}, "driver": None}
    return json.loads(TRIAL.read_text("utf-8"))


def description(settings: dict) -> str:
    labels = settings.get("coverage") or {}
    coverage = (
        "AT-SPI coverage of this task's apps (measured by the harness): "
        + "; ".join(f"{app}: {label}" for app, label in labels.items())
        + "."
        if labels
        else "AT-SPI coverage of this task's apps: not measured."
    )
    grant = GRANTED if settings.get("foreground_grant") else NOT_GRANTED
    return DESCRIPTION.replace("{coverage}", coverage).replace("{grant}", grant)


SCHEMA = {
    "type": "object",
    "required": ["script"],
    "properties": {
        "script": {
            "type": "string",
            "description": "Python statements (a function body; `return` a value).",
        },
        "timeout_ms": {
            "type": "integer",
            "description": f"Wall-time limit, default {DEFAULT_TIMEOUT_MS}, max {MAX_TIMEOUT_MS}.",
        },
    },
    "additionalProperties": False,
}


def _log(entry: dict) -> None:
    if LOG is None:
        return
    with LOG.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry) + "\n")


def runner_env(settings: dict) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in {"RAW_LOG", "RAW_TRIAL", "RAW_GRANT"}}
    env["RAW_GRANT"] = "1" if settings.get("foreground_grant") is True else "0"
    driver = settings.get("driver") or {}
    for key, name in (("socket", "RAW_DRIVER_SOCKET"), ("bin", "RAW_DRIVER_BIN"), ("session", "RAW_DRIVER_SESSION")):
        if driver.get(key):
            env[name] = str(driver[key])
    return env


def run_script(script: str, timeout_ms: int | None) -> tuple[list[dict], bool, dict]:
    settings = trial()
    limit = max(1_000, min(int(timeout_ms or DEFAULT_TIMEOUT_MS), MAX_TIMEOUT_MS)) / 1000
    shots = Path(tempfile.mkdtemp(prefix="call-", dir=TMP))
    started = time.monotonic()
    try:
        done = subprocess.run(
            [PYTHON, "-I", "-B", str(RUNNER), str(shots)],
            input=script,
            capture_output=True,
            text=True,
            timeout=limit,
            env=runner_env(settings),
        )
        line = (done.stdout.strip().splitlines() or [""])[-1]
        result = (
            json.loads(line)
            if line.startswith("{")
            else {
                "ok": False,
                "error": f"runner exited {done.returncode}: {done.stderr.strip()[-1500:]}",
            }
        )
    except subprocess.TimeoutExpired:
        result = {
            "ok": False,
            "error": f"the script did not finish within {limit:.0f} s and was killed",
        }
    elapsed = round(time.monotonic() - started, 2)
    text: list[str] = []
    if result.get("lint_text"):
        text.append(result["lint_text"])
    granted = bool(settings.get("foreground_grant"))
    watch = result.get("watch") or {}
    if watch.get("lines"):
        text.append(
            "Runtime background check: "
            + "; ".join(watch["lines"])
            + (
                " (this task grants foreground)."
                if granted
                else ". This task does not grant foreground: if your script caused this, use a background route "
                "(do_action, set_text_contents, set_current_value, or raw.driver with default delivery)."
            )
        )
    for item in result.get("runtime_blocked") or []:
        text.append(f"Refused at run time: {item}")
    if result.get("ok"):
        if result.get("stdout"):
            text.append("stdout:\n" + result["stdout"])
        text.append("return: " + json.dumps(result.get("value"), ensure_ascii=False)[:20000])
    else:
        if result.get("stdout"):
            text.append("stdout:\n" + result["stdout"])
        text.append("error: " + str(result.get("error")))
    content = [{"type": "text", "text": "\n\n".join(text)}]
    for path in result.get("images") or []:
        try:
            data = Path(path).read_bytes()
        except OSError:
            continue
        content.append(
            {"type": "image", "data": base64.b64encode(data).decode(), "mimeType": "image/png"}
        )
    entry = {
        "t": time.time(),
        "elapsed_s": elapsed,
        "ok": bool(result.get("ok")),
        "lint_blocked": bool(result.get("lint_blocked")),
        "lint": result.get("lint") or [],
        "granted": granted,
        "foreground_used": bool(
            granted
            and (
                any(f.get("severity") == "granted" for f in result.get("lint") or [])
                or any(c.get("foreground") for c in result.get("driver_calls") or [])
            )
        ),
        "driver_calls": result.get("driver_calls") or [],
        "raw_actions": result.get("raw_actions") or {},
        "runtime_blocked": result.get("runtime_blocked") or [],
        "runtime_warned": result.get("runtime_warned") or [],
        "watch_changed": watch.get("changed") or [],
        "watch_lines": watch.get("lines") or [],
        "error": None if result.get("ok") else str(result.get("error"))[:500],
        "script_chars": len(script),
    }
    return content, not result.get("ok"), entry


def serve() -> None:
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        mid, method = message.get("id"), message.get("method")

        def send(payload: dict) -> None:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, **payload}) + "\n")
            sys.stdout.flush()

        if method == "initialize":
            send(
                {
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "cua", "version": "raw-linux-prototype"},
                        "instructions": "One tool, run_script: Python against Linux low-level APIs. "
                        "Read its description first.",
                    }
                }
            )
        elif method == "tools/list":
            send(
                {
                    "result": {
                        "tools": [
                            {
                                "name": "run_script",
                                "description": description(trial()),
                                "inputSchema": SCHEMA,
                            }
                        ]
                    }
                }
            )
        elif method == "tools/call":
            params = message.get("params") or {}
            args = params.get("arguments") or {}
            if params.get("name") != "run_script" or not isinstance(args.get("script"), str):
                send(
                    {
                        "result": {
                            "content": [{"type": "text", "text": "unknown tool or missing script"}],
                            "isError": True,
                        }
                    }
                )
                continue
            content, failed, entry = run_script(args["script"], args.get("timeout_ms"))
            _log(entry)
            send({"result": {"content": content, "isError": failed}})
        elif mid is not None:
            send({"result": {}})


if __name__ == "__main__":
    serve()


__all__ = ["DESCRIPTION", "rawlint", "run_script"]
