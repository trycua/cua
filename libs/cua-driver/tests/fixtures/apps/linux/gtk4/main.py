#!/usr/bin/env python3
"""Small GTK4 accessibility fixture for process-level AT-SPI snapshots."""

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402


def activate(app):
    window = Gtk.ApplicationWindow(application=app)
    window.set_title("CuaTestHarness GTK4")
    window.set_default_size(420, 180)

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
    box.set_margin_top(20)
    box.set_margin_bottom(20)
    box.set_margin_start(20)
    box.set_margin_end(20)

    box.append(Gtk.Label(label="GTK4_ATSPI_MARKER_v1"))
    box.append(Gtk.Entry(placeholder_text="GTK4 editable target"))
    box.append(Gtk.Button(label="GTK4 actionable target"))
    window.set_child(box)
    window.present()


application = Gtk.Application(application_id="ai.cua.testharness.gtk4")
application.connect("activate", activate)
raise SystemExit(application.run(None))
