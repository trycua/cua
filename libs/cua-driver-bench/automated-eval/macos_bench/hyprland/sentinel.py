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

With --mouse-user (UM-01) the window is the canvas of a simulated user who moves the real pointer through the
compositor (`hypr.move_cursor`) and clicks with wlrctl or ydotool, from a script (SHARED-DESKTOP.md). Nothing can
tag that input, so the trajectory check is the measure.

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
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
import benchlab_common as C  # noqa: E402
import hypr  # noqa: E402

OWN_BID = "ai.cua.benchsentinel"
DRAWS_OVER = ("floating", "pinned")


def comm(pid: int) -> str:
    try:
        return Path(f"/proc/{pid}/comm").read_text().strip()
    except OSError:
        return ""


class MouseUser:
    """UM-01's simulated user (SHARED-DESKTOP.md, "Mouse user script"), a 25 ms GLib timeout state machine on the
    GTK main loop, so scripted input, checks and the canvas's button-down handler never run concurrently. The
    pointer moves through Hyprland IPC, the click is one `wlrctl`/`ydotool` command (press and release together,
    started without waiting for it), and canvas coordinates map to the layout through the window's position."""

    TICK_MS = 25
    ORIGIN_MAX_AGE = 0.25

    def __init__(self, sentinel: "Sentinel", script: dict) -> None:
        self.s = sentinel
        self.script = script
        self.running = False
        self.stopping = False
        self.offset: tuple[int, int] | None = None
        self.origin_at: tuple[float, float] | None = None
        self.origin_read = 0.0
        GLib.timeout_add(self.TICK_MS, self.tick)

    # --- canvas geometry
    def origin(self) -> tuple[float, float] | None:
        """Layout position of the canvas's top-left: the window's position plus the canvas's offset in it."""
        now = time.monotonic()
        if self.origin_at is not None and now - self.origin_read < self.ORIGIN_MAX_AGE:
            return self.origin_at
        if not self.s.address or not self.s.canvas.get_mapped():
            return None
        moved = self.s.canvas.translate_coordinates(self.s.window, 0, 0)
        client = next((c for c in hypr.clients() if c.get("address") == self.s.address), None)
        if moved is None or client is None:
            return None
        self.origin_at = (client["at"][0] + moved[0], client["at"][1] + moved[1])
        self.origin_read = now
        return self.origin_at

    def to_layout(self, p: tuple[float, float]) -> tuple[int, int] | None:
        o = self.origin()
        return None if o is None else (round(p[0] + o[0]), round(p[1] + o[1]))

    # --- lifecycle
    def start(self) -> None:
        self.s.emit(
            "mu_start",
            tagging=False,
            tolerance_px=self.script.get("tolerance_px", 3),
            platform="linux-hyprland",
        )
        self.click_argv = hypr.click_command()
        if self.click_argv is None:
            self.s.emit("mu_error", why="no click tool (wlrctl or ydotool)")
            return
        self.origin_at = None
        self.cmd: list[tuple[float, float]] = []  # commanded canvas points, newest last
        self.cur = tuple(self.script["home"])
        self.leg = 0
        self.points: list[tuple[int, int]] = []
        self.k = 0
        self.checks = 0
        self.clicks = 0
        self.pending: dict | None = None
        self.state = "home"
        self.next_at = 0.0
        self.running = True

    def stop(self) -> None:
        if not self.running:
            return
        self.stopping = True
        if self.state == "release":  # stopped between the click and its release time: dropped
            self.pending = None
        if self.pending and self.pending["logged"]:
            end = time.monotonic() + 0.06
            while time.monotonic() < end and not self.pending.get("rx"):
                Gtk.main_iteration_do(False)
            self.resolve(force=True)
        self.running = False
        self.stopping = False
        self.pending = None
        self.s.emit("mu_stop", clicks=self.clicks, checks=self.checks)

    def fail(self, why: str) -> None:
        self.s.emit("mu_error", why=why)
        self.running = False

    # --- one check: the real pointer against the script
    def check(self) -> None:
        o = self.origin()
        if o is None or not self.cmd:
            return
        px, py = hypr.cursor()
        got = (px - o[0], py - o[1])
        dists = [((got[0] - c[0]) ** 2 + (got[1] - c[1]) ** 2) ** 0.5 for c in self.cmd[-2:]]
        self.s.emit(
            "mu", i=self.checks, want=list(self.cmd[-1]),
            got=[round(got[0], 1), round(got[1], 1)], d=round(min(dists), 2),
        )  # fmt: skip
        self.checks += 1

    def post_move(self, p: tuple[float, float]) -> None:
        layout = self.to_layout(p)
        if layout is not None:
            hypr.move_cursor(*layout)
        self.cmd.append(p)

    def wait(self, ms: float, now: float) -> None:
        self.next_at = max(self.next_at + ms / 1000.0, now) if self.next_at else now + ms / 1000.0

    # --- the button-down handler of the canvas
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
            own = ((at[0] - p["at"][0]) ** 2 + (at[1] - p["at"][1]) ** 2) ** 0.5 <= tol
            p["rx"] = {"dot": dot, "at": at, "own": own}
        self.s.emit("mu_rx", dot=dot, at=[round(at[0], 1), round(at[1], 1)], own=own)
        if not own:  # a button-down that is not ours is a leak, as on any other window
            self.s.emit("mouseDown")

    def resolve(self, force: bool = False) -> None:
        p = self.pending
        if not p or not p.get("logged"):
            return
        rx = p.get("rx")
        late = time.monotonic() - p["t"] > self.script["click_timeout_ms"] / 1000
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
        self.pending = None

    # --- the state machine
    def tick(self) -> bool:
        if not self.running or self.stopping:
            return True
        now = time.monotonic()
        if self.pending:
            self.resolve()
        if now < self.next_at - 0.002:
            return True
        script = self.script
        try:
            return self.step(now, script)
        except (OSError, ValueError, RuntimeError) as error:  # the compositor did not answer
            self.fail(f"compositor request failed: {error}")
            return True

    def step(self, now: float, script: dict) -> bool:
        if self.state == "home":
            home = tuple(script["home"])
            layout = self.to_layout(home)
            if layout is None or not hypr.move_cursor(*layout):
                self.fail("the pointer did not reach home (no cursor-move dispatcher worked)")
                return True
            self.cmd.append(home)
            self.state = "plan"
            self.next_at = now
        elif self.state == "plan":
            dot = script["dots"][script["sequence"][self.leg % len(script["sequence"])]]
            self.target = (dot["x"], dot["y"])
            self.points = C.mouse_leg(self.cur, self.target, script["step_px"])
            self.k = 0
            self.state = "move"
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
            layout = self.to_layout(self.target)
            if layout is not None:
                hypr.move_cursor(*layout)  # the click is at the dot's centre
            self.clicks += 1
            self.pending = {
                "n": self.clicks, "dot": script["sequence"][self.leg % len(script["sequence"])],
                "at": self.target, "t": now, "rx": None, "logged": False,
            }  # fmt: skip
            subprocess.Popen(
                self.click_argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True,
            )  # fmt: skip
            self.state = "release"
            self.wait(script["press_ms"], now)
        elif self.state == "release":
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
        self.address = ""
        self.control_at = 0
        self.lock = threading.Lock()
        self.latest_front: dict = {}
        self.mouse = None
        self.title = "User activity" if args.user_activity or args.mouse_user else "Bench Sentinel"
        self.window = Gtk.Window(title=self.title)
        self.window.set_wmclass("BenchSentinel", "BenchSentinel")
        self.window.set_default_size(600, 320)
        if args.frame:
            _, _, w, h = map(int, args.frame.split(","))
            self.window.set_default_size(w, h)
        self.entry = Gtk.Entry()
        self.entry.get_accessible().set_name("User activity text")
        self.entry.connect("changed", self.changed)
        self.window.connect("destroy", Gtk.main_quit)
        self.window.connect("focus-out-event", lambda *_: self.emit("didResignKey"))
        self.window.connect("focus-in-event", lambda *_: self.emit("didBecomeActive"))
        if args.mouse_user:
            # UM-01: the window's content is the canvas the simulated user clicks on (no typing user). It has
            # no title bar, so the canvas sits at the window's top-left.
            self.script = json.loads(args.mouse_user.read_text(encoding="utf-8"))
            self.window.set_decorated(False)
            self.canvas = Gtk.DrawingArea()
            self.canvas.set_size_request(*self.script["canvas"])
            self.canvas.set_halign(Gtk.Align.START)
            self.canvas.set_valign(Gtk.Align.START)
            self.canvas.set_can_focus(True)
            self.canvas.add_events(
                Gdk.EventMask.BUTTON_PRESS_MASK
                | Gdk.EventMask.KEY_PRESS_MASK
                | Gdk.EventMask.SCROLL_MASK
            )
            self.canvas.connect("draw", self.draw_canvas)
            self.canvas.get_accessible().set_name("User activity canvas")
            self.canvas.connect("button-press-event", self.canvas_press)
            self.canvas.connect("key-press-event", lambda *_: self.emit("keyDown"))
            self.canvas.connect("scroll-event", lambda *_: self.emit("scrollWheel"))
            self.window.add(self.canvas)
            self.focus_widget = self.canvas
            self.mouse = MouseUser(self, self.script)
        else:
            self.window.add(self.entry)
            self.focus_widget = self.entry
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
        if event.type == Gdk.EventType.BUTTON_PRESS and self.armed:
            self.mouse.rx(event)
        return True

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
        self.focus_widget.grab_focus()
        return True

    def toggle(self, *_: object) -> bool:
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
    parser.add_argument("--mouse-user", type=Path, help="UM-01: the mouse-user script (JSON)")
    witness = Sentinel(parser.parse_args())
    try:
        Gtk.main()
    finally:
        if witness.watcher:
            witness.watcher.terminate()
        witness.log.close()


if __name__ == "__main__":
    main()
