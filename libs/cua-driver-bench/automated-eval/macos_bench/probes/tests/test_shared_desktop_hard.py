# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""PA-02 and UM-02, the hard variants (SHARED-DESKTOP.md, "Hard variants"): derivations, the Confirm-step and
fragment-attribution checker, per-copy deadlines, the busy user's plan and drag accounting, and the wiring. Model
free, on synthetic logs shaped like the BenchLab desk mode and the sentinel write them: a scripted oracle passes; a
no-op, cross-talk, a premature or pending Confirm, a late copy, a displaced pointer, a lost click and a cut drag are
caught. PA-01 and UM-01 are checked unchanged."""

from __future__ import annotations

import json
import math
import re
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
BENCH = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "swift"))
import bench_v2 as v2  # noqa: E402
import shared_desktop as SD  # noqa: E402
import summarize_sentinel  # noqa: E402
import synth as S  # noqa: E402
from synth import C  # noqa: E402

SEEDS = [0, 1, 42, 517139, 999_999]
PA2 = S.load_module("PA-02", "evaluate.py")
UM2 = S.load_module("UM-02", "evaluate.py")
CODE = re.compile(r"^[A-Z]{3}-\d{4}-[a-zA-Z0-9]{5}[#@%+&]$")


class ConfirmDeskSim(S.Sim):
    """A desk started with --confirm, as the app writes it."""

    def __init__(self, seed: int, label: str, t0: float = S.T0) -> None:
        self.label = label
        self.mode, self.seed = "desk", seed
        self.seq, self.t, self.events = 0, t0, []
        self.code, self.shelf, self.submits, self.pending = "", None, 0, False
        self.emit("app_start", mode="desk", seed=seed, instance=label)

    def type_text(self, text: str) -> None:
        for ch in text:
            self.code += ch
            self.emit("field_edit", field="code", value=self.code)

    def clear(self) -> None:
        self.code = ""
        self.emit("field_edit", field="code", value="")

    def pick(self, shelf: str) -> None:
        self.shelf = shelf
        self.emit("shelf_select", shelf=shelf)

    def press_submit(self) -> None:
        self.pending = True
        self.emit("confirm_open", code=self.code, shelf=self.shelf)

    def confirm(self) -> None:
        self.pending = False
        self.submits += 1
        self.emit("submit", code=self.code, shelf=self.shelf)

    def cancel(self) -> None:
        self.pending = False
        self.emit("confirm_cancel")

    def state(self) -> tuple[dict[str, Any], list[dict[str, Any]]]:
        return self.finish(
            {
                "instance": self.label,
                "code": self.code,
                "shelf": self.shelf,
                "submit_count": self.submits,
                "submitted": {"code": self.code, "shelf": self.shelf} if self.submits else None,
                "confirm_pending": self.pending,
            }
        )


def fill(sim: ConfirmDeskSim, cell: dict[str, Any]) -> ConfirmDeskSim:
    sim.type_text(cell["code"])
    sim.pick(cell["shelf"])
    sim.press_submit()
    sim.confirm()
    return sim


def oracle(seed: int, cell: dict[str, Any], t0: float = S.T0) -> ConfirmDeskSim:
    return fill(ConfirmDeskSim(seed, cell["label"], t0), cell)


def write_desk(root: Path, sim: ConfirmDeskSim) -> None:
    state, events = sim.state()
    d = SD.desk_dir(root, sim.label)
    d.mkdir(parents=True, exist_ok=True)
    (d / "state.json").write_text(json.dumps(state))
    (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))


def run_pa2(seed: int, n: int, sims: list[ConfirmDeskSim], t0: float = S.T0) -> dict:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for sim in sims:
            write_desk(root, sim)
        SD.write_meta(root, n, t0, C.PA2_BUDGET_S, copy_deadline_s=C.PA2_COPY_DEADLINE_S)
        out = root / "result.json"
        PA2.main(["--seed", str(seed), "--dir", str(root), "--result", str(out)])
        return json.loads(out.read_text())


def plain_desk_files(root: Path, seed: int, cell: dict[str, Any]) -> tuple[dict, list]:
    """A desk without --confirm (PA-01 / UM-01), as the app writes it."""
    sim = S.Sim("desk", seed)
    sim.events[0]["details"]["instance"] = cell["label"]
    code = ""
    for ch in cell["code"]:
        code += ch
        sim.emit("field_edit", field="code", value=code)
    sim.emit("shelf_select", shelf=cell["shelf"])
    sim.emit("submit", code=code, shelf=cell["shelf"])
    return sim.finish(
        {"instance": cell["label"], "code": code, "shelf": cell["shelf"], "submit_count": 1,
         "submitted": {"code": code, "shelf": cell["shelf"]}}
    )  # fmt: skip


class Derivations(unittest.TestCase):
    def test_cells_stable_across_n_distinct_and_attributable(self) -> None:
        for seed in SEEDS:
            sixteen = C.derive_crowded(seed, 16)["cells"]
            self.assertEqual(sixteen[:8], C.derive_crowded(seed, 8)["cells"])
            self.assertEqual(sixteen[:1], C.derive_crowded(seed, 1)["cells"])
            self.assertEqual(len({c["label"] for c in sixteen}), 16)
            self.assertEqual(len({c["tag"] for c in sixteen}), 16)
            self.assertEqual({c["title"] for c in sixteen}, {"BenchLab Desk"})
            seen: set[str] = set()
            for c in sixteen:
                self.assertRegex(c["code"], CODE)
                self.assertTrue(c["code"].startswith(c["tag"] + "-"))
                frags = C._fragments(c["code"])
                self.assertFalse(frags & seen, "no fragment is shared between two copies' codes")
                seen |= frags
                self.assertIn(c["shelf"], C.DESK_SHELVES)
            self.assertNotEqual(sixteen, C.derive_crowded(seed + 1, 16)["cells"])

    def test_cascade_overlaps_and_fits(self) -> None:
        cells = C.derive_crowded(7, 16)["cells"]
        for c in cells:
            x, y, w, h = c["frame"]
            self.assertGreaterEqual(x, 0)
            self.assertLessEqual(x + w, C.PA2_WITNESS_FRAME[0], "the witness strip is free")
            self.assertLessEqual(y + h, C.PA2_MIN_SCREEN[1])
        by_pos = {(c["row"], c["col"]): c["frame"] for c in cells}
        for row in (1, 2, 3):
            for col in range(1, 5):
                upper, lower = by_pos[(row, col)], by_pos[(row + 1, col)]
                self.assertLess(lower[1], upper[1] + upper[3], "a row overlaps the one above")
                # Submit sits at about window-y 290 in the desk layout: the next row hides it
                self.assertLess(lower[1] - upper[1], 290)
        self.assertGreaterEqual(C.PA2_PITCH[0], C.DESK_SIZE[0], "no horizontal overlap")

    def test_a_coordinate_click_misses_in_the_cascade_not_in_the_grid(self) -> None:
        """Why the cascade presses on a driver that clicks screen coordinates: the Submit button of 12 of 16
        copies sits under another copy's window, while in PA-01's grid every Submit is in front of its own."""
        submit_y = 300  # window-relative y of Submit (about 290 to 320 in the three desk layouts)
        submit_x = 70

        def owner_of(frames: list[tuple[int, int, int, int]], k: int) -> int:
            x, y = frames[k][0] + submit_x, frames[k][1] + submit_y
            hit = k
            for j, (fx, fy, fw, fh) in enumerate(
                frames
            ):  # later cells are opened later, so they are on top
                if fx <= x < fx + fw and fy <= y < fy + fh:
                    hit = j
            return hit

        crowded = [tuple(c["frame"]) for c in C.derive_crowded(7, 16)["cells"]]
        grid = [tuple(c["frame"]) for c in C.derive_parallel(7, 10)["cells"]]
        self.assertEqual(sum(owner_of(crowded, k) != k for k in range(16)), 12)
        self.assertEqual(sum(owner_of(grid, k) != k for k in range(10)), 0)

    def test_pa01_and_um01_unchanged(self) -> None:
        um = C.derive_mouse_user(42)
        self.assertEqual(um["schema"], "cdb-mouse-user/1")
        self.assertNotIn("plan", um)
        self.assertEqual(
            (
                um["step_ms"],
                um["step_px"],
                um["dwell_before_ms"],
                um["press_ms"],
                um["dwell_after_ms"],
            ),
            (25, 9, 150, 60, 250),
        )
        self.assertEqual(C.derive_mouse_user(42, "um01"), um)
        ten = C.derive_parallel(42, 10)["cells"]
        self.assertEqual(len(ten), 10)
        self.assertTrue(all("tag" not in c for c in ten))
        self.assertEqual(SD.instances_for({"id": "PA-01", "instances": 10}), 10)
        with self.assertRaises(ValueError):
            SD.instances_for({"id": "PA-01"}, 11)

    def test_briefs(self) -> None:
        render = SD._renderer("PA-02")
        for seed in SEEDS:
            for n in (1, 8, 16):
                cells = C.derive_crowded(seed, n)["cells"]
                for k, brief in enumerate(render.render_all(seed, n)):
                    c = cells[k]
                    self.assertIn(c["code"], brief)
                    self.assertIn(f'"Desk {c["label"]}"', brief)
                    self.assertIn(c["position"], brief)
                    self.assertIn(c["shelf"], brief)
                    self.assertNotIn("{{", brief)
                    for other in cells:
                        if other is not c:
                            self.assertNotIn(other["code"], brief)
        c = C.derive_busy_desk(5)
        self.assertIn(c["code"], SD._renderer("UM-02").render(5))
        self.assertEqual(c["frame"], list(C.UM2_DESK_FRAME))
        self.assertLessEqual(v2.USER_WINDOW_FRAME[0] - C.UM2_DESK_FRAME[0], 120)

    def test_mouse_plan(self) -> None:
        for seed in SEEDS:
            m = C.derive_mouse_user(seed, "um02")
            self.assertEqual(m["schema"], "cdb-mouse-user/2")
            self.assertEqual((m["step_ms"], m["drag_step_ms"], m["step_px"]), (8, 8, 12))
            plan = m["plan"]
            self.assertGreaterEqual(sum(1 for leg in plan if leg["kind"] == "drag"), 4)
            for j, leg in enumerate(plan):
                self.assertEqual(
                    leg["kind"] == "drag", j % C.UM2_DRAG_EVERY == C.UM2_DRAG_EVERY - 1
                )
                if leg["kind"] == "drag":
                    self.assertNotEqual(leg["from"], leg["to"])
            ends = [leg["to"] if leg["kind"] == "drag" else leg["dot"] for leg in plan]
            starts = [leg["from"] if leg["kind"] == "drag" else leg["dot"] for leg in plan]
            self.assertTrue(all(s != e for s, e in zip(starts[1:], ends[:-1])))
            slow = C.derive_mouse_user(seed)
            self.assertEqual(m["dots"], slow["dots"])
            self.assertGreater(slow["step_ms"] / m["step_ms"], 3)


class CrowdedChecker(unittest.TestCase):
    def cells(self, seed: int, n: int) -> list[dict[str, Any]]:
        return C.derive_crowded(seed, n)["cells"]

    def test_oracles_pass(self) -> None:
        for n in (1, 8, 16):
            for seed in SEEDS[:3]:
                r = run_pa2(seed, n, [oracle(seed, c) for c in self.cells(seed, n)])
                self.assertTrue(r["passed"], r)
                self.assertEqual(r["diagnostics"]["parallel"]["value"]["cross_talk"], 0)
                self.assertEqual(r["diagnostics"]["instances_passed"]["value"], n)

    def test_noop_fails(self) -> None:
        seed = 42
        r = run_pa2(seed, 16, [ConfirmDeskSim(seed, c["label"]) for c in self.cells(seed, 16)])
        self.assertFalse(r["passed"])
        self.assertEqual(r["diagnostics"]["instances_passed"]["value"], 0)

    def test_cross_talk_caught_by_tag_and_fragment(self) -> None:
        seed, cells = 42, self.cells(42, 16)
        for leak in (cells[1]["tag"], cells[1]["code"][4:9], cells[1]["code"]):
            sims = [oracle(seed, c) for c in cells]
            bad = ConfirmDeskSim(seed, cells[0]["label"])
            bad.type_text(leak)  # another copy's text reaches copy 0 ...
            bad.clear()  # ... and is corrected
            fill(bad, cells[0])
            sims[0] = bad
            r = run_pa2(seed, 16, sims)
            value = r["diagnostics"]["parallel"]["value"]
            self.assertEqual(value["cross_talk"], 1, leak)
            self.assertEqual(value["cross_talk_labels"], [cells[0]["label"]])
            self.assertFalse(r["passed"], "a corrected leak is still cross-talk")

    def test_own_fragments_and_bare_digits_are_not_cross_talk(self) -> None:
        seed, cells = 42, self.cells(42, 16)
        sims = [oracle(seed, c) for c in cells]
        bad = ConfirmDeskSim(seed, cells[0]["label"])
        bad.type_text("1234")  # digits alone are not attributable
        bad.clear()
        bad.type_text(cells[0]["code"][:6])
        bad.clear()
        fill(bad, cells[0])
        sims[0] = bad
        self.assertTrue(run_pa2(seed, 16, sims)["passed"])

    def test_premature_confirm_fails_that_copy(self) -> None:
        seed, cells = 42, self.cells(42, 16)
        sims = [oracle(seed, c) for c in cells]
        early = ConfirmDeskSim(seed, cells[3]["label"])  # another agent pressed Confirm too early
        early.press_submit()
        early.confirm()
        fill(early, cells[3])
        sims[3] = early
        r = run_pa2(seed, 16, sims)
        self.assertFalse(r["passed"])
        self.assertEqual(
            [k for k, v in r["instances"].items() if not v["passed"]], [cells[3]["label"]]
        )
        self.assertFalse(r["instances"][cells[3]["label"]]["checks"]["submitted_once"]["passed"])

    def test_pending_and_missing_confirm_fail(self) -> None:
        seed, cells = 42, self.cells(42, 8)
        sims = [oracle(seed, c) for c in cells]
        pend = ConfirmDeskSim(seed, cells[2]["label"])
        pend.type_text(cells[2]["code"])
        pend.pick(cells[2]["shelf"])
        pend.press_submit()  # never confirmed
        sims[2] = pend
        checks = run_pa2(seed, 8, sims)["instances"][cells[2]["label"]]["checks"]
        self.assertFalse(checks["confirm_resolved"]["passed"])
        self.assertFalse(checks["submitted_once"]["passed"])
        sims = [oracle(seed, c) for c in cells]
        odd = ConfirmDeskSim(
            seed, cells[4]["label"]
        )  # a submit event with no confirm_open (the app never writes one)
        odd.type_text(cells[4]["code"])
        odd.pick(cells[4]["shelf"])
        odd.confirm()
        sims[4] = odd
        checks = run_pa2(seed, 8, sims)["instances"][cells[4]["label"]]["checks"]
        self.assertFalse(checks["confirm_before_submit"]["passed"])

    def test_cancel_then_resubmit_is_fine(self) -> None:
        seed, cells = 42, self.cells(42, 8)
        sims = [oracle(seed, c) for c in cells]
        sim = ConfirmDeskSim(seed, cells[5]["label"])
        sim.type_text(cells[5]["code"])
        sim.pick(cells[5]["shelf"])
        sim.press_submit()
        sim.cancel()
        sim.press_submit()
        sim.confirm()
        sims[5] = sim
        self.assertTrue(run_pa2(seed, 8, sims)["passed"])

    def test_copy_deadline(self) -> None:
        seed, cells = 42, self.cells(42, 8)
        for offset_ms, ok in (
            (170_000, True),
            (185_000, False),
        ):  # the copy deadline is 180 s, the budget 240 s
            sims = [oracle(seed, c) for c in cells]
            sims[6] = oracle(seed, cells[6], t0=S.T0 + offset_ms)
            r = run_pa2(seed, 8, sims)
            self.assertEqual(r["passed"], ok, offset_ms)
            if not ok:
                self.assertFalse(r["instances"][cells[6]["label"]]["checks"]["in_budget"]["passed"])

    def test_missing_copy_is_harness_error(self) -> None:
        seed, cells = 42, self.cells(42, 4)
        self.assertIn("harness_error", run_pa2(seed, 4, [oracle(seed, c) for c in cells[:3]]))

    def test_pa01_checker_keeps_its_rules(self) -> None:
        """PA-01's evaluator, unchanged: letters, no Confirm checks."""
        pa = S.load_module("PA-01", "evaluate.py")
        seed = 42
        cells = C.derive_parallel(seed, 4)["cells"]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for cell in cells:
                state, events = plain_desk_files(root, seed, cell)
                d = SD.desk_dir(root, cell["label"])
                d.mkdir(parents=True)
                (d / "state.json").write_text(json.dumps(state))
                (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
            SD.write_meta(root, 4, S.T0, 360)
            out = root / "r.json"
            pa.main(["--seed", str(seed), "--dir", str(root), "--result", str(out)])
            r = json.loads(out.read_text())
        self.assertTrue(r["passed"], r)
        self.assertNotIn("confirm_resolved", r["instances"][cells[0]["label"]]["checks"])


# ------------------------------------------------------------------ busy user


def busy_log(
    seed: int,
    path: Path,
    legs: int = 6,
    displace_at: int | None = None,
    lose_action: int | None = None,
    cut_drag: int | None = None,
    foreign_moves: int = 0,
    tagging: bool = True,
) -> None:
    """A sentinel log as the busy mouse user writes it (scripted oracle), with optional disturbances."""
    m = C.derive_mouse_user(seed, "um02")
    t = S.T0
    lines: list[dict[str, Any]] = []
    own = True if tagging else None

    def emit(ev: str, **kw: Any) -> None:
        nonlocal t
        t += m["step_ms"]
        lines.append({"t": t, "ev": ev, "armed": True, **kw})

    front = {"bid": "ai.cua.benchsentinel", "pid": 1}
    lines.append({"t": t, "ev": "armed", "armed": True, "mouse": [0, 0], "front": front})
    emit("mu_start", tagging=tagging, tolerance_px=m["tolerance_px"], platform="test",
         click_timeout_ms=m["click_timeout_ms"], drag_timeout_ms=m["drag_timeout_ms"])  # fmt: skip
    cur, prev = tuple(m["home"]), tuple(m["home"])
    i = n = 0

    def walk(to: tuple[int, int], step_px: float) -> None:
        nonlocal cur, prev, i
        for p in C.mouse_leg(cur, to, step_px):
            got = cur
            if displace_at is not None and i == displace_at:
                got = (cur[0] + 140, cur[1] + 60)
            d = round(min(math.dist(got, cur), math.dist(got, prev)), 2)
            emit("mu", i=i, want=list(cur), got=list(got), d=d)
            for _ in range(foreign_moves if i == 3 else 0):
                emit("mu_foreign", kind="move")
            prev, cur = cur, p
            i += 1

    for j in range(legs):
        leg = m["plan"][j % len(m["plan"])]
        a = m["dots"][leg["dot"] if leg["kind"] == "click" else leg["from"]]
        walk((a["x"], a["y"]), m["step_px"])
        emit("mu", i=i, want=list(cur), got=list(cur), d=0.0)
        i += 1
        n += 1
        if leg["kind"] == "click":
            emit("mu_click", n=n, dot=a["index"], at=[a["x"], a["y"]])
            if lose_action == n:
                emit("mu_click_result", n=n, ok=False, why="lost")
            else:
                emit("mu_rx", dot=a["index"], at=[a["x"], a["y"]], own=own)
                emit("mu_click_result", n=n, ok=True, why="landed")
            continue
        b = m["dots"][leg["to"]]
        walk((b["x"], b["y"]), m["drag_step_px"])
        emit("mu_drag", n=n, **{"from": a["index"], "to": b["index"]},
             at_from=[a["x"], a["y"]], at_to=[b["x"], b["y"]])  # fmt: skip
        if lose_action == n:
            emit("mu_click_result", n=n, ok=False, why="lost", kind="drag")
            continue
        emit("mu_rx", dot=a["index"], at=[a["x"], a["y"]], own=own)
        if cut_drag == n:
            emit("mu_click_result", n=n, ok=False, why="cut", kind="drag")
        else:
            emit("mu_up", dot=b["index"], at=[b["x"], b["y"]], own=own)
            emit("mu_click_result", n=n, ok=True, why="landed", kind="drag")
    idle = {
        "front": front,
        "key": True,
        "clip": 0,
        "onspace": True,
        "sheet": False,
        "above": [],
        "above_n": 0,
    }
    for _ in range(12):
        emit("s", mouse=list(cur), **idle)
    emit("mu_stop")
    emit(
        "disarmed",
        mouse=list(cur),
        front=front,
        clip=0,
        onspace=True,
        sheet=False,
        above=[],
        above_n=0,
    )
    path.write_text("".join(json.dumps(ln) + "\n" for ln in lines))


def busy_summary(**kw: Any) -> dict[str, Any]:
    with tempfile.TemporaryDirectory() as tmp:
        log = Path(tmp) / "sentinel.jsonl"
        busy_log(kw.pop("seed", 42), log, **kw)
        return summarize_sentinel.summarize(str(log))


def disturbance_of(s: dict[str, Any]) -> dict[str, Any]:
    keys = ("key_loss", "windows_raised", "keystrokes_leaked", "clicks_leaked", "scrolls_leaked",
            "pointer_max_deviation_px", "clipboard_changes", "space_switches", "sheets_shown",
            "front_changes", "activations_lost", "activations_gained", "user_activity", "mouse_user")  # fmt: skip
    return {k: s.get(k) for k in keys}


class BusyUser(unittest.TestCase):
    def test_scripted_busy_user_undisturbed(self) -> None:
        s = busy_summary(legs=9)
        mu = s["mouse_user"]
        self.assertEqual((mu["clicks"], mu["clicks_landed"]), (9, 9))
        self.assertEqual((mu["drags"], mu["drags_landed"], mu["drags_cut"]), (3, 3, 0))
        self.assertEqual(mu["max_displacement_px"], 0.0)
        self.assertFalse(mu["disturbed"])
        crit = v2.multiplayer_criteria(disturbance_of(s))
        self.assertTrue(crit["pointer"] and crit["typing"])
        mp = v2.multiplayer_score(disturbance_of(s), True, True, "background")
        self.assertTrue(mp["background_completion"], mp)

    def test_cut_drag_caught(self) -> None:
        s = busy_summary(legs=9, cut_drag=3)
        mu = s["mouse_user"]
        self.assertEqual((mu["drags"], mu["drags_landed"], mu["drags_cut"]), (3, 2, 1))
        self.assertEqual(mu["clicks_cut"], 1)
        self.assertTrue(mu["disturbed"])
        crit = v2.multiplayer_criteria(disturbance_of(s))
        self.assertFalse(crit["typing"])
        self.assertTrue(crit["pointer"])

    def test_lost_drag_and_lost_click_caught(self) -> None:
        s = busy_summary(legs=9, lose_action=3)  # action 3 is a drag
        self.assertEqual((s["mouse_user"]["drags"], s["mouse_user"]["clicks_lost"]), (3, 1))
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["typing"])
        s = busy_summary(legs=9, lose_action=2)
        self.assertEqual(s["mouse_user"]["drags_landed"], 3)
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["typing"])

    def test_displacement_during_a_drag_caught(self) -> None:
        s = busy_summary(legs=9, displace_at=60)
        self.assertGreater(s["mouse_user"]["max_displacement_px"], 3)
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["pointer"])

    def test_foreign_pointer_events_caught_when_tagged(self) -> None:
        s = busy_summary(foreign_moves=2)
        self.assertEqual(s["mouse_user"]["foreign_pointer_events"], 2)
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["pointer"])
        self.assertIsNone(
            busy_summary(foreign_moves=2, tagging=False)["mouse_user"]["foreign_pointer_events"]
        )

    def test_drag_in_flight_at_disarm_not_counted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "s.jsonl"
            busy_log(42, log, legs=6)
            lines = [json.loads(x) for x in log.read_text().splitlines()]
            end = lines[-1]["t"]
            lines.insert(
                -1, {"t": end - 100, "ev": "mu_drag", "armed": True, "n": 99, "from": 0, "to": 1}
            )
            log.write_text("".join(json.dumps(ln) + "\n" for ln in lines))
            mu = summarize_sentinel.summarize(str(log))["mouse_user"]
        self.assertEqual((mu["clicks"], mu["clicks_lost"], mu["drags"]), (6, 0, 2))

    def test_um01_summary_gains_only_zero_drag_counters(self) -> None:
        import test_shared_desktop as T

        mu = T.summary()["mouse_user"]
        self.assertEqual((mu["clicks"], mu["clicks_landed"], mu["clicks_lost"]), (6, 6, 0))
        self.assertEqual(
            (mu["drags"], mu["drags_landed"], mu["drags_cut"], mu["clicks_cut"]), (0, 0, 0, 0)
        )
        self.assertFalse(mu["disturbed"])

    def test_checker_and_harness_error(self) -> None:
        seed = 11
        cell = C.derive_busy_desk(seed)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            state, events = plain_desk_files(root, seed, cell)
            (root / "state.json").write_text(json.dumps(state))
            (root / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
            busy_log(seed, root / "s.jsonl", legs=20)
            argv = ["--seed", str(seed), "--state", str(root / "state.json"), "--events",
                    str(root / "events.jsonl"), "--result", str(root / "r.json"),
                    "--sentinel", str(root / "s.jsonl")]  # fmt: skip
            UM2.main(argv)
            r = json.loads((root / "r.json").read_text())
            self.assertTrue(r["passed"], r)
            self.assertNotIn("harness_error", r)
            busy_log(seed, root / "short.jsonl", legs=6)  # 6 actions: fewer than the 15 UM-02 needs
            argv[-1] = str(root / "short.jsonl")
            UM2.main(argv)
            self.assertIn("harness_error", json.loads((root / "r.json").read_text()))


class Wiring(unittest.TestCase):
    def test_selectors_and_helpers(self) -> None:
        self.assertEqual(v2.SHARED_DESKTOP_TASKS, ("PA-01", "UM-01", "PA-02", "UM-02"))
        for tid in ("PA-02", "UM-02"):
            spec = json.loads((BENCH / "probes" / tid / "task.json").read_text())
            self.assertEqual(v2.category_of(spec), "shared_desktop")
            self.assertEqual(spec["app_args"], ["--mode", "desk"])
            self.assertEqual(spec["id"], tid)
        pa = json.loads((BENCH / "probes" / "PA-02" / "task.json").read_text())
        self.assertTrue(SD.is_parallel(pa))
        self.assertEqual(
            (SD.instances_for(pa), SD.instances_for(pa, 8), SD.instances_for(pa, 16)), (16, 8, 16)
        )
        with self.assertRaises(ValueError):
            SD.instances_for(pa, 17)
        self.assertEqual(
            (pa["timeout_s"], pa["copy_deadline_s"]), (C.PA2_BUDGET_S, C.PA2_COPY_DEADLINE_S)
        )
        um = json.loads((BENCH / "probes" / "UM-02" / "task.json").read_text())
        self.assertTrue(SD.is_mouse_user(um))
        self.assertEqual(um["conditions"], ["background"])
        self.assertEqual(SD.witness_frame("PA-02"), C.PA2_WITNESS_FRAME)
        self.assertEqual(SD.witness_frame("PA-01"), SD.PA_WITNESS_FRAME)
        self.assertEqual((SD.budget_s("PA-02"), SD.budget_s("PA-01")), (240, 360))
        self.assertEqual((SD.mouse_profile("UM-02"), SD.mouse_profile("UM-01")), ("um02", "um01"))
        self.assertEqual((SD.max_instances("PA-02"), SD.max_instances("PA-01")), (16, 10))

    def test_pa_agents_plan_crowded(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            agents = SD.pa_agents(
                77, 16, Path(tmp), ("on your Mac", "on this Windows PC"), task="PA-02"
            )
            old = SD.pa_agents(77, 4, Path(tmp))
        self.assertEqual(len(agents), 16)
        self.assertEqual(agents[0]["app_args"][-3:], ["--title", "BenchLab Desk", "--confirm"])
        self.assertEqual({x["title"] for x in agents}, {"BenchLab Desk"})
        self.assertTrue(all("on this Windows PC" in x["brief"] for x in agents))
        self.assertNotIn("--confirm", old[0]["app_args"])  # PA-01's plan is what it was
        self.assertNotIn("--title", old[0]["app_args"])

    def test_um_app_args_and_script(self) -> None:
        self.assertEqual(SD.um_app_args(5, "UM-01")[-2:], ["--frame", "200,200,360,400"])
        self.assertEqual(SD.um_app_args(5, "UM-02")[-2:], ["--frame", "320,300,360,400"])
        with tempfile.TemporaryDirectory() as tmp:
            script = SD.write_mouse_script(5, Path(tmp) / "m.json", "UM-02")
            self.assertEqual(json.loads((Path(tmp) / "m.json").read_text()), script)
            self.assertEqual(script["schema"], "cdb-mouse-user/2")
            self.assertEqual(
                SD.write_mouse_script(5, Path(tmp) / "m1.json")["schema"], "cdb-mouse-user/1"
            )

    def test_evaluate_argv(self) -> None:
        argv = SD.evaluate_argv("py", 3, Path("/r"), Path("/r/x.json"), "PA-02")
        self.assertTrue(argv[1].endswith("probes/PA-02/evaluate.py"))
        plain = SD.evaluate_argv("py", 3, Path("/r"), Path("/r/x.json"))
        self.assertTrue(plain[1].endswith("probes/PA-01/evaluate.py"))

    def test_analysis_tables(self) -> None:
        sys.path.insert(0, str(BENCH / "tools"))
        import analyze_v2 as A

        def pa(n: int, run: int, passed: int, minutes: float) -> dict:
            return {"task": "PA-02", "arm": "x", "condition": "foreground", "category": "shared_desktop",
                    "run_index": run, "instances": n, "passed": passed == n, "trial_id": f"{n}-{run}",
                    "parallel": {"instances": n, "passed": passed, "cross_talk": 0,
                                 "makespan_s": minutes * 60, "throughput_per_min": passed / minutes}}  # fmt: skip

        rows = [pa(1, 0, 1, 0.5), pa(16, 0, 14, 2.0)]
        rows.append({"task": "UM-02", "arm": "x", "condition": "foreground", "category": "shared_desktop",
                     "run_index": 0, "passed": True, "trial_id": "um",
                     "disturbance": disturbance_of(busy_summary(legs=9, cut_drag=3))})  # fmt: skip
        sd = A.shared_desktop(rows)
        self.assertEqual(sd["PA-02"]["16"]["copies_passed_mean"], 14)
        self.assertEqual((sd["UM-02"]["user_drags"], sd["UM-02"]["user_drags_cut"]), (3, 1))
        text = A.markdown(A.analyze(rows))
        self.assertIn("crowded desk, overlapping copies (PA-02)", text)
        self.assertIn("(UM-02)", text)
        self.assertIn("1/3 drags cut", text)


if __name__ == "__main__":
    unittest.main()
