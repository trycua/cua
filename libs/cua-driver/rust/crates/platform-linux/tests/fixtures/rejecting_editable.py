# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Private AT-SPI application: one editable that rejects SetTextContents.

It registers with the real AT-SPI registry on the accessibility bus of the
session it runs in, so the driver finds it the way it finds any application.
`ToolkitName` and the unit `InsertText` reads its length in follow the toolkit
the test names: GTK reads UTF-8 bytes, Qt resizes the string to exactly that
many UTF-16 units, and with no toolkit the property is absent. The test reads
the text, caret and calls back over stdin/stdout, not over AT-SPI.
"""

import json
import os
import sys
from typing import Any

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

assert os.environ.get("CUA_PRIVATE_ATSPI_TEST") == "1"
DBusGMainLoop(set_as_default=True)
session = dbus.SessionBus()
address = session.get_object("org.a11y.Bus", "/org/a11y/bus").GetAddress(dbus_interface="org.a11y.Bus")
bus = dbus.bus.BusConnection(str(address))
A = "org.a11y.atspi.Accessible"
APP = "org.a11y.atspi.Application"
T = "org.a11y.atspi.Text"
E = "org.a11y.atspi.EditableText"
ROOT = "/org/a11y/atspi/accessible/root"
ENTRY = "/org/a11y/atspi/accessible/entry"
state: dict[str, Any] = dict(text="", caret=0, mode="reject", toolkit=None, sets=0, inserts=[])


def unknown_property():
    return dbus.exceptions.DBusException("unknown property", name="org.freedesktop.DBus.Error.UnknownProperty")


def insert_as_toolkit(text, length):
    """The text a toolkit inserts for `InsertText(text, length)`."""
    if state["toolkit"] == "Qt":
        # QString::resize: shorter drops the tail, longer appends garbage.
        units = text.encode("utf-16-le")[: 2 * length].decode("utf-16-le", errors="replace")
        return units + "\0" * max(0, length - len(text.encode("utf-16-le")) // 2)
    # UTF-8 bytes; an incomplete final sequence is dropped.
    return text.encode("utf-8")[:length].decode("utf-8", errors="ignore")


class Accessible(dbus.service.Object):
    def __init__(self, path, root=False):
        self.root = root
        super().__init__(bus, path)

    @dbus.service.method(A, in_signature="", out_signature="a(so)")
    def GetChildren(self):
        return [(bus.get_unique_name(), dbus.ObjectPath(ENTRY))] if self.root else []

    @dbus.service.method(A, in_signature="", out_signature="as")
    def GetInterfaces(self):
        return [A, APP] if self.root else [A, T, E]

    @dbus.service.method(A, in_signature="", out_signature="s")
    def GetRoleName(self):
        return "application" if self.root else "text"

    @dbus.service.method(A, in_signature="", out_signature="au")
    def GetState(self):
        return [256 | (1 << 30), 0]  # Enabled and Showing; not focused.

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
    def Get(self, interface, prop):
        if prop == "ToolkitName" and interface == APP and self.root:
            if state["toolkit"] is None:
                raise unknown_property()
            return state["toolkit"]
        if prop == "Name":
            return "cua-rejecting-fixture" if self.root else "cua-rejecting-owned-entry"
        if prop == "CaretOffset":
            if state["mode"] == "caret-error":
                raise dbus.exceptions.DBusException("caret failed", name="org.a11y.Test.Error")
            return dbus.Int32(state["caret"])
        if prop == "CharacterCount":
            return dbus.Int32(len(state["text"]))
        raise unknown_property()

    @dbus.service.method(T, in_signature="ii", out_signature="s")
    def GetText(self, start, end):
        return state["text"][start : end if end >= 0 else None]

    @dbus.service.method(E, in_signature="s", out_signature="b")
    def SetTextContents(self, text):
        state["sets"] += 1
        if state["mode"] == "accept":
            state["text"] = str(text)
            return True
        if state["mode"] == "set-error":
            raise dbus.exceptions.DBusException("set failed", name="org.a11y.Test.Error")
        return False

    @dbus.service.method(E, in_signature="isi", out_signature="b")
    def InsertText(self, offset, text, length):
        state["inserts"].append([int(offset), str(text), int(length)])
        if state["mode"] == "insert-error":
            raise dbus.exceptions.DBusException("insert failed", name="org.a11y.Test.Error")
        if state["mode"] == "insert-false":
            return False
        inserted = insert_as_toolkit(str(text), int(length))
        state["text"] = state["text"][:offset] + inserted + state["text"][offset:]
        state["caret"] = int(offset) + len(inserted)
        return True


root = Accessible(ROOT, True)
entry = Accessible(ENTRY)
registry = bus.get_object("org.a11y.atspi.Registry", ROOT)
registry.Embed((bus.get_unique_name(), dbus.ObjectPath(ROOT)), dbus_interface="org.a11y.atspi.Socket")
loop = GLib.MainLoop()


def reply(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


def command(source, condition):
    line = sys.stdin.readline()
    if not line:
        loop.quit()
        return False
    request = json.loads(line)
    if request["op"] == "reset":
        state.update(
            text=request.get("text", ""),
            caret=request.get("caret", 0),
            mode=request.get("mode", "reject"),
            toolkit=request["toolkit"],
            sets=0,
            inserts=[],
        )
    reply(state)
    return True


GLib.io_add_watch(sys.stdin, GLib.IO_IN | GLib.IO_HUP, command)
GLib.timeout_add_seconds(90, lambda: (loop.quit(), False)[1])
reply({"pid": os.getpid()})
loop.run()
