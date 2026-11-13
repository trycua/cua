# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The `raw` runtime a raw-linux script sees: Linux low-level APIs, guarded, plus a few read-back helpers.

What a script gets (all through the global ``raw``):

* ``raw.Atspi``: the GObject-introspection AT-SPI 2 module (``gi.repository.Atspi``), unmodified except that
  ``generate_mouse_event`` / ``generate_keyboard_event`` refuse unless the task grants foreground, and
  ``grab_focus`` is logged. Semantic actions (Action, EditableText, Value, Selection) are counted per app.
* ``raw.x11``, ``raw.xtst``, ``raw.xi``: libX11, libXtst and libXi through ctypes, with ``raw.dpy`` an open Display.
  Pointer, focus, raise and key functions refuse unless the task grants foreground; grabs always refuse.
* ``raw.xdotool(*args)``: the xdotool CLI, subcommands checked against the same rules.
* Read-back helpers (no input): ``raw.windows()``, ``raw.app(name)``, ``raw.find(...)``, ``raw.dump(...)``,
  ``raw.cells(table, "A1:F20")`` (a spreadsheet range through the AT-SPI Table interface),
  ``raw.screenshot(...)``, ``raw.pointer()``, ``raw.active_window()``, ``raw.sleep(s)``.
* Foreground helpers (need the task's grant): ``raw.activate(window_id)``, ``raw.key(window_id, "ctrl+s")``,
  ``raw.type_text(window_id, text)``, ``raw.click(window_id, x, y)``, ``raw.scroll(window_id, x, y, clicks)`` and
  ``raw.drag(x1, y1, x2, y2)``. Each activates the target window, confirms it is active and injects XTest input
  through xdotool. When the script ends, the runner hands the active window back to the one that had it before,
  unless the input left a popup open.

No Cua Driver: the arm has no route to it (rule DRV1, at lint and at run time).

Foreground is granted by the harness per task (RAW_GRANT=1 in the runner's environment, from the task definition),
never by the script.

AT-SPI calls time out after RAW_ATSPI_TIMEOUT_MS (default 2000 ms) instead of D-Bus's 15 s, so one app that stopped
answering on the bus costs a script seconds, not 15 s. Apps that time out are recorded per call; the MCP server hands
their pids to later scripts of the same trial (RAW_ATSPI_SUSPECT), and raw.app asks those with a 300 ms timeout.

The runtime hook (``Watch``) samples the real pointer, the core focus, the active window and the top of the
stacking order every 50 ms while the script runs, and reports every change. It does not undo anything.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import json
import os
import shutil
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import warnings

import gi

warnings.simplefilter(
    "ignore", DeprecationWarning
)  # gi's deprecated Accessible.get_text etc. stay callable
gi.require_version("Atspi", "2.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Atspi, Gdk  # noqa: E402

import rawlint  # noqa: E402

Window = ctypes.c_ulong
Atom = ctypes.c_ulong


class Blocked(RuntimeError):
    """A guarded call the background contract forbids; the message carries the rule and an alternative."""


def _lib(name: str) -> ctypes.CDLL:
    path = ctypes.util.find_library(name) or f"lib{name}.so"
    return ctypes.CDLL(path)


_X11 = _lib("X11")
_X11.XInitThreads()  # the runtime hook samples on its own connection from a second thread
_XTST = _lib("Xtst")
_XI = _lib("Xi")
_X11.XOpenDisplay.restype = ctypes.c_void_p
_X11.XOpenDisplay.argtypes = [ctypes.c_char_p]
_X11.XDefaultRootWindow.restype = Window
_X11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
_X11.XInternAtom.restype = Atom
_X11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
_X11.XQueryPointer.argtypes = [ctypes.c_void_p, Window] + [ctypes.c_void_p] * 7
_X11.XGetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
_X11.XGetWindowProperty.argtypes = [
    ctypes.c_void_p,
    Window,
    Atom,
    ctypes.c_long,
    ctypes.c_long,
    ctypes.c_int,
    Atom,
] + [ctypes.c_void_p] * 5
_X11.XFree.argtypes = [ctypes.c_void_p]
_X11.XGetGeometry.argtypes = [ctypes.c_void_p, Window] + [ctypes.c_void_p] * 7
_X11.XTranslateCoordinates.argtypes = [
    ctypes.c_void_p,
    Window,
    Window,
    ctypes.c_int,
    ctypes.c_int,
] + [ctypes.c_void_p] * 3
_X11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
_X11.XQueryTree.argtypes = [ctypes.c_void_p, Window] + [ctypes.c_void_p] * 4


class _XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int),
        ("y", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("border_width", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p),
        ("root", Window),
        ("c_class", ctypes.c_int),
        ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int),
        ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int),
        ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int),
        ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int),
        ("screen", ctypes.c_void_p),
    ]


_X11.XGetWindowAttributes.argtypes = [ctypes.c_void_p, Window, ctypes.POINTER(_XWindowAttributes)]
_X11.XSetErrorHandler.restype = ctypes.c_void_p
_ERROR_HANDLER = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)(lambda d, e: 0)
_X11.XSetErrorHandler(
    _ERROR_HANDLER
)  # a window that vanished between QueryTree and a read is not fatal
_X11.XCloseDisplay.argtypes = [ctypes.c_void_p]


class _X:
    """A private Display connection for reads (the hook and the helpers), never handed to the script."""

    def __init__(self) -> None:
        self.dpy = _X11.XOpenDisplay(None)
        if not self.dpy:
            raise RuntimeError("cannot open the X display")
        self.root = _X11.XDefaultRootWindow(self.dpy)
        self.atoms: dict[str, int] = {}

    def atom(self, name: str) -> int:
        if name not in self.atoms:
            self.atoms[name] = _X11.XInternAtom(self.dpy, name.encode(), 0)
        return self.atoms[name]

    def prop(
        self, window: int, name: str, kind: int = 0, length: int = 4096
    ) -> tuple[int, list[int] | bytes]:
        actual, fmt = Atom(), ctypes.c_int()
        n, after = ctypes.c_ulong(), ctypes.c_ulong()
        data = ctypes.c_void_p()
        status = _X11.XGetWindowProperty(
            self.dpy,
            window,
            self.atom(name),
            0,
            length,
            0,
            kind,
            ctypes.byref(actual),
            ctypes.byref(fmt),
            ctypes.byref(n),
            ctypes.byref(after),
            ctypes.byref(data),
        )
        if status != 0 or not data.value:
            return 0, []
        try:
            if fmt.value == 32:
                arr = ctypes.cast(data, ctypes.POINTER(ctypes.c_ulong))
                return 32, [arr[i] for i in range(n.value)]
            if fmt.value == 8:
                return 8, ctypes.string_at(data, n.value)
            return fmt.value, []
        finally:
            _X11.XFree(data)

    def pointer(self) -> tuple[int, int]:
        r, c = Window(), Window()
        rx, ry, wx, wy = ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_int()
        mask = ctypes.c_uint()
        _X11.XQueryPointer(
            self.dpy,
            self.root,
            ctypes.byref(r),
            ctypes.byref(c),
            ctypes.byref(rx),
            ctypes.byref(ry),
            ctypes.byref(wx),
            ctypes.byref(wy),
            ctypes.byref(mask),
        )
        return rx.value, ry.value

    def focus(self) -> int:
        w, revert = Window(), ctypes.c_int()
        _X11.XGetInputFocus(self.dpy, ctypes.byref(w), ctypes.byref(revert))
        return int(w.value)

    def active(self) -> int:
        _, v = self.prop(self.root, "_NET_ACTIVE_WINDOW")
        return int(v[0]) if v else 0

    def top(self) -> int:
        _, v = self.prop(self.root, "_NET_CLIENT_LIST_STACKING")
        return int(v[-1]) if v else 0

    def name(self, window: int) -> str:
        fmt, v = self.prop(window, "_NET_WM_NAME")
        if not v:
            fmt, v = self.prop(window, "WM_NAME")
        return v.decode("utf-8", "replace") if isinstance(v, bytes) else ""

    def pid(self, window: int) -> int | None:
        _, v = self.prop(window, "_NET_WM_PID")
        return int(v[0]) if v else None

    def geometry(self, window: int) -> tuple[int, int, int, int]:
        root = Window()
        x, y = ctypes.c_int(), ctypes.c_int()
        w, h, b, d = ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()
        _X11.XGetGeometry(
            self.dpy,
            window,
            ctypes.byref(root),
            ctypes.byref(x),
            ctypes.byref(y),
            ctypes.byref(w),
            ctypes.byref(h),
            ctypes.byref(b),
            ctypes.byref(d),
        )
        sx, sy, child = ctypes.c_int(), ctypes.c_int(), Window()
        _X11.XTranslateCoordinates(
            self.dpy,
            window,
            self.root,
            0,
            0,
            ctypes.byref(sx),
            ctypes.byref(sy),
            ctypes.byref(child),
        )
        return sx.value, sy.value, w.value, h.value

    def popups(self) -> tuple[int, ...]:
        """Mapped override-redirect children of the root: open menus, combo popups, tooltips. A GTK menu or combo
        popup holds the keyboard grab while it is open, so the user's window loses key focus."""
        root, parent = Window(), Window()
        children = ctypes.POINTER(Window)()
        n = ctypes.c_uint()
        if not _X11.XQueryTree(
            self.dpy,
            self.root,
            ctypes.byref(root),
            ctypes.byref(parent),
            ctypes.byref(children),
            ctypes.byref(n),
        ):
            return ()
        out = []
        try:
            attrs = _XWindowAttributes()
            for i in range(n.value):
                w = children[i]
                if (
                    _X11.XGetWindowAttributes(self.dpy, w, ctypes.byref(attrs))
                    and attrs.override_redirect
                ):
                    if attrs.map_state == 2 and attrs.width > 1 and attrs.height > 1:  # IsViewable
                        out.append(int(w))
        finally:
            if children:
                _X11.XFree(children)
        return tuple(sorted(out))

    def within(self, window: int, top: int) -> bool:
        """Whether ``window`` is ``top`` or one of its descendants (the WM frame is above ``top``, not below)."""
        root, parent = Window(), Window()
        children = ctypes.POINTER(Window)()
        n = ctypes.c_uint()
        for _ in range(32):
            if window == top:
                return True
            if not window or window == self.root:
                return False
            if not _X11.XQueryTree(
                self.dpy, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(n)
            ):
                return False
            if children:
                _X11.XFree(children)
            window = int(parent.value)
        return False

    def clients(self) -> list[int]:
        _, v = self.prop(self.root, "_NET_CLIENT_LIST_STACKING")
        return [int(w) for w in v]

    def close(self) -> None:
        if self.dpy:
            _X11.XCloseDisplay(self.dpy)
            self.dpy = None


# ------------------------------------------------------------------ state shared with the runner


class State:
    def __init__(self) -> None:
        self.granted = os.environ.get("RAW_GRANT") == "1"
        self.raw_actions: dict[str, int] = {}  # semantic AT-SPI actions per app name
        self.routes: dict[str, int] = {}  # foreground helper and cell-read uses, by helper
        self.stalled: dict[int, str] = {}  # pid -> error, apps whose AT-SPI calls timed out
        self.runtime_blocked: list[str] = []
        self.runtime_warned: list[str] = []
        self.images: list[Path] = []
        self.image_dir: Path | None = None
        self.lock = threading.Lock()

    def route(self, name: str) -> None:
        with self.lock:
            self.routes[name] = self.routes.get(name, 0) + 1

    def refuse(self, rule_id: str, what: str) -> None:
        rule = rawlint.RULES[rule_id]
        instead = (
            rawlint.NATIVE_INSTEAD.get(rule.id, rule.instead) if os.environ.get("RAW_NATIVE") == "1" else rule.instead
        )
        text = (
            f"BLOCKED at run time [{rule.id} {rule.title}]: {what}. {rule.why} Source: {rule.source}. "
            f"Instead: {instead}"
        )
        with self.lock:
            self.runtime_blocked.append(f"{rule.id}: {what}")
        raise Blocked(text)

    def check(self, rule_id: str | None, what: str) -> None:
        if rule_id is None:
            return
        rule = rawlint.RULES[rule_id]
        if rule.severity == "block" or (rule.severity == "granted" and not self.granted):
            self.refuse(rule_id, what)
        if rule.severity == "warn" or (rule.severity == "granted" and self.granted):
            with self.lock:
                self.runtime_warned.append(f"{rule.id}: {what}")


STATE = State()


def _guarded_lib(lib: ctypes.CDLL, label: str) -> Any:
    """A ctypes library whose forbidden symbols refuse at lookup, so a computed name is caught too. The library is
    held in a closure, not an attribute, so the script has no unguarded handle to reach for."""

    class GuardedLib:
        __slots__ = ()

        def __getattr__(self, name: str) -> Any:
            if name.startswith("_"):
                raise AttributeError(name)
            rule = rawlint.NAMES.get(name)
            STATE.check(rule if rule != "GUI1" else None, f"{label}.{name}")
            fn = getattr(lib, name)
            if name == "XInternAtom":

                def intern(dpy: Any, atom: Any, only_if_exists: Any) -> Any:
                    text = atom.decode("latin-1") if isinstance(atom, bytes) else str(atom)
                    for needle, rule_id in rawlint.STRINGS.items():
                        if needle in text:
                            STATE.check(rule_id, f"XInternAtom({needle})")
                    return fn(dpy, atom, only_if_exists)

                return intern
            return fn

        def __setattr__(self, name: str, value: Any) -> None:
            raise Blocked("the raw libraries are read-only")

        def __getitem__(self, name: str) -> Any:
            return self.__getattr__(name)

    return GuardedLib()


def _guard_gi() -> None:
    real_mouse = Atspi.generate_mouse_event
    real_key = Atspi.generate_keyboard_event

    def mouse(*args: Any, **kwargs: Any) -> Any:
        STATE.check("BG1", "Atspi.generate_mouse_event")
        return real_mouse(*args, **kwargs)

    def key(*args: Any, **kwargs: Any) -> Any:
        STATE.check("BG3", "Atspi.generate_keyboard_event")
        return real_key(*args, **kwargs)

    Atspi.generate_mouse_event = mouse
    Atspi.generate_keyboard_event = key
    for cls in (Atspi.Accessible, Atspi.Component):
        real = cls.grab_focus

        def grab(self: Any, _real: Any = real) -> Any:
            STATE.check("BG7", "grab_focus")
            return _real(self)

        cls.grab_focus = grab
    for cls, names in (
        (Atspi.Action, ("do_action",)),
        (Atspi.EditableText, ("set_text_contents", "insert_text", "delete_text", "paste_text")),
        (Atspi.Value, ("set_current_value",)),
        (Atspi.Selection, ("select_child", "deselect_child", "select_all")),
    ):
        for name in names:
            real = getattr(cls, name, None)
            if real is None:
                continue

            def counted(acc: Any, *args: Any, _real: Any = real, **kwargs: Any) -> Any:
                try:
                    app = acc.get_application().get_name() or "?"
                except Exception:  # noqa: BLE001
                    app = "?"
                with STATE.lock:
                    STATE.raw_actions[app] = STATE.raw_actions.get(app, 0) + 1
                return _real(acc, *args, **kwargs)

            setattr(cls, name, counted)


# ------------------------------------------------------------------ the runtime hook


class Watch:
    """Samples pointer, core focus, active window and top of stacking every 50 ms during the script."""

    def __init__(self, interval: float = 0.05) -> None:
        self.x = _X()
        self.interval = interval
        self.before = self.sample()
        self.changes: dict[str, list[Any]] = {
            "pointer": [],
            "focus": [],
            "active": [],
            "top": [],
            "popups": [],
        }
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.loop, daemon=True)

    def sample(self) -> dict[str, Any]:
        return {
            "pointer": self.x.pointer(),
            "focus": self.x.focus(),
            "active": self.x.active(),
            "top": self.x.top(),
            "popups": self.x.popups(),
        }

    def loop(self) -> None:
        last = dict(self.before)
        while not self.stop.wait(self.interval):
            try:
                now = self.sample()
            except Exception:  # noqa: BLE001 - a sample error must not kill the script
                continue
            for key in last:
                if now[key] != last[key]:
                    self.changes[key].append(now[key])
            last = now

    def start(self) -> "Watch":
        self.thread.start()
        return self

    def finish(self) -> dict[str, Any]:
        self.stop.set()
        self.thread.join(timeout=1)
        after = self.sample()
        for key in after:
            if after[key] != self.before[key] and (
                not self.changes[key] or self.changes[key][-1] != after[key]
            ):
                self.changes[key].append(after[key])
        moved = {k: v for k, v in self.changes.items() if v}

        def label(window: int) -> str:
            if not window:
                return "none"
            name = self.x.name(window)
            return f"0x{window:x} {name!r} pid {self.x.pid(window)}"

        lines = []
        if moved.get("pointer"):
            lines.append(
                f"real pointer moved from {self.before['pointer']} to {after['pointer']} "
                f"({len(moved['pointer'])} change(s) sampled)"
            )
        if moved.get("active"):
            lines.append(
                f"active window changed from {label(self.before['active'])} to {label(after['active'])}"
            )
        if moved.get("focus"):
            lines.append(
                f"keyboard focus changed from {label(self.before['focus'])} to {label(after['focus'])}"
            )
        new_popups = sorted(
            {w for seen in moved.get("popups", []) for w in seen} - set(self.before["popups"])
        )
        if new_popups:
            lines.append(
                f"{len(new_popups)} popup window(s) opened during the script (a menu or combo popup takes the "
                "keyboard grab from the user's window while it is open)"
            )
        else:
            moved.pop("popups", None)
        if moved.get("top"):
            lines.append(
                f"top of the stacking order changed from {label(self.before['top'])} to {label(after['top'])}"
            )
        self.x.close()
        return {"before": self.before, "after": after, "changed": sorted(moved), "lines": lines}


# ------------------------------------------------------------------ the script's API


# A spreadsheet's table reports every cell of the sheet as a child (LibreOffice: over a billion); the helpers walk at
# most this many children of one node, so a tree read ends. A script's own loop is the script's business.
MAX_CHILDREN = 1000
MAX_CELLS = 2000  # one raw.cells read
# 0 keeps D-Bus's own 25 s method timeout (libatspi's default; a stalled app costs about 15 s in practice)
ATSPI_TIMEOUT_MS = int(os.environ.get("RAW_ATSPI_TIMEOUT_MS") or 2000)
# apps that stalled in an earlier script of this trial (from the MCP server); raw.app asks them with a short timeout
SUSPECT_PIDS = {int(p) for p in (os.environ.get("RAW_ATSPI_SUSPECT") or "").split(",") if p.strip().isdigit()}
SUSPECT_TIMEOUT_MS = min(300, ATSPI_TIMEOUT_MS) if ATSPI_TIMEOUT_MS > 0 else 300


def _cell_ref(ref: str) -> tuple[int, int]:
    """'B7' -> (row 6, column 1)."""
    ref = ref.strip().upper().replace("$", "")
    letters = "".join(ch for ch in ref if ch.isalpha())
    digits = ref[len(letters) :]
    if not letters or not digits.isdigit() or int(digits) < 1:
        raise ValueError(f"not a cell reference: {ref!r}")
    col = 0
    for ch in letters:
        col = col * 26 + (ord(ch) - 64)
    return int(digits) - 1, col - 1


def _col_name(col: int) -> str:
    name = ""
    col += 1
    while col:
        col, rem = divmod(col - 1, 26)
        name = chr(65 + rem) + name
    return name


class Raw:
    Atspi = Atspi
    Blocked = Blocked

    def __init__(self) -> None:
        self.x11 = _guarded_lib(_X11, "x11")
        self.xtst = _guarded_lib(_XTST, "xtst")
        self.xi = _guarded_lib(_XI, "xi")
        self.dpy = _X11.XOpenDisplay(None)
        self.root = _X11.XDefaultRootWindow(self.dpy)
        self._x = _X()
        self._xdotool = shutil.which("xdotool") or "/usr/bin/xdotool"
        self._restore_to: int | None = None  # the active window before the script's first foreground helper
        self._popups_before: set[int] = set()

    def __getattr__(self, name: str) -> Any:
        if name == "driver":  # the fallback this arm does not have
            STATE.refuse("DRV1", "raw.driver")
        raise AttributeError(name)

    # foreground helpers: the granted routes Cua Driver's foreground delivery took, built from xdotool/XTest
    def _run_xdotool(self, *argv: Any, timeout: float = 20) -> str:
        done = subprocess.run(
            [self._xdotool, *[str(a) for a in argv]], capture_output=True, text=True, timeout=timeout
        )
        if done.returncode != 0:
            raise RuntimeError(
                f"xdotool {' '.join(str(a) for a in argv[:3])} exited {done.returncode}: {done.stderr.strip()[:300]}"
            )
        return done.stdout.strip()

    def _window(self, window_id: Any) -> int:
        wid = int(window_id)
        if wid not in self._x.clients():
            raise RuntimeError(f"window {wid} is not a top-level client window; see raw.windows()")
        return wid

    def activate(self, window_id: int) -> dict[str, Any]:
        """Make ``window_id`` the active window and wait (up to 3 s) until the WM confirms it and the input focus sits
        inside it. Needs the task's grant (BG2). Returns {active, focus_ok}."""
        return self._foreground("activate", "BG2", window_id, lambda: None)

    def _activate(self, wid: int) -> dict[str, Any]:
        deadline = time.monotonic() + 3.0
        if self._x.active() != wid:
            try:
                self._run_xdotool("windowactivate", "--sync", wid, timeout=4)
            except (RuntimeError, subprocess.TimeoutExpired):
                self._run_xdotool("windowraise", wid)
                self._run_xdotool("windowfocus", "--sync", wid, timeout=4)
        while True:
            active = self._x.active()
            focus = self._x.focus()
            # the core focus is on the toplevel or one of its children (a GTK/VCL focus proxy)
            ok = active == wid and self._x.within(focus, wid)
            if ok or time.monotonic() > deadline:
                return {"active": active == wid, "focus_ok": ok}
            time.sleep(0.05)

    def _foreground(
        self, name: str, rule: str, window_id: Any, body: Any, keyboard: bool = False
    ) -> dict[str, Any]:
        STATE.check(rule, f"raw.{name}")
        STATE.route(name)
        if self._restore_to is None:
            self._restore_to = self._x.active()
            self._popups_before = set(self._x.popups())
        out: dict[str, Any] = {}
        if window_id is not None:
            wid = self._window(window_id)
            out.update(self._activate(wid))
            if not out["active"]:
                raise RuntimeError(f"window {wid} did not become active; nothing was sent")
            if keyboard and not out["focus_ok"]:
                # keys go to the X input focus: never send them while it sits outside the target
                raise RuntimeError(f"the input focus did not move into window {wid}; no keys were sent")
        body()
        time.sleep(0.1)
        return out

    def restore_foreground(self) -> dict[str, Any] | None:
        """At the end of a script that used a foreground helper: give the active window back to the window that had
        it before, unless the input left a popup open (restoring would close it). Called by the runner."""
        before = self._restore_to
        if before is None:
            return None
        opened = sorted(set(self._x.popups()) - self._popups_before)
        out = {"to": before, "restored": False, "popups_open": len(opened)}
        if before and before != self._x.active() and not opened and before in self._x.clients():
            try:
                self._run_xdotool("windowactivate", "--sync", before, timeout=4)
                out["restored"] = self._x.active() == before
            except (RuntimeError, subprocess.TimeoutExpired):
                pass
        return out

    def key(self, window_id: int, *keys: str, repeat: int = 1) -> dict[str, Any]:
        """Press keys or chords in ``window_id``, in order (xdotool names: "ctrl+z", "Return", "Tab", "shift+Tab",
        "Down", "F2"), ``repeat`` times. Foreground: needs the task's grant (BG3)."""
        names = [str(k) for k in keys]
        if not names:
            raise RuntimeError("raw.key needs at least one key")
        count = max(1, min(int(repeat), 200))
        return self._foreground(
            "key", "BG3", window_id,
            lambda: self._run_xdotool("key", "--clearmodifiers", "--delay", 30, *(names * count), timeout=60),
            keyboard=True,
        )

    def type_text(self, window_id: int, text: str) -> dict[str, Any]:
        """Type ``text`` into the focused control of ``window_id`` ("\\t" is Tab, "\\n" is Return, sent as key
        presses). Foreground: needs the task's grant (BG3)."""
        text = str(text)

        def body() -> None:
            chunk = ""
            for ch in text + "\0":
                if ch in "\t\n\0":
                    if chunk:
                        self._run_xdotool("type", "--clearmodifiers", "--delay", 12, "--", chunk, timeout=110)
                        chunk = ""
                    if ch != "\0":
                        self._run_xdotool("key", "--clearmodifiers", "Tab" if ch == "\t" else "Return")
                        time.sleep(0.05)
                else:
                    chunk += ch

        return self._foreground("type_text", "BG3", window_id, body, keyboard=True)

    def click(
        self, window_id: int, x: int, y: int, button: int = 1, count: int = 1, screen: bool = False
    ) -> dict[str, Any]:
        """Click at (x, y) in ``window_id``: window coordinates (from the window's top-left, as in its screenshot),
        or screen coordinates with ``screen=True``. ``button`` 1 left, 2 middle, 3 right; ``count`` 2 is a double
        click. Foreground: needs the task's grant (BG1)."""
        wid = self._window(window_id)
        fx, fy, _, _ = self._x.geometry(wid)
        sx, sy = (int(x), int(y)) if screen else (fx + int(x), fy + int(y))
        return self._foreground(
            "click", "BG1", wid,
            lambda: (
                self._run_xdotool("mousemove", "--sync", sx, sy),
                self._run_xdotool("click", "--repeat", max(1, min(int(count), 3)), "--delay", 80, int(button)),
            ),
        )

    def scroll(self, window_id: int, x: int, y: int, clicks: int, screen: bool = False) -> dict[str, Any]:
        """Scroll at (x, y) in ``window_id``: ``clicks`` > 0 down, < 0 up. Foreground: needs the task's grant
        (BG1)."""
        wid = self._window(window_id)
        fx, fy, _, _ = self._x.geometry(wid)
        sx, sy = (int(x), int(y)) if screen else (fx + int(x), fy + int(y))
        n = int(clicks)
        return self._foreground(
            "scroll", "BG1", wid,
            lambda: (
                self._run_xdotool("mousemove", "--sync", sx, sy),
                self._run_xdotool("click", "--repeat", max(1, min(abs(n), 50)), "--delay", 40, 5 if n > 0 else 4),
            ),
        )

    def drag(
        self, x1: int, y1: int, x2: int, y2: int, window_id: int | None = None, steps: int = 20,
        duration: float = 0.6, hold: float = 0.3, screen: bool = False,
    ) -> dict[str, Any]:
        """Press the left button at (x1, y1), move to (x2, y2) in ``steps`` steps over ``duration`` seconds, pause
        ``hold`` seconds and release: a real drag-and-drop. With ``window_id`` the points are window coordinates
        (unless ``screen=True``) and that window is activated first; without it they are screen coordinates.
        Foreground: needs the task's grant (BG1)."""
        ox = oy = 0
        if window_id is not None and not screen:
            ox, oy, _, _ = self._x.geometry(self._window(window_id))
        a = (ox + int(x1), oy + int(y1))
        b = (ox + int(x2), oy + int(y2))
        n = max(2, min(int(steps), 200))

        def body() -> None:
            self._run_xdotool("mousemove", "--sync", *a)
            self._run_xdotool("mousedown", 1)
            try:
                time.sleep(0.15)
                for i in range(1, n + 1):
                    self._run_xdotool(
                        "mousemove", a[0] + (b[0] - a[0]) * i // n, a[1] + (b[1] - a[1]) * i // n
                    )
                    time.sleep(max(0.0, float(duration)) / n)
                time.sleep(max(0.0, min(float(hold), 3.0)))
            finally:
                self._run_xdotool("mouseup", 1)

        return self._foreground("drag", "BG1", window_id, body)

    # reads
    def cells(self, table: Any, ref: str) -> list[list[str]]:
        """The text of a spreadsheet range ("A1:F20", or one cell "B7") as rows of strings, read through the AT-SPI
        Table interface of ``table`` (LibreOffice Calc: the [table] under 'document spreadsheet', or the app, whose
        first table is used). Read-only; at most 2000 cells per call."""
        STATE.route("cells")
        if table is not None and table.get_table_iface() is None:
            found = self.find(table, role="table", limit=1)
            table = found[0] if found else None
        if table is None:
            raise RuntimeError("no AT-SPI table found")
        first, _, last = str(ref).partition(":")
        r1, c1 = _cell_ref(first)
        r2, c2 = _cell_ref(last or first)
        r1, r2 = sorted((r1, r2))
        c1, c2 = sorted((c1, c2))
        if (r2 - r1 + 1) * (c2 - c1 + 1) > MAX_CELLS:
            raise RuntimeError(f"range {ref} is over {MAX_CELLS} cells; read it in parts")
        rows = []
        for r in range(r1, r2 + 1):
            row = []
            for c in range(c1, c2 + 1):
                cell = Atspi.Table.get_accessible_at(table, r, c)
                text = ""
                if cell is not None:
                    if cell.get_text_iface() is not None:
                        text = Atspi.Text.get_text(cell, 0, -1) or ""
                    else:
                        text = cell.get_name() or ""
                row.append(text)
            rows.append(row)
        return rows

    def cell_name(self, row: int, col: int) -> str:
        """Zero-based (row, col) -> "B7"."""
        return f"{_col_name(int(col))}{int(row) + 1}"
    def sleep(self, seconds: float) -> None:
        time.sleep(min(float(seconds), 30.0))

    def pointer(self) -> tuple[int, int]:
        return self._x.pointer()

    def active_window(self) -> int:
        return self._x.active()

    def windows(self) -> list[dict[str, Any]]:
        """Top-level client windows, bottom to top: id, title, pid, screen frame (x, y, w, h)."""
        out = []
        for w in self._x.clients():
            out.append(
                {
                    "window_id": w,
                    "title": self._x.name(w),
                    "pid": self._x.pid(w),
                    "frame": self._x.geometry(w),
                }
            )
        return out

    def app(self, name: str | None = None, pid: int | None = None) -> Any:
        """The AT-SPI application accessible with this name (case-insensitive) or pid, or None."""
        desktop = Atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            a = desktop.get_child_at_index(i)
            if a is None:
                continue
            try:
                app_pid = a.get_process_id()  # answered by the bus daemon, not the app
                if pid is not None and app_pid == pid:
                    return a
                if name is None or app_pid in STATE.stalled:
                    continue
                suspect = app_pid in SUSPECT_PIDS and ATSPI_TIMEOUT_MS > 0
                limit = (SUSPECT_TIMEOUT_MS if suspect else ATSPI_TIMEOUT_MS or 25_000) / 1000
                if suspect:  # stalled in an earlier script of this trial: ask briefly, it may be back
                    Atspi.set_timeout(SUSPECT_TIMEOUT_MS, SUSPECT_TIMEOUT_MS)
                started = time.monotonic()
                try:
                    app_name = a.get_name() or ""
                finally:
                    if suspect:
                        Atspi.set_timeout(ATSPI_TIMEOUT_MS, ATSPI_TIMEOUT_MS)
                if time.monotonic() - started >= limit * 0.9:
                    # an app that does not answer: skip it for the rest of this script
                    with STATE.lock:
                        STATE.stalled.setdefault(app_pid, "no answer within the AT-SPI timeout")
                    continue
                if app_name.casefold() == name.casefold():
                    return a
            except Exception as error:  # noqa: BLE001 - a dead or hung app on the bus
                _note_stall(a, error)
                continue
        return None

    def find(
        self,
        root: Any,
        role: str | None = None,
        name: str | None = None,
        contains: str | None = None,
        showing: bool | None = None,
        limit: int = 200,
    ) -> list[Any]:
        """Descendants of ``root`` matching an AT-SPI role name ("push button"), an exact name or a substring."""
        out: list[Any] = []

        def walk(node: Any, depth: int) -> None:
            if node is None or len(out) >= limit or depth > 60:
                return
            try:
                r = node.get_role_name()
                n = node.get_name() or ""
            except Exception:  # noqa: BLE001
                return
            ok = (role is None or r == role) and (name is None or n == name)
            ok = ok and (contains is None or contains.casefold() in n.casefold())
            if ok and showing is not None:
                ok = node.get_state_set().contains(Atspi.StateType.SHOWING) == showing
            if ok and node is not root:
                out.append(node)
            try:
                count = node.get_child_count()
            except Exception:  # noqa: BLE001
                return
            for i in range(min(count, MAX_CHILDREN)):
                if len(out) >= limit:
                    return
                walk(node.get_child_at_index(i), depth + 1)

        walk(root, 0)
        return out

    def describe(self, node: Any) -> str:
        """One line: role, name, states, screen extents, actions, text or value."""
        try:
            role, name = node.get_role_name(), node.get_name() or ""
        except Exception as error:  # noqa: BLE001
            return f"<gone: {error}>"
        parts = [f"[{role}] {name!r}"]
        states = node.get_state_set()
        flags = [
            s
            for s, t in (
                ("focused", Atspi.StateType.FOCUSED),
                ("showing", Atspi.StateType.SHOWING),
                ("disabled", None),
                ("checked", Atspi.StateType.CHECKED),
                ("selected", Atspi.StateType.SELECTED),
                ("editable", Atspi.StateType.EDITABLE),
                ("expanded", Atspi.StateType.EXPANDED),
            )
            if (t is not None and states.contains(t))
            or (s == "disabled" and not states.contains(Atspi.StateType.ENABLED))
        ]
        if flags:
            parts.append(",".join(flags))
        if node.get_component_iface() is not None:
            e = Atspi.Component.get_extents(node, Atspi.CoordType.SCREEN)
            if e.x > -100000:
                parts.append(f"@({e.x},{e.y} {e.width}x{e.height})")
        if node.get_action_iface() is not None:
            names = [
                Atspi.Action.get_action_name(node, i)
                for i in range(Atspi.Action.get_n_actions(node))
            ]
            if names:
                parts.append("actions=" + "/".join(names))
        if node.get_text_iface() is not None:
            text = Atspi.Text.get_text(node, 0, 200)
            if text and text != name:
                parts.append(f"text={text!r}")
        if node.get_value_iface() is not None:
            parts.append(f"value={Atspi.Value.get_current_value(node)}")
        return " ".join(parts)

    def dump(self, root: Any, depth: int = 12, limit: int = 400, showing_only: bool = True) -> str:
        """An indented text tree of ``root`` (``describe`` per node)."""
        lines: list[str] = []

        def walk(node: Any, level: int) -> None:
            if node is None or len(lines) >= limit or level > depth:
                return
            try:
                if (
                    showing_only
                    and level
                    and not node.get_state_set().contains(Atspi.StateType.SHOWING)
                ):
                    return
                lines.append("  " * level + self.describe(node))
                count = node.get_child_count()
            except Exception:  # noqa: BLE001
                return
            if count > MAX_CHILDREN:
                lines.append("  " * (level + 1) + f"... {count} children (only the first {MAX_CHILDREN} are walked)")
            for i in range(min(count, MAX_CHILDREN)):
                if len(lines) >= limit:
                    return
                walk(node.get_child_at_index(i), level + 1)

        walk(root, 0)
        if len(lines) >= limit:
            lines.append(f"... cut at {limit} nodes")
        return "\n".join(lines)

    def screenshot(
        self, window_id: int | None = None, region: tuple[int, int, int, int] | None = None
    ) -> str:
        """A PNG of the screen, a window's screen frame or a region; it is returned to you with the result."""
        if STATE.image_dir is None:
            raise RuntimeError("no image directory")
        if window_id is not None:
            region = self._x.geometry(int(window_id))
        root = Gdk.get_default_root_window()
        if region is None:
            region = (0, 0, root.get_width(), root.get_height())
        x, y, w, h = (int(v) for v in region)
        pixbuf = Gdk.pixbuf_get_from_window(root, x, y, w, h)
        if pixbuf is None:
            raise RuntimeError(f"no pixels for region {region}")
        path = STATE.image_dir / f"shot-{len(STATE.images) + 1}.png"
        pixbuf.savev(str(path), "png", [], [])
        STATE.images.append(path)
        return f"screenshot {len(STATE.images)} ({w}x{h} at {x},{y})"

    # xdotool
    def xdotool(self, *args: Any) -> str:
        """Run xdotool with these arguments; subcommands that move the pointer, focus or type are checked."""
        argv = [str(a) for a in args]
        if not argv:
            raise RuntimeError("xdotool needs a command")
        commands = rawlint.xdotool_commands(argv)
        if len(commands) > 1:
            STATE.check("BG9", "xdotool command chain (" + " ".join(c for c, _ in commands) + ")")
        for command, windowed in commands:
            rule = rawlint.XDOTOOL.get(command)
            if rule == "GUI1":
                STATE.refuse("GUI1", f"xdotool {command}")
            if rule == "BG3" and windowed:
                STATE.check("BG6", f"xdotool {command} --window")
            elif rule:
                STATE.check(rule, f"xdotool {command}")
        done = subprocess.run([self._xdotool, *argv], capture_output=True, text=True, timeout=20)
        if done.returncode != 0:
            raise RuntimeError(
                f"xdotool {' '.join(argv)} exited {done.returncode}: {done.stderr.strip()[:300]}"
            )
        return done.stdout.strip()


def _note_stall(acc: Any, error: Exception) -> None:
    """Record an app whose AT-SPI call timed out (its pid comes from the bus daemon, which still answers)."""
    if "timeout" not in str(error).casefold():
        return
    try:
        pid = int(acc.get_process_id())
    except Exception:  # noqa: BLE001
        pid = -1
    with STATE.lock:
        STATE.stalled.setdefault(pid, str(error)[:200])


class Native:
    """The no-wrapper arm's harness side (RAW_NATIVE=1): no helpers, only the checked xdotool CLI and the
    end-of-script hand-back the runner calls (nothing to hand back: the arm has no foreground helpers)."""

    def __init__(self) -> None:
        self._xdotool = shutil.which("xdotool") or "/usr/bin/xdotool"

    def xdotool(self, *args: Any) -> str:
        """Run the xdotool CLI with these arguments; the same checks as raw.xdotool."""
        return Raw.xdotool(self, *args)  # type: ignore[arg-type]

    def restore_foreground(self) -> None:
        return None


def _guard_xlib() -> None:
    """The native arm's python-xlib routes under the same rules as their libX11/libXtst names (raw.x11, raw.xtst)."""
    try:
        from Xlib import display as xdisplay
        from Xlib.ext import xtest
        from Xlib.xobject import drawable
    except ImportError:
        return

    def guard(owner: Any, name: str, rule: str, label: str) -> None:
        real = getattr(owner, name, None)
        if real is None:
            return

        def checked(*args: Any, _real: Any = real, **kwargs: Any) -> Any:
            STATE.check(rule, label)
            return _real(*args, **kwargs)

        setattr(owner, name, checked)

    real_fake = xtest.fake_input

    def fake_input(disp: Any, event_type: int, *args: Any, **kwargs: Any) -> Any:
        # X.KeyPress 2 / X.KeyRelease 3 are keys (BG3); ButtonPress/Release and MotionNotify move or press (BG1)
        STATE.check("BG3" if event_type in (2, 3) else "BG1", f"Xlib xtest fake_input({event_type})")
        return real_fake(disp, event_type, *args, **kwargs)

    xtest.fake_input = fake_input
    guard(xtest, "grab_control", "BG4", "Xlib xtest grab_control")
    for cls in (xdisplay.Display, drawable.Window):
        guard(cls, "set_input_focus", "BG2", "Xlib set_input_focus")
        guard(cls, "warp_pointer", "BG1", "Xlib warp_pointer")
        real_send = getattr(cls, "send_event", None)
        if real_send is None:
            continue

        def send_event(self: Any, *args: Any, _real: Any = real_send, **kwargs: Any) -> Any:
            # a ClientMessage naming _NET_ACTIVE_WINDOW or _NET_RESTACK_WINDOW is an activation request (BG2)
            event = kwargs.get("event", args[0] if args and not isinstance(self, xdisplay.Display) else
                               (args[1] if len(args) > 1 else None))
            rule, what = "BG6", "Xlib send_event"
            try:
                kind = getattr(event, "client_type", None)
                disp = self if isinstance(self, xdisplay.Display) else self.display
                name = disp.get_atom_name(kind) if kind else ""
            except Exception:  # noqa: BLE001
                name = ""
            for needle, rule_id in rawlint.STRINGS.items():
                if needle in (name or ""):
                    rule, what = rule_id, f"Xlib send_event({needle})"
            STATE.check(rule, what)
            return _real(self, *args, **kwargs)

        cls.send_event = send_event
    guard(xdisplay.Display, "grab_server", "BG4", "Xlib grab_server")
    for name in ("grab_pointer", "grab_keyboard", "grab_key", "grab_button"):
        guard(drawable.Window, name, "BG4", f"Xlib {name}")
    guard(drawable.Window, "raise_window", "BG2", "Xlib raise_window")
    real_configure = drawable.Window.configure

    def configure(self: Any, onerror: Any = None, **keys: Any) -> Any:
        if "stack_mode" in keys or "sibling" in keys:
            STATE.check("BG2", "Xlib configure(stack_mode)")
        return real_configure(self, onerror=onerror, **keys)

    drawable.Window.configure = configure
    guard(drawable.Window, "circulate", "BG2", "Xlib circulate")
    guard(drawable.Window, "set_selection_owner", "BG8", "Xlib set_selection_owner")


def _guard_ctypes() -> None:
    """A library the native script loads itself (ctypes.CDLL("libXtst.so.6")) must be one of rawlint.NATIVE_LIBS, and
    refuses forbidden symbols at lookup, as raw.x11/raw.xtst did, so a computed name is caught at run time too. The
    runtime's own libraries were loaded before this patch."""
    real_init = ctypes.CDLL.__init__

    def init(self: Any, name: Any, *args: Any, **kwargs: Any) -> None:
        if not rawlint.native_lib_ok(name):
            STATE.refuse("GUI1", f"ctypes.CDLL({name!r})")
        real_init(self, name, *args, **kwargs)

    ctypes.CDLL.__init__ = init  # type: ignore[method-assign]
    real = ctypes.CDLL.__getattr__
    names = {**rawlint.NAMES, **rawlint.NATIVE_NAMES}

    def lookup(self: Any, name: str) -> Any:
        rule = names.get(name)
        if rule and rule not in ("GUI1", "DRV1"):
            STATE.check(rule, f"ctypes {name}")
        return real(self, name)

    real_item = ctypes.CDLL.__getitem__

    def item(self: Any, name: Any) -> Any:
        rule = names.get(name) if isinstance(name, str) else None
        if rule and rule not in ("GUI1", "DRV1"):
            STATE.check(rule, f"ctypes {name}")
        return real_item(self, name)

    ctypes.CDLL.__getattr__ = lookup  # type: ignore[method-assign]
    ctypes.CDLL.__getitem__ = item  # type: ignore[method-assign]


def _guard_gdk() -> None:
    """Gdk's pointer warp and synthetic input under the same rules (the wrapper arm never exposed Gdk)."""
    for owner, name, rule in (
        (Gdk.Device, "warp", "BG1"),
        (Gdk, "test_simulate_button", "BG1"),
        (Gdk, "test_simulate_key", "BG3"),
        (Gdk.Device, "grab", "BG4"),
        (Gdk.Seat, "grab", "BG4"),
        (Gdk, "pointer_grab", "BG4"),
        (Gdk, "keyboard_grab", "BG4"),
        (Gdk.Window, "focus", "BG2"),
        (Gdk.Window, "raise_", "BG2"),
    ):
        real = getattr(owner, name, None)
        if real is None:
            continue

        def checked(*args: Any, _real: Any = real, _rule: str = rule, _name: str = name, **kwargs: Any) -> Any:
            STATE.check(_rule, f"Gdk {_name}")
            return _real(*args, **kwargs)

        setattr(owner, name, checked)


def make_native() -> Native:
    """The native arm's process setup: the same AT-SPI timeout and route checks as make(), and no helper object."""
    Gdk.init([])
    if ATSPI_TIMEOUT_MS > 0:
        Atspi.set_timeout(ATSPI_TIMEOUT_MS, ATSPI_TIMEOUT_MS)
    _guard_gi()
    _guard_xlib()
    _guard_ctypes()
    _guard_gdk()
    return Native()


def make() -> Raw:
    Gdk.init([])
    if ATSPI_TIMEOUT_MS > 0:
        Atspi.set_timeout(ATSPI_TIMEOUT_MS, ATSPI_TIMEOUT_MS)
    _guard_gi()
    return Raw()
