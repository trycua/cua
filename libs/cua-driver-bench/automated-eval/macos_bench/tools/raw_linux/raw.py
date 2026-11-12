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
* Helpers (read-back only, no input): ``raw.windows()``, ``raw.app(name)``, ``raw.find(...)``, ``raw.dump(...)``,
  ``raw.screenshot(...)``, ``raw.pointer()``, ``raw.active_window()``, ``raw.sleep(s)``.
* ``raw.driver(tool, args)``: one Cua Driver tool call (the fallback for apps AT-SPI covers only in part), through
  the trial's driver daemon and session. Foreground delivery needs the task's grant, like the raw routes.

Foreground is granted by the harness per task (RAW_GRANT=1 in the runner's environment, from the task definition),
never by the script.

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
        self.driver_calls: list[dict[str, Any]] = []
        self.raw_actions: dict[str, int] = {}  # semantic AT-SPI actions per app name
        self.runtime_blocked: list[str] = []
        self.runtime_warned: list[str] = []
        self.images: list[Path] = []
        self.image_dir: Path | None = None
        self.lock = threading.Lock()

    def refuse(self, rule_id: str, what: str) -> None:
        rule = rawlint.RULES[rule_id]
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

    # the Cua Driver fallback
    def driver(self, tool: str, args: dict | None = None, image: bool = False) -> Any:
        """Call one Cua Driver tool (GUI reads and actions only) and return its JSON result. Screenshots in the
        result are dropped unless ``image=True``, which returns them to you as images."""
        tool = str(tool)
        args = dict(args or {})
        if tool not in rawlint.DRIVER_TOOLS:
            STATE.refuse("GUI1", f"raw.driver({tool!r})")
        why = rawlint.driver_foreground(tool, args)
        if why:
            STATE.check("BG10", f"raw.driver: {why}")
        socket, session, binary = (os.environ.get(k) for k in ("RAW_DRIVER_SOCKET", "RAW_DRIVER_SESSION", "RAW_DRIVER_BIN"))
        if not (socket and binary):
            raise RuntimeError("no Cua Driver is configured for this trial")
        args["session"] = session or "raw-linux"
        entry: dict[str, Any] = {"tool": tool, "pid": args.get("pid"), "foreground": bool(why)}
        started = time.monotonic()
        try:
            done = subprocess.run(
                [binary, "call", "--socket", socket, tool, json.dumps(args)],
                capture_output=True,
                text=True,
                timeout=90,
            )
        finally:
            entry["elapsed_s"] = round(time.monotonic() - started, 3)
        text = done.stdout.strip()
        try:
            result: Any = json.loads(text) if text else {}
        except ValueError:
            result = {"text": text[-4000:]}
        entry["ok"] = done.returncode == 0
        if isinstance(result, dict):
            refusal = result.get("refusal") or {}
            entry["refusal"] = refusal.get("code") if isinstance(refusal, dict) else None
            for key in [k for k in result if k.endswith("_png_b64") or k.endswith("image_b64")]:
                data = result.pop(key)
                if image and STATE.image_dir is not None and isinstance(data, str):
                    import base64

                    path = STATE.image_dir / f"shot-{len(STATE.images) + 1}.png"
                    path.write_bytes(base64.b64decode(data))
                    STATE.images.append(path)
                    result[key.replace("_b64", "")] = f"returned as image {len(STATE.images)}"
        with STATE.lock:
            STATE.driver_calls.append(entry)
        if done.returncode != 0:
            raise RuntimeError(f"cua-driver {tool} failed: {(done.stderr.strip() or text)[-1500:]}")
        return result

    # reads
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
                if (pid is not None and a.get_process_id() == pid) or (
                    name is not None and (a.get_name() or "").casefold() == name.casefold()
                ):
                    return a
            except Exception:  # noqa: BLE001 - a dead app on the bus
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


def make() -> Raw:
    Gdk.init([])
    _guard_gi()
    return Raw()
