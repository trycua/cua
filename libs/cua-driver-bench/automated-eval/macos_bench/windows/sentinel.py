# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""BenchSentinel for Windows: the witness / "User activity" window and its disturbance log.

A port of linux/sentinel.py with the same JSONL log, read by swift/summarize_sentinel.py unchanged.
Windows has no SIGUSR1/SIGUSR2, so the runner controls it through a control file (--control): each line
is a command, `activate`, `arm`, `disarm` or `quit`, applied once in order.

Measured while armed, every 200 ms:
  front     GetForegroundWindow -> process stem (the sentinel itself is ai.cua.benchsentinel)
  mouse     GetCursorPos (the real pointer)
  clip      GetClipboardSequenceNumber
  onspace   IVirtualDesktopManager::IsWindowOnCurrentVirtualDesktop on the sentinel window
  sheet     a shown window owned by the sentinel window (a dialog on the user's app)
  above     shown top-level windows above the sentinel in the z-order that overlap it
Window events: WM_ACTIVATE (didBecomeActive / didResignKey), key, click and wheel input landing in the
user window (keyDown, mouseDown, scrollWheel), and the simulated user's typing (user_type, user_blocked,
user_text_disrupted, user_text).

With --mouse-user PATH (UM-01, SHARED-DESKTOP.md) the user window holds a canvas with numbered dots instead of
a text field, and while armed the sentinel's user moves and clicks the real pointer along the script at PATH
(SendInput, every event tagged with dwExtraInfo = winenv.MAGIC). A WH_MOUSE_LL hook logs every pointer event
that is not its own as mu_foreign. It writes mu_start, mu (the check), mu_click, mu_rx, mu_click_result,
mu_error and mu_stop.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import sys
import time
from pathlib import Path

import wx

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
import benchlab_common as C  # noqa: E402
import winenv as W  # noqa: E402

BID = "ai.cua.benchsentinel"
CANVAS_MARGIN = 8


class MouseUser:
    """The simulated user of UM-01: follows the script at --mouse-user while the sentinel is armed.

    A wx.Timer state machine on the GUI thread (the one that owns the hook and pumps its messages), driven by
    deadlines: home -> (leg: check + move per point, dwell, check, press, release)*. The canvas reports every
    button-down it receives to `canvas_down`, which resolves the scripted clicks."""

    TICK_MS = 10

    def __init__(self, owner: Sentinel, script: dict, canvas: wx.Window) -> None:
        self.owner, self.script, self.canvas = owner, script, canvas
        self.tolerance = float(script.get("tolerance_px", 3))
        self.step_s = script["step_ms"] / 1000
        self.timeout_s = script.get("click_timeout_ms", 400) / 1000
        self.hook = W.MouseHook()
        self.tagging = self.hook.handle is not None
        self.active = False
        self.finishing = False
        self.timer = wx.Timer(owner.frame)
        owner.frame.Bind(wx.EVT_TIMER, lambda _: self.tick(), self.timer)

    # -- geometry and input

    def local(self) -> tuple[int, int]:
        """The real pointer in canvas coordinates (physical pixels, from the canvas's current origin)."""
        x, y = W.cursor()
        ox, oy = W.client_origin(self.canvas.GetHandle())
        return x - ox, y - oy

    def post(self, point: tuple[int, int], buttons: int = 0) -> None:
        ox, oy = W.client_origin(self.canvas.GetHandle())
        W.send_mouse(ox + point[0], oy + point[1], buttons)

    def check(self) -> None:
        got = self.local()
        d = W.distance(got, self.last)
        if self.prev is not None:
            d = min(d, W.distance(got, self.prev))
        self.owner.emit("mu", i=self.checks, want=list(self.last), got=list(got), d=round(d, 2))
        self.checks += 1

    # -- control

    def start(self) -> None:
        self.owner.emit(
            "mu_start", tagging=self.tagging, tolerance_px=self.tolerance, platform="windows"
        )
        self.checks = self.clicks = self.leg = self.index = 0
        self.pending: list[dict] = []
        self.pressed = False
        self.prev = None
        self.last = tuple(self.script["home"])
        self.hook.reset()
        self.hook.enabled = self.tagging
        self.active, self.finishing = True, False
        self.post(self.last)
        now = time.monotonic()
        self.phase, self.next_at, self.home_by = "home", now + 0.02, now + 0.2
        self.timer.Start(self.TICK_MS)

    def request_stop(self) -> None:
        """Disarmed: stop at once (never leave a button down), then wait for the clicks in flight."""
        if self.pressed:
            self.post(self.last, W.MOUSEEVENTF_LEFTUP)
            self.pressed = False
        self.active, self.finishing = False, True
        self.owner.emit("mu_stop", clicks=self.clicks, checks=self.checks)

    def stop_timer(self) -> None:
        self.timer.Stop()
        self.hook.enabled = False
        self.finishing = False

    def tick(self) -> None:
        now = time.monotonic()
        self.drain()
        for p in self.pending:
            if not p["done"] and now - p["t"] > self.timeout_s:
                self.resolve(p, False, "lost")
        self.pending = [p for p in self.pending if not p["done"]]
        if self.finishing:
            if not self.pending:
                self.stop_timer()
                self.owner.finish_disarm()
            return
        if self.active and now >= self.next_at:
            getattr(self, "do_" + self.phase)(now)

    def drain(self) -> None:
        events, self.hook.foreign[:] = list(self.hook.foreign), []
        if self.owner.armed:
            for t, kind in events:
                self.owner.log.write(json.dumps({"t": t, "ev": "mu_foreign", "kind": kind}) + "\n")

    # -- phases

    def advance(self, now: float, seconds: float) -> None:
        self.next_at = max(self.next_at + seconds, now)

    def do_home(self, now: float) -> None:
        if W.distance(self.local(), self.last) <= self.tolerance:
            self.begin_leg(now)
        elif now > self.home_by:
            self.owner.emit("mu_error", why="the pointer did not reach home within 200 ms")
            self.active = False

    def begin_leg(self, now: float) -> None:
        dots = self.script["dots"]
        sequence = self.script["sequence"]
        self.target = dots[sequence[self.leg % len(sequence)]]
        self.points = C.mouse_leg(
            self.last, (self.target["x"], self.target["y"]), self.script["step_px"]
        )
        self.index = 0
        self.phase = "step"
        self.next_at = now

    def do_step(self, now: float) -> None:
        self.check()
        point = self.points[self.index]
        self.post(point)
        self.prev, self.last = self.last, point
        self.index += 1
        if self.index < len(self.points):
            self.advance(now, self.step_s)
        else:
            self.phase = "press"
            self.advance(now, self.step_s + self.script["dwell_before_ms"] / 1000)

    def do_press(self, now: float) -> None:
        self.check()
        self.post(self.last, W.MOUSEEVENTF_LEFTDOWN)
        self.pressed = True
        self.clicks += 1
        at = [self.target["x"], self.target["y"]]
        self.click = {
            "n": self.clicks,
            "dot": self.target["index"],
            "at": at,
            "t": now,
            "done": False,
        }
        self.pending.append(self.click)
        self.phase = "release"
        self.advance(now, self.script["press_ms"] / 1000)

    def do_release(self, now: float) -> None:
        self.post(self.last, W.MOUSEEVENTF_LEFTUP)
        self.pressed = False
        p = self.click
        self.owner.emit("mu_click", n=p["n"], dot=p["dot"], at=p["at"])
        self.leg += 1
        self.advance(now, self.script["dwell_after_ms"] / 1000)
        self.begin_leg(self.next_at)

    # -- clicks

    def resolve(self, pending: dict, ok: bool, why: str) -> None:
        pending["done"] = True
        self.owner.emit("mu_click_result", n=pending["n"], ok=ok, why=why)

    def take_own(self, at: tuple[int, int] | None) -> bool:
        """Is a button-down the script's own? By its tag where the hook runs, else by matching a pending click."""
        now = time.monotonic()
        if self.tagging:
            self.hook.own_downs[:] = [t for t in self.hook.own_downs if now - t <= 0.5]
            if self.hook.own_downs:
                self.hook.own_downs.pop(0)
                return True
            return False
        return at is not None and any(
            not p["done"]
            and now - p["t"] <= self.timeout_s
            and W.distance(at, p["at"]) <= self.tolerance
            for p in self.pending
        )

    def canvas_down(self, at: tuple[int, int]) -> bool:
        """The canvas received a button-down at canvas point `at`. Returns whether it was the script's own."""
        own = self.take_own(at)
        dot = C.dot_at(self.script, at[0], at[1])
        self.owner.emit("mu_rx", dot=dot, at=list(at), own=own)
        waiting = next((p for p in self.pending if not p["done"]), None)
        if own and waiting is not None:
            if dot == waiting["dot"] and W.distance(at, waiting["at"]) <= self.tolerance:
                self.resolve(waiting, True, "landed")
            elif dot is not None:
                self.resolve(waiting, False, "wrong_dot")
            else:
                self.resolve(waiting, False, "off_target")
        return own


class Sentinel:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        args.log.parent.mkdir(parents=True, exist_ok=True)
        self.log = args.log.open("a", buffering=1, encoding="utf-8")
        self.armed = False
        self.expected = ""
        self.previous = None
        self.done_commands = 0
        self.desktops = W.VirtualDesktops()
        title = "User activity" if args.user_activity or args.mouse_user else "Bench Sentinel"
        self.frame = wx.Frame(None, title=title, size=(600, 320))
        self.frame.SetName("BenchSentinel")
        if args.frame:
            x, y, w, h = map(int, args.frame.split(","))
            self.frame.SetPosition((x, y))
            self.frame.SetSize((w, h))
        panel = wx.Panel(self.frame)
        self.entry = None
        self.mouse = None
        sizer = wx.BoxSizer(wx.VERTICAL)
        if args.mouse_user:
            self.script = json.loads(args.mouse_user.read_text(encoding="utf-8"))
            self.canvas = self.make_canvas(panel)
            sizer.Add(self.canvas, 0, wx.ALL, CANVAS_MARGIN)
            self.mouse = MouseUser(self, self.script, self.canvas)
            panel.Bind(wx.EVT_LEFT_DOWN, self.panel_down)
        else:
            self.entry = wx.TextCtrl(panel, name="User activity text")
            sizer.Add(self.entry, 0, wx.EXPAND | wx.ALL, 16)
            self.entry.Bind(wx.EVT_TEXT, self.changed)
            self.entry.Bind(wx.EVT_KEY_DOWN, lambda e: (self.emit("keyDown"), e.Skip()))
            self.entry.Bind(wx.EVT_LEFT_DOWN, lambda e: (self.emit("mouseDown"), e.Skip()))
            self.entry.Bind(wx.EVT_MOUSEWHEEL, lambda e: (self.emit("scrollWheel"), e.Skip()))
            panel.Bind(wx.EVT_LEFT_DOWN, lambda e: (self.emit("mouseDown"), e.Skip()))
        panel.SetSizer(sizer)
        self.frame.Bind(wx.EVT_ACTIVATE, self.activated)
        if args.no_activate:
            self.frame.ShowWithoutActivating()
        else:
            self.frame.Show()
            self.activate()
        self.own = self.frame.GetHandle()
        self.timers = []
        self.every(100, self.control)
        self.every(200, self.sample)
        if args.user_activity:
            self.every(500, self.activity)
        extra = (
            {"canvas": list(self.canvas.GetSize()), "tagging": self.mouse.tagging}
            if self.mouse
            else {}
        )
        self.emit("ready", hwnd=self.own, **extra)

    def make_canvas(self, parent: wx.Window) -> wx.Window:
        """The user's drawing area: exactly script.canvas pixels, with the numbered dots."""
        width, height = self.script["canvas"]
        canvas = wx.Panel(parent, size=(width, height), name="User activity canvas")
        canvas.SetMinSize((width, height))
        canvas.SetBackgroundStyle(wx.BG_STYLE_PAINT)
        canvas.Bind(wx.EVT_PAINT, self.paint)
        canvas.Bind(wx.EVT_LEFT_DOWN, self.canvas_down)
        canvas.Bind(wx.EVT_KEY_DOWN, lambda e: (self.emit("keyDown"), e.Skip()))
        canvas.Bind(wx.EVT_MOUSEWHEEL, lambda e: (self.emit("scrollWheel"), e.Skip()))
        return canvas

    def paint(self, _event: wx.PaintEvent) -> None:
        dc = wx.PaintDC(self.canvas)
        dc.SetBackground(wx.WHITE_BRUSH)
        dc.Clear()
        radius = self.script["radius"]
        dc.SetPen(wx.Pen(wx.Colour(40, 90, 160), 2))
        dc.SetBrush(wx.Brush(wx.Colour(210, 225, 245)))
        for dot in self.script["dots"]:
            dc.DrawCircle(dot["x"], dot["y"], radius)
            label = str(dot["index"] + 1)
            w, h = dc.GetTextExtent(label)
            dc.DrawText(label, dot["x"] - w // 2, dot["y"] - h // 2)

    def canvas_down(self, event: wx.MouseEvent) -> None:
        event.Skip()
        pos = event.GetPosition()
        if not self.armed:
            return
        if not self.mouse.canvas_down((pos.x, pos.y)):
            self.emit(
                "mouseDown"
            )  # not the script's own: a leak, like any other click in the user window

    def panel_down(self, event: wx.MouseEvent) -> None:
        event.Skip()
        if not self.mouse.take_own(None):
            self.emit("mouseDown")

    def every(self, ms: int, fn) -> None:
        timer = wx.Timer(self.frame)
        self.frame.Bind(wx.EVT_TIMER, lambda _: fn(), timer)
        timer.Start(ms)
        self.timers.append(timer)

    def activated(self, event: wx.ActivateEvent) -> None:
        event.Skip()
        if event.GetActive():
            self.emit("didBecomeActive")
        else:
            self.emit("didResignKey")

    def changed(self, _event: object) -> None:
        if self.entry.GetValue() != self.expected:
            self.emit("user_text_disrupted")

    def snapshot(self) -> dict:
        front = W.identity(W.user32.GetForegroundWindow(), self.own, BID)
        above = [W.identity(w, self.own, BID)["bid"] for w in W.windows_above(self.own)]
        on = self.desktops.on_current(self.own)
        sample = {
            "front": front,
            "mouse": W.cursor(),
            "clip": W.clipboard_seq(),
            "sheet": W.owned_dialog_shown(self.own),
            "above": above,
            "above_n": len(above),
        }
        if on is not None:  # not measured is left out, which the summary reports as None
            sample["onspace"] = on
        return sample

    def emit(self, event: str, **details: object) -> None:
        if self.armed or event in ("armed", "disarmed", "ready"):
            self.log.write(json.dumps({"t": time.time() * 1000, "ev": event, **details}) + "\n")

    def activate(self) -> None:
        W.force_foreground(self.frame.GetHandle())
        (self.entry or self.canvas).SetFocus()

    def control(self) -> None:
        path = self.args.control
        if not path or not path.exists():
            return
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except OSError:
            return
        for command in lines[self.done_commands :]:
            self.done_commands += 1
            command = command.strip()
            if command == "activate":
                self.activate()
            elif command == "arm" and not self.armed:
                self.armed = True
                self.emit("armed", **self.snapshot())
                if self.mouse:
                    self.mouse.start()
            elif command == "disarm" and self.armed:
                if self.mouse and self.mouse.active:
                    self.mouse.request_stop()  # the script stops at once; the disarm follows its clicks in flight
                elif not (self.mouse and self.mouse.finishing):
                    self.finish_disarm()
            elif command == "quit":
                self.frame.Close()
                return

    def finish_disarm(self) -> None:
        if self.args.user_activity:
            self.emit("user_text", intact=self.entry.GetValue() == self.expected)
        self.emit("disarmed", **self.snapshot())
        self.armed = False

    def sample(self) -> None:
        if not self.armed:
            return
        sample = self.snapshot()
        if sample["front"] != self.previous:
            self.emit("front", **sample)
            if self.previous and self.previous.get("bid") == BID:
                self.emit("didResignActive")
        self.previous = sample["front"]
        self.emit("s", **sample)

    def activity(self) -> None:
        if not self.armed:
            return
        if W.identity(W.user32.GetForegroundWindow(), self.own, BID)["bid"] == BID:
            if self.entry.GetValue() != self.expected:
                self.emit("user_text_disrupted")
            self.expected += "a"
            self.entry.ChangeValue(self.expected)
            self.entry.SetInsertionPointEnd()
            self.emit("user_type")
        else:
            self.emit("user_blocked")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--control", type=Path, help="command file: activate, arm, disarm, quit")
    parser.add_argument("--no-activate", action="store_true")
    parser.add_argument("--user-activity", action="store_true")
    parser.add_argument(
        "--mouse-user",
        type=Path,
        help="UM-01: the user moves and clicks the pointer from this script",
    )
    parser.add_argument("--frame", help="x,y,width,height")
    args = parser.parse_args()
    if args.mouse_user and args.user_activity:
        parser.error("--mouse-user and --user-activity are exclusive")
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
    app = wx.App(False)
    app.SetAppName("BenchSentinel")
    witness = Sentinel(args)
    try:
        app.MainLoop()
    finally:
        if witness.mouse:
            witness.mouse.hook.close()
        witness.log.close()
        os._exit(0)


if __name__ == "__main__":
    main()
