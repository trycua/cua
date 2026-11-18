# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Windows desktop contracts (no model): runs on a Windows interactive desktop with wxPython and pywinauto.

Through launch/windows_session.py, the real launcher:
* every BenchLab task is solvable through the GUI: the scripted oracle (windows/oracle.py, real input)
  passes the task's checker, with no GUI-only violation;
* an agent that does nothing fails the checker, and the sentinel measures all eight A22 criteria as held;
* a deliberate disturbance (focus steal, raise, pointer, clipboard) is seen by the sentinel.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
SESSION = BENCH / "launch" / "windows_session.py"
ORACLE = BENCH / "windows" / "oracle.py"
TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")


def _available() -> bool:
    return sys.platform == "win32" and all(
        importlib.util.find_spec(m) is not None for m in ("wx", "pywinauto")
    )


@unittest.skipUnless(_available(), "needs a Windows desktop with wxPython and pywinauto")
class WindowsDesktopTest(unittest.TestCase):
    def trial(self, task: str, condition: str, agent: list[str]) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / task
            done = subprocess.run(
                [sys.executable, str(SESSION), "--task", task, "--run-index", "0",
                 "--condition", condition, "--out", str(out), "--timeout", "150", "--", *agent],
                capture_output=True, text=True, timeout=300,
            )  # fmt: skip
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
            return json.loads((out / "row.json").read_text("utf-8"))

    def test_oracle_solves_every_task(self) -> None:
        for task in TASKS:
            with self.subTest(task=task):
                row = self.trial(
                    task,
                    "foreground",
                    [sys.executable, str(ORACLE), "--mode", "{mode}", "--seed", "{seed}"],
                )
                failed = [k for k, v in row["evaluation"]["checks"].items() if not v["passed"]]
                self.assertTrue(row["checker_passed"], failed)
                self.assertFalse(row["gui_only"]["violation"], row["gui_only"])
                self.assertTrue(row["passed"])

    def test_noop_fails_and_leaves_the_user_undisturbed(self) -> None:
        row = self.trial(
            "IR-02", "background", [sys.executable, "-c", "import time; time.sleep(8)"]
        )
        self.assertFalse(row["checker_passed"])
        self.assertTrue(row["sentinel_front_at_start"])
        self.assertEqual(row["a22"]["unmeasured"], [])
        self.assertTrue(row["a22"]["user_undisturbed"], row["a22"]["criteria"])

    def test_disturbance_is_seen(self) -> None:
        row = self.trial(
            "IR-02",
            "background",
            [sys.executable, str(ORACLE), "--mode", "{mode}", "--seed", "{seed}", "--disturb"],
        )
        criteria = row["a22"]["criteria"]
        for key in ("key_focus", "window_raised", "pointer", "clipboard", "activation"):
            self.assertFalse(criteria[key], key)
        self.assertFalse(row["a22"]["user_undisturbed"])


if __name__ == "__main__":
    unittest.main()
