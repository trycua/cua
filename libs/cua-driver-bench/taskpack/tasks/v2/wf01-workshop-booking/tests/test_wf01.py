"""Checker tests for WF-01 on an untouched, a solved and near-miss states."""

from __future__ import annotations

import copy
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

SOLVED = {
    "id": "B-1040", "course": "bandsaw", "date": "2026-11-12", "slot": "14:00", "participants": 2,
    "member": "FH-30417", "access": True, "access_note": "Step-free access for one participant",
}


class WF01(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.initial = common.read_json(BUNDLE / "app/initial-state.json")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, bookings: list, submits: int) -> None:
        common.write_json(self.ws / "state/state.json", {"bookings": bookings})
        lines = [json.dumps({"type": "booking_submitted", "seq": i + 1, "booking": bookings[-1]}) for i in range(submits)]
        (self.ws / "state/events.jsonl").write_text("\n".join(lines) + ("\n" if lines else ""))

    def run_eval(self) -> dict:
        return common.evaluate_dir("WF-01", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_untouched_after_server_start_fails_with_zero(self) -> None:
        self.write(copy.deepcopy(self.initial["bookings"]), 0)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.write(self.initial["bookings"] + [SOLVED], 1)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_wrong_slot_fails(self) -> None:
        self.write(self.initial["bookings"] + [{**SOLVED, "slot": "18:30"}], 1)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["slot_ok"]["passed"])

    def test_double_booking_fails(self) -> None:
        self.write(self.initial["bookings"] + [SOLVED, {**SOLVED, "id": "B-1050"}], 2)
        self.assertFalse(self.run_eval()["passed"])

    def test_cancelled_existing_fails(self) -> None:
        self.write([SOLVED], 1)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)


if __name__ == "__main__":
    unittest.main()
