#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Stdio MCP server of the `pi-raw-macos` bench arm: one tool, run_script, that runs Python against the macOS
Accessibility API, per-process event posting and CoreGraphics capture. No Cua Driver: the arm uses macOS's own APIs
only, and the lint blocks any call path to the driver (rule DRV1).

Each call: the background-use lint (rawlint_mac.py) reads the script; a blocked script is returned unrun with the
rule, its source and a background-safe alternative. A script that passes runs in a fresh runner process
(runner_mac.py) with restricted builtins and imports, under the call's time limit, while the runtime hook samples the
frontmost app, the key window, the focused element, the top window and the real cursor. The result carries the return
value, printed output, screenshots as image content, lint warnings, runtime refusals and every change the hook saw.

The runner is spawned as its own TCC-responsible process (posix_spawn with responsibility_spawnattrs_setdisclaim, as
Cua Driver's own re-exec does), so the Accessibility, Screen Recording and PostEvent grants of the runner's Python
binary apply, not those of whatever started the agent.

Environment: RAW_LOG (call log path), RAW_TMP (scratch directory for screenshots), PYTHON (interpreter for the
runner), RAW_TRIAL (the per-trial JSON the harness writes before each trial: its id, the foreground grant, the
per-app AX coverage labels and the user's apps). The trial file is read on every call. Element refs from raw.dump live
in RAW_TMP/refs-<trial id>.json, so a later call of the same trial can use them.
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
# The no-wrapper arm (pi-raw-macos-native): scripts call the frameworks directly, with no raw.* runtime
NATIVE = os.environ.get("RAW_NATIVE") == "1"
# The bare arm (pi-raw-macos-bare; Zane, 13 Nov 2026 17:02 SGT): one generic paragraph, no feedback beyond the result
BARE = os.environ.get("RAW_BARE") == "1"
if BARE:  # the other raw arms' prompts say to pass 120000 on every call; the bare arm's says nothing, so that is its default
    DEFAULT_TIMEOUT_MS = MAX_TIMEOUT_MS
# The docs arm (pi-raw-macos-docs; Zane, 13 Nov 2026 21:58 SGT): the bare arm plus a lookup tool over the docs generated
# for this macOS build (docs/gen_docs.py) and the generic notes (docs/notes.md)
DOCS_DIR = Path(os.environ["RAW_DOCS_DIR"]) if os.environ.get("RAW_DOCS_DIR") else None
LOOKUP_DESCRIPTION = (
    "Look up macOS API documentation (generated from this Mac's SDK headers and framework exports, public and "
    "private) and short how-to notes: a query in, the matching entries out."
)
LOOKUP_SCHEMA = {
    "type": "object",
    "required": ["query"],
    "properties": {
        "query": {"type": "string", "description": "Words or an API name."},
        "limit": {"type": "integer", "description": "Entries to return, default 8, max 20."},
    },
    "additionalProperties": False,
}
_DOCS_INDEX = None


def lookup(query: str, limit: int | None) -> tuple[list[dict], bool, dict]:
    global _DOCS_INDEX
    import docs_lookup

    started = time.monotonic()
    if _DOCS_INDEX is None:
        _DOCS_INDEX = docs_lookup.Index.load(DOCS_DIR)
    found = _DOCS_INDEX.search(query, limit or docs_lookup.DEFAULT_LIMIT)
    entry = {
        "t": time.time(),
        "tool": "lookup",
        "query": query[:500],
        "limit": limit,
        "hits": [e["name"] for e in found],
        "elapsed_s": round(time.monotonic() - started, 3),
    }
    return [{"type": "text", "text": docs_lookup.render(found)}], False, entry


# raw_mac.GUARD_MARKER (raw_mac imports the frameworks, so the server does not import it)
GUARD_MARKER = "key-focus-guard-open"

DESCRIPTION = """\
Run one Python script against the macOS desktop. The script body runs inside a function, so `return` a value \
(JSON-able) to see it; print() output is returned too. Each call is a fresh process: nothing persists between calls \
except element refs from raw.dump.

Everything goes through macOS's own APIs: the Accessibility API, events posted to one process, and window capture. \
There is no other automation layer to call.
{coverage}

Read (cheap first):
- raw.apps() -> [{pid, name, bundle_id, active, hidden}]; raw.windows(pid=None) -> on-screen windows front to back \
[{window_id, pid, app, title, layer, frame:(x,y,w,h)}]; raw.front() -> the user's frontmost app and key window.
- raw.dump(root, depth=14, limit=400, query=None) -> a compact text tree: wrapper groups are folded away, and every \
element you can act on carries a ref `[n]`; `raw.el(n)` returns that element in this or any later call of the task. \
`query="Save"` returns only the matching lines (with refs). Dump a window or a found subtree, not the whole app, \
when the app is big. `full=True` gives every node with frames and actions.
- raw.text(root) -> the visible strings under root in reading order (static text, headings, links, field values): \
the cheapest way to read a message, a page or a list.
- raw.find(root, role=None, title=None, contains=None) -> elements by AXRole ("AXButton", "AXTextField", \
"AXPopUpButton", "AXCheckBox", "AXRadioButton", "AXRow", "AXStaticText", "AXTable", "AXWebArea") and by exact \
title/description/value (`title`) or a substring (`contains`); raw.describe(el) -> one line.
- raw.ax: raw.ax.app(pid) (asks Chromium/Electron apps for their web-content tree), raw.ax.get(el, "AXValue"), \
raw.ax.names(el), raw.ax.actions(el), raw.ax.children(el), raw.ax.settable(el, attr), raw.ax.at(x, y), raw.ax.pid(el).
- raw.screenshot(window_id=None, region=None) -> the PNG comes back to you as an image (a window is captured even \
behind others).

Act:
- raw.ax.perform(el, "AXPress") for buttons, links, checkboxes, radio buttons, rows, tabs, menu items (also \
AXIncrement, AXDecrement, AXConfirm, AXCancel, AXPick); raw.ax.set(el, "AXSelected", True) for rows and tabs. \
Both return the AX error code (0 = accepted, not proof).
- raw.set_text(el, "text") -> {route, value, ok}: sets a text field and reads it back. It writes AXValue; where the \
app ignores that (web views, a browser address bar) it focuses the field inside that app, selects its text and types \
over it as key events to the app's pid. Follow with raw.post_key(pid, "return") to submit an address bar.
- raw.select(el, "Label") -> {route, value, ok}: chooses an option of an AXPopUpButton or AXComboBox. A web \
<select> (Chrome, Electron, web views) gets the label typed into it and its menu never opens. A native AppKit pop-up is \
opened and the item pressed, under the key-focus guard when the trial does not grant foreground.
- raw.set_cell(pid, "H4", "text", window=None, sheet=None) -> {value, ok}: writes one LibreOffice Calc cell (on \
screen) by selecting it and typing the text plus Return through the window-key route below; "" clears it. \
sheet="Controls" switches sheets first (Sheet > Navigate > To Next Sheet; pressing a sheet tab does nothing in the \
background). Calc parses typed text: "13:00" becomes a time; start with ' to keep it as text. Close a dialog that \
holds Calc's key focus (the "Welcome to LibreOffice!" window: press its AXCloseButton) first; the error names it. \
Save with raw.menu(pid, ["File", "Save"]).
- raw.menu(pid, ["File", "Save"]) -> runs a menu-bar command without opening the menu.
- Events to one process (the real cursor does not move): raw.post_key(pid, "return") or raw.post_key(pid, \
["cmd", "s"]), raw.post_text(pid, "text") (to that app's focused element), raw.post_click(pid, x, y, \
button="left", count=1), raw.post_scroll(pid, x, y, dy). Screen coordinates, top-left origin. Many apps ignore \
posted clicks while in the background: prefer AX actions. Window-key route: raw.post_key(pid, keys, \
window_id=w) and raw.post_text(pid, text, window_id=w) first make window w the app's key window without \
activating or raising it (SkyLight focus records), then post through SLEventPostToPid. LibreOffice drops plain \
posted keys in the background and takes these (Tab, Return and arrows move between its cells).
- Foreground pointer (moves the real cursor): raw.pointer_move(x, y), raw.pointer_click(x, y), \
raw.pointer_drag(x0, y0, x1, y1). raw.sleep(s).

Background contract (checked before the script runs, and again while it runs):
- The user's frontmost app, key window, focused element, window order and real cursor must stay as they are. \
Foreground routes: raw.pointer_*, AXRaise, setting AXFrontmost / AXMain / AXFocused / AXFocusedUIElement / \
AXMinimized / AXHidden through raw.ax.set. raw.set_text, raw.select and raw.set_cell move focus only inside a \
background target app and are refused (MB3) if the user's front app, key window or focused element changes.
- {grant}
- Menus are foreground too: AXShowMenu, and AXPress on an AXPopUpButton, AXMenuButton or AXMenuBarItem, open a menu \
that takes key focus from the user's window and lifts the target window over it. A native AppKit pop-up lists its \
items only while its menu is open and ignores an AXValue write: raw.select on one opens it. With the grant it opens \
as is; without it raw.select opens it under a key-focus guard that keeps the user's window key and on top (it hands \
the menu's key-focus theft back with SLPSReleaseKeyFocusWithID) and refuses under MB7 if anything stayed changed. \
The runtime check reports the brief raise the guard put back; that is expected. Never press the pop-up or post keys \
to it yourself: a posted Down, Up or Space while a pop-up has keyboard focus opens its menu and is refused (MB7). Run \
menu-bar commands with raw.menu.
- Always blocked: launching or opening apps (open, NSWorkspace, AppleScript, osascript), shells, files, network, \
pyobjc and framework imports in the script itself (use the raw.* helpers, which make the native calls, public and \
private), and Cua Driver in any form.
- Warned: cmd+L, cmd+shift+G and tab-switching shortcuts make apps activate themselves; clipboard writes replace \
what the user copied.
- Not available: files, processes, network and other imports (json, re, math, time, itertools, collections, \
functools, string, textwrap are importable); eval/exec, private attributes, getattr with computed names.
- An action the API accepted is not proof it worked: read the element back (raw.ax.get, raw.text, raw.dump with a \
query, a screenshot) before you report success.
"""
NATIVE_DESCRIPTION = """\
Run one Python script against the macOS desktop. The script body runs inside a function, so `return` a value \
(JSON-able) to see it; print() output is returned too. Each call is a fresh process: nothing persists between calls.

Call macOS's own APIs directly; there is no helper library and no other automation layer. Any native macOS API, \
public or private, is allowed. Import what you need: pyobjc's ApplicationServices / HIServices (the Accessibility \
API: AXUIElementCreateApplication, AXUIElementCopyAttributeValue, AXUIElementSetAttributeValue, \
AXUIElementPerformAction, AXUIElementCopyActionNames, ...), Quartz (CGWindowListCopyWindowInfo, \
CGEventCreateKeyboardEvent, CGEventCreateMouseEvent, CGEventKeyboardSetUnicodeString, CGEventPostToPid, \
CGWindowListCreateImage, CGImageDestination*), AppKit / Foundation (NSWorkspace.runningApplications, \
NSRunningApplication), CoreFoundation, ScreenCaptureKit, objc, and ctypes for private frameworks and symbols (for \
example ctypes.CDLL("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight") for SLEventPostToPid, \
SLPSPostEventRecordTo, SLPSReleaseKeyFocusWithID, or _AXUIElementGetWindow in ApplicationServices). A pyobjc object's \
pointer for ctypes is obj.__c_void_p__(). Also importable: json, re, math, time, itertools, collections, \
functools, string, textwrap, dataclasses, typing, enum.
{coverage}

Screenshots: write PNG files into the directory named by the global SHOTS (for example with \
CGWindowListCreateImage and CGImageDestinationCreateWithURL); each PNG there comes back to you as an image.

Background contract (the script is checked before it runs, and a hook samples the user's frontmost app, key window, \
focused element, top window and real cursor every 50 ms while it runs and reports every change):
- The user's frontmost app, key window, focused element, window order and real cursor must stay as they are. \
Foreground routes: events posted at the HID or session tap (CGEventPost) or cursor warps, which move or press the \
real pointer; activating an app or AXRaise; writing AXFrontmost / AXMain / AXFocused / AXFocusedUIElement / \
AXFocusedWindow / AXMainWindow / AXMinimized / AXHidden; opening a menu (AXShowMenu, AXPress on an AXPopUpButton, \
AXMenuButton or AXMenuBarItem, a posted right-click, or a posted Down, Up or Space to a focused pop-up), which takes \
key focus from the user's window and lifts the target window over it.
- {grant}
- Posting keys or clicks to the user's own app (the apps the task names as the user's) changes their work: post to \
the target app's pid.
- Always blocked: launching or opening apps (open, NSWorkspace open/launch, AppleScript, osascript, NSTask), shells, \
processes, files, network, ctypes.pythonapi, eval/exec, dunder attributes other than __c_void_p__, getattr with \
computed names, and Cua Driver in any form.
- Warned: cmd+L, cmd+shift+G and tab-switching shortcuts make apps activate themselves; clipboard writes replace \
what the user copied.
- An action the API accepted is not proof it worked: read the element back (AXValue, the window's text, a \
screenshot) before you report success.
"""
BARE_DESCRIPTION = """\
Run a Python script on this macOS desktop and get back its result. The script body runs inside a function: \
`return` a JSON-able value to see it (print output comes back too), and each call is a fresh process. The \
platform's native APIs are importable: pyobjc (ApplicationServices, Quartz, AppKit, Foundation, CoreFoundation, \
ScreenCaptureKit, objc) and ctypes for any framework or symbol, public or private. PNG files written into the \
directory named by the global SHOTS come back as images. Shells, subprocesses, file and network access are not \
available.
"""
NATIVE_GRANTED = (
    "This trial grants foreground: the foreground routes above are allowed when no background route works. Each "
    "use is reported."
)
NATIVE_NOT_GRANTED = (
    "This trial does not grant foreground: the routes above are not allowed. Use a background route (AX actions and "
    "values, events posted to the target app's pid)."
)
GRANTED = (
    "This trial grants foreground: the routes above are allowed when no background route works (a real hover, a "
    "drag-and-drop with no accessible action, a tooltip that only shows on hover, a native pop-up menu). Each use is "
    "reported."
)
NOT_GRANTED = (
    "This trial does not grant foreground: the routes above are blocked. Use a background route."
)


def trial() -> dict:
    """The harness's per-trial settings (id, grant, coverage labels, user apps), or the safe default: no grant."""
    if TRIAL is None or not TRIAL.is_file():
        return {"trial_id": "none", "foreground_grant": False, "coverage": {}}
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
    if BARE:
        return BARE_DESCRIPTION
    if NATIVE:
        grant = NATIVE_GRANTED if settings.get("foreground_grant") else NATIVE_NOT_GRANTED
        return NATIVE_DESCRIPTION.replace("{coverage}", coverage).replace("{grant}", grant)
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
    trial_id = "".join(
        c for c in str(settings.get("trial_id") or "none") if c.isalnum() or c in "-_."
    )
    env["RAW_REFS"] = str(TMP / f"refs-{trial_id}.json")
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
                except (ProcessLookupError, PermissionError):
                    break  # gone; EPERM once the group's only member is an unreaped zombie
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
    result: dict | None = None
    if code is not None:
        line = (stdout.strip().splitlines() or [""])[-1]
        try:
            parsed = json.loads(line) if line.startswith("{") else None
        except ValueError:
            parsed = None  # cut off mid-write
        result = parsed if isinstance(parsed, dict) else None
    # complete: the runner wrote its result after the hook's report (a lint block never ran anything)
    complete = result is not None and ("watch" in result or bool(result.get("lint_blocked")))
    if result is None:
        result = {
            "ok": False,
            "error": f"the script did not finish within {limit:.0f} s and was killed"
            if code is None
            else f"runner exited {code}: {stderr.strip()[-1500:]}",
        }
    if not complete and (shots / GUARD_MARKER).exists():
        # no complete result from a call that opened a background pop-up menu under the key-focus guard (killed,
        # crashed, cut off, or failed before its hook report): what it left on screen is unknown, so it is refused
        result["runtime_blocked"] = list(result.get("runtime_blocked") or []) + [
            "MB7: the script ended without a complete result after raw.select opened a background pop-up menu "
            "under the key-focus guard; the user's key focus and window order were not checked"
        ]
    elapsed = round(time.monotonic() - started, 2)
    text: list[str] = []
    if result.get("lint_text"):
        text.append(result["lint_text"])
    granted = bool(settings.get("foreground_grant"))
    watch = result.get("watch") or {}
    if watch.get("lines") and not BARE:
        text.append(
            "Runtime background check: "
            + "; ".join(watch["lines"])
            + (
                " (this trial grants foreground)."
                if granted
                else ". This trial does not grant foreground: if your script caused this, use a background route "
                + (
                    "(an AX action or value, or an event posted to the app's pid)."
                    if NATIVE
                    else "(an AX action or value, raw.set_text / raw.select / raw.set_cell / raw.menu, or an event "
                    "posted to the app's pid)."
                )
            )
        )
    for item in [] if BARE else result.get("runtime_blocked") or []:
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
                or any(
                    w.split(":")[0] in {"MB1", "MB2", "MB3", "MB4", "MB6", "MB7"}
                    for w in result.get("runtime_warned") or []
                )
            )
        ),
        "routes": result.get("routes") or {},
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
                        "instructions": "Two tools: run_script runs Python on this macOS desktop; lookup searches "
                        "macOS API documentation."
                        if BARE and DOCS_DIR is not None
                        else "One tool, run_script: Python on this macOS desktop."
                        if BARE
                        else "One tool, run_script: Python against the macOS Accessibility API, "
                        "per-process events and window capture"
                        + (", called directly (no helper library)" if NATIVE else "")
                        + ". Read its description first.",
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
                        + (
                            [
                                {
                                    "name": "lookup",
                                    "description": LOOKUP_DESCRIPTION,
                                    "inputSchema": LOOKUP_SCHEMA,
                                }
                            ]
                            if DOCS_DIR is not None
                            else []
                        )
                    }
                }
            )
        elif method == "tools/call":
            params = message.get("params") or {}
            args = params.get("arguments") or {}
            if (
                DOCS_DIR is not None
                and params.get("name") == "lookup"
                and isinstance(args.get("query"), str)
                and args["query"].strip()
            ):
                try:
                    content, failed, entry = lookup(args["query"], args.get("limit"))
                except Exception as error:  # noqa: BLE001 - reported to the model and logged
                    content, failed = [{"type": "text", "text": f"lookup failed: {error}"}], True
                    entry = {
                        "t": time.time(),
                        "tool": "lookup",
                        "query": args["query"][:500],
                        "error": str(error),
                    }
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
