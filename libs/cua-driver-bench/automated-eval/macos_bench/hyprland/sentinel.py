# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""BenchSentinel for Hyprland (Wayland): the witness and simulated user of the Linux/Omarchy BenchLab port.

A GTK3 window ("User activity" in the background condition) that writes the same JSONL schema as the macOS
BenchSentinel, read by swift/summarize_sentinel.py. A Wayland client cannot see other windows, so the compositor
state comes from Hyprland's IPC socket (hypr.py):

* front: the active window (`activewindow`); the sentinel's own window is reported as ai.cua.benchsentinel.
* above: windows after the sentinel's in Hyprland's stacking order (`clients`) on its workspace (or pinned) that
  overlap it and draw over it (floating, pinned or full screen), plus top and overlay layer-shell surfaces that
  overlap it. A layer surface owned by a cua-driver process is named after it, so the overlay rule applies.
* mouse: the real pointer (`cursorpos`); the agent seats of the cua Hyprland plugin have their own pointers.
* onspace: the sentinel's workspace is the active one, no special workspace is open over it, and no other window
  is full screen on it.
* sheet: another window of the sentinel's own process (a dialog on the user's app).
* clip: clipboard changes, from `wl-paste --watch` (data-control; GTK's owner-change without wl-clipboard).
* key focus, typed keys, clicks and scrolls: GTK events on the sentinel's own window.

Control: one command per line appended to --control (activate, arm, disarm, quit); SIGUSR1 activates and SIGUSR2
toggles the armed state, as on X11.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hypr  # noqa: E402

OWN_BID = "ai.cua.benchsentinel"
DRAWS_OVER = ("floating", "pinned")


def comm(pid: int) -> str:
    try:
        return Path(f"/proc/{pid}/comm").read_text().strip()
    except OSError:
        return ""


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
        self.address = ""
        self.control_at = 0
        self.lock = threading.Lock()
        self.latest_front: dict = {}
        self.title = "User activity" if args.user_activity else "Bench Sentinel"
        self.window = Gtk.Window(title=self.title)
        self.window.set_wmclass("BenchSentinel", "BenchSentinel")
        self.window.set_default_size(600, 320)
        if args.frame:
            _, _, w, h = map(int, args.frame.split(","))
            self.window.set_default_size(w, h)
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
        self.watcher = None
        if not shutil.which("wl-paste"):
            # GTK only hears of selection changes while the window has keyboard focus.
            self.clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
            self.clipboard.connect("owner-change", self.clip_changed)
        else:
            # data-control sees every selection change, focused or not (and the current one when it starts,
            # long before the sentinel is armed; changes are counted from the armed sample).
            self.watcher = subprocess.Popen(
                ["wl-paste", "--watch", "echo", "change"], stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, text=True,
            )  # fmt: skip
            threading.Thread(target=self.watch_clipboard, daemon=True).start()
        self.window.show_all()
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, self.activate)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR2, self.toggle)
        GLib.timeout_add(100, self.find_window)

    def find_window(self) -> bool:
        mine = [c for c in hypr.by_pid(os.getpid()) if c.get("title") == self.title]
        if not mine:
            return True
        self.address = mine[0]["address"]
        if self.args.frame:
            hypr.place(self.address, tuple(map(int, self.args.frame.split(","))))
        if not self.args.no_activate:
            self.activate()
        # Compositor reads can stall while the compositor is busy; they run off the GTK thread so the user's
        # typing, focus events and control commands never wait on them.
        threading.Thread(target=self.sampler, daemon=True).start()
        GLib.timeout_add(100, self.poll_control)
        if self.args.user_activity:
            GLib.timeout_add(500, self.activity)
        self.emit("ready", address=self.address, pid=os.getpid())
        return False

    def watch_clipboard(self) -> None:
        for _ in self.watcher.stdout:
            GLib.idle_add(self.clip_changed)

    def clip_changed(self, *_: object) -> bool:
        self.clip += 1
        return False

    def changed(self, *_: object) -> None:
        if self.entry.get_text() != self.expected:
            self.emit("user_text_disrupted")

    def own(self, clients: list[dict]) -> dict:
        return next((c for c in clients if c.get("address") == self.address), {})

    def identity(self, client: dict) -> dict:
        if not client:
            return {"bid": "none", "pid": -1}
        if client.get("address") == self.address:
            return {"bid": OWN_BID, "pid": os.getpid()}
        return {"bid": hypr.identity(client), "pid": int(client.get("pid", -1))}

    def snapshot(self) -> dict:
        clients = hypr.query("clients") or []
        own = self.own(clients)
        workspace = (own.get("workspace") or {}).get("id")
        front = self.identity(hypr.active_window())
        x, y = hypr.cursor()
        above: list[str] = []
        if own in clients:
            for c in clients[clients.index(own) + 1 :]:
                if not c.get("mapped", True) or c.get("hidden"):
                    continue
                here = c.get("pinned") or (c.get("workspace") or {}).get("id") == workspace
                over = c.get("floating") or c.get("pinned") or c.get("fullscreen")
                if here and over and hypr.overlaps(c, own):
                    above.append(self.identity(c)["bid"])
        for c in clients:
            if (
                c is not own
                and c.get("fullscreen")
                and (c.get("workspace") or {}).get("id") == workspace
            ):
                name = self.identity(c)["bid"]
                if name not in above:
                    above.append(name)
        layers = hypr.query("layers") or {}
        for monitor in layers.values():
            for level in ("2", "3"):
                for layer in (monitor.get("levels") or {}).get(level, []):
                    box = {
                        "at": (layer.get("x", 0), layer.get("y", 0)),
                        "size": (layer.get("w", 0), layer.get("h", 0)),
                    }
                    if own and hypr.overlaps(box, own):
                        pid = int(layer.get("pid", -1))
                        owner = comm(pid) or f"pid{pid}"
                        above.append(f"{owner}:layer:{layer.get('namespace', '')}")
        monitors = hypr.query("monitors") or []
        monitor = next(
            (m for m in monitors if m.get("id") == own.get("monitor")),
            monitors[0] if monitors else {},
        )
        special = bool((monitor.get("specialWorkspace") or {}).get("id"))
        fullscreen_other = any(
            c is not own
            and c.get("fullscreen")
            and (c.get("workspace") or {}).get("id") == workspace
            for c in clients
        )
        onspace = (
            (monitor.get("activeWorkspace") or {}).get("id") == workspace
            and not special
            and not fullscreen_other
        )
        sheet = any(
            c.get("pid") == os.getpid()
            and c.get("address") != self.address
            and c.get("mapped", True)
            for c in clients
        )
        return {
            "front": front,
            "mouse": [x, y],
            "clip": self.clip,
            "onspace": bool(onspace),
            "sheet": sheet,
            "above": above,
            "above_n": len(above),
        }

    def emit(self, event: str, **details: object) -> bool:
        if self.armed or event in ("armed", "disarmed", "ready"):
            with self.lock:
                self.log.write(json.dumps({"t": time.time() * 1000, "ev": event, **details}) + "\n")
        return False

    def activate(self, *_: object) -> bool:
        if self.address:
            hypr.focus(self.address)
            hypr.raise_to_top(self.address)
        self.window.present()
        self.entry.grab_focus()
        return True

    def toggle(self, *_: object) -> bool:
        if self.armed:
            if self.args.user_activity:
                self.emit("user_text", intact=self.entry.get_text() == self.expected)
            self.emit("disarmed", **self.snapshot())
            self.armed = False
        else:
            self.armed = True
            self.emit("armed", **self.snapshot())
        return True

    def poll_control(self) -> bool:
        if not self.args.control or not self.args.control.exists():
            return True
        with self.args.control.open(encoding="utf-8") as handle:
            handle.seek(self.control_at)
            lines = handle.read().splitlines()
            self.control_at = handle.tell()
        for line in lines:
            command = line.strip()
            if command == "activate":
                self.activate()
            elif command == "arm" and not self.armed:
                self.toggle()
            elif command == "disarm" and self.armed:
                self.toggle()
            elif command == "quit":
                Gtk.main_quit()
                return False
        return True

    def sampler(self) -> None:
        while True:
            started = time.monotonic()
            self.sample()
            time.sleep(max(0.0, 0.2 - (time.monotonic() - started)))

    def sample(self) -> None:
        try:
            sample = self.snapshot()
        except (OSError, ValueError, RuntimeError) as error:
            self.emit("sample_error", error=str(error))
            return
        self.latest_front = sample["front"]
        if not self.armed:
            return
        if sample["front"] != self.previous:
            self.emit("front", **sample)
            if self.previous and self.previous.get("bid") == OWN_BID:
                self.emit("didResignActive")
        self.previous = sample["front"]
        self.emit("s", **sample)

    def activity(self) -> bool:
        if self.armed:
            if self.latest_front.get("bid") == OWN_BID and self.window.has_toplevel_focus():
                if self.entry.get_text() != self.expected:
                    self.emit("user_text_disrupted")
                self.expected += "a"
                self.entry.set_text(self.expected)
                self.entry.set_position(-1)
                self.emit("user_type")
            else:
                self.emit("user_blocked")
        return True


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--control", type=Path)
    parser.add_argument("--no-activate", action="store_true")
    parser.add_argument("--user-activity", action="store_true")
    parser.add_argument("--frame")
    witness = Sentinel(parser.parse_args())
    try:
        Gtk.main()
    finally:
        if witness.watcher:
            witness.watcher.terminate()
        witness.log.close()


if __name__ == "__main__":
    main()
