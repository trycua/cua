# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gdk, GLib, Gtk

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
import benchlab_common as C


def xprop(*args: str) -> dict[str, str]:
    done = subprocess.run(["xprop", *args], capture_output=True, text=True, timeout=2)
    return {line.split("(", 1)[0]: line.split(" = ", 1)[-1] for line in done.stdout.splitlines()}


def visible_here(window: int, current: str) -> bool:
    props = xprop("-id", hex(window), "_NET_WM_DESKTOP", "_NET_WM_STATE")
    return props.get("_NET_WM_DESKTOP", "") in ("4294967295", current) and (
        "_NET_WM_STATE_HIDDEN" not in props.get("_NET_WM_STATE", "")
    )


def xid(value: str) -> int:
    match = re.search(r"0x[0-9a-fA-F]+", value)
    return int(match[0], 16) if match else 0


class XTest:
    """Real pointer input through XTest (libX11 + libXtst via ctypes) on a display of our own. XTest events
    cannot be told apart from anyone else's, so there is no tagging on X11."""

    def __init__(self) -> None:
        x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
        xtst = ctypes.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so.6")
        x11.XOpenDisplay.restype = ctypes.c_void_p
        x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        x11.XDefaultRootWindow.restype = ctypes.c_ulong
        x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        x11.XQueryPointer.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, *[ctypes.c_void_p] * 7,
        ]  # fmt: skip
        xtst.XTestFakeMotionEvent.argtypes = (
            [ctypes.c_void_p, ctypes.c_int] + [ctypes.c_int] * 2 + [ctypes.c_ulong]
        )
        xtst.XTestFakeButtonEvent.argtypes = [
            ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong,
        ]  # fmt: skip
        self.x11, self.xtst = x11, xtst
        self.display = x11.XOpenDisplay(None)
        if not self.display:
            raise RuntimeError("cannot open the X display")
        self.root = x11.XDefaultRootWindow(self.display)

    def move(self, x: int, y: int) -> None:
        self.xtst.XTestFakeMotionEvent(self.display, -1, int(x), int(y), 0)
        self.x11.XSync(self.display, 0)

    def button(self, down: bool) -> None:
        self.xtst.XTestFakeButtonEvent(self.display, 1, 1 if down else 0, 0)
        self.x11.XSync(self.display, 0)

    def pointer(self) -> tuple[int, int]:
        root, child = ctypes.c_ulong(), ctypes.c_ulong()
        rx, ry, wx, wy = (ctypes.c_int() for _ in range(4))
        mask = ctypes.c_uint()
        self.x11.XQueryPointer(
            self.display, self.root, ctypes.byref(root), ctypes.byref(child),
            ctypes.byref(rx), ctypes.byref(ry), ctypes.byref(wx), ctypes.byref(wy),
            ctypes.byref(mask),
        )  # fmt: skip
        return rx.value, ry.value

    def close(self) -> None:
        self.x11.XCloseDisplay(ctypes.c_void_p(self.display))


class MouseUser:
    """The simulated user (SHARED-DESKTOP.md, "Mouse user script"), a GLib timeout state machine on the main loop,
    so scripted input, checks and the canvas's button handlers never run concurrently. A schema-1 script (UM-01)
    has clicks only; a schema-2 script (UM-02) has a `plan` of click and drag legs and a finer pointer."""

    TICK_MS = 4  # finer than the fastest step_ms (8); waits are measured on the monotonic clock, not counted in ticks

    def __init__(self, sentinel: "Sentinel", script: dict) -> None:
        self.s = sentinel
        self.script = script
        # schema 1 has no plan: its sequence is a plan of click legs, so UM-01 behaves exactly as before
        self.v2 = "plan" in script
        self.plan = script.get("plan") or [{"kind": "click", "dot": d} for d in script["sequence"]]
        self.xt: XTest | None = None
        self.running = False
        self.stopping = False
        GLib.timeout_add(self.TICK_MS, self.tick)

    # --- canvas geometry
    def origin(self) -> tuple[int, int] | None:
        gdk_window = self.s.canvas.get_window()
        if gdk_window is None or not self.s.canvas.get_mapped():
            return None
        found = gdk_window.get_origin()
        return int(found[-2]), int(found[-1])

    def to_screen(self, p: tuple[float, float]) -> tuple[int, int] | None:
        o = self.origin()
        return None if o is None else (round(p[0]) + o[0], round(p[1]) + o[1])

    def dot_xy(self, index: int) -> tuple[int, int]:
        dot = self.script["dots"][index]
        return dot["x"], dot["y"]

    # --- lifecycle
    def start(self) -> None:
        extra = (
            {
                "click_timeout_ms": self.script["click_timeout_ms"],
                "drag_timeout_ms": self.script["drag_timeout_ms"],
            }
            if self.v2
            else {}
        )
        self.s.emit(
            "mu_start",
            tagging=False,
            tolerance_px=self.script.get("tolerance_px", 3),
            platform="linux-x11",
            **extra,
        )
        try:
            self.xt = self.xt or XTest()
        except (OSError, RuntimeError, AttributeError) as error:
            self.s.emit("mu_error", why=f"cannot post input through XTest: {error}")
            return
        self.cmd: list[tuple[float, float]] = []  # commanded canvas points, newest last
        self.cur = tuple(self.script["home"])
        self.leg = 0
        self.points: list[tuple[int, int]] = []
        self.k = 0
        self.checks = 0
        self.clicks = 0
        self.pending: dict | None = None
        self.open: list[
            dict
        ] = []  # schema 2: released actions still waiting for their button events
        self.expect_up: list[
            dict
        ] = []  # schema 2: our own releases, so the canvas can tell them from leaks
        self.action: dict = {}
        self.down = False
        self.state = "home"
        self.t_start = time.monotonic()
        self.next_at = 0.0
        self.running = True

    def stop(self) -> None:
        if not self.running:
            return
        self.stopping = True
        # stopped between the button-down and the button-up: that click (or drag) is dropped
        if self.down and self.xt:
            self.xt.button(False)
            self.down = False
            self.pending = None
        if self.pending and self.pending["logged"]:
            # the click is logged; give a button-down already posted a moment to reach the canvas
            end = time.monotonic() + 0.06
            while time.monotonic() < end and not self.pending.get("rx"):
                Gtk.main_iteration_do(False)
            self.resolve(force=True)
        for p in list(self.open):
            self.resolve(p, force=True)
        self.running = False
        self.stopping = False
        self.pending = None
        self.open = []
        self.expect_up = []
        self.s.emit("mu_stop", clicks=self.clicks, checks=self.checks)

    def fail(self, why: str) -> None:
        self.s.emit("mu_error", why=why)
        self.running = False
        if self.down and self.xt:
            self.xt.button(False)
            self.down = False

    # --- one check: the real pointer against the script
    def check(self) -> None:
        o = self.origin()
        if o is None or not self.cmd:
            return
        px, py = self.xt.pointer()
        got = (px - o[0], py - o[1])
        dists = [((got[0] - c[0]) ** 2 + (got[1] - c[1]) ** 2) ** 0.5 for c in self.cmd[-2:]]
        self.s.emit(
            "mu", i=self.checks, want=list(self.cmd[-1]), got=list(got), d=round(min(dists), 2)
        )
        self.checks += 1

    def post_move(self, p: tuple[float, float]) -> None:
        screen = self.to_screen(p)
        if screen is not None:
            self.xt.move(*screen)
        self.cmd.append(p)

    def wait(self, ms: float, now: float) -> None:
        self.next_at = max(self.next_at + ms / 1000.0, now) if self.next_at else now + ms / 1000.0

    # --- the button handlers of the canvas
    def rx(self, event: Gdk.EventButton) -> None:
        at = (event.x, event.y)
        dot = C.dot_at(self.script, *at)
        own = False
        p = self.pending
        if (
            p
            and not p.get("rx")
            and time.monotonic() - p["t"] <= self.script["click_timeout_ms"] / 1000
        ):
            tol = self.script.get("tolerance_px", 3)
            if ((at[0] - p["at"][0]) ** 2 + (at[1] - p["at"][1]) ** 2) ** 0.5 <= tol:
                own = True
            p["rx"] = {"dot": dot, "at": at, "own": own}
        self.s.emit("mu_rx", dot=dot, at=[round(at[0], 1), round(at[1], 1)], own=own)
        if not own:  # a button-down that is not ours is a leak, as on any other window
            self.s.emit("mouseDown")

    def up(self, event: Gdk.EventButton) -> None:
        """The canvas's button-up handler (schema 2 only: a UM-01 log has no button-up events). Own = the release
        we posted for the action in flight, at its end dot, in time, with a button-down of ours before it."""
        if not self.v2:
            return
        at = (event.x, event.y)
        dot = C.dot_at(self.script, *at)
        now = time.monotonic()
        tol = self.script.get("tolerance_px", 3)
        own = False
        mine = None
        for entry in self.expect_up:
            p = entry["p"]
            near = ((at[0] - entry["at"][0]) ** 2 + (at[1] - entry["at"][1]) ** 2) ** 0.5 <= tol
            down_ok = bool(p.get("rx") and p["rx"]["own"])
            if (
                near
                and now <= entry["deadline"]
                and not p.get("up")
                and (down_ok or entry["kind"] == "click")
            ):
                own, mine = True, entry
                break
        if mine is not None:
            mine["p"]["up"] = {"dot": dot, "at": at, "own": True}
            self.expect_up.remove(mine)
        else:
            # a release that is not ours: it cuts the drag in flight (if its down was ours and no release came yet)
            for p in [*self.open, *([self.pending] if self.pending else [])]:
                if p.get("kind") == "drag" and p.get("rx") and p["rx"]["own"] and not p.get("up"):
                    p["up"] = {"dot": dot, "at": at, "own": False}
        self.s.emit("mu_up", dot=dot, at=[round(at[0], 1), round(at[1], 1)], own=own)
        if not own:
            self.s.emit("mouseUp")

    def resolve(self, p: dict | None = None, force: bool = False) -> None:
        if p is None:
            p = self.pending
        if not p or not p.get("logged"):
            return
        drag = p.get("kind") == "drag"
        rx = p.get("rx")
        timeout = self.script["drag_timeout_ms" if drag else "click_timeout_ms"] / 1000
        late = time.monotonic() - (p["released_t"] if drag else p["t"]) > timeout
        if drag:
            up = p.get("up")
            if not (late or force or (rx is not None and up is not None)):
                return
            if rx is None:
                ok, why = False, "lost"
            elif not rx["own"]:
                ok, why = False, ("off_target" if rx["dot"] is None else "wrong_dot")
            elif up is not None and up["own"] and up["dot"] == p["to"]:
                ok, why = True, "landed"
            else:
                ok, why = False, "cut"
            self.s.emit("mu_click_result", n=p["n"], ok=ok, why=why, kind="drag")
        else:
            if rx is None and not (late or force):
                return
            if rx is None:
                ok, why = False, "lost"
            elif rx["own"] and rx["dot"] == p["dot"]:
                ok, why = True, "landed"
            elif rx["dot"] is None:
                ok, why = False, "off_target"
            else:
                ok, why = False, "wrong_dot"
            self.s.emit("mu_click_result", n=p["n"], ok=ok, why=why)
        if p is self.pending:
            self.pending = None
        if p in self.open:
            self.open.remove(p)

    def release(self, now: float, p: dict, at: tuple[float, float], timeout_ms: float) -> None:
        """Post our button-up; schema 2 first records it as expected so the canvas can tell it from a leak."""
        if p.get("kind") == "drag":
            screen = self.to_screen(at)
            if screen is not None:
                self.xt.move(
                    *screen
                )  # a drag ends at the end dot's centre, wherever the pointer really is
        p["released_t"] = now
        if self.v2:
            self.expect_up.append(
                {
                    "p": p,
                    "at": at,
                    "kind": p.get("kind", "click"),
                    "deadline": now + timeout_ms / 1000,
                }
            )
        self.xt.button(False)
        self.down = False

    # --- the state machine
    def tick(self) -> bool:
        if not self.running or self.stopping:
            return True
        now = time.monotonic()
        if self.pending:
            self.resolve()
        for p in list(self.open):
            self.resolve(p)
        self.expect_up = [e for e in self.expect_up if e["deadline"] + 0.5 > now]
        if now < self.next_at - 0.002:
            return True
        script = self.script
        if self.state == "home":
            home = tuple(script["home"])
            screen = self.to_screen(home)
            if screen is None:
                self.fail("the canvas is not mapped")
                return True
            self.xt.move(*screen)
            self.cmd.append(home)
            self.state = "verify"
            self.t_home = now
        elif self.state == "verify":
            o = self.origin()
            px, py = self.xt.pointer()
            want = self.cmd[-1]
            if o and abs(px - o[0] - want[0]) <= 1 and abs(py - o[1] - want[1]) <= 1:
                self.state = "plan"
            elif now - self.t_home > 0.2:
                self.fail("the pointer did not reach home within 200 ms")
                return True
        if self.state == "plan":
            leg = self.plan[self.leg % len(self.plan)]
            self.action = leg
            first = leg["from"] if leg["kind"] == "drag" else leg["dot"]
            self.target = self.dot_xy(first)
            self.points = C.mouse_leg(self.cur, self.target, script["step_px"])
            self.k = 0
            self.state = "move"
            self.next_at = now
        elif self.state == "move":
            self.check()
            self.post_move(self.points[self.k])
            self.k += 1
            if self.k >= len(self.points):
                self.state = "arrive"
            self.wait(script["step_ms"], now)
        elif self.state == "arrive":
            self.state = "press"
            self.wait(script["dwell_before_ms"], now)
        elif self.state == "press":
            self.check()
            screen = self.to_screen(self.target)
            if screen is not None:
                self.xt.move(
                    *screen
                )  # the click is at the dot's centre, wherever the pointer really is
            self.clicks += 1
            if self.pending and self.v2:
                self.open.append(
                    self.pending
                )  # still waiting for its button events: keep resolving it
            leg = self.action
            self.pending = {
                "n": self.clicks, "dot": leg.get("dot"),
                "at": self.target, "t": now, "rx": None, "logged": False,
            }  # fmt: skip
            if leg["kind"] == "drag":
                end = self.dot_xy(leg["to"])
                self.pending.update(kind="drag", dot=leg["from"], to=leg["to"], to_at=end, up=None)
                self.points = C.mouse_leg(self.target, end, script["drag_step_px"])
                self.k = 0
            self.xt.button(True)
            self.down = True
            if leg["kind"] == "drag":
                self.state = "drag_move"
                self.wait(script["drag_step_ms"], now)
            else:
                self.state = "release"
                self.wait(script["press_ms"], now)
        elif self.state == "drag_move":
            self.check()
            self.post_move(self.points[self.k])  # plain motion with the button held
            self.k += 1
            if self.k >= len(self.points):
                self.state = "drag_release"
                self.wait(script["drag_hold_ms"], now)
            else:
                self.wait(script["drag_step_ms"], now)
        elif self.state == "drag_release":
            p = self.pending
            self.release(now, p, p["to_at"], script["drag_timeout_ms"])
            p["logged"] = True
            self.s.emit(
                "mu_drag", n=p["n"], to=p["to"], at_from=[p["at"][0], p["at"][1]],
                at_to=[p["to_at"][0], p["to_at"][1]], **{"from": p["dot"]},
            )  # fmt: skip
            self.cur = p["to_at"]
            self.leg += 1
            self.state = "plan"
            self.wait(script["dwell_after_ms"], now)
            self.resolve()
        elif self.state == "release":
            self.release(now, self.pending, self.target, script["click_timeout_ms"])
            self.pending["logged"] = True
            self.s.emit(
                "mu_click", n=self.pending["n"], dot=self.pending["dot"],
                at=[self.target[0], self.target[1]],
            )  # fmt: skip
            self.cur = self.target
            self.leg += 1
            self.state = "plan"
            self.wait(script["dwell_after_ms"], now)
            self.resolve()
        return True


class Sentinel:
    def __init__(self, args: argparse.Namespace) -> None:
        ctypes.CDLL(None).prctl(15, b"BenchSentinel", 0, 0, 0)
        GLib.set_prgname("BenchSentinel")
        GLib.set_application_name("BenchSentinel")
        self.args = args
        args.log.parent.mkdir(parents=True, exist_ok=True)
        self.log = args.log.open("a", buffering=1)
        self.armed = False
        self.clip = 0
        self.expected = ""
        self.previous = None
        self.mouse = None
        self.window = Gtk.Window(
            title="User activity" if args.user_activity or args.mouse_user else "Bench Sentinel"
        )
        self.window.set_wmclass("BenchSentinel", "BenchSentinel")
        self.window.set_default_size(600, 320)
        if args.frame:
            x, y, w, h = map(int, args.frame.split(","))
            self.window.move(x, y)
            self.window.resize(w, h)
        self.entry = Gtk.Entry()
        self.entry.get_accessible().set_name("User activity text")
        self.entry.connect("changed", self.changed)
        self.window.connect("destroy", Gtk.main_quit)
        self.window.connect("focus-out-event", lambda *_: self.emit("didResignKey"))
        self.window.connect("focus-in-event", lambda *_: self.emit("didBecomeActive"))
        if args.mouse_user:
            # UM-01: the window's content is the canvas the simulated user clicks on (no typing user).
            self.script = json.loads(args.mouse_user.read_text(encoding="utf-8"))
            self.canvas = Gtk.DrawingArea()
            self.canvas.set_size_request(*self.script["canvas"])
            self.canvas.set_halign(Gtk.Align.START)
            self.canvas.set_valign(Gtk.Align.START)
            self.canvas.set_can_focus(True)
            self.canvas.add_events(
                Gdk.EventMask.BUTTON_PRESS_MASK
                | Gdk.EventMask.BUTTON_RELEASE_MASK
                | Gdk.EventMask.KEY_PRESS_MASK
                | Gdk.EventMask.SCROLL_MASK
            )
            self.canvas.connect("draw", self.draw_canvas)
            self.canvas.get_accessible().set_name("User activity canvas")
            self.canvas.connect("button-press-event", self.canvas_press)
            self.canvas.connect("button-release-event", self.canvas_release)
            self.canvas.connect("key-press-event", lambda *_: self.emit("keyDown"))
            self.canvas.connect("scroll-event", lambda *_: self.emit("scrollWheel"))
            self.window.add(self.canvas)
            self.focus_widget = self.canvas
        else:
            self.window.add(self.entry)
            self.focus_widget = self.entry
            self.entry.connect("key-press-event", lambda *_: self.emit("keyDown"))
            self.entry.connect("button-press-event", lambda *_: self.emit("mouseDown"))
            self.entry.connect("scroll-event", lambda *_: self.emit("scrollWheel"))
        self.clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
        self.clipboard.connect("owner-change", self.clip_changed)
        self.window.show_all()
        self.own = self.window.get_window().get_xid()
        if not args.no_activate:
            self.window.present()
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, self.activate)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR2, self.toggle)
        self.listener = Atspi.EventListener.new(self.ax_focus, None)
        self.listener.register("object:state-changed:focused")
        GLib.timeout_add(200, self.sample)
        if args.user_activity:
            GLib.timeout_add(500, self.activity)
        if args.mouse_user:
            self.mouse = MouseUser(self, self.script)
        self.emit("ready")

    def draw_canvas(self, widget: Gtk.DrawingArea, cr: object) -> bool:
        try:  # needs the cairo foreign binding (python3-gi-cairo); the canvas still takes input without it
            width, height = self.script["canvas"]
            cr.set_source_rgb(0.96, 0.96, 0.94)
            cr.rectangle(0, 0, width, height)
            cr.fill()
            for dot in self.script["dots"]:
                cr.set_source_rgb(0.2, 0.45, 0.8)
                cr.arc(dot["x"], dot["y"], self.script["radius"], 0, 6.2832)
                cr.fill()
                cr.set_source_rgb(1, 1, 1)
                cr.set_font_size(16)
                label = str(dot["index"] + 1)
                x_bearing, y_bearing, w, h = cr.text_extents(label)[:4]
                cr.move_to(dot["x"] - w / 2 - x_bearing, dot["y"] - h / 2 - y_bearing)
                cr.show_text(label)
        except Exception:  # noqa: BLE001
            pass
        return False

    def canvas_press(self, widget: Gtk.DrawingArea, event: Gdk.EventButton) -> bool:
        if event.type == Gdk.EventType.BUTTON_PRESS and self.mouse is not None:
            if self.armed:
                self.mouse.rx(event)
        return True

    def canvas_release(self, widget: Gtk.DrawingArea, event: Gdk.EventButton) -> bool:
        if self.mouse is not None and self.armed:
            self.mouse.up(event)
        return True

    def clip_changed(self, *_: object) -> None:
        self.clip += 1

    def changed(self, *_: object) -> None:
        if self.entry.get_text() != self.expected:
            self.emit("user_text_disrupted")

    def ax_focus(self, event: object, *_: object) -> None:
        if self.armed:
            self.emit("ax_focus", name=event.source.get_name(), focused=bool(event.detail1))

    def identity(self, window: int) -> dict:
        if window == self.own:
            return {"bid": "ai.cua.benchsentinel", "pid": os.getpid()}
        props = xprop("-id", hex(window), "_NET_WM_PID", "WM_CLASS") if window else {}
        pid = props.get("_NET_WM_PID", "")
        names = re.findall(r'"([^"]+)"', props.get("WM_CLASS", ""))
        return {
            "bid": names[-1] if names else f"x11:{window}",
            "pid": int(pid) if pid.isdigit() else -1,
        }

    def snapshot(self) -> dict:
        root = xprop(
            "-root", "_NET_ACTIVE_WINDOW", "_NET_CLIENT_LIST_STACKING", "_NET_CURRENT_DESKTOP"
        )
        front = self.identity(xid(root.get("_NET_ACTIVE_WINDOW", "")))
        pointer = Gdk.Display.get_default().get_default_seat().get_pointer().get_position()
        own = xprop("-id", hex(self.own), "_NET_WM_DESKTOP")
        desktop = own.get("_NET_WM_DESKTOP", "")
        stack = [
            int(v, 16)
            for v in re.findall(r"0x[0-9a-fA-F]+", root.get("_NET_CLIENT_LIST_STACKING", ""))
        ]
        above = (
            [
                self.identity(w)["bid"]
                for w in stack[stack.index(self.own) + 1 :]
                if visible_here(w, root.get("_NET_CURRENT_DESKTOP", "").strip())
            ]
            if self.own in stack
            else []
        )
        sheet = any(
            xid(xprop("-id", hex(w), "WM_TRANSIENT_FOR").get("WM_TRANSIENT_FOR", "")) == self.own
            for w in stack
            if w != self.own
        )
        return {
            "front": front,
            "mouse": [pointer.x, pointer.y],
            "clip": self.clip,
            "onspace": desktop in ("4294967295", root.get("_NET_CURRENT_DESKTOP")),
            "sheet": sheet,
            "above": above,
            "above_n": len(above),
        }

    def emit(self, event: str, **details: object) -> bool:
        if self.armed or event in ("armed", "disarmed", "ready"):
            self.log.write(json.dumps({"t": time.time() * 1000, "ev": event, **details}) + "\n")
        return False

    def activate(self) -> bool:
        self.window.present()
        self.focus_widget.grab_focus()
        return True

    def toggle(self) -> bool:
        if self.armed:
            if self.args.user_activity:
                self.emit("user_text", intact=self.entry.get_text() == self.expected)
            if self.mouse is not None:
                self.mouse.stop()
            self.emit("disarmed", **self.snapshot())
            self.armed = False
        else:
            self.armed = True
            self.emit("armed", **self.snapshot())
            if self.mouse is not None:
                self.mouse.start()
        return True

    def sample(self) -> bool:
        if self.armed:
            sample = self.snapshot()
            if sample["front"] != self.previous:
                self.emit("front", **sample)
                if self.previous and self.previous.get("bid") == "ai.cua.benchsentinel":
                    self.emit("didResignActive")
            self.previous = sample["front"]
            self.emit("s", **sample)
        return True

    def activity(self) -> bool:
        if self.armed:
            if self.snapshot()["front"].get("bid") == "ai.cua.benchsentinel":
                if self.entry.get_text() != self.expected:
                    self.emit("user_text_disrupted")
                self.expected += "a"
                self.entry.set_text(self.expected)
                self.emit("user_type")
            else:
                self.emit("user_blocked")
        return True


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--no-activate", action="store_true")
    parser.add_argument("--user-activity", action="store_true")
    parser.add_argument("--frame")
    parser.add_argument("--mouse-user", type=Path, help="UM-01: the mouse-user script (JSON)")
    witness = Sentinel(parser.parse_args())
    try:
        Gtk.main()
    finally:
        witness.listener.deregister("object:state-changed:focused")
        witness.log.close()


if __name__ == "__main__":
    main()
