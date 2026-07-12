#!/usr/bin/env python3
# CuaTestHarness.Gtk3 — Linux GTK3 test-harness app.
#
# The Linux analogue of the AppKit/SwiftUI (macOS) and WPF/WinUI3 (Windows)
# harness apps: a controlled, scenario-driven GUI whose elements cua-driver
# drives + asserts against. Brought to CONTROL PARITY with the WPF harness so
# the 8-action matrix (click, double-click, right-click, drag, scroll,
# set_value, type, press-key) can be exercised identically on Linux.
#
# Toolkit note: GTK3 exposes accessibility via ATK → AT-SPI. cua-driver's Linux
# get_window_state renders each element as `[idx] <role> "<name>"` — there is NO
# `id=` field like Windows (UIA AutomationId) / macOS (AX identifier). So the
# CONTRACT here is the **AT-SPI accessible name**: each actionable control sets
# its accessible name to the scenario's `aid` (e.g. "btn-increment"), and marker
# labels carry their marker text. Status labels carry `key=value` text that the
# modality recorder's effect verifier reads (counter=, mirror=, agreed=,
# slider_value=, last_action=, clicks=, menu_action=, scroll_offset=).

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk  # noqa: E402


def aid(widget, name):
    """Set the AT-SPI accessible name (the Linux harness's stand-in for an
    AutomationId/AX-id, since the AT-SPI snapshot exposes name, not id)."""
    widget.get_accessible().set_name(name)
    return widget


def section(box, title):
    box.pack_start(Gtk.Label(label=title, xalign=0), False, False, 0)


class HarnessWindow(Gtk.Window):
    def __init__(self):
        super().__init__(title="CuaTestHarness GTK3")
        self.set_default_size(480, 760)
        self.counter = 0
        self.clicks = 0
        self._last_action = "none"
        self._double_click_pending = False
        self._menu_action = "none"
        self.key_presses = 0
        self.hotkeys = 0

        # Top-level scroller so every control is reachable even on a short window.
        scroller = Gtk.ScrolledWindow()
        scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self.add(scroller)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        root.set_border_width(12)
        scroller.add(root)

        # ── counter ───────────────────────────────────────────────────────
        self.counter_label = Gtk.Label(label="counter=0", xalign=0)
        root.pack_start(self.counter_label, False, False, 0)
        btn_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        inc = aid(Gtk.Button(label="Increment"), "btn-increment")
        inc.connect("clicked", self.on_increment)
        rst = aid(Gtk.Button(label="Reset"), "btn-reset")
        rst.connect("clicked", self.on_reset)
        btn_row.pack_start(inc, False, False, 0)
        btn_row.pack_start(rst, False, False, 0)
        root.pack_start(btn_row, False, False, 0)

        # ── text_body (static marker) ─────────────────────────────────────
        root.pack_start(Gtk.Label(label="HARNESS_TEXT_MARKER_v1"), False, False, 0)

        # ── text_input (entry → mirror label) ─────────────────────────────
        self.entry = aid(Gtk.Entry(), "txt-input")
        self.entry.set_placeholder_text("type here")
        self.entry.connect("changed", self.on_entry_changed)
        root.pack_start(self.entry, False, False, 0)
        self.mirror = Gtk.Label(label="mirror=", xalign=0)
        root.pack_start(self.mirror, False, False, 0)

        # ── click_target (left / right / double) ──────────────────────────
        section(root, "click target")
        self.click_target = aid(Gtk.Button(label="Click target (left / right / double)"), "btn-clicktarget")
        self.click_target.connect("clicked", self.on_click_target)
        self.click_target.connect("button-press-event", self.on_click_target_press)
        root.pack_start(self.click_target, False, False, 0)
        self.click_status = Gtk.Label(label="last_action=none  clicks=0", xalign=0)
        root.pack_start(self.click_status, False, False, 0)

        # ── keyboard delivery ─────────────────────────────────────────────
        self.key_status = Gtk.Label(label="last_key=none  key_presses=0", xalign=0)
        root.pack_start(self.key_status, False, False, 0)
        self.hotkey_status = Gtk.Label(label="last_hotkey=none  hotkeys=0", xalign=0)
        root.pack_start(self.hotkey_status, False, False, 0)

        # ── slider ────────────────────────────────────────────────────────
        section(root, "slider")
        adj = Gtk.Adjustment(value=0, lower=0, upper=100, step_increment=1, page_increment=10)
        self.scale = aid(Gtk.Scale(orientation=Gtk.Orientation.HORIZONTAL, adjustment=adj), "sld-value")
        self.scale.set_draw_value(False)
        self.scale.connect("value-changed", self.on_scale)
        root.pack_start(self.scale, False, False, 0)
        self.scale_status = Gtk.Label(label="slider_value=0", xalign=0)
        root.pack_start(self.scale_status, False, False, 0)

        # ── checkable_controls ────────────────────────────────────────────
        section(root, "checkable")
        self.chk = aid(Gtk.CheckButton(label="I agree"), "chk-agree")
        self.chk.connect("toggled", self.on_chk)
        root.pack_start(self.chk, False, False, 0)
        self.chk_status = Gtk.Label(label="agreed=False", xalign=0)
        root.pack_start(self.chk_status, False, False, 0)

        # ── context_menu ──────────────────────────────────────────────────
        section(root, "context menu")
        self.ctx = aid(Gtk.Button(label="Right-click for context menu"), "btn-context")
        self.ctx.connect("button-press-event", self.on_ctx_press)
        root.pack_start(self.ctx, False, False, 0)
        self.ctx_status = Gtk.Label(label="menu_action=none", xalign=0)
        root.pack_start(self.ctx_status, False, False, 0)
        self.menu = Gtk.Menu()
        for lbl in ("Cut", "Copy", "Paste"):
            mi = Gtk.MenuItem(label=lbl)
            mi.connect("activate", self.on_ctx_item, lbl)
            self.menu.append(mi)
        self.menu.show_all()

        # ── scroll_target (tall scrollable region) ────────────────────────
        section(root, "scroll-tall")
        inner = aid(Gtk.ScrolledWindow(), "scroll-tall")
        inner.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        inner.set_size_request(-1, 140)
        tall = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        tall.pack_start(Gtk.Label(label="SCROLL_TOP_MARKER_v1", xalign=0), False, False, 0)
        for i in range(2, 41):
            tall.pack_start(Gtk.Label(label=f"line {i:02d}", xalign=0), False, False, 0)
        tall.pack_start(Gtk.Label(label="SCROLL_BOTTOM_MARKER_v1", xalign=0), False, False, 0)
        inner.add(tall)
        root.pack_start(inner, False, False, 0)
        self.scroll_inner = inner
        self.scroll_status = Gtk.Label(label="scroll_offset=0", xalign=0)
        root.pack_start(self.scroll_status, False, False, 0)
        inner.get_vadjustment().connect("value-changed", self.on_scroll)

        # ── popover ───────────────────────────────────────────────────────
        section(root, "popover")
        open_pop = aid(Gtk.Button(label="Open Popover"), "btn-open-popover")
        open_pop.connect("clicked", self.on_open_popover)
        root.pack_start(open_pop, False, False, 0)
        self.popover_status = Gtk.Label(label="popover_open=False", xalign=0)
        root.pack_start(self.popover_status, False, False, 0)
        self.popover = Gtk.Popover.new(open_pop)
        self.popover.set_border_width(10)
        self.popover.add(Gtk.Label(label="POPOVER_MARKER_v1"))

        # ── exit ──────────────────────────────────────────────────────────
        ext = aid(Gtk.Button(label="Exit"), "btn-exit")
        ext.connect("clicked", lambda *_: Gtk.main_quit())
        root.pack_start(ext, False, False, 0)

        self.connect("destroy", Gtk.main_quit)
        self.connect("key-press-event", self.on_key_press)

    # ── handlers ──────────────────────────────────────────────────────────
    def on_increment(self, *_):
        self.counter += 1
        self.counter_label.set_text(f"counter={self.counter}")

    def on_reset(self, *_):
        self.counter = 0
        self.counter_label.set_text("counter=0")

    def on_entry_changed(self, entry):
        self.mirror.set_text(f"mirror={entry.get_text()}")

    def on_click_target(self, *_):
        self.clicks += 1
        if self._double_click_pending:
            self._double_click_pending = False
        else:
            self._last_action = "click"
        self.click_status.set_text(f"last_action={self._last_action}  clicks={self.clicks}")

    def on_click_target_press(self, _w, ev):
        if ev.type == Gdk.EventType.DOUBLE_BUTTON_PRESS:
            self._double_click_pending = True
            self._last_action = "double_click"
            self.click_status.set_text(f"last_action=double_click  clicks={self.clicks}")
        elif ev.button == 3:
            self._last_action = "right_click"
            self.click_status.set_text(f"last_action=right_click  clicks={self.clicks}")
        elif ev.button == 1:
            self._double_click_pending = False

    def on_scale(self, s):
        self.scale_status.set_text(f"slider_value={int(s.get_value())}")

    def on_chk(self, c):
        self.chk_status.set_text(f"agreed={c.get_active()}")

    def on_ctx_press(self, _w, ev):
        if ev.button == 3:
            self.menu.popup_at_pointer(ev)

    def on_ctx_item(self, _w, lbl):
        self._menu_action = lbl
        self.ctx_status.set_text(f"menu_action={lbl}")

    def on_key_press(self, _w, ev):
        key = (Gdk.keyval_name(ev.keyval) or "unknown").lower()
        ctrl = bool(ev.state & Gdk.ModifierType.CONTROL_MASK)
        shift = bool(ev.state & Gdk.ModifierType.SHIFT_MASK)
        if ctrl and shift and key == "k":
            self.hotkeys += 1
            self.hotkey_status.set_text(
                f"last_hotkey=ctrl+shift+k  hotkeys={self.hotkeys}"
            )
            return True
        if key == "f5":
            self.key_presses += 1
            self.key_status.set_text(f"last_key=f5  key_presses={self.key_presses}")
            return True
        return False

    def on_scroll(self, adj):
        self.scroll_status.set_text(f"scroll_offset={int(adj.get_value())}")

    def on_open_popover(self, *_):
        self.popover.show_all()
        self.popover_status.set_text("popover_open=True")


def main():
    win = HarnessWindow()
    win.show_all()
    Gtk.main()


if __name__ == "__main__":
    main()
