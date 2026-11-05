"""Checker tests for NA-06 on exported Notes states."""

from __future__ import annotations

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

STATE = common.read_json(BUNDLE / "reset/app_state.json")


def note(name: str, lines: list[str], folder: str = "Tasting panel") -> dict:
    return {"folder": folder, "name": name, "body": "", "plaintext": "\n".join(lines)}


START = [note("Panel members", STATE["lines"]["Panel members"]), note("Round 3 scores", STATE["lines"]["Round 3 scores"])]
SOLVED = [note("Panel members", STATE["lines"]["Panel members"]), note("Round 3 scores", ev.ROUND3), note("Round 4 shortlist", ev.ROUND4)]


class NA06(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_with(self, notes: list[dict]) -> dict:
        common.write_json(self.ws / "export/notes.json", {"folders": ["Notes", "Tasting panel"], "notes": notes})
        return common.evaluate_dir("NA-06", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_with(START)
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        r = self.run_with(SOLVED)
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_blank_lines_and_nbsp_tolerated(self) -> None:
        notes = [SOLVED[0], note("Round 3 scores", ["Round 3 scores", "", "Apple & ginger: 7", "Pear & cardamom: 8", "Plum & star anise: 9", "Quince & bay: 8", ""]), SOLVED[2]]
        self.assertTrue(self.run_with(notes)["passed"])

    def test_earlier_trials_in_recently_deleted_ignored(self) -> None:
        old = [note("Round 4 shortlist", ev.ROUND4, folder="Recently Deleted"), note("Round 3 scores", ev.ROUND3, folder="Recently Deleted")]
        self.assertTrue(self.run_with(SOLVED + old)["passed"])

    def test_new_note_in_default_folder_fails(self) -> None:
        notes = SOLVED[:2] + [note("Round 4 shortlist", ev.ROUND4, folder="Notes")]
        self.assertFalse(self.run_with(notes)["passed"])

    def test_quince_not_last_fails(self) -> None:
        r3 = ["Round 3 scores", "Quince & bay: 8", "Apple & ginger: 7", "Pear & cardamom: 8", "Plum & star anise: 9"]
        notes = [SOLVED[0], note("Round 3 scores", r3), SOLVED[2]]
        self.assertFalse(self.run_with(notes)["passed"])

    def test_old_score_kept_fails(self) -> None:
        r3 = ev.ROUND3[:3] + ["Plum & star anise: 6", "Plum & star anise: 9", "Quince & bay: 8"]
        self.assertFalse(self.run_with([SOLVED[0], note("Round 3 scores", r3), SOLVED[2]])["passed"])


if __name__ == "__main__":
    unittest.main()
