from __future__ import annotations

import argparse
import ctypes
import json
import os
import re
import signal
import subprocess
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gdk, GLib, Gtk


def xprop(*args: str) -> dict[str, str]:
    done = subprocess.run(["xprop", *args], capture_output=True, text=True, timeout=2)
    return {line.split("(", 1)[0]: line.split(" = ", 1)[-1] for line in done.stdout.splitlines()}


def xid(value: str) -> int:
    match = re.search(r"0x[0-9a-fA-F]+", value)
    return int(match[0], 16) if match else 0


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
        self.window = Gtk.Window(title="BenchSentinel — User window")
        self.window.set_wmclass("BenchSentinel", "BenchSentinel")
        self.window.set_default_size(600, 320)
        if args.frame:
            x, y, w, h = map(int, args.frame.split(","))
            self.window.move(x, y)
            self.window.resize(w, h)
        self.entry = Gtk.Entry()
        self.entry.get_accessible().set_name("User activity text")
        self.entry.connect("changed", self.changed)
        self.window.add(self.entry)
        self.window.connect("destroy", Gtk.main_quit)
        self.window.connect("focus-out-event", lambda *_: self.emit("didResignKey"))
        self.window.connect("focus-in-event", lambda *_: self.emit("didBecomeActive"))
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
        self.emit("ready")

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
        return {"bid": names[-1] if names else f"x11:{window}", "pid": int(pid) if pid.isdigit() else -1}

    def snapshot(self) -> dict:
        root = xprop("-root", "_NET_ACTIVE_WINDOW", "_NET_CLIENT_LIST_STACKING", "_NET_CURRENT_DESKTOP")
        front = self.identity(xid(root.get("_NET_ACTIVE_WINDOW", "")))
        pointer = Gdk.Display.get_default().get_default_seat().get_pointer().get_position()
        own = xprop("-id", hex(self.own), "_NET_WM_DESKTOP")
        desktop = own.get("_NET_WM_DESKTOP", "")
        stack = [int(v, 16) for v in re.findall(r"0x[0-9a-fA-F]+", root.get("_NET_CLIENT_LIST_STACKING", ""))]
        above = [self.identity(w)["bid"] for w in stack[stack.index(self.own) + 1:]] if self.own in stack else []
        sheet = any(xid(xprop("-id", hex(w), "WM_TRANSIENT_FOR").get("WM_TRANSIENT_FOR", "")) == self.own for w in stack if w != self.own)
        return {"front": front, "mouse": [pointer.x, pointer.y], "clip": self.clip,
                "onspace": desktop in ("4294967295", root.get("_NET_CURRENT_DESKTOP")),
                "sheet": sheet, "above": above, "above_n": len(above)}

    def emit(self, event: str, **details: object) -> bool:
        if self.armed or event in ("armed", "disarmed", "ready"):
            self.log.write(json.dumps({"t": time.time() * 1000, "ev": event, **details}) + "\n")
        return False

    def activate(self) -> bool:
        self.window.present()
        self.entry.grab_focus()
        return True

    def toggle(self) -> bool:
        if self.armed:
            if self.args.user_activity:
                self.emit("user_text", intact=self.entry.get_text() == self.expected)
            self.emit("disarmed", **self.snapshot())
            self.armed = False
        else:
            self.armed = True
            self.emit("armed", **self.snapshot())
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
    witness = Sentinel(parser.parse_args())
    try:
        Gtk.main()
    finally:
        witness.listener.deregister("object:state-changed:focused")
        witness.log.close()


if __name__ == "__main__":
    main()
