#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Stdio MCP server of the `pi-raw-macos` bench arm: one tool, run_script, that runs Python against the macOS
Accessibility API and per-process event posting, with Cua Driver only as the fallback a script calls itself.

Each call: the background-use lint (rawlint_mac.py) reads the script; a blocked script is returned unrun with the
rule, its source and a background-safe alternative. A script that passes runs in a fresh runner process
(runner_mac.py) with restricted builtins and imports, under the call's time limit, while the runtime hook samples the
frontmost app, the key window, the focused element, the top window and the real cursor. The result carries the return
value, printed output, screenshots as image content, lint warnings, runtime refusals and every change the hook saw.

The runner is spawned as its own TCC-responsible process (posix_spawn with responsibility_spawnattrs_setdisclaim, as
Cua Driver's own re-exec does), so the Accessibility, Screen Recording and PostEvent grants of the runner's Python
binary apply, not those of whatever started the agent.

Environment: RAW_LOG (call log path), RAW_TMP (scratch directory for screenshots), PYTHON (interpreter for the
runner), RAW_TRIAL (the per-trial JSON the harness writes before each trial: the foreground grant, the per-app AX
coverage labels, and the Cua Driver socket, binary and session for raw.driver). The trial file is read on every call.
"""

from __future__ import annotations

import base64
import ctypes
import json
import os
import signal
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import rawlint_mac as rawlint  # noqa: E402

RUNNER = HERE / "runner_mac.py"
LOG = Path(os.environ["RAW_LOG"]) if os.environ.get("RAW_LOG") else None
TRIAL = Path(os.environ["RAW_TRIAL"]) if os.environ.get("RAW_TRIAL") else None
TMP = Path(os.environ.get("RAW_TMP") or tempfile.mkdtemp(prefix="raw-macos-"))
TMP.mkdir(parents=True, exist_ok=True)
PYTHON = os.environ.get("PYTHON") or sys.executable
DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS = 30_000, 120_000

DESCRIPTION = """\
Run one Python script against the macOS desktop. The script body runs inside a function, so `return` a value \
(JSON-able) to see it; print() output is returned too. Each call is a fresh process: nothing persists between calls.

Two routes, in one script if you like:
1. Raw macOS APIs (cheap, background-safe): the Accessibility API and events posted to one process. You find, act \
on and read back elements yourself.
2. Cua Driver, `raw.driver(tool, args)`: the fallback for apps whose AX tree does not reach what you need \
(spreadsheet cells, canvas, web content that ignores AX actions). Same tools and arguments as the Cua Driver MCP \
tools; the result is returned to the script as a dict. GUI tools only: list_windows, list_apps, get_window_state, \
get_desktop_state, get_screen_size, get_cursor_position, zoom, parse_visual_regions, clipboard_read, verify_state, \
click, double_click, right_click, type_text, set_value, press_key, hotkey, scroll, drag, invoke_menu, move_cursor, \
mouse_button_down, mouse_drag, mouse_button_up, set_window_frame, bring_to_front. Typical: \
`s = raw.driver("get_window_state", {"pid": p, "window_id": w}); return s["tree_markdown"]`, then \
`raw.driver("click", {"pid": p, "window_id": w, "element_token": "<token from the tree>"})` or \
`raw.driver("type_text", {"pid": p, "window_id": w, "element_token": t, "text": "..."})`. `image=True` returns the \
driver's screenshot to you. A failed driver call raises with the driver's message.

Which route: start with raw AX for an app labelled "full" below. For an app labelled "partial", use raw where its \
tree has the control, and raw.driver where it does not or where a raw action had no effect. Read back either way.
{coverage}

Raw API (global `raw`):
- AX: raw.ax.app(pid) -> the app element; raw.ax.get(el, "AXValue"), raw.ax.names(el), raw.ax.actions(el), \
raw.ax.children(el), raw.ax.settable(el, attr), raw.ax.at(x, y) (element at a screen point), raw.ax.pid(el). \
Act: raw.ax.perform(el, "AXPress") (also AXIncrement, AXDecrement, AXConfirm, AXCancel, AXPick, AXShowAlternateUI); \
raw.ax.set(el, "AXValue", "text") for text fields, combo boxes and sliders, raw.ax.set(el, "AXSelected", True) for \
rows and tabs. Both return the AX error code (0 = accepted). Useful attributes: AXRole, AXSubrole, AXTitle, \
AXDescription, AXValue, AXHelp (tooltip text), AXEnabled, AXFocused, AXPosition, AXSize, AXWindows, AXMainWindow, \
AXChildren, AXRows, AXSelectedRows, AXSheets, AXMenuBar.
- raw.find(root, role=None, title=None, contains=None) -> elements whose AXRole matches ("AXButton", \
"AXTextField", "AXPopUpButton", "AXCheckBox", "AXMenuItem", "AXRow", "AXStaticText", "AXSheet", "AXWindow") and \
whose title, description, value, identifier or help equals `title` or contains `contains`; raw.describe(el) -> one \
line; raw.dump(root, depth=14) -> text tree.
- raw.apps() -> [{pid, name, bundle_id, active, hidden}]; raw.windows(pid=None) -> on-screen windows front to back \
[{window_id, pid, app, title, layer, frame:(x,y,w,h)}] (layer 0 = normal windows); raw.front() -> the frontmost app \
and its key window; raw.cursor().
- raw.screenshot(window_id=None, region=None) -> the PNG is returned to you as an image (a window is captured even \
when it is behind others).
- Events to one process (the real cursor does not move): raw.post_click(pid, x, y, button="left", count=1), \
raw.post_key(pid, ["cmd", "s"]) or raw.post_key(pid, "return"), raw.post_text(pid, "text") (to that app's focused \
field), raw.post_scroll(pid, x, y, dy). Screen coordinates, top-left origin. Many apps ignore posted clicks while \
in the background: prefer AX actions and read back.
- Foreground pointer (the real cursor moves): raw.pointer_move(x, y), raw.pointer_click(x, y), \
raw.pointer_drag(x0, y0, x1, y1). raw.sleep(s).

Background contract (checked before the script runs, and again while it runs):
- The user's frontmost app, key window, focused element, window order and real cursor must stay as they are. \
Foreground routes: raw.pointer_*, AXRaise, setting AXFrontmost / AXMain / AXFocused / AXFocusedUIElement / \
AXMinimized / AXHidden, and raw.driver with delivery_mode "foreground", a desktop scope or target, or bring_to_front.
- {grant}
- Always blocked: launching or opening apps (open, NSWorkspace, AppleScript, osascript), shells, files, network, \
pyobjc and framework imports, private window-server calls.
- Menus are foreground too: AXShowMenu, and AXPress on an AXPopUpButton, AXMenuButton or AXMenuBarItem, open a \
menu that takes key focus from the user's window and lifts the target window. An AppKit pop-up lists its items \
only while open and refuses an AXValue write, so choose an item with \
`raw.driver("set_value", {"pid": p, "window_id": w, "element_token": t, "value": "High"})` (the driver opens it \
under its key-focus guard and hands focus back), and run menu-bar commands with `raw.driver("invoke_menu", ...)`. \
Get the token from `raw.driver("get_window_state", {"pid": p, "window_id": w})`: its tree rows read \
`[3] AXPopUpButton = "Low" (Priority)` and the token is `"<snapshot_id>:3"`.
- Warned: cmd+L, cmd+shift+G and tab-switching shortcuts make apps activate themselves; clipboard writes replace \
what the user copied.
- Not available: files, processes, network and other imports (json, re, math, time, itertools, collections, \
functools, string, textwrap are importable); eval/exec, private attributes, getattr with computed names.
- An action the API accepted is not proof it worked: read the element back (raw.ax.get, raw.describe, a \
screenshot, a driver get_window_state) before you report success.
"""
GRANTED = (
    "This trial grants foreground: the routes above are allowed when no background route works (a real hover, a "
    "drag-and-drop with no accessible action, a tooltip that only shows on hover). Each use is reported."
)
NOT_GRANTED = (
    "This trial does not grant foreground: the routes above are blocked. Use a background route."
)


def trial() -> dict:
    """The harness's per-trial settings (grant, coverage labels, driver), or the safe default: no grant, no driver."""
    if TRIAL is None or not TRIAL.is_file():
        return {"foreground_grant": False, "coverage": {}, "driver": None}
    return json.loads(TRIAL.read_text("utf-8"))


def description(settings: dict) -> str:
    labels = settings.get("coverage") or {}
    coverage = (
        "AX coverage of this task's apps (the harness's labels): "
        + "; ".join(f"{app}: {label}" for app, label in labels.items())
        + "."
        if labels
        else "AX coverage of this task's apps: not measured."
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
    for key, name in (
        ("socket", "RAW_DRIVER_SOCKET"),
        ("bin", "RAW_DRIVER_BIN"),
        ("session", "RAW_DRIVER_SESSION"),
    ):
        if driver.get(key):
            env[name] = str(driver[key])
    env["RAW_USER_APPS"] = ",".join(settings.get("user_apps") or [])
    return env


_LIBC = ctypes.CDLL(None, use_errno=True)


def spawn_disclaimed(
    argv: list[str], env: dict[str, str], stdin: Path, stdout: Path, stderr: Path
) -> int:
    """posix_spawn ``argv`` as its own TCC-responsible process with stdin/stdout/stderr from these files."""
    attr = ctypes.c_void_p()
    actions = ctypes.c_void_p()
    if _LIBC.posix_spawnattr_init(ctypes.byref(attr)) or _LIBC.posix_spawn_file_actions_init(
        ctypes.byref(actions)
    ):
        raise OSError("posix_spawn init failed")
    try:
        if _LIBC.responsibility_spawnattrs_setdisclaim(ctypes.byref(attr), 1):
            raise OSError("responsibility_spawnattrs_setdisclaim failed")
        # the child becomes its own process group, so a timeout kills the whole tree
        _LIBC.posix_spawnattr_setflags(
            ctypes.byref(attr), ctypes.c_short(0x0002)
        )  # POSIX_SPAWN_SETPGROUP
        _LIBC.posix_spawnattr_setpgroup(ctypes.byref(attr), 0)
        fds = [
            os.open(stdin, os.O_RDONLY),
            os.open(stdout, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600),
            os.open(stderr, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600),
        ]
        try:
            for target, fd in enumerate(fds):
                _LIBC.posix_spawn_file_actions_adddup2(ctypes.byref(actions), fd, target)
            c_argv = (ctypes.c_char_p * (len(argv) + 1))(*[a.encode() for a in argv], None)
            pairs = [f"{k}={v}".encode() for k, v in env.items()]
            c_env = (ctypes.c_char_p * (len(pairs) + 1))(*pairs, None)
            pid = ctypes.c_int()
            rc = _LIBC.posix_spawn(
                ctypes.byref(pid),
                argv[0].encode(),
                ctypes.byref(actions),
                ctypes.byref(attr),
                c_argv,
                c_env,
            )
            if rc:
                raise OSError(rc, os.strerror(rc))
            return pid.value
        finally:
            for fd in fds:
                os.close(fd)
    finally:
        _LIBC.posix_spawn_file_actions_destroy(ctypes.byref(actions))
        _LIBC.posix_spawnattr_destroy(ctypes.byref(attr))


def run_runner(
    script: str, shots: Path, limit: float, env: dict[str, str]
) -> tuple[int | None, str, str]:
    """(exit code or None on timeout, stdout, stderr) of one disclaimed runner."""
    src, out, err = shots / "script.py", shots / "out.json", shots / "err.txt"
    src.write_text(script, "utf-8")
    pid = spawn_disclaimed([PYTHON, "-I", "-B", str(RUNNER), str(shots)], env, src, out, err)
    deadline = time.monotonic() + limit
    code: int | None = None
    while True:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            code = os.waitstatus_to_exitcode(status)
            break
        if time.monotonic() > deadline:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.killpg(pid, sig)
                except ProcessLookupError:
                    break
                time.sleep(0.3)
            os.waitpid(pid, 0)
            break
        time.sleep(0.01)
    return code, out.read_text("utf-8", "replace"), err.read_text("utf-8", "replace")


def run_script(script: str, timeout_ms: int | None) -> tuple[list[dict], bool, dict]:
    settings = trial()
    limit = max(1_000, min(int(timeout_ms or DEFAULT_TIMEOUT_MS), MAX_TIMEOUT_MS)) / 1000
    shots = Path(tempfile.mkdtemp(prefix="call-", dir=TMP))
    started = time.monotonic()
    code, stdout, stderr = run_runner(script, shots, limit, runner_env(settings))
    if code is None:
        result = {
            "ok": False,
            "error": f"the script did not finish within {limit:.0f} s and was killed",
        }
    else:
        line = (stdout.strip().splitlines() or [""])[-1]
        result = (
            json.loads(line)
            if line.startswith("{")
            else {"ok": False, "error": f"runner exited {code}: {stderr.strip()[-1500:]}"}
        )
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
                " (this trial grants foreground)."
                if granted
                else ". This trial does not grant foreground: if your script caused this, use a background route "
                "(an AX action or value, an event posted to the app's pid, or raw.driver with default delivery)."
            )
        )
    for item in result.get("runtime_blocked") or []:
        text.append(f"Refused at run time: {item}")
    if result.get("stdout"):
        text.append("stdout:\n" + result["stdout"])
    if result.get("ok"):
        text.append("return: " + json.dumps(result.get("value"), ensure_ascii=False)[:20000])
    else:
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
                or any(
                    w.split(":")[0] in {"MB1", "MB2", "MB3", "MB4", "MB10"}
                    for w in result.get("runtime_warned") or []
                )
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
                        "serverInfo": {"name": "cua", "version": "raw-macos-prototype"},
                        "instructions": "One tool, run_script: Python against the macOS Accessibility API and "
                        "per-process events. Read its description first.",
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
