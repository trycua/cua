# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Guest probe window for latency.bench.ts: 800x600 at (0,0), black or white,
# toggled by every button press. ANIM=1 also flips it every 33 ms (flat
# content); ANIM=scene draws a moving gradient scene at 30 fps instead
# (natural content: every pixel changes, like video or scrolling).
import os

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

import math
import time

state = [0]
MODE = os.environ.get("ANIM", "")
win = Gtk.Window(title="cua-bench-probe")
win.set_decorated(False)
win.set_default_size(800, 600)
win.move(0, 0)
area = Gtk.DrawingArea()


def draw(_area, cr):
    if MODE == "scene":
        t = time.monotonic()
        import cairo  # noqa: PLC0415

        for i in range(8):
            x = 400 + 300 * math.cos(t * (0.7 + i * 0.13) + i)
            y = 300 + 220 * math.sin(t * (0.9 + i * 0.11) + i * 2)
            g = cairo.RadialGradient(x, y, 10, x, y, 260)
            g.add_color_stop_rgba(0, (i * 37 % 255) / 255, (i * 91 % 255) / 255, (i * 53 % 255) / 255, 1)
            g.add_color_stop_rgba(1, 0, 0, 0, 0.15)
            cr.set_source(g)
            cr.paint()
        return
    v = float(state[0])
    cr.set_source_rgb(v, v, v)
    cr.paint()


def toggle(*_args):
    state[0] ^= 1
    area.queue_draw()
    return True


area.connect("draw", draw)
win.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
win.connect("button-press-event", toggle)
if MODE == "scene":
    GLib.timeout_add(33, lambda: (area.queue_draw(), True)[1])
elif MODE:
    GLib.timeout_add(33, toggle)
win.add(area)
win.show_all()
win.set_keep_above(True)
win.connect("destroy", Gtk.main_quit)
Gtk.main()
