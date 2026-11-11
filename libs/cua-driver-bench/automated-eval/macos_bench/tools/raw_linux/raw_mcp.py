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
runner, default this one).
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
sys.path.insert(0, str(HERE))
import rawlint  # noqa: E402

RUNNER = HERE / "runner.py"
LOG = Path(os.environ["RAW_LOG"]) if os.environ.get("RAW_LOG") else None
TMP = Path(os.environ.get("RAW_TMP") or tempfile.mkdtemp(prefix="raw-linux-"))
PYTHON = os.environ.get("PYTHON") or sys.executable
DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS = 30_000, 120_000

DESCRIPTION = """\
Run one Python script against the Linux desktop's low-level APIs. There is no higher-level driver: you find, act \
on and read back elements yourself. The script body runs inside a function, so `return` a value (JSON-able) to see \
it; print() output is returned too. Each call is a fresh process: nothing persists between calls.

Available (global `raw`, plus `Atspi`):
- raw.Atspi / Atspi: gi.repository.Atspi (AT-SPI 2). Useful: Atspi.get_desktop(0) -> apps; acc.get_role_name(), \
get_name(), get_child_count(), get_child_at_index(i), get_state_set().contains(Atspi.StateType.X); \
acc.get_action_iface() -> get_n_actions(), get_action_name(i), do_action(i); acc.get_editable_text_iface() -> \
set_text_contents(s); acc.get_text_iface() -> get_text(0, -1); acc.get_value_iface() -> get_current_value(), \
set_current_value(v); acc.get_component_iface() -> get_extents(Atspi.CoordType.SCREEN); \
acc.get_selection_iface() -> select_child(i).
- raw.app(name=None, pid=None) -> the AT-SPI application; raw.find(root, role=None, name=None, contains=None, \
showing=None) -> list of accessibles (role names like "push button", "text", "spin button", "combo box", \
"check box", "label", "menu item", "list item", "table cell"); raw.describe(acc) -> one line; raw.dump(root, \
depth=12) -> text tree of showing nodes.
- raw.windows() -> [{window_id, title, pid, frame:(x,y,w,h)}] bottom to top; raw.pointer(); raw.active_window().
- raw.screenshot(window_id=None, region=None) -> the PNG is returned to you as an image.
- raw.x11, raw.xtst, raw.xi: libX11 / libXtst / libXi through ctypes, raw.dpy an open Display*, raw.root the root \
window. raw.xdotool(*args) runs xdotool. raw.sleep(s).

Background contract (checked before the script runs, and again while it runs):
- The user's real pointer, keyboard focus, active window and stacking must stay as they are. Blocked: XTest motion/\
buttons/keys, XWarpPointer, Atspi.generate_mouse_event / generate_keyboard_event, xdotool mousemove/click/key/type \
(without --window)/windowactivate/windowfocus/windowraise, XSetInputFocus, XRaiseWindow, _NET_ACTIVE_WINDOW, \
wmctrl, Wayland/portal input. Grabs (XGrabKeyboard/Pointer/Key/Button/Server) are always blocked.
- Prefer AT-SPI: do_action for buttons/checks/menu items, set_text_contents for text, set_current_value for spin \
buttons and sliders. These need no focus and no pointer.
- When a task truly needs the real pointer or keyboard (e.g. a hover-only control, a drag with no accessible \
action, a tooltip that only shows on hover) and no accessible route works, put \
`raw.escalate_foreground("<why the background route is unavailable>")` at the top of that script. The lint then \
allows the pointer/focus/key calls in that script only, and the result reports the escalation.
- Not available: files, processes, network, other imports (GUI-only); eval/exec/getattr with computed names.
- An action the API accepted is not proof it worked: read the element back (get_text, get_current_value, states, \
a screenshot) before you report success.
"""

SCHEMA = {
    "type": "object",
    "required": ["script"],
    "properties": {
        "script": {"type": "string", "description": "Python statements (a function body; `return` a value)."},
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


def run_script(script: str, timeout_ms: int | None) -> tuple[list[dict], bool, dict]:
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
            env={k: v for k, v in os.environ.items() if k not in {"RAW_LOG"}},
        )
        line = (done.stdout.strip().splitlines() or [""])[-1]
        result = json.loads(line) if line.startswith("{") else {
            "ok": False,
            "error": f"runner exited {done.returncode}: {done.stderr.strip()[-1500:]}",
        }
    except subprocess.TimeoutExpired:
        result = {"ok": False, "error": f"the script did not finish within {limit:.0f} s and was killed"}
    elapsed = round(time.monotonic() - started, 2)
    text: list[str] = []
    if result.get("lint_text"):
        text.append(result["lint_text"])
    if result.get("escalated"):
        text.append(f'Foreground escalation in effect: "{result["escalated"]}".')
    watch = result.get("watch") or {}
    if watch.get("lines"):
        text.append(
            "Runtime background check: "
            + "; ".join(watch["lines"])
            + (
                "."
                if result.get("escalated")
                else ". This script did not declare a foreground escalation: the user's desktop was disturbed. "
                "Use an AT-SPI route (do_action, set_text_contents, set_current_value) instead."
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
        content.append({"type": "image", "data": base64.b64encode(data).decode(), "mimeType": "image/png"})
    entry = {
        "t": time.time(),
        "elapsed_s": elapsed,
        "ok": bool(result.get("ok")),
        "lint_blocked": bool(result.get("lint_blocked")),
        "lint": result.get("lint") or [],
        "escalated": result.get("escalated"),
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
            send({"result": {"tools": [{"name": "run_script", "description": DESCRIPTION, "inputSchema": SCHEMA}]}})
        elif method == "tools/call":
            params = message.get("params") or {}
            args = params.get("arguments") or {}
            if params.get("name") != "run_script" or not isinstance(args.get("script"), str):
                send({"result": {"content": [{"type": "text", "text": "unknown tool or missing script"}], "isError": True}})
                continue
            content, failed, entry = run_script(args["script"], args.get("timeout_ms"))
            _log(entry)
            send({"result": {"content": content, "isError": failed}})
        elif mid is not None:
            send({"result": {}})


if __name__ == "__main__":
    serve()


__all__ = ["DESCRIPTION", "rawlint", "run_script"]
