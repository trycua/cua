# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""PA-01 and UM-01 (SHARED-DESKTOP.md): derivations, the desk checker, the parallel checker, the mouse-user
summary and the undisturbed criteria, on synthetic logs shaped like the BenchLab desk mode and the sentinel
write them. Scripted oracles pass; a no-op, cross-talk, pointer displacement and a lost click are caught."""

from __future__ import annotations

import json
import math
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
PA = S.load_module("PA-01", "evaluate.py")
UM = S.load_module("UM-01", "evaluate.py")


class DeskSim(S.Sim):
    def __init__(self, seed: int, label: str, t0: float = S.T0) -> None:
        self.label = label
        self.mode, self.seed = "desk", seed
        self.seq, self.t, self.events = 0, t0, []
        self.code, self.shelf, self.submits = "", None, 0
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

    def submit(self) -> None:
        self.submits += 1
        self.emit("submit", code=self.code, shelf=self.shelf)

    def state(self) -> tuple[dict[str, Any], list[dict[str, Any]]]:
        return self.finish(
            {
                "instance": self.label,
                "code": self.code,
                "shelf": self.shelf,
                "submit_count": self.submits,
                "submitted": {"code": self.code, "shelf": self.shelf} if self.submits else None,
            }
        )


def oracle(seed: int, cell: dict[str, Any], t0: float = S.T0) -> DeskSim:
    sim = DeskSim(seed, cell["label"], t0)
    sim.type_text(cell["code"])
    sim.pick(cell["shelf"])
    sim.submit()
    return sim


def write_desk(root: Path, sim: DeskSim) -> None:
    state, events = sim.state()
    d = SD.desk_dir(root, sim.label)
    d.mkdir(parents=True, exist_ok=True)
    (d / "state.json").write_text(json.dumps(state))
    (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))


def run_pa(seed: int, n: int, sims: list[DeskSim], t0: float = S.T0, budget: float = 360) -> dict:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for sim in sims:
            write_desk(root, sim)
        SD.write_meta(root, n, t0, budget)
        out = root / "result.json"
        PA.main(["--seed", str(seed), "--dir", str(root), "--result", str(out)])
        return json.loads(out.read_text())


class Derivations(unittest.TestCase):
    def test_cells_stable_across_n_and_distinct(self) -> None:
        for seed in SEEDS:
            ten = C.derive_parallel(seed, 10)["cells"]
            four = C.derive_parallel(seed, 4)["cells"]
            self.assertEqual(ten[:4], four)
            self.assertEqual(len({c["label"] for c in ten}), 10)
            self.assertEqual(len({c["shelf"] for c in ten}), 10)
            letters = "".join(c["letters"] for c in ten)
            self.assertEqual(len(set(letters)), 20, "code letters are distinct across copies")
            self.assertTrue(all(ch in C.DESK_CODE_LETTERS for ch in letters))
            for c in ten:
                x, y, w, h = c["frame"]
                self.assertLessEqual(x + w, 1920)
                self.assertLessEqual(y + h, 1000)
            frames = [tuple(c["frame"]) for c in ten]
            for i, a in enumerate(frames):
                for b in frames[i + 1 :]:
                    overlap = (
                        a[0] < b[0] + b[2]
                        and b[0] < a[0] + a[2]
                        and a[1] < b[1] + b[3]
                        and b[1] < a[1] + a[3]
                    )
                    self.assertFalse(overlap)

    def test_seed_changes_layout(self) -> None:
        a = C.derive_parallel(1)["cells"]
        b = C.derive_parallel(2)["cells"]
        self.assertNotEqual([c["label"] for c in a], [c["label"] for c in b])

    def test_um_desk_partly_under_user_window(self) -> None:
        x, y, w, h = C.derive_um_desk(7)["frame"]
        ux, uy, uw, uh = v2.USER_WINDOW_FRAME
        self.assertTrue(x < ux + uw and ux < x + w and y < uy + uh and uy < y + h)
        self.assertLess(x, ux, "the left part (Code field, Submit) stays uncovered")

    def test_briefs_name_the_copy(self) -> None:
        render = S.load_module("PA-01", "render_brief.py")
        seed = 4242
        cells = C.derive_parallel(seed, 10)["cells"]
        briefs = render.render_all(seed, 10)
        for c, brief in zip(cells, briefs):
            self.assertIn(f'"BenchLab Desk {c["label"]}"', brief)
            self.assertIn(c["code"], brief)
            self.assertIn(f'"{c["shelf"]}"', brief)
            self.assertIn(c["position"], brief)
            self.assertIn("other 9 copies", brief)
            for o in cells:
                if o is not c:
                    self.assertNotIn(o["code"], brief)
        self.assertIn("One BenchLab window", render.render_cell(seed, 0, 1))
        um = S.load_module("UM-01", "render_brief.py").render(seed)
        self.assertIn(C.derive_um_desk(seed)["code"], um)
        self.assertIn(
            "moving the pointer and clicking", v2.brief_for(um, "background", mouse_user=True)
        )

    def test_mouse_script(self) -> None:
        for seed in SEEDS:
            m = C.derive_mouse_user(seed)
            w, h = m["canvas"]
            self.assertEqual(len(m["dots"]), 8)
            for d in m["dots"]:
                self.assertTrue(m["radius"] <= d["x"] <= w - m["radius"])
                self.assertTrue(m["radius"] <= d["y"] <= h - m["radius"])
            for a, b in zip(m["sequence"], m["sequence"][1:]):
                self.assertNotEqual(a, b)
            self.assertEqual(C.derive_mouse_user(seed), m)
        leg = C.mouse_leg((0, 0), (90, 0), 9)
        self.assertEqual(len(leg), 10)
        self.assertEqual(leg[-1], (90, 0))
        self.assertEqual(C.mouse_leg((5, 5), (5, 5), 9), [(5, 5)])
        for p, q in zip(leg, leg[1:]):
            self.assertLessEqual(math.dist(p, q), 9.0)
        m = C.derive_mouse_user(1)
        d0 = m["dots"][0]
        self.assertEqual(C.dot_at(m, d0["x"] + 3, d0["y"] - 3), 0)
        self.assertIsNone(C.dot_at(m, -100, -100))


class DeskChecker(unittest.TestCase):
    def test_um_oracle_passes_noop_fails(self) -> None:
        for seed in SEEDS:
            cell = C.derive_um_desk(seed)
            state, events = oracle(seed, cell).state()
            r = S.evaluate("UM-01", "desk", seed, state, events)
            self.assertTrue(r["passed"], r)
            noop = DeskSim(seed, cell["label"]).state()
            self.assertFalse(S.evaluate("UM-01", "desk", seed, *noop)["passed"])

    def test_wrong_values_and_double_submit(self) -> None:
        seed = 42
        cell = C.derive_um_desk(seed)
        sim = oracle(seed, cell)
        sim.submit()
        r = S.evaluate("UM-01", "desk", seed, *sim.state())
        self.assertFalse(r["checks"]["submitted_once"]["passed"])
        sim = DeskSim(seed, cell["label"])
        sim.type_text(cell["code"])
        other = next(s for s in C.DESK_SHELVES if s != cell["shelf"])
        sim.pick(other)
        sim.submit()
        r = S.evaluate("UM-01", "desk", seed, *sim.state())
        self.assertFalse(r["checks"]["shelf_correct"]["passed"])
        self.assertFalse(r["checks"]["submit_values"]["passed"])

    def test_wrong_instance_label(self) -> None:
        seed = 42
        cell = dict(C.derive_um_desk(seed))
        sim = oracle(seed, cell)
        sim.label = "Z"
        state, events = sim.state()
        r = S.evaluate("UM-01", "desk", seed, state, events)
        self.assertFalse(r["checks"]["instance_matches"]["passed"])

    def test_self_correction_is_fine(self) -> None:
        seed = 517139
        cell = C.derive_um_desk(seed)
        sim = DeskSim(seed, cell["label"])
        sim.type_text(cell["code"][:3])
        sim.clear()
        sim.pick(C.DESK_SHELVES[0] if cell["shelf"] != C.DESK_SHELVES[0] else C.DESK_SHELVES[1])
        sim.type_text(cell["code"])
        sim.pick(cell["shelf"])
        sim.submit()
        self.assertTrue(S.evaluate("UM-01", "desk", seed, *sim.state())["passed"])


class ParallelChecker(unittest.TestCase):
    def test_all_oracles_pass_with_throughput(self) -> None:
        for n in C.PA_INSTANCE_VARIANTS:
            seed = 1234
            cells = C.derive_parallel(seed, n)["cells"]
            sims = [oracle(seed, c, S.T0 + 1000 * k) for k, c in enumerate(cells)]
            r = run_pa(seed, n, sims)
            self.assertTrue(r["passed"], json.dumps(r)[:2000])
            self.assertEqual(r["score"], 1.0)
            p = r["diagnostics"]["parallel"]["value"]
            self.assertEqual((p["instances"], p["passed"], p["cross_talk"]), (n, n, 0))
            self.assertGreater(p["throughput_per_min"], 0)
            last = max(s.events[-1]["t"] for s in sims)
            self.assertAlmostEqual(p["makespan_s"], round((last - S.T0) / 1000, 2))

    def test_noop_fails_and_partial_scores(self) -> None:
        seed, n = 99, 4
        cells = C.derive_parallel(seed, n)["cells"]
        sims = [DeskSim(seed, c["label"]) for c in cells]
        r = run_pa(seed, n, sims)
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)
        self.assertEqual(r["diagnostics"]["throughput_per_min"]["value"], 0.0)
        sims = [oracle(seed, c) for c in cells[:2]] + [DeskSim(seed, c["label"]) for c in cells[2:]]
        r = run_pa(seed, n, sims)
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.5)

    def test_cross_talk_caught(self) -> None:
        seed, n = 7, 10
        cells = C.derive_parallel(seed, n)["cells"]
        sims = [oracle(seed, c) for c in cells]
        # agent 1's keystrokes partly land in copy 0 (focus moved mid-typing), copy 0's agent fixes its field
        victim = DeskSim(seed, cells[0]["label"])
        victim.type_text(cells[1]["code"][:2])
        victim.clear()
        victim.type_text(cells[0]["code"])
        victim.pick(cells[0]["shelf"])
        victim.submit()
        sims[0] = victim
        r = run_pa(seed, n, sims)
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"][f"{cells[0]['label']}.no_foreign_input"]["passed"])
        p = r["diagnostics"]["parallel"]["value"]
        self.assertEqual(p["cross_talk"], 1)
        self.assertEqual(p["cross_talk_labels"], [cells[0]["label"]])
        self.assertEqual(p["passed"], 9)

    def test_single_foreign_letter_caught(self) -> None:
        seed, n = 8, 4
        cells = C.derive_parallel(seed, n)["cells"]
        sims = [oracle(seed, c) for c in cells]
        victim = DeskSim(seed, cells[2]["label"])
        victim.type_text(cells[2]["code"][:4] + cells[3]["letters"][1])
        victim.clear()
        victim.type_text(cells[2]["code"])
        victim.pick(cells[2]["shelf"])
        victim.submit()
        sims[2] = victim
        r = run_pa(seed, n, sims)
        self.assertEqual(r["diagnostics"]["parallel"]["value"]["cross_talk"], 1)

    def test_inactive_cells_letters_are_not_foreign(self) -> None:
        seed, n = 9, 4
        cells10 = C.derive_parallel(seed, 10)["cells"]
        cells = cells10[:n]
        sims = [oracle(seed, c) for c in cells]
        victim = DeskSim(seed, cells[0]["label"])
        victim.type_text(cells10[9]["letters"])  # a letter of a cell that has no agent at N = 4
        victim.clear()
        victim.type_text(cells[0]["code"])
        victim.pick(cells[0]["shelf"])
        victim.submit()
        sims[0] = victim
        self.assertTrue(run_pa(seed, n, sims)["passed"])

    def test_foreign_shelf_recorded_not_attributed(self) -> None:
        seed, n = 10, 4
        cells = C.derive_parallel(seed, n)["cells"]
        sims = [oracle(seed, c) for c in cells]
        s = DeskSim(seed, cells[0]["label"])
        s.type_text(cells[0]["code"])
        s.pick(cells[1]["shelf"])
        s.pick(cells[0]["shelf"])
        s.submit()
        sims[0] = s
        r = run_pa(seed, n, sims)
        self.assertTrue(r["passed"])
        self.assertEqual(r["diagnostics"]["foreign_shelf_selections"]["value"], 1)

    def test_out_of_budget(self) -> None:
        seed, n = 11, 4
        cells = C.derive_parallel(seed, n)["cells"]
        sims = [oracle(seed, c) for c in cells[:3]] + [oracle(seed, cells[3], S.T0 + 400_000)]
        r = run_pa(seed, n, sims, budget=360)
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"][f"{cells[3]['label']}.in_budget"]["passed"])

    def test_missing_copy_is_harness_error(self) -> None:
        seed, n = 12, 4
        cells = C.derive_parallel(seed, n)["cells"]
        r = run_pa(seed, n, [oracle(seed, c) for c in cells[:3]])
        self.assertFalse(r["passed"])
        self.assertEqual(r["harness_error"], "BenchLab copy did not start")

    def test_aggregate_agents(self) -> None:
        agg = SD.aggregate_agents(
            [
                {"label": "A", "wall_s": 40, "turns": 8, "cost_usd": 0.2, "tokens": {"input": 10}},
                {
                    "label": "B",
                    "wall_s": 90,
                    "turns": 12,
                    "cost_usd": 0.3,
                    "timed_out": True,
                    "tokens": {"input": 5},
                },
                {"label": "C", "wall_s": 30, "turns": 5, "cost_usd": 0.1, "infra": "rate_limit"},
            ]
        )
        self.assertEqual(agg["wall_s"], 90)
        self.assertEqual(agg["turns"], 25)
        self.assertAlmostEqual(agg["cost_usd"], 0.6)
        self.assertTrue(agg["timed_out"])
        self.assertEqual(agg["infra"], "rate_limit")
        self.assertEqual(agg["tokens"], {"input": 15})


# ------------------------------------------------------------------ mouse user


def mouse_log(
    seed: int,
    path: Path,
    legs: int = 6,
    displace_at: int | None = None,
    lose_click: int | None = None,
    foreign_moves: int = 0,
    tagging: bool = True,
    leak_click: bool = False,
) -> None:
    """A sentinel log as the mouse user writes it (scripted oracle), with optional disturbances."""
    m = C.derive_mouse_user(seed)
    t = S.T0
    lines: list[dict[str, Any]] = []

    def emit(ev: str, **kw: Any) -> None:
        nonlocal t
        t += m["step_ms"]
        lines.append({"t": t, "ev": ev, "armed": True, **kw})

    lines.append(
        {
            "t": t,
            "ev": "armed",
            "armed": True,
            "mouse": [0, 0],
            "front": {"bid": "ai.cua.benchsentinel", "pid": 1},
        }
    )
    emit("mu_start", tagging=tagging, tolerance_px=m["tolerance_px"], platform="test")
    cur = tuple(m["home"])
    prev = cur
    i = n = 0
    for j in range(legs):
        dot = m["dots"][m["sequence"][j % len(m["sequence"])]]
        for p in C.mouse_leg(cur, (dot["x"], dot["y"]), m["step_px"]):
            got = cur
            if displace_at is not None and i == displace_at:
                got = (cur[0] + 140, cur[1] + 60)
            d = min(math.dist(got, cur), math.dist(got, prev))
            emit("mu", i=i, want=list(cur), got=list(got), d=round(d, 2))
            for _ in range(foreign_moves if i == 3 else 0):
                emit("mu_foreign", kind="move")
            prev, cur = cur, p
            i += 1
        emit("mu", i=i, want=list(cur), got=list(cur), d=0.0)
        i += 1
        n += 1
        emit("mu_click", n=n, dot=dot["index"], at=[dot["x"], dot["y"]])
        if lose_click == n:
            emit("mu_click_result", n=n, ok=False, why="lost")
        else:
            emit("mu_rx", dot=dot["index"], at=[dot["x"], dot["y"]], own=True if tagging else None)
            emit("mu_click_result", n=n, ok=True, why="landed")
    if leak_click:
        emit("mouseDown")
    for _ in range(12):
        emit("s", mouse=list(cur), front={"bid": "ai.cua.benchsentinel", "pid": 1}, key=True,
             clip=0, onspace=True, sheet=False, above=[], above_n=0)  # fmt: skip
    emit("mu_stop")
    emit("disarmed", mouse=list(cur), front={"bid": "ai.cua.benchsentinel", "pid": 1}, clip=0,
         onspace=True, sheet=False, above=[], above_n=0)  # fmt: skip
    path.write_text("".join(json.dumps(ln) + "\n" for ln in lines))


def summary(**kw: Any) -> dict[str, Any]:
    with tempfile.TemporaryDirectory() as tmp:
        log = Path(tmp) / "sentinel.jsonl"
        mouse_log(kw.pop("seed", 42), log, **kw)
        return summarize_sentinel.summarize(str(log))


def disturbance_of(s: dict[str, Any]) -> dict[str, Any]:
    """The row fields the runners copy from a sentinel summary."""
    keys = ("key_loss", "windows_raised", "keystrokes_leaked", "clicks_leaked", "scrolls_leaked",
            "pointer_max_deviation_px", "clipboard_changes", "space_switches", "sheets_shown",
            "front_changes", "activations_lost", "activations_gained", "user_activity", "mouse_user")  # fmt: skip
    return {k: s.get(k) for k in keys}


class MouseUser(unittest.TestCase):
    def test_scripted_user_undisturbed(self) -> None:
        s = summary()
        mu = s["mouse_user"]
        self.assertEqual(mu["clicks"], 6)
        self.assertEqual(mu["clicks_landed"], 6)
        self.assertEqual(mu["max_displacement_px"], 0.0)
        self.assertEqual(mu["foreign_pointer_events"], 0)
        self.assertFalse(mu["disturbed"])
        self.assertGreater(s["pointer_travel_px"], 100, "the user really moved")
        self.assertEqual(s["pointer_max_deviation_px"], 0.0)
        self.assertEqual(s["clicks_leaked"], 0)
        crit = v2.multiplayer_criteria(disturbance_of(s))
        self.assertTrue(crit["pointer"])
        self.assertTrue(crit["typing"])
        mp = v2.multiplayer_score(disturbance_of(s), True, True, "background")
        self.assertTrue(mp["background_completion"], mp)
        self.assertIsNone(v2.mouse_user_problem(s))

    def test_displacement_caught(self) -> None:
        s = summary(displace_at=10)
        self.assertGreater(s["mouse_user"]["max_displacement_px"], 3)
        self.assertEqual(s["mouse_user"]["displacement_episodes"], 1)
        self.assertGreater(s["pointer_max_deviation_px"], 3)
        crit = v2.multiplayer_criteria(disturbance_of(s))
        self.assertFalse(crit["pointer"])
        self.assertTrue(crit["typing"])
        self.assertFalse(
            v2.multiplayer_score(disturbance_of(s), True, True, "background")[
                "background_completion"
            ]
        )

    def test_foreign_pointer_events_caught_when_tagged(self) -> None:
        s = summary(foreign_moves=3)
        self.assertEqual(s["mouse_user"]["foreign_pointer_events"], 3)
        self.assertEqual(s["mouse_user"]["max_displacement_px"], 0.0)
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["pointer"])

    def test_untagged_platform_uses_trajectory_only(self) -> None:
        s = summary(tagging=False)
        self.assertIsNone(s["mouse_user"]["foreign_pointer_events"])
        self.assertTrue(v2.multiplayer_criteria(disturbance_of(s))["pointer"])
        s = summary(tagging=False, displace_at=4)
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["pointer"])

    def test_lost_click_caught(self) -> None:
        s = summary(lose_click=2)
        mu = s["mouse_user"]
        self.assertEqual((mu["clicks"], mu["clicks_landed"], mu["clicks_lost"]), (6, 5, 1))
        crit = v2.multiplayer_criteria(disturbance_of(s))
        self.assertTrue(crit["pointer"])
        self.assertFalse(crit["typing"])
        a15 = v2.background_condition_score(
            v2.background_score(disturbance_of(s) | {"available": True}, True),
            disturbance_of(s), True, "background",
        )  # fmt: skip
        self.assertTrue(a15["user_input_disrupted"])

    def test_agent_click_in_user_window_caught(self) -> None:
        s = summary(leak_click=True)
        self.assertEqual(s["clicks_leaked"], 1)
        self.assertFalse(v2.multiplayer_criteria(disturbance_of(s))["typing"])

    def test_unresolved_click_counts_as_lost(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "s.jsonl"
            mouse_log(42, log)
            lines = [json.loads(x) for x in log.read_text().splitlines()]
            lines = [ln for ln in lines if not (ln["ev"] == "mu_click_result" and ln["n"] == 3)]
            log.write_text("".join(json.dumps(ln) + "\n" for ln in lines))
            mu = summarize_sentinel.summarize(str(log))["mouse_user"]
        self.assertEqual(mu["clicks_lost"], 1)

    def test_harness_error_when_user_did_not_run(self) -> None:
        self.assertIsNotNone(v2.mouse_user_problem({"mouse_user": None}))
        self.assertIsNotNone(v2.mouse_user_problem(summary(legs=2)))
        with tempfile.TemporaryDirectory() as tmp:
            seed = 42
            cell = C.derive_um_desk(seed)
            state, events = oracle(seed, cell).state()
            sp, ep, rp = Path(tmp) / "state.json", Path(tmp) / "events.jsonl", Path(tmp) / "r.json"
            sp.write_text(json.dumps(state))
            ep.write_text("".join(json.dumps(e) + "\n" for e in events))
            sentinel = Path(tmp) / "sentinel.jsonl"
            sentinel.write_text(json.dumps({"t": 1, "ev": "armed", "armed": True}) + "\n")
            UM.main(["--seed", str(seed), "--state", str(sp), "--events", str(ep), "--result", str(rp),
                     "--sentinel", str(sentinel)])  # fmt: skip
            r = json.loads(rp.read_text())
            self.assertTrue(r["passed"])
            self.assertIn("harness_error", r)
            mouse_log(seed, sentinel)
            UM.main(["--seed", str(seed), "--state", str(sp), "--events", str(ep), "--result", str(rp),
                     "--sentinel", str(sentinel)])  # fmt: skip
            r = json.loads(rp.read_text())
            self.assertTrue(r["passed"])
            self.assertNotIn("harness_error", r)
            self.assertEqual(r["diagnostics"]["mouse_user"]["value"]["clicks_landed"], 6)

    def test_typing_logs_unchanged(self) -> None:
        """A sentinel log without the mouse user keeps the old pointer measure and has mouse_user None."""
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "s.jsonl"
            rows = [{"t": S.T0, "ev": "armed", "armed": True, "mouse": [10, 10]}]
            rows += [
                {"t": S.T0 + 50 * k, "ev": "s", "armed": True, "mouse": [10 + k, 10]}
                for k in range(1, 12)
            ]
            rows += [{"t": S.T0 + 700, "ev": "disarmed", "armed": True, "mouse": [21, 10]}]
            log.write_text("".join(json.dumps(r) + "\n" for r in rows))
            s = summarize_sentinel.summarize(str(log))
        self.assertIsNone(s["mouse_user"])
        self.assertEqual(s["pointer_max_deviation_px"], 11.0)
        self.assertNotIn("pointer_travel_px", s)


class Wiring(unittest.TestCase):
    def test_sentinel_args(self) -> None:
        self.assertEqual(v2.sentinel_args("foreground", "x.json"), [])
        args = v2.sentinel_args("background", "x.json")
        self.assertEqual(args[:2], ["--mouse-user", "x.json"])
        self.assertNotIn("--user-activity", args)
        self.assertIn("--user-activity", v2.sentinel_args("background"))

    def test_selectors(self) -> None:
        for tid in v2.SHARED_DESKTOP_TASKS:
            spec = json.loads((BENCH / "probes" / tid / "task.json").read_text())
            self.assertEqual(v2.category_of(spec), "shared_desktop")
            self.assertEqual(spec["app_args"], ["--mode", "desk"])
        pa = json.loads((BENCH / "probes" / "PA-01" / "task.json").read_text())
        self.assertTrue(SD.is_parallel(pa))
        self.assertEqual(SD.instances_for(pa), 10)
        self.assertEqual(SD.instances_for(pa, 4), 4)
        with self.assertRaises(ValueError):
            SD.instances_for(pa, 11)
        um = json.loads((BENCH / "probes" / "UM-01" / "task.json").read_text())
        self.assertTrue(SD.is_mouse_user(um))
        self.assertEqual(um["conditions"], ["background"])
        self.assertEqual(
            v2.tasks_for_condition(["UM-01", "PA-01"], "foreground", {"UM-01": um, "PA-01": pa}),
            ["PA-01"],
        )

    def test_pa_agents_plan(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            agents = SD.pa_agents(77, 4, Path(tmp), ("on your Mac", "on this Windows PC"))
        self.assertEqual(len(agents), 4)
        self.assertTrue(all("on this Windows PC" in a["brief"] for a in agents))
        a = agents[0]
        self.assertEqual(
            a["app_args"][-4:],
            ["--instance", a["label"], "--frame", ",".join(map(str, a["frame"]))],
        )
        self.assertEqual(a["dir"].name, f"desk-{a['label']}")


if __name__ == "__main__":
    unittest.main()


class InFlightAndAnalysis(unittest.TestCase):
    def test_click_in_flight_at_disarm_not_counted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "s.jsonl"
            mouse_log(42, log)
            lines = [json.loads(x) for x in log.read_text().splitlines()]
            end = lines[-1]["t"]
            lines.insert(
                -1,
                {"t": end - 100, "ev": "mu_click", "armed": True, "n": 99, "dot": 0, "at": [0, 0]},
            )
            log.write_text("".join(json.dumps(ln) + "\n" for ln in lines))
            mu = summarize_sentinel.summarize(str(log))["mouse_user"]
        self.assertEqual((mu["clicks"], mu["clicks_lost"]), (6, 0))

    def test_analysis_shared_desktop(self) -> None:
        sys.path.insert(0, str(BENCH / "tools"))
        import analyze_v2 as A

        def pa(n: int, run: int, passed: int, minutes: float, cross: int = 0) -> dict:
            return {"task": "PA-01", "arm": "x", "condition": "foreground", "category": "shared_desktop",
                    "run_index": run, "instances": n, "passed": passed == n, "trial_id": f"{n}-{run}",
                    "parallel": {"instances": n, "passed": passed, "cross_talk": cross,
                                 "makespan_s": minutes * 60, "throughput_per_min": passed / minutes}}  # fmt: skip

        rows = [pa(1, 0, 1, 0.5), pa(1, 1, 1, 1.0), pa(4, 0, 4, 1.0), pa(4, 1, 3, 1.0, cross=1)]
        good = summary()
        rows.append({"task": "UM-01", "arm": "x", "condition": "foreground", "category": "shared_desktop",
                     "run_index": 0, "passed": True, "trial_id": "um", "disturbance": disturbance_of(good)})  # fmt: skip
        sd = A.shared_desktop(rows)
        four = sd["PA-01"]["4"]
        self.assertEqual(four["all_copies_passed"]["k"], 1)
        self.assertEqual(four["trials_with_cross_talk"], 1)
        self.assertAlmostEqual(four["speedup_vs_1_mean"], round((4 / 2 + 3 / 1) / 2, 3))
        self.assertEqual(sd["UM-01"]["user_clicks_not_landed"], 0)
        text = A.markdown(A.analyze(rows))
        self.assertIn("parallel agents (PA-01)", text)
        self.assertIn("mouse keeps working (UM-01)", text)
