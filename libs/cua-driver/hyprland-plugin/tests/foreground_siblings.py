#!/usr/bin/env python3
"""Supplemental foreground safety fixture: two windows, one Wayland client.

Each drawing area records its own raw events and live counters. Focus changes
are performed independently by the Rust test through compositor IPC.
"""

import argparse
import json
import os
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("GdkWayland", "3.0")
from gi.repository import Gdk, GdkWayland, GLib, Gtk


class Actor:
    def __init__(self, name, directory):
        self.name = name
        self.journal = (directory / (name + ".jsonl")).open("x", buffering=1)
        self.state = {"clicks": 0, "keys": "", "held": False, "motion": 0}
        self.window = Gtk.Window(title="Cua Foreground " + name)
        self.window.set_default_size(600, 500)
        self.canvas = Gtk.DrawingArea()
        self.canvas.set_can_focus(True)
        self.canvas.add_events(Gdk.EventMask.ALL_EVENTS_MASK)
        self.window.add(self.canvas)
        self.canvas.connect("draw", self.draw)
        self.canvas.connect("event", self.event)
        self.window.connect("destroy", Gtk.main_quit)
        self.window.show_all()
        self.canvas.grab_focus()
        GLib.timeout_add(100, self.heartbeat)

    def record(self, kind, **values):
        self.journal.write(json.dumps({"kind": kind, "time": time.monotonic_ns(),
                                       "actor": self.name, **values}) + "\n")

    def draw(self, widget, cr):
        cr.set_source_rgb(0.08, 0.13, 0.18)
        cr.paint()
        cr.set_source_rgb(0.75, 0.9, 0.75)
        cr.select_font_face("sans-serif", 0, 0)
        cr.set_font_size(26)
        for index, line in enumerate([self.name, *[
                str(key) + "=" + str(value) for key, value in self.state.items()]]):
            cr.move_to(25, 55 + index * 48)
            cr.show_text(line[-60:])
        return False

    def event(self, widget, event):
        data = {}
        if event.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE,
                          Gdk.EventType.MOTION_NOTIFY, Gdk.EventType.SCROLL):
            data.update(x=event.x, y=event.y)
        if event.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE):
            self.state["held"] = event.type == Gdk.EventType.BUTTON_PRESS
            if not self.state["held"]:
                self.state["clicks"] += 1
            _, button = event.get_button()
            data["button"] = int(button)
            self.canvas.grab_focus()
        elif event.type == Gdk.EventType.MOTION_NOTIFY:
            self.state["motion"] += 1
        elif event.type in (Gdk.EventType.KEY_PRESS, Gdk.EventType.KEY_RELEASE):
            key = Gdk.keyval_name(event.keyval) or "unknown"
            data.update(key=key, modifiers=int(event.state))
            if event.type == Gdk.EventType.KEY_PRESS:
                self.state["keys"] += key
        if event.type not in (Gdk.EventType.EXPOSE, Gdk.EventType.CONFIGURE):
            self.record(event.type.value_nick, **data)
        self.canvas.queue_draw()
        return False

    def heartbeat(self):
        self.record("state", **self.state)
        return True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--journal-dir", type=Path, required=True)
    args = parser.parse_args()
    display = Gdk.Display.get_default()
    assert isinstance(display, GdkWayland.WaylandDisplay), "native Wayland required"
    actors = [Actor(name, args.journal_dir) for name in ("Target", "Sibling")]
    for actor in actors:
        assert actor.window.get_display() == display
        actor.record("ready", pid=os.getpid(), same_display=True, native_wayland=True)
    Gtk.main()
    for actor in actors:
        actor.journal.close()


if __name__ == "__main__":
    main()
