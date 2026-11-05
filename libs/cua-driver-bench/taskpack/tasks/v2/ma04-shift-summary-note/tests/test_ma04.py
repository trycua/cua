"""Checker tests for MA-04: notes written as TextEdit RTF (built with textutil) and the rota state."""

from __future__ import annotations

import copy
import html
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

TEXTUTIL = Path("/usr/bin/textutil")
SOLVED = [
    "Monday 13:00-16:00 Packing",
    "Tuesday 09:30-13:00 Front desk",
    "Wednesday 09:00-12:00 Warehouse sorting",
    "Wednesday 14:00-17:00 Delivery driver",
    "Thursday 17:00-19:30 Front desk",
    "Friday 13:00-16:00 Packing",
    "Saturday 10:00-13:00 Front desk",
]


@unittest.skipUnless(TEXTUTIL.exists(), "textutil is macOS only")
class MA04(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.initial = common.read_json(BUNDLE / "app/initial-state.json")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def note(self, lines: list[str]) -> None:
        src = Path(self.tmp.name) / "note.html"
        body = "".join(f"<p>{html.escape(line)}</p>\n" for line in lines)
        src.write_text(f'<!DOCTYPE html><html><head><meta charset="utf-8"></head><body>\n{body}</body></html>\n', "utf-8")
        subprocess.run([str(TEXTUTIL), "-convert", "rtf", "-output", str(self.ws / "summary.rtf"), str(src)], check=True, capture_output=True)

    def rota(self, assign: dict[str, str] | None = None) -> None:
        state = copy.deepcopy(self.initial)
        for s in state["shifts"]:
            if s["id"] in (assign or {}):
                s["volunteer"] = assign[s["id"]]
        common.write_json(self.ws / "state/state.json", state)

    def run_eval(self) -> dict:
        return common.evaluate_dir("MA-04", ev.evaluate, self.ws)

    def test_expected_matches_the_rota(self) -> None:
        open_shifts = sorted((s for s in self.initial["shifts"] if not s["volunteer"] and not s["cancelled"]), key=lambda s: (s["date"], s["start"]))
        self.assertEqual([(s["day"].lower(), s["start"], s["end"], s["role"].lower()) for s in open_shifts], ev.EXPECTED)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)
        self.rota()
        self.note([])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.rota()
        self.note(SOLVED)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_en_dash_lower_case_and_trailing_space_pass(self) -> None:
        self.note([line.replace("-", "–").lower() + "  " for line in SOLVED])
        self.assertTrue(self.run_eval()["passed"])

    def test_blank_lines_between_pass(self) -> None:
        lines = []
        for line in SOLVED:
            lines += [line, ""]
        self.note(lines)
        self.assertTrue(self.run_eval()["passed"])

    def test_cancelled_shifts_included_fails(self) -> None:
        self.note(SOLVED[:1] + ["Tuesday 08:30-11:30 Delivery driver"] + SOLVED[1:5] + ["Friday 09:00-12:00 Warehouse sorting"] + SOLVED[5:])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["nothing_else_in_note"]["passed"])

    def test_page_order_fails(self) -> None:
        by_role = sorted(SOLVED, key=lambda line: line.split(" ", 2)[2])
        self.note(by_role)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertTrue(r["checks"]["every_open_shift_listed"]["passed"])
        self.assertFalse(r["checks"]["chronological_order"]["passed"])

    def test_missing_last_page_fails(self) -> None:
        self.note(SOLVED[:-1])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["every_open_shift_listed"]["passed"])

    def test_heading_line_fails(self) -> None:
        self.note(["Shifts still needing a volunteer"] + SOLVED)
        self.assertFalse(self.run_eval()["passed"])

    def test_short_day_names_fail(self) -> None:
        self.note([line.replace("day", "") for line in SOLVED])
        self.assertEqual(self.run_eval()["score"], 0.0)

    def test_rota_edited_fails(self) -> None:
        self.note(SOLVED)
        self.rota({"SH-303": "Someone"})
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["rota_unchanged"]["passed"])


if __name__ == "__main__":
    unittest.main()
