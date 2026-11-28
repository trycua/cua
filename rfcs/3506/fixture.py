#!/usr/bin/python
"""RFC-only two-window raw-event oracle, adapted from isolated-input/main.py.

Two independent native GTK3 toplevels share one PID. Never synthesizes input.
"""
import argparse
import json
import os
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk
import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop

parser = argparse.ArgumentParser()
parser.add_argument("--directory", type=Path, required=True)
args = parser.parse_args()
args.directory.mkdir(parents=True, exist_ok=True)
journal = (args.directory / "fixture.jsonl").open("x", buffering=1)
DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
windows = {}
states = {}


def record(label, kind, **values):
    journal.write(json.dumps({"time_ns": time.monotonic_ns(), "window": label,
                              "kind": kind, **values}) + "\n")


def create(label):
    state = {"presses": 0, "releases": 0, "clicks": 0, "button_releases": 0,
             "keys": "", "held_keys": [], "generation": time.monotonic_ns()}
    states[label] = state
    window = Gtk.Window(title="Cua 3506 Synthetic " + label)
    window.set_default_size(560, 400)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=18)
    box.set_border_width(28)
    display = Gtk.Label()
    display.set_xalign(0)
    entry = Gtk.Entry()
    entry.set_placeholder_text("Synthetic accessible field — no private data")
    entry.set_name("cua-3506-field-" + label)
    button = Gtk.Button(label="Synthetic AX action counter")
    state["ax_clicks"] = 0
    box.pack_start(display, True, True, 0)
    box.pack_start(entry, False, False, 0)
    box.pack_start(button, False, False, 0)
    window.add(box)

    def refresh():
        display.set_markup("<span size='22000'>" + label + " — native Wayland\n" +
                           "PID=" + str(os.getpid()) + "\n" +
                           "key presses=" + str(state["presses"]) + " releases=" +
                           str(state["releases"]) + "\nclicks=" + str(state["clicks"]) +
                           " releases=" + str(state["button_releases"]) +
                           "\nAX actions=" + str(state["ax_clicks"]) + "</span>")

    def event(_widget, e):
        values = {}
        if e.type in (Gdk.EventType.KEY_PRESS, Gdk.EventType.KEY_RELEASE):
            key = Gdk.keyval_name(e.keyval)
            pressed = e.type == Gdk.EventType.KEY_PRESS
            state["presses" if pressed else "releases"] += 1
            if pressed:
                state["keys"] += key + " "
                state["held_keys"].append(key)
            elif key in state["held_keys"]:
                state["held_keys"].remove(key)
            values = {"key": key, "hardware_keycode": e.hardware_keycode,
                      "modifiers": int(e.state)}
        elif e.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE):
            state["clicks" if e.type == Gdk.EventType.BUTTON_PRESS else "button_releases"] += 1
            values = {"x": e.x, "y": e.y, "button": e.button}
        else:
            return False
        record(label, e.type.value_nick, **values)
        refresh()
        return False

    def ax_action(_button):
        state["ax_clicks"] += 1
        record(label, "button-action", count=state["ax_clicks"])
        refresh()

    def closed(_window):
        record(label, "destroyed", generation=state["generation"])
        windows.pop(label, None)

    window.add_events(Gdk.EventMask.ALL_EVENTS_MASK)
    window.connect("event", event)
    window.connect("destroy", closed)
    button.connect("clicked", ax_action)
    windows[label] = window
    refresh()
    window.show_all()
    entry.grab_focus()  # Widget-local, not compositor activation.
    record(label, "ready", pid=os.getpid(), generation=state["generation"])


class Fixture(dbus.service.Object):
    @dbus.service.method("org.cua.ProofFixture", in_signature="", out_signature="s")
    def GetState(self):
        return json.dumps({"pid": os.getpid(), "time_ns": time.monotonic_ns(),
                           "windows": {k: v for k, v in states.items() if k in windows}})

    @dbus.service.method("org.cua.ProofFixture", in_signature="s", out_signature="")
    def Close(self, label):
        windows[str(label)].destroy()

    @dbus.service.method("org.cua.ProofFixture", in_signature="s", out_signature="")
    def Recreate(self, label):
        if label not in ("A", "B") or label in windows:
            raise ValueError("invalid fixture label or already live")
        create(str(label))

    @dbus.service.method("org.cua.ProofFixture", in_signature="", out_signature="")
    def Quit(self):
        GLib.idle_add(Gtk.main_quit)


service = Fixture(bus, "/Fixture")
create("A")
create("B")
(args.directory / "fixture-owner.json").write_text(json.dumps({
    "pid": os.getpid(), "owner": bus.get_unique_name(), "path": "/Fixture"}))
Gtk.main()
for window in list(windows.values()):
    window.destroy()
journal.close()
