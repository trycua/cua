"""Checker tests for NA-04 on real files (locked flags and comment attributes)."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common, finder  # noqa: E402
import evaluate as ev  # noqa: E402


class NA04(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        self.reset()
        self.folder = self.ws / "Committee papers"

    def reset(self) -> None:
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)

    def tearDown(self) -> None:
        finder.unlock_tree(self.ws)
        self.tmp.cleanup()

    def solve(self) -> None:
        subprocess.run(["/usr/bin/chflags", "uchg", str(self.folder / "constitution-2024.txt")], check=True)
        finder.set_comment(self.folder / "minutes-march.txt", "Approved at the April meeting")
        (self.folder / "draft agenda.txt").rename(self.folder / "agenda-may.txt")

    def result(self) -> dict:
        return common.evaluate_dir("NA-04", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.result()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes_and_reset_clears_locks(self) -> None:
        self.solve()
        r = self.result()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)
        self.reset()  # two locked files now: the reset must still rebuild the workspace
        self.assertFalse(self.result()["passed"])

    def test_comment_on_wrong_file_fails(self) -> None:
        self.solve()
        finder.set_comment(self.folder / "minutes-march.txt", "")
        finder.set_comment(self.folder / "membership-list.txt", "Approved at the April meeting")
        self.assertFalse(self.result()["passed"])

    def test_unlocking_the_treasurer_report_fails(self) -> None:
        self.solve()
        subprocess.run(["/usr/bin/chflags", "nouchg", str(self.folder / "treasurer-report.txt")], check=True)
        r = self.result()
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)

    def test_copy_instead_of_rename_fails(self) -> None:
        self.solve()
        (self.folder / "draft agenda.txt").write_bytes((self.folder / "agenda-may.txt").read_bytes())
        self.assertFalse(self.result()["passed"])


if __name__ == "__main__":
    unittest.main()
