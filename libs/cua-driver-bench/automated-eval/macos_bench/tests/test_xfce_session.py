# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Linux XFCE launcher contracts that hold on every OS (no desktop, no model): the ten tasks, the pack tasks'
Linux launch descriptors, placeholder substitution and window-title matching."""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "launch"))
import xfce_session as XS  # noqa: E402


class XfceSessionTest(unittest.TestCase):
    def test_ten_tasks(self) -> None:
        self.assertEqual(len(XS.TASKS), 10)
        self.assertEqual(set(XS.TASKS), set(XS.CDB_TASKS) | set(XS.LAB_TASKS))

    def test_pack_tasks_have_linux_descriptors(self) -> None:
        for task in XS.CDB_TASKS:
            spec = XS.hs.task_spec(task)
            self.assertEqual(spec["kind"], "cdb")
            path = XS.TASKPACK / "tasks" / spec["pack_task"] / "platform" / "launch.linux.json"
            descriptor = json.loads(path.read_text("utf-8"))
            self.assertTrue(descriptor["apps"])
            for step in ("setup", "verify"):
                self.assertTrue(descriptor["semantics"]["reset"][step])
            self.assertTrue(descriptor["semantics"]["evaluate"])

    def test_substitute_leaves_unknown_names(self) -> None:
        self.assertEqual(XS.substitute("${a}/${b}", {"a": "x"}), "x/${b}")

    def test_title_matches(self) -> None:
        self.assertTrue(XS.title_matches({"title": "Calc"}, "Calc"))
        self.assertTrue(XS.title_matches({"title": "Calc"}, "Calc - file.ods"))
        self.assertFalse(XS.title_matches({"title": "Calc"}, "Calculator"))
        self.assertTrue(XS.title_matches({"title_contains": "mail"}, "Copperfield Mail"))
        self.assertFalse(XS.title_matches({}, "anything"))


if __name__ == "__main__":
    unittest.main()
