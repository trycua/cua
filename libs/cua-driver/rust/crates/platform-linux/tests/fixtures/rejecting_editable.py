"""Private AT-SPI service: rejects SetTextContents, byte-bounded InsertText.
No GUI, input simulation, or personal application access. Independent pipe readback.
"""
import json
import os
import sys
import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

assert os.environ.get('CUA_NATIVE_GTK_TEST') == '1'
DBusGMainLoop(set_as_default=True)
session = dbus.SessionBus()
address = session.get_object('org.a11y.Bus', '/org/a11y/bus').GetAddress(dbus_interface='org.a11y.Bus')
bus = dbus.bus.BusConnection(str(address))
A = 'org.a11y.atspi.Accessible'
T = 'org.a11y.atspi.Text'
E = 'org.a11y.atspi.EditableText'
ROOT = '/org/a11y/atspi/accessible/root'
ENTRY = '/org/a11y/atspi/accessible/entry'
from typing import Any
state: dict[str, Any] = dict(text='', caret=0, mode='reject', sets=0, inserts=[])

class Accessible(dbus.service.Object):
    def __init__(self, path, root=False):
        self.root = root
        super().__init__(bus, path)

    @dbus.service.method(A, in_signature='', out_signature='a(so)')
    def GetChildren(self):
        return [(bus.get_unique_name(), dbus.ObjectPath(ENTRY))] if self.root else []

    @dbus.service.method(A, in_signature='', out_signature='as')
    def GetInterfaces(self):
        return [A, 'org.a11y.atspi.Application'] if self.root else [A, T, E]

    @dbus.service.method(A, in_signature='', out_signature='s')
    def GetRoleName(self):
        return 'application' if self.root else 'text'

    @dbus.service.method(A, in_signature='', out_signature='au')
    def GetState(self):
        return [256 | (1 << 30), 0]  # Enabled and Showing; not focused.

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='ss', out_signature='v')
    def Get(self, interface, prop):
        if prop == 'Name':
            return 'cua-rejecting-owned-entry' if not self.root else 'cua-rejecting-fixture'
        if prop == 'CaretOffset':
            if state['mode'] == 'caret-error':
                raise dbus.exceptions.DBusException('caret failed', name='org.a11y.Test.Error')
            return dbus.Int32(state['caret'])
        if prop == 'CharacterCount':
            return dbus.Int32(len(state['text']))
        raise dbus.exceptions.DBusException('unknown property', name='org.freedesktop.DBus.Error.UnknownProperty')

    @dbus.service.method(T, in_signature='ii', out_signature='s')
    def GetText(self, start, end):
        return state['text'][start:end if end >= 0 else None]

    @dbus.service.method(E, in_signature='s', out_signature='b')
    def SetTextContents(self, text):
        state['sets'] += 1
        if state['mode'] == 'accept':
            state['text'] = str(text)
            return True
        if state['mode'] == 'set-error':
            raise dbus.exceptions.DBusException('set failed', name='org.a11y.Test.Error')
        return False

    @dbus.service.method(E, in_signature='isi', out_signature='b')
    def InsertText(self, offset, text, length):
        state['inserts'].append([int(offset), str(text), int(length)])
        if state['mode'] == 'insert-error':
            raise dbus.exceptions.DBusException('insert failed', name='org.a11y.Test.Error')
        if state['mode'] == 'insert-false':
            return False
        # AT-SPI length is bytes. Ignore an incomplete final UTF-8 sequence,
        # as a byte-bounded native toolkit insertion does.
        inserted = str(text).encode('utf-8')[:length].decode('utf-8', errors='ignore')
        state['text'] = state['text'][:offset] + inserted + state['text'][offset:]
        state['caret'] = int(offset) + len(inserted)
        return True

root = Accessible(ROOT, True)
entry = Accessible(ENTRY)
registry = bus.get_object('org.a11y.atspi.Registry', ROOT)
registry.Embed((bus.get_unique_name(), dbus.ObjectPath(ROOT)), dbus_interface='org.a11y.atspi.Socket')
loop = GLib.MainLoop()

def reply(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)

def command(source, condition):
    line = sys.stdin.readline()
    if not line:
        loop.quit()
        return False
    request = json.loads(line)
    if request['op'] == 'reset':
        state.update(text=request.get('text', ''), caret=request.get('caret', 0), mode=request.get('mode', 'reject'), sets=0, inserts=[])
    reply(state)
    return True

GLib.io_add_watch(sys.stdin, GLib.IO_IN | GLib.IO_HUP, command)
GLib.timeout_add_seconds(90, lambda: (loop.quit(), False)[1])
reply({'pid': os.getpid()})
loop.run()
