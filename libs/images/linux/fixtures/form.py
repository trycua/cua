#!/usr/bin/env python3
"""Second fixture window: a small accessible form (GTK3).

Widgets carry stable accessible names so AT-SPI queries have something real to
find: an entry ("Name"), a multi-line text view ("Notes"), a check box
("Subscribe"), a combo box ("Color"), a "Submit" button and a status label.
Each widget change, key event and click is logged as JSONL (see
fixturelog.py). "Submit" logs the complete form state.

Environment: CUA_FIXTURE_NAME (default "form"), CUA_FORM_TITLE,
CUA_FIXTURE_LOG_DIR.
"""

from __future__ import annotations

import os
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fixturelog import FixtureLog  # noqa: E402

NAME = os.environ.get("CUA_FIXTURE_NAME", "form")
TITLE = os.environ.get("CUA_FORM_TITLE", "CUA Fixture Form")
COLORS = ["red", "green", "blue"]


def named(widget: Gtk.Widget, name: str) -> Gtk.Widget:
    widget.set_name(name)
    widget.get_accessible().set_name(name)
    return widget


class FormWindow(Gtk.Window):
    def __init__(self, log: FixtureLog) -> None:
        super().__init__(title=TITLE)
        self.log = log
        self.set_default_size(420, 320)

        grid = Gtk.Grid(column_spacing=8, row_spacing=8, margin=12)
        self.entry = named(Gtk.Entry(), "Name")
        self.notes = named(Gtk.TextView(), "Notes")
        self.notes.set_size_request(-1, 90)
        self.check = named(Gtk.CheckButton(label="Subscribe"), "Subscribe")
        self.combo = named(Gtk.ComboBoxText(), "Color")
        for c in COLORS:
            self.combo.append_text(c)
        self.combo.set_active(0)
        self.button = named(Gtk.Button(label="Submit"), "Submit")
        self.status = named(Gtk.Label(label="idle"), "Status")

        grid.attach(Gtk.Label(label="Name", xalign=0), 0, 0, 1, 1)
        grid.attach(self.entry, 1, 0, 1, 1)
        grid.attach(Gtk.Label(label="Notes", xalign=0), 0, 1, 1, 1)
        grid.attach(self.notes, 1, 1, 1, 1)
        grid.attach(self.check, 1, 2, 1, 1)
        grid.attach(Gtk.Label(label="Color", xalign=0), 0, 3, 1, 1)
        grid.attach(self.combo, 1, 3, 1, 1)
        grid.attach(self.button, 1, 4, 1, 1)
        grid.attach(self.status, 0, 5, 2, 1)
        self.add(grid)

        self.entry.connect("changed", lambda e: log.emit("entry_changed", widget="Name", text=e.get_text()))
        self.entry.connect("activate", lambda e: log.emit("entry_activate", widget="Name", text=e.get_text()))
        self.notes.get_buffer().connect("changed", self.on_notes)
        self.check.connect("toggled", lambda c: log.emit("toggled", widget="Subscribe", active=c.get_active()))
        self.combo.connect("changed", lambda c: log.emit("combo_changed", widget="Color", value=c.get_active_text()))
        self.button.connect("clicked", self.on_submit)
        self.connect("key-press-event", self.on_key, "key_press")
        self.connect("key-release-event", self.on_key, "key_release")
        self.connect("button-press-event", self.on_button, "button_press")
        self.connect("button-release-event", self.on_button, "button_release")
        self.connect("focus-in-event", lambda *_: log.emit("focus_in") or False)
        self.connect("focus-out-event", lambda *_: log.emit("focus_out") or False)
        self.connect("destroy", self.on_destroy)

    def on_notes(self, buf) -> None:
        start, end = buf.get_bounds()
        self.log.emit("notes_changed", widget="Notes", text=buf.get_text(start, end, True))

    def form_state(self) -> dict:
        buf = self.notes.get_buffer()
        start, end = buf.get_bounds()
        return {
            "name": self.entry.get_text(),
            "notes": buf.get_text(start, end, True),
            "subscribe": self.check.get_active(),
            "color": self.combo.get_active_text(),
        }

    def on_submit(self, _b) -> None:
        state = self.form_state()
        self.status.set_text("submitted")
        self.log.emit("submit", **state)

    def on_key(self, _w, event, kind: str) -> bool:
        uni = Gdk.keyval_to_unicode(event.keyval)
        focus = self.get_focus()
        self.log.emit(
            kind,
            keyval=event.keyval,
            key=Gdk.keyval_name(event.keyval),
            text=chr(uni) if uni else "",
            keycode=event.hardware_keycode,
            focus=focus.get_name() if focus else None,
        )
        return False

    def on_button(self, _w, event, kind: str) -> bool:
        self.log.emit(
            kind,
            button=event.button,
            x=round(event.x, 2),
            y=round(event.y, 2),
            x_root=round(event.x_root, 2),
            y_root=round(event.y_root, 2),
        )
        return False

    def on_destroy(self, *_):
        self.log.emit("exit")
        Gtk.main_quit()


def main() -> int:
    # WM_CLASS = ("cua-fixture-<name>", "CuaFixture") for window matching.
    GLib.set_prgname(f"cua-fixture-{NAME}")
    Gdk.set_program_class("CuaFixture")
    log = FixtureLog(NAME)
    win = FormWindow(log)
    win.show_all()
    GLib.timeout_add(
        300,
        lambda: log.emit("ready", pid=os.getpid(), title=TITLE, wm_class=f"cua-fixture-{NAME}", **win.form_state())
        or False,
    )
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main())
