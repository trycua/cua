#!/usr/bin/env python3
"""Deterministic color-grid fixture window (GTK3).

Paints a COLS x ROWS grid of CELL-pixel squares. Cell (col, row) is filled with
the RGB color::

    r = col * 255 // (COLS - 1)
    g = row * 255 // (ROWS - 1)
    b = BLUE

so any sampled pixel identifies the cell it came from, and a screenshot can be
checked without a golden image. Every pointer, scroll, key, focus and
configure event is logged as JSONL (see fixturelog.py) with window-relative and
root coordinates plus the cell under the pointer.

Environment: CUA_FIXTURE_NAME (default "grid"), CUA_GRID_COLS/ROWS/CELL/BLUE,
CUA_GRID_TITLE, CUA_FIXTURE_LOG_DIR.
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

NAME = os.environ.get("CUA_FIXTURE_NAME", "grid")
COLS = int(os.environ.get("CUA_GRID_COLS", "8"))
ROWS = int(os.environ.get("CUA_GRID_ROWS", "6"))
CELL = int(os.environ.get("CUA_GRID_CELL", "80"))
BLUE = int(os.environ.get("CUA_GRID_BLUE", "128"))
TITLE = os.environ.get("CUA_GRID_TITLE", "CUA Fixture Grid")


def cell_color(col: int, row: int) -> tuple[int, int, int]:
    return (col * 255 // max(COLS - 1, 1), row * 255 // max(ROWS - 1, 1), BLUE)


def cell_at(x: float, y: float) -> list[int] | None:
    col, row = int(x // CELL), int(y // CELL)
    if 0 <= col < COLS and 0 <= row < ROWS:
        return [col, row]
    return None


def mods(state: Gdk.ModifierType) -> list[str]:
    names = []
    for flag, name in (
        (Gdk.ModifierType.SHIFT_MASK, "shift"),
        (Gdk.ModifierType.CONTROL_MASK, "ctrl"),
        (Gdk.ModifierType.MOD1_MASK, "alt"),
        (Gdk.ModifierType.SUPER_MASK, "super"),
        (Gdk.ModifierType.BUTTON1_MASK, "button1"),
        (Gdk.ModifierType.BUTTON2_MASK, "button2"),
        (Gdk.ModifierType.BUTTON3_MASK, "button3"),
    ):
        if state & flag:
            names.append(name)
    return names


class GridWindow(Gtk.Window):
    def __init__(self, log: FixtureLog) -> None:
        super().__init__(title=TITLE)
        self.log = log
        self.set_resizable(False)
        area = Gtk.DrawingArea()
        area.set_size_request(COLS * CELL, ROWS * CELL)
        area.set_can_focus(True)
        area.get_accessible().set_name(f"{TITLE} canvas")
        area.add_events(
            Gdk.EventMask.BUTTON_PRESS_MASK
            | Gdk.EventMask.BUTTON_RELEASE_MASK
            | Gdk.EventMask.POINTER_MOTION_MASK
            | Gdk.EventMask.SCROLL_MASK
            | Gdk.EventMask.SMOOTH_SCROLL_MASK
            | Gdk.EventMask.KEY_PRESS_MASK
            | Gdk.EventMask.KEY_RELEASE_MASK
            | Gdk.EventMask.ENTER_NOTIFY_MASK
            | Gdk.EventMask.LEAVE_NOTIFY_MASK
        )
        area.connect("draw", self.on_draw)
        area.connect("button-press-event", self.on_button, "button_press")
        area.connect("button-release-event", self.on_button, "button_release")
        area.connect("motion-notify-event", self.on_motion)
        area.connect("scroll-event", self.on_scroll)
        area.connect("enter-notify-event", self.on_crossing, "enter")
        area.connect("leave-notify-event", self.on_crossing, "leave")
        self.connect("key-press-event", self.on_key, "key_press")
        self.connect("key-release-event", self.on_key, "key_release")
        self.connect("focus-in-event", lambda *_: self.log.emit("focus_in") or False)
        self.connect("focus-out-event", lambda *_: self.log.emit("focus_out") or False)
        self.connect("configure-event", self.on_configure)
        self.connect("destroy", self.on_destroy)
        self.add(area)
        self.area = area
        self._last_geom = None

    def on_draw(self, _widget, cr) -> bool:
        for row in range(ROWS):
            for col in range(COLS):
                r, g, b = cell_color(col, row)
                cr.set_source_rgb(r / 255, g / 255, b / 255)
                cr.rectangle(col * CELL, row * CELL, CELL, CELL)
                cr.fill()
        return False

    def _pointer(self, event) -> dict:
        return {
            "x": round(event.x, 2),
            "y": round(event.y, 2),
            "x_root": round(event.x_root, 2),
            "y_root": round(event.y_root, 2),
            "cell": cell_at(event.x, event.y),
            "mods": mods(event.state),
        }

    def on_button(self, _w, event, kind: str) -> bool:
        if kind == "button_press":
            self.area.grab_focus()
        click = {
            Gdk.EventType.BUTTON_PRESS: 1,
            Gdk.EventType._2BUTTON_PRESS: 2,
            Gdk.EventType._3BUTTON_PRESS: 3,
        }.get(event.type, 0)
        self.log.emit(kind, button=event.button, click_count=click, **self._pointer(event))
        return True

    def on_motion(self, _w, event) -> bool:
        self.log.emit("motion", **self._pointer(event))
        return True

    def on_scroll(self, _w, event) -> bool:
        direction = {
            Gdk.ScrollDirection.UP: "up",
            Gdk.ScrollDirection.DOWN: "down",
            Gdk.ScrollDirection.LEFT: "left",
            Gdk.ScrollDirection.RIGHT: "right",
            Gdk.ScrollDirection.SMOOTH: "smooth",
        }.get(event.direction, str(event.direction))
        ok, dx, dy = event.get_scroll_deltas()
        self.log.emit(
            "scroll",
            direction=direction,
            dx=round(dx, 3) if ok else 0.0,
            dy=round(dy, 3) if ok else 0.0,
            **self._pointer(event),
        )
        return True

    def on_crossing(self, _w, event, kind: str) -> bool:
        self.log.emit(kind, **self._pointer(event))
        return False

    def on_key(self, _w, event, kind: str) -> bool:
        uni = Gdk.keyval_to_unicode(event.keyval)
        self.log.emit(
            kind,
            keyval=event.keyval,
            key=Gdk.keyval_name(event.keyval),
            text=chr(uni) if uni else "",
            keycode=event.hardware_keycode,
            mods=mods(event.state),
        )
        return True

    def on_configure(self, _w, event) -> bool:
        geom = (event.x, event.y, event.width, event.height)
        if geom != self._last_geom:
            self._last_geom = geom
            self.log.emit("configure", x=event.x, y=event.y, width=event.width, height=event.height)
        return False

    def on_destroy(self, *_):
        self.log.emit("exit")
        Gtk.main_quit()


def main() -> int:
    # WM_CLASS = ("cua-fixture-<name>", "CuaFixture") for window matching.
    GLib.set_prgname(f"cua-fixture-{NAME}")
    Gdk.set_program_class("CuaFixture")
    log = FixtureLog(NAME)
    win = GridWindow(log)
    win.show_all()

    def ready() -> bool:
        gdk_win = win.get_window()
        origin = gdk_win.get_origin() if gdk_win else (0, 0, 0)
        log.emit(
            "ready",
            pid=os.getpid(),
            title=TITLE,
            wm_class=f"cua-fixture-{NAME}",
            xid=gdk_win.get_xid() if gdk_win and hasattr(gdk_win, "get_xid") else None,
            origin=[origin[1], origin[2]] if len(origin) == 3 else list(origin),
            cols=COLS,
            rows=ROWS,
            cell=CELL,
            blue=BLUE,
            color_formula="r=col*255//(cols-1), g=row*255//(rows-1), b=blue",
        )
        return False

    GLib.timeout_add(300, ready)
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main())
