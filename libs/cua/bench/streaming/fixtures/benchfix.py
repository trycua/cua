#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Streaming benchmark fixture (GTK3, stdlib + PyGObject only).

One undecorated window, "CUA Bench Timecode" (WM_CLASS cua-bench-timecode),
whose content is self-describing so a client can measure latency from the
decoded frames alone:

* **Timecode strip** at content (0, 0): 48 cells of CELL x CELL pixels in
  one row, black (0) or white (1):
    - cells 0..3   sync  1 0 1 0
    - cells 4..35  guest wall clock in ms (unix ms mod 2**32), MSB first
    - cells 36..43 checksum: XOR of the four value bytes
    - cells 44..47 sync  0 1 0 1
  The value is the wall-clock time at which the frame was drawn.
* **Photon square** at content (PHOTON_X, PHOTON_Y), PHOTON x PHOTON pixels:
  black, and toggles white/black on every button press anywhere in the
  window (input-to-photon).
* **Content area** below y = CONTENT_Y, depending on --mode:
    static    nothing moves; the timecode is drawn once (bytes/s floor)
    timecode  only the strip and a small spinner change (small damage)
    scroll    a text page scrolling up continuously (text scroll)
    video     moving 8x8 colour blocks over a moving gradient (video-like)
    drag      a small window that moves itself along a circle (window drag)
* **Time server** on TCP --time-port (default 18081): for every byte read it
  answers 8 bytes, big-endian `time.time_ns()`, so the client can estimate
  the guest/host clock offset (min-RTT, NTP style).

Events go to $CUA_FIXTURE_LOG_DIR/<name>.jsonl like the image's fixtures:
`ready`, `click` (with the photon state), `moved` (drag mode).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import socket
import struct
import sys
import threading
import time

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
import cairo  # noqa: E402
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

TITLE = "CUA Bench Timecode"
CELL = 16
CELLS = 48
PHOTON_X, PHOTON_Y, PHOTON = 16, 32, 96
CONTENT_Y = 144
SIZES = {
    "static": (1024, 640),
    "timecode": (1024, 640),
    "scroll": (1024, 640),
    "video": (1024, 640),
    "drag": (800, 320),
}


def timecode_bits(ms: int) -> list[int]:
    value = ms & 0xFFFFFFFF
    b = value.to_bytes(4, "big")
    check = b[0] ^ b[1] ^ b[2] ^ b[3]
    bits = [1, 0, 1, 0]
    bits += [(value >> (31 - i)) & 1 for i in range(32)]
    bits += [(check >> (7 - i)) & 1 for i in range(8)]
    bits += [0, 1, 0, 1]
    return bits


class Log:
    def __init__(self, name: str) -> None:
        log_dir = os.environ.get("CUA_FIXTURE_LOG_DIR", "/tmp/cua-fixtures")
        os.makedirs(log_dir, exist_ok=True)
        self.fh = open(os.path.join(log_dir, f"{name}.jsonl"), "a", buffering=1)
        self.lock = threading.Lock()

    def emit(self, kind: str, **fields) -> None:
        rec = {"ts": round(time.time(), 6), "fixture": "benchfix", "type": kind}
        rec.update(fields)
        with self.lock:
            self.fh.write(json.dumps(rec, sort_keys=True) + "\n")


def serve_time(port: int, log: Log) -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", port))
    srv.listen(4)

    def client(conn: socket.socket) -> None:
        conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        with conn:
            while True:
                data = conn.recv(64)
                if not data:
                    return
                for _ in data:
                    conn.sendall(struct.pack(">Q", time.time_ns()))

    while True:
        conn, _ = srv.accept()
        threading.Thread(target=client, args=(conn,), daemon=True).start()


class BenchWindow(Gtk.Window):
    def __init__(self, mode: str, log: Log, x: int, y: int) -> None:
        super().__init__(title=TITLE)
        self.mode, self.log = mode, log
        self.width, self.height = SIZES[mode]
        self.photon = False
        self.frame = 0
        self.origin = (x, y)
        self.set_decorated(False)
        self.set_resizable(False)
        self.set_keep_above(True)
        self.set_default_size(self.width, self.height)
        self.move(x, y)
        area = Gtk.DrawingArea()
        area.set_size_request(self.width, self.height)
        area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
        area.connect("draw", self.on_draw)
        area.connect("button-press-event", self.on_press)
        self.add(area)
        self.area = area
        self.connect("destroy", lambda *_: Gtk.main_quit())
        rng = random.Random(1234)
        self.blocks = [
            (rng.random(), rng.random(), rng.random()) for _ in range(64 * 64)
        ]
        self.lines = [
            f"{i:05d}  The quick brown fox jumps over the lazy dog; "
            f"streaming benchmark text line {i * 7919 % 10007:05d}."
            for i in range(400)
        ]
        if mode not in ("static",):
            GLib.timeout_add(16, self.tick)

    def tick(self) -> bool:
        self.frame += 1
        if self.mode == "drag":
            t = time.monotonic()
            cx, cy = self.origin
            x = int(cx + 180 * math.cos(t * 1.5))
            y = int(cy + 120 * math.sin(t * 1.5))
            self.move(x, y)
        self.area.queue_draw()
        return True

    def on_press(self, _w, event) -> bool:
        self.photon = not self.photon
        self.log.emit(
            "click",
            photon=self.photon,
            x=round(event.x, 1),
            y=round(event.y, 1),
            button=event.button,
        )
        self.area.queue_draw()
        return True

    def on_draw(self, _w, cr) -> bool:
        cr.set_source_rgb(0.5, 0.5, 0.5)
        cr.paint()
        self.draw_content(cr)
        # Photon square.
        v = 1.0 if self.photon else 0.0
        cr.set_source_rgb(v, v, v)
        cr.rectangle(PHOTON_X, PHOTON_Y, PHOTON, PHOTON)
        cr.fill()
        # Timecode last, stamped as late as possible.
        ms = time.time_ns() // 1_000_000
        for i, bit in enumerate(timecode_bits(ms)):
            cr.set_source_rgb(bit, bit, bit)
            cr.rectangle(i * CELL, 0, CELL, CELL)
            cr.fill()
        return False

    def draw_content(self, cr) -> None:
        w, h = self.width, self.height
        if self.mode == "timecode":
            # A small spinner next to the photon square.
            a = self.frame * 0.2
            cr.set_source_rgb(0.1, 0.1, 0.8)
            cr.arc(200 + 30 * math.cos(a), 80 + 30 * math.sin(a), 10, 0, 2 * math.pi)
            cr.fill()
        elif self.mode == "scroll":
            cr.set_source_rgb(1, 1, 1)
            cr.rectangle(0, CONTENT_Y, w, h - CONTENT_Y)
            cr.fill()
            cr.select_font_face("monospace", cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_NORMAL)
            cr.set_font_size(14)
            cr.set_source_rgb(0, 0, 0)
            line_h = 18
            offset = (self.frame * 3) % (line_h * len(self.lines))
            first = offset // line_h
            y = CONTENT_Y + line_h - (offset % line_h)
            i = first
            cr.save()
            cr.rectangle(0, CONTENT_Y, w, h - CONTENT_Y)
            cr.clip()
            while y < h + line_h:
                cr.move_to(8, y)
                cr.show_text(self.lines[i % len(self.lines)])
                y += line_h
                i += 1
            cr.restore()
        elif self.mode == "video":
            t = self.frame
            grad = cairo.LinearGradient(0, CONTENT_Y, w, h)
            p = (t % 120) / 120
            grad.add_color_stop_rgb(0, p, 0.2, 1 - p)
            grad.add_color_stop_rgb(1, 1 - p, 0.8, p)
            cr.set_source(grad)
            cr.rectangle(0, CONTENT_Y, w, h - CONTENT_Y)
            cr.fill()
            bs = 8
            dx, dy = (t * 3) % (64 * bs), (t * 2) % (64 * bs)
            for j in range((h - CONTENT_Y) // bs // 2):
                for i in range(w // bs // 2):
                    r, g, b = self.blocks[((j * 2 + dy // bs) % 64) * 64 + (i * 2 + dx // bs) % 64]
                    cr.set_source_rgb(r, g, b)
                    cr.rectangle(i * bs * 2, CONTENT_Y + j * bs * 2, bs, bs)
                    cr.fill()
        elif self.mode == "drag":
            cr.set_source_rgb(0.2, 0.6, 0.2)
            cr.rectangle(0, CONTENT_Y, w, h - CONTENT_Y)
            cr.fill()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=sorted(SIZES), default="timecode")
    ap.add_argument("--time-port", type=int, default=18081)
    ap.add_argument("--x", type=int, default=128)
    ap.add_argument("--y", type=int, default=96)
    ap.add_argument("--name", default="benchfix")
    args = ap.parse_args()
    GLib.set_prgname("cua-bench-timecode")
    Gdk.set_program_class("CuaBench")
    log = Log(args.name)
    if args.time_port:
        threading.Thread(target=serve_time, args=(args.time_port, log), daemon=True).start()
    win = BenchWindow(args.mode, log, args.x, args.y)
    win.show_all()

    def ready() -> bool:
        gdk_win = win.get_window()
        origin = gdk_win.get_origin() if gdk_win else (0, 0, 0)
        log.emit(
            "ready",
            pid=os.getpid(),
            title=TITLE,
            mode=args.mode,
            x=origin[1],
            y=origin[2],
            width=win.width,
            height=win.height,
            cell=CELL,
        )
        return False

    GLib.timeout_add(300, ready)
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main())
