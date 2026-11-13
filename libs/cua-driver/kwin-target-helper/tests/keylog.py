# SPDX-License-Identifier: MIT
# A GTK 4 window that logs each key and click it receives: "NAME key X", "NAME click N".
import sys
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gdk  # noqa: E402 (after gi.require_version)

name, log = sys.argv[1], sys.argv[2]


def w(line):
    with open(log, "a") as f:
        f.write(f"{name} {line}\n")


def act(app):
    win = Gtk.ApplicationWindow(application=app, title=name)
    win.set_default_size(500, 400)
    k = Gtk.EventControllerKey()
    k.connect("key-pressed", lambda c, kv, kc, st: w(f"key {Gdk.keyval_name(kv)}") or False)
    win.add_controller(k)
    g = Gtk.GestureClick()
    g.set_button(0)
    g.connect("pressed", lambda c, n, x, y: w(f"click {c.get_current_button()}"))
    win.add_controller(g)
    win.present()


app = Gtk.Application(application_id=f"ai.cua.test.{name}")
app.connect("activate", act)
app.run([])
