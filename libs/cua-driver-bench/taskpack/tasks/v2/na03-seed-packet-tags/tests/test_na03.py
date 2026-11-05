"""Checker tests for NA-03 on real files with real Finder tag attributes."""

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


class NA03(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.folder = self.ws / "Seed packets 2026"

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def solve(self) -> None:
        for n in ev.RED:
            finder.set_tags(self.folder / n, ["Red"])
        for n in ev.GREEN:
            finder.set_tags(self.folder / n, ["Green"])
        finder.set_tags(self.folder / ev.LETTUCE, ["Blue"])

    def result(self) -> dict:
        return common.evaluate_dir("NA-03", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.result()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.solve()
        (self.folder / ".DS_Store").write_bytes(b"\0")  # Finder may write one
        r = self.result()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_extra_tag_on_tomato_fails(self) -> None:
        self.solve()
        finder.set_tags(self.folder / ev.RED[0], ["Red", "Orange"])
        self.assertFalse(self.result()["passed"])

    def test_both_lettuce_tags_removed_fails(self) -> None:
        self.solve()
        finder.set_tags(self.folder / ev.LETTUCE, [])
        self.assertFalse(self.result()["checks"]["lettuce_yellow_removed"]["passed"])

    def test_tagging_everything_red_fails(self) -> None:
        self.solve()
        finder.set_tags(self.folder / "chard-copper-stem.txt", ["Red"])
        r = self.result()
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)

    def test_renamed_file_fails(self) -> None:
        self.solve()
        (self.folder / "beetroot-dusk-globe.txt").rename(self.folder / "beetroot.txt")
        self.assertFalse(self.result()["passed"])


if __name__ == "__main__":
    unittest.main()
