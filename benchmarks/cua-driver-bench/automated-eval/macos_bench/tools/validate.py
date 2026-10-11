#!/usr/bin/env python3
"""Validate the MB-* fixtures and checkers with real input.

For every task: (1) no-op run -> fail; (2) oracle through real HID events (tools/benchinput, CGEvent) -> pass;
(3) reset twice in a row -> clean state both times; (4) a deliberately wrong action -> fail with the
expected failed check. Writes validation/results.json (read by make_tasks_md.py).

The oracle moves the real pointer and takes the foreground. Run it only when the desktop is free.

  validate.py [--tasks MB-01 MB-02 ...] [--round 0]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "probes" / "_common"))
import benchlab_common as C  # noqa: E402
import fixture as F  # noqa: E402

BENCHINPUT = str(F.DEFAULT_BUILD / "benchinput")
OX, OY = (
    60,
    112,
)  # BenchLab content origin on screen (outer top-left (60, 80) plus a 32 px title bar)
SX, SY = 860, 152  # Settings window content origin (outer top-left (860, 120) plus the title bar)
CANVAS = (30, 16)


def seed_for(task: str, rnd: int) -> int:
    return int(hashlib.sha256(f"{task}:{rnd}".encode()).hexdigest()[:8], 16) % 1_000_000


def inp(*a: Any) -> None:
    subprocess.run([BENCHINPUT, *map(str, a)], check=True)


def G(x: float, y: float) -> tuple[float, float]:
    return OX + x, OY + y


def click(x: float, y: float, button: str = "left", count: int = 1) -> None:
    inp("click", x, y, button, count)


def cclick(x: float, y: float, button: str = "left", count: int = 1) -> None:
    gx, gy = G(x, y)
    click(gx, gy, button, count)


CURRENT_DIR: Path | None = None


def focus_window() -> None:
    # Bring BenchLab to the front (other windows on this shared desktop may cover it), then click its
    # title bar so it is key.
    if CURRENT_DIR is not None:
        inp("activate", (CURRENT_DIR / "lab.pid").read_text())
    time.sleep(0.3)
    click(OX + 500, OY - 16)
    time.sleep(0.4)


def state(d: Path) -> dict[str, Any]:
    return json.loads(F.paths(d)["state"].read_text("utf-8"))


def settle(s: float = 0.5) -> None:
    time.sleep(s)


class Ctx:
    def __init__(self, task: str, seed: int, d: Path) -> None:
        self.task, self.seed, self.d = task, seed, d


# ------------------------------------------------------------------ per-task oracles and wrong actions


def canvas_xy(c: dict[str, Any]) -> tuple[float, float]:
    return c["cx"] + CANVAS[0], c["cy"] + CANVAS[1]


def click_done(x: float = 80, y: float = 467) -> None:
    cclick(x, y)


def mb01(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_canvasclick(ctx.seed)
    by = {c["label"]: c for c in p["circles"]}
    seq = list(p["left_sequence"])
    if wrong:
        seq[0], seq[1] = seq[1], seq[0]
    focus_window()
    for label in seq:
        cclick(*canvas_xy(by[label]))
    click_done()


def pick_menu(index: int) -> None:
    # Type-ahead on the first letter (the five titles start with different letters), then Return. Arrow
    # keys are not reliable here: a pointer resting on the first item may or may not have highlighted it.
    settle(0.9)
    inp("type", C.MENU_ACTIONS[index][0].lower())
    settle(0.2)
    inp("key", "return")


def mb02(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_canvasmenu(ctx.seed)
    by = {c["label"]: c for c in p["circles"]}
    focus_window()
    for n, (label, action) in enumerate(p["expected"].items()):
        idx = C.MENU_ACTIONS.index(action)
        if wrong and n == 1:
            idx = (idx + 1) % len(C.MENU_ACTIONS)
        cclick(*canvas_xy(by[label]), "right")
        pick_menu(idx)
        settle(0.3)
    click_done()


def mb03(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_tablesel(ctx.seed)
    row = p["row"] - (1 if wrong else 0)
    focus_window()
    cclick(76, 56)  # first row
    settle(0.3)
    inp("key", "down", "repeat", row - 1)
    settle(0.5)
    cclick(66, 465)  # Confirm


def mb04(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_canvas(ctx.seed)
    zones = {z["name"]: z for z in p["zones"]}
    tiles = {t["name"]: t for t in p["tiles"]}
    focus_window()
    order = ["red", "green", "blue"]
    for i, name in enumerate(order):
        t = tiles[name]
        target = zones[order[(i + 1) % 3] if (wrong and i < 2) else name]
        sx, sy = CANVAS[0] + t["x"] + 28, CANVAS[1] + t["y"] + 28
        ex, ey = CANVAS[0] + target["x"] + 48, CANVAS[1] + target["y"] + 48
        gx, gy = G(sx, sy)
        hx, hy = G(ex, ey)
        inp("drag", gx, gy, hx, hy, 24, 600)
        settle(0.3)
    click_done()


def mb05(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_forms(ctx.seed)
    focus_window()
    cclick(360, 38)
    inp("type", p["customer_name"])
    cclick(270, 82)
    inp("type", p["invoice_amount"])
    cat = p["category_index"] if not wrong else (p["category_index"] % 7) + 1
    cclick(300, 126)
    settle(0.5)
    inp("key", "down", "repeat", cat)
    inp("key", "return")
    settle(0.3)
    prio = C.PRIORITIES.index(p["priority"])
    cclick(190 + 92 * prio + 12, 169)
    if p["notify"]:
        cclick(202, 213)
    cclick(225, 258)
    inp("key", "a", "cmd")
    inp("type", str(p["quantity"]))
    cclick(430, 345)
    inp("type", p["notes"])
    cclick(240, 433)


def mb06(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_richtext(ctx.seed)
    focus_window()
    cclick(380, 150)
    inp("type", p["paragraph"])
    settle(0.3)
    sents = p["sentences"]
    bi = p["bold_index"] if not wrong else (p["bold_index"] + 1) % 3
    start = sum(len(s) + 1 for s in sents[:bi])
    inp("key", "up", "cmd")
    inp("key", "right", "repeat", start)
    inp("key", "right", "shift", "repeat", len(sents[bi]))
    cclick(70, 30)  # Bold button
    settle(0.3)
    # replace the word
    ri = p["replace_index"]
    s = sents[ri]
    off = sum(len(x) + 1 for x in sents[:ri]) + s.index(p["old_word"])
    inp("key", "up", "cmd")
    inp("key", "right", "repeat", off)
    inp("key", "right", "shift", "repeat", len(p["old_word"]))
    inp("type", p["new_word"])
    settle(0.3)
    cclick(80, 391)  # Done


def activate_by_title() -> None:
    focus_window()


def mb07(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_clipboard(ctx.seed)
    subprocess.run(["open", "-a", "Calculator"], check=True)
    time.sleep(2.0)
    inp("key", "escape")  # clear (Esc is Clear in Calculator)
    expr = f"{p['a']}*{p['b']}+{p['c']}"
    inp("type", expr)
    inp("key", "return")
    settle(0.4)
    inp("key", "c", "cmd")
    settle(0.3)
    activate_by_title()
    cclick(260, 42)
    inp("key", "v", "cmd")
    if wrong:
        inp("type", "1")
    settle(0.3)
    cclick(170, 89)


def mb08(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_settings(ctx.seed)
    focus_window()
    inp("key", "comma", "cmd")
    settle(1.0)
    click(SX + 600 - 345, SY + 38)  # first click makes the Settings window key
    settle(0.3)
    click(SX + 255, SY + 38)
    settle(0.2)
    inp("key", "a", "cmd")
    inp("type", p["name"])
    theme = p["theme"]
    if wrong:
        theme = C.THEME_TARGETS[(C.THEME_TARGETS.index(theme) + 1) % 3]
    click(SX + 220, SY + 83)
    settle(0.5)
    inp("key", "down", "repeat", C.THEME_CHOICES.index(theme))
    inp("key", "return")
    settle(0.3)
    if p["compact_target"] != p["compact_default"]:
        click(SX + 152, SY + 129)
    click(SX + 190, SY + 183)
    settle(0.5)
    st = state(ctx.d)
    ticket = st.get("ticket_shown")
    click(OX + 500, OY - 16)  # back to the main window
    settle(0.4)
    click(OX + 230, OY + 259)
    inp("type", str(ticket))
    click(OX + 376, OY + 259)


def row_y(i: int) -> float:
    return 46 + 1 + 28 + 36 * i + 18


def mb09(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_listdrag(ctx.seed)
    target = list(p["target"])
    if wrong:
        target[0], target[1] = target[1], target[0]
    focus_window()
    for pos in range(5):
        order = state(ctx.d)["order"]
        if order[pos] == target[pos]:
            continue
        src = order.index(target[pos])
        gx, gy = G(230, row_y(src))
        hx, hy = G(230, row_y(pos) - 12)  # upper half of the destination row: drop above it
        inp("drag", gx, gy, hx, hy, 24, 700)
        settle(0.6)
    click_done(80, 331)


def mb10(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_hover(ctx.seed)
    idx = p["target_index"]
    if wrong:
        idx = (idx + 1) % 4
    focus_window()
    hx, hy = G(103, 46)
    inp("hover", hx, hy, 0.6)
    bx, by = G(28 + 112 * idx + 52, 96)
    inp("move", bx, by)
    settle(0.2)
    click(bx, by)


def mb11(ctx: Ctx, wrong: bool = False) -> None:
    p = C.derive_tooltip(ctx.seed)
    idx = p["target_index"]
    if wrong:
        idx = (idx + 1) % 4
    focus_window()
    hx, hy = G(125 + 165 * idx, 95)
    inp("hover", hx, hy, 3.5)
    code = p["codes"][idx]
    cclick(210, 222)
    inp("type", code)
    cclick(386, 222)


def mb12_foreground(ctx: Ctx, wrong: bool = False) -> None:
    # Disturbing run: the oracle foregrounds BenchLab and moves the real pointer.
    mb05_fields(ctx)


def mb05_fields(ctx: Ctx) -> None:
    p = C.derive_forms(ctx.seed)
    focus_window()
    cclick(360, 38)
    inp("type", p["customer_name"])
    cclick(300, 126)
    settle(0.5)
    inp("key", "down", "repeat", p["category_index"])
    inp("key", "return")
    settle(0.3)
    cclick(190 + 92 * C.PRIORITIES.index(p["priority"]) + 12, 169)
    cclick(240, 433)


ORACLES: dict[str, tuple[Callable[..., None], set[str]]] = {
    "MB-01": (mb01, {"left_sequence_in_order", "no_extra_left_clicks"}),
    "MB-02": (mb02, {"target_2_action"}),
    "MB-03": (mb03, {"selected_target_at_confirm", "no_wrong_confirm"}),
    "MB-04": (mb04, {"red_in_zone", "green_in_zone"}),
    "MB-05": (mb05, {"category"}),
    "MB-06": (mb06, {"bold_covers_sentence", "no_bold_elsewhere"}),
    "MB-07": (mb07, {"result_correct"}),
    "MB-08": (mb08, {"applied_theme", "ticket_correct"}),
    "MB-09": (mb09, {"order_correct"}),
    "MB-10": (mb10, {"clicked_target", "no_wrong_clicks"}),
    "MB-11": (mb11, {"code_correct", "tooltip_displayed_for_target"}),
}


# ------------------------------------------------------------------ driver


def failed(res: dict[str, Any]) -> list[str]:
    return sorted(k for k, v in res["checks"].items() if not v["passed"])


def strip_volatile(s: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in s.items() if k not in ("pid", "updated_ms")}


def run_task(task: str, rnd: int, out: Path) -> dict[str, Any]:
    seed = seed_for(task, rnd)
    d = out / task
    global CURRENT_DIR
    CURRENT_DIR = d
    r: dict[str, Any] = {"seed": seed}
    ctx = Ctx(task, seed, d)

    # (1) no-op
    su = F.setup(task, seed, d)
    r["setup_s"] = su["seconds"]
    res = F.check(task, seed, d)
    r["noop"] = "FAIL (as required)" if not res["passed"] else "PASSED (BUG)"
    r["noop_failed_checks"] = failed(res)
    r["check_s"] = res["check_seconds"]
    r["noop_ok"] = not res["passed"]
    initial = strip_volatile(state(d))
    rs = F.reset(task, d)
    r["reset_s"] = rs["seconds"]

    # (3) reset clean twice: state after a second setup equals the first, nothing left behind
    clean = []
    for _ in range(2):
        F.setup(task, seed, d)
        s2 = strip_volatile(state(d))
        ev = F.paths(d)["events"].read_text("utf-8").splitlines()
        clean.append(s2 == initial and len(ev) == 1)
        F.reset(task, d)
        left = subprocess.run(
            ["pgrep", "-f", "BenchLab.app/Contents/MacOS/BenchLab"], capture_output=True, text=True
        ).stdout.strip()
        clean.append(
            left == "" and not F.paths(d)["state"].exists() and not F.paths(d)["events"].exists()
        )
    r["reset_clean"] = all(clean)
    r["reset"] = "clean twice" if all(clean) else f"NOT clean {clean}"

    if task in ORACLES:
        fn, expect = ORACLES[task]
        # (2) oracle (retried up to 3 times: the desktop is shared during the build phase and another
        # process moving the pointer or the focus breaks a real-input oracle; attempts are recorded)
        attempts = 0
        while True:
            attempts += 1
            F.setup(task, seed, d)
            t0 = time.monotonic()
            fn(ctx)
            settle(0.8)
            res = F.check(task, seed, d)
            if res["passed"] or attempts >= 3:
                break
            r.setdefault("oracle_failed_attempts", []).append(failed(res))
            F.reset(task, d)
        r["oracle_attempts"] = attempts
        r["oracle_s"] = round(time.monotonic() - t0, 1)
        r["oracle_ok"] = bool(res["passed"])
        r["oracle"] = "PASS" if res["passed"] else f"FAIL {failed(res)}"
        r["oracle_score"] = res["score"]
        r["oracle_diag"] = {k: v["value"] for k, v in res.get("diagnostics", {}).items()}
        F.reset(task, d)
        # (4) wrong action
        F.setup(task, seed, d)
        fn(ctx, wrong=True)
        settle(0.8)
        res = F.check(task, seed, d)
        got = set(failed(res))
        r["wrong_ok"] = (not res["passed"]) and expect <= got
        r["wrong"] = (
            f"FAIL as required, failed {sorted(got)}" if not res["passed"] else "PASSED (BUG)"
        )
        r["wrong_expected"] = sorted(expect)
        F.reset(task, d)
    r["timing"] = f"setup {r['setup_s']} s, check {r['check_s']} s, reset {r['reset_s']} s"
    return r


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", nargs="*", default=sorted(ORACLES))
    ap.add_argument("--round", type=int, default=0)
    ap.add_argument("--out", default="/tmp/mbv/val")
    a = ap.parse_args()
    out = Path(a.out)
    res_path = ROOT / "validation" / "results.json"
    res_path.parent.mkdir(exist_ok=True)
    data = json.loads(res_path.read_text()) if res_path.exists() else {"tasks": {}}
    for task in a.tasks:
        print(f"== {task}", flush=True)
        data["tasks"][task] = run_task(task, a.round, out)
        print(json.dumps(data["tasks"][task], indent=1), flush=True)
        res_path.write_text(json.dumps(data, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
