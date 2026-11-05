"""Checker tests for PR-03 on an untouched, a solved and near-miss answers."""

from __future__ import annotations

import json
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

RIGHT = {"depot": "Sallowmead North", "week": "W38", "returns": 418}


class PR03(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, subs: list[dict]) -> None:
        common.write_json(self.ws / "state/state.json", {"submissions": subs})
        lines = [json.dumps({"type": "answer_submitted", "seq": i + 1, **s}) for i, s in enumerate(subs)]
        (self.ws / "state/events.jsonl").write_text("\n".join(lines) + ("\n" if lines else ""))

    def run_eval(self) -> dict:
        return common.evaluate_dir("PR-03", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_untouched_after_server_start_fails_with_zero(self) -> None:
        self.write([])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.write([RIGHT])
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_close_bar_week_33_fails(self) -> None:
        # Week 33 (417) draws exactly as tall as week 38; only the tooltip tells them apart.
        self.write([{**RIGHT, "week": "W33", "returns": 417}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.3333)

    def test_tallest_bar_of_other_series_fails(self) -> None:
        self.write([{**RIGHT, "week": "W32", "returns": 431}])
        self.assertFalse(self.run_eval()["passed"])

    def test_right_week_estimated_value_fails(self) -> None:
        self.write([{**RIGHT, "returns": 420}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["returns_ok"]["passed"])

    def test_submitted_twice_fails(self) -> None:
        self.write([{**RIGHT, "week": "W33", "returns": 417}, RIGHT])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["submitted_once"]["passed"])

    def test_wrong_depot_fails(self) -> None:
        self.write([{**RIGHT, "depot": "Sallowmead South"}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.5)


if __name__ == "__main__":
    unittest.main()
