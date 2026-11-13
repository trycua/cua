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
foreground grant and the per-app AT-SPI coverage labels). The trial file is read on every call, so one server
process serves every trial of a run. RAW_ATSPI_TIMEOUT_MS passes through to the runner (raw.py).
"""

from __future__ import annotations

import base64
import json
import os
import re
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
# The no-wrapper arm (pi-raw-linux-native): scripts call Linux's own bindings directly, with no raw.* runtime
NATIVE = os.environ.get("RAW_NATIVE") == "1"
# The bare arm (pi-raw-linux-bare, RAW_BARE=1 with RAW_NATIVE=1): the native runtime and the same checks, but the model
# sees only BARE_DESCRIPTION; lint and runtime messages name the rule and nothing else.
BARE = NATIVE and os.environ.get("RAW_BARE") == "1"
# The docs arm (pi-raw-linux-docs, RAW_DOCS=<index.jsonl> with the bare settings): the bare arm plus one lookup tool
# over the generated API docs (gen_docs.py). Its description is the one line below; nothing else changes.
DOCS = Path(os.environ["RAW_DOCS"]) if BARE and os.environ.get("RAW_DOCS") else None
LOOKUP_DESCRIPTION = "Search the API documentation of this desktop's native libraries: a query in, matching entries out."
LOOKUP_SCHEMA = {
    "type": "object",
    "required": ["query"],
    "properties": {
        "query": {"type": "string"},
        "limit": {"type": "integer"},
    },
    "additionalProperties": False,
}
# pids whose AT-SPI calls timed out in an earlier script, per trial (raw.app asks them with a short timeout)
STALLED: dict[str, set[str]] = {}

DESCRIPTION = """\
Run one Python script against the Linux desktop. The script body runs inside a function, so `return` a value \
(JSON-able) to see it; print() output is returned too. Each call is a fresh process: nothing persists between calls.

Only Linux's own APIs and tools: AT-SPI, X11/XTest and xdotool. There is no Cua Driver (rule DRV1). Start with \
AT-SPI (cheap, background-safe): find, act on and read back elements yourself. Where an app's tree does not reach \
what you need (spreadsheet cells, ledger registers, a drag), use the foreground helpers below if this task grants \
foreground. Read back either way.
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
- raw.cells(table_or_app, "A1:F20") -> rows of cell text through the AT-SPI Table interface (LibreOffice Calc: \
pass raw.app("soffice") or its [table]; one cell "B7" works too); raw.cell_name(row, col) -> "B7". Read-only.
- Foreground helpers (only on a task that grants foreground; each activates the window, confirms it is active, \
sends real XTest input through xdotool; when the script ends the previous active window is given back unless a \
popup is open, so do a whole edit in one script; \
coordinates are window coordinates, as in raw.screenshot(window_id), unless screen=True):
  raw.activate(window_id); raw.key(window_id, "ctrl+Home", "Return", repeat=1) (xdotool key names: ctrl+s, \
shift+Tab, Down, F2, Escape); raw.type_text(window_id, text) ("\\t" is Tab, "\\n" is Return); \
raw.click(window_id, x, y, button=1, count=1); raw.scroll(window_id, x, y, clicks); \
raw.drag(x1, y1, x2, y2, window_id=None, steps=20, duration=0.6, hold=0.3) (a real drag-and-drop).
  LibreOffice Calc: go to a cell through the Name Box (raw.key(w, "ctrl+shift+F5"); raw.type_text(w, "B7\\n")), \
then raw.type_text(w, "value\\tnext\\n") fills across; save with raw.key(w, "ctrl+s"); read back with raw.cells(raw.app("soffice"), "A1:F20").
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
_NET_ACTIVE_WINDOW, wmctrl, Wayland/portal input, and the foreground helpers (raw.activate, raw.key, \
raw.type_text, raw.click, raw.scroll, raw.drag). Grabs (XGrabKeyboard/Pointer/Key/Button/Server) are always blocked.
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
a screenshot, raw.cells) before you report success.
"""
NATIVE_DESCRIPTION = """\
Run one Python script against the Linux desktop (X11). The script body runs inside a function, so `return` a value \
(JSON-able) to see it; print() output is returned too. Each call is a fresh process: nothing persists between calls.

Call Linux's own APIs directly; there is no helper library and no Cua Driver (rule DRV1). Import what you need:
- AT-SPI 2: `import gi; gi.require_version("Atspi", "2.0"); from gi.repository import Atspi` (or `import pyatspi`). \
Tree: Atspi.get_desktop(0), acc.get_child_count(), get_child_at_index(i), get_role_name(), get_name(), \
get_process_id(), get_parent(), get_state_set().contains(Atspi.StateType.SHOWING). Call interface methods in class \
form on the accessible itself: Atspi.Action.get_n_actions(acc), get_action_name(acc, i), do_action(acc, i); \
Atspi.EditableText.set_text_contents(acc, s); Atspi.Text.get_text(acc, 0, -1); Atspi.Value.get_current_value(acc), \
set_current_value(acc, v); Atspi.Component.get_extents(acc, Atspi.CoordType.SCREEN); \
Atspi.Selection.select_child(acc, i); Atspi.Table.get_n_rows(t), get_n_columns(t), get_accessible_at(t, row, col). \
A spreadsheet's table reports every cell of the sheet as a child (LibreOffice: over a billion), so address cells \
by row and column, never by walking all children.
- X11: python-xlib (`from Xlib import display, X, Xatom, XK; from Xlib.ext import xtest`; d = display.Display(); \
root = d.screen().root; _NET_CLIENT_LIST, _NET_WM_NAME, _NET_WM_PID through get_full_property; \
xtest.fake_input(d, X.KeyPress, keycode) / X.ButtonPress / X.MotionNotify, then d.sync()), or ctypes for libX11, \
libXtst or libxdo (ctypes.CDLL("libXtst.so.6")).
- Gdk 3 (`gi.require_version("Gdk", "3.0")`) for screenshots: Gdk.pixbuf_get_from_window(Gdk.get_default_root_window(), \
x, y, w, h).savev(SHOTS + "/a.png", "png", [], []). Every PNG written into the directory named by the global SHOTS \
comes back to you as an image.
- D-Bus: `import dbus` (session bus reads; the AT-SPI bus is reachable through Atspi).
- The xdotool CLI: the global `xdotool(*args)` runs one xdotool command and returns its stdout (an argument equal \
to an xdotool command name counts as a new command, and chains are foreground). xdotool search, getwindowname, \
getwindowgeometry, getactivewindow are reads.
- Also importable: json, re, math, time, itertools, collections, functools, string, textwrap, dataclasses, typing, \
enum, and from gi.repository only Atspi, Gdk, GdkPixbuf, GdkX11, GLib and GObject.
{coverage}

Start with AT-SPI (cheap, background-safe): find, act on and read back elements yourself. Where an app's tree does \
not reach what you need (spreadsheet cell entry, ledger registers, a drag), send real input if this task grants \
foreground: activate the target window, confirm it is active and holds the input focus, then send XTest or xdotool \
input; keys go to whatever window holds the focus. Give the active window back to the one that had it when you \
are done. LibreOffice Calc: the Name Box (ctrl+shift+F5, type "B7", Return) moves to a cell; typed text with Tab \
and Return fills across; ctrl+s saves. Read back either way.

Background contract (checked before the script runs, and again while it runs):
- The user's real pointer, keyboard focus, active window and stacking must stay as they are. Foreground routes: \
XTest motion/buttons/keys (python-xlib xtest.fake_input, XTestFake*), warp_pointer / XWarpPointer, Gdk warp and \
test_simulate_*, Atspi.generate_mouse_event / generate_keyboard_event, xdotool mousemove/click/key/type (without \
--window)/windowactivate/windowfocus/windowraise, set_input_focus / XSetInputFocus, raise_window / XRaiseWindow, \
configure(stack_mode=...), _NET_ACTIVE_WINDOW, wmctrl, libxdo's input and activation calls, Wayland/portal input. \
Grabs (grab_pointer/keyboard/key/button/server, XGrab*) are always blocked.
- {grant}
- Prefer AT-SPI: Action.do_action for buttons/checks/menu items/combo boxes, EditableText.set_text_contents for \
text, Value.set_current_value for spin buttons and sliders. These need no focus and no pointer.
- Combo boxes: opening one with its "press" action shows a popup that takes the keyboard grab from the user's \
window. Select an item without opening it: Atspi.Action.do_action on the (hidden) menu item, or \
Atspi.Selection.select_child(combo, i).
- Not available: files, processes, shells, network, ctypes.pythonapi, Gio/Gtk, GLib spawn and file calls, \
eval/exec, dunder attributes, getattr with computed names, Cua Driver in any form.
- An action the API accepted is not proof it worked: read the element back (get_text, get_current_value, states, \
a screenshot) before you report success.
"""
BARE_DESCRIPTION = (
    "Run a Python script on this Linux (X11) desktop and get back its return value, printed output and any PNG it "
    "saves into the directory named by the global SHOTS. Each call is a fresh process. Linux's native desktop APIs "
    "are importable (gi.repository Atspi/Gdk/GdkPixbuf/GdkX11/GLib/GObject, pyatspi, python-xlib with XTest, dbus, "
    "ctypes for the X11/XTest/xdo/AT-SPI libraries), and the global xdotool(*args) runs the xdotool CLI. Calls the "
    "harness does not allow come back as an error."
)
GRANTED = (
    "This task grants foreground: the routes above are allowed when no background route works (a real hover, a "
    "drag with no accessible action, a tooltip that only shows on hover, keys into a spreadsheet or ledger cell). "
    "Each use is reported."
)
NOT_GRANTED = "This task does not grant foreground: the routes above are blocked. Use a background route."


def trial() -> dict:
    """The harness's per-trial settings (grant, coverage labels), or the safe default: no grant."""
    if TRIAL is None or not TRIAL.is_file():
        return {"foreground_grant": False, "coverage": {}}
    return json.loads(TRIAL.read_text("utf-8"))


def description(settings: dict) -> str:
    if BARE:
        return BARE_DESCRIPTION
    labels = settings.get("coverage") or {}
    coverage = (
        "AT-SPI coverage of this task's apps (measured by the harness): "
        + "; ".join(f"{app}: {label}" for app, label in labels.items())
        + "."
        if labels
        else "AT-SPI coverage of this task's apps: not measured."
    )
    grant = GRANTED if settings.get("foreground_grant") else NOT_GRANTED
    if NATIVE:
        grant = grant.replace("the routes above", "the foreground routes above")
        return NATIVE_DESCRIPTION.replace("{coverage}", coverage).replace("{grant}", grant)
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
    env = {
        k: v
        for k, v in os.environ.items()
        if k not in {"RAW_LOG", "RAW_TRIAL", "RAW_GRANT", "RAW_ATSPI_SUSPECT"} and not k.startswith("RAW_DRIVER_")
    }
    env["RAW_GRANT"] = "1" if settings.get("foreground_grant") is True else "0"
    stalled = STALLED.get(str(settings.get("trial_id")))
    if stalled:
        env["RAW_ATSPI_SUSPECT"] = ",".join(sorted(stalled))
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
    if result.get("atspi_stalled"):
        STALLED.setdefault(str(settings.get("trial_id")), set()).update(result["atspi_stalled"])
    text: list[str] = []
    if result.get("lint_text"):
        text.append(result["lint_text"])
    granted = bool(settings.get("foreground_grant"))
    watch = result.get("watch") or {}
    if watch.get("lines") and not BARE:  # the bare arm gets no hints; the watch is still logged
        text.append(
            "Runtime background check: "
            + "; ".join(watch["lines"])
            + (
                " (this task grants foreground)."
                if granted
                else ". This task does not grant foreground: if your script caused this, use a background route "
                "(do_action, set_text_contents, set_current_value)."
            )
        )
    for item in result.get("runtime_blocked") or []:  # the call log keeps the detail either way
        text.append(f"Refused at run time: {str(item).split(':')[0] if BARE else item}")
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
            granted and any(f.get("severity") == "granted" for f in result.get("lint") or [])
        ),
        "raw_actions": result.get("raw_actions") or {},
        "routes": result.get("routes") or {},
        "atspi_stalled": result.get("atspi_stalled") or {},
        "runtime_blocked": result.get("runtime_blocked") or [],
        "runtime_warned": result.get("runtime_warned") or [],
        "watch_changed": watch.get("changed") or [],
        "watch_lines": watch.get("lines") or [],
        "error": None if result.get("ok") else str(result.get("error"))[:500],
        "script_chars": len(script),
    }
    return content, not result.get("ok"), entry


_INDEX: list[tuple[dict, str, str]] | None = None
WORD = re.compile(r"[A-Za-z0-9_]+")


def _split(word: str) -> list[str]:
    """A symbol's searchable parts: the whole, its snake/camel pieces (XTestFakeKeyEvent -> xtest fake key event)."""
    parts = re.findall(r"[A-Z]+(?![a-z])|[A-Z]?[a-z]+|[0-9]+", word)
    return [word.lower(), *(p.lower() for p in parts if len(p) > 1)]


def index() -> list[tuple[dict, str, str]]:
    """The docs index, loaded once: (entry, name text, body text), in the file's order."""
    global _INDEX
    if _INDEX is None:
        _INDEX = []
        for line in DOCS.read_text("utf-8").splitlines():
            e = json.loads(line)
            name = " ".join(t for w in WORD.findall(e["id"]) for t in _split(w))
            body = " ".join(t for w in WORD.findall(e["sig"] + " " + e["doc"]) for t in _split(w))
            _INDEX.append((e, f" {name} ", f" {body} "))
    return _INDEX


def lookup(query: str, limit: int | None) -> tuple[list[dict], bool, dict]:
    """Rank entries by query terms in the entry's name (weight 3) and its signature and doc (1); ties keep index
    order. A term that is an exact symbol name scores highest."""
    started = time.monotonic()
    try:
        n = max(1, min(int(limit or 10), 30))
    except (TypeError, ValueError, OverflowError):
        n = 10  # a malformed limit falls back to the default rather than failing the call
    terms = list(dict.fromkeys(t for w in WORD.findall(query) for t in _split(w)))
    exact = {w.lower() for w in WORD.findall(query)}
    scored = []
    for pos, (e, name, body) in enumerate(index() if terms else []):
        last = e["id"].rsplit(".", 1)[-1].rsplit(":", 1)[-1].lower()
        score = 10 * (last in exact) + sum(3 * (f" {t} " in name) + (f" {t} " in body) for t in terms)
        matched = sum((f" {t} " in name) or (f" {t} " in body) for t in terms)
        if score:
            scored.append((-matched, -score, pos, e))
    scored.sort(key=lambda x: x[:3])
    hits = [x[3] for x in scored[:n]]
    out = [f"{len(scored)} matching entries" + (f", top {len(hits)}:" if hits else ".")]
    for e in hits:
        out.append(f"[{e['kind']}] {e['sig']}\n  source: {e['source']}" + (f"\n  {e['doc']}" if e["doc"] else ""))
    entry = {"t": time.time(), "tool": "lookup", "query": query[:500], "hits": len(scored),
             "returned": [e["id"] for e in hits], "elapsed_s": round(time.monotonic() - started, 3)}
    return [{"type": "text", "text": "\n\n".join(out)}], False, entry


def tools_list() -> list[dict]:
    tools = [{"name": "run_script", "description": description(trial()), "inputSchema": SCHEMA}]
    if DOCS is not None:
        tools.append({"name": "lookup", "description": LOOKUP_DESCRIPTION, "inputSchema": LOOKUP_SCHEMA})
    return tools


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
                        "instructions": "Two tools, run_script and lookup."
                        if DOCS is not None
                        else "One tool, run_script."
                        if BARE
                        else "One tool, run_script: Python against Linux low-level APIs. Read its description first.",
                    }
                }
            )
        elif method == "tools/list":
            send(
                {
                    "result": {
                        "tools": tools_list()
                    }
                }
            )
        elif method == "tools/call":
            params = message.get("params") or {}
            args = params.get("arguments") or {}
            if DOCS is not None and params.get("name") == "lookup" and isinstance(args.get("query"), str):
                content, failed, entry = lookup(args["query"], args.get("limit"))
                _log(entry)
                send({"result": {"content": content, "isError": failed}})
                continue
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
