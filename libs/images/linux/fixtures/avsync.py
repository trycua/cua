#!/usr/bin/env python3
"""A/V sync fixture: a white flash and a beep at the same instant, every second.

A 240x240 window ("CUA Fixture AV Sync") stays black except for 100 ms after
each wall-clock second boundary, when it turns white; at the same boundary a
1 kHz, 100 ms beep starts on the default sink. A streaming client measures the
skew between the first white video frame and the beep onset (budget +-40 ms).

JSONL records (see fixturelog.py), all carrying the shared `boundary` (unix
seconds, integer) the pair belongs to:
  beep         the beep's first chunk was handed to the audio server
  flash        the white frame was requested
  flash_drawn  GTK actually painted it
`mono` fields are time.monotonic() in this process, so skews are computed on
one clock.

Environment: CUA_FIXTURE_NAME (default "avsync"), CUA_FIXTURE_AUDIO_SINK.
"""

from __future__ import annotations

import math
import os
import sys
import threading
import time

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fixtureaudio import RATE, PacedPlayer, silence, tone  # noqa: E402
from fixturelog import FixtureLog  # noqa: E402

NAME = os.environ.get("CUA_FIXTURE_NAME", "avsync")
TITLE = "CUA Fixture AV Sync"
FLASH_MS = 100
BEEP_HZ = 1000


class AvWindow(Gtk.Window):
    def __init__(self, log: FixtureLog) -> None:
        super().__init__(title=TITLE)
        self.log = log
        self.white = False
        self.pending: int | None = None
        self.set_default_size(240, 240)
        self.set_resizable(False)
        area = Gtk.DrawingArea()
        area.set_size_request(240, 240)
        area.connect("draw", self.on_draw)
        self.add(area)
        self.area = area
        self.connect("destroy", lambda *_: Gtk.main_quit())

    def on_draw(self, _w, cr) -> bool:
        v = 1.0 if self.white else 0.0
        cr.set_source_rgb(v, v, v)
        cr.paint()
        if self.white and self.pending is not None:
            self.log.emit("flash_drawn", boundary=self.pending, mono=round(time.monotonic(), 6))
            self.pending = None
        return False

    def flash(self, boundary: int) -> bool:
        self.white = True
        self.pending = boundary
        self.log.emit("flash", boundary=boundary, mono=round(time.monotonic(), 6))
        self.area.queue_draw()
        GLib.timeout_add(FLASH_MS, self.unflash)
        return False

    def unflash(self) -> bool:
        self.white = False
        self.area.queue_draw()
        return False


def audio_loop(win: AvWindow, log: FixtureLog) -> None:
    player = PacedPlayer(NAME)
    beep = tone(BEEP_HZ, FLASH_MS)
    while True:
        now_wall, now_mono = time.time(), time.monotonic()
        boundary = math.floor(now_wall) + 1
        boundary_mono = now_mono + (boundary - now_wall)
        # Silence up to the boundary on the stream's own frame clock.
        frames = int((boundary_mono - player.t0) * RATE) - player.frames
        if frames > 0:
            player.play(silence(frames * 1000 // RATE))
        # Flash is requested at the instant the beep's first chunk is written.
        mono = player.play(beep, on_start=lambda b=boundary: GLib.idle_add(win.flash, b))
        log.emit("beep", boundary=boundary, mono=round(mono, 6), freq_hz=BEEP_HZ, ms=FLASH_MS)


def main() -> int:
    GLib.set_prgname(f"cua-fixture-{NAME}")
    Gdk.set_program_class("CuaFixture")
    log = FixtureLog(NAME)
    win = AvWindow(log)
    win.show_all()
    threading.Thread(target=audio_loop, args=(win, log), daemon=True).start()
    GLib.timeout_add(300, lambda: log.emit("ready", pid=os.getpid(), title=TITLE, flash_ms=FLASH_MS, beep_hz=BEEP_HZ) or False)
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main())
