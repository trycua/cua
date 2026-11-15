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
import winenv as W  # noqa: E402

BID = "ai.cua.benchsentinel"


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
        title = "User activity" if args.user_activity else "Bench Sentinel"
        self.frame = wx.Frame(None, title=title, size=(600, 320))
        self.frame.SetName("BenchSentinel")
        if args.frame:
            x, y, w, h = map(int, args.frame.split(","))
            self.frame.SetPosition((x, y))
            self.frame.SetSize((w, h))
        panel = wx.Panel(self.frame)
        self.entry = wx.TextCtrl(panel, name="User activity text")
        sizer = wx.BoxSizer(wx.VERTICAL)
        sizer.Add(self.entry, 0, wx.EXPAND | wx.ALL, 16)
        panel.SetSizer(sizer)
        self.entry.Bind(wx.EVT_TEXT, self.changed)
        self.entry.Bind(wx.EVT_KEY_DOWN, lambda e: (self.emit("keyDown"), e.Skip()))
        self.entry.Bind(wx.EVT_LEFT_DOWN, lambda e: (self.emit("mouseDown"), e.Skip()))
        self.entry.Bind(wx.EVT_MOUSEWHEEL, lambda e: (self.emit("scrollWheel"), e.Skip()))
        panel.Bind(wx.EVT_LEFT_DOWN, lambda e: (self.emit("mouseDown"), e.Skip()))
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
        self.emit("ready", hwnd=self.own)

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
        self.entry.SetFocus()

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
            elif command == "disarm" and self.armed:
                if self.args.user_activity:
                    self.emit("user_text", intact=self.entry.GetValue() == self.expected)
                self.emit("disarmed", **self.snapshot())
                self.armed = False
            elif command == "quit":
                self.frame.Close()
                return

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
    parser.add_argument("--frame", help="x,y,width,height")
    args = parser.parse_args()
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
    app = wx.App(False)
    app.SetAppName("BenchSentinel")
    witness = Sentinel(args)
    try:
        app.MainLoop()
    finally:
        witness.log.close()
        os._exit(0)


if __name__ == "__main__":
    main()
