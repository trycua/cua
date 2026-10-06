"""Tests of the generic CDB adapter with a synthetic pack (no private material, no GUI, no model)."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import cdb_adapter  # noqa: E402
import claude_arms as ca  # noqa: E402
import run_bench as rb  # noqa: E402

DESCRIPTOR = {
    "semantics": {
        "reset": {"setup": ["${python}", "reset/setup.py", "--workspace", "${workspace}"], "verify": ["true"]},
        "evaluate": ["${python}", "evaluator/evaluate.py", "--workspace", "${workspace}", "--result", "${result}"],
    },
    "workspace_root": "${HOME}/cdb-synthetic",
    "apps": [
        {
            "id": "web",
            "kind": "web-server",
            "command": ["node", "server.js"],
            "ready": {"http": "http://127.0.0.1:4999/healthz"},
        },
        {
            "id": "desk",
            "kind": "electron",
            "command": ["npx", "electron", "."],
            "window": {"title": "Synthetic Desk", "bounds": {"x": 1, "y": 2, "width": 3, "height": 4}},
        },
        {"id": "browser", "kind": "browser", "command": ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]},
        {"id": "terminal", "kind": "terminal", "command": ["open", "-a", "Terminal"]},
    ],
}


class AdapterTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name) / "tasks" / "shared" / "syn"
        (root / "platform").mkdir(parents=True)
        (root / "platform" / "launch.macos.json").write_text(json.dumps(DESCRIPTOR))
        (root / "brief.md").write_text("Use the configured `cua` MCP server.\n")
        self.old = os.environ.get("CDB_TASKPACK")
        os.environ["CDB_TASKPACK"] = self.tmp.name
        self.spec = {"id": "CDB-SYN", "kind": "cdb", "pack_task": "shared/syn"}

    def tearDown(self) -> None:
        if self.old is None:
            os.environ.pop("CDB_TASKPACK", None)
        else:
            os.environ["CDB_TASKPACK"] = self.old
        self.tmp.cleanup()

    def test_allowed_names_and_kill_policy(self) -> None:
        names = cdb_adapter.allowed_app_names(self.spec)
        self.assertIn("synthetic desk", names)
        self.assertIn("google chrome", names)
        self.assertIn("electron", names)
        self.assertNotIn("terminal", names)
        kill_names, ports = cdb_adapter.kill_policy(self.spec)
        self.assertEqual(ports, [4999])
        self.assertIn("Electron", kill_names)
        self.assertIn("Google Chrome", kill_names)

    def test_brief_and_substitution(self) -> None:
        task = cdb_adapter.CdbTask(self.spec, Path(self.tmp.name))
        brief = task.brief()
        self.assertIn("configured computer-use MCP server", brief)
        self.assertNotIn("`cua`", brief)
        self.assertIn(str(task.workspace), brief)
        self.assertEqual(
            task.sub("${python} ${workspace} ${unknown}").split()[2], "${unknown}"
        )
        self.assertEqual([a["id"] for a in task.apps()], ["web", "desk", "browser"])
        self.assertEqual(task.sub("${workspace_uri}"), task.workspace.as_uri())
        self.assertTrue(task.sub("${workspace_uri}").startswith("file:///"))

    def test_kill_only_in_disposable_vm(self) -> None:
        os.environ.pop("CDB_BENCH_DISPOSABLE", None)
        cdb_adapter.clear_leftovers(self.spec)  # must be a no-op, never a pkill on a real desktop


class ArgvAndScanTest(unittest.TestCase):
    def test_coding_tools_only_when_asked(self) -> None:
        base = dict(
            mcp_config=Path("/tmp/x.json"), server="srv", model="m", max_turns=5, max_budget_usd=1
        )
        plain = ca.claude_argv(**base)
        coding = ca.claude_argv(**base, coding_tools=True)
        self.assertNotIn("Bash", plain[plain.index("--tools") + 1])
        self.assertIn("Bash", coding[coding.index("--tools") + 1].split(","))
        self.assertEqual(plain[plain.index("--allowedTools") + 1], "mcp__srv")
        self.assertIn("Edit", coding[coding.index("--allowedTools") + 1].split(","))

    def test_peek_scan(self) -> None:
        events = [
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "tool_use", "name": "Bash", "input": {"command": "ls ~/bench-work"}},
                        {"type": "tool_use", "name": "Read", "input": {"file_path": "/Users/x/ws/a.js"}},
                    ]
                },
            }
        ]
        self.assertEqual(rb.evaluator_peeks(events, []), ["Bash:bench-work"])
        self.assertEqual(rb.evaluator_peeks(events, ["a.js"]), ["Read:a.js"])


class SideDoorTest(unittest.TestCase):
    def test_scan_flags_shell_and_scripting_apps(self) -> None:
        events = [
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "tool_use", "name": "mcp__codex-cu__js", "input": {"code": "await import('node:child_process')"}},
                        {"type": "tool_use", "name": "mcp__cua__launch_app", "input": {"bundle_id": "com.apple.Terminal"}},
                        {"type": "tool_use", "name": "mcp__cua__click", "input": {"element_index": 3}},
                    ]
                },
            }
        ]
        flags = rb.side_door_scan(events)
        self.assertIn("mcp__codex-cu__js:child_process", flags)
        self.assertIn("mcp__cua__launch_app:Terminal", flags)
        self.assertEqual([f for f in flags if "click" in f], [])

    def test_separate_run_tasks_are_not_in_the_default_schedule(self) -> None:
        tasks = rb.load_tasks(HERE / "probes")
        default = [t for t in tasks if rb.default_task(tasks, t)]
        self.assertIn("CDB-S02", default)
        self.assertNotIn("CDB-G02", default)
        self.assertIn("CDB-G02", tasks)
        self.assertFalse(rb.task_spec(tasks["CDB-G02"]).get("coding_tools"))


if __name__ == "__main__":
    unittest.main()
