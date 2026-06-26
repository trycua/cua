#!/usr/bin/env python3
# CuaTestHarness.Gtk3 — Linux GTK3 test-harness app.
#
# The Linux analogue of the AppKit/SwiftUI (macOS) and WPF/WinUI3 (Windows)
# harness apps: a controlled, scenario-driven GUI whose elements cua-driver
# drives + asserts against, so Linux reaches parity with the other platforms'
# controlled-harness coverage (in addition to the real-app Nix scenarios).
#
# Toolkit note: GTK3 exposes accessibility via ATK → AT-SPI. cua-driver's Linux
# get_window_state renders each element as `[idx] <role> "<name>"` — there is NO
# `id=` field like Windows (UIA AutomationId) / macOS (AX identifier). So the
# CONTRACT here is the **AT-SPI accessible name**: each actionable control sets
# its accessible name to the scenario's `aid` (e.g. "btn-increment"), and marker
# labels carry their marker text. The Rust test matches those names in the tree.
#
# Scenarios mirror the SwiftUI set (shared/scenarios.json "gtk3"):
#   counter     : button increments a State<int>, label shows "counter=N"
#   text_body   : static label carrying HARNESS_TEXT_MARKER_v1
#   text_input  : entry whose text is mirrored into a label
#   popover     : GtkPopover (the GTK analogue of SwiftUI .popover() /
#                 WinUI3 CommandBarFlyout) carrying POPOVER_MARKER_v1
#   exit        : button that quits

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk  # noqa: E402


def aid(widget, name):
    """Set the AT-SPI accessible name (the Linux harness's stand-in for an
    AutomationId/AX-id, since the AT-SPI snapshot exposes name, not id)."""
    widget.get_accessible().set_name(name)
    return widget


class HarnessWindow(Gtk.Window):
    def __init__(self):
        super().__init__(title="CuaTestHarness GTK3")
        self.set_default_size(420, 520)
        self.set_border_width(12)
        self.counter = 0

        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        self.add(root)

        # ── counter ───────────────────────────────────────────────────────
        self.counter_label = Gtk.Label(label="counter=0")
        root.pack_start(self.counter_label, False, False, 0)

        btn_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        inc = aid(Gtk.Button(label="Increment"), "btn-increment")
        inc.connect("clicked", self.on_increment)
        rst = aid(Gtk.Button(label="Reset"), "btn-reset")
        rst.connect("clicked", self.on_reset)
        btn_row.pack_start(inc, False, False, 0)
        btn_row.pack_start(rst, False, False, 0)
        root.pack_start(btn_row, False, False, 0)

        # ── text_body (static marker) ─────────────────────────────────────
        root.pack_start(Gtk.Label(label="HARNESS_TEXT_MARKER_v1"), False, False, 0)

        # ── text_input (entry → mirror label) ─────────────────────────────
        self.entry = aid(Gtk.Entry(), "txt-input")
        self.entry.set_placeholder_text("type here")
        self.entry.connect("changed", self.on_entry_changed)
        root.pack_start(self.entry, False, False, 0)
        self.mirror = Gtk.Label(label="mirror=")
        root.pack_start(self.mirror, False, False, 0)

        # ── popover ───────────────────────────────────────────────────────
        open_pop = aid(Gtk.Button(label="Open Popover"), "btn-open-popover")
        open_pop.connect("clicked", self.on_open_popover)
        root.pack_start(open_pop, False, False, 0)
        self.popover = Gtk.Popover.new(open_pop)
        self.popover.set_border_width(10)
        self.popover.add(Gtk.Label(label="POPOVER_MARKER_v1"))

        # ── exit ──────────────────────────────────────────────────────────
        ext = aid(Gtk.Button(label="Exit"), "btn-exit")
        ext.connect("clicked", lambda *_: Gtk.main_quit())
        root.pack_start(ext, False, False, 0)

        self.connect("destroy", Gtk.main_quit)

    def on_increment(self, *_):
        self.counter += 1
        self.counter_label.set_text(f"counter={self.counter}")

    def on_reset(self, *_):
        self.counter = 0
        self.counter_label.set_text("counter=0")

    def on_entry_changed(self, entry):
        self.mirror.set_text(f"mirror={entry.get_text()}")

    def on_open_popover(self, *_):
        self.popover.show_all()


def main():
    win = HarnessWindow()
    win.show_all()
    Gtk.main()


if __name__ == "__main__":
    main()
