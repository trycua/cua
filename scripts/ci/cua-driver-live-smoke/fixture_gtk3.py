#!/usr/bin/env python3
"""GTK3 window for the cua-driver live smoke: a text entry, two buttons, a
dropdown combo box and an editable combo box, each with a label that echoes
its state so a fresh snapshot can verify every action."""

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk  # noqa: E402


def named(widget, name):
    widget.get_accessible().set_name(name)
    return widget


def main():
    win = Gtk.Window(title="Cua Live Smoke")
    win.set_default_size(420, 760)
    win.connect("destroy", Gtk.main_quit)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    box.set_border_width(8)
    win.add(box)

    entry = named(Gtk.Entry(), "Name field")
    status = Gtk.Label(label="Status: idle")
    apply_button = Gtk.Button(label="Apply")
    apply_button.connect(
        "clicked", lambda _b: status.set_text("Applied: " + entry.get_text())
    )

    count = Gtk.Label(label="Count: 0")
    clicks = {"n": 0}

    def increment(_button):
        clicks["n"] += 1
        count.set_text(f"Count: {clicks['n']}")

    increment_button = Gtk.Button(label="Increment")
    increment_button.connect("clicked", increment)

    color = Gtk.Label(label="Color: Red")
    color_combo = named(Gtk.ComboBoxText(), "Color")
    for item in ("Red", "Green", "Blue"):
        color_combo.append_text(item)
    color_combo.set_active(0)
    color_combo.connect(
        "changed", lambda c: color.set_text("Color: " + (c.get_active_text() or ""))
    )

    size = Gtk.Label(label="Size: Small")
    size_combo = named(Gtk.ComboBoxText.new_with_entry(), "Size")
    for item in ("Small", "Medium", "Large"):
        size_combo.append_text(item)
    size_combo.set_active(0)
    size_combo.get_child().get_accessible().set_name("Size entry")
    size_combo.connect(
        "changed", lambda c: size.set_text("Size: " + (c.get_active_text() or ""))
    )

    for widget in (
        entry, apply_button, status, increment_button, count,
        color_combo, color, size_combo, size,
    ):
        box.pack_start(widget, False, False, 0)
    # Padding rows keep a one-row change small next to the whole tree, as in a
    # real window, so `since` answers with a diff rather than a full read.
    for row in range(1, 16):
        box.pack_start(Gtk.Label(label=f"Row {row}"), False, False, 0)

    win.show_all()
    Gtk.main()


if __name__ == "__main__":
    main()
