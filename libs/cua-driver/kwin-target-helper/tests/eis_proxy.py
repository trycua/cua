# SPDX-License-Identifier: MIT
# Serve KWin's EIS on a named socket for $LIBEI_SOCKET. KWin hands out EIS only as
# an fd over D-Bus, so each client connection gets its own KWin connection, and
# bytes and passed fds are forwarded both ways.
import dbus
import os
import select
import socket
import sys
import threading

path = sys.argv[1]
bus = dbus.SessionBus()
eis = dbus.Interface(
    bus.get_object("org.kde.KWin", "/org/kde/KWin/EIS/RemoteDesktop"),
    "org.kde.KWin.EIS.RemoteDesktop",
)


def pump(a, b):
    socks = [a, b]
    while True:
        for s in select.select(socks, [], [])[0]:
            other = b if s is a else a
            try:
                data, fds, _flags, _addr = socket.recv_fds(s, 65536, 16)
            except OSError:
                return
            if not data and not fds:
                return
            socket.send_fds(other, [data], fds) if fds else other.sendall(data)
            for fd in fds:
                os.close(fd)


srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
if os.path.exists(path):
    os.unlink(path)
srv.bind(path)
srv.listen(4)
print("eis proxy ready", flush=True)
while True:
    client, _ = srv.accept()
    fd, _cookie = eis.connectToEIS(3)
    kwin = socket.socket(fileno=fd.take())
    threading.Thread(target=pump, args=(client, kwin), daemon=True).start()
