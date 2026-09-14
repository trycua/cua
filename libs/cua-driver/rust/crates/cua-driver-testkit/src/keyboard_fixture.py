import ctypes as c
import json
import os

x = c.CDLL("libX11.so.6")

def api(name, result, *args):
    fn = getattr(x, name)
    fn.restype = result
    fn.argtypes = args
    return fn

p = c.c_void_p
u = c.c_ulong
i = c.c_int
open_display = api("XOpenDisplay", p, c.c_char_p)
root_window = api("XDefaultRootWindow", u, p)
create = api("XCreateSimpleWindow", u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)
select = api("XSelectInput", i, p, u, c.c_long)
map_window = api("XMapWindow", i, p, u)
store_name = api("XStoreName", i, p, u, c.c_char_p)
atom = api("XInternAtom", u, p, c.c_char_p, i)
property_ = api("XChangeProperty", i, p, u, u, u, i, i, p, i)
flush = api("XSync", i, p, i)
next_event = api("XNextEvent", i, p, p)
lookup = api("XLookupKeysym", u, p, i)
destroy = api("XDestroyWindow", i, p, u)

class KeyEvent(c.Structure):
    _fields_ = [("type", i), ("serial", u), ("send_event", i), ("display", p),
                ("window", u), ("root", u), ("subwindow", u), ("time", u),
                ("x", i), ("y", i), ("x_root", i), ("y_root", i),
                ("state", c.c_uint), ("keycode", c.c_uint), ("same_screen", i)]

def emit(kind, **fields):
    print(json.dumps(dict(kind=kind, **fields)), flush=True)

display = open_display(None)
assert display, "X11 display unavailable"
window = create(display, root_window(display), 160, 160, 420, 240, 0, 0, 0xffffff)
store_name(display, window, b"Cua Keyboard Oracle")
wm_class = b"keyboard-oracle\0XTerm\0"
property_(display, window, atom(display, b"WM_CLASS", 0), 31, 8, 0, c.cast(c.c_char_p(wm_class), p), len(wm_class))
pid = u(os.getpid())
property_(display, window, atom(display, b"_NET_WM_PID", 0), 6, 32, 0, c.byref(pid), 1)
select(display, window, (1 << 0) | (1 << 1) | (1 << 17))
map_window(display, window)
flush(display, 0)
event = (c.c_long * 24)()
closed = False
ready = False
while True:
    next_event(display, c.byref(event))
    key = c.cast(c.byref(event), c.POINTER(KeyEvent)).contents
    if key.type == 19 and not ready:
        ready = True
        emit("ready", window=window)
    if key.type not in (2, 3):
        continue
    emit("down" if key.type == 2 else "up", key=lookup(c.byref(event), 0), flags=key.state, synthetic=bool(key.send_event))
    if key.type == 2 and os.environ.get("CUA_KEYBOARD_CLOSE_ON_KEY") == "1" and not closed:
        destroy(display, window)
        flush(display, 0)
        closed = True
        emit("closed")
