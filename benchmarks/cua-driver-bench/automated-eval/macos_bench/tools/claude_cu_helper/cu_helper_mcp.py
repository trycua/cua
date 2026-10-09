#!/usr/bin/env python3
"""Minimal MCP adapter around Claude Desktop's ``app-cu-helper`` (arm ``cc-claude-cu-helper``, Amendment 11):
"Claude Desktop 2.31226.0 computer-use helper via a minimal adapter".

The helper of Claude Desktop 2.31226.0 is not an MCP server. It reads newline-delimited JSON-RPC on stdin and
writes the responses on stdout. Its methods (probed in the VM): ``probe``, ``dispatchRaw`` (kinds click, rclick,
mclick, drag, scroll, hover, key, type: input delivered to one window of one process at a window-local point, in the
background), ``wakeChromiumCompositor``, ``bringWindowToActiveSpace`` and ``keyWindowSpoof``. This adapter exposes
the input as MCP tools shaped like Claude's own background computer-use tools (``app_*``) and adds what the helper
does not provide:

* **Which windows exist** (``app_list_windows``): ``winlist``, a small Swift tool built in the VM that prints
  ``CGWindowListCopyWindowInfo`` (owner, pid, window id, bounds, title). Public macOS API only.
* **What a window shows** (``app_screenshot``): ``/usr/sbin/screencapture -l <window id>`` of that one window, the
  stock macOS tool, downscaled with ``/usr/bin/sips`` to Claude Code's own screenshot budget. Desktop captures with
  its in-process native module (``@ant/claude-swift``, ``appScoped.captureWindow``), not with the helper, so that path
  is not reachable here. Like Desktop, the adapter asks the helper to wake a Chromium window's compositor first.

Everything that changes the screen goes through the helper's ``dispatchRaw`` and nothing else. When the helper
refuses or cannot do something, the tool returns the helper's refusal; no other input path is used. Desktop's
accessibility layer (element indexes, menus, AX text entry) lives in its main process, not in the helper, and is not
offered. Cua Driver is not used anywhere in this arm.

Run: python3 -I -B cu_helper_mcp.py --helper <app-cu-helper> --winlist <winlist> [--launcher <cu-disclaim>]
     [--actions click,right_click,type,key,scroll,drag,hover]
Stdlib only. Logs go to stderr; nothing is written to disk except the temporary screenshot files, which are deleted
after each call.
"""

from __future__ import annotations

import argparse
import base64
import json
import math
import os
import queue
import struct
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, Callable

ADAPTER_VERSION = "1.0.0"
SERVER_NAME = "claude-cu-helper"
SCREENCAPTURE = "/usr/sbin/screencapture"
SIPS = "/usr/bin/sips"
# Claude Code's built-in computer use downscales every screenshot before it reaches the model (a 3456x2234
# capture becomes about 1372x887, docs "Screenshots are downscaled automatically"): about 1.22 megapixels. The
# adapter uses the same budget, so the model sees the same amount of detail as with Claude's own tool.
MAX_PIXELS = 1372 * 887
MAX_EDGE = 1568
HELPER_TIMEOUT_S = 15.0

# The input tools and the helper call each one makes. The mapping copies Claude Desktop 2.31226.0's own raw-input path
# (its OTr function and dispatch call site in app.asar, read in the VM): dispatchRaw with pid, windowId, winLocalPt,
# focusedTarget, hostPid and nativeMouseVariant plus the kind fields below; scroll units are Desktop's x40 points;
# key combos map meta/cmd -> cmd, alt/option -> option, ctrl, shift, fn; typing is capped at 4000 characters and a
# "replace" type first sends cmd+a with partOfTextWrite, as Desktop does. Which tools are offered is fixed by
# --actions (pinned from the VM probe); nothing here falls back to any other input path.
ALL_ACTIONS = ("click", "right_click", "type", "key", "scroll", "drag", "hover")
TYPE_MAX_CHARS = 4000
SCROLL_POINTS_PER_UNIT = 40
BUSY_RETRIES, BUSY_WAIT_S = 6, 0.4  # Desktop retries "user_actively_typing" 6 times, 400 ms apart
MODIFIERS = {"cmd": "cmd", "command": "cmd", "meta": "cmd", "super": "cmd", "shift": "shift", "alt": "option",
             "option": "option", "opt": "option", "ctrl": "ctrl", "control": "ctrl", "fn": "fn"}
KEY_ALIASES = {"enter": "return", "esc": "escape", "pgdn": "pageDown", "pagedown": "pageDown", "pgup": "pageUp",
               "pageup": "pageUp", "arrowup": "up", "arrowdown": "down", "arrowleft": "left", "arrowright": "right"}


def log(message: str) -> None:
    sys.stderr.write(f"[{SERVER_NAME}] {message}\n")
    sys.stderr.flush()


# ---------------------------------------------------------------- helper process


class HelperError(RuntimeError):
    pass


class Helper:
    """The helper as a child process: one request at a time, newline-delimited JSON-RPC 2.0."""

    def __init__(self, argv: list[str], env: dict[str, str] | None = None) -> None:
        self.argv = argv
        self.env = env
        self.proc: subprocess.Popen[str] | None = None
        self.lines: "queue.Queue[str | None]" = queue.Queue()
        self.next_id = 0
        self.lock = threading.Lock()

    def _start(self) -> None:
        self.proc = subprocess.Popen(
            self.argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,  # the helper logs on stderr; it lands in Claude Code's MCP log with ours
            text=True,
            bufsize=1,
            env=self.env,
        )
        self.lines = queue.Queue()
        threading.Thread(target=self._pump, args=(self.proc, self.lines), daemon=True).start()

    @staticmethod
    def _pump(proc: subprocess.Popen[str], lines: "queue.Queue[str | None]") -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            lines.put(line)
        lines.put(None)

    def call(self, method: str, params: dict[str, Any], timeout: float = HELPER_TIMEOUT_S) -> dict[str, Any]:
        with self.lock:
            if self.proc is None or self.proc.poll() is not None:
                self._start()
            assert self.proc is not None and self.proc.stdin is not None
            self.next_id += 1
            ident = self.next_id
            self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": ident, "method": method, "params": params}) + "\n")
            self.proc.stdin.flush()
            deadline = time.monotonic() + timeout
            while True:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise HelperError(f"helper {method}: no response within {timeout:g} s")
                try:
                    line = self.lines.get(timeout=left)
                except queue.Empty:
                    continue
                if line is None:
                    raise HelperError(f"helper exited during {method} (rc={self.proc.poll()})")
                try:
                    reply = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if reply.get("id") != ident:
                    continue
                if "error" in reply:
                    raise HelperError(f"helper {method}: {json.dumps(reply['error'])[:500]}")
                return reply.get("result") or {}

    def close(self) -> None:
        if self.proc is not None and self.proc.poll() is None:
            try:
                assert self.proc.stdin is not None
                self.proc.stdin.close()  # the helper exits when stdin closes
                self.proc.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                self.proc.kill()


# ---------------------------------------------------------------- windows and screenshots


def list_windows(winlist: str) -> list[dict[str, Any]]:
    done = subprocess.run([winlist], capture_output=True, text=True, timeout=10)
    if done.returncode != 0:
        raise RuntimeError(f"winlist failed: {done.stderr.strip()[:300]}")
    return json.loads(done.stdout)


def find_window(winlist: str, window_id: int) -> dict[str, Any]:
    for win in list_windows(winlist):
        if int(win.get("window_id", -1)) == int(window_id):
            return win
    raise ValueError(f"no on-screen window with window_id {window_id}; call list_windows")


def png_size(data: bytes) -> tuple[int, int]:
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("not a PNG")
    width, height = struct.unpack(">II", data[16:24])
    return int(width), int(height)


def target_size(width: int, height: int) -> tuple[int, int]:
    """Largest size within MAX_PIXELS and MAX_EDGE with the same aspect ratio (never upscaled)."""
    scale = min(1.0, math.sqrt(MAX_PIXELS / float(width * height)), MAX_EDGE / float(max(width, height)))
    return max(1, int(width * scale)), max(1, int(height * scale))


def capture_window(window_id: int, screencapture: str = SCREENCAPTURE, sips: str = SIPS) -> tuple[bytes, int, int]:
    """PNG of one window (no shadow), downscaled to the screenshot budget. Returns (png, width, height)."""
    with tempfile.TemporaryDirectory(prefix="cuhelper-") as tmp:
        path = os.path.join(tmp, "w.png")
        done = subprocess.run([screencapture, "-x", "-o", "-t", "png", f"-l{int(window_id)}", path],
                              capture_output=True, text=True, timeout=20)
        if done.returncode != 0 or not os.path.exists(path) or os.path.getsize(path) == 0:
            raise RuntimeError(f"screencapture failed (rc={done.returncode}): {done.stderr.strip()[:300]}")
        with open(path, "rb") as handle:
            data = handle.read()
        width, height = png_size(data)
        new_w, new_h = target_size(width, height)
        if (new_w, new_h) != (width, height):
            subprocess.run([sips, "-z", str(new_h), str(new_w), path], capture_output=True, timeout=20, check=True)
            with open(path, "rb") as handle:
                data = handle.read()
            width, height = png_size(data)
        return data, width, height


# ---------------------------------------------------------------- the MCP server


def parse_combo(combo: str) -> tuple[str, list[str]]:
    """"cmd+shift+z" -> ("z", ["cmd", "shift"]); the last non-modifier part is the key."""
    parts = [p.strip() for p in str(combo or "").replace(" ", "").split("+") if p.strip()]
    mods: list[str] = []
    keys: list[str] = []
    for part in parts:
        low = part.lower()
        if low in MODIFIERS:
            if MODIFIERS[low] not in mods:
                mods.append(MODIFIERS[low])
        else:
            keys.append(KEY_ALIASES.get(low, part if len(part) > 1 else low))
    if len(keys) != 1:
        raise ValueError(f"combo {combo!r} must name exactly one key, e.g. \"return\" or \"cmd+a\"")
    return keys[0], mods


class Adapter:
    """Tools in the shape of Claude's own background computer-use tools (``app_list_windows``, ``app_screenshot``,
    ``app_click``, ``app_type``, ``app_key``, ``app_scroll``, ``app_drag``: ``app`` + ``window_id`` targeting,
    ``coordinate`` in screenshot pixels), limited to what the helper can do. Desktop's accessibility layer (element
    indexes, menus, AX text entry) is not part of the helper and is not offered."""

    def __init__(self, helper: Helper, winlist: str, actions: list[str], host_pid: int | None = None,
                 screencapture: str = SCREENCAPTURE, sips: str = SIPS, busy_wait_s: float = BUSY_WAIT_S) -> None:
        self.helper = helper
        self.winlist = winlist
        self.actions = [a for a in ALL_ACTIONS if a in actions]
        self.host_pid = host_pid if host_pid is not None else os.getpid()
        self.screencapture = screencapture
        self.sips = sips
        self.busy_wait_s = busy_wait_s
        # window_id -> (window width in points, height in points, screenshot width, screenshot height)
        self.frames: dict[int, tuple[float, float, int, int]] = {}

    # -------------------------------------------------- tool definitions

    def tools(self) -> list[dict[str, Any]]:
        app = {"type": "string", "description": "Application name as shown by app_list_windows (e.g. \"BenchLab\")."}
        window_id = {"type": "integer", "description": "window_id from app_list_windows; default: the app's frontmost window"}
        coordinate = {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2,
                      "description": "[x, y] in pixels of the latest app_screenshot of this window (origin top-left)"}
        bg = " without bringing it to the front (delivered by Claude Desktop's computer-use helper)."

        def tool(name: str, description: str, props: dict[str, Any], required: list[str]) -> dict[str, Any]:
            return {"name": name, "description": description, "inputSchema": {
                "type": "object", "properties": props, "required": required, "additionalProperties": False}}

        out = [
            tool("app_list_windows", "List the windows on screen. Without app: every app. Returns [{app, pid, window_id, "
                 "title, x, y, width, height}] (frame in screen points). Use the window_id with app_screenshot and the "
                 "action tools.", {"app": app}, []),
            tool("app_screenshot", "Capture a screenshot of one window of an application. The (x, y) coordinates you "
                 "pass to the action tools are ALWAYS pixels in the latest screenshot of that window.",
                 {"app": app, "window_id": window_id}, ["app"]),
        ]
        if "click" in self.actions:
            buttons = ["left", "right"] if "right_click" in self.actions else ["left"]
            out.append(tool("app_click", "Click a point within one window of an application" + bg + " Right-clicks "
                            "that would open a context menu, and clicks on pop-up menus, are refused by the helper.",
                            {"app": app, "window_id": window_id, "coordinate": coordinate,
                             "button": {"type": "string", "enum": buttons},
                             "count": {"type": "number", "enum": [1, 2, 3]}}, ["app", "coordinate"]))
        if "type" in self.actions:
            out.append(tool("app_type", "Type text into one window of an application" + bg + " With coordinate, the "
                            "text goes to the text element at that point (it must be the focused element: click it "
                            "first); without coordinate, to the app's focused element. mode \"replace\" selects all "
                            f"first (cmd+a). At most {TYPE_MAX_CHARS} characters.",
                            {"app": app, "window_id": window_id, "coordinate": coordinate, "text": {"type": "string"},
                             "mode": {"type": "string", "enum": ["insert", "replace"]}}, ["app", "text"]))
        if "key" in self.actions:
            out.append(tool("app_key", "Send a key or shortcut to one window of an application" + bg + " Examples: "
                            "\"return\", \"escape\", \"tab\", \"backspace\", \"delete\", \"up\", \"pageDown\", "
                            "\"cmd+a\". Some shortcuts are refused in the background.",
                            {"app": app, "window_id": window_id, "coordinate": coordinate,
                             "combo": {"type": "string"}}, ["app", "combo"]))
        if "scroll" in self.actions:
            out.append(tool("app_scroll", "Scroll the content at a point within one window of an application" + bg +
                            f" dy > 0 scrolls toward the bottom, dy < 0 toward the top; dx > 0 toward the right. One "
                            f"unit is about {SCROLL_POINTS_PER_UNIT} points.",
                            {"app": app, "window_id": window_id, "coordinate": coordinate, "dy": {"type": "number"},
                             "dx": {"type": "number"}}, ["app", "coordinate"]))
        if "drag" in self.actions:
            out.append(tool("app_drag", "Drag from coordinate to to_coordinate inside one window of an application" + bg,
                            {"app": app, "window_id": window_id, "coordinate": coordinate,
                             "to_coordinate": coordinate}, ["app", "coordinate", "to_coordinate"]))
        if "hover" in self.actions:
            out.append(tool("app_hover", "Move the pointer over a point within one window of an application" + bg +
                            " (hover: tooltips and hover-only controls).",
                            {"app": app, "window_id": window_id, "coordinate": coordinate}, ["app", "coordinate"]))
        return out

    # -------------------------------------------------- targeting

    def window_for(self, app: str, window_id: Any = None) -> dict[str, Any]:
        wins = list_windows(self.winlist)
        if window_id is not None:
            for win in wins:
                if int(win.get("window_id", -1)) == int(window_id):
                    return win
            raise ValueError(f"no on-screen window with window_id {window_id}; call app_list_windows")
        name = (app or "").strip().lower()
        for win in wins:  # front to back: the first match is the app's frontmost window
            if str(win.get("app", "")).lower() == name:
                return win
        raise ValueError(f"no on-screen window of app {app!r}; call app_list_windows")

    def to_window_points(self, wid: int, coord: Any) -> dict[str, float]:
        if not isinstance(coord, (list, tuple)) or len(coord) != 2:
            raise ValueError("coordinate must be [x, y]")
        x, y = float(coord[0]), float(coord[1])
        if wid not in self.frames:
            raise ValueError(f"take an app_screenshot of window {wid} first; coordinates are screenshot pixels")
        w_pt, h_pt, w_px, h_px = self.frames[wid]
        if not (0 <= x < w_px and 0 <= y < h_px):
            raise ValueError(f"point ({x:g}, {y:g}) is outside the {w_px}x{h_px} screenshot of window {wid}")
        return {"x": round(x * w_pt / w_px, 2), "y": round(y * h_pt / h_px, 2)}

    # -------------------------------------------------- tool calls

    def call(self, name: str, args: dict[str, Any]) -> list[dict[str, Any]]:
        if name == "app_list_windows":
            wins = list_windows(self.winlist)
            if args.get("app"):
                wins = [w for w in wins if str(w.get("app", "")).lower() == str(args["app"]).strip().lower()]
            return [{"type": "text", "text": json.dumps(wins)}]
        if name == "app_screenshot":
            win = self.window_for(args.get("app", ""), args.get("window_id"))
            wid = int(win["window_id"])
            try:  # Desktop wakes a Chromium window's compositor through the helper before every capture
                self.helper.call("wakeChromiumCompositor", {"pid": int(win["pid"]), "windowId": wid,
                                                            "hostPid": self.host_pid, "debug": True})
            except HelperError as error:
                log(f"wakeChromiumCompositor: {error}")
            data, width, height = capture_window(wid, self.screencapture, self.sips)
            self.frames[wid] = (float(win["width"]), float(win["height"]), width, height)
            text = (f"{win.get('app')} window {wid} ({win.get('title') or 'untitled'}): screenshot {width}x{height} px; "
                    "coordinates are pixels of this image.")
            return [{"type": "text", "text": text},
                    {"type": "image", "data": base64.b64encode(data).decode("ascii"), "mimeType": "image/png"}]
        handler = {
            "app_click": ("click", self.click), "app_type": ("type", self.type_text), "app_key": ("key", self.key),
            "app_scroll": ("scroll", self.scroll), "app_drag": ("drag", self.drag), "app_hover": ("hover", self.hover),
        }.get(name)
        if handler is None or handler[0] not in self.actions:
            raise ValueError(f"unknown tool {name}")
        win = self.window_for(args.get("app", ""), args.get("window_id"))
        return handler[1](win, args)

    def dispatch(self, win: dict[str, Any], point: dict[str, float] | None, fields: dict[str, Any],
                 focused: bool = False) -> dict[str, Any]:
        params = {"pid": int(win["pid"]), "windowId": int(win["window_id"]),
                  "winLocalPt": point or {"x": round(float(win["width"]) / 2, 2), "y": round(float(win["height"]) / 2, 2)},
                  "focusedTarget": focused, "hostPid": self.host_pid, "nativeMouseVariant": False,
                  "debug": True, **fields}  # debug: refusals carry the helper's blockedReason
        for attempt in range(BUSY_RETRIES + 1):
            result = self.helper.call("dispatchRaw", params)
            if result.get("code") != "user_actively_typing" or attempt == BUSY_RETRIES:
                return result
            time.sleep(self.busy_wait_s)
        return result

    @staticmethod
    def report(what: str, result: dict[str, Any], point: dict[str, float] | None) -> list[dict[str, Any]]:
        info = {"delivered": result.get("delivered"), "path": result.get("path")}
        for key in ("code", "charsDelivered"):
            if key in result:
                info[key] = result[key]
        reason = (result.get("debugFields") or {}).get("blockedReason")
        if reason:
            info["reason"] = reason
        if point:
            info["window_point"] = [point["x"], point["y"]]
        text = json.dumps(info)
        if result.get("delivered") is not True:
            raise RuntimeError(f"the helper did not deliver the {what}: {text}")
        return [{"type": "text", "text": text}]

    def point_of(self, win: dict[str, Any], args: dict[str, Any], required: bool) -> dict[str, float] | None:
        if args.get("coordinate") is None and not required:
            return None
        return self.to_window_points(int(win["window_id"]), args.get("coordinate"))

    def click(self, win: dict[str, Any], args: dict[str, Any]) -> list[dict[str, Any]]:
        button = str(args.get("button") or "left")
        if button not in ("left", "right") or (button == "right" and "right_click" not in self.actions):
            raise ValueError(f"button {button!r} is not available")
        count = int(args.get("count") or 1)
        if count not in (1, 2, 3):
            raise ValueError("count must be 1, 2 or 3")
        point = self.point_of(win, args, True)
        result = self.dispatch(win, point, {"kind": "rclick" if button == "right" else "click", "count": count})
        return self.report("click", result, point)

    def type_text(self, win: dict[str, Any], args: dict[str, Any]) -> list[dict[str, Any]]:
        text = str(args.get("text") or "")
        if not text:
            raise ValueError("text is empty")
        if len(text) > TYPE_MAX_CHARS:
            raise ValueError(f"text is longer than {TYPE_MAX_CHARS} characters")
        point = self.point_of(win, args, False)
        focused = point is None
        if (args.get("mode") or "insert") == "replace":
            sel = self.dispatch(win, point, {"kind": "key", "keyName": "a", "modifiers": ["cmd"], "partOfTextWrite": True},
                                focused)
            if sel.get("delivered") is not True:
                return self.report("select-all before replace", sel, point)
        result = self.dispatch(win, point, {"kind": "type", "text": text}, focused)
        return self.report("text", result, point)

    def key(self, win: dict[str, Any], args: dict[str, Any]) -> list[dict[str, Any]]:
        key, mods = parse_combo(args.get("combo", ""))
        point = self.point_of(win, args, False)
        result = self.dispatch(win, point, {"kind": "key", "keyName": key, "modifiers": mods}, point is None)
        return self.report("key", result, point)

    def scroll(self, win: dict[str, Any], args: dict[str, Any]) -> list[dict[str, Any]]:
        dy, dx = float(args.get("dy") or 0), float(args.get("dx") or 0)
        if not dy and not dx:
            raise ValueError("give dy and/or dx")
        point = self.point_of(win, args, True)
        result = self.dispatch(win, point, {"kind": "scroll", "dx": round(-dx * SCROLL_POINTS_PER_UNIT),
                                            "dy": round(-dy * SCROLL_POINTS_PER_UNIT), "ticks": 1})
        return self.report("scroll", result, point)

    def drag(self, win: dict[str, Any], args: dict[str, Any]) -> list[dict[str, Any]]:
        point = self.point_of(win, args, True)
        to = self.to_window_points(int(win["window_id"]), args.get("to_coordinate"))
        result = self.dispatch(win, point, {"kind": "drag", "toWinLocalPt": to})
        return self.report("drag", result, point)

    def hover(self, win: dict[str, Any], args: dict[str, Any]) -> list[dict[str, Any]]:
        point = self.point_of(win, args, True)
        result = self.dispatch(win, point, {"kind": "hover"})
        return self.report("hover", result, point)


def serve(adapter: Adapter, stdin: Any = None, stdout: Any = None) -> None:
    stdin = stdin or sys.stdin
    stdout = stdout or sys.stdout

    def send(obj: dict[str, Any]) -> None:
        stdout.write(json.dumps(obj) + "\n")
        stdout.flush()

    handlers: dict[str, Callable[[dict[str, Any]], dict[str, Any]]] = {
        "initialize": lambda p: {
            "protocolVersion": p.get("protocolVersion") or "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": SERVER_NAME, "version": ADAPTER_VERSION},
        },
        "tools/list": lambda p: {"tools": adapter.tools()},
        "ping": lambda p: {},
    }
    for raw in stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            continue
        method, ident = msg.get("method"), msg.get("id")
        if ident is None:
            continue  # notifications
        params = msg.get("params") or {}
        if method == "tools/call":
            try:
                content = adapter.call(params.get("name", ""), params.get("arguments") or {})
                send({"jsonrpc": "2.0", "id": ident, "result": {"content": content, "isError": False}})
            except Exception as error:  # noqa: BLE001 - every failure goes back to the model as a tool error
                send({"jsonrpc": "2.0", "id": ident,
                      "result": {"content": [{"type": "text", "text": f"{type(error).__name__}: {error}"}],
                                 "isError": True}})
        elif method in handlers:
            send({"jsonrpc": "2.0", "id": ident, "result": handlers[method](params)})
        else:
            send({"jsonrpc": "2.0", "id": ident, "error": {"code": -32601, "message": f"method not found: {method}"}})
    adapter.helper.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--helper", required=True)
    parser.add_argument("--winlist", required=True)
    parser.add_argument("--launcher", help="cu-disclaim: start the helper as its own responsible process (A11)")
    parser.add_argument("--actions", default=",".join(ALL_ACTIONS))
    parser.add_argument("--screencapture", default=SCREENCAPTURE)
    parser.add_argument("--sips", default=SIPS)
    args = parser.parse_args(argv)
    helper = Helper([args.launcher, args.helper] if args.launcher else [args.helper], env={"PATH": "/usr/bin:/bin", "HOME": os.environ.get("HOME", "/tmp")})
    adapter = Adapter(helper, args.winlist, [a for a in args.actions.split(",") if a],
                      screencapture=args.screencapture, sips=args.sips)
    log(f"adapter {ADAPTER_VERSION} up; helper {args.helper}; actions {adapter.actions}")
    serve(adapter)
    return 0


if __name__ == "__main__":
    sys.exit(main())
