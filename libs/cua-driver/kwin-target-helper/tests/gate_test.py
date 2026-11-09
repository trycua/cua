# SPDX-License-Identifier: MIT
# The helper's input gate, driven by a raw libei client like the driver's.
# Windows A and B log every key and click they receive to argv[1].
import ctypes as C
import dbus
import json
import os
import sys
import time
import tempfile

LOG = sys.argv[1]
bus = dbus.SessionBus()
helper = dbus.Interface(
    bus.get_object("org.cua.KWinTarget", "/org/cua/KWinTarget"), "org.cua.KWinTarget"
)


def windows():
    return json.loads(str(helper.GetWindows()))


def win(title):
    return next(w for w in windows() if w["title"] == title)


def activate(title):
    js = tempfile.NamedTemporaryFile("w", suffix=".js", delete=False)
    js.write(
        "for (const w of workspace.windowList()) if (w.caption === %s) workspace.activeWindow = w;"
        % json.dumps(title)
    )
    js.close()
    s = dbus.Interface(bus.get_object("org.kde.KWin", "/Scripting"), "org.kde.kwin.Scripting")
    sid = s.loadScript(js.name, "cuatest-activate", signature="ss")
    dbus.Interface(
        bus.get_object("org.kde.KWin", f"/Scripting/Script{sid}"), "org.kde.kwin.Script"
    ).run()
    time.sleep(0.3)
    s.unloadScript("cuatest-activate")
    for _ in range(40):
        if json.loads(str(helper.GetFocus(win(title)["uuid"])))["keyboard"]:
            return
        time.sleep(0.05)
    raise SystemExit(f"could not focus {title}")


ei = C.CDLL("libei.so.1")
for f, r, a in [
    ("ei_new_sender", C.c_void_p, [C.c_void_p]),
    ("ei_configure_name", None, [C.c_void_p, C.c_char_p]),
    ("ei_setup_backend_fd", C.c_int, [C.c_void_p, C.c_int]),
    ("ei_dispatch", None, [C.c_void_p]),
    ("ei_get_event", C.c_void_p, [C.c_void_p]),
    ("ei_event_get_type", C.c_int, [C.c_void_p]),
    ("ei_event_get_seat", C.c_void_p, [C.c_void_p]),
    ("ei_event_get_device", C.c_void_p, [C.c_void_p]),
    ("ei_event_unref", C.c_void_p, [C.c_void_p]),
    ("ei_device_ref", C.c_void_p, [C.c_void_p]),
    ("ei_device_has_capability", C.c_bool, [C.c_void_p, C.c_int]),
    ("ei_device_get_name", C.c_char_p, [C.c_void_p]),
    ("ei_device_start_emulating", None, [C.c_void_p, C.c_uint32]),
    ("ei_device_frame", None, [C.c_void_p, C.c_uint64]),
    ("ei_now", C.c_uint64, [C.c_void_p]),
    ("ei_device_keyboard_key", None, [C.c_void_p, C.c_uint32, C.c_bool]),
    ("ei_device_pointer_motion_absolute", None, [C.c_void_p, C.c_double, C.c_double]),
    ("ei_device_button_button", None, [C.c_void_p, C.c_uint32, C.c_bool]),
]:
    getattr(ei, f).restype = r
    getattr(ei, f).argtypes = a
KBD, ABS, BTN = 1 << 2, 1 << 1, 1 << 5


class Client:
    def __init__(self, name):
        eis = dbus.Interface(
            bus.get_object("org.kde.KWin", "/org/kde/KWin/EIS/RemoteDesktop"),
            "org.kde.KWin.EIS.RemoteDesktop",
        )
        fd, _cookie = eis.connectToEIS(3)
        self.ei = ei.ei_new_sender(None)
        ei.ei_configure_name(self.ei, name.encode())
        assert ei.ei_setup_backend_fd(self.ei, fd.take()) == 0
        self.kbd = self.ptr = None
        self.seq = 1
        for _ in range(100):
            self.pump()
            if self.kbd and self.ptr:
                return
            time.sleep(0.05)
        raise SystemExit("no libei devices")

    def pump(self):
        ei.ei_dispatch(self.ei)
        while ev := ei.ei_get_event(self.ei):
            t = ei.ei_event_get_type(ev)
            if t == 3:
                ei.ei_seat_bind_capabilities(
                    C.c_void_p(ei.ei_event_get_seat(ev)),
                    C.c_int(KBD),
                    C.c_int(ABS),
                    C.c_int(BTN),
                    C.c_int(1 << 0),
                    C.c_int(1 << 4),
                    C.c_void_p(None),
                )
            elif t == 8:
                d = ei.ei_event_get_device(ev)
                if ei.ei_device_has_capability(d, KBD):
                    self.kbd = ei.ei_device_ref(d)
                elif ei.ei_device_has_capability(d, ABS):
                    self.ptr = ei.ei_device_ref(d)
                ei.ei_device_start_emulating(ei.ei_device_ref(d), self.seq)
                self.seq += 1
            ei.ei_event_unref(ev)

    def key(self, code, press):
        ei.ei_device_keyboard_key(self.kbd, code, press)
        ei.ei_device_frame(self.kbd, ei.ei_now(self.ei))
        self.pump()
        time.sleep(0.05)

    def tap(self, code):
        self.key(code, True)
        self.key(code, False)

    def click(self, x, y):
        ei.ei_device_pointer_motion_absolute(self.ptr, x, y)
        ei.ei_device_frame(self.ptr, ei.ei_now(self.ei))
        self.pump()
        time.sleep(0.05)
        for p in (True, False):
            ei.ei_device_button_button(self.ptr, 0x110, p)
            ei.ei_device_frame(self.ptr, ei.ei_now(self.ei))
            self.pump()
            time.sleep(0.05)


def log():
    time.sleep(0.4)
    return open(LOG).read().split("\n")[:-1]


def fresh():
    open(LOG, "w").close()


def arm(title, client="cua-driver 4242"):
    return json.loads(str(helper.ArmInput(win(title)["uuid"], client)))


def disarm(a):
    return json.loads(str(helper.DisarmInput(dbus.UInt64(a["arm"]))))


def centre(title):
    w = win(title)
    return w["x"] + w["w"] / 2, w["y"] + w["h"] / 2


print("version", int(helper.GetVersion()))
drv = Client("cua-driver 4242")
other = Client("other-tool")
print(
    "devices", ei.ei_device_get_name(drv.kbd).decode(), "|", ei.ei_device_get_name(drv.ptr).decode()
)
results = []


def check(name, ok, detail):
    results.append(ok)
    print(("PASS" if ok else "FAIL"), name, detail)


activate("A")
fresh()
a = arm("A")
drv.tap(30)
st = disarm(a)
got = log()
check(
    "1 target focused: key reaches A",
    got == ["A key a"] and st == {"passed": 1, "dropped": 0},
    (got, st),
)
activate("B")
fresh()
a = arm("A")
drv.tap(48)
st = disarm(a)
got = log()
check(
    "2 other window focused: key dropped, B gets nothing",
    got == [] and st["dropped"] == 1 and st["passed"] == 0,
    (got, st),
)
fresh()
drv.tap(46)
got = log()
check("3 no gate armed: key goes to focused B as before", got == ["B key c"], got)
check("4 non-cua client cannot be gated", "error" in arm("A", "other-tool"), "")
activate("B")
fresh()
a = arm("A")
other.tap(32)
st = disarm(a)
got = log()
check(
    "5 armed gate leaves other clients alone",
    got == ["B key d"] and st == {"passed": 0, "dropped": 0},
    (got, st),
)
activate("A")
fresh()
a = arm("A")
drv.key(32, True)
activate("B")
drv.key(32, False)
drv.tap(18)
st = disarm(a)
got = log()
check(
    "6 focus moves mid-input: rest dropped, nothing reaches B",
    got == ["A key d"] and st == {"passed": 1, "dropped": 1},
    (got, st),
)
activate("B")
fresh()
a = arm("A")
drv.click(*centre("B"))
st = disarm(a)
got = log()
check(
    "7 click aimed at B while armed for A: dropped",
    got == [] and st["passed"] == 0 and st["dropped"] >= 1,
    (got, st),
)
activate("A")
fresh()
a = arm("A")
drv.click(*centre("A"))
st = disarm(a)
got = log()
check(
    "8 click on A while armed for A: reaches A",
    got == ["A click 1"] and st["dropped"] == 0,
    (got, st),
)
check("9 client with no libei device cannot be armed", "error" in arm("A", "cua-driver 9999"), "")
priv = dbus.bus.BusConnection(os.environ["DBUS_SESSION_BUS_ADDRESS"])
ph = dbus.Interface(
    priv.get_object("org.cua.KWinTarget", "/org/cua/KWinTarget"), "org.cua.KWinTarget"
)
activate("B")
pa = json.loads(str(ph.ArmInput(win("A")["uuid"], "cua-driver 4242")))
priv.close()
time.sleep(0.5)
fresh()
drv.tap(19)
got = log()
again = arm("A")
disarm(again)
check(
    "10 caller connection closes: gate released",
    "arm" in pa and got == ["B key r"] and "arm" in again,
    (pa, got, again),
)
print("gate suite", sum(results), "/", len(results))
if not all(results):
    sys.exit(1)
