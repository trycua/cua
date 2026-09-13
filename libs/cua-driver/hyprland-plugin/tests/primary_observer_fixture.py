#!/usr/bin/env python3
"""Test-only GTK foreground observer with independent, synchronized wire evidence.

Requires PyGObject GTK3 and pycairo on native Wayland. No plugin trace is used.
All paths must be fresh. The controller only requests synchronization, never
input. WAYLAND_DEBUG is enabled before GTK connects and retained verbatim.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import stat
import struct
import time
import uuid

MAX_BYTES = 32 * 1024 * 1024
MAX_RECORDS = 100000


def main():
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('journal', 'wire', 'control'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--lifetime-ms', type=int, default=180000)
    args = parser.parse_args()
    assert 5000 <= args.lifetime_ms <= 600000
    paths = [path.absolute() for path in (args.journal, args.wire, args.control)]
    assert len(set(paths)) == 3 and all(path.parent.resolve() == path.parent for path in paths)
    journal_path, wire_path, control_path = paths
    os.environ['GDK_BACKEND'] = 'wayland'
    os.environ['WAYLAND_DEBUG'] = 'client'
    journal = journal_path.open('xb', buffering=0)
    wire_fd = os.open(wire_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.dup2(wire_fd, 2)
    os.close(wire_fd)
    import gi
    gi.require_version('Gtk', '3.0')
    gi.require_version('Gdk', '3.0')
    from gi.repository import Gdk, GLib, GObject, Gtk

    display = Gdk.Display.get_default()
    assert display and GObject.type_name(display.__gtype__) == 'GdkWaylandDisplay'
    controller = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    controller.bind(str(control_path))
    os.chmod(control_path, 0o600)
    controller.listen(1)
    controller.setblocking(False)
    identity = {'pid': os.getpid(), 'uid': os.getuid(), 'instance': uuid.uuid4().hex,
                'source_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                'native_wayland': True}
    for name, path in (('journal', journal_path), ('wire', wire_path)):
        info = path.stat()
        assert stat.S_ISREG(info.st_mode)
        identity[name] = {'path': str(path), 'device': info.st_dev, 'inode': info.st_ino}
    counters = {'clicks': 0, 'keys': '', 'scroll': 0, 'motion': 0, 'held': False}
    buttons, keys, nonces = set(), set(), set()
    sequence, failed = 0, False
    window = Gtk.Window(title='Cua Isolated Input Foreground')
    window.set_default_size(600, 500)
    canvas = Gtk.DrawingArea()
    canvas.set_can_focus(True)
    canvas.add_events(Gdk.EventMask.ALL_EVENTS_MASK)
    window.add(canvas)

    def checked_sizes():
        assert sequence < MAX_RECORDS, 'observer record limit reached'
        assert os.fstat(journal.fileno()).st_size < MAX_BYTES and os.fstat(2).st_size < MAX_BYTES, \
            'observer byte limit reached'

    def record(kind, **values):
        nonlocal sequence
        checked_sizes()
        sequence += 1
        row = {'kind': kind, 'time': time.monotonic_ns(), 'seq': sequence,
               'instance': identity['instance'], **values}
        encoded = (json.dumps(row, separators=(',', ':')) + '\n').encode()
        written = journal.write(encoded)
        assert written == len(encoded), 'short observer journal write'
        return row

    def current():
        return {**counters, 'buttons': sorted(buttons), 'keys_down': sorted(keys),
                'window_active': bool(window.is_active()), 'canvas_focus': bool(canvas.has_focus())}

    def abort(error):
        nonlocal failed
        failed = True
        # A missing final sync or any non-protocol wire line fails the reader.
        print('primary observer failed: ' + str(error), flush=True)
        Gtk.main_quit()

    def draw(widget, cr):
        cr.set_source_rgb(0.08, 0.13, 0.18)
        cr.paint()
        cr.set_source_rgb(0.75, 0.9, 0.75)
        cr.select_font_face('sans-serif', 0, 0)
        cr.set_font_size(24)
        for index, line in enumerate(('Foreground — independent primary observer',
                                      'held=' + str(counters['held']),
                                      'motion=' + str(counters['motion']))):
            cr.move_to(20, 55 + index * 45)
            cr.show_text(line)
        return False

    def event(widget, e):
        try:
            data = {}
            if e.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE,
                          Gdk.EventType.MOTION_NOTIFY, Gdk.EventType.SCROLL):
                data.update(x=e.x, y=e.y)
            if e.type in (Gdk.EventType.BUTTON_PRESS, Gdk.EventType.BUTTON_RELEASE):
                _, button = e.get_button()
                data['button'] = int(button)
                if e.type == Gdk.EventType.BUTTON_PRESS:
                    buttons.add(int(button))
                else:
                    buttons.discard(int(button))
                    counters['clicks'] += 1
                counters['held'] = 1 in buttons
                canvas.grab_focus()
            elif e.type in (Gdk.EventType.KEY_PRESS, Gdk.EventType.KEY_RELEASE):
                key = Gdk.keyval_name(e.keyval) or 'unknown'
                data.update(key=key, modifiers=int(e.state))
                if e.type == Gdk.EventType.KEY_PRESS:
                    keys.add(int(e.hardware_keycode))
                    counters['keys'] += key
                else:
                    keys.discard(int(e.hardware_keycode))
            elif e.type == Gdk.EventType.MOTION_NOTIFY:
                counters['motion'] += 1
            elif e.type == Gdk.EventType.SCROLL:
                counters['scroll'] += 1
            if e.type not in (Gdk.EventType.EXPOSE, Gdk.EventType.CONFIGURE):
                record(e.type.value_nick, **data)
            canvas.queue_draw()
        except Exception as error:
            abort(error)
        return False

    def heartbeat():
        try:
            record('state', **current())
            return True
        except Exception as error:
            abort(error)
            return False

    def drain():
        deadline = time.monotonic() + 1
        iterations = 0
        while Gtk.events_pending():
            assert time.monotonic() < deadline and iterations < 1024, 'GTK event drain did not complete'
            Gtk.main_iteration_do(False)
            iterations += 1

    def request(channel, condition):
        try:
            peer, _ = controller.accept()
        except BlockingIOError:
            return True
        with peer:
            peer.settimeout(1)
            try:
                credentials = peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
                _, uid, _ = struct.unpack('3i', credentials)
                assert uid == os.getuid(), 'foreign controller'
                packet, _, flags, _ = peer.recvmsg(1024)
                assert not flags & socket.MSG_TRUNC, 'truncated sync request'
                command = json.loads(packet)
                nonce = command['nonce']
                assert set(command) == {'command', 'nonce'} and command['command'] == 'SYNC'
                assert isinstance(nonce, str) and len(nonce) == 32 and all(c in '0123456789abcdef' for c in nonce)
                assert nonce not in nonces and len(nonces) < 32, 'replayed or excessive synchronization'
                nonces.add(nonce)
                checked_sizes()
                wire_start = os.fstat(2).st_size
                # Two explicit roundtrips, with queued GTK delivery drained.
                # The controller validates the matching callback.done records.
                for _ in range(2):
                    display.sync()
                    drain()
                checked_sizes()
                wire_end = os.fstat(2).st_size
                marker = record('sync', nonce=nonce, wire_start=wire_start,
                                wire_end=wire_end, **current())
                response = {'identity': identity, 'marker': marker, 'journal_end': journal.tell()}
                peer.sendall(json.dumps(response).encode())
            except Exception as error:
                abort(error)
        return not failed

    canvas.connect('draw', draw)
    canvas.connect('event', event)
    window.connect('destroy', Gtk.main_quit)
    window.show_all()
    canvas.grab_focus()
    record('ready', **identity)
    GLib.timeout_add(100, heartbeat)
    GLib.io_add_watch(controller.fileno(), GLib.IO_IN, request)
    GLib.timeout_add(args.lifetime_ms, lambda: (abort('bounded observer lifetime expired'), False)[1])
    print('PRIMARY_OBSERVER_READY', flush=True)
    try:
        Gtk.main()
    finally:
        controller.close()
        journal.close()
    return 1 if failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
