# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The Windows pack-task runner (windows/cdb.py) without a desktop: every multi-app task's Windows descriptor
resolves, Electron apps get their own named executables, leftovers are cleared by exact executable name and
port, the brief matches the macOS adapter's, and the scripted oracle passes each checker while doing nothing
fails it."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "windows"))
import bench_v2 as v2  # noqa: E402
import cdb as C  # noqa: E402
import cdb_oracle as O  # noqa: E402

TASKS = ("CDB-G02", "CDB-G03", "CDB-G04")


def spec(task: str) -> dict:
    return json.loads((BENCH / "probes" / task / "task.json").read_text("utf-8"))


def descriptor(task: str) -> dict:
    return json.loads(C.descriptor_path(spec(task)["pack_task"]).read_text("utf-8"))


class DescriptorTest(unittest.TestCase):
    def test_every_multi_app_task_runs_on_windows(self) -> None:
        manifest = {t["id"]: t for t in v2.load_taskset()["tasks"]}
        for task in TASKS:
            self.assertIn("windows", manifest[task]["os"])
            d = descriptor(task)
            self.assertEqual(d["platform"], "windows")
            self.assertTrue(d["workspace_root"].startswith("${USERPROFILE}" + "\\"), task)
            self.assertTrue(C.CdbTask(spec(task), Path(tempfile.gettempdir())).apps(), task)

    def test_electron_apps_get_named_executables(self) -> None:
        argv = C.electron_argv(
            ["npx.cmd", "electron", ".", "--surface=mail"],
            "C:/b/app",
            "Copperfield Mail",
            copy=False,
        )
        self.assertEqual(Path(argv[0]).name, "Copperfield Mail.exe")
        self.assertEqual(Path(argv[0]).parent.as_posix(), "C:/b/app/node_modules/electron/dist")
        self.assertEqual(argv[1:], [".", "--surface=mail"])
        self.assertEqual(
            C.electron_argv(["npx", "electron", "."], "C:/b/a", "X", copy=False)[1:], ["."]
        )
        self.assertEqual(
            C.electron_argv(["node", "server.js"], "C:/b", "X", copy=False), ["node", "server.js"]
        )

    def test_leftovers_are_named_exactly(self) -> None:
        g02, g03, g04 = (descriptor(t) for t in TASKS)
        self.assertTrue({"copperfield mail.exe", "signal quay chat.exe", "daymark calendar.exe", "soffice.bin"}
                        <= C.process_names(g02))  # fmt: skip
        self.assertTrue(
            {"ledger lantern mail.exe", "gnucash.exe", "scalc.exe"} <= C.process_names(g03)
        )
        self.assertIn("chrome.exe", C.process_names(g04))
        self.assertNotIn("node.exe", C.process_names(g04))  # the archive service goes by its port
        self.assertEqual(C.ports(g04), [4184])
        self.assertEqual(C.ports(g02), [])

    def test_window_titles(self) -> None:
        self.assertTrue(C.title_matches({"title": "Copperfield Mail"}, "Copperfield Mail"))
        self.assertFalse(C.title_matches({"title": "Copperfield Mail"}, "Copperfield"))
        calc = {"title_contains": "kestrel-wharf-reconciliation"}
        self.assertTrue(
            C.title_matches(calc, "kestrel-wharf-reconciliation.ods - LibreOffice Calc")
        )
        self.assertFalse(C.title_matches(calc, "kestrel-wharf.gnucash - Accounts - GnuCash"))

    def test_brief_names_the_workspace(self) -> None:
        task = C.CdbTask(spec("CDB-G02"), Path(tempfile.gettempdir()))
        brief = task.brief()
        self.assertIn(f"Your workspace directory is `{task.workspace}`.", brief)
        self.assertNotIn("configured `cua` MCP server", brief)


class OracleTest(unittest.TestCase):
    def test_oracle_passes_and_noop_fails(self) -> None:
        for task in TASKS:
            with self.subTest(task=task), tempfile.TemporaryDirectory() as tmp:
                bundle = C.TASKPACK / "tasks" / spec(task)["pack_task"]
                ws, result = Path(tmp) / "ws", Path(tmp) / "result.json"

                def evaluate() -> bool:
                    subprocess.run(
                        [sys.executable, "evaluator/evaluate.py", "--workspace", str(ws), "--artifacts", tmp,
                         "--result", str(result), "--agent-exit-code", "0"],
                        cwd=bundle, capture_output=True, check=False,
                    )  # fmt: skip
                    return json.loads(result.read_text("utf-8"))["passed"]

                subprocess.run([sys.executable, "reset/setup.py", "--workspace", str(ws)], cwd=bundle,
                               capture_output=True, check=True)  # fmt: skip
                self.assertFalse(evaluate())
                O.solve(task, ws)
                self.assertTrue(evaluate())


if __name__ == "__main__":
    unittest.main()
