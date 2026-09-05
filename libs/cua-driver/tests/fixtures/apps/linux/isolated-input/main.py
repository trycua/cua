#!/usr/bin/env python3
"""Native GTK3 raw-event fixture; no accessibility action emulates input.

Run each actor as a separate process. WAYLAND_DEBUG=1 supplies an independent
wire-level primary-pointer oracle in addition to this application journal.
"""
import argparse
import json
import time
import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib

parser = argparse.ArgumentParser()
parser.add_argument("--actor", choices=["Background", "Foreground"], required=True)
parser.add_argument("--journal", required=True)
args = parser.parse_args()
journal = open(args.journal, "x", buffering=1)
state = {"clicks": 0, "keys": "", "scroll": 0, "motion": 0, "held": False}


def record(kind, **values):
    journal.write(json.dumps({"kind": kind, "time": time.monotonic_ns(), **values}) + "\n")


window = Gtk.Window(title="Cua Isolated Input " + args.actor)
window.set_default_size(600, 500)
canvas = Gtk.DrawingArea()
canvas.set_can_focus(True)
canvas.add_events(Gdk.EventMask.ALL_EVENTS_MASK)
window.add(canvas)


def draw(widget, cr):
    cr.set_source_rgb(0.08, 0.13, 0.18)
    cr.paint()
    cr.set_source_rgb(0.75, 0.9, 0.75)
    cr.select_font_face("sans-serif", 0, 0)
    cr.set_font_size(28)
    lines = [args.actor + " — raw Wayland events", "clicks=" + str(state["clicks"]),
             "keys=" + state["keys"][-25:], "scroll=" + str(state["scroll"]),
             "motion=" + str(state["motion"]), "held=" + str(state["held"])]
    for index, text in enumerate(lines):
        cr.move_to(25, 55 + index * 48)
        cr.show_text(text)
    return False


def event(widget, e):
    kind = e.type.value_nick
    data = {}
    if e.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE,
                   Gdk.EventType.MOTION_NOTIFY, Gdk.EventType.SCROLL):
        data.update(x=e.x, y=e.y)
    if e.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE):
        state["held"] = e.type == Gdk.EventType.BUTTON_PRESS
        if not state["held"]:
            state["clicks"] += 1
        _ok, button_number = e.get_button()
        data["button"] = int(button_number)
        canvas.grab_focus()  # Widget-local focus, not a compositor activation.
    elif e.type == Gdk.EventType.MOTION_NOTIFY:
        state["motion"] += 1
    elif e.type == Gdk.EventType.KEY_PRESS:
        name = Gdk.keyval_name(e.keyval)
        state["keys"] += (name if len(name) == 1 else "[" + name + "]")
        data.update(key=name, modifiers=int(e.state))
    elif e.type == Gdk.EventType.SCROLL:
        state["scroll"] += 1
    if e.type not in (Gdk.EventType.EXPOSE, Gdk.EventType.CONFIGURE):
        record(kind, **data)
    canvas.queue_draw()
    return False


canvas.connect("draw", draw)
canvas.connect("event", event)
window.connect("destroy", Gtk.main_quit)
window.show_all()
canvas.grab_focus()
record("ready", name=args.actor)


def heartbeat():
    record("state", **state)
    return True


GLib.timeout_add(250, heartbeat)
Gtk.main()
journal.close()
