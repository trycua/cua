#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Drive raw_mcp.py over stdio with fixed scripts against BenchLab (no model): the smoke test of the prototype.

Usage: smoke_client.py RAW_MCP_PATH   (in an X11 + AT-SPI session with BenchLab running)
"""

import json
import subprocess
import sys

SCRIPTS = {
    "look": 'a = raw.app("BenchLab")\nreturn {"windows": raw.windows(), "tree": raw.dump(a, depth=14)[:6000]}',
    "lint_block": "raw.xtst.XTestFakeMotionEvent(raw.dpy, -1, 300, 300, 0)\nreturn raw.pointer()",
    "escalated_motion": (
        'raw.escalate_foreground("smoke test of the declared foreground route")\n'
        "raw.xtst.XTestFakeMotionEvent(raw.dpy, -1, 333, 222, 0)\n"
        "raw.x11.XFlush(raw.dpy)\nraw.sleep(0.3)\nreturn raw.pointer()"
    ),
    "lint_dynamic": 'sub = "mouse" + "move"\nraw.xdotool(sub, 50, 50)\nreturn raw.pointer()',
    # passes the lint (a plain local name the lint does not know), refused by the runtime guard at lookup
    "runtime_refuse": (
        'n = "_NET_ACTIVE" + "_WINDOW"\nraw.x11.XInternAtom(raw.dpy, n.encode(), 0)\nreturn raw.active_window()'
    ),
    # an app-side focus change the lint cannot see: the hook reports it (the AT-SPI action itself is allowed)
    "runtime_hook": (
        'a = raw.app("BenchLab")\nq = raw.find(a, role="text", name="Quantity")[0]\n'
        "Atspi.Component.grab_focus(q)\nraw.sleep(0.4)\nreturn raw.describe(q)"
    ),
    "combo_popup": (
        'a = raw.app("BenchLab")\np = raw.find(a, role="combo box", name="Priority")[0]\n'
        "Atspi.Action.do_action(p, 0)\nraw.sleep(0.6)\nshown = raw.describe(p)\n"
        'lo = raw.find(a, role="menu item", name="Low")[0]\nAtspi.Action.do_action(lo, 0)\nraw.sleep(0.4)\n'
        "return shown"
    ),
    "chain_no_escalation": 'raw.xdotool("key", "--window", "1", "a", "key", "b")\nreturn raw.pointer()',
    "escalated_chain": (
        'raw.escalate_foreground("smoke: escalated pointer chain")\n'
        'raw.xdotool("mousemove", "700", "500", "mousemove", "710", "505")\nreturn raw.pointer()'
    ),
    "private_handle": 'f = raw.xtst._lib\nreturn 1',
    "act": (
        'a = raw.app("BenchLab")\n'
        'q = raw.find(a, role="text", name="Quantity")[0]\n'
        'Atspi.EditableText.set_text_contents(q, "3")\n'
        "Atspi.Action.do_action(q, 0)\n"
        'p = raw.find(a, role="combo box", name="Priority")[0]\n'
        "Atspi.Action.do_action(p, 0)\n"
        "raw.sleep(0.5)\n"
        'hi = raw.find(a, role="menu item", name="High")[0]\n'
        "Atspi.Action.do_action(hi, 0)\n"
        "raw.sleep(0.5)\n"
        'for name in ("Dismiss", "Apply"):\n'
        '    b = [b for b in raw.find(a, role="button", name=name, showing=True)]\n'
        "    if b:\n"
        "        Atspi.Action.do_action(b[0], 0)\n"
        "        raw.sleep(0.5)\n"
        'return {"q": Atspi.Text.get_text(q, 0, -1), "p": raw.describe(p), '
        '"buttons": [raw.describe(b) for b in raw.find(a, role="button")]}'
    ),
    "shot": "w = [w for w in raw.windows() if 'Bench' in w['title']]\nreturn raw.screenshot(w[-1]['window_id'] if w else None)",
}


def main() -> int:
    proc = subprocess.Popen(
        [sys.executable, "-I", "-B", sys.argv[1]],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )

    def rpc(i, method, params=None):
        proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params or {}}) + "\n"
        )
        proc.stdin.flush()
        return json.loads(proc.stdout.readline())

    rpc(0, "initialize")
    tools = rpc(1, "tools/list")["result"]["tools"]
    print("tools:", [t["name"] for t in tools], "description chars", len(tools[0]["description"]))
    for n, (name, script) in enumerate(SCRIPTS.items(), 2):
        res = rpc(
            n,
            "tools/call",
            {"name": "run_script", "arguments": {"script": script, "timeout_ms": 60000}},
        )["result"]
        kinds = [c["type"] for c in res["content"]]
        text = next(c["text"] for c in res["content"] if c["type"] == "text")
        print(f"===== {name}: isError={res['isError']} content={kinds}\n{text[:3500]}\n")
    proc.stdin.close()
    proc.wait(timeout=10)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
