#!/usr/bin/env python3
# CuaTestHarness GTK3 (recording edition) — rich controls so the Windows/WPF
# 8-action matrix maps onto Linux. Each actionable control sets its AT-SPI
# accessible name. The harness also (a) exports each named widget's screen rect
# (cua-driver's Linux AT-SPI snapshot has no sliders/scrollers or frames) and
# (b) writes its live state to a file so the recorder can VERIFY each action's
# effect (slider moved? checkbox toggled? text changed?), not just focus-steal.
import json
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib

STATE_FILE = "/tmp/cua-lin-state.json"
GEOM_FILE = "/tmp/cua-lin-geom.json"


def aid(widget, name):
    widget.get_accessible().set_name(name)
    return widget


class Harness(Gtk.Window):
    def __init__(self):
        super().__init__(title="CuaTestHarness GTK3")
        self.set_default_size(536, 740)
        self.set_resizable(False)
        self.clicks = 0
        self._last_action = "none"
        self._ctx = "none"

        scroller = Gtk.ScrolledWindow()
        scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self.add(scroller)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        root.set_border_width(14)
        scroller.add(root)

        def section(title):
            root.pack_start(Gtk.Label(label=title, xalign=0), False, False, 0)

        section("text input")
        self.entry = aid(Gtk.Entry(), "txt-input")
        self.entry.set_placeholder_text("type here")
        self.entry.connect("changed", self._on_entry)
        root.pack_start(self.entry, False, False, 0)
        self.mirror = Gtk.Label(label="mirror=", xalign=0)
        root.pack_start(self.mirror, False, False, 0)

        section("click target")
        self.btn = aid(Gtk.Button(label="Click target (left / right / double)"), "btn-clicktarget")
        self.btn.connect("clicked", self._on_click)
        self.btn.connect("button-press-event", self._on_btn_press)
        root.pack_start(self.btn, False, False, 0)
        self.btn_status = Gtk.Label(label="last_action=none  clicks=0", xalign=0)
        root.pack_start(self.btn_status, False, False, 0)

        section("slider")
        adj = Gtk.Adjustment(value=0, lower=0, upper=100, step_increment=1, page_increment=10)
        self.scale = aid(Gtk.Scale(orientation=Gtk.Orientation.HORIZONTAL, adjustment=adj), "sld-value")
        self.scale.set_draw_value(False)
        self.scale.connect("value-changed", self._on_scale)
        root.pack_start(self.scale, False, False, 0)
        self.scale_status = Gtk.Label(label="slider_value=0", xalign=0)
        root.pack_start(self.scale_status, False, False, 0)

        section("checkable")
        self.chk = aid(Gtk.CheckButton(label="I agree"), "chk-agree")
        self.chk.connect("toggled", self._on_chk)
        root.pack_start(self.chk, False, False, 0)
        self.chk_status = Gtk.Label(label="agreed=False", xalign=0)
        root.pack_start(self.chk_status, False, False, 0)

        section("context menu")
        self.ctx = aid(Gtk.Button(label="Right-click for context menu"), "btn-context")
        self.ctx.connect("button-press-event", self._on_ctx_press)
        root.pack_start(self.ctx, False, False, 0)
        self.ctx_status = Gtk.Label(label="ctx=none", xalign=0)
        root.pack_start(self.ctx_status, False, False, 0)
        self.menu = Gtk.Menu()
        for lbl in ("Cut", "Copy", "Paste"):
            mi = Gtk.MenuItem(label=lbl)
            mi.connect("activate", self._on_ctx_item, lbl)
            self.menu.append(mi)
        self.menu.show_all()

        section("scroll-tall")
        inner = aid(Gtk.ScrolledWindow(), "scroll-tall")
        inner.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        inner.set_size_request(-1, 150)
        tall = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        for i in range(1, 61):
            tall.pack_start(Gtk.Label(label=f"row {i}", xalign=0), False, False, 0)
        inner.add(tall)
        root.pack_start(inner, False, False, 0)
        self.scroll_inner = inner
        inner.get_vadjustment().connect("value-changed", lambda *_: self._save_state())

        self.named = {"chk":self.chk,"btn":self.btn,"ctx":self.ctx,
                      "sld":self.scale,"scr":self.scroll_inner,"txt":self.entry}
        self.connect("destroy", Gtk.main_quit)
        self._save_state()

    # ---- state file (for the recorder's per-action effect verifier) ----
    def _save_state(self):
        try: sc = int(self.scroll_inner.get_vadjustment().get_value())
        except Exception: sc = 0
        st = {"slider": int(self.scale.get_value()), "agreed": self.chk.get_active(),
              "mirror": self.entry.get_text(), "last_action": self._last_action,
              "ctx": self._ctx, "clicks": self.clicks, "scroll": sc}
        try: json.dump(st, open(STATE_FILE, "w"))
        except Exception: pass

    def export_geom(self):
        out = {}
        for k, wdg in self.named.items():
            try:
                res = wdg.translate_coordinates(self, 0, 0)
                if not res: continue
                lx, ly = res
                ok, gx, gy = self.get_window().get_origin()
                a = wdg.get_allocation()
                out[k] = {"x":gx+lx, "y":gy+ly, "w":a.width, "h":a.height}
            except Exception:
                pass
        json.dump(out, open(GEOM_FILE, "w"))
        self._save_state()
        return False

    def _on_entry(self, e):
        self.mirror.set_text(f"mirror={e.get_text()}"); self._save_state()

    def _on_click(self, *_):
        self.clicks += 1; self._last_action = "click"
        self.btn_status.set_text(f"last_action=click  clicks={self.clicks}"); self._save_state()

    def _on_btn_press(self, _w, ev):
        if ev.type == Gdk.EventType.DOUBLE_BUTTON_PRESS:
            self._last_action = "double_click"
            self.btn_status.set_text(f"last_action=double_click  clicks={self.clicks}"); self._save_state()

    def _on_scale(self, s):
        self.scale_status.set_text(f"slider_value={int(s.get_value())}"); self._save_state()

    def _on_chk(self, c):
        self.chk_status.set_text(f"agreed={c.get_active()}"); self._save_state()

    def _on_ctx_press(self, _w, ev):
        if ev.button == 3:
            self._ctx = "opened"; self.ctx_status.set_text("ctx=opened")
            self.menu.popup_at_pointer(ev); self._save_state()

    def _on_ctx_item(self, _w, lbl):
        self._ctx = lbl; self.ctx_status.set_text(f"ctx={lbl}"); self._save_state()


if __name__ == "__main__":
    import sys, traceback
    try:
        w = Harness()
        print("INIT_DONE", flush=True)
        w.move(0, 0)
        w.show_all()
        print("SHOWN", flush=True)
        GLib.timeout_add(1200, w.export_geom)
    except Exception:
        traceback.print_exc(); sys.stdout.flush(); raise
    Gtk.main()
