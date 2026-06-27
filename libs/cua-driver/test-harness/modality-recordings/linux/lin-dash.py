#!/usr/bin/env python3
# Native dashboard panel: a GTK window hosting a WebKit2GTK webview that loads
# the SAME dashboard.html served over loopback (exact parity with the Windows
# chrome --app panel, but no browser process — works headless under Xvfb).
import sys
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, WebKit2

URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8146/"
X = int(sys.argv[2]) if len(sys.argv) > 2 else 544
Y = int(sys.argv[3]) if len(sys.argv) > 3 else 0
W = int(sys.argv[4]) if len(sys.argv) > 4 else 480
H = int(sys.argv[5]) if len(sys.argv) > 5 else 740

win = Gtk.Window(title="cua-driver-panel")
win.set_default_size(W, H)
win.set_resizable(False)
view = WebKit2.WebView()
win.add(view)
view.load_uri(URL)
win.connect("destroy", Gtk.main_quit)
win.show_all()
win.move(X, Y)
Gtk.main()
