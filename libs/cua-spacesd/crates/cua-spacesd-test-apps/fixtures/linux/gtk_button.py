#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""GTK 3 accessibility fixture for Linux conformance tests.

A window titled by --title with one button labelled "Press Me" and one text
entry. Every activation of the button (mouse, keyboard or an AT-SPI action)
appends {"event": "clicked"} to --log, so a test can prove that
AccessibilityService.Act reached the real widget. Runs only inside the test
container (Xvfb + AT-SPI bus).
"""
import argparse
import json
import os
import time

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument("--title", default="cua gtk fixture")
parser.add_argument("--log", required=True)
args = parser.parse_args()
started = time.monotonic()


def log(**event):
    event["t_ms"] = int((time.monotonic() - started) * 1000)
    with open(args.log, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(event) + "\n")


window = Gtk.Window(title=args.title)
window.set_default_size(320, 160)
window.move(40, 40)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
button = Gtk.Button(label="Press Me")
button.connect("clicked", lambda *_: log(event="clicked"))
entry = Gtk.Entry()
entry.connect("changed", lambda widget: log(event="text", value=widget.get_text()))
box.pack_start(button, True, True, 0)
box.pack_start(entry, True, True, 0)
window.add(box)
window.connect("destroy", Gtk.main_quit)
window.show_all()
GLib.idle_add(lambda: log(event="ready", pid=os.getpid(), title=args.title) and False)
Gtk.main()
