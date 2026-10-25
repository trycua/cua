# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

from __future__ import annotations

import argparse
import ctypes
import json
import os
import sys
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
import benchlab_common as C

MODES = ("listdrag", "hover", "tooltip", "irmodal", "irbanner", "irunsaved", "irconsent")


class Lab:
    def __init__(self, args: argparse.Namespace) -> None:
        ctypes.CDLL(None).prctl(15, b"BenchLab", 0, 0, 0)
        GLib.set_prgname("BenchLab")
        GLib.set_application_name("BenchLab")
        self.args = args
        self.state = {"mode": args.mode, "seed": args.seed, "seq": 0}
        self.params = getattr(C, "derive_" + args.mode)(args.seed)
        self.fields = {}
        self.loading = False
        args.events.parent.mkdir(parents=True, exist_ok=True)
        self.events = args.events.open("w", buffering=1)
        self.window = Gtk.Window(title=f"BenchLab — {args.mode}")
        self.window.set_wmclass("BenchLab", "BenchLab")
        self.window.set_default_size(760, 540)
        self.window.connect("destroy", Gtk.main_quit)
        self.box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12, margin=24)
        self.window.add(self.box)
        getattr(self, args.mode)()
        self.window.show_all()
        for widget in getattr(self, "hidden", []):
            widget.hide()
        self.emit("app_start", mode=args.mode, seed=args.seed)

    def emit(self, event: str, **details: object) -> None:
        self.state["seq"] += 1
        self.events.write(
            json.dumps(
                {
                    "seq": self.state["seq"],
                    "t": time.time() * 1000,
                    "type": event,
                    "details": details,
                }
            )
            + "\n"
        )
        self.args.state.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.args.state.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.state))
        os.replace(tmp, self.args.state)

    def label(self, text: str, box: Gtk.Box | None = None) -> Gtk.Label:
        label = Gtk.Label(label=text)
        (box or self.box).pack_start(label, False, False, 0)
        return label

    def button(self, text: str, callback, box: Gtk.Box | None = None) -> Gtk.Button:
        button = Gtk.Button(label=text)
        button.connect("clicked", lambda *_: callback())
        (box or self.box).pack_start(button, False, False, 0)
        return button

    def field(self, name: str, choices: list[str] | None = None, value: str = ""):
        self.label(name.capitalize())
        if choices:
            widget = Gtk.ComboBoxText()
            for item in choices:
                widget.append_text(item)
            widget.set_active(choices.index(value) if value in choices else 0)
        else:
            widget = Gtk.Entry()
            widget.set_text(value)
        widget.get_accessible().set_name(name.capitalize())
        self.fields[name] = widget
        widget.connect("changed", lambda *_: self.edit(name))
        self.box.pack_start(widget, False, False, 0)
        return widget

    def values(self) -> dict:
        return {
            key: widget.get_active_text()
            if isinstance(widget, Gtk.ComboBoxText)
            else widget.get_active()
            if isinstance(widget, Gtk.CheckButton)
            else widget.get_text()
            for key, widget in self.fields.items()
        }

    def edit(self, name: str) -> None:
        if self.loading:
            return
        values = self.values()
        self.state["fields"] = values
        if self.args.mode == "irunsaved":
            self.state["dirty"] = True
        self.emit("field_edit", field=name, value=values[name])
        if self.args.mode == "irbanner" and not self.state["banner_shown"]:
            self.state["banner_shown"] = True
            GLib.timeout_add(200, self.show_banner)
        if self.args.mode == "irconsent" and not self.state["consent_shown"]:
            self.state["consent_shown"] = True
            GLib.timeout_add(250, self.show_consent)

    def dialog(self, title: str, buttons: list[tuple[str, str]], callback) -> None:
        dialog = Gtk.Dialog(title=title, transient_for=self.window, modal=True)
        dialog.set_focus_on_map(False)
        dialog.get_content_area().add(Gtk.Label(label=title, margin=24))
        for index, (text, _) in enumerate(buttons):
            dialog.add_button(text, index)

        def response(widget, code):
            widget.destroy()
            if 0 <= code < len(buttons):
                callback(buttons[code][1])

        dialog.connect("response", response)
        dialog.show_all()

    def listdrag(self) -> None:
        self.state.update(
            order=list(self.params["initial"]),
            move_count=0,
            drag_begins=0,
            done=False,
            done_count=0,
        )
        self.label("Drag the rows to reorder them, then press Done")
        self.rows = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        self.box.pack_start(self.rows, True, True, 0)
        self.drag_item = None
        self.draw_rows()
        self.button("Done", self.list_done)

    def draw_rows(self) -> None:
        for child in self.rows.get_children():
            child.destroy()
        targets = [Gtk.TargetEntry.new("text/plain", Gtk.TargetFlags.SAME_APP, 0)]
        for index, item in enumerate(self.state["order"]):
            row = Gtk.EventBox()
            row.add(Gtk.Label(label=item, margin=12))
            row.drag_source_set(
                Gdk.ModifierType.BUTTON1_MASK, targets, Gdk.DragAction.COPY | Gdk.DragAction.MOVE
            )
            row.drag_dest_set(
                Gtk.DestDefaults.ALL, targets, Gdk.DragAction.COPY | Gdk.DragAction.MOVE
            )
            row.connect("drag-begin", self.drag_begin, item)
            row.connect(
                "drag-data-get", lambda w, ctx, data, *_: data.set_text(self.drag_item or "", -1)
            )
            row.connect("drag-data-received", self.drop, index)
            self.rows.pack_start(row, True, True, 0)
        self.rows.show_all()

    def drag_begin(self, widget, context, item: str) -> None:
        self.drag_item = item
        self.state["drag_begins"] += 1
        self.emit("drag_begin", item=item)

    def drop(self, widget, context, x, y, data, info, timestamp, index: int) -> None:
        item = data.get_text()
        if item != self.drag_item or item not in self.state["order"]:
            Gtk.drag_finish(context, False, False, timestamp)
            return
        order = self.state["order"]
        source = order.index(item)
        order.pop(source)
        order.insert(index, item)
        self.state["move_count"] += 1
        self.emit("reorder", **{"from": source, "to": index, "item": item, "order": list(order)})
        Gtk.drag_finish(context, True, False, timestamp)
        self.drag_item = None
        GLib.idle_add(self.draw_rows)

    def list_done(self) -> None:
        self.state.update(
            done=True, done_count=self.state["done_count"] + 1, done_order=list(self.state["order"])
        )
        self.emit("done_click", order=self.state["done_order"])

    def hover(self) -> None:
        self.state.update(
            overlay_names=self.params["names"], clicks=[], hover_enter_count=0, hover_exit_count=0
        )
        zone = Gtk.EventBox()
        zone.add_events(Gdk.EventMask.ENTER_NOTIFY_MASK | Gdk.EventMask.LEAVE_NOTIFY_MASK)
        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        zone.add(content)
        self.label("Hover here to reveal actions", content)
        actions = Gtk.Box(spacing=10)
        content.pack_start(actions, False, False, 0)
        for name in self.params["names"]:
            self.button(name, lambda n=name: self.hover_click(n), actions)
        self.hidden = [actions]
        self.visible = False

        def crossing(widget, event, entered):
            if event.detail == Gdk.NotifyType.INFERIOR:
                return False
            self.state["hover_enter_count" if entered else "hover_exit_count"] += 1
            self.emit(
                "mouse", region="hot_zone", phase="mouseEntered" if entered else "mouseExited"
            )
            self.visible = entered
            actions.set_visible(entered)
            if entered:
                self.emit("overlay_shown")
            return False

        zone.connect("enter-notify-event", crossing, True)
        zone.connect("leave-notify-event", crossing, False)
        self.box.pack_start(zone, True, True, 0)

    def hover_click(self, name: str) -> None:
        self.state["clicks"].append({"name": name})
        self.state.update(last_clicked=name, last_click_overlay_visible=self.visible)
        self.emit("overlay_click", name=name, overlay_visible=self.visible)

    def tooltip(self) -> None:
        self.state.update(submit_count=0, status="Ready")
        icons = Gtk.Box(spacing=24)
        self.box.pack_start(icons, True, True, 0)
        for index, icon in enumerate(self.params["order"]):
            widget = Gtk.Label(label=icon)
            widget.set_has_tooltip(True)
            widget.connect("query-tooltip", self.tooltip_query, icon, self.params["codes"][index])
            icons.pack_start(widget, True, True, 0)
        self.field("code")
        self.button("Submit", self.tooltip_submit)

    def tooltip_query(self, widget, x, y, keyboard, tooltip, icon, code) -> bool:
        if keyboard:
            return False
        tooltip.set_text(code)
        self.emit("tooltip_query", icon=icon)
        return True

    def tooltip_submit(self) -> None:
        value = self.values()["code"]
        self.state.update(
            submit_count=self.state["submit_count"] + 1, status="Submitted", submitted_value=value
        )
        self.emit("submit", value=value)

    def irmodal(self) -> None:
        self.state.update(
            prompt_answers=[], submit_count=0, page=1, prompt_state="not_shown", next_count=0
        )
        self.field("name")
        self.field("email")
        self.step1 = list(self.box.get_children())
        self.field("team", C.IR_TEAMS)
        self.field("seats")
        self.step2 = [widget for widget in self.box.get_children() if widget not in self.step1]
        self.hidden = self.step2
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
            widget.hide()
        for widget in self.step2:
            widget.show_all()
        self.submit_button.destroy()
        self.button("Submit", self.form_submit).show()

    def form_submit(self) -> None:
        values = self.values()
        self.state.update(submit_count=self.state["submit_count"] + 1, submitted=values)
        self.emit("submit", values=values)

    def irbanner(self) -> None:
        self.state.update(
            banner_shown=False, banner_actions=[], banner_body_clicks=0, apply_count=0
        )
        self.label(self.params["item"])
        self.field("quantity", value="1")
        self.field("priority", C.PRIORITIES)
        self.apply_button = self.button("Apply", self.apply)
        self.box.remove(self.apply_button)
        overlay = Gtk.Overlay()
        overlay.add(self.apply_button)
        self.banner = Gtk.EventBox()
        content = Gtk.Box(spacing=8)
        self.banner.add(content)
        self.label("Update available", content)
        for label, action in (
            ("Restart Now", "restart"),
            ("Later", "later"),
            ("Dismiss", "dismiss"),
        ):
            self.button(label, lambda a=action: self.banner_action(a), content)
        self.banner.connect("button-press-event", self.banner_body_click)
        overlay.add_overlay(self.banner)
        self.box.pack_end(overlay, False, False, 0)
        self.hidden = [self.banner]
        self.state["fields"] = self.values()

    def banner_body_click(self, *_: object) -> bool:
        self.state["banner_body_clicks"] += 1
        self.emit("banner_body_click", count=self.state["banner_body_clicks"])
        return False

    def show_banner(self) -> bool:
        self.banner.show_all()
        self.emit("banner_shown")
        return False

    def banner_action(self, action: str) -> None:
        self.state["banner_actions"].append(action)
        if action == "restart":
            self.fields["quantity"].set_text("1")
            self.fields["priority"].set_active(0)
        self.banner.hide()
        self.emit("banner_action", action=action)

    def apply(self) -> None:
        self.state["apply_count"] += 1
        self.emit("apply", values=self.values())

    def irunsaved(self) -> None:
        self.state.update(
            saved=[
                {
                    "title": title,
                    "status": C.IR_INITIAL_STATUS,
                    "body": f"Working notes for {title}.",
                }
                for title in self.params["titles"]
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
            return
        self.selected = index
        self.loading = True
        for key, value in self.state["saved"][index].items():
            widget = self.fields[key]
            if isinstance(widget, Gtk.ComboBoxText):
                widget.set_active([C.IR_INITIAL_STATUS, *C.IR_STATUS_WORDS].index(value))
            else:
                widget.set_text(value)
        self.loading = False
        self.emit("note_selected", index=index)

    def irconsent(self) -> None:
        self.state.update(
            consent_shown=False, consent_answers=[], submit_count=0, submits_blocked=0
        )
        self.field("email")
        self.field("plan", C.IR_PLANS)
        digest = Gtk.CheckButton(label="Email digest")
        self.fields["digest"] = digest
        digest.connect("toggled", lambda *_: self.edit("digest"))
        self.box.pack_start(digest, False, False, 0)
        self.button("Sign up", self.signup)

    def show_consent(self) -> bool:
        self.emit("consent_shown")
        self.dialog(
            "Cookie preferences",
            [("Reject optional", "reject"), ("Accept all", "accept")],
            self.consent_answer,
        )
        return False

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
    lab = Lab(parser.parse_args())
    try:
        Gtk.main()
    finally:
        lab.events.close()


if __name__ == "__main__":
    main()
