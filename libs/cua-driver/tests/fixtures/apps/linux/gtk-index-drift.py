#!/usr/bin/python3
"""Small real GTK app: enabling one earlier control must not retarget OK."""
import gi

gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib

GLib.set_prgname('cua-index-lab')
GLib.set_application_name('Cua Index Lab')
window = Gtk.Window(title='Cua Index Lab')
window.set_default_size(440, 320)
window.set_border_width(20)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
window.add(box)
action = Gtk.Label(label='Action: none')
mode = Gtk.Label(label='Earlier: disabled')

def button(label, callback):
    widget = Gtk.Button(label=label)
    widget.connect('clicked', callback)
    box.pack_start(widget, False, False, 0)
    return widget

earlier = button('Earlier control', lambda _: action.set_text('Action: EARLIER'))
earlier.set_sensitive(False)
button('Cancel', lambda _: action.set_text('Action: CANCEL'))
ok = button('OK', lambda _: action.set_text('Action: OK'))

def enable(_):
    earlier.set_sensitive(True)
    mode.set_text('Earlier: enabled')

button('Enable earlier control', enable)
button('Remove OK', lambda _: ok.destroy())
box.pack_start(mode, False, False, 0)
box.pack_start(action, False, False, 0)
window.connect('destroy', Gtk.main_quit)
window.show_all()
Gtk.main()
