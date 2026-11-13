# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The `raw` runtime of a raw-macos script: macOS low-level APIs with guards, read-back helpers and a runtime hook.

What a script gets (``raw``):

* ``raw.ax``: the Accessibility API on AXUIElement handles (pyobjc ApplicationServices): ``app(pid)``, ``get``,
  ``set``, ``names``, ``actions``, ``perform``, ``children``, ``at(x, y)``. ``set`` and ``perform`` check the
  focus/activation rules with the real attribute or action name.
* Per-process event posting (CGEventPostToPid, Quartz): ``post_click``, ``post_key``, ``post_text``,
  ``post_scroll``. They reach one process and never move the real cursor. ``post_key`` and ``post_text`` with
  ``window_id=`` first make that window its app's key window without activating or raising it (SkyLight focus
  records) and post through SLEventPostToPid, the route Cua Driver uses for apps such as LibreOffice that ignore
  plain per-process keys while in the background.
* Foreground pointer routes for a trial with the harness's grant: ``pointer_move``, ``pointer_click``,
  ``pointer_drag`` (CGEventPost at the HID tap: the real cursor moves).
* Read-back helpers: ``apps``, ``windows``, ``front``, ``cursor``, ``find``, ``describe``, ``dump`` (compact, or
  scoped to a query), ``text`` (an element's visible text only), ``screenshot``.
* Value helpers built only from the calls above, one per control kind the AX value write alone does not reach:
  ``set_text`` (a text field), ``select`` (a native pop-up, guarded in the background, or a web ``<select>``),
  ``set_cell`` (a LibreOffice Calc cell, typed through the window-key route), ``menu`` (a menu-bar command without
  opening the menu). Each reads the control back and says how it was set.

Nothing here calls Cua Driver: only macOS's own APIs, public and private (ApplicationServices, Quartz, AppKit through
pyobjc, and SkyLight through ctypes). Zane, 13 Nov 2026 13:38 SGT: any native macOS API counts as raw; only Cua
Driver is banned (rule DRV1).

The hook (``Watch``) samples the frontmost app, the top on-screen window, the focused app's focused window and
element, and the real cursor every 50 ms while the script runs, and reports every change.
"""

from __future__ import annotations

import base64
import ctypes
import json
import os
import re
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

import AppKit  # noqa: F401 - NSWorkspace, NSPasteboard (read only)
import ApplicationServices as AX
import Quartz

import rawlint_mac as lint

AX.AXUIElementSetMessagingTimeout(AX.AXUIElementCreateSystemWide(), 3.0)
# pyobjc loads framework names lazily, and a first lookup from two threads at once (the hook's sampler and the script,
# or the key-focus guard) raised KeyError('AXUIElementGetPid') in the bench guest; resolve the shared ones up front
for _name in (
    "AXUIElementGetPid",
    "AXUIElementCopyAttributeValue",
    "AXUIElementCreateApplication",
    "AXUIElementPerformAction",
):
    getattr(AX, _name)
for _name in ("CGWindowListCopyWindowInfo", "CGEventGetLocation", "CGEventCreate"):
    getattr(Quartz, _name)


class Blocked(RuntimeError):
    """A guarded call the background rules refuse at run time."""


class State:
    def __init__(self) -> None:
        self.granted = os.environ.get("RAW_GRANT") == "1"
        # apps the harness names as the user's (the bench's "User activity" window): posted input there is MB4
        self.user_apps = {a for a in (os.environ.get("RAW_USER_APPS") or "").split(",") if a}
        self.routes: dict[str, int] = {}  # which route each value helper used
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

    def route(self, name: str) -> None:
        with self.lock:
            self.routes[name] = self.routes.get(name, 0) + 1


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


# Written into the call's directory when the key-focus guard opens a menu (read by raw_mcp_mac when the runner does not
# return a complete result).
GUARD_MARKER = "key-focus-guard-open"
# Disturbances a helper saw and undid itself (the key-focus guard's window restores), for the hook's report.
WATCH_NOTES: list[str] = []


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
        # what the key-focus guard saw and undid between two samples is a change too
        lines = list(WATCH_NOTES)
        changed = set(moved) | ({"top"} if WATCH_NOTES else set())
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
        return {"before": b, "after": after, "changed": sorted(changed), "lines": lines}


def _has_web_area(el: Any, depth: int = 0, budget: list[int] | None = None) -> bool:
    budget = budget if budget is not None else [400]
    if el is None or depth > 12 or budget[0] <= 0:
        return False
    budget[0] -= 1
    if _ax_get(el, "AXRole") == "AXWebArea":
        return bool(_ax_get(el, "AXChildren"))
    return any(_has_web_area(c, depth + 1, budget) for c in (_ax_get(el, "AXChildren") or [])[:50])


def _enable_web_tree(app: Any) -> None:
    """Chromium ships its web-content AX tree off until an assistive client asks (enablement.rs:4-11). Setting the
    opt-in has no focus or screen-reader side effect (bindings.rs:835-838)."""
    err = AX.AXUIElementSetAttributeValue(app, "AXManualAccessibility", True)
    if err == AX.kAXErrorAttributeUnsupported:
        err = AX.AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface", True)
    if err != 0:
        return  # not a Chromium app, or it refused: nothing to wait for
    deadline = time.monotonic() + 2.0
    windows = _ax_get(app, "AXWindows") or []
    while windows and time.monotonic() < deadline:
        if any(_has_web_area(w) for w in windows):
            return
        time.sleep(0.1)


# ------------------------------------------------------------------ AX


# Roles whose press opens a menu (MB6 for the menu bar, MB7 otherwise).
MENU_OPENERS = ("AXPopUpButton", "AXMenuBarItem", "AXMenuButton")


class _AX:
    """AXUIElement calls. Elements are pyobjc AXUIElementRef handles; attribute values come back as Python values."""

    def app(self, pid: int) -> Any:
        """The app element. For a Chromium/Electron app this also asks it to build its web-content tree, as an
        assistive client does (AXManualAccessibility, else AXEnhancedUserInterface; Cua Driver's
        platform-macos/src/ax/enablement.rs), and waits up to 2 s for the tree to appear."""
        el = AX.AXUIElementCreateApplication(int(pid))
        _enable_web_tree(el)
        return el

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
        (
            AX.kAXValueCFRangeType,
            lambda v: tuple(v) if isinstance(v, tuple) else (v.location, v.length),
        ),
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

    def dump(
        self,
        root: Any,
        depth: int = 14,
        limit: int = 400,
        menus: bool = False,
        full: bool = False,
        query: str | None = None,
    ) -> str:
        """An indented text tree of ``root``. Compact by default: unlabelled wrapper groups are skipped (their
        children move up), text is one quoted line, and every element you can act on gets a ref ``[n]`` that
        ``raw.el(n)`` returns in this or a later call. ``query`` keeps only the lines whose text contains it (with
        refs). ``full=True`` is the old one-``describe``-per-node tree. The menu bar is left out unless
        ``menus=True``."""
        lines: list[str] = []
        refs = _Refs.load()
        pid = self.ax.pid(root)
        base = _path_of(root, pid)
        needle = query.casefold() if query else None

        def walk(el: Any, level: int, path: list[int]) -> None:
            if el is None or len(lines) >= limit or level > depth:
                return
            role = _ax_get(el, "AXRole")
            if not menus and path and role == "AXMenuBar":
                return
            kids = _ax_get(el, "AXChildren") or []
            skip = not full and path and _is_wrapper(el, role)
            if not skip:
                line = self.describe(el) if full else _compact(el, role)
                if not full and (_actionable(el, role) or role in TEXT_INPUTS):
                    line = f"[{refs.add(pid, base + path, el)}] " + line
                if needle is None or needle in line.casefold():
                    lines.append(("" if needle else "  " * level) + line)
            if len(kids) > MAX_CHILDREN:
                lines.append(
                    "  " * (level + 1)
                    + f"... {len(kids)} children (only the first {MAX_CHILDREN} are walked)"
                )
            for i, child in enumerate(kids[:MAX_CHILDREN]):
                if len(lines) >= limit:
                    return
                walk(child, level if skip else level + 1, path + [i])

        walk(root, 0, [])
        refs.save()
        if len(lines) >= limit:
            lines.append(f"... cut at {limit} lines (pass a smaller root, query=, or limit=)")
        return "\n".join(lines)

    def el(self, ref: int) -> Any:
        """The element behind ref ``[n]`` of an earlier ``raw.dump`` in this task. Raises LookupError when it is
        gone (dump again)."""
        return _Refs.load().resolve(int(ref))

    def text(self, root: Any, limit: int = 300) -> list[str]:
        """The visible text under ``root`` in reading order (static text, headings, links, field values), each
        string once. The cheapest way to read a page, a message or a table."""
        out: list[str] = []
        seen: set[str] = set()

        def walk(el: Any, d: int) -> None:
            if el is None or len(out) >= limit or d > 40:
                return
            role = _ax_get(el, "AXRole")
            if role in ("AXMenuBar", "AXMenu"):
                return
            if role in TEXT_ROLES or role in TEXT_INPUTS:
                for attr in ("AXValue", "AXTitle", "AXDescription"):
                    v = _ax_get(el, attr)
                    if isinstance(v, str) and v.strip() and v not in seen:
                        seen.add(v)
                        out.append(v)
                        break
            for child in (_ax_get(el, "AXChildren") or [])[:MAX_CHILDREN]:
                walk(child, d + 1)

        walk(root, 0)
        return out

    # ---------------------------------------------------------------- value helpers (AX and per-process events only)

    def set_text(self, el: Any, text: str, typed: bool | None = None) -> dict[str, Any]:
        """Put ``text`` in a text field and read it back. First an AXValue write, read back for up to 1 s; if the
        field does not show the text by then (web views, browser address bars), the field is focused inside its own
        app, its whole text selected through AXSelectedTextRange and ``text`` typed over it as key events posted to
        that app's pid. ``typed=True`` goes straight to typing. A LibreOffice Calc cell goes through ``set_cell``.
        Returns the route, the value read back and ``ok`` (the value equals ``text``)."""
        text = str(text)
        pid = self.ax.pid(el)
        STATE.to_user(pid, "set_text")
        role = _ax_get(el, "AXRole")
        if role in MENU_OPENERS:
            STATE.check(
                "MB6" if role == "AXMenuBarItem" else "MB7",
                f"raw.set_text on {role} (use raw.select)",
            )
        if _calc_cell(el):
            return self.set_cell(pid, str(_ax_get(el, "AXTitle")), text)
        if typed is None:
            typed = _chromium(pid) and not _in_web_area(el)
        if not typed:
            STATE.count(pid)
            AX.AXUIElementSetAttributeValue(el, "AXValue", text)
            # An Electron field can echo the write later than 0.15 s; typing before the echo lands appended the
            # text to it ("13:0013:00", #7584), so wait for it first.
            if _wait_text(el, text, 1.0):
                STATE.route("set_text:ax_value")
                return {"route": "ax_value", "value": text, "ok": True}
        _focus_in_app(el, "raw.set_text")
        # Select what the field holds through AX: a posted cmd+a does not select in a background Electron field, and
        # the typed text was appended (#7584).
        current = _text_of(el)
        AX.AXUIElementSetAttributeValue(
            el,
            "AXSelectedTextRange",
            AX.AXValueCreate(AX.kAXValueCFRangeType, (0, len(current))),
        )
        time.sleep(0.1)
        self.post_text(pid, text)
        ok = _wait_text(el, text, 1.5)
        STATE.route("set_text:typed")
        return {"route": "typed", "value": _text_of(el), "ok": ok}

    def select(self, el: Any, label: str) -> dict[str, Any]:
        """Choose ``label`` in a pop-up button, web ``<select>`` or combo box and read the choice back.
        A web ``<select>`` (Chrome, Electron, web views) is focused inside the page and ``label`` typed into it, so
        its menu never opens. A combo box takes an AXValue write. A native AppKit pop-up lists its items only while
        its menu is open, and opening it takes key focus and raises the window (rule MB7). On a trial with the
        foreground grant it is opened and picked as is; otherwise the menu is opened under the key-focus guard
        (``_pick_guarded``), and anything the guard could not undo is refused under MB7."""
        label = str(label)
        pid = self.ax.pid(el)
        role = _ax_get(el, "AXRole")
        STATE.to_user(pid, "select")
        if _chosen(el) == label.casefold():
            return {"route": "already", "value": label}
        if role == "AXComboBox":
            STATE.count(pid)
            AX.AXUIElementSetAttributeValue(el, "AXValue", label)
            time.sleep(0.2)
            STATE.route("select:ax_value")
            return {"route": "ax_value", "value": _ax_get(el, "AXValue")}
        if role != "AXPopUpButton":
            raise ValueError(f"raw.select needs an AXPopUpButton or AXComboBox, not {role}")
        if _in_web_area(el):
            STATE.route("select:typeahead")
            for attempt in range(2):
                _typeahead_pause(pid)
                _focus_in_app(el, "raw.select")
                # the label goes to the select itself (a space in it is part of the type-ahead, not a menu key)
                _post_chars(pid, label, _post_to_pid)
                _typeahead_mark(pid)
                deadline = time.monotonic() + 1.0
                while time.monotonic() < deadline and _chosen(el) != label.casefold():
                    time.sleep(0.05)
                if _chosen(el) == label.casefold():
                    return {"route": "typeahead", "value": _ax_get(el, "AXValue"), "ok": True}
            return {
                "route": "typeahead",
                "value": _ax_get(el, "AXValue"),
                "ok": False,
                "next": (
                    f"typing {label!r} into the closed control did not choose it (twice). Check the option's exact "
                    "label (raw.dump the pop-up) and call raw.select again; do not press the pop-up or post "
                    "Down, Up or Space to it: that opens its menu, which raises the window (MB7)"
                ),
            }
        if STATE.granted:
            STATE.check("MB7", "raw.select on a native pop-up opens its menu")
            return self._pick_from_menu(el, pid, label)
        return self._pick_guarded(el, pid, label)

    def _pick_guarded(self, el: Any, pid: int, label: str) -> dict[str, Any]:
        """A native pop-up in a background app, without the grant: open its menu and pick the item while the user's
        app keeps key focus and its window stays on top, the way Cua Driver's key-focus guard does it
        (platform-macos/src/key_focus_guard.rs:4-28). A tap on the user's app (CGEventTapCreateForPid) drops the
        WindowServer notice that the target's menu took key focus; the theft is then released by id with
        SLPSReleaseKeyFocusWithID (Zane, 13 Nov: allowed as a macOS API); if the target's window rose over the
        user's, the user's own focused window is raised back (AXRaise on it). Anything the guard could not undo is
        refused under MB7 after the fact, so it is reported."""
        front = front_pid()
        if not front or front == pid:
            STATE.check("MB7", "raw.select on a native pop-up of the frontmost app opens its menu")
            return self._pick_from_menu(el, pid, label)
        guard = _KeyFocusGuard(front, pid)
        if not guard.start():
            STATE.refuse("MB7", "raw.select: the key-focus guard could not tap the user's app")
        # the call used the guard: if the runner does not hand back a complete result (killed at its time limit, or
        # crashed), the server sees this file and refuses the call under MB7 (raw_mcp_mac.run_script). It is never
        # removed; a complete result carries the guard's own refusals and hook report instead.
        marker = STATE.image_dir / GUARD_MARKER if STATE.image_dir is not None else None
        if marker is not None:
            marker.write_text(app_name(pid), "utf-8")
        try:
            result = self._pick_from_menu(el, pid, label, guard=guard)
        finally:
            # also when the pick raised (no such item): what the guard could not undo is refused either way
            problems = guard.finish()
            if problems:
                STATE.refuse("MB7", "raw.select guarded pop-up: " + "; ".join(problems))
        STATE.route("select:menu_guarded")
        result["route"] = "menu_guarded"
        result["key_focus_released"] = len(guard.released)
        result["window_restores"] = guard.restores
        result["key_focus_held_ms"] = guard.held_ms
        return result

    def _pick_from_menu(
        self, el: Any, pid: int, label: str, guard: "_KeyFocusGuard | None" = None
    ) -> dict[str, Any]:
        """Open a native pop-up's menu (AXPress on its own thread: the call blocks while the menu tracks), press the
        matching item as soon as it is listed, and close the menu if nothing matched. With ``guard``, the item is
        pressed only after the guard has handed key focus back."""
        STATE.count(pid)
        AX.AXUIElementSetMessagingTimeout(el, 0.5)
        opener = threading.Thread(
            target=AX.AXUIElementPerformAction, args=(el, "AXPress"), daemon=True
        )
        opener.start()
        deadline = time.monotonic() + 2.5
        items: list[Any] = []
        hit = None
        while time.monotonic() < deadline and hit is None:
            items = _menu_items(el)
            hit = next(
                (
                    i
                    for i in items
                    if str(_ax_get(i, "AXTitle") or "").casefold() == label.casefold()
                ),
                None,
            )
            time.sleep(0.02)
        if guard is not None:
            guard.settle()
        if hit is None:
            for child in _ax_get(el, "AXChildren") or []:
                AX.AXUIElementPerformAction(child, "AXCancel")
            opener.join(1.0)
            names = [str(_ax_get(i, "AXTitle") or "") for i in items]
            raise LookupError(f"no item {label!r} in the pop-up; it lists {names}")
        AX.AXUIElementPerformAction(hit, "AXPress")
        opener.join(1.0)
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline and _chosen(el) != label.casefold():
            time.sleep(0.05)
        STATE.route("select:menu")
        return {
            "route": "menu",
            "value": _ax_get(el, "AXValue"),
            "ok": _chosen(el) == label.casefold(),
        }

    def set_cell(
        self,
        pid: int,
        cell: str,
        value: str,
        window: str | None = None,
        sheet: str | None = None,
    ) -> dict[str, Any]:
        """Write ``value`` into cell ``cell`` ("H4") of LibreOffice Calc process ``pid`` and read it back;
        ``value=""`` clears it. ``sheet`` ("Controls") switches to that sheet first with the menu command
        Sheet > Navigate > To Next / To Previous Sheet; pressing a sheet tab does nothing in the background.
        ``window``: part of the document window's title when several are open. Calc ignores AX writes to a cell and
        plain per-process keys while it is in the background, so this makes the document window Calc's key window
        without activating or raising it (the window-key route of ``post_key(..., window_id=)``), selects the cell
        through AX and types the value followed by Return. Calc parses what it is given as it would typed text
        ("13:00" becomes a time and reads back as "01:00:00 PM"; start with ' to keep text). A dialog that holds
        Calc's key focus ("Welcome to LibreOffice!") is named in the error: close it first. Only cells on screen can
        be selected."""
        pid, cell, value = int(pid), str(cell).upper(), str(value)
        STATE.to_user(pid, "set_cell")
        win = _calc_window(pid, window)
        STATE.count(pid)
        if sheet is not None:
            self._calc_sheet(pid, win, str(sheet))
        wid = _ax_window_id(pid, win)
        _window_key(pid, wid, "raw.set_cell")
        before = _calc_cell_value(win, cell)
        self._calc_select(win, cell)
        _window_key(pid, wid, "raw.set_cell")
        if value:
            self.post_text(pid, value, window_id=wid)
            # AutoInput may have completed the text from the column (typing "blocked" over "blocked:LHP-C17"); the
            # completion is selected, and forward delete removes it before Return enters the cell
            self.post_key(pid, "forwarddelete", window_id=wid)
            self.post_key(pid, "return", window_id=wid)
        else:
            # forward delete on a selected cell clears its contents without the Delete Contents dialog
            self.post_key(pid, "forwarddelete", window_id=wid)
        deadline = time.monotonic() + 1.5
        got = _calc_cell_value(win, cell)
        while time.monotonic() < deadline and (
            got == before and before != value or not _calc_shows(value, got)
        ):
            time.sleep(0.1)
            got = _calc_cell_value(win, cell)
        if AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(pid) is None:
            raise RuntimeError(
                "LibreOffice quit during the cell write; its unsaved changes are gone"
            )
        STATE.route("set_cell:keys")
        out = {
            "route": "keys",
            "cell": cell,
            "value": got,
            "ok": _calc_shows(value, got),
            "changed": got != before,
        }
        if out["ok"] and got != value and not value.startswith("'"):
            out["parsed"] = True  # Calc turned the text into a number, date or time
        return out

    def _calc_sheet(self, pid: int, win: Any, name: str) -> None:
        """Show sheet ``name``: Sheet > Navigate > To Next Sheet (or Previous) until the sheet's table is shown."""
        tabs = [
            str(_ax_get(t, "AXTitle") or "") for t in self.find(win, role="AXRadioButton", limit=64)
        ]
        if name not in tabs:
            raise LookupError(f"no sheet {name!r}; the sheet tabs are {tabs}")
        want = f"Sheet {name}"
        for direction in ("To Next Sheet", "To Previous Sheet"):
            for _ in range(len(tabs)):
                if _calc_table_title(win) == want:
                    return
                self.menu(pid, ["Sheet", "Navigate", direction])
                deadline = time.monotonic() + 1.5
                while time.monotonic() < deadline and _calc_table_title(win) == "":
                    time.sleep(0.05)
                time.sleep(0.2)
        if _calc_table_title(win) != want:
            raise RuntimeError(
                f"could not switch to sheet {name!r} (shown: {_calc_table_title(win)!r})"
            )

    def _calc_select(self, win: Any, cell: str) -> None:
        table = next(iter(self.find(win, role="AXTable", limit=1)), None)
        if table is None:
            raise LookupError("no sheet (AXTable) in that window")
        target = _calc_cell_el(table, cell)
        if target is None:
            raise LookupError(f"cell {cell} is not on screen in this sheet")
        _focus_in_app(table, "raw.set_cell")
        AX.AXUIElementSetAttributeValue(target, "AXSelected", True)
        time.sleep(0.1)
        _focus_in_app(target, "raw.set_cell")
        deadline = time.monotonic() + 1.5
        while time.monotonic() < deadline and _calc_name_box(win) != cell:
            time.sleep(0.05)
        time.sleep(0.3)
        if _calc_name_box(win) != cell:
            raise RuntimeError(
                f"Calc did not select {cell} (name box shows {_calc_name_box(win)!r}); a dialog may be open"
            )

    def menu(self, pid: int, path: list[str]) -> dict[str, Any]:
        """Run a menu-bar command of process ``pid`` (``["File", "Save"]``) without opening any menu: the item is
        found in the app's AXMenuBar and pressed. Fails if an item on the path is missing or disabled."""
        pid = int(pid)
        STATE.to_user(pid, "menu")
        node = _ax_get(AX.AXUIElementCreateApplication(pid), "AXMenuBar")
        if node is None:
            raise LookupError(f"{app_name(pid)} exposes no menu bar")
        for depth, name in enumerate(path):
            kids = _ax_get(node, "AXChildren") or []
            if depth and kids and _ax_get(kids[0], "AXRole") == "AXMenu":
                kids = _ax_get(kids[0], "AXChildren") or []
            hit = next((k for k in kids if str(_ax_get(k, "AXTitle") or "") == name), None)
            if hit is None:
                hit = next(
                    (
                        k
                        for k in kids
                        if str(_ax_get(k, "AXTitle") or "").casefold().rstrip(".…")
                        == name.casefold().rstrip(".…")
                    ),
                    None,
                )
            if hit is None:
                have = [str(_ax_get(k, "AXTitle") or "") for k in kids if _ax_get(k, "AXTitle")]
                raise LookupError(
                    f"menu item {name!r} not found under {path[:depth]}; there: {have[:40]}"
                )
            node = hit
        if _ax_get(node, "AXRole") != "AXMenuItem":
            raise ValueError("the path must end at a menu item, not a menu-bar title")
        if _ax_get(node, "AXEnabled") is False:
            raise RuntimeError(
                f"menu item {path} is disabled while {app_name(pid)} is in the background"
            )
        STATE.count(pid)
        err = int(AX.AXUIElementPerformAction(node, "AXPress"))
        STATE.route("menu:press_item")
        return {"route": "press_item", "ax_error": err}

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
        # hit-test inside the target app, not the screen: the point may be covered by another app's window
        err, hit = AX.AXUIElementCopyElementAtPosition(self.ax.app(pid), float(x), float(y), None)
        role = _ax_get(hit, "AXRole") if err == 0 and hit is not None else None
        if err != 0 and not STATE.granted:
            STATE.check(
                "MB7",
                f"posted click at ({x}, {y}): no element of {app_name(pid)} found there to check",
            )
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

    def post_key(self, pid: int, keys: list[str] | str, window_id: int | None = None) -> str:
        """Post one key or combo (["cmd", "s"], "return") to process ``pid`` only (CGEventPostToPid). With
        ``window_id`` (one of ``pid``'s windows), that window is first made the app's key window without activating
        or raising it, and the key goes through SLEventPostToPid: the route for apps that drop plain per-process
        keys while in the background (LibreOffice)."""
        pid = int(pid)
        STATE.to_user(pid, "posted keys")
        combo = [keys] if isinstance(keys, str) else [str(k) for k in keys]
        lowered = [k.lower() for k in combo]
        if lint.combo(lowered) in lint.FOCUS_COMBOS:
            STATE.check("MB9", "keys " + "+".join(lowered))
        post = _post_to_pid
        if window_id is not None:
            _window_key(pid, int(window_id), "raw.post_key")
            post = _sl_post_to_pid
        # checked where the keys go: after the key-window switch, so a pop-up focused in that window counts
        if lowered and lowered[-1] in lint.POPUP_OPEN_KEYS:
            role = _focused_popup(pid)
            if role is not None:
                # #7551: Down, Up or Space on a focused pop-up opens its menu, which takes key focus and raises the
                # window; raw.select chooses the option without opening it
                STATE.check(
                    "MB7",
                    f"posted {'+'.join(lowered)} to {app_name(pid)}, whose focused element is an {role} (opens its "
                    "menu); choose the option with raw.select(el, 'Label')",
                )
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
            post(pid, ev)
            time.sleep(0.01)
        STATE.count(pid)
        return f"posted {'+'.join(combo)} to {app_name(pid)}"

    def post_text(self, pid: int, text: str, window_id: int | None = None) -> str:
        """Type text into process ``pid``'s focused field as unicode key events (CGEventPostToPid). Setting the
        field's AXValue is usually simpler and does not depend on which field has focus. ``window_id``: as in
        ``post_key``, for apps that drop plain per-process keys in the background."""
        pid = int(pid)
        STATE.to_user(pid, "posted text")
        post = _post_to_pid
        if window_id is not None:
            _window_key(pid, int(window_id), "raw.post_text")
            post = _sl_post_to_pid
        if " " in str(text):
            role = _focused_popup(pid)
            if role is not None:
                STATE.check(
                    "MB7",
                    f"posted text with a space to {app_name(pid)}, whose focused element is an {role} (a space opens "
                    "its menu); choose the option with raw.select(el, 'Label')",
                )
        _post_chars(pid, str(text), post)
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

    def sleep(self, seconds: float) -> None:
        time.sleep(min(float(seconds), 30.0))


# An app's AX tree can be huge (a spreadsheet table lists every cell); the helpers walk at most this many children
# of one node, so a tree read ends. A script's own loop is the script's business.
MAX_CHILDREN = 1000

# ------------------------------------------------------------------ compact dumps and refs

TEXT_ROLES = ("AXStaticText", "AXHeading", "AXLink", "AXCell")
TEXT_INPUTS = ("AXTextField", "AXTextArea", "AXComboBox", "AXSearchField")
# Roles an agent acts on even when they list no action (set a value, select).
ACTABLE_ROLES = (
    "AXButton",
    "AXPopUpButton",
    "AXMenuButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXComboBox",
    "AXRow",
    "AXSlider",
    "AXIncrementor",
    "AXTab",
    "AXLink",
    "AXMenuItem",
    "AXDisclosureTriangle",
    "AXOutline",
    "AXTable",
)
WRAPPERS = ("AXGroup", "AXScrollArea", "AXSplitGroup", "AXLayoutArea", "AXUnknown")
PASSIVE_ACTIONS = ("AXShowMenu", "AXScrollToVisible")


def _label_parts(el: Any) -> list[str]:
    out = []
    for attr in ("AXTitle", "AXDescription", "AXPlaceholderValue"):
        v = _ax_get(el, attr)
        if isinstance(v, str) and v.strip() and v not in out:
            out.append(v)
    return out


def _actions(el: Any) -> list[str]:
    err, names = AX.AXUIElementCopyActionNames(el, None)
    return [a for a in (names or []) if a not in PASSIVE_ACTIONS] if err == 0 else []


def _actionable(el: Any, role: Any) -> bool:
    return role in ACTABLE_ROLES or bool(_actions(el))


def _is_wrapper(el: Any, role: Any) -> bool:
    """A layout node with nothing of its own to say: no label, no value, no action."""
    if role not in WRAPPERS or _label_parts(el) or _actions(el):
        return False
    v = _ax_get(el, "AXValue")
    return v in (None, "")


def _short(v: Any, n: int = 160) -> str:
    s = str(v).replace("\n", " ")
    return repr(s if len(s) <= n else s[: n - 1] + "…")


def _compact(el: Any, role: Any) -> str:
    """One short line: role, label, value, state flags, non-press actions."""
    parts = [str(role or "?").removeprefix("AX")]
    sub = _ax_get(el, "AXSubrole")
    if sub in (
        "AXDialog",
        "AXSystemDialog",
        "AXApplicationDialog",
        "AXSearchField",
        "AXSecureTextField",
    ):
        parts[0] += "/" + str(sub).removeprefix("AX")
    labels = _label_parts(el)
    v = _ax_get(el, "AXValue")
    if role in TEXT_ROLES and isinstance(v, str) and not labels:
        parts.append(_short(v, 300))
    else:
        if labels:
            parts.append(_short(" / ".join(labels), 120))
        if v is not None and v != "" and not isinstance(v, bool):
            parts.append("= " + _short(_py(v), 200))
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
    if role in ("AXCheckBox", "AXRadioButton") and v in (0, 1, True, False):
        flags.append("on" if v else "off")
        parts = [p for p in parts if not p.startswith("= ")]
    if flags:
        parts.append("(" + ",".join(flags) + ")")
    acts = [a.removeprefix("AX") for a in _actions(el) if a != "AXPress"]
    if acts:
        parts.append("actions=" + "/".join(acts))
    return " ".join(parts)


def _path_of(el: Any, pid: int) -> list[int]:
    """Child indexes from the app element down to ``el``."""
    path: list[int] = []
    node = el
    for _ in range(64):
        if _ax_get(node, "AXRole") == "AXApplication":
            return list(reversed(path))
        parent = _ax_get(node, "AXParent")
        if parent is None:
            break
        kids = list(_ax_get(parent, "AXChildren") or [])
        try:
            path.append(kids.index(node))
        except ValueError:
            break
        node = parent
    return list(reversed(path)) if _ax_get(node, "AXRole") == "AXApplication" else []


class _Refs:
    """Numbered element refs that outlive one script: pid, child-index path from the app, role and label. A ref is
    resolved by walking the path again and checking role and label; a moved element is found by role and label."""

    def __init__(self, path: Path | None, data: dict[str, Any]) -> None:
        self.path, self.data = path, data
        self.by_key = {(v["pid"], tuple(v["path"])): int(k) for k, v in data.items()}

    @classmethod
    def load(cls) -> "_Refs":
        name = os.environ.get("RAW_REFS")
        path = Path(name) if name else None
        data: dict[str, Any] = {}
        if path is not None and path.is_file():
            try:
                data = json.loads(path.read_text("utf-8"))
            except ValueError:
                data = {}
        return cls(path, data)

    def add(self, pid: int, path: list[int], el: Any) -> int:
        """The ref of this element: the same number as before when the same element sits at the same place, else a
        new number (a number never changes what it points to)."""
        key = (pid, tuple(path))
        entry = {
            "pid": pid,
            "path": path,
            "role": _ax_get(el, "AXRole"),
            "label": _label_parts(el)[:1],
        }
        n = self.by_key.get(key)
        if n is not None:
            old = self.data[str(n)]
            if old["role"] == entry["role"] and old["label"] == entry["label"]:
                return n
        n = max((int(k) for k in self.data), default=0) + 1
        self.by_key[key] = n
        self.data[str(n)] = entry
        return n

    def save(self) -> None:
        if self.path is not None:
            self.path.write_text(json.dumps(self.data), "utf-8")

    def resolve(self, n: int) -> Any:
        entry = self.data.get(str(n))
        if entry is None:
            raise LookupError(f"no ref [{n}] in this task; dump first")
        node = AX.AXUIElementCreateApplication(int(entry["pid"]))
        for i in entry["path"]:
            kids = _ax_get(node, "AXChildren") or []
            if i >= len(kids):
                node = None
                break
            node = kids[i]
        if (
            node is not None
            and _ax_get(node, "AXRole") == entry["role"]
            and _label_parts(node)[:1] == entry["label"]
        ):
            return node
        if entry["label"]:
            hits = Raw().find(
                AX.AXUIElementCreateApplication(int(entry["pid"])),
                role=entry["role"],
                title=entry["label"][0],
                limit=2,
            )
            if len(hits) == 1:
                return hits[0]
        raise LookupError(f"ref [{n}] ({entry['role']} {entry['label']}) is gone; dump again")


# ------------------------------------------------------------------ value-helper internals


def _text_of(el: Any) -> str:
    v = _ax_get(el, "AXValue")
    return v if isinstance(v, str) else ("" if v is None else str(v))


def _wait_text(el: Any, text: str, wait: float) -> bool:
    """Whether the field reads ``text`` within ``wait`` seconds."""
    deadline = time.monotonic() + wait
    while True:
        if _text_of(el) == text:
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.05)


# Chromium keeps a <select>'s type-ahead buffer for about 1 s: a second label typed into another select of the same
# page within that time was appended to the first and chose nothing about one time in two (CDB-G04 bg r5; 16/16 with a
# 1.2 s gap in the bench guest). The last type-ahead time per pid is kept across scripts.
_TYPEAHEAD_GAP_S = 1.2


def _typeahead_file(pid: int) -> Path:
    return Path(tempfile.gettempdir()) / f"raw-macos-typeahead-{int(pid)}"


def _typeahead_pause(pid: int) -> None:
    try:
        last = float(_typeahead_file(pid).read_text().strip())
    except (OSError, ValueError):
        return
    wait = last + _TYPEAHEAD_GAP_S - time.time()
    if 0 < wait <= _TYPEAHEAD_GAP_S:
        time.sleep(wait)


def _typeahead_mark(pid: int) -> None:
    try:
        _typeahead_file(pid).write_text(repr(time.time()))
    except OSError:
        pass


# Roles whose menu a posted Down, Up or Space opens when they have keyboard focus (#7551).
POPUP_ROLES = ("AXPopUpButton", "AXMenuButton", "AXComboBox")


def _focused_popup(pid: int) -> str | None:
    """The role of ``pid``'s focused element if it is a pop-up, else None. The app's AXFocusedUIElement first; a
    background Electron app reports none there (traced on Rillwatch Cases), so its focused window's pop-ups are then
    asked for AXFocused themselves."""
    app = AX.AXUIElementCreateApplication(int(pid))
    el = _ax_get(app, "AXFocusedUIElement")
    role = _ax_get(el, "AXRole") if el is not None else None
    if role is not None:
        return str(role) if role in POPUP_ROLES else None
    win = _ax_get(app, "AXFocusedWindow")
    for popup_role in POPUP_ROLES:
        for popup in Raw().find(win, role=popup_role, limit=200) if win is not None else []:
            if _ax_get(popup, "AXFocused") is True:
                return popup_role
    return None


def _chosen(el: Any) -> str:
    v = _ax_get(el, "AXValue")
    if not isinstance(v, str) or not v:
        v = _ax_get(el, "AXTitle")
    return str(v or "").strip().casefold()


def _in_web_area(el: Any) -> bool:
    node = el
    for _ in range(40):
        node = _ax_get(node, "AXParent")
        if node is None:
            return False
        if _ax_get(node, "AXRole") == "AXWebArea":
            return True
    return False


def _chromium(pid: int) -> bool:
    app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(int(pid))
    bid = str(app.bundleIdentifier() or "").lower() if app is not None else ""
    return "chrome" in bid or "chromium" in bid or "electron" in bid


def _focus_in_app(el: Any, what: str) -> None:
    """Move a background app's own keyboard focus to ``el`` (AXFocused), as Cua Driver's background set_value does for
    a web <select> (platform-macos/src/tools/set_value.rs:979-985), and prove it left the user alone. The element must
    not belong to the frontmost app or the user's app; the frontmost app, its key window and its focused element are
    read before and after, and any change is refused under MB3 like a script's own AXFocused write (warned on a
    granted trial). In the bench guest this focus move never made the background window key (sentinel key_loss 0)."""
    pid = Raw().ax.pid(el)
    before = focus_sample()
    if pid == before["front"]:
        STATE.check("MB3", f"{what}: focus inside {app_name(pid)}, the frontmost app")
    STATE.to_user(pid, what)
    AX.AXUIElementSetAttributeValue(el, "AXFocused", True)
    time.sleep(0.12)
    after = focus_sample()
    moved = [k for k in ("front", "key_window", "focused") if after[k] != before[k]]
    STATE.route("in_app_focus")
    if moved:
        STATE.check(
            "MB3",
            f"{what}: the user's {', '.join(moved)} changed after an in-app focus in {app_name(pid)}",
        )


# ------------------------------------------------------------------ SkyLight (window-key route, key-focus guard)

_CG = ctypes.CDLL("/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices")
_CFL = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
_SKYLIGHT = ctypes.CDLL("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight")
_SKYLIGHT.CGSMainConnectionID.restype = ctypes.c_uint32
_SKYLIGHT.SLSGetWindowOwner.argtypes = [
    ctypes.c_uint32,
    ctypes.c_uint32,
    ctypes.POINTER(ctypes.c_uint32),
]
_SKYLIGHT.SLSGetWindowOwner.restype = ctypes.c_int32
_SKYLIGHT.SLSGetConnectionPSN.argtypes = [ctypes.c_uint32, ctypes.c_void_p]
_SKYLIGHT.SLSGetConnectionPSN.restype = ctypes.c_int32
_SKYLIGHT.SLPSPostEventRecordTo.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_SKYLIGHT.SLPSPostEventRecordTo.restype = ctypes.c_int32
_SKYLIGHT.SLEventPostToPid.argtypes = [ctypes.c_int32, ctypes.c_void_p]
_SKYLIGHT.SLEventPostToPid.restype = None
_CG._AXUIElementGetWindow.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
_CG._AXUIElementGetWindow.restype = ctypes.c_int32
# Pause between the key-window records: Calc drops the focus record while it still handles the defocus
# (skylight.rs KEY_RECORD_SETTLE).
_KEY_RECORD_SETTLE_S = 0.12


def _post_chars(pid: int, text: str, post: Any) -> None:
    """Unicode key events, one pair per character, to ``pid`` through ``post``."""
    for ch in text:
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(None, 0, down)
            Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
            post(pid, ev)
        time.sleep(0.005)
    STATE.count(pid)


def _post_to_pid(pid: int, ev: Any) -> None:
    Quartz.CGEventPostToPid(pid, ev)


def _sl_post_to_pid(pid: int, ev: Any) -> None:
    """SLEventPostToPid: the per-process post that goes through WindowServer's activity path (skylight.rs:8-11)."""
    _SKYLIGHT.SLEventPostToPid(int(pid), ctypes.c_void_p(ev.__c_void_p__().value))


def _ax_window_id(pid: int, win: Any) -> int:
    """The CGWindowID of an AX window (_AXUIElementGetWindow)."""
    wid = ctypes.c_uint32(0)
    if _CG._AXUIElementGetWindow(ctypes.c_void_p(win.__c_void_p__().value), ctypes.byref(wid)) == 0:
        return int(wid.value)
    raise LookupError(f"no window id for {app_name(pid)}'s window {_ax_get(win, 'AXTitle')!r}")


def _window_psn(pid: int, wid: int) -> Any:
    """The process serial number of the window's owner (SLSGetWindowOwner, SLSGetConnectionPSN)."""
    owner = ctypes.c_uint32(0)
    psn = (ctypes.c_uint8 * 8)()
    if (
        _SKYLIGHT.SLSGetWindowOwner(_SKYLIGHT.CGSMainConnectionID(), int(wid), ctypes.byref(owner))
        or not owner.value
        or _SKYLIGHT.SLSGetConnectionPSN(owner.value, psn)
    ):
        raise LookupError(f"window {wid} of {app_name(pid)} has no owner process on this display")
    return psn


def _key_record(wid: int, kind: int, direction: int | None = None) -> Any:
    """A 248-byte event record: kind 0x0d with direction 1 (focus) or 2 (defocus), or the make-key pair 0x01/0x02
    (skylight.rs focus_record, make_key_window_record)."""
    rec = (ctypes.c_uint8 * 0xF8)()
    rec[0x04] = 0xF8
    rec[0x08] = kind
    rec[0x3C:0x40] = list(int(wid).to_bytes(4, "little"))
    if direction is not None:
        rec[0x8A] = direction
    if kind in (0x01, 0x02):
        rec[0x3A] = 0x10
        for i in range(0x20, 0x30):
            rec[i] = 0xFF
    return rec


def _focused_window_id(pid: int) -> int | None:
    win = _ax_get(AX.AXUIElementCreateApplication(int(pid)), "AXFocusedWindow")
    if win is None:
        return None
    try:
        return _ax_window_id(pid, win)
    except LookupError:
        return None


def _window_key(pid: int, wid: int, what: str) -> None:
    """Make window ``wid`` of ``pid`` that app's key window without activating it or raising a window, the way Cua
    Driver does for background keys (skylight.rs post_window_key_records): a defocus record for the app's current key
    window, a focus record and the make-key pair for ``wid``, each sent to ``pid``'s process only. Confirmed by
    re-reading the app's AXFocusedWindow; refused (no keys are sent) when another window, such as a dialog, keeps it.
    The user's front app, key window and focused element are read before and after, and a change is refused under
    MB3 as for any in-app focus move."""
    pid, wid = int(pid), int(wid)
    if not any(w["window_id"] == wid and w["pid"] == pid for w in window_list()):
        raise LookupError(f"window {wid} is not an on-screen window of {app_name(pid)}")
    current = _focused_window_id(pid)
    if current == wid:
        return
    before = focus_sample()
    if pid == before["front"]:
        STATE.check("MB3", f"{what}: key window change inside {app_name(pid)}, the frontmost app")
    STATE.to_user(pid, what)
    psn = _window_psn(pid, wid)
    for _ in range(
        2
    ):  # WindowServer delivers the records asynchronously; a second pass catches a busy app
        if current is not None:
            _SKYLIGHT.SLPSPostEventRecordTo(psn, _key_record(current, 0x0D, 2))
            time.sleep(_KEY_RECORD_SETTLE_S)
        _SKYLIGHT.SLPSPostEventRecordTo(psn, _key_record(wid, 0x0D, 1))
        time.sleep(_KEY_RECORD_SETTLE_S)
        for kind in (0x01, 0x02):
            _SKYLIGHT.SLPSPostEventRecordTo(psn, _key_record(wid, kind))
        deadline = time.monotonic() + 0.5
        while time.monotonic() < deadline and _focused_window_id(pid) != wid:
            time.sleep(0.04)
        if _focused_window_id(pid) == wid:
            break
    STATE.route("window_key")
    after = focus_sample()
    moved = [k for k in ("front", "key_window", "focused") if after[k] != before[k]]
    if moved:
        STATE.check(
            "MB3",
            f"{what}: the user's {', '.join(moved)} changed after a key window change in {app_name(pid)}",
        )
    holder = _focused_window_id(pid)
    if holder != wid:
        title = next((w["title"] for w in window_list() if w["window_id"] == holder), "")
        raise RuntimeError(
            f"window {wid} of {app_name(pid)} did not become its key window, so no keys were sent: "
            f"{('the window ' + repr(title)) if title else 'another window'} (window_id {holder}) holds key focus, "
            "most likely a dialog. Close or answer it (raw.dump that window and press its button, or "
            "raw.ax.perform(raw.ax.get(w, 'AXCloseButton'), 'AXPress') on it), then repeat the call"
        )


# Key-focus guard (background native pop-ups): SLPSReleaseKeyFocusWithID hands a menu's key-focus theft back.
_release_key_focus = _SKYLIGHT.SLPSReleaseKeyFocusWithID
_release_key_focus.argtypes = [ctypes.c_int32]
_release_key_focus.restype = ctypes.c_int32
_TAP_CB = ctypes.CFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p
)
_CG.CGEventTapCreateForPid.argtypes = [
    ctypes.c_int32,
    ctypes.c_uint32,
    ctypes.c_uint32,
    ctypes.c_uint64,
    _TAP_CB,
    ctypes.c_void_p,
]
_CG.CGEventTapCreateForPid.restype = ctypes.c_void_p
_CG.CGEventTapEnable.argtypes = [ctypes.c_void_p, ctypes.c_bool]
_CG.CGEventGetIntegerValueField.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
_CG.CGEventGetIntegerValueField.restype = ctypes.c_int64
_CFL.CFMachPortCreateRunLoopSource.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_long]
_CFL.CFMachPortCreateRunLoopSource.restype = ctypes.c_void_p
_CFL.CFMachPortInvalidate.argtypes = [ctypes.c_void_p]
_CFL.CFRunLoopGetCurrent.restype = ctypes.c_void_p
_CFL.CFRunLoopAddSource.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
_CFL.CFRunLoopStop.argtypes = [ctypes.c_void_p]
_COMMON_MODES = ctypes.c_void_p.in_dll(_CFL, "kCFRunLoopCommonModes")

# WindowServer focus notices (key_focus_guard.rs:325-333): event type 21, subtype field 64, thief pid field 73.
_FOCUS_NOTICE, _F_SUBTYPE, _F_THIEF, _LOST_KEY = 21, 64, 73, 0x4000
_TAP_DISABLED = (0xFFFFFFFE, 0xFFFFFFFF)
# Theft ids come from one session-wide counter (it was about 120 in the bench guest after a day of trials, and grows
# by one per menu a background app opens). A release returns 0 for a live theft and 1010 otherwise, and a theft is
# live only while its menu is open. The guard predates the rule that allows SLPSStealKeyFocusReturningID, so the
# target's id is unknown and the guard releases ids upward from the last id it released (a hint kept across scripts) until it finds the live
# one, then a few past it; a scan costs about 0.1 ms per id.
_THEFT_HINT = Path(tempfile.gettempdir()) / "raw-macos-theft-id"
# start this far below the hint, in case the counter was reset (a WindowServer restart)
_THEFT_BACK = 64
_THEFT_AFTER = 16  # ids past the first live one, for a theft that landed just after it
_THEFT_AHEAD = 10_000  # ids above the hint to look at before falling back to a scan from 1
_THEFT_BUDGET_S = 2.0
_MENU_LAYER = 101


def _theft_hint() -> int:
    try:
        return max(1, int(_THEFT_HINT.read_text().strip()) - _THEFT_BACK)
    except (OSError, ValueError):
        return 1


def _release_thefts() -> list[int]:
    """Release the live key-focus thefts from the hint up: the first live id and any within _THEFT_AFTER of it. If the
    hinted range (up to _THEFT_AHEAD ids) has none, the ids below it are scanned from 1 (the counter was reset)."""
    deadline = time.monotonic() + _THEFT_BUDGET_S
    hint = _theft_hint()
    for start, end in ((hint, hint + _THEFT_AHEAD), (1, hint)):
        released: list[int] = []
        theft, stop = start, end
        while theft < stop and time.monotonic() < deadline:
            if _release_key_focus(theft) == 0:
                released.append(theft)
                stop = min(stop, theft + _THEFT_AFTER + 1)
            theft += 1
        if released:
            try:
                _THEFT_HINT.write_text(str(released[-1]))
            except OSError:
                pass
            return released
    return []


def _target_above(target: int, front: int) -> bool:
    """Whether a normal window of ``target`` sits above every normal window of ``front`` (front to back order)."""
    for w in window_list():
        if w["layer"] != 0 or w["frame"][2] <= 1 or w["frame"][3] <= 1:
            continue
        if w["pid"] == target:
            return True
        if w["pid"] == front:
            return False
    return False


def _menu_up(pid: int) -> bool:
    return any(w["pid"] == pid and w["layer"] >= _MENU_LAYER for w in window_list())


class _KeyFocusGuard:
    """Keeps the user's app key, and its window on top, while a background target's pop-up menu is open."""

    def __init__(self, front: int, target: int) -> None:
        self.front, self.target = int(front), int(target)
        self.port: int | None = None
        self.loop: int | None = None
        self.ready = threading.Event()
        self.stolen = threading.Event()  # the tap saw (and dropped) the target's theft
        self.released: list[int] = []  # the theft ids handed back
        self.scanned = False
        self.restores = 0
        self.interrupted = False
        self.failed: list[str] = []  # errors inside the guard's own threads
        self.thread: threading.Thread | None = None
        self.stolen_at = 0.0
        self.held_ms: int | None = None  # how long the target held key focus before the release
        self.done = threading.Event()
        self.lock = threading.Lock()
        # the user's own focused window, resolved once so a restore is one AX call
        self.front_window = _ax_get(AX.AXUIElementCreateApplication(self.front), "AXFocusedWindow")
        self.callback = _TAP_CB(self._on_event)  # kept alive for the tap's lifetime

    def _on_event(self, proxy: Any, etype: int, event: Any, refcon: Any) -> Any:
        if etype in _TAP_DISABLED:
            if self.port and not self.done.is_set():
                self.interrupted = True  # a notice may have passed while the tap was off
                _CG.CGEventTapEnable(self.port, True)
            return event
        if etype != _FOCUS_NOTICE or not event:
            return event
        lost = _CG.CGEventGetIntegerValueField(event, _F_SUBTYPE) == _LOST_KEY
        if lost and _CG.CGEventGetIntegerValueField(event, _F_THIEF) == self.target:
            self.stolen.set()
            self.stolen_at = time.monotonic()
            threading.Thread(target=self._guarded(self._release), daemon=True).start()
            return None  # the user's app never hears it lost key focus
        return event

    def _guarded(self, body: Any) -> Any:
        def run() -> None:
            try:
                body()
            except Exception as error:  # noqa: BLE001 - a guard thread that died is a failure to report
                with self.lock:
                    self.failed.append(f"the key-focus guard failed: {error!r}"[:300])

        return run

    def _release(self) -> None:
        # Released before the menu window is on screen, AppKit orders the menu group to the front and lifts the
        # target's window (key_focus_guard.rs:589-592), so wait for it first.
        deadline = time.monotonic() + 0.15
        while time.monotonic() < deadline and not _menu_up(self.target):
            time.sleep(0.002)
        self.restore()
        released = _release_thefts()
        with self.lock:
            self.released = released
            self.scanned = True
            self.held_ms = round((time.monotonic() - self.stolen_at) * 1000)

    def _run(self) -> None:
        port = _CG.CGEventTapCreateForPid(self.front, 0, 0, 1 << _FOCUS_NOTICE, self.callback, None)
        if not port:
            self.ready.set()
            return
        source = _CFL.CFMachPortCreateRunLoopSource(None, port, 0)
        self.port, self.loop = port, _CFL.CFRunLoopGetCurrent()
        _CFL.CFRunLoopAddSource(self.loop, source, _COMMON_MODES)
        _CG.CGEventTapEnable(port, True)
        self.ready.set()
        _CFL.CFRunLoopRun()
        _CFL.CFMachPortInvalidate(port)

    def _watch_order(self) -> None:
        # _release restores once the menu window is up; from then on this keeps the user's window on top for the rest
        # of the pick, as the driver's order watch does (key_focus_guard.rs:208-215)
        while not self.done.is_set() and not self.scanned:
            time.sleep(0.005)
        while not self.done.is_set():
            self.restore()
            time.sleep(0.005)

    def start(self) -> bool:
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()
        self.ready.wait(2.0)
        if self.port:
            threading.Thread(target=self._guarded(self._watch_order), daemon=True).start()
        return bool(self.port)

    def restore(self) -> None:
        """Raise the user's own focused window back above the target's (its app is already active, so key focus does
        not move), the driver's put_front_window_back (key_focus_guard.rs:114-128)."""
        if not _target_above(self.target, self.front):
            return
        win = self.front_window
        if win is not None:
            AX.AXUIElementPerformAction(win, "AXRaise")
        with self.lock:
            self.restores += 1

    def settle(self) -> None:
        """Before the item is pressed: wait until the theft was seen and released (up to 1 s)."""
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            with self.lock:
                if self.scanned:
                    return
            time.sleep(0.01)

    def finish(self) -> list[str]:
        """Watch the window order for 1 s after the menu closed, stop the tap, and say what was not undone."""
        # the order watcher keeps running; a late theft notice still goes through the tap
        time.sleep(1.0)
        self.done.set()
        # disable before the run loop ends: the tap thread invalidates the port on its way out
        if self.port:
            _CG.CGEventTapEnable(self.port, False)
        if self.loop:
            _CFL.CFRunLoopStop(self.loop)
        if self.thread is not None:
            self.thread.join(1.0)
        problems = []
        with self.lock:
            # fail closed: any gap in what the guard could see or attribute is refused, not assumed fine
            if self.stolen.is_set() and not self.released:
                problems.append(
                    "the target's key-focus theft was not released (the user's window lost key focus)"
                )
            if not self.stolen.is_set():
                # the pick pressed the pop-up, and an open menu always takes key focus: no notice means the tap
                # missed it, so the user's window may have lost key focus unseen
                problems.append(
                    "the guard never saw the menu take key focus (the user's window may have lost it)"
                )
            problems += self.failed
            if self.interrupted:
                problems.append("the guard's event tap was interrupted while the menu was open")
            if len(self.released) > 1:
                # theft ids are session-wide and carry no pid: one of these may have been another app's
                problems.append(
                    f"{len(self.released)} key-focus thefts were live and released; only one was the target's"
                )
            elif self.stolen.is_set():
                # the user's app was never told, but keys typed in that moment went to the menu: report it
                WATCH_NOTES.append(
                    f"{app_name(self.target)}'s pop-up menu took key focus for {self.held_ms} ms before the "
                    "key-focus guard handed it back to the user's window"
                )
            if self.restores:
                # a raise the guard undid was still on screen for a moment: the hook reports it (WATCH_NOTES)
                WATCH_NOTES.append(
                    f"a window of {app_name(self.target)} rose over the user's window while its pop-up menu opened "
                    f"and the key-focus guard put it back ({self.restores} time(s))"
                )
        if _target_above(self.target, self.front):
            problems.append("the target's window is still above the user's window")
        return problems


def _menu_items(popup: Any) -> list[Any]:
    out: list[Any] = []
    for child in _ax_get(popup, "AXChildren") or []:
        if _ax_get(child, "AXRole") == "AXMenu":
            out += list(_ax_get(child, "AXChildren") or [])
    return out


def _calc_cell(el: Any) -> bool:
    parent = _ax_get(el, "AXParent")
    return (
        _ax_get(el, "AXRole") == "AXTextField"
        and parent is not None
        and _ax_get(parent, "AXRole") == "AXTable"
        and app_name(Raw().ax.pid(el)) == "LibreOffice"
    )


def _calc_window(pid: int, title: str | None) -> Any:
    windows = _ax_get(AX.AXUIElementCreateApplication(int(pid)), "AXWindows") or []
    for w in windows:
        t = str(_ax_get(w, "AXTitle") or "")
        if (title is None or title.casefold() in t.casefold()) and Raw().find(
            w, role="AXTable", limit=1, depth=12
        ):
            return w
    raise LookupError(
        f"no Calc document window{' titled ' + repr(title) if title else ''} with a sheet"
    )


def _formula_bar(win: Any) -> Any:
    return next(
        iter(Raw().find(win, role="AXToolbar", title="Formula Tool Bar", limit=1, depth=8)), None
    )


def _calc_name_box(win: Any) -> str:
    bar = _formula_bar(win)
    box = next(iter(Raw().find(bar, role="AXComboBox", limit=1)), None) if bar is not None else None
    return str(_ax_get(box, "AXValue") or "").strip().upper() if box is not None else ""


def _calc_cell_el(table: Any, cell: str) -> Any:
    return next(
        (c for c in _ax_get(table, "AXChildren") or [] if _ax_get(c, "AXTitle") == cell), None
    )


def _calc_table_title(win: Any) -> str:
    """The shown sheet's AXTable title, "Sheet <name>" (the tabs' AXValue does not follow the shown sheet)."""
    table = next(iter(Raw().find(win, role="AXTable", limit=1)), None)
    return str(_ax_get(table, "AXTitle") or "") if table is not None else ""


def _clock(text: str) -> tuple[int, int, int] | None:
    """(hour 0-23, minute, second) of "13:00", "13:00:00", "1:00 PM" or "01:00:00 PM", else None."""
    m = re.fullmatch(r"\s*(\d{1,2}):(\d{2})(?::(\d{2}))?\s*([AaPp][Mm])?\s*", text)
    if not m:
        return None
    hour, minute, second = int(m.group(1)), int(m.group(2)), int(m.group(3) or 0)
    if m.group(4):
        if not 1 <= hour <= 12:
            return None
        hour = hour % 12 + (12 if m.group(4).lower() == "pm" else 0)
    return (hour, minute, second) if hour < 24 and minute < 60 and second < 60 else None


def _number(text: str) -> float | None:
    try:
        return float(text.strip().replace(",", ""))
    except ValueError:
        return None


def _calc_shows(value: str, got: str) -> bool:
    """Whether a cell showing ``got`` holds ``value`` as typed: the text itself; for a time or number Calc parsed, a
    display of the same time or number ("13:00" shows as "01:00:00 PM"). A leading ' keeps text and is not shown.
    Anything else Calc may have reformatted is not counted as confirmed."""
    if value.startswith("'"):
        return got == value[1:]
    if got == value:
        return True
    want, have = _clock(value), _clock(got)
    if want is not None:
        return want == have
    want_n, have_n = _number(value), _number(got)
    return want_n is not None and have_n is not None and want_n == have_n


def _calc_cell_value(win: Any, cell: str) -> str:
    table = next(iter(Raw().find(win, role="AXTable", limit=1)), None)
    el = _calc_cell_el(table, cell) if table is not None else None
    return str(_ax_get(el, "AXValue") or "") if el is not None else ""


def make() -> Raw:
    return Raw()


def b64(path: Path) -> str:
    return base64.b64encode(path.read_bytes()).decode()
