#!/usr/bin/env python3
"""Fast-path A/B bench and race tests against FastPathFixture (CUA-3102).

Runs on macOS with a Cua Driver daemon and the fixture running:
  swiftc -O tests/fixtures/apps/macos/fast-path/FastPathFixture.swift -o /tmp/FastPathFixture
  /tmp/FastPathFixture /tmp/fpfix &
  python3 scripts/fast-path-bench.py --out /tmp/fast-path-results.json

Every tool call is one model-visible turn in a direct-call agent loop, so
`turns` counts calls. Tool wall time is measured per call; model time is
not included (the report adds an estimate per turn).
"""

import argparse
import json
import os
import random
import re
import statistics
import subprocess
import time

FIX = "/tmp/fpfix"
CLI = os.path.expanduser(os.environ.get("CUA_DRIVER_CLI", "~/.local/bin/cua-driver"))
SESSION = os.environ.get("FP_SESSION", "fast-path-bench")
WIN = {"x": 0.0, "y": 0.0, "scale": 1.0}


def call(tool, args):
    t = time.perf_counter()
    # Snapshots are session-scoped and each CLI call is its own connection,
    # so a shared session label is what lets reads and actions chain.
    if tool not in ("set_config", "get_config"):
        args = dict(args, session=SESSION)
    p = subprocess.run([CLI, "call", tool, json.dumps(args)], capture_output=True, text=True, timeout=60)
    dt = (time.perf_counter() - t) * 1000
    out = p.stdout.strip()
    data = None
    try:
        data = json.loads(out)
    except ValueError:
        m = re.search(r"\{.*\}\s*$", out, re.S)
        if m:
            try:
                data = json.loads(m.group(0))
            except ValueError:
                data = None
    return {"tool": tool, "ms": dt, "text": out, "json": data or {}, "rc": p.returncode, "err": p.stderr.strip()}


def structured(r):
    j = r["json"]
    return j.get("structuredContent") or j.get("structured_content") or j


def is_error(r):
    j = r["json"]
    return bool(j.get("isError") or j.get("is_error")) or r["rc"] != 0


def fixture(cmd):
    with open(os.path.join(FIX, "cmd"), "a") as f:
        f.write(cmd + "\n")


def wait_ack(n, budget=2.0):
    end = time.time() + budget
    while time.time() < end:
        try:
            if open(os.path.join(FIX, "ack")).read().strip() == f"ack {n}":
                return True
        except OSError:
            pass
        time.sleep(0.01)
    return False


class Fixture:
    def __init__(self):
        self.n = 0
        open(os.path.join(FIX, "log"), "a").close()

    def do(self, cmd, wait=True):
        fixture(cmd)
        self.n += 1
        if wait:
            wait_ack(self.n)

    def log_mark(self):
        return len(open(os.path.join(FIX, "log")).read().splitlines())

    def log_since(self, mark):
        time.sleep(0.25)
        lines = open(os.path.join(FIX, "log")).read().splitlines()[mark:]
        return [l for l in lines if not l.startswith("applied")]


def target():
    r = call("list_windows", {"app": "FastPathFixture"})
    s = structured(r)
    wins = s.get("windows") or []
    for w in wins:
        if "FastPath" in (w.get("title") or ""):
            b = w.get("bounds") or w.get("frame") or {}
            WIN["x"], WIN["y"] = float(b.get("x", 0)), float(b.get("y", 0))
            return w["pid"], w["window_id"], b
    raise SystemExit(f"fixture window not found: {r['text'][:400]}")


def read(pid, wid, screenshot=True, since=None):
    args = {"pid": pid, "window_id": wid, "tree_format": "elements", "include_screenshot": screenshot}
    if since:
        args["since"] = since
    r = call("get_window_state", args)
    st = structured(r)
    sw, wb = st.get("screenshot_width"), st.get("window_bounds") or {}
    if sw and wb.get("width"):
        WIN["scale"] = sw / wb["width"]
    return r, st


def elements(s):
    return s.get("elements") or []


def find(s, label, role=None):
    for e in elements(s):
        names = [e.get(k) for k in ("title", "label", "description", "name")]
        if label in [n for n in names if n] and (role is None or e.get("role") == role):
            return e
    return None


def token(s, e):
    return e.get("element_token") or f"{s.get('snapshot_id')}:{e.get('element_index', e.get('index'))}"


def center_local(e, win):
    f = e.get("frame") or e.get("window_frame")
    if isinstance(f, dict):
        x, y = f["x"], f["y"]
        w, h = f.get("w", f.get("width")), f.get("h", f.get("height"))
    else:
        x, y, w, h = f
    return x + w / 2 - WIN["x"], y + h / 2 - WIN["y"]


def px(x, y):
    """Window-local points to screenshot pixels, the unit pixel clicks take."""
    args = {"x": int(round(x * WIN["scale"])), "y": int(round(y * WIN["scale"]))}
    # Pixel clicks are delivered in the foreground by default so the bench is
    # deterministic. Background pixel clicks were re-checked on the macOS 27.2
    # VM (two window origins, target behind another app, fast_path off, guard and on) and
    # land on the right element; set FP_PIXEL_DELIVERY=background to use them.
    mode = os.environ.get("FP_PIXEL_DELIVERY", "foreground")
    if mode != "background":
        args["delivery_mode"] = mode
    return args


def set_mode(mode):
    call("set_config", {"fast_path": mode})


# ── Bench tasks ─────────────────────────────────────────────────────────


def task_form(pid, wid, mode, fx):
    fx.do("reset")
    calls = []
    r, s = read(pid, wid)
    calls.append(r)
    values = {"Name": "Ada", "Email": "ada@example.com", "City": "Turin"}
    for label, value in values.items():
        if mode == "off":
            r, s = read(pid, wid)
            calls.append(r)
        e = find(s, label)
        if e is None:
            return calls, "field not found"
        r = call("type_text", {"pid": pid, "window_id": wid, "element_token": token(s, e), "text": value})
        calls.append(r)
        s = next_state(s, r)
    if mode == "off":
        r, s = read(pid, wid)
        calls.append(r)
    e = find(s, "Submit")
    r = call("click", {"pid": pid, "window_id": wid, "element_token": token(s, e)})
    calls.append(r)
    return calls, None


def next_state(s, r):
    """After an action with fast_path=on, tokens move to the delta's snapshot."""
    d = (structured(r).get("fast_path") or {}).get("delta") or {}
    sid = d.get("snapshot_id")
    if not sid:
        return s
    s2 = dict(s)
    s2["snapshot_id"] = sid
    s2["elements"] = [dict(e, element_token=f"{sid}:{e.get('element_index', e.get('index'))}") for e in elements(s)]
    return s2


def task_settings(pid, wid, mode, fx):
    fx.do("reset")
    calls = []
    r, s = read(pid, wid)
    calls.append(r)
    for i in range(1, 7):
        if mode == "off":
            r, s = read(pid, wid)
            calls.append(r)
        e = find(s, f"Option {i}")
        r = call("click", {"pid": pid, "window_id": wid, "element_token": token(s, e)})
        calls.append(r)
        s = next_state(s, r)
    return calls, None


def task_canvas(pid, wid, mode, fx, canvas_origin):
    """Three pixel clicks on the canvas tile (no tree there)."""
    fx.do("reset")
    calls = []
    r, s = read(pid, wid)
    calls.append(r)
    cx, cy = canvas_origin[0] + 64, canvas_origin[1] + 54
    for _ in range(3):
        if mode == "off":
            r, s = read(pid, wid)
            calls.append(r)
        r = call("click", dict({"pid": pid, "window_id": wid}, **px(cx, cy)))
        calls.append(r)
    return calls, None


def run_bench(pid, wid, fx, canvas_origin, reps):
    out = {}
    for mode in ("off", "on"):
        set_mode(mode)
        for name, fn in (("form", task_form), ("settings", task_settings), ("canvas", task_canvas)):
            rows = []
            for _ in range(reps):
                mark = fx.log_mark()
                t = time.perf_counter()
                calls, err = fn(pid, wid, mode, fx, canvas_origin) if name == "canvas" else fn(pid, wid, mode, fx)
                wall = (time.perf_counter() - t) * 1000
                refusals = sum(1 for c in calls if is_error(c) and "target_changed" in c["text"] + json.dumps(c["json"]))
                actions = sum(1 for c in calls if '"tool"' not in c["text"][:0]) - sum(1 for _ in [])
                rows.append({
                    "wall_ms": wall,
                    "turns": len(calls),
                    "tool_ms": sum(c["ms"] for c in calls),
                    "per_call": [(c["tool"], c["ms"]) for c in calls],
                    "errors": sum(1 for c in calls if is_error(c)),
                    "false_refusals": refusals,
                    "err": err,
                    "log": fx.log_since(mark),
                })
            out[f"{name}/{mode}"] = {
                "wall_ms_median": statistics.median(r["wall_ms"] for r in rows),
                "turns": rows[0]["turns"],
                "tool_ms_median": statistics.median(r["tool_ms"] for r in rows),
                "per_tool_ms_median": {
                    t: statistics.median(ms for r in rows for (tt, ms) in r["per_call"] if tt == t)
                    for t in sorted({tt for r in rows for (tt, _) in r["per_call"]})
                },
                "all_ok": all(not r["errors"] for r in rows),
                "errors": sum(r["errors"] for r in rows),
                "false_refusals": sum(r["false_refusals"] for r in rows),
                "sample_log": rows[0]["log"],
                "err": [r["err"] for r in rows if r["err"]],
            }
    set_mode("off")
    return out


# ── Race tests (fast_path=guard) ───────────────────────────────────────


def race_tests(pid, wid, fx, canvas_origin, iterations):
    set_mode("guard")
    res = {}

    def pixel_click_after(change, label):
        fx.do("reset")
        r, s = read(pid, wid)
        e = find(s, label)
        x, y = center_local(e, None)
        mark = fx.log_mark()
        fx.do(change)
        c = call("click", dict({"pid": pid, "window_id": wid}, **px(x, y)))
        return c, fx.log_since(mark)

    def token_click_after(change, label):
        fx.do("reset")
        r, s = read(pid, wid)
        e = find(s, label)
        mark = fx.log_mark()
        fx.do(change)
        c = call("click", {"pid": pid, "window_id": wid, "element_token": token(s, e)})
        return c, fx.log_since(mark)

    def summarize(c, log):
        st = structured(c)
        return {
            "refused": is_error(c),
            "reason": st.get("reason"),
            "fast_path": st.get("fast_path"),
            "log": log,
        }

    res["moved_pixel"] = summarize(*pixel_click_after("move_alpha 120", "Alpha"))
    res["moved_token"] = summarize(*token_click_after("move_alpha 120", "Alpha"))
    res["swap_pixel"] = summarize(*pixel_click_after("swap", "Alpha"))
    res["swap_token"] = summarize(*token_click_after("swap", "Alpha"))
    res["relabel_token"] = summarize(*token_click_after("relabel_alpha Gamma", "Alpha"))
    res["removed_token"] = summarize(*token_click_after("remove_beta", "Beta"))

    cx, cy = canvas_origin[0] + 64, canvas_origin[1] + 54
    for name, change in (("canvas_move", "canvas_move 40"), ("canvas_shape", "canvas_shape")):
        fx.do("reset")
        read(pid, wid)
        mark = fx.log_mark()
        fx.do(change)
        c = call("click", dict({"pid": pid, "window_id": wid}, **px(cx, cy)))
        res[name] = summarize(c, fx.log_since(mark))

    # Randomized race: a random change lands 0-50 ms after the read.
    wrong = refused = acted = 0
    changes = ["move_alpha 120", "swap", "relabel_alpha Gamma", "noop"]
    for i in range(iterations):
        fx.do("reset")
        r, s = read(pid, wid, screenshot=False)
        e = find(s, "Alpha")
        x, y = center_local(e, None)
        change = random.choice(changes)
        pixel = random.random() < 0.5
        mark = fx.log_mark()
        if change != "noop":
            fx.do(change, wait=False)
        time.sleep(random.uniform(0, 0.05))
        args = {"pid": pid, "window_id": wid}
        args.update(px(x, y) if pixel else {"element_token": token(s, e)})
        c = call("click", args)
        log = fx.log_since(mark)
        presses = [l for l in log if l.startswith("pressed")]
        if is_error(c):
            refused += 1
        else:
            acted += 1
        if any(l != "pressed Alpha" for l in presses):
            # Relabelled Alpha shows as "pressed Gamma": the agent meant Alpha.
            wrong += 1
    res["random_race"] = {"iterations": iterations, "acted": acted, "refused": refused, "wrong_target": wrong}

    # Static false refusals: no change at all.
    fx.do("reset")
    r, s = read(pid, wid)
    e = find(s, "Alpha")
    x, y = center_local(e, None)
    stat = {"alpha_pixel": 0, "canvas_pixel": 0, "n": iterations}
    for _ in range(iterations):
        if is_error(call("click", dict({"pid": pid, "window_id": wid}, **px(x, y)))):
            stat["alpha_pixel"] += 1
        if is_error(call("click", dict({"pid": pid, "window_id": wid}, **px(cx, cy)))):
            stat["canvas_pixel"] += 1
    res["static_false_refusals"] = stat
    set_mode("off")
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="/tmp/fast-path-results.json")
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--iterations", type=int, default=100)
    ap.add_argument("--canvas-origin", default="380,40", help="canvas top-left in window points")
    a = ap.parse_args()
    pid, wid, _ = target()
    fx = Fixture()
    origin = tuple(float(v) for v in a.canvas_origin.split(","))
    result = {"bench": run_bench(pid, wid, fx, origin, a.reps), "race": race_tests(pid, wid, fx, origin, a.iterations)}
    json.dump(result, open(a.out, "w"), indent=2)
    print(json.dumps(result, indent=2)[:6000])


if __name__ == "__main__":
    main()
