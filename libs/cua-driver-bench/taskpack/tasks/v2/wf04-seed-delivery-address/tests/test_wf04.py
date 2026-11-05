"""Checker tests for WF-04 on an untouched, a solved and near-miss states."""

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

SHIP = {"name": "Joris Elzinga", "line1": "Hoefslagpad 23", "line2": "Greenhouse 4", "town": "Leeuwarden",
        "region": "Friesland", "postcode": "8912 KL", "country": "NL"}
BILL = {"name": "Joris Elzinga", "line1": "Am Lindenhof 6", "line2": "", "town": "Oldenburg",
        "region": "Lower Saxony", "postcode": "26121", "country": "DE"}


class WF04(unittest.TestCase):
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
        s["delivery"].update(status="saved", shipping=dict(SHIP), billing_same=False, billing=dict(BILL), option="pallet")
        return s

    def run_eval(self, state: dict | None = None) -> dict:
        if state is not None:
            common.write_json(self.ws / "state/state.json", state)
        return common.evaluate_dir("WF-04", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        for state in (None, copy.deepcopy(self.initial)):
            r = self.run_eval(state)
            self.assertFalse(r["passed"])
            self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        r = self.run_eval(self.solved())
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_postcode_spacing_and_case_tolerated(self) -> None:
        s = self.solved()
        s["delivery"]["shipping"]["postcode"] = "8912kl"
        self.assertTrue(self.run_eval(s)["passed"])

    def test_billing_same_ticked_fails(self) -> None:
        s = self.solved()
        s["delivery"].update(billing_same=True, billing=None)
        r = self.run_eval(s)
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["billing_separate"]["passed"])

    def test_addresses_swapped_fails(self) -> None:
        s = self.solved()
        s["delivery"]["shipping"], s["delivery"]["billing"] = dict(BILL), dict(SHIP)
        self.assertFalse(self.run_eval(s)["passed"])

    def test_wrong_region_fails(self) -> None:
        s = self.solved()
        s["delivery"]["billing"]["region"] = "Saxony"
        self.assertFalse(self.run_eval(s)["passed"])

    def test_notes_cleared_fails(self) -> None:
        s = self.solved()
        s["delivery"]["notes"] = ""
        r = self.run_eval(s)
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)

    def test_default_option_fails(self) -> None:
        s = self.solved()
        s["delivery"]["option"] = "hub"
        self.assertFalse(self.run_eval(s)["passed"])


if __name__ == "__main__":
    unittest.main()
