# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

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
        "reset": {
            "setup": ["${python}", "reset/setup.py", "--workspace", "${workspace}"],
            "verify": ["true"],
        },
        "evaluate": [
            "${python}",
            "evaluator/evaluate.py",
            "--workspace",
            "${workspace}",
            "--result",
            "${result}",
        ],
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
            "window": {
                "title": "Synthetic Desk",
                "bounds": {"x": 1, "y": 2, "width": 3, "height": 4},
            },
        },
        {
            "id": "browser",
            "kind": "browser",
            "command": ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
        },
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
        self.assertEqual(task.sub("${python} ${workspace} ${unknown}").split()[2], "${unknown}")
        self.assertEqual([a["id"] for a in task.apps()], ["web", "desk", "browser"])
        self.assertEqual(task.sub("${workspace_uri}"), task.workspace.as_uri())
        self.assertTrue(task.sub("${workspace_uri}").startswith("file:///"))

    def test_seed_and_rendered_brief(self) -> None:
        task = cdb_adapter.CdbTask(self.spec, Path(self.tmp.name), seed=7)
        task.workspace = Path(self.tmp.name) / "workspace"
        rendered = task.workspace / ".runtime/brief.md"
        rendered.parent.mkdir(parents=True)
        rendered.write_text("Seeded person: Mira Vale")
        task.descriptor["semantics"]["brief"] = "${workspace}/.runtime/brief.md"
        self.assertIn("Mira Vale", task.brief())
        self.assertEqual(task.sub("${seed}"), "7")

    def test_seed_identity_direct_and_sudo(self) -> None:
        from unittest import mock
        import subprocess

        task = cdb_adapter.CdbTask(self.spec, Path(self.tmp.name), seed=7)
        task.descriptor["semantics"]["reset"]["setup"].extend(["--seed", "${seed}"])
        for sudo in ("0", "1"):
            for reported in (None, 8, 7):
                with self.subTest(sudo=sudo, reported=reported):
                    data = {"passed": True, "score": 1, "detail": {"checks": {"fields": True}}}
                    if reported is not None:
                        data["seed"] = reported
                    result_file = task.artifacts / "cdb-result.json"
                    result_file.write_text(json.dumps(data))
                    done = subprocess.CompletedProcess([], 0, json.dumps(data), "")
                    with (
                        mock.patch.dict(os.environ, {"CDB_EVAL_SUDO": sudo}),
                        mock.patch.object(task, "_run", return_value=done),
                        mock.patch.object(cdb_adapter.subprocess, "run", return_value=done),
                    ):
                        verdict = task.evaluate(0)
                    self.assertEqual(verdict["passed"], reported == 7)
                    self.assertEqual(verdict["checks"]["seed_identity"], reported == 7)
                    self.assertEqual(verdict.get("seed"), reported)

    def test_missing_electron_apps_follow_manifests(self) -> None:
        root = Path(self.tmp.name) / "tasks"
        expected = ["shared/syn/apps/desk", "shared/syn/apps/mail"]
        for name in expected:
            app = root / name
            (app / "node_modules/electron/dist").mkdir(parents=True)
            (app / "package.json").write_text("{}")
        (root / "shared/syn/apps/unmanaged/node_modules/electron/dist").mkdir(parents=True)
        self.assertEqual(cdb_adapter.missing_electron_apps(root), (expected, []))
        (root / expected[1] / "node_modules/electron/dist").rmdir()
        self.assertEqual(cdb_adapter.missing_electron_apps(), (expected, [expected[1]]))

    def test_electron_apps_get_their_own_name_and_bundle_id(self) -> None:
        import plistlib
        from unittest import mock

        app_dir = Path(self.tmp.name) / "desk"
        source = app_dir / "node_modules/electron/dist/Electron.app"
        (source / "Contents/MacOS").mkdir(parents=True)
        (source / "Contents/MacOS/Electron").write_text("bin")
        helper = source / "Contents/Frameworks/Electron Helper (Renderer).app/Contents"
        helper.mkdir(parents=True)
        (source / "Contents/Info.plist").write_bytes(
            plistlib.dumps(
                {
                    "CFBundleIdentifier": "com.github.Electron",
                    "CFBundleName": "Electron",
                    "CFBundleExecutable": "Electron",
                }
            )
        )
        (helper / "Info.plist").write_bytes(
            plistlib.dumps({"CFBundleIdentifier": "com.github.Electron.helper.Renderer"})
        )
        root = Path(self.tmp.name) / "named"
        with (
            mock.patch.dict(os.environ, {"CDB_ELECTRON_APPS": str(root)}),
            mock.patch.object(cdb_adapter, "_sign") as sign,
            mock.patch.object(cdb_adapter, "LSREGISTER", Path("/nonexistent")),
        ):
            argv = cdb_adapter.electron_argv(
                ["env", "-u", "ELECTRON_RUN_AS_NODE", "npx", "electron", ".", "--surface=mail"],
                str(app_dir),
                "Synthetic Mail",
            )
            again = cdb_adapter.electron_argv(
                ["npx", "electron", "."], str(app_dir), "Synthetic Mail"
            )
        bundle = root / "synthetic-mail" / "Synthetic Mail.app"
        self.assertEqual(
            argv,
            [
                "env",
                "-u",
                "ELECTRON_RUN_AS_NODE",
                str(bundle / "Contents/MacOS/Electron"),
                ".",
                "--surface=mail",
            ],
        )
        self.assertEqual(again[0], str(bundle / "Contents/MacOS/Electron"))
        self.assertEqual(sign.call_count, 1)  # made once, then reused
        info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
        self.assertEqual(info["CFBundleIdentifier"], "com.trycua.cdbbench.synthetic-mail")
        self.assertEqual(info["CFBundleName"], "Synthetic Mail")
        self.assertEqual(info["CFBundleExecutable"], "Electron")
        hinfo = plistlib.loads(
            (
                bundle / "Contents/Frameworks/Electron Helper (Renderer).app/Contents/Info.plist"
            ).read_bytes()
        )
        self.assertEqual(
            hinfo["CFBundleIdentifier"], "com.trycua.cdbbench.synthetic-mail.helper.Renderer"
        )
        # the pack's own Electron.app is untouched
        self.assertEqual(
            plistlib.loads((source / "Contents/Info.plist").read_bytes())["CFBundleIdentifier"],
            "com.github.Electron",
        )
        self.assertEqual(
            cdb_adapter.electron_argv(["node", "server.js"], str(app_dir), "x"),
            ["node", "server.js"],
        )
        self.assertEqual(cdb_adapter.app_display_name(DESCRIPTOR["apps"][1]), "Synthetic Desk")

    def test_lab_occluders_are_windows_above_benchlab_that_overlap_it(self) -> None:
        def w(app, z, x, y, wd=300, h=200, layer=0):
            return {
                "app_name": app,
                "window_id": z,
                "z_index": z,
                "layer": layer,
                "is_on_screen": True,
                "bounds": {"x": x, "y": y, "width": wd, "height": h},
            }

        lab = w("BenchLab", 10, 60, 80, 760, 560)
        terminal = w("Terminal", 20, 100, 100)  # above and overlapping: occludes
        below = w("Finder", 5, 100, 100)  # overlapping but behind
        aside = w("BenchSentinel", 30, 1200, 700)  # above but elsewhere
        menubar = w("Window Server", 40, 0, 0, 1920, 25, layer=24)
        found, above = rb.lab_occluders([lab, terminal, below, aside, menubar])
        self.assertEqual(found["window_id"], 10)
        self.assertEqual([x["app_name"] for x in above], ["Terminal"])
        self.assertEqual(rb.lab_occluders([terminal]), (None, []))

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
                        {
                            "type": "tool_use",
                            "name": "Bash",
                            "input": {"command": "ls ~/bench-work"},
                        },
                        {
                            "type": "tool_use",
                            "name": "Read",
                            "input": {"file_path": "/Users/x/ws/a.js"},
                        },
                    ]
                },
            }
        ]
        self.assertEqual(rb.evaluator_peeks(events, []), ["Bash:bench-work"])
        self.assertEqual(rb.evaluator_peeks(events, ["a.js"]), ["Read:a.js"])


class WebWiringTest(unittest.TestCase):
    def test_browser_selection_row_manifest_and_dry_run_prompt(self) -> None:
        import contextlib
        import hashlib
        import io
        from unittest import mock

        with tempfile.TemporaryDirectory() as tmp:
            args = rb.build_parser().parse_args(
                [
                    "run",
                    "--tasks",
                    "WEB-01",
                    "WEB-02",
                    "--arms",
                    "cc-codex-cu-1007-browser",
                    "--runs-dir",
                    tmp,
                    "--run-id",
                    "web-test",
                    "--ledger",
                    str(Path(tmp) / "ledger.jsonl"),
                    "--phase1-runs",
                    "1",
                    "--phase2-runs",
                    "0",
                ]
            )
            with (
                mock.patch.object(rb, "claude_version", return_value="test"),
                mock.patch.object(rb, "sw_vers", return_value={}),
                mock.patch.object(rb, "git_commit", return_value="test"),
            ):
                ctx = rb.make_ctx(args)
            self.assertEqual(ctx.task_ids, ["WEB-01", "WEB-02"])
            entries = rb.core.flat_entries(ctx.blocks())
            self.assertEqual({entry["task"] for entry in entries}, {"WEB-01", "WEB-02"})
            arm = ctx.arm_names[0]
            ctx.mcp[arm] = (Path(tmp) / "mcp.json", "browser")
            with (
                mock.patch.object(rb.pilot, "hid_idle_seconds", return_value=1000),
                mock.patch.object(rb.cdb_adapter, "CdbTask"),
                mock.patch.object(
                    rb, "kill_bench_apps", side_effect=[RuntimeError("stop before GUI"), None]
                ),
            ):
                row = rb.run_attempt(ctx, entries[0], 1)
            rb.write_manifest(ctx, entries)
            manifest = json.loads((ctx.run_dir / "manifest.json").read_text())
            prompt = ca.system_prompt_for(arm, browser=True)
            digest = hashlib.sha256(prompt.encode()).hexdigest()
            self.assertEqual(row["system_prompt_sha256"], digest)
            self.assertEqual(manifest["system_prompt_sha256_by_arm_browser"][arm], digest)
            self.assertIn(prompt, manifest["example_argv"][arm])
            output = io.StringIO()
            with (
                mock.patch.object(rb, "make_ctx", return_value=ctx),
                mock.patch.object(ca, "mcp_config_for", return_value=ctx.mcp[arm]),
                contextlib.redirect_stdout(output),
            ):
                self.assertEqual(rb.cmd_dry_run(args), 0)
            self.assertIn(
                "Do not use the internet; the task's localhost site is allowed.", output.getvalue()
            )

    def test_browser_policy_and_state_peek(self) -> None:
        browser = ca.system_prompt_for("cc-cua-driver", browser=True)
        plain = ca.system_prompt_for("cc-cua-driver")
        sentence = "Do not use the internet; the task's localhost site is allowed."
        self.assertIn(sentence, browser)
        self.assertEqual(browser.replace(sentence, "Do not use the network."), plain)
        events = [
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Read",
                            "input": {"url": "http://127.0.0.1:4191/__state", "path": "oracle"},
                        }
                    ]
                },
            }
        ]
        self.assertEqual(rb.evaluator_peeks(events, []), ["Read:oracle", "Read:/__state"])

    def test_setup_brief_and_profile_reset(self) -> None:
        import subprocess

        tasks = rb.load_tasks(HERE / "probes")
        with tempfile.TemporaryDirectory() as tmp:
            for tid in ("WEB-01", "WEB-02"):
                task = cdb_adapter.CdbTask(rb.task_spec(tasks[tid]), Path(tmp), seed=0)
                task.workspace = Path(tmp) / tid
                setup = [task.sub(arg) for arg in task.descriptor["semantics"]["reset"]["setup"]]
                subprocess.run(setup, cwd=task.bundle, check=True, timeout=15)
                self.assertIn("Mira Vale", task.brief())
                self.assertNotIn("{{values}}", task.brief())
                stale = task.workspace / ".runtime/browser/stale"
                stale.write_text("old cookie")
                subprocess.run(setup, cwd=task.bundle, check=True, timeout=15)
                self.assertFalse(stale.exists())
                task.seed = 1
                subprocess.run(
                    [task.sub(arg) for arg in task.descriptor["semantics"]["reset"]["setup"]],
                    cwd=task.bundle,
                    check=True,
                    timeout=15,
                )
                self.assertIn("Oren Brook", task.brief())

    def test_selectors_pins_ports_and_ci_skip(self) -> None:
        import re
        import subprocess

        tasks = rb.load_tasks(HERE / "probes")
        pins = json.loads((HERE / "pins.json").read_text())["cdb_pack"]["tree_sha256"]
        self.assertEqual(rb.WEB_TASKS, ("WEB-01", "WEB-02"))
        for tid in rb.WEB_TASKS:
            spec = rb.task_spec(tasks[tid])
            self.assertTrue(spec["browser"])
            self.assertFalse(spec["coding_tools"])
            self.assertEqual(spec["category"], "web_forms")
            self.assertIn(spec["pack_task"], pins)
            self.assertNotIn(tid, rb.v2.V2_TASKS)
            self.assertFalse(rb.default_task(tasks, tid))
            descriptor = json.loads(
                (
                    cdb_adapter.pack_tasks_root() / spec["pack_task"] / "platform/launch.macos.json"
                ).read_text()
            )
            self.assertIn("timeout_seconds", descriptor["apps"][0]["ready"])
            chrome = descriptor["apps"][1]["command"]
            self.assertIn("Google Chrome", chrome[0])
            self.assertIn("--user-data-dir=${workspace}/.runtime/browser", chrome)
            self.assertTrue(chrome[-1].startswith("http://127.0.0.1:"))
        self.assertTrue(rb.task_spec(tasks["CDB-G04"])["browser"])
        ports = {}
        for descriptor in cdb_adapter.pack_tasks_root().glob("shared/*/platform/launch.macos.json"):
            for port in set(re.findall(r"127\.0\.0\.1:(\d+)", descriptor.read_text())):
                self.assertNotIn(port, ports, (port, descriptor, ports))
                ports[port] = descriptor
        workflow = HERE.parents[3] / ".github/workflows/ci-cua-driver-bench.yml"
        text = (
            workflow.read_text()
            .split("- name: Evaluator, reset and manifest tests", 1)[1]
            .split("- name:", 1)[0]
        )
        guard = next(line.strip() for line in text.splitlines() if "task.cuabench.json" in line)
        done = subprocess.run(
            ["bash", "-c", "for task in */; do " + guard + '; echo "${task%/}"; done'],
            cwd=cdb_adapter.pack_tasks_root() / "shared",
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
        self.assertNotIn("web-common", done.stdout.splitlines())
        self.assertIn("web-01", done.stdout.splitlines())
        self.assertIn("web-02", done.stdout.splitlines())


class SideDoorTest(unittest.TestCase):
    def test_scan_flags_shell_and_scripting_apps(self) -> None:
        events = [
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "mcp__codex-cu__js",
                            "input": {"code": "await import('node:child_process')"},
                        },
                        {
                            "type": "tool_use",
                            "name": "mcp__cua__launch_app",
                            "input": {"bundle_id": "com.apple.Terminal"},
                        },
                        {
                            "type": "tool_use",
                            "name": "mcp__cua__click",
                            "input": {"element_index": 3},
                        },
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
