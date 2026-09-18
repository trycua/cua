import os
import pty
import signal
import socket
import sys
import gi

gi.require_version("Gtk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gtk, Gdk, GdkX11, GLib

observer = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
address = ("127.0.0.1", int(sys.argv[1]))


def emit(message):
    observer.sendto(message.encode(), address)


child, master = pty.fork()
if child == 0:
    emit("pty_ready")
    for line in sys.stdin:
        emit("pty")
    os._exit(0)

window = Gtk.Window(title="Cua Terminal Oracle")
window.set_wmclass("CuaTerminalOracle", "XTerm")
window.set_default_size(400, 240)
entry = Gtk.Entry()
entry.set_text("unchanged")
entry.get_accessible().set_name("Terminal input")
window.add(entry)


def key_pressed(widget, event):
    if event.keyval == Gdk.KEY_Return:
        emit("gui_ctrl" if event.state & Gdk.ModifierType.CONTROL_MASK else "gui")
        os.write(master, b"\n")
        return True
    return False


def ready():
    emit("gui_ready:" + str(window.get_window().get_xid()))
    return False


entry.connect("key-press-event", key_pressed)
window.connect("destroy", Gtk.main_quit)
window.show_all()
entry.grab_focus()
GLib.idle_add(ready)
Gtk.main()
os.kill(child, signal.SIGTERM)
os.waitpid(child, 0)
