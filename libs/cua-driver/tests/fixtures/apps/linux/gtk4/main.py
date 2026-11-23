#!/usr/bin/env python3
"""Small GTK4 accessibility fixture for process-level AT-SPI snapshots.

The window draws its own stock header bar, so its X11 client window and its
screenshot both include the title bar. The magenta pixel target lets a test
ground a click on the screenshot alone, the way a vision model does (#3237).
"""

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

PIXEL_TARGET_CSS = """
button.pixel-target {
    background: #ff00ff;
    background-image: none;
    border: none;
    box-shadow: none;
    min-height: 0;
    padding: 0;
}
"""


def install_css(display):
    provider = Gtk.CssProvider()
    if hasattr(provider, "load_from_string"):
        provider.load_from_string(PIXEL_TARGET_CSS)
    else:
        provider.load_from_data(PIXEL_TARGET_CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        display, provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )


def activate(app):
    window = Gtk.ApplicationWindow(application=app)
    window.set_title("CuaTestHarness GTK4")
    window.set_titlebar(Gtk.HeaderBar())
    window.set_default_size(420, 180)
    install_css(window.get_display())

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
    box.set_margin_top(20)
    box.set_margin_bottom(20)
    box.set_margin_start(20)
    box.set_margin_end(20)

    box.append(Gtk.Label(label="GTK4_ATSPI_MARKER_v1"))
    box.append(Gtk.Entry(placeholder_text="GTK4 editable target"))
    box.append(Gtk.Button(label="GTK4 actionable target"))

    clicks = Gtk.Label(label="GTK4_PIXEL_CLICKS=0")
    box.append(clicks)
    count = [0]

    def on_pixel_click(_button):
        count[0] += 1
        clicks.set_label(f"GTK4_PIXEL_CLICKS={count[0]}")

    # Shorter than a header bar, with only margin below: a click that lands
    # a title bar's height off the screenshot misses it.
    target = Gtk.Button()
    target.add_css_class("pixel-target")
    target.set_size_request(160, 20)
    target.set_halign(Gtk.Align.CENTER)
    target.set_tooltip_text("GTK4 pixel target")
    target.connect("clicked", on_pixel_click)
    box.append(target)

    window.set_child(box)
    window.present()


application = Gtk.Application(application_id="ai.cua.testharness.gtk4")
application.connect("activate", activate)
raise SystemExit(application.run(None))
