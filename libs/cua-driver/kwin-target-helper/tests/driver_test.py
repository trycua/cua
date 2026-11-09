# SPDX-License-Identifier: MIT
# The real cua-driver (MCP over stdio) against headless KWin with the helper loaded.
# Windows A and B log every key and click they receive to argv[2].
import dbus
import json
import subprocess
import sys
import tempfile
import threading
import time

BIN, LOG = sys.argv[1], sys.argv[2]
bus = dbus.SessionBus()
helper = lambda: dbus.Interface(
    bus.get_object("org.cua.KWinTarget", "/org/cua/KWinTarget"), "org.cua.KWinTarget"
)


def kwin_script(source):
    js = tempfile.NamedTemporaryFile("w", suffix=".js", delete=False)
    js.write(source)
    js.close()
    s = dbus.Interface(bus.get_object("org.kde.KWin", "/Scripting"), "org.kde.kwin.Scripting")
    name = "cuatest-%d" % time.monotonic_ns()
    sid = s.loadScript(js.name, name, signature="ss")
    dbus.Interface(
        bus.get_object("org.kde.KWin", f"/Scripting/Script{sid}"), "org.kde.kwin.Script"
    ).run()
    s.unloadScript(name)


def activate(title):
    kwin_script(
        "for (const w of workspace.windowList()) if (w.caption === %s) workspace.activeWindow = w;"
        % json.dumps(title)
    )
    time.sleep(0.3)


def active_title():
    return next((w["title"] for w in json.loads(str(helper().GetWindows())) if w["active"]), None)


class Mcp:
    def __init__(self):
        self.p = subprocess.Popen(
            [BIN, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )
        self.n = 0
        self.rpc(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "kwin-gate-test", "version": "1"},
            },
        )
        self.p.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"
        )
        self.p.stdin.flush()

    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(
            json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n"
        )
        self.p.stdin.flush()
        while True:
            msg = json.loads(self.p.stdout.readline())
            if msg.get("id") == self.n:
                return msg

    def call(self, tool, args):
        r = self.rpc("tools/call", {"name": tool, "arguments": args})["result"]
        text = " ".join(c.get("text", "") for c in r.get("content", []) if c.get("type") == "text")
        return bool(r.get("isError")), text


def log():
    time.sleep(0.5)
    return open(LOG).read().split("\n")[:-1]


def fresh():
    open(LOG, "w").close()


results = []


def check(name, ok, detail):
    results.append(ok)
    print(("PASS" if ok else "FAIL"), name, "|", detail, flush=True)


d = Mcp()
for _ in range(30):
    r = d.rpc("tools/call", {"name": "list_windows", "arguments": {}})["result"]
    by = {w.get("title"): w for w in r.get("structuredContent", {}).get("windows", [])}
    if "A" in by and "B" in by:
        break
    time.sleep(0.5)
A, B = by["A"], by["B"]
fg = {"delivery_mode": "foreground"}

# 1. Typing into A while B is active: A gets the text, B nothing, B active again after.
activate("B")
fresh()
err, text = d.call("type_text", {"pid": A["pid"], "window_id": A["window_id"], "text": "hi", **fg})
got = log()
check(
    "1 type_text into A while B active",
    not err
    and [x for x in got if x.startswith("A key") and x != "A key Shift_L"] == ["A key h", "A key i"]
    and not any(x.startswith("B") for x in got)
    and active_title() == "B",
    (err, text[:200], got, active_title()),
)

# 2. Pixel click on A while B is active.
# The driver takes pixel coordinates only from a screenshot it has read.
d.call(
    "get_window_state",
    {
        "pid": A["pid"],
        "window_id": A["window_id"],
        "include_accessibility_tree": False,
        "max_image_dimension": 0,
    },
)
activate("B")
fresh()
err, text = d.call(
    "click",
    {
        "pid": A["pid"],
        "window_id": A["window_id"],
        "x": A["width"] // 2,
        "y": A["height"] // 2,
        **fg,
    },
)
got = log()
check(
    "2 click on A while B active",
    not err and "A click 1" in got and not any(x.startswith("B") for x in got),
    (err, text[:200], got),
)

# 3. Focus stolen mid-typing: the call fails, nothing reaches B.
activate("B")
fresh()
stop = threading.Event()


def thief():
    time.sleep(0.5)
    while not stop.is_set():
        activate("B")


t = threading.Thread(target=thief)
t.start()
err, text = d.call(
    "type_text_chars",
    {
        "pid": A["pid"],
        "window_id": A["window_id"],
        "text": "abcdefghijklmnopqrst",
        "delay_ms": 120,
        **fg,
    },
)
stop.set()
t.join()
got = log()
check(
    "3 focus stolen mid-typing: refused, B gets nothing",
    err and "input_interrupted" in text and not any(x.startswith("B key") for x in got),
    (err, text[:300], got),
)

# 4. Hotkey into A while B is active.
activate("B")
fresh()
err, text = d.call(
    "press_key", {"pid": A["pid"], "window_id": A["window_id"], "key": "Return", **fg}
)
got = log()
check(
    "4 press_key into A while B active",
    not err and "A key Return" in got and not any(x.startswith("B") for x in got),
    (err, text[:200], got),
)

# 5. Background stays refused on a KWin window.
activate("B")
fresh()
err, text = d.call(
    "click",
    {"pid": A["pid"], "window_id": A["window_id"], "x": A["width"] // 2, "y": A["height"] // 2},
)
got = log()
check("5 background click is refused", err and got == [], (err, text[:200], got))

# 6. Without the helper's gate, foreground input is refused and nothing is sent.
dbus.Interface(bus.get_object("org.kde.KWin", "/Effects"), "org.kde.kwin.Effects").unloadEffect(
    "cua_kwin_target_helper"
)
time.sleep(0.5)
activate("B")
fresh()
err, text = d.call("type_text", {"pid": A["pid"], "window_id": A["window_id"], "text": "x", **fg})
got = log()
check(
    "6 no gate: refused, nothing sent",
    err and "foreground_unavailable" in text and got == [],
    (err, text[:300], got),
)

print("driver suite", sum(results), "/", len(results), flush=True)
d.p.terminate()
if not all(results):
    sys.exit(1)
