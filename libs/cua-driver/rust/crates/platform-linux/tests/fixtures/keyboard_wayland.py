import json
import os

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, GLib, Gtk


def emit(kind, **fields):
    print(json.dumps({"kind": kind, **fields}), flush=True)


def key_event(widget, event, kind):
    emit(kind, key=int(event.keyval), flags=int(event.state), synthetic=bool(event.send_event))
    return False


name = f"CuaKeyboardOracle-{os.getpid()}"
GLib.set_prgname(name)
Gtk.init([])
assert "Wayland" in Gdk.Display.get_default().__gtype__.name
window = Gtk.Window(title=name)
window.set_default_size(420, 240)
entry = Gtk.Entry(text="unchanged")
entry.get_accessible().set_name("Keyboard input")
entry.connect("key-press-event", key_event, "down")
entry.connect("key-release-event", key_event, "up")
window.add(entry)
window.connect("destroy", Gtk.main_quit)
window.show_all()
entry.grab_focus()
window.present()
GLib.timeout_add(200, lambda: emit("ready", window=0))
Gtk.main()
