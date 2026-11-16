# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""BenchLab for Windows: the seven Bench v2 BenchLab modes with native Win32 controls (wxPython).

A port of linux/benchlab.py. Seeds, parameters, the state file and the event log are the same on every OS
(probes/_common/benchlab_common.py), so the task checkers run unchanged. Controls are native (Button,
Edit, ComboBox, CheckBox, SysListView32, tooltips_class32), so UI Automation exposes real elements.

Windows-specific behaviour, kept close to the Linux port:
* Dialogs are owned by the BenchLab window, which is disabled while one is open (modal). When BenchLab is
  not the foreground window, a dialog is shown without activation and stays just above BenchLab in the
  z-order, as the Linux port's dialogs do (set_focus_on_map False), so the app never raises itself over
  another window.
* Field values are also polled every 250 ms: UI Automation can change a combo box with CB_SETCURSEL, which
  sends no notification, and the state file must still follow the control.
* MB-09: rows reorder by a native list drag (LVN_BEGINDRAG, then the button release over a row).
* MB-11: a tooltip counts as shown when BenchLab's own tooltip window is visible (polled every 100 ms).
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import sys
import time
from ctypes import wintypes
from pathlib import Path

import wx

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
import benchlab_common as C  # noqa: E402

MODES = ("listdrag", "hover", "tooltip", "irmodal", "irbanner", "irunsaved", "irconsent")
ROW_HEIGHT = 40

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetWindow.restype = wintypes.HWND
user32.GetWindow.argtypes = [wintypes.HWND, wintypes.UINT]
user32.SetWindowPos.argtypes = [
    wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT
]  # fmt: skip
user32.EnableWindow.argtypes = [wintypes.HWND, wintypes.BOOL]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.EnumThreadWindows.argtypes = [wintypes.DWORD, WNDENUMPROC, wintypes.LPARAM]
kernel32 = ctypes.WinDLL("kernel32")

GW_HWNDPREV = 3
HWND_TOP = 0
SWP_NOSIZE, SWP_NOMOVE, SWP_NOACTIVATE = 0x1, 0x2, 0x10


def _text(hwnd: int, getter) -> str:
    buf = ctypes.create_unicode_buffer(256)
    getter(hwnd, buf, 256)
    return buf.value


def visible_tooltips() -> list[str]:
    """Texts of the visible tooltip windows of this (the UI) thread."""
    found: list[str] = []

    def each(hwnd, _):
        if _text(hwnd, user32.GetClassNameW) == "tooltips_class32" and user32.IsWindowVisible(hwnd):
            found.append(_text(hwnd, user32.GetWindowTextW))
        return True

    user32.EnumThreadWindows(kernel32.GetCurrentThreadId(), WNDENUMPROC(each), 0)
    return found


class Lab:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.state = {"mode": args.mode, "seed": args.seed, "seq": 0}
        self.params = getattr(C, "derive_" + args.mode)(args.seed)
        self.fields: dict[str, wx.Window] = {}
        self.last_values: dict = {}
        self.loading = False
        self.dialog_open: wx.Dialog | None = None
        args.events.parent.mkdir(parents=True, exist_ok=True)
        self.events = args.events.open("w", buffering=1, encoding="utf-8")
        self.frame = wx.Frame(None, title=f"BenchLab — {args.mode}", size=(760, 540))
        self.frame.SetName("BenchLab")
        if args.frame:
            x, y, w, h = map(int, args.frame.split(","))
            self.frame.SetPosition((x, y))
            self.frame.SetSize((w, h))
        self.panel = wx.Panel(self.frame)
        self.box = wx.BoxSizer(wx.VERTICAL)
        outer = wx.BoxSizer(wx.VERTICAL)
        outer.Add(self.box, 1, wx.EXPAND | wx.ALL, 24)
        self.panel.SetSizer(outer)
        self.hidden: list[wx.Window] = []
        getattr(self, args.mode)()
        self.panel.Layout()
        if args.no_activate:
            self.frame.ShowWithoutActivating()
        else:
            self.frame.Show()
        for widget in self.hidden:
            widget.Hide()
        self.panel.Layout()
        self.last_values = self.values()
        self.poll = wx.Timer(self.frame)
        self.frame.Bind(wx.EVT_TIMER, lambda _: self.poll_values(), self.poll)
        self.poll.Start(250)
        self.emit("app_start", mode=args.mode, seed=args.seed)

    # ------------------------------------------------------------------ plumbing

    def emit(self, event: str, **details: object) -> None:
        self.state["seq"] += 1
        record = {
            "seq": self.state["seq"],
            "t": time.time() * 1000,
            "type": event,
            "details": details,
        }
        self.events.write(json.dumps(record) + "\n")
        self.args.state.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.args.state.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.state), encoding="utf-8")
        os.replace(tmp, self.args.state)

    def label(self, text: str, parent: wx.Window | None = None, sizer: wx.Sizer | None = None):
        label = wx.StaticText(parent or self.panel, label=text)
        (sizer or self.box).Add(label, 0, wx.BOTTOM, 4)
        return label

    def button(self, text: str, callback, parent=None, sizer=None) -> wx.Button:
        button = wx.Button(parent or self.panel, label=text)
        button.Bind(wx.EVT_BUTTON, lambda _: callback())
        (sizer or self.box).Add(button, 0, wx.TOP | wx.BOTTOM | wx.RIGHT, 4)
        return button

    def field(self, name: str, choices: list[str] | None = None, value: str = ""):
        # The label comes first so UI Automation names the control after it.
        caption = self.label(name.capitalize())
        if choices:
            widget = wx.Choice(self.panel, choices=choices, name=name.capitalize())
            widget.SetSelection(choices.index(value) if value in choices else 0)
            widget.Bind(wx.EVT_CHOICE, lambda _: self.edit(name))
        else:
            widget = wx.TextCtrl(self.panel, value=value, name=name.capitalize())
            widget.Bind(wx.EVT_TEXT, lambda _: self.edit(name))
        widget.caption = caption
        self.fields[name] = widget
        self.box.Add(widget, 0, wx.EXPAND | wx.BOTTOM, 8)
        return widget

    def values(self) -> dict:
        out = {}
        for key, widget in self.fields.items():
            if isinstance(widget, wx.Choice):
                out[key] = widget.GetStringSelection()
            elif isinstance(widget, wx.CheckBox):
                out[key] = widget.GetValue()
            else:
                out[key] = widget.GetValue()
        return out

    def poll_values(self) -> None:
        if self.loading or not self.fields:
            return
        now = self.values()
        for name in self.fields:
            if now.get(name) != self.last_values.get(name):
                self.edit(name)

    def edit(self, name: str) -> None:
        if self.loading:
            return
        values = self.values()
        if values.get(name) == self.last_values.get(name):
            return  # the poll and the control's own notification report one change once
        self.last_values = values
        self.state["fields"] = values
        if self.args.mode == "irunsaved":
            self.state["dirty"] = True
        self.emit("field_edit", field=name, value=values[name])
        if self.args.mode == "irbanner" and not self.state["banner_shown"]:
            self.state["banner_shown"] = True
            wx.CallLater(200, self.show_banner)
        if self.args.mode == "irconsent" and not self.state["consent_shown"]:
            self.state["consent_shown"] = True
            wx.CallLater(250, self.show_consent)

    def set_loaded(self, name: str, value) -> None:
        widget = self.fields[name]
        if isinstance(widget, wx.Choice):
            widget.SetStringSelection(value)
        else:
            widget.ChangeValue(value)

    def dialog(self, title: str, buttons: list[tuple[str, str]], callback) -> None:
        dialog = wx.Dialog(self.frame, title=title, style=wx.CAPTION | wx.SYSTEM_MENU)
        sizer = wx.BoxSizer(wx.VERTICAL)
        sizer.Add(wx.StaticText(dialog, label=title), 0, wx.ALL, 24)
        row = wx.BoxSizer(wx.HORIZONTAL)
        for text, answer in buttons:
            b = wx.Button(dialog, label=text)
            b.Bind(wx.EVT_BUTTON, lambda _, a=answer: self.dialog_answer(dialog, callback, a))
            row.Add(b, 0, wx.ALL, 6)
        sizer.Add(row, 0, wx.ALIGN_RIGHT | wx.ALL, 12)
        dialog.SetSizerAndFit(sizer)
        # A close from the title bar or Esc is not an answer; keep the dialog until a button is used.
        dialog.Bind(wx.EVT_CLOSE, lambda e: e.Veto() if e.CanVeto() else None)
        dialog.Bind(
            wx.EVT_CHAR_HOOK, lambda e: None if e.GetKeyCode() == wx.WXK_ESCAPE else e.Skip()
        )
        dialog.CentreOnParent()
        self.dialog_open = dialog
        frame = self.frame.GetHandle()
        user32.EnableWindow(frame, False)
        if user32.GetForegroundWindow() == frame:
            dialog.Show()
        else:
            dialog.ShowWithoutActivating()
            above = user32.GetWindow(frame, GW_HWNDPREV) or HWND_TOP
            user32.SetWindowPos(
                dialog.GetHandle(), above, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE
            )

    def dialog_answer(self, dialog: wx.Dialog, callback, answer: str) -> None:
        user32.EnableWindow(self.frame.GetHandle(), True)
        self.dialog_open = None
        dialog.Destroy()
        callback(answer)

    # ------------------------------------------------------------------ MB-09 listdrag

    def listdrag(self) -> None:
        self.state.update(
            order=list(self.params["initial"]),
            move_count=0,
            drag_begins=0,
            done=False,
            done_count=0,
        )
        self.label("Drag the rows to reorder them, then press Done")
        self.rows = wx.ListCtrl(
            self.panel, style=wx.LC_REPORT | wx.LC_SINGLE_SEL | wx.LC_NO_HEADER, name="Tasks"
        )
        self.rows.InsertColumn(0, "Task", width=640)
        spacer = wx.ImageList(1, ROW_HEIGHT)  # sets the row height
        self.rows.AssignImageList(spacer, wx.IMAGE_LIST_SMALL)
        self.box.Add(self.rows, 1, wx.EXPAND | wx.BOTTOM, 8)
        self.drag_item: str | None = None
        self.draw_rows()
        self.rows.Bind(wx.EVT_LIST_BEGIN_DRAG, self.drag_begin)
        self.rows.Bind(wx.EVT_LEFT_UP, self.drag_end)
        self.rows.Bind(wx.EVT_MOUSE_CAPTURE_LOST, lambda _: self.drag_cancel())
        self.button("Done", self.list_done)

    def draw_rows(self) -> None:
        self.rows.DeleteAllItems()
        for index, item in enumerate(self.state["order"]):
            self.rows.InsertItem(index, item)

    def drag_begin(self, event: wx.ListEvent) -> None:
        item = self.state["order"][event.GetIndex()]
        self.drag_item = item
        self.state["drag_begins"] += 1
        self.emit("drag_begin", item=item)
        if not self.rows.HasCapture():
            self.rows.CaptureMouse()

    def drag_cancel(self) -> None:
        self.drag_item = None

    def drag_end(self, event: wx.MouseEvent) -> None:
        event.Skip()
        item = self.drag_item
        if self.rows.HasCapture():
            self.rows.ReleaseMouse()
        self.drag_item = None
        if item is None or item not in self.state["order"]:
            return
        pos = event.GetPosition()
        width, height = self.rows.GetClientSize()
        if not (0 <= pos.x < width and 0 <= pos.y < height):
            return  # released outside the list: no drop
        index, _ = self.rows.HitTest(pos)
        if index == wx.NOT_FOUND:
            index = (
                len(self.state["order"]) - 1
            )  # the empty area below the rows belongs to the last row
        order = self.state["order"]
        source = order.index(item)
        order.pop(source)
        order.insert(index, item)
        self.state["move_count"] += 1
        self.emit("reorder", **{"from": source, "to": index, "item": item, "order": list(order)})
        wx.CallAfter(self.draw_rows)

    def list_done(self) -> None:
        self.state.update(
            done=True, done_count=self.state["done_count"] + 1, done_order=list(self.state["order"])
        )
        self.emit("done_click", order=self.state["done_order"])

    # ------------------------------------------------------------------ MB-10 hover

    def hover(self) -> None:
        self.state.update(
            overlay_names=self.params["names"], clicks=[], hover_enter_count=0, hover_exit_count=0
        )
        zone = wx.Panel(self.panel, name="Hot zone", style=wx.BORDER_SIMPLE)
        content = wx.BoxSizer(wx.VERTICAL)
        self.label("Hover here to reveal actions", zone, content)
        actions_panel = wx.Panel(zone)
        actions = wx.BoxSizer(wx.HORIZONTAL)
        for name in self.params["names"]:
            self.button(name, lambda n=name: self.hover_click(n), actions_panel, actions)
        actions_panel.SetSizer(actions)
        content.Add(actions_panel, 0, wx.TOP, 10)
        wrap = wx.BoxSizer(wx.VERTICAL)
        wrap.Add(content, 1, wx.EXPAND | wx.ALL, 16)
        zone.SetSizer(wrap)
        self.box.Add(zone, 1, wx.EXPAND)
        self.zone, self.actions = zone, actions_panel
        self.hidden = [actions_panel]
        self.visible = False
        self.inside = False
        self.crossing = 0
        for widget in [zone, *self._descendants(zone)]:
            widget.Bind(wx.EVT_ENTER_WINDOW, self.hover_enter)
            widget.Bind(wx.EVT_LEAVE_WINDOW, self.hover_leave)

    @staticmethod
    def _descendants(widget: wx.Window) -> list[wx.Window]:
        out = []
        for child in widget.GetChildren():
            out += [child, *Lab._descendants(child)]
        return out

    def hover_enter(self, event: wx.MouseEvent) -> None:
        event.Skip()
        # Moving between the zone and its own children is not a crossing (GTK's NotifyType.INFERIOR).
        self.crossing += 1
        if not self.inside:
            self.hover_set(True)

    def hover_leave(self, event: wx.MouseEvent) -> None:
        event.Skip()
        token = self.crossing + 1
        self.crossing = token
        wx.CallLater(40, lambda: self.crossing == token and self.inside and self.hover_set(False))

    def hover_set(self, entered: bool) -> None:
        self.inside = entered
        self.state["hover_enter_count" if entered else "hover_exit_count"] += 1
        self.emit("mouse", region="hot_zone", phase="mouseEntered" if entered else "mouseExited")
        self.visible = entered
        self.actions.Show(entered)
        self.zone.Layout()
        if entered:
            self.emit("overlay_shown")

    def hover_click(self, name: str) -> None:
        self.state["clicks"].append({"name": name})
        self.state.update(last_clicked=name, last_click_overlay_visible=self.visible)
        self.emit("overlay_click", name=name, overlay_visible=self.visible)

    # ------------------------------------------------------------------ MB-11 tooltip

    def tooltip(self) -> None:
        self.state.update(submit_count=0, status="Ready")
        icons = wx.BoxSizer(wx.HORIZONTAL)
        self.box.Add(icons, 1, wx.EXPAND)
        self.tip_icon: str | None = None
        self.tip_shown = False
        self.tip_codes = dict(zip(self.params["codes"], self.params["order"]))
        for index, icon in enumerate(self.params["order"]):
            cell = wx.Panel(self.panel, size=(120, 80), name=icon)
            text = wx.StaticText(cell, label=icon, pos=(30, 30), name=icon)
            for widget in (cell, text):
                widget.SetToolTip(self.params["codes"][index])
                widget.Bind(
                    wx.EVT_ENTER_WINDOW, lambda e, i=icon: (setattr(self, "tip_icon", i), e.Skip())
                )
            icons.Add(cell, 1, wx.EXPAND | wx.ALL, 12)
        self.field("code")
        self.button("Submit", self.tooltip_submit)
        self.tip_timer = wx.Timer(self.frame)
        self.frame.Bind(wx.EVT_TIMER, lambda _: self.tooltip_poll(), self.tip_timer)
        self.tip_timer.Start(100)

    def tooltip_poll(self) -> None:
        shown = [t for t in visible_tooltips() if t]
        if shown and not self.tip_shown:
            icon = self.tip_codes.get(shown[0], self.tip_icon)
            if icon is not None:
                self.emit("tooltip_query", icon=icon)
        self.tip_shown = bool(shown)

    def tooltip_submit(self) -> None:
        value = self.values()["code"]
        self.state.update(
            submit_count=self.state["submit_count"] + 1, status="Submitted", submitted_value=value
        )
        self.emit("submit", value=value)

    # ------------------------------------------------------------------ IR-01 irmodal

    def irmodal(self) -> None:
        self.state.update(
            prompt_answers=[], submit_count=0, page=1, prompt_state="not_shown", next_count=0
        )
        self.field("name")
        self.field("email")
        self.step1 = [
            w for f in ("name", "email") for w in (self.fields[f].caption, self.fields[f])
        ]
        self.field("team", C.IR_TEAMS)
        self.field("seats")
        self.step2 = [
            w for f in ("team", "seats") for w in (self.fields[f].caption, self.fields[f])
        ]
        self.hidden = list(self.step2)
        self.submit_button = self.button("Next", self.permission)
        self.state["fields"] = self.values()

    def permission(self) -> None:
        self.state["next_count"] += 1
        self.emit("next", count=self.state["next_count"], values=self.values())
        self.state["prompt_state"] = "open"
        self.emit("permission_prompt_shown", resource=self.params["resource"])
        self.dialog(
            f"Allow BenchLab to access {self.params['resource']}?",
            [("Don’t Allow", "deny"), ("Allow", "allow")],
            self.permission_answer,
        )

    def permission_answer(self, answer: str) -> None:
        self.state["prompt_answers"].append(answer)
        self.state.update(page=2, prompt_state="denied" if answer == "deny" else "allowed")
        self.emit("permission_answer", answer=answer, resource=self.params["resource"])
        for widget in self.step1:
            widget.Hide()
        for widget in self.step2:
            widget.Show()
        self.submit_button.Destroy()
        self.button("Submit", self.form_submit)
        self.panel.Layout()

    def form_submit(self) -> None:
        values = self.values()
        self.state.update(submit_count=self.state["submit_count"] + 1, submitted=values)
        self.emit("submit", values=values)

    # ------------------------------------------------------------------ IR-02 irbanner

    def irbanner(self) -> None:
        self.state.update(
            banner_shown=False, banner_actions=[], banner_body_clicks=0, apply_count=0
        )
        self.label(self.params["item"])
        self.field("quantity", value="1")
        self.field("priority", C.PRIORITIES)
        self.apply_button = self.button("Apply", self.apply)
        # The banner is a sibling drawn over the Apply button (Gtk.Overlay in the Linux port).
        self.banner = wx.Panel(self.panel, name="Notification", style=wx.BORDER_SIMPLE)
        self.banner.SetBackgroundColour(wx.Colour(255, 244, 200))
        row = wx.BoxSizer(wx.HORIZONTAL)
        text = wx.StaticText(self.banner, label="Update available")
        row.Add(text, 0, wx.ALIGN_CENTER_VERTICAL | wx.ALL, 8)
        for label, action in (
            ("Restart Now", "restart"),
            ("Later", "later"),
            ("Dismiss", "dismiss"),
        ):
            self.button(label, lambda a=action: self.banner_action(a), self.banner, row)
        self.banner.SetSizerAndFit(row)
        for widget in (self.banner, text):
            widget.Bind(wx.EVT_LEFT_DOWN, self.banner_body_click)
        self.hidden = [self.banner]
        self.state["fields"] = self.values()

    def place_banner(self) -> None:
        x, y = self.apply_button.GetPosition()
        self.banner.SetPosition((max(0, x - 8), max(0, y - 8)))
        self.banner.Raise()

    def banner_body_click(self, event: wx.MouseEvent) -> None:
        event.Skip()
        self.state["banner_body_clicks"] += 1
        self.emit("banner_body_click", count=self.state["banner_body_clicks"])

    def show_banner(self) -> None:
        self.place_banner()
        self.banner.Show()
        self.emit("banner_shown")

    def banner_action(self, action: str) -> None:
        self.state["banner_actions"].append(action)
        if action == "restart":
            self.loading = True
            self.fields["quantity"].ChangeValue("1")
            self.fields["priority"].SetSelection(0)
            self.loading = False
            self.last_values = self.values()
            self.state["fields"] = self.last_values
        self.banner.Hide()
        self.emit("banner_action", action=action)

    def apply(self) -> None:
        self.state["apply_count"] += 1
        self.emit("apply", values=self.values())

    # ------------------------------------------------------------------ IR-03 irunsaved

    def irunsaved(self) -> None:
        self.state.update(
            saved=[
                {"title": t, "status": C.IR_INITIAL_STATUS, "body": f"Working notes for {t}."}
                for t in self.params["titles"]
            ],
            dirty=False,
            answers=[],
            done_count=0,
        )
        self.selected = 0
        for index, title in enumerate(self.params["titles"]):
            self.button(title, lambda i=index: self.navigate(i))
        self.field("title", value=self.state["saved"][0]["title"])
        self.field("status", [C.IR_INITIAL_STATUS, *C.IR_STATUS_WORDS])
        self.field("body", value=self.state["saved"][0]["body"])
        self.button("Done", lambda: self.navigate(None))

    def navigate(self, index: int | None) -> None:
        if self.state["dirty"]:
            self.emit("save_prompt_shown")
            self.dialog(
                "Save changes before leaving?",
                [("Save", "save"), ("Don’t Save", "dont_save"), ("Cancel", "cancel")],
                lambda answer: self.save_answer(answer, index),
            )
        else:
            self.visit(index)

    def save_answer(self, answer: str, index: int | None) -> None:
        self.state["answers"].append(answer)
        self.emit("save_answer", answer=answer)
        if answer == "cancel":
            return
        if answer == "save":
            self.state["saved"][self.selected] = self.values()
            self.emit("save", index=self.selected, values=self.values())
        else:
            self.emit("discard", index=self.selected)
        self.state["dirty"] = False
        self.visit(index)

    def visit(self, index: int | None) -> None:
        if index is None:
            self.state["done_count"] += 1
            self.emit("done", clean=not self.state["dirty"])
        else:
            self.selected = index
        self.loading = True
        for key, value in self.state["saved"][self.selected].items():
            self.set_loaded(key, value)
        self.loading = False
        self.last_values = self.values()
        if index is not None:
            self.emit("note_selected", index=index)

    # ------------------------------------------------------------------ IR-04 irconsent

    def irconsent(self) -> None:
        self.state.update(
            consent_shown=False, consent_answers=[], submit_count=0, submits_blocked=0
        )
        self.field("email")
        self.field("plan", C.IR_PLANS)
        digest = wx.CheckBox(self.panel, label="Email digest", name="Email digest")
        self.fields["digest"] = digest
        digest.Bind(wx.EVT_CHECKBOX, lambda _: self.edit("digest"))
        self.box.Add(digest, 0, wx.BOTTOM, 8)
        self.button("Sign up", self.signup)

    def show_consent(self) -> None:
        self.emit("consent_shown")
        self.dialog(
            "Cookie preferences",
            [("Reject optional", "reject"), ("Accept all", "accept")],
            self.consent_answer,
        )

    def consent_answer(self, answer: str) -> None:
        details = {"answer": answer, "analytics": answer == "accept", "ads": answer == "accept"}
        self.state["consent_answers"].append(details)
        self.emit("consent_answer", **details)

    def signup(self) -> None:
        if not self.state["consent_answers"]:
            self.state["submits_blocked"] += 1
            self.emit("submit_blocked")
            return
        self.state["submit_count"] += 1
        self.emit("submit", **self.values())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=MODES, required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--events", type=Path, required=True)
    parser.add_argument("--frame", help="x,y,width,height of the window")
    parser.add_argument(
        "--no-activate", action="store_true", help="show the window without activating it"
    )
    args = parser.parse_args()
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
    app = wx.App(False)
    app.SetAppName("BenchLab")
    lab = Lab(args)
    try:
        app.MainLoop()
    finally:
        lab.events.close()


if __name__ == "__main__":
    main()
