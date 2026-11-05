"""Checker tests for WF-03 on an untouched, a solved and near-miss states."""

from __future__ import annotations

import copy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common  # noqa: E402
import evaluate as ev  # noqa: E402


def change(state: dict, vid: str, **values) -> None:
    for v in state["volunteers"]:
        if v["id"] == vid:
            v.update(values)
            return
    raise KeyError(vid)


class WF03(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.initial = common.read_json(BUNDLE / "app/initial-state.json")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def solved(self) -> dict:
        s = copy.deepcopy(self.initial)
        for vid in ("V-1112", "V-1229", "V-1259"):
            change(s, vid, team="pantry")
        change(s, "V-1214", status="inactive")
        return s

    def run_eval(self, state: dict | None = None) -> dict:
        if state is not None:
            common.write_json(self.ws / "state/state.json", state)
        return common.evaluate_dir("WF-03", ev.evaluate, self.ws)

    def test_names_in_fixture(self) -> None:
        by_id = {v["id"]: v for v in self.initial["volunteers"]}
        self.assertEqual(by_id["V-1214"]["email"], "maren.ostwick@moorpost.example")
        self.assertEqual(by_id["V-1211"]["name"], "Maren Ostwick")
        for vid, name in ev.MOVE.items():
            self.assertEqual(by_id[vid]["name"], name)
            self.assertNotEqual(by_id[vid]["team"], "pantry")
        order = [v["id"] for v in self.initial["volunteers"]]
        self.assertGreaterEqual(order.index("V-1259"), 15)  # not on the first page

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)
        r = self.run_eval(copy.deepcopy(self.initial))
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        r = self.run_eval(self.solved())
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_wrong_maren_fails(self) -> None:
        s = copy.deepcopy(self.initial)
        for vid in ("V-1112", "V-1229", "V-1259"):
            change(s, vid, team="pantry")
        change(s, "V-1211", status="inactive")
        r = self.run_eval(s)
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["deactivated_right_maren"]["passed"])
        self.assertFalse(r["checks"]["other_maren_still_active"]["passed"])

    def test_both_marens_deactivated_fails(self) -> None:
        s = self.solved()
        change(s, "V-1211", status="inactive")
        r = self.run_eval(s)
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)

    def test_ilsa_moved_instead_of_ilse_fails(self) -> None:
        s = copy.deepcopy(self.initial)
        for vid in ("V-1112", "V-1229"):
            change(s, vid, team="pantry")
        change(s, "V-1256", team="events")
        change(s, "V-1214", status="inactive")
        r = self.run_eval(s)
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["moved_V-1259"]["passed"])

    def test_wrong_team_fails(self) -> None:
        s = self.solved()
        change(s, "V-1229", team="tools")
        self.assertFalse(self.run_eval(s)["passed"])

    def test_bystander_changed_fails(self) -> None:
        s = self.solved()
        change(s, "V-1100", team="pantry")
        r = self.run_eval(s)
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)


if __name__ == "__main__":
    unittest.main()
