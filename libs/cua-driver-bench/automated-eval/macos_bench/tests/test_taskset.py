# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The task manifest: manifest, selectors, splits, conditions, native apps, the export step, harness errors and the
held-out pack (no GUI, no model, no VM)."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import bench_v2 as v2  # noqa: E402
import cdb_adapter  # noqa: E402
import run_bench as rb  # noqa: E402

TOOLS = HERE / "tools"


class TaskSetTest(unittest.TestCase):
    def test_manifest_is_the_ten_public_core_tasks(self) -> None:
        manifest = v2.load_taskset()
        self.assertEqual(tuple(e["id"] for e in manifest["tasks"]), v2.V2_TASKS)
        self.assertTrue(all(e["split"] == "public" for e in manifest["tasks"]))
        self.assertTrue(set(e["category"] for e in manifest["tasks"]) <= set(v2.CATEGORIES))

    def test_platforms(self) -> None:
        by_id = {e["id"]: e for e in v2.load_taskset()["tasks"]}
        for tid in v2.V2_TASKS:
            self.assertEqual(by_id[tid]["os"], ["macos", "linux", "windows"], tid)
        self.assertIn("GnuCash", by_id["CDB-G03"]["apps"])

    def test_task_sets_and_splits(self) -> None:
        sets = v2.task_sets()
        self.assertEqual(sets["v2"], v2.V2_TASKS)
        self.assertEqual(sets["v2-public"], v2.V2_TASKS)
        self.assertEqual(sets["v2-heldout"], ())
        fake = {
            "tasks": [
                {"id": t, "split": "heldout" if t in ("IR-03", "IR-04") else "public"}
                for t in v2.V2_TASKS
            ]
        }
        split = v2.task_sets(fake)
        self.assertEqual(split["v2-heldout"], ("IR-03", "IR-04"))
        self.assertNotIn("IR-03", split["v2-public"])
        self.assertEqual(v2.split_of("IR-03", fake), "heldout")
        self.assertEqual(v2.split_of("MB-09", fake), "public")

    def test_conditions(self) -> None:
        specs = {"MB-11": {"conditions": ["foreground"]}, "MB-09": {}}
        self.assertEqual(v2.tasks_for_condition(["MB-09", "MB-11"], "background", specs), ["MB-09"])
        self.assertEqual(
            v2.tasks_for_condition(["MB-09", "MB-11"], "foreground", specs), ["MB-09", "MB-11"]
        )

    def test_core_tasks_have_selectors(self) -> None:
        tasks = rb.load_tasks(HERE / "probes")
        for tid in v2.V2_TASKS:
            with self.subTest(task=tid):
                self.assertIn(tid, tasks)
                self.assertEqual(v2.validate_tasks({tid: rb.task_spec(tasks[tid])}, [tid]), [])

    def test_taskset_tool_check_passes(self) -> None:
        done = subprocess.run(
            [sys.executable, str(TOOLS / "taskset.py"), "check"], capture_output=True, text=True
        )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_health_requirements(self) -> None:
        by_id = {e["id"]: e for e in v2.load_taskset()["tasks"]}
        self.assertIn("node", by_id["CDB-G04"]["health"]["evaluator_tools"])
        self.assertEqual(by_id["MB-11"]["conditions"], ["foreground"])


NATIVE = {
    "semantics": {
        "reset": {"setup": ["true"], "verify": ["true"]},
        "export": ["${python}", "export.py", "--workspace", "${workspace}"],
        "evaluate": ["${python}", "evaluate.py", "--result", "${result}"],
    },
    "workspace_root": "${HOME}/cdb-synthetic-native",
    "apps": [
        {
            "id": "calendar",
            "kind": "native",
            "role": "Calendar",
            "command": ["/usr/bin/open", "-a", "Calendar"],
            "process_names": ["Calendar"],
        }
    ],
}


@unittest.skipUnless(sys.platform == "darwin", "macOS descriptors")
class NativeExportTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.bundle = self.tmp / "tasks" / "v2" / "syn-native"
        (self.bundle / "platform").mkdir(parents=True)
        (self.bundle / "platform" / "launch.macos.json").write_text(json.dumps(NATIVE))
        (self.tmp / "tasks" / "v2" / "_kit").mkdir()
        self.env = mock.patch.dict(
            os.environ, {"CDB_TASKPACK": str(self.tmp), "CDB_EVAL_SUDO": "0"}
        )
        self.env.start()
        self.spec = {"id": "SYN-01", "kind": "cdb", "pack_task": "v2/syn-native"}
        self.artifacts = self.tmp / "artifacts"
        self.artifacts.mkdir()

    def tearDown(self) -> None:
        self.env.stop()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def write(self, export_rc: int, result: dict | None) -> None:
        (self.bundle / "export.py").write_text(f"import sys\nsys.exit({export_rc})\n")
        body = "import sys, json\n"
        if result is not None:
            body += f"open(sys.argv[sys.argv.index('--result') + 1], 'w').write({json.dumps(json.dumps(result))})\n"
        (self.bundle / "evaluate.py").write_text(body)

    def test_native_app_names_and_kill_policy(self) -> None:
        self.assertIn("calendar", cdb_adapter.allowed_app_names(self.spec))
        names, _ = cdb_adapter.kill_policy(self.spec)
        self.assertIn("Calendar", names)
        self.assertEqual(
            cdb_adapter.shared_kit(self.spec), (self.tmp / "tasks" / "v2" / "_kit").resolve()
        )

    def test_export_then_evaluate_and_task_diagnostics(self) -> None:
        self.write(
            0,
            {
                "passed": True,
                "score": 1.0,
                "checks": {"a": {"passed": True}},
                "diagnostics": {"interruption": {"shown": True}},
            },
        )
        verdict = cdb_adapter.CdbTask(self.spec, self.artifacts).evaluate(0)
        self.assertTrue(verdict["passed"])
        self.assertNotIn("harness_error", verdict)
        self.assertEqual(verdict["diagnostics"]["export_rc"], 0)
        self.assertEqual(verdict["diagnostics"]["interruption"], {"shown": True})

    def test_failed_export_is_a_harness_error(self) -> None:
        self.write(3, {"passed": True, "score": 1.0, "checks": {}})
        verdict = cdb_adapter.CdbTask(self.spec, self.artifacts).evaluate(0)
        self.assertEqual(verdict["harness_error"], "export_failed")
        self.assertFalse(verdict["passed"])

    def test_no_evaluator_result_is_a_harness_error(self) -> None:
        self.write(0, None)
        verdict = cdb_adapter.CdbTask(self.spec, self.artifacts).evaluate(0)
        self.assertEqual(verdict["harness_error"], "evaluator_no_result")

    def test_health_check_raises_on_a_missing_checker_tool(self) -> None:
        task = cdb_adapter.CdbTask(self.spec, self.artifacts)
        with mock.patch.object(
            cdb_adapter, "task_health", return_value={"evaluator_tools": ["no-such-tool-xyz"]}
        ):
            with self.assertRaisesRegex(RuntimeError, "no-such-tool-xyz"):
                task.health_check()
        with mock.patch.object(
            cdb_adapter, "task_health", return_value={"evaluator_tools": ["python3"]}
        ):
            checks = task.health_check()
        self.assertTrue(all(c["ok"] for c in checks))


@unittest.skipUnless(sys.platform == "darwin", "macOS descriptors")
class HeldOutPackTest(unittest.TestCase):
    """A held-out task runs from CDB_TASKPACK_HELDOUT with the same code as a public one."""

    def test_heldout_selector_root_and_pins(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "tasks" / "v2" / "syn-held" / "platform").mkdir(parents=True)
            (root / "tasks" / "v2" / "syn-held" / "platform" / "launch.macos.json").write_text(
                json.dumps(NATIVE)
            )
            (root / "probes" / "SYN-99").mkdir(parents=True)
            selector = {
                "id": "SYN-99",
                "category": "multi_app",
                "kind": "cdb",
                "pack_task": "v2/syn-held",
                "split": "heldout",
                "coding_tools": False,
                "timeout_s": 360,
                "max_turns": 45,
            }
            (root / "probes" / "SYN-99" / "task.json").write_text(json.dumps(selector))
            (root / "pins.heldout.json").write_text(
                json.dumps({"tree_sha256": {"v2/syn-held": "abc"}})
            )
            with mock.patch.dict(os.environ, {"CDB_TASKPACK_HELDOUT": str(root)}):
                tasks = rb.load_tasks(HERE / "probes")
                self.assertIn("SYN-99", tasks)
                spec = rb.task_spec(tasks["SYN-99"])
                task = cdb_adapter.CdbTask(spec, root)
                self.assertEqual(task.bundle, root / "tasks" / "v2" / "syn-held")
                self.assertEqual(cdb_adapter.heldout_pins(), {"v2/syn-held": "abc"})
            with mock.patch.dict(os.environ, {}, clear=False):
                os.environ.pop("CDB_TASKPACK_HELDOUT", None)
                self.assertNotIn("SYN-99", rb.load_tasks(HERE / "probes"))
                with self.assertRaisesRegex(RuntimeError, "CDB_TASKPACK_HELDOUT"):
                    cdb_adapter.pack_tasks_root(spec)


if __name__ == "__main__":
    unittest.main()
