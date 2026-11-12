# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The `raw` runtime of a raw-macos script: macOS low-level APIs with guards, read-back helpers and a runtime hook.

What a script gets (``raw``):

* ``raw.ax``: the Accessibility API on AXUIElement handles (pyobjc ApplicationServices): ``app(pid)``, ``get``,
  ``set``, ``names``, ``actions``, ``perform``, ``children``, ``at(x, y)``. ``set`` and ``perform`` check the
  focus/activation rules with the real attribute or action name.
* Per-process event posting (CGEventPostToPid, Quartz): ``post_click``, ``post_key``, ``post_text``,
  ``post_scroll``. They reach one process and never move the real cursor.
* Foreground pointer routes for a trial with the harness's grant: ``pointer_move``, ``pointer_click``,
  ``pointer_drag`` (CGEventPost at the HID tap: the real cursor moves).
* Read-back helpers only: ``apps``, ``windows``, ``front``, ``cursor``, ``find``, ``describe``, ``dump``,
  ``screenshot``.
* ``raw.driver(tool, args)``: the main build's Cua Driver over its daemon socket, the fallback for what AX does not
  reach.

There is no click-by-name or type-by-name helper: every action is the script's own AX or event call.

The hook (``Watch``) samples the frontmost app, the top on-screen window, the focused app's focused window and
element, and the real cursor every 50 ms while the script runs, and reports every change.
"""

from __future__ import annotations

import base64
import json
import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import AppKit  # noqa: F401 - NSWorkspace, NSPasteboard (read only)
import ApplicationServices as AX
import Quartz

import rawlint_mac as lint

AX.AXUIElementSetMessagingTimeout(AX.AXUIElementCreateSystemWide(), 3.0)


class Blocked(RuntimeError):
    """A guarded call the background rules refuse at run time."""


class State:
    def __init__(self) -> None:
        self.granted = os.environ.get("RAW_GRANT") == "1"
        # apps the harness names as the user's (the bench's "User activity" window): posted input there is MB4
        self.user_apps = {a for a in (os.environ.get("RAW_USER_APPS") or "").split(",") if a}
        self.driver_calls: list[dict[str, Any]] = []
        self.raw_actions: dict[str, int] = {}  # AX actions/sets and posted events per app name
        self.runtime_blocked: list[str] = []
        self.runtime_warned: list[str] = []
        self.images: list[Path] = []
        self.image_dir: Path | None = None
        self.lock = threading.Lock()

    def refuse(self, rule_id: str, what: str) -> None:
        rule = lint.RULES[rule_id]
        text = (
            f"BLOCKED at run time [{rule.id} {rule.title}]: {what}. {rule.why} Source: {rule.source}. "
            f"Instead: {rule.instead}"
        )
        with self.lock:
            self.runtime_blocked.append(f"{rule.id}: {what}")
        raise Blocked(text)

    def check(self, rule_id: str | None, what: str) -> None:
        if rule_id is None:
            return
        rule = lint.RULES[rule_id]
        if rule.severity == "block" or (rule.severity == "granted" and not self.granted):
            self.refuse(rule_id, what)
        if rule.severity == "warn" or (rule.severity == "granted" and self.granted):
            with self.lock:
                self.runtime_warned.append(f"{rule.id}: {what}")

    def to_user(self, pid: int, what: str) -> None:
        if app_name(pid) in self.user_apps:
            self.check("MB4", f"{what} to {app_name(pid)}, the user's app")

    def count(self, pid: int | None) -> None:
        name = app_name(pid) if pid else "?"
        with self.lock:
            self.raw_actions[name] = self.raw_actions.get(name, 0) + 1


STATE = State()
WS = AppKit.NSWorkspace.sharedWorkspace()


def app_name(pid: int | None) -> str:
    if not pid:
        return "?"
    app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(int(pid))
    return str(app.localizedName()) if app is not None else f"pid {pid}"


# ------------------------------------------------------------------ observation (also the hook's sampler)

ONSCREEN = Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements


def window_list() -> list[dict[str, Any]]:
    """On-screen windows, front to back: CGWindowList's order."""
    out = []
    for w in Quartz.CGWindowListCopyWindowInfo(ONSCREEN, Quartz.kCGNullWindowID) or []:
        b = w.get("kCGWindowBounds") or {}
        out.append(
            {
                "window_id": int(w.get("kCGWindowNumber", 0)),
                "pid": int(w.get("kCGWindowOwnerPID", 0)),
                "app": str(w.get("kCGWindowOwnerName") or ""),
                "title": str(w.get("kCGWindowName") or ""),
                "layer": int(w.get("kCGWindowLayer", 0)),
                "frame": (
                    int(b.get("X", 0)),
                    int(b.get("Y", 0)),
                    int(b.get("Width", 0)),
                    int(b.get("Height", 0)),
                ),
            }
        )
    return out


def _ax_get(el: Any, attr: str) -> Any:
    err, value = AX.AXUIElementCopyAttributeValue(el, attr, None)
    return value if err == 0 else None


def front_pid() -> int:
    """The frontmost app's pid as WindowServer sees it: the owner of the top layer-0 window, cross-checked by AX."""
    sw = AX.AXUIElementCreateSystemWide()
    app = _ax_get(sw, "AXFocusedApplication")
    if app is not None:
        err, pid = AX.AXUIElementGetPid(app, None)
        if err == 0:
            return int(pid)
    top = next((w for w in window_list() if w["layer"] == 0), None)
    return top["pid"] if top else 0


def cursor() -> tuple[int, int]:
    p = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    return int(round(p.x)), int(round(p.y))


def focus_sample() -> dict[str, Any]:
    """Frontmost app, its focused window title and focused element role, the top layer-0 window, the cursor."""
    pid = front_pid()
    win_title = role = None
    if pid:
        app = AX.AXUIElementCreateApplication(pid)
        win = _ax_get(app, "AXFocusedWindow")
        if win is not None:
            win_title = str(_ax_get(win, "AXTitle") or "")
        el = _ax_get(app, "AXFocusedUIElement")
        if el is not None:
            role = str(_ax_get(el, "AXRole") or "")
    top = next((w for w in window_list() if w["layer"] == 0), None)
    return {
        "front": pid,
        "key_window": win_title,
        "focused": role,
        "top": (top["pid"], top["title"]) if top else None,
        "cursor": cursor(),
    }


class Watch:
    """Samples the frontmost app, key window, focused element, top window and the real cursor every 50 ms."""

    KEYS = ("front", "key_window", "focused", "top", "cursor")

    def __init__(self, interval: float = 0.05) -> None:
        self.interval = interval
        self.before = focus_sample()
        self.changes: dict[str, list[Any]] = {k: [] for k in self.KEYS}
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.loop, daemon=True)

    def loop(self) -> None:
        last = dict(self.before)
        while not self.stop.wait(self.interval):
            try:
                now = focus_sample()
            except Exception:  # noqa: BLE001 - a sample error must not kill the script
                continue
            for key in self.KEYS:
                if now[key] != last[key]:
                    self.changes[key].append(now[key])
            last = now

    def start(self) -> "Watch":
        self.thread.start()
        return self

    def finish(self) -> dict[str, Any]:
        self.stop.set()
        self.thread.join(timeout=1)
        after = focus_sample()
        for key in self.KEYS:
            if after[key] != self.before[key] and (
                not self.changes[key] or self.changes[key][-1] != after[key]
            ):
                self.changes[key].append(after[key])
        moved = {k: v for k, v in self.changes.items() if v}
        b = self.before
        lines = []
        if moved.get("front"):
            seen = ", ".join(app_name(p) for p in dict.fromkeys(moved["front"]))
            lines.append(f"frontmost app changed from {app_name(b['front'])} (seen: {seen})")
        if moved.get("key_window") and not moved.get("front"):
            lines.append(
                f"the front app's key window changed from {b['key_window']!r} to {after['key_window']!r}"
            )
        if moved.get("focused") and not moved.get("front"):
            lines.append(
                f"the front app's focused element changed from {b['focused']!r} to {after['focused']!r}"
            )
        if moved.get("top"):
            tops = ", ".join(
                f"{app_name(p)} {t!r}"
                for p, t in dict.fromkeys(tuple(x) for x in moved["top"] if x)
            )
            lines.append(
                f"a window was raised above {app_name(b['top'][0]) if b['top'] else 'none'}'s (seen on top: {tops})"
            )
        if moved.get("cursor"):
            lines.append(
                f"real cursor moved from {b['cursor']} to {after['cursor']} ({len(moved['cursor'])} sample(s))"
            )
        return {"before": b, "after": after, "changed": sorted(moved), "lines": lines}


# ------------------------------------------------------------------ AX


# Roles whose press opens a menu (MB6 for the menu bar, MB7 otherwise).
MENU_OPENERS = ("AXPopUpButton", "AXMenuBarItem", "AXMenuButton")


class _AX:
    """AXUIElement calls. Elements are pyobjc AXUIElementRef handles; attribute values come back as Python values."""

    def app(self, pid: int) -> Any:
        return AX.AXUIElementCreateApplication(int(pid))

    def system(self) -> Any:
        return AX.AXUIElementCreateSystemWide()

    def get(self, el: Any, attr: str) -> Any:
        """One attribute's value, or None. Points, sizes and ranges come back as tuples."""
        value = _ax_get(el, str(attr))
        return _py(value)

    def names(self, el: Any) -> list[str]:
        err, names = AX.AXUIElementCopyAttributeNames(el, None)
        return list(names or []) if err == 0 else []

    def settable(self, el: Any, attr: str) -> bool:
        err, ok = AX.AXUIElementIsAttributeSettable(el, str(attr), None)
        return bool(ok) if err == 0 else False

    def actions(self, el: Any) -> list[str]:
        err, names = AX.AXUIElementCopyActionNames(el, None)
        return list(names or []) if err == 0 else []

    def children(self, el: Any) -> list[Any]:
        return list(_ax_get(el, "AXChildren") or [])

    def at(self, x: float, y: float) -> Any:
        """The element at a screen point (AXUIElementCopyElementAtPosition on the system-wide element)."""
        err, el = AX.AXUIElementCopyElementAtPosition(self.system(), float(x), float(y), None)
        return el if err == 0 else None

    def pid(self, el: Any) -> int:
        err, pid = AX.AXUIElementGetPid(el, None)
        return int(pid) if err == 0 else 0

    def set(self, el: Any, attr: str, value: Any) -> int:
        """Write one attribute (AXUIElementSetAttributeValue); returns the AX error code (0 is success)."""
        attr = str(attr)
        STATE.check(lint.AX_SET.get(attr), f"raw.ax.set {attr}")
        STATE.count(self.pid(el))
        return int(AX.AXUIElementSetAttributeValue(el, attr, value))

    def perform(self, el: Any, action: str) -> int:
        """Perform one AX action (AXPress, AXIncrement, AXConfirm, ...); returns the AX error code."""
        action = str(action)
        STATE.check(lint.AX_ACTION.get(action), f"raw.ax.perform {action}")
        if action == "AXPress" and _ax_get(el, "AXRole") in (
            "AXPopUpButton",
            "AXMenuBarItem",
            "AXMenuButton",
        ):
            rule = "MB6" if _ax_get(el, "AXRole") == "AXMenuBarItem" else "MB7"
            STATE.check(rule, f"AXPress on {_ax_get(el, 'AXRole')}")
        STATE.count(self.pid(el))
        return int(AX.AXUIElementPerformAction(el, action))


def _py(value: Any) -> Any:
    """AXValue structs to tuples; leave everything else as pyobjc gives it."""
    if value is None:
        return None
    try:
        type_id = AX.AXValueGetType(value)
    except Exception:  # noqa: BLE001 - not an AXValue
        return value
    for kind, shape in (
        (AX.kAXValueCGPointType, lambda v: (v.x, v.y)),
        (AX.kAXValueCGSizeType, lambda v: (v.width, v.height)),
        (AX.kAXValueCGRectType, lambda v: (v.origin.x, v.origin.y, v.size.width, v.size.height)),
        (AX.kAXValueCFRangeType, lambda v: (v.location, v.length)),
    ):
        if type_id == kind:
            ok, struct = AX.AXValueGetValue(value, kind, None)
            return shape(struct) if ok else None
    return value


# ------------------------------------------------------------------ events

MOUSE = {
    "left": (Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp, Quartz.kCGMouseButtonLeft),
    "right": (
        Quartz.kCGEventRightMouseDown,
        Quartz.kCGEventRightMouseUp,
        Quartz.kCGMouseButtonRight,
    ),
}
KEYCODES = {
    **{c: k for c, k in zip("asdfhgzxcv", (0, 1, 2, 3, 4, 5, 6, 7, 8, 9))},
    "b": 11,
    "q": 12,
    "w": 13,
    "e": 14,
    "r": 15,
    "y": 16,
    "t": 17,
    "1": 18,
    "2": 19,
    "3": 20,
    "4": 21,
    "6": 22,
    "5": 23,
    "=": 24,
    "9": 25,
    "7": 26,
    "-": 27,
    "8": 28,
    "0": 29,
    "]": 30,
    "o": 31,
    "u": 32,
    "[": 33,
    "i": 34,
    "p": 35,
    "l": 37,
    "j": 38,
    "'": 39,
    "k": 40,
    ";": 41,
    "\\": 42,
    ",": 43,
    "/": 44,
    "n": 45,
    "m": 46,
    ".": 47,
    "`": 50,
    "return": 36,
    "enter": 36,
    "tab": 48,
    "space": 49,
    "delete": 51,
    "backspace": 51,
    "escape": 53,
    "esc": 53,
    "left": 123,
    "right": 124,
    "down": 125,
    "up": 126,
    "home": 115,
    "end": 119,
    "pageup": 116,
    "pagedown": 121,
    "forwarddelete": 117,
}
FLAGS = {
    "cmd": Quartz.kCGEventFlagMaskCommand,
    "command": Quartz.kCGEventFlagMaskCommand,
    "shift": Quartz.kCGEventFlagMaskShift,
    "alt": Quartz.kCGEventFlagMaskAlternate,
    "option": Quartz.kCGEventFlagMaskAlternate,
    "ctrl": Quartz.kCGEventFlagMaskControl,
    "control": Quartz.kCGEventFlagMaskControl,
}


def _window_of_point(pid: int, x: float, y: float) -> int:
    for w in window_list():
        fx, fy, fw, fh = w["frame"]
        if w["pid"] == pid and w["layer"] == 0 and fx <= x < fx + fw and fy <= y < fy + fh:
            return w["window_id"]
    return 0


def _mouse_event(kind: int, x: float, y: float, button: int, pid: int | None = None) -> Any:
    ev = Quartz.CGEventCreateMouseEvent(None, kind, (float(x), float(y)), button)
    if pid:
        wid = _window_of_point(pid, x, y)
        if wid:
            # route to that window, as a click under the pointer would be
            Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGMouseEventWindowUnderMousePointer, wid)
            Quartz.CGEventSetIntegerValueField(
                ev, Quartz.kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent, wid
            )
    return ev


class Raw:
    Blocked = Blocked

    def __init__(self) -> None:
        self.ax = _AX()

    # ---------------------------------------------------------------- reads

    def apps(self) -> list[dict[str, Any]]:
        """Regular (Dock) apps: pid, name, bundle id, whether active, whether hidden."""
        out = []
        for a in WS.runningApplications():
            if a.activationPolicy() != AppKit.NSApplicationActivationPolicyRegular:
                continue
            out.append(
                {
                    "pid": int(a.processIdentifier()),
                    "name": str(a.localizedName() or ""),
                    "bundle_id": str(a.bundleIdentifier() or ""),
                    "active": bool(a.isActive()),
                    "hidden": bool(a.isHidden()),
                }
            )
        return out

    def windows(self, pid: int | None = None) -> list[dict[str, Any]]:
        """On-screen windows front to back (layer 0 are normal windows): window_id, pid, app, title, frame."""
        return [w for w in window_list() if pid is None or w["pid"] == int(pid)]

    def front(self) -> dict[str, Any]:
        """The frontmost app and its key window: what the user is using."""
        s = focus_sample()
        return {
            "pid": s["front"],
            "app": app_name(s["front"]),
            "key_window": s["key_window"],
            "focused": s["focused"],
        }

    def cursor(self) -> tuple[int, int]:
        return cursor()

    def find(
        self,
        root: Any,
        role: str | None = None,
        title: str | None = None,
        contains: str | None = None,
        limit: int = 200,
        depth: int = 40,
    ) -> list[Any]:
        """Descendants of ``root`` matching an AXRole ("AXButton"), an exact title/description/value or a
        substring of one."""
        out: list[Any] = []

        def text(el: Any) -> list[str]:
            return [
                str(v)
                for v in (
                    _ax_get(el, a)
                    for a in (
                        "AXTitle",
                        "AXDescription",
                        "AXValue",
                        "AXIdentifier",
                        "AXHelp",
                        "AXPlaceholderValue",
                    )
                )
                if v not in (None, "")
            ]

        def walk(el: Any, d: int) -> None:
            if el is None or len(out) >= limit or d > depth:
                return
            r = _ax_get(el, "AXRole")
            ok = role is None or r == role
            if ok and (title is not None or contains is not None):
                names = text(el)
                ok = (title is None or title in names) and (
                    contains is None or any(contains.casefold() in n.casefold() for n in names)
                )
            if ok and d:
                out.append(el)
            for child in (_ax_get(el, "AXChildren") or [])[:MAX_CHILDREN]:
                walk(child, d + 1)

        walk(root, 0)
        return out

    def describe(self, el: Any) -> str:
        """One line: role/subrole, title, description, value, enabled/focused/selected, frame, actions."""
        if el is None:
            return "<none>"
        role = _ax_get(el, "AXRole")
        if role is None:
            return "<gone>"
        parts = [str(role) + (f"/{_ax_get(el, 'AXSubrole')}" if _ax_get(el, "AXSubrole") else "")]
        for attr, label in (
            ("AXTitle", ""),
            ("AXDescription", "desc="),
            ("AXValue", "value="),
            ("AXHelp", "help="),
            ("AXPlaceholderValue", "placeholder="),
            ("AXIdentifier", "id="),
        ):
            v = _ax_get(el, attr)
            if v not in (None, ""):
                s = str(v)
                parts.append(f"{label}{s[:120]!r}")
        flags = [n for n, a in (("disabled", "AXEnabled"),) if _ax_get(el, a) is False]
        flags += [
            n
            for n, a in (
                ("focused", "AXFocused"),
                ("selected", "AXSelected"),
                ("expanded", "AXExpanded"),
            )
            if _ax_get(el, a) is True
        ]
        if flags:
            parts.append(",".join(flags))
        pos, size = _py(_ax_get(el, "AXPosition")), _py(_ax_get(el, "AXSize"))
        if pos and size:
            parts.append(f"@({int(pos[0])},{int(pos[1])} {int(size[0])}x{int(size[1])})")
        actions = [a for a in self.ax.actions(el) if a not in ("AXShowMenu", "AXScrollToVisible")]
        if actions:
            parts.append("actions=" + "/".join(a.removeprefix("AX") for a in actions))
        return " ".join(parts)

    def dump(self, root: Any, depth: int = 14, limit: int = 400, menus: bool = False) -> str:
        """An indented text tree of ``root`` (``describe`` per node). The app's menu bar is left out unless
        ``menus=True``."""
        lines: list[str] = []

        def walk(el: Any, level: int) -> None:
            if el is None or len(lines) >= limit or level > depth:
                return
            if not menus and level and _ax_get(el, "AXRole") == "AXMenuBar":
                return
            lines.append("  " * level + self.describe(el))
            kids = _ax_get(el, "AXChildren") or []
            if len(kids) > MAX_CHILDREN:
                lines.append(
                    "  " * (level + 1)
                    + f"... {len(kids)} children (only the first {MAX_CHILDREN} are walked)"
                )
            for child in kids[:MAX_CHILDREN]:
                if len(lines) >= limit:
                    return
                walk(child, level + 1)

        walk(root, 0)
        if len(lines) >= limit:
            lines.append(f"... cut at {limit} nodes")
        return "\n".join(lines)

    def screenshot(
        self, window_id: int | None = None, region: tuple[int, int, int, int] | None = None
    ) -> str:
        """A PNG of one window (even behind others), a screen region, or the main display. It is returned to you as
        an image with the result."""
        if STATE.image_dir is None:
            raise RuntimeError("no image directory")
        if window_id is not None:
            img = Quartz.CGWindowListCreateImage(
                Quartz.CGRectNull,
                Quartz.kCGWindowListOptionIncludingWindow,
                int(window_id),
                Quartz.kCGWindowImageBoundsIgnoreFraming | Quartz.kCGWindowImageNominalResolution,
            )
        else:
            rect = (
                Quartz.CGRectInfinite
                if region is None
                else Quartz.CGRectMake(*[float(v) for v in region])
            )
            img = Quartz.CGWindowListCreateImage(
                rect,
                Quartz.kCGWindowListOptionOnScreenOnly,
                Quartz.kCGNullWindowID,
                Quartz.kCGWindowImageNominalResolution,
            )
        if img is None:
            raise RuntimeError(f"no pixels for window {window_id} / region {region}")
        path = STATE.image_dir / f"shot-{len(STATE.images) + 1}.png"
        url = AppKit.NSURL.fileURLWithPath_(str(path))
        dest = Quartz.CGImageDestinationCreateWithURL(url, "public.png", 1, None)
        Quartz.CGImageDestinationAddImage(dest, img, None)
        Quartz.CGImageDestinationFinalize(dest)
        STATE.images.append(path)
        return f"screenshot {len(STATE.images)} ({Quartz.CGImageGetWidth(img)}x{Quartz.CGImageGetHeight(img)})"

    # ---------------------------------------------------------------- per-process events (background)

    def post_click(self, pid: int, x: float, y: float, button: str = "left", count: int = 1) -> str:
        """Post a mouse down/up at screen point (x, y) to process ``pid`` only (CGEventPostToPid). The real cursor
        does not move. Many apps ignore clicks while in the background; prefer an AX action and read back."""
        pid = int(pid)
        STATE.to_user(pid, "posted click")
        if button == "right":
            STATE.check("MB7", "posted right-click (opens a context menu)")
        hit = self.ax.at(x, y)
        role = _ax_get(hit, "AXRole") if hit is not None else None
        if role in MENU_OPENERS:
            STATE.check("MB6" if role == "AXMenuBarItem" else "MB7", f"posted click on {role}")
        down, up, btn = MOUSE[button]
        for n in range(1, int(count) + 1):
            for kind in (down, up):
                ev = _mouse_event(kind, x, y, btn, pid)
                Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGMouseEventClickState, n)
                Quartz.CGEventPostToPid(pid, ev)
                time.sleep(0.01)
        STATE.count(pid)
        return f"posted {button} click x{count} at ({x}, {y}) to {app_name(pid)}"

    def post_key(self, pid: int, keys: list[str] | str) -> str:
        """Post one key or combo (["cmd", "s"], "return") to process ``pid`` only (CGEventPostToPid)."""
        pid = int(pid)
        STATE.to_user(pid, "posted keys")
        combo = [keys] if isinstance(keys, str) else [str(k) for k in keys]
        lowered = [k.lower() for k in combo]
        if lint.combo(lowered) in lint.FOCUS_COMBOS:
            STATE.check("MB9", "keys " + "+".join(lowered))
        flags = 0
        for k in lowered[:-1]:
            if k not in FLAGS:
                raise ValueError(f"unknown modifier {k!r}; use one of {sorted(FLAGS)}")
            flags |= FLAGS[k]
        key = lowered[-1]
        if key not in KEYCODES:
            raise ValueError(f"unknown key {key!r}; for text use raw.post_text or set AXValue")
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(None, KEYCODES[key], down)
            Quartz.CGEventSetFlags(ev, flags)
            Quartz.CGEventPostToPid(pid, ev)
            time.sleep(0.01)
        STATE.count(pid)
        return f"posted {'+'.join(combo)} to {app_name(pid)}"

    def post_text(self, pid: int, text: str) -> str:
        """Type text into process ``pid``'s focused field as unicode key events (CGEventPostToPid). Setting the
        field's AXValue is usually simpler and does not depend on which field has focus."""
        pid = int(pid)
        STATE.to_user(pid, "posted text")
        for ch in str(text):
            for down in (True, False):
                ev = Quartz.CGEventCreateKeyboardEvent(None, 0, down)
                Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
                Quartz.CGEventPostToPid(pid, ev)
            time.sleep(0.005)
        STATE.count(pid)
        return f"posted {len(text)} characters to {app_name(pid)}"

    def post_scroll(self, pid: int, x: float, y: float, dy: int, dx: int = 0) -> str:
        """Post a scroll wheel event (lines) at (x, y) to process ``pid`` only."""
        pid = int(pid)
        STATE.to_user(pid, "posted scroll")
        ev = Quartz.CGEventCreateScrollWheelEvent(
            None, Quartz.kCGScrollEventUnitLine, 2, int(dy), int(dx)
        )
        Quartz.CGEventSetLocation(ev, (float(x), float(y)))
        Quartz.CGEventPostToPid(pid, ev)
        STATE.count(pid)
        return f"posted scroll ({dx}, {dy}) at ({x}, {y}) to {app_name(pid)}"

    # ---------------------------------------------------------------- foreground pointer (granted trials only)

    def pointer_move(self, x: float, y: float) -> str:
        """Move the real cursor to (x, y) (CGEventPost at the HID tap). Foreground: only on a trial that grants it
        (a real hover or tooltip)."""
        STATE.check("MB1", "raw.pointer_move")
        ev = Quartz.CGEventCreateMouseEvent(
            None, Quartz.kCGEventMouseMoved, (float(x), float(y)), 0
        )
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
        STATE.count(front_pid())
        return f"moved the real cursor to ({x}, {y})"

    def pointer_click(self, x: float, y: float, button: str = "left") -> str:
        """Move the real cursor to (x, y) and click there (HID tap). Foreground: granted trials only."""
        STATE.check("MB1", "raw.pointer_click")
        down, up, btn = MOUSE[button]
        Quartz.CGEventPost(
            Quartz.kCGHIDEventTap,
            Quartz.CGEventCreateMouseEvent(
                None, Quartz.kCGEventMouseMoved, (float(x), float(y)), 0
            ),
        )
        time.sleep(0.05)
        for kind in (down, up):
            Quartz.CGEventPost(
                Quartz.kCGHIDEventTap,
                Quartz.CGEventCreateMouseEvent(None, kind, (float(x), float(y)), btn),
            )
            time.sleep(0.03)
        STATE.count(front_pid())
        return f"clicked the real cursor at ({x}, {y})"

    def pointer_drag(
        self, x0: float, y0: float, x1: float, y1: float, steps: int = 20, hold_s: float = 0.3
    ) -> str:
        """A real drag-and-drop with the real cursor from (x0, y0) to (x1, y1) (HID tap). Foreground: granted
        trials only. macOS tracks drag-and-drop sessions with the real pointer, so a list reorder needs this."""
        STATE.check("MB1", "raw.pointer_drag")
        tap = Quartz.kCGHIDEventTap
        Quartz.CGEventPost(
            tap,
            Quartz.CGEventCreateMouseEvent(
                None, Quartz.kCGEventMouseMoved, (float(x0), float(y0)), 0
            ),
        )
        time.sleep(0.05)
        Quartz.CGEventPost(
            tap,
            Quartz.CGEventCreateMouseEvent(
                None, Quartz.kCGEventLeftMouseDown, (float(x0), float(y0)), 0
            ),
        )
        time.sleep(float(hold_s))
        n = max(2, int(steps))
        for i in range(1, n + 1):
            x = x0 + (x1 - x0) * i / n
            y = y0 + (y1 - y0) * i / n
            Quartz.CGEventPost(
                tap,
                Quartz.CGEventCreateMouseEvent(
                    None, Quartz.kCGEventLeftMouseDragged, (float(x), float(y)), 0
                ),
            )
            time.sleep(0.02)
        time.sleep(float(hold_s))
        Quartz.CGEventPost(
            tap,
            Quartz.CGEventCreateMouseEvent(
                None, Quartz.kCGEventLeftMouseUp, (float(x1), float(y1)), 0
            ),
        )
        STATE.count(front_pid())
        return f"dragged the real cursor from ({x0}, {y0}) to ({x1}, {y1})"

    # ---------------------------------------------------------------- the Cua Driver fallback

    def driver(self, tool: str, args: dict | None = None, image: bool = False) -> Any:
        """Call one Cua Driver tool (GUI reads and actions only) and return its JSON result. Screenshots in the
        result are dropped unless ``image=True``, which returns them to you as images."""
        tool = str(tool)
        args = dict(args or {})
        if tool not in lint.DRIVER_TOOLS:
            STATE.refuse("GUI1", f"raw.driver({tool!r})")
        why = lint.driver_foreground(tool, args)
        if why:
            STATE.check("MB10", f"raw.driver: {why}")
        if tool in ("press_key", "hotkey"):
            keys = args.get("keys") or ([args["key"]] if args.get("key") else [])
            if lint.combo([str(k).lower() for k in keys]) in lint.FOCUS_COMBOS:
                STATE.check("MB9", f"raw.driver {tool} " + "+".join(map(str, keys)))
        socket, session, binary = (
            os.environ.get(k) for k in ("RAW_DRIVER_SOCKET", "RAW_DRIVER_SESSION", "RAW_DRIVER_BIN")
        )
        if not (socket and binary):
            raise RuntimeError("no Cua Driver is configured for this trial")
        args.setdefault("session", session or "raw-macos")
        entry: dict[str, Any] = {"tool": tool, "pid": args.get("pid"), "foreground": bool(why)}
        shot = (
            STATE.image_dir / f"drv-{len(STATE.driver_calls) + 1}.png"
            if (image and STATE.image_dir)
            else None
        )
        argv = [
            binary,
            "call",
            "--socket",
            socket,
            *(["--screenshot-out-file", str(shot)] if shot else []),
            tool,
            json.dumps(args),
        ]
        started = time.monotonic()
        try:
            done = subprocess.run(argv, capture_output=True, text=True, timeout=90)
        finally:
            entry["elapsed_s"] = round(time.monotonic() - started, 3)
        text = done.stdout.strip()
        try:
            result: Any = json.loads(text) if text else {}
        except ValueError:
            result = {"text": text[-6000:]}
        entry["ok"] = done.returncode == 0
        if isinstance(result, dict):
            for key in [k for k in result if k.endswith("_png_b64") or k.endswith("image_b64")]:
                result.pop(key)
        if shot is not None and shot.is_file():
            STATE.images.append(shot)
            if isinstance(result, dict):
                result["screenshot"] = f"returned as image {len(STATE.images)}"
        with STATE.lock:
            STATE.driver_calls.append(entry)
        if done.returncode != 0:
            raise RuntimeError(f"cua-driver {tool} failed: {(done.stderr.strip() or text)[-1500:]}")
        return result

    def sleep(self, seconds: float) -> None:
        time.sleep(min(float(seconds), 30.0))


# An app's AX tree can be huge (a spreadsheet table lists every cell); the helpers walk at most this many children
# of one node, so a tree read ends. A script's own loop is the script's business.
MAX_CHILDREN = 1000


def make() -> Raw:
    return Raw()


def b64(path: Path) -> str:
    return base64.b64encode(path.read_bytes()).decode()
