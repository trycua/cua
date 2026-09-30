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

import json
import os

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
        # Keep the nested scroll viewport visible on the canonical 1024x768
        # desktop; the outer scroller still exposes controls below it.
        self.set_default_size(560, 720)
        self.counter = 0
        self.clicks = 0
        self._last_action = "none"
        self._double_click_pending = False
        self._menu_action = "none"
        self.key_presses = 0
        self.hotkeys = 0
        self._drag_modifier_seen = False
        self.drag_events = 0

        # Top-level scroller so every control is reachable even on a short window.
        scroller = Gtk.ScrolledWindow()
        scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self.add(scroller)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        root.set_border_width(12)
        scroller.add(root)

        # Native application menu used by invoke_menu E2E. The three-level
        # hierarchy exercises live path re-resolution after each submenu opens.
        menubar = Gtk.MenuBar()
        window_item = Gtk.MenuItem(label="Window")
        window_menu = Gtk.Menu()
        arrange_item = Gtk.MenuItem(label="Arrange")
        arrange_menu = Gtk.Menu()
        left_item = Gtk.MenuItem(label="Left")
        left_item.connect("activate", self.on_native_menu_left)
        arrange_menu.append(left_item)
        arrange_item.set_submenu(arrange_menu)
        window_menu.append(arrange_item)
        window_item.set_submenu(window_menu)
        menubar.append(window_item)
        root.pack_start(menubar, False, False, 0)

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
        self.click_target.add_events(
            Gdk.EventMask.POINTER_MOTION_MASK | Gdk.EventMask.BUTTON_RELEASE_MASK
        )
        self.click_target.connect("motion-notify-event", self.on_drag_motion)
        self.click_target.connect("button-release-event", self.on_drag_release)
        root.pack_start(self.click_target, False, False, 0)
        self.click_status = Gtk.Label(label="last_action=none  clicks=0", xalign=0)
        root.pack_start(self.click_status, False, False, 0)
        self.drag_status = Gtk.Label(label="drag_modifier=none  drag_events=0", xalign=0)
        root.pack_start(self.drag_status, False, False, 0)

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

        # ── multi-selection ───────────────────────────────────────────────
        section(root, "multi selection")
        self.selection = Gtk.ListBox()
        self.selection.set_selection_mode(Gtk.SelectionMode.MULTIPLE)
        for value in ("alpha", "beta", "gamma"):
            row = aid(Gtk.ListBoxRow(), f"selection-{value}")
            row.set_activatable(True)
            row.set_selectable(True)
            row.set_can_focus(True)
            row.add(Gtk.Label(label=value, xalign=0))
            self.selection.add(row)
        self.selection.connect("selected-rows-changed", self.on_selection_changed)
        root.pack_start(self.selection, False, False, 0)
        self.selection_status = Gtk.Label(label="selection=none", xalign=0)
        root.pack_start(self.selection_status, False, False, 0)

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
        scroll_target = aid(Gtk.Button(label="Scroll viewport target"), "scroll-tall-viewport")
        scroll_target.set_can_focus(False)
        tall.pack_start(scroll_target, False, False, 0)
        tall.pack_start(Gtk.Label(label="SCROLL_TOP_MARKER_v1", xalign=0), False, False, 0)
        for i in range(2, 41):
            tall.pack_start(Gtk.Label(label=f"line {i:02d}", xalign=0), False, False, 0)
        tall.pack_start(Gtk.Label(label="SCROLL_BOTTOM_MARKER_v1", xalign=0), False, False, 0)
        inner.add(tall)
        aid(inner.get_vscrollbar(), "scroll-tall-vertical")
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
        if ev.button == 1:
            self._drag_modifier_seen = bool(
                ev.state & Gdk.ModifierType.CONTROL_MASK
            )
        if ev.type == Gdk.EventType.DOUBLE_BUTTON_PRESS:
            self._double_click_pending = True
            self._last_action = "double_click"
            self.click_status.set_text(f"last_action=double_click  clicks={self.clicks}")
        elif ev.button == 3:
            self._last_action = "right_click"
            self.click_status.set_text(f"last_action=right_click  clicks={self.clicks}")
        elif ev.button == 1:
            self._double_click_pending = False

    def on_drag_motion(self, _w, ev):
        if ev.state & Gdk.ModifierType.BUTTON1_MASK and ev.state & Gdk.ModifierType.CONTROL_MASK:
            self._drag_modifier_seen = True
        return False

    def on_drag_release(self, _w, ev):
        if ev.button == 1:
            self.drag_events += 1
            modifier = "ctrl" if self._drag_modifier_seen else "none"
            self.drag_status.set_text(
                f"drag_modifier={modifier}  drag_events={self.drag_events}"
            )
        return False

    def on_scale(self, s):
        self.scale_status.set_text(f"slider_value={int(s.get_value())}")

    def on_chk(self, c):
        self.chk_status.set_text(f"agreed={c.get_active()}")

    def on_selection_changed(self, list_box):
        values = sorted(
            row.get_child().get_text() for row in list_box.get_selected_rows()
        )
        self.selection_status.set_text(
            "selection=" + (",".join(values) if values else "none")
        )

    def on_ctx_press(self, _w, ev):
        if ev.button == 3:
            self.menu.popup_at_pointer(ev)

    def on_ctx_item(self, _w, lbl):
        self._menu_action = lbl
        self.ctx_status.set_text(f"menu_action={lbl}")

    def on_native_menu_left(self, _w):
        self._menu_action = "window_arrange_left"
        self.ctx_status.set_text("menu_action=window_arrange_left")

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
        if key == "f5" and not ctrl and not shift:
            self.key_presses += 1
            self.key_status.set_text(f"last_key=f5  key_presses={self.key_presses}")
            return True
        return False

    def on_scroll(self, adj):
        self.scroll_status.set_text(f"scroll_offset={int(adj.get_value())}")

    def on_open_popover(self, *_):
        self.popover.show_all()
        self.popover_status.set_text("popover_open=True")


TASK_STATE_ENV = "CUA_GTK3_TASK_STATE"
TASK_STATE_SCHEMA = "cua.gtk3_task_state_v1"
TASK_DENSITY_ENV = "CUA_GTK3_TASK_DENSITY"

# Opt-in distractor controls for measuring jev-use accuracy at larger
# candidate sets (#4312). The labels are benign (no risky-action phrase) and
# match the AppKit harness, so candidate IDs agree across platforms. Some are
# unrelated to every task; some are close to a task control ("Save draft",
# "Increase font size", "Note title", "Large icons"). Density 12 uses the first
# entries of each list; density 24 uses all of them.
DISTRACTOR_BUTTONS = (
    "New folder", "Refresh", "Undo", "Redo", "Zoom in", "Zoom out", "Save draft",
    "Increase font size", "Copy link", "Duplicate", "Rename", "Print preview", "Export PDF",
    "Import", "Bold", "Italic", "Underline", "Align left", "Align center", "Align right",
    "Insert table", "Insert image", "Spell check", "Word count", "Show sidebar", "Help",
)
DISTRACTOR_CHECKBOXES = (
    "Show ruler", "Show previews", "Word wrap", "Auto-save", "Line numbers", "Dark mode",
    "Show hidden files", "Sync on startup", "Compact layout", "Show status bar",
    "Remember window size", "Check spelling as you type",
)
DISTRACTOR_RADIO_GROUPS = (
    ("Light", "Dark", "System"),
    ("List", "Grid", "Columns"),
    ("Name", "Date", "Kind"),
    ("Small icons", "Medium icons", "Large icons"),
)
DISTRACTOR_FIELDS = ("Search", "Note title")
# density -> (buttons, checkboxes, radio groups, text fields)
DENSITY_COUNTS = {12: (8, 3, 1, 1), 24: (26, 12, 4, 2)}


def task_density():
    """The opt-in distractor density: None, 12, or 24. Anything else is an error."""
    raw = os.environ.get(TASK_DENSITY_ENV, "").strip()
    if not raw:
        return None
    if raw not in ("12", "24"):
        raise SystemExit(f"{TASK_DENSITY_ENV} must be 12 or 24, not {raw!r}")
    return int(raw)


class TaskWindow(Gtk.Window):
    """Opt-in jev-use task window (RFC #4268), shown instead of HarnessWindow
    when CUA_GTK3_TASK_STATE=<path> is set.

    It carries the same labeled controls as the AppKit and WPF task modes
    (Increment, Reset, I agree, Small/Medium/Large, Note, Save note, Exit) in a
    small window where every control is on screen. CUA_GTK3_TASK_DENSITY=12 or
    24 also adds benign distractor controls before them (#4312). Accessible names are the
    visible labels here, not the aid-style names of HarnessWindow, so candidate
    IDs match across platforms. Every change atomically rewrites an app-owned
    JSON state file; the jev-use task oracle reads that file and never depends
    on Cua Driver output. Ordinary launches never create this window.
    """

    def __init__(self, state_path, density=None):
        super().__init__(title="CuaTestHarness GTK3 Tasks")
        self.set_default_size(480, 320)
        self.state_path = state_path
        self.density = density
        self.distractor_actions = 0
        self.counter = 0
        self.agreed = False
        self.size = "none"
        self.saved_note = None
        self.sequence = 0

        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        root.set_border_width(16)
        self.add(root)
        if density is not None:
            # Before the task controls, as a toolbar and sidebar precede the
            # content in a typical document app's depth-first order.
            self.add_distractors(root, density)
        self.counter_label = Gtk.Label(label="counter=0", xalign=0)
        root.pack_start(self.counter_label, False, False, 0)

        counter_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        counter_row.pack_start(self.button("Increment", self.on_increment), False, False, 0)
        counter_row.pack_start(self.button("Reset", self.on_reset), False, False, 0)
        root.pack_start(counter_row, False, False, 0)

        agree = Gtk.CheckButton(label="I agree")
        agree.connect("toggled", self.on_agree)
        root.pack_start(agree, False, False, 0)

        # A GTK radio group always has one active member. A hidden "none"
        # member keeps Small/Medium/Large all unselected at launch.
        size_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        self.no_size = Gtk.RadioButton(label="none")
        for title in ("Small", "Medium", "Large"):
            radio = Gtk.RadioButton.new_with_label_from_widget(self.no_size, title)
            radio.connect("toggled", self.on_size, title.lower())
            size_row.pack_start(radio, False, False, 0)
        root.pack_start(size_row, False, False, 0)

        note_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        # An accessible name and no placeholder text, like the other harnesses.
        self.note = aid(Gtk.Entry(), "Note")
        self.note.set_width_chars(28)
        note_row.pack_start(self.note, False, False, 0)
        note_row.pack_start(self.button("Save note", self.on_save_note), False, False, 0)
        root.pack_start(note_row, False, False, 0)

        root.pack_start(self.button("Exit", lambda *_: Gtk.main_quit()), False, False, 0)
        self.connect("destroy", Gtk.main_quit)
        self.publish()

    def add_distractors(self, root, density):
        buttons, checkboxes, groups, fields = DENSITY_COUNTS[density]
        grid = Gtk.Grid(column_spacing=6, row_spacing=6)
        for index, label in enumerate(DISTRACTOR_BUTTONS[:buttons]):
            grid.attach(self.button(label, self.on_distractor), index % 7, index // 7, 1, 1)
        root.pack_start(grid, False, False, 0)
        field_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        for label in DISTRACTOR_FIELDS[:fields]:
            # An accessible name and no placeholder, like the Note field.
            entry = aid(Gtk.Entry(), label)
            entry.set_width_chars(18)
            entry.connect("changed", self.on_distractor)
            field_row.pack_start(entry, False, False, 0)
        root.pack_start(field_row, False, False, 0)
        checks = Gtk.Grid(column_spacing=12, row_spacing=4)
        for index, label in enumerate(DISTRACTOR_CHECKBOXES[:checkboxes]):
            check = Gtk.CheckButton(label=label)
            check.connect("toggled", self.on_distractor)
            checks.attach(check, index % 4, index // 4, 1, 1)
        root.pack_start(checks, False, False, 0)
        for options in DISTRACTOR_RADIO_GROUPS[:groups]:
            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
            # A hidden member keeps every visible option unselected at launch.
            hidden = Gtk.RadioButton(label="none")
            row.hidden_member = hidden
            for label in options:
                radio = Gtk.RadioButton.new_with_label_from_widget(hidden, label)
                radio.connect("toggled", self.on_distractor_radio)
                row.pack_start(radio, False, False, 0)
            root.pack_start(row, False, False, 0)
        root.pack_start(Gtk.Separator(), False, False, 4)

    def on_distractor(self, *_):
        self.distractor_actions += 1
        self.publish()

    def on_distractor_radio(self, radio):
        if radio.get_active():
            self.on_distractor()

    @staticmethod
    def button(label, handler):
        widget = Gtk.Button(label=label)
        widget.set_halign(Gtk.Align.START)
        widget.connect("clicked", handler)
        return widget

    def on_increment(self, *_):
        self.counter += 1
        self.publish()

    def on_reset(self, *_):
        self.counter = 0
        self.publish()

    def on_agree(self, check):
        self.agreed = check.get_active()
        self.publish()

    def on_size(self, radio, value):
        if radio.get_active():
            self.size = value
            self.publish()

    def on_save_note(self, *_):
        self.saved_note = self.note.get_text()
        self.publish()

    def publish(self):
        self.counter_label.set_text(f"counter={self.counter}")
        self.sequence += 1
        state = {
            "schema": TASK_STATE_SCHEMA,
            "pid": os.getpid(),
            "seq": self.sequence,
            "counter": self.counter,
            "agreed": self.agreed,
            "size": self.size,
            "note_saved": self.saved_note,
        }
        if self.density is not None:
            state["density"] = self.density
            state["distractor_actions"] = self.distractor_actions
        temporary = f"{self.state_path}.{os.getpid()}.tmp"
        with open(temporary, "w", encoding="utf-8") as stream:
            json.dump(state, stream, sort_keys=True)
        os.replace(temporary, self.state_path)


def main():
    task_state = os.environ.get(TASK_STATE_ENV, "").strip()
    win = TaskWindow(task_state, task_density()) if task_state else HarnessWindow()
    win.show_all()
    Gtk.main()


if __name__ == "__main__":
    main()
