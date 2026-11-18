#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Minimal stdlib MCP adapter for vercel-labs/agent-desktop (Apache-2.0; bench arm `cc-agent-desktop`).

agent-desktop is a plain CLI, so each MCP tool maps 1:1 to one CLI command. The command's stdout (one JSON envelope)
goes back as text and base64 PNGs become MCP image content. Tool descriptions are the CLI's own one-line command
help; the server instructions are the agent-desktop skill text (`ad-instructions.md`, from the agent-desktop
package, Apache-2.0, unmodified). No shell, no skills/session/trace/permissions/notification commands.

Environment:
  AD_BIN        path of the agent-desktop executable (default: `agent-desktop` on PATH)
  AD_HEADED     agent (default; the model may pass `headed` on tools that can move the pointer) | never | always
  AD_INSTRUCTIONS  path of the instructions file (default: ad-instructions.md next to this file)
"""

import base64
import json
import os
import re
import subprocess
import sys

HEADED = os.environ.get("AD_HEADED", "agent")
INSTR = os.environ.get(
    "AD_INSTRUCTIONS",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "ad-instructions.md"),
)
S = lambda d: {"type": "string", "description": d}
INT = lambda d: {"type": "integer", "description": d}
B = lambda d: {"type": "boolean", "description": d}
REF = S("Qualified ref from snapshot (@<snapshot_id>:eN)")
APP, WIN = S("Application name"), S("Window ID from list-windows")
# tool: (cli command, description, positional params, {param: (flag, kind, schema)}, takes_headed)
T = {
    "snapshot": (
        "snapshot",
        "Capture accessibility tree as structured JSON with @ref IDs",
        [],
        {
            "app": ("--app", "s", APP),
            "window_id": ("--window-id", "s", WIN),
            "surface": ("--surface", "s", S("window|focused|menu|menubar|sheet|popover|alert")),
            "root": ("--root", "s", S("Start from this ref")),
            "interactive_only": ("-i", "f", B("Interactive elements only")),
            "compact": ("--compact", "f", B("Collapse single-child unnamed nodes")),
            "skeleton": ("--skeleton", "f", B("Shallow overview")),
            "max_depth": ("--max-depth", "s", INT("Max depth (default 10)")),
            "include_bounds": ("--include-bounds", "f", B("Include element bounds")),
            "timeout_ms": ("--timeout-ms", "s", INT("Observation deadline ms")),
        },
        False,
    ),
    "find": (
        "find",
        "Search elements by role, name, value, or text content",
        [],
        {
            "app": ("--app", "s", APP),
            "window_id": ("--window-id", "s", WIN),
            "role": ("--role", "s", S("Accessibility role")),
            "name": ("--name", "s", S("Accessible name")),
            "value": ("--value", "s", S("Current value")),
            "text": ("--text", "s", S("Text in name/value/title/description")),
            "exact": ("--exact", "f", B("Exact match")),
            "root": ("--root", "s", S("Search inside this ref")),
            "surface": ("--surface", "s", S("Surface")),
            "first": ("--first", "f", B("First match only")),
            "limit": ("--limit", "s", INT("Max matches")),
        },
        False,
    ),
    "get": (
        "get",
        "Read an element property (text, value, title, bounds, role, states)",
        ["ref"],
        {
            "ref": ("", "p", REF),
            "property": ("--property", "s", S("text|value|title|bounds|role|states")),
        },
        False,
    ),
    "is": (
        "is",
        "Check element state (visible, enabled, checked, focused, expanded, selected)",
        ["ref"],
        {"ref": ("", "p", REF), "property": ("--property", "s", S("State"))},
        False,
    ),
    "screenshot": (
        "screenshot",
        "Take a PNG screenshot of an application window",
        [],
        {
            "app": ("--app", "s", APP),
            "window_id": ("--window-id", "s", WIN),
            "screen": ("--screen", "s", INT("Display index")),
        },
        False,
    ),
    "click": (
        "click",
        "Click element via accessibility press action",
        ["ref"],
        {"ref": ("", "p", REF)},
        True,
    ),
    "double_click": (
        "double-click",
        "Physically double-click element; requires headed",
        ["ref"],
        {"ref": ("", "p", REF)},
        True,
    ),
    "right_click": (
        "right-click",
        "Open a context menu semantically, or physically with headed",
        ["ref"],
        {"ref": ("", "p", REF)},
        True,
    ),
    "type": (
        "type",
        "Insert text into a text target",
        ["ref", "text"],
        {"ref": ("", "p", REF), "text": ("", "p", S("Text to type"))},
        True,
    ),
    "set_value": (
        "set-value",
        "Set element value directly via accessibility attribute",
        ["ref", "value"],
        {"ref": ("", "p", REF), "value": ("", "p", S("Value"))},
        False,
    ),
    "clear": (
        "clear",
        "Clear element value to empty string",
        ["ref"],
        {"ref": ("", "p", REF)},
        True,
    ),
    "focus": ("focus", "Set keyboard focus on element", ["ref"], {"ref": ("", "p", REF)}, False),
    "select": (
        "select",
        "Select an option in a list or dropdown",
        ["ref", "value"],
        {"ref": ("", "p", REF), "value": ("", "p", S("Option"))},
        False,
    ),
    "toggle": ("toggle", "Toggle a checkbox or switch", ["ref"], {"ref": ("", "p", REF)}, False),
    "check": (
        "check",
        "Set checkbox or switch to checked state (idempotent)",
        ["ref"],
        {"ref": ("", "p", REF)},
        False,
    ),
    "uncheck": (
        "uncheck",
        "Set checkbox or switch to unchecked state (idempotent)",
        ["ref"],
        {"ref": ("", "p", REF)},
        False,
    ),
    "expand": (
        "expand",
        "Expand a disclosure triangle or tree item",
        ["ref"],
        {"ref": ("", "p", REF)},
        False,
    ),
    "collapse": (
        "collapse",
        "Collapse a disclosure triangle or tree item",
        ["ref"],
        {"ref": ("", "p", REF)},
        False,
    ),
    "scroll": (
        "scroll",
        "Scroll element (--direction up/down/left/right, --amount N)",
        ["ref"],
        {
            "ref": ("", "p", REF),
            "direction": ("--direction", "s", S("up|down|left|right")),
            "amount": ("--amount", "s", INT("Scroll units (default 3)")),
        },
        True,
    ),
    "scroll_to": (
        "scroll-to",
        "Scroll element into visible area",
        ["ref"],
        {"ref": ("", "p", REF)},
        False,
    ),
    "press": (
        "press",
        "Send a key combo: return, escape, cmd+c, shift+tab ...",
        ["combo"],
        {"combo": ("", "p", S("Key combo")), "window_id": ("--window-id", "s", WIN)},
        False,
    ),
    "hover": (
        "hover",
        "Move cursor to element center or coordinates (requires headed)",
        [],
        {"ref": ("", "o", REF), "xy": ("--xy", "s", S("x,y"))},
        "force",
    ),
    "drag": (
        "drag",
        "Drag from one element or point to another (requires headed)",
        [],
        {
            "from": ("--from", "s", REF),
            "from_xy": ("--from-xy", "s", S("x,y")),
            "to": ("--to", "s", REF),
            "to_xy": ("--to-xy", "s", S("x,y")),
            "duration": ("--duration", "s", INT("ms")),
            "drop_delay": ("--drop-delay", "s", INT("ms")),
        },
        "force",
    ),
    "mouse_move": (
        "mouse-move",
        "Move cursor to absolute screen coordinates (requires headed)",
        [],
        {"xy": ("--xy", "s", S("x,y"))},
        "force",
    ),
    "mouse_click": (
        "mouse-click",
        "Click at absolute screen coordinates (requires headed)",
        [],
        {
            "xy": ("--xy", "s", S("x,y")),
            "button": ("--button", "s", S("left|right|middle")),
            "count": ("--count", "s", INT("Clicks")),
        },
        "force",
    ),
    "mouse_wheel": (
        "mouse-wheel",
        "Scroll the mouse wheel at absolute coordinates (requires headed)",
        [],
        {
            "x": ("--x", "s", INT("X")),
            "y": ("--y", "s", INT("Y")),
            "dy": ("--dy", "s", INT("Vertical lines")),
        },
        "force",
    ),
    "launch": (
        "launch",
        "Launch application and return once its process is running; --activate waits for a window",
        ["app"],
        {
            "app": ("", "p", S("App name or bundle ID")),
            "activate": ("--activate", "f", B("Wait for a window")),
        },
        False,
    ),
    "close_app": (
        "close-app",
        "Quit an application gracefully (--force to terminate)",
        ["app"],
        {"app": ("", "p", APP), "force": ("--force", "f", B("Terminate"))},
        False,
    ),
    "list_windows": (
        "list-windows",
        "List all visible windows (--app to filter by application)",
        [],
        {"app": ("--app", "s", APP)},
        False,
    ),
    "list_apps": (
        "list-apps",
        "List all running GUI applications (--app to filter)",
        [],
        {"app": ("--app", "s", APP)},
        False,
    ),
    "focus_window": (
        "focus-window",
        "Bring a window to front and confirm OS focus",
        [],
        {
            "window_id": ("--window-id", "s", WIN),
            "app": ("--app", "s", APP),
            "title": ("--title", "s", S("Window title")),
        },
        False,
    ),
    "resize_window": (
        "resize-window",
        "Resize application window",
        [],
        {
            "app": ("--app", "s", APP),
            "window_id": ("--window-id", "s", WIN),
            "width": ("--width", "s", INT("px")),
            "height": ("--height", "s", INT("px")),
        },
        False,
    ),
    "move_window": (
        "move-window",
        "Move application window to new position",
        [],
        {
            "app": ("--app", "s", APP),
            "window_id": ("--window-id", "s", WIN),
            "x": ("--x", "s", INT("X")),
            "y": ("--y", "s", INT("Y")),
        },
        False,
    ),
    "clipboard_get": (
        "clipboard-get",
        "Read plain-text or typed clipboard contents",
        [],
        {},
        False,
    ),
    "clipboard_set": (
        "clipboard-set",
        "Write text to the clipboard",
        ["text"],
        {"text": ("", "p", S("Text"))},
        False,
    ),
    "wait": (
        "wait",
        "Wait for time (ms), element presence, text, or window appearance",
        ["ms"],
        {
            "ms": ("", "o", INT("Milliseconds to pause")),
            "element": ("--element", "s", REF),
            "text": ("--text", "s", S("Text")),
            "window": ("--window", "s", S("Window title")),
            "predicate": ("--predicate", "s", S("exists|enabled|visible|actionable|value")),
            "app": ("--app", "s", APP),
            "timeout": ("--timeout", "s", INT("ms")),
        },
        False,
    ),
    "batch": (
        "batch",
        "Execute a bounded, sequential, non-atomic JSON command batch",
        ["json"],
        {"json": ("", "p", S("JSON array of {command, args} objects"))},
        False,
    ),
}
# Not exposed (parity with the other arms' tool sets; no shell, no host side effects): skills, session, trace, cursor-overlay,
# permissions, status, notification commands, open-system-surface, list-surfaces, minimize/maximize/restore (add if cua-driver's set_window_frame is judged equal).


def argv_for(name, a):
    cmd, _, pos, params, hd = T[name]
    out = []
    if (
        HEADED == "always"
        or hd == "force"
        or (HEADED == "agent" and hd is True and a.get("headed"))
    ):
        out.append("--headed")
    out.append(cmd)
    for k, (flag, kind, _) in params.items():
        v = a.get(k)
        if v is None or v is False:
            continue
        if kind in ("p", "o"):
            continue
        if kind == "f":
            out.append(flag)
        else:
            out += [flag + "=" + str(v)] if str(v).startswith("-") else [flag, str(v)]
    out += [str(a[k]) for k in pos if a.get(k) is not None]
    return out


def tools():
    res = []
    for n, (cmd, desc, pos, params, hd) in T.items():
        props = {k: v[2] for k, v in params.items()}
        if hd is True and HEADED == "agent":
            props["headed"] = B(
                "Physical delivery (moves the pointer/focus). Default is headless semantic delivery."
            )
        res.append(
            {
                "name": n,
                "description": desc,
                "inputSchema": {
                    "type": "object",
                    "properties": props,
                    "required": [k for k in pos if params[k][1] == "p"],
                },
            }
        )
    return res


class Backend:
    def __init__(self):
        self.exe = os.environ.get("AD_BIN") or "agent-desktop"

    def run(self, argv, timeout=120):
        try:
            r = subprocess.run([self.exe] + argv, capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return 124, b"timed out"
        return r.returncode, (r.stdout + r.stderr)


def to_content(rc, raw):
    text = raw.decode("utf-8", "replace")
    imgs = []

    def strip(m):
        imgs.append(m.group(0))
        return "<image %d>" % len(imgs)

    text = re.sub(r"[A-Za-z0-9+/=]{2000,}", strip, text)
    out = [{"type": "text", "text": text}]
    out += [{"type": "image", "data": i, "mimeType": "image/png"} for i in imgs]
    return out, rc != 0 and not text.lstrip().startswith("{")


def serve():
    be = Backend()
    instr = open(INSTR).read() if os.path.exists(INSTR) else ""
    for line in sys.stdin:
        m = json.loads(line)
        mid, meth = m.get("id"), m.get("method")

        def send(r):
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, **r}) + "\n")
            sys.stdout.flush()

        if meth == "initialize":
            send(
                {
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "agentdesktop", "version": "bench-adapter"},
                        "instructions": instr,
                    }
                }
            )
        elif meth == "tools/list":
            send({"result": {"tools": tools()}})
        elif meth == "tools/call":
            p = m["params"]
            try:
                if p["name"] not in T:
                    raise KeyError(p["name"])
                content, err = to_content(*be.run(argv_for(p["name"], p.get("arguments") or {})))
                send({"result": {"content": content, "isError": err}})
            except Exception as e:
                send(
                    {
                        "result": {
                            "content": [{"type": "text", "text": "adapter error: %s" % e}],
                            "isError": True,
                        }
                    }
                )
        elif mid is not None:
            send({"result": {}})


if __name__ == "__main__":
    serve()
