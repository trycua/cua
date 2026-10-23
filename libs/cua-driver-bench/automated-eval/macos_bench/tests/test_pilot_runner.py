# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Unit tests for the pilot runner pieces that do not need a GUI or a model."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import arms  # noqa: E402
import codex_events  # noqa: E402


def _stamp(events: list[tuple[float, dict]]) -> Path:
    handle = tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False)
    for stamp, event in events:
        handle.write(f"{stamp}\t{json.dumps(event)}\n")
    handle.close()
    return Path(handle.name)


def _mcp(item_id: str, server: str, tool: str, arguments: dict, status: str = "completed") -> dict:
    return {
        "id": item_id,
        "type": "mcp_tool_call",
        "server": server,
        "tool": tool,
        "arguments": arguments,
        "status": status,
    }


class DesktopRuntimeTest(unittest.TestCase):
    def test_platform_and_session_environment(self) -> None:
        import desktop_runtime as desktop

        self.assertEqual(desktop.detect_platform("Darwin"), "macos")
        self.assertEqual(desktop.detect_platform("Linux"), "linux")
        with self.assertRaises(ValueError):
            desktop.detect_platform("Windows")
        from unittest.mock import patch

        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(desktop, "detect_platform", return_value="linux"),
            patch.dict(
                os.environ,
                {
                    "DISPLAY": ":92",
                    "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/bus",
                    "SECRET_TOKEN": "secret",
                },
            ),
        ):
            env = desktop.desktop_env(Path(tmp))
            self.assertEqual(env["DISPLAY"], ":92")
            self.assertEqual(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/tmp/bus")
            self.assertNotIn("SECRET_TOKEN", env)

    def test_macos_environment_uses_closed_path_and_locale(self) -> None:
        import desktop_runtime as desktop
        import run_pilot as pilot
        from unittest.mock import patch

        with (
            patch.object(desktop, "detect_platform", return_value="macos"),
            patch.dict(os.environ, {"PATH": "/injected/bin", "LANG": "injected", "DISPLAY": ":92"}),
        ):
            env = desktop.desktop_env(Path("/tmp/home"))
            self.assertEqual(env["PATH"], "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
            self.assertEqual(env["LANG"], "en_US.UTF-8")
            self.assertNotIn("DISPLAY", env)
            self.assertEqual(
                pilot.clean_app_env(Path("/tmp/home"))["PATH"],
                f"/opt/homebrew/bin:{arms.CLOSED_PATH}",
            )

    def test_stop_group_tolerates_permission_error_after_termination(self) -> None:
        import desktop_runtime as desktop
        from unittest.mock import patch

        with (
            patch.object(desktop.os, "killpg", side_effect=[None, PermissionError]),
            patch.object(desktop.time, "sleep"),
        ):
            desktop.stop_group(12345)

    @unittest.skipUnless(sys.platform == "linux", "Linux desktop")
    def test_missing_display_is_an_actionable_doctor_failure(self) -> None:
        import desktop_runtime as desktop
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, {"DISPLAY": ""}):
            checks = desktop.doctor(
                Path("/nonexistent/cua-driver"), "/tmp/no-driver.sock", Path(tmp)
            )
        by_name = {c.name: c for c in checks}
        self.assertEqual(by_name["display"].status, "fail")
        self.assertIn("DISPLAY", by_name["display"].detail)
        self.assertEqual(by_name["driver"].status, "fail")

    @unittest.skipUnless(sys.platform == "linux", "Linux desktop")
    def test_unreachable_desktop_and_recording_dependencies_fail_doctor(self) -> None:
        import desktop_runtime as desktop
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with patch.dict(
                os.environ,
                {
                    "DISPLAY": ":9876",
                    "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent/bus",
                    "PATH": str(root),
                },
            ):
                checks = {
                    c.name: c
                    for c in desktop.doctor(root / "driver", str(root / "socket"), root / "home")
                }
                for name in ("display", "at-spi", "ffmpeg", "driver"):
                    self.assertEqual(checks[name].status, "fail", name)
                ffmpeg = root / "ffmpeg"
                ffmpeg.write_text("#!/bin/sh\nprintf 'Devices: no X11 support\\n'\n")
                ffmpeg.chmod(0o700)
                checks = {
                    c.name: c
                    for c in desktop.doctor(root / "driver", str(root / "socket"), root / "home")
                }
                self.assertEqual(checks["ffmpeg"].status, "fail")
                self.assertIn("x11grab", checks["ffmpeg"].detail)

    @unittest.skipUnless(sys.platform == "linux", "Linux desktop")
    def test_linux_health_requires_capabilities_not_tcc(self) -> None:
        import desktop_runtime as desktop

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / "driver"
            binary.write_text(
                "#!/usr/bin/python3\nimport json\nprint(json.dumps({'checks': [{'name': 'tcc_accessibility', 'status': 'skip'}, {'name': 'tcc_screen_recording', 'status': 'skip'}, {'name': 'ax_capability', 'status': 'pass'}, {'name': 'screen_capture_capability', 'status': 'pass'}]}))\n"
            )
            binary.chmod(0o700)
            health = desktop.cua_health(binary, str(root / "socket"), root / "home")
            self.assertTrue(desktop.capabilities_ok(health))
            binary.write_text(
                binary.read_text().replace(
                    "'ax_capability', 'status': 'pass'", "'ax_capability', 'status': 'fail'"
                )
            )
            self.assertFalse(
                desktop.capabilities_ok(
                    desktop.cua_health(binary, str(root / "socket"), root / "home")
                )
            )

    @unittest.skipUnless(
        sys.platform == "linux" and os.environ.get("DISPLAY"), "headless X11 session required"
    )
    def test_task_helpers_emit_evaluator_compatible_initial_state_and_evidence(self) -> None:
        import desktop_runtime as desktop
        import run_bench as runner
        import run_pilot as pilot

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tasks = pilot.load_tasks()
            for task_id in ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04"):
                with self.subTest(task=task_id):
                    trial = root / task_id
                    trial.mkdir()
                    (trial / "artifacts").mkdir()
                    _, paths = pilot.prepare_probe(tasks[task_id], 7, trial, root / "BenchLab.app")
                    proc = pilot.launch_lab(
                        tasks[task_id],
                        7,
                        paths,
                        root / "BenchLab.app",
                        pilot.clean_app_env(trial / "home"),
                    )
                    try:
                        result = pilot.evaluate_probe(tasks[task_id], 7, paths, trial / "artifacts")
                        self.assertFalse(result["passed"])
                        state = json.loads(paths["state"].read_text())
                        self.assertEqual(state["seed"], 7)
                        self.assertEqual(
                            json.loads(paths["events"].read_text().splitlines()[0])["type"],
                            "app_start",
                        )
                        self.assertEqual(
                            Path(f"/proc/{proc.pid}/comm").read_text().strip(), "BenchLab"
                        )
                        if task_id == "MB-11":
                            desktop.capture_screenshot(os.environ["DISPLAY"], trial / "screen.png")
                            import gi

                            gi.require_version("GdkPixbuf", "2.0")
                            from gi.repository import GdkPixbuf

                            image = GdkPixbuf.Pixbuf.new_from_file(str(trial / "screen.png"))
                            geometry = subprocess.check_output(
                                ["xdotool", "getdisplaygeometry"], text=True, timeout=5
                            )
                            self.assertEqual(
                                (image.get_width(), image.get_height()),
                                tuple(map(int, geometry.split())),
                            )
                        runner.kill_bench_apps()
                        proc.wait(timeout=5)
                        self.assertIsNotNone(proc.returncode)
                    finally:
                        desktop.stop_group(proc.pid)
                        proc.wait(timeout=5)
                        desktop.stop_group(proc.pid)
            checks = {
                c.name: c
                for c in desktop.doctor(
                    Path("/nonexistent/driver"), str(root / "socket"), root / "doctor"
                )
            }
            for name in ("display", "at-spi", "ffmpeg"):
                self.assertEqual(checks[name].status, "pass", checks[name].detail)

    @unittest.skipUnless(
        sys.platform == "linux" and os.environ.get("DISPLAY"), "headless X11 session required"
    )
    def test_real_gtk_interactions_are_accepted_by_unchanged_evaluators(self) -> None:
        import desktop_runtime as desktop
        import run_pilot as pilot
        import gi

        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi

        sys.path.insert(0, str(HERE.parent / "probes" / "_common"))
        import benchlab_common as common

        def input_(*argv):
            subprocess.run(["xdotool", *map(str, argv)], check=True, timeout=10)
            time.sleep(0.15)

        def find(name, role=None):
            if role == "push button":
                role = "button"

            def walk(node):
                if node.get_name() == name and (role is None or node.get_role_name() == role):
                    return node
                for i in range(node.get_child_count()):
                    found = walk(node.get_child_at_index(i))
                    if found is not None:
                        return found
                return None

            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                found = walk(Atspi.get_desktop(0))
                if found is not None:
                    bounds = found.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
                    if bounds.width > 0 and bounds.height > 0:
                        return bounds.x + bounds.width // 2, bounds.y + bounds.height // 2
                time.sleep(0.1)
            self.fail(f"accessible control not found: {name} / {role}")

        def click(name, role=None):
            x, y = find(name, role)
            input_("mousemove", x, y, "click", 1)

        def fill(name, value):
            click(name, "text")
            input_("key", "ctrl+a")
            input_("type", "--clearmodifiers", value)

        def combo(name, choices, value):
            click(name, "combo box")
            input_("key", "Home", *(["Down"] * choices.index(value)), "Return")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tasks = pilot.load_tasks()
            for task_id in ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04"):
                with self.subTest(task=task_id):
                    trial = root / task_id
                    (trial / "artifacts").mkdir(parents=True)
                    _, paths = pilot.prepare_probe(tasks[task_id], 7, trial, root / "BenchLab.app")
                    proc = pilot.launch_lab(
                        tasks[task_id],
                        7,
                        paths,
                        root / "BenchLab.app",
                        pilot.clean_app_env(trial / "home"),
                    )
                    try:
                        mode = json.loads(paths["state"].read_text())["mode"]
                        params = getattr(common, "derive_" + mode)(7)
                        if mode == "listdrag":
                            order = list(params["initial"])
                            for target, item in enumerate(params["target"]):
                                source = order.index(item)
                                if source == target:
                                    continue
                                x, y = find(item, "label")
                                tx, ty = find(order[target], "label")
                                input_("mousemove", x, y, "mousedown", 1)
                                input_("mousemove", x + 20, y + 8)
                                input_("mousemove", tx, ty)
                                input_("mousemove", tx + 3, ty + 3)
                                time.sleep(0.3)
                                input_("mouseup", 1)
                                order.pop(source)
                                order.insert(target, item)
                            click("Done", "push button")
                        elif mode == "hover":
                            x, y = find("Hover here to reveal actions", "label")
                            input_("mousemove", x, y)
                            click(params["target"], "push button")
                        elif mode == "tooltip":
                            x, y = find(params["target"], "label")
                            input_("mousemove", x, y)
                            time.sleep(1.5)
                            fill("Code", params["target_code"])
                            click("Submit", "push button")
                        elif mode == "irmodal":
                            fill("Name", params["name"])
                            fill("Email", params["email"])
                            click("Next", "push button")
                            click("Don’t Allow", "push button")
                            combo("Team", common.IR_TEAMS, params["team"])
                            fill("Seats", str(params["seats"]))
                            click("Submit", "push button")
                        elif mode == "irbanner":
                            fill("Quantity", str(params["quantity"]))
                            time.sleep(0.5)
                            click("Later", "push button")
                            combo("Priority", common.PRIORITIES, params["priority"])
                            click("Apply", "push button")
                        elif mode == "irunsaved":
                            click(params["titles"][params["status_index"]], "push button")
                            combo(
                                "Status",
                                [common.IR_INITIAL_STATUS, *common.IR_STATUS_WORDS],
                                params["status"],
                            )
                            click(params["titles"][params["rename_index"]], "push button")
                            click("Save", "push button")
                            fill("Title", params["new_title"])
                            click("Done", "push button")
                            click("Save", "push button")
                        else:
                            fill("Email", params["email"])
                            time.sleep(0.5)
                            click("Reject optional", "push button")
                            combo("Plan", common.IR_PLANS, params["plan"])
                            if params["digest"]:
                                click("Email digest", "check box")
                            click("Sign up", "push button")
                        time.sleep(0.4)
                        result = pilot.evaluate_probe(tasks[task_id], 7, paths, trial / "artifacts")
                        self.assertTrue(result["passed"], result)
                        if mode == "irunsaved":
                            fill("Title", "Unsafe edit")
                            click("Done", "push button")
                            click("Don’t Save", "push button")
                            rejected = pilot.evaluate_probe(
                                tasks[task_id], 7, paths, trial / "artifacts"
                            )
                            self.assertFalse(rejected["passed"])
                            self.assertFalse(rejected["checks"]["no_work_discarded"])
                    finally:
                        desktop.stop_group(proc.pid)
                        proc.wait(timeout=5)

    @unittest.skipUnless(
        sys.platform == "linux" and os.environ.get("DISPLAY") and os.environ.get("CUA_DRIVER_BIN"),
        "CUA_DRIVER_BIN required for driver smoke",
    )
    def test_real_driver_health_and_recording(self) -> None:
        import desktop_runtime as desktop
        import recorder

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = Path(os.environ["CUA_DRIVER_BIN"])
            socket = str(root / "driver.sock")
            proc = desktop.start_driver(binary, socket, root / "daemon", overlay=False)
            capture = recorder.CuaRecorder(
                binary, socket, desktop.desktop_env(root / "daemon" / "home")
            )
            try:
                for check in desktop.doctor(binary, socket, root / "doctor"):
                    self.assertEqual(check.status, "pass", check.detail)
                self.assertTrue(capture.start(root / "recording"), capture.last_error)
                time.sleep(2)
                video = capture.stop()
                self.assertIsNotNone(video, capture.last_error)
                self.assertGreater(recorder.ffprobe_duration(video), 0.5)
                capture.stop()
            finally:
                capture.close()
                capture.close()
                desktop.stop_group(proc.pid)
                proc.wait(timeout=5)
                desktop.stop_group(proc.pid)

    @unittest.skipUnless(
        sys.platform == "linux" and os.environ.get("DISPLAY"), "headless X11 session required"
    )
    def test_sentinel_witness_and_helper_cleanup(self) -> None:
        import desktop_runtime as desktop
        import run_bench as runner
        import run_pilot as pilot

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sentinel = pilot.Sentinel(
                root / "BenchSentinel.app", root / "sentinel.jsonl", ["--user-activity"]
            )
            other = None
            try:
                sentinel.start()
                self.assertIsNotNone(sentinel.proc)
                sentinel.activate()
                sentinel.toggle_armed()
                time.sleep(2.5)
                other = desktop.start_app(
                    [
                        "/usr/bin/python3",
                        "-c",
                        "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk; w=Gtk.Window(title='Witness target'); w.show_all(); Gtk.main()",
                    ],
                    root / "home",
                    root / "other.log",
                )
                time.sleep(1)
                window = subprocess.check_output(
                    ["xdotool", "search", "--name", "^Witness target$"], text=True
                ).splitlines()[0]
                subprocess.run(
                    ["xdotool", "windowactivate", "--sync", window, "mousemove", "310", "270"],
                    check=True,
                    timeout=10,
                )
                time.sleep(2)
                sentinel.toggle_armed()
                summary = sentinel.summary()
                self.assertTrue(summary["available"])
                self.assertGreater(summary["front_changes"], 0)
                self.assertGreater(summary["pointer_max_deviation_px"], 10)
                self.assertGreater(summary["user_activity"]["blocked"], 0)
                for key in (
                    "clipboard_changes",
                    "space_switches",
                    "sheets_shown",
                    "windows_raised",
                ):
                    self.assertIsNotNone(summary[key], key)
                proc = sentinel.proc
                runner.kill_bench_apps()
                proc.wait(timeout=5)
                self.assertIsNotNone(proc.returncode)
            finally:
                sentinel.stop()
                sentinel.stop()
                if other:
                    desktop.stop_group(other.pid)
                    other.wait(timeout=5)


class CodexEventsTest(unittest.TestCase):
    def test_classes_latency_and_tokens(self) -> None:
        events = [
            (1000.0, {"type": "item.started", "item": _mcp("a", "cua", "click", {})}),
            (1250.0, {"type": "item.completed", "item": _mcp("a", "cua", "click", {})}),
            (
                1300.0,
                {
                    "type": "item.started",
                    "item": {
                        "id": "b",
                        "type": "command_execution",
                        "command": "cua-driver get_window_state '{}'",
                    },
                },
            ),
            (
                1400.0,
                {
                    "type": "item.completed",
                    "item": {
                        "id": "b",
                        "type": "command_execution",
                        "command": "cua-driver get_window_state '{}'",
                    },
                },
            ),
            (
                1500.0,
                {
                    "type": "item.completed",
                    "item": {"id": "c", "type": "command_execution", "command": "ls ../evaluator"},
                },
            ),
            (
                1600.0,
                {
                    "type": "item.completed",
                    "item": _mcp(
                        "d",
                        "node_repl",
                        "js",
                        {
                            "code": "await sky.click({app:'x',x:1,y:2}); await sky.type_text({app:'x',text:'a'})"
                        },
                    ),
                },
            ),
            (
                1650.0,
                {"type": "item.completed", "item": _mcp("e", "codex", "list_mcp_resources", {})},
            ),
            (
                1700.0,
                {
                    "type": "item.completed",
                    "item": {
                        "id": "m",
                        "type": "agent_message",
                        "text": "Would you like me to continue?",
                    },
                },
            ),
            (
                1800.0,
                {
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 10,
                        "cached_input_tokens": 4,
                        "output_tokens": 3,
                        "reasoning_output_tokens": 1,
                    },
                },
            ),
        ]
        path = _stamp(events)
        summary = codex_events.summarize(codex_events.read_events(path))
        self.assertTrue(summary["turn_completed"])
        self.assertEqual(summary["tool_calls"]["mcp"], 2)
        self.assertEqual(summary["tool_calls"]["cua_cli_via_shell"], 1)
        self.assertEqual(summary["tool_calls"]["by_class"]["click"], 2)
        self.assertEqual(summary["tool_calls"]["by_class"]["type"], 1)
        self.assertEqual(summary["tool_calls"]["by_class"]["observe"], 1)
        self.assertEqual(summary["shell_commands"], 1)
        self.assertEqual(summary["steps"], 4)
        self.assertTrue(summary["evaluator_read_suspected"])
        self.assertTrue(summary["confirmation_requested"])
        self.assertEqual(
            summary["tokens"], {"input": 10, "cached_input": 4, "output": 3, "reasoning": 1}
        )
        self.assertEqual(summary["action_latency_ms"]["click"]["n"], 2)

    def test_error_event_marks_infrastructure(self) -> None:
        events = [(1.0, {"type": "error", "message": "429 Too Many Requests: rate limit"})]
        summary = codex_events.summarize(codex_events.read_events(_stamp(events)))
        self.assertTrue(summary["infra_error_suspected"])
        self.assertFalse(summary["turn_completed"])

    def test_rollout_usage_takes_last_cumulative_count(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            sessions = Path(tmp) / "sessions" / "2026" / "10" / "05"
            sessions.mkdir(parents=True)
            lines = []
            for total in (100, 250):
                lines.append(
                    json.dumps(
                        {
                            "payload": {
                                "type": "token_count",
                                "info": {
                                    "total_token_usage": {
                                        "input_tokens": total,
                                        "cached_input_tokens": total // 2,
                                        "output_tokens": 7,
                                        "reasoning_output_tokens": 2,
                                    }
                                },
                            }
                        }
                    )
                )
            (sessions / "rollout-x.jsonl").write_text("\n".join(lines) + "\n", "utf-8")
            self.assertEqual(
                codex_events.rollout_usage(Path(tmp)),
                {"input": 250, "cached_input": 125, "output": 7, "reasoning": 2},
            )
            self.assertIsNone(codex_events.rollout_usage(Path(tmp) / "missing"))


_CODEX_NODE_REPL_DOC = Path(
    "/Applications/ChatGPT.app/Contents/Resources/plugins/openai-bundled/plugins/computer-use/.codex-plugin/computer-use-node-repl.md"
)


class ArmsTest(unittest.TestCase):
    @unittest.skipUnless(
        _CODEX_NODE_REPL_DOC.exists(), "needs the ChatGPT app's bundled computer-use plugin"
    )
    def test_arm_homes_differ_only_in_tool_layer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cua = Path(tmp) / "cua"
            native = Path(tmp) / "native"
            arms.render_home("cua-driver-mcp", cua, "m", "high")
            arms.render_home("codex-native-cu", native, "m", "high")
            common = arms.common_config("m", "high")
            for home in (cua, native):
                text = (home / ".codex" / "config.toml").read_text("utf-8")
                self.assertTrue(text.startswith(common))
                self.assertIn('default_tools_approval_mode = "approve"', text)
            self.assertIn("[mcp_servers.cua]", (cua / ".codex/config.toml").read_text("utf-8"))
            self.assertIn(
                "[mcp_servers.node_repl]", (native / ".codex/config.toml").read_text("utf-8")
            )

    def test_exec_flags_are_identical_across_arms(self) -> None:
        argv = arms.codex_argv(Path("/w"), "m")
        self.assertIn("workspace-write", argv)
        self.assertNotIn("--dangerously-bypass-approvals-and-sandbox", argv)

    def test_preamble_forbids_confirmation_requests(self) -> None:
        self.assertIn("do not ask for confirmation", arms.PREAMBLE)


class ScheduleTest(unittest.TestCase):
    def test_schedule_is_deterministic_and_balanced(self) -> None:
        sys.path.insert(0, str(HERE.parent))
        import run_pilot

        first = run_pilot.build_schedule(["A", "B", "C"], ["x", "y"], 3, 7)
        second = run_pilot.build_schedule(["A", "B", "C"], ["x", "y"], 3, 7)
        self.assertEqual(first, second)
        self.assertEqual(len(first), 18)
        for task in "ABC":
            for run in range(3):
                arms_seen = [e["arm"] for e in first if e["task"] == task and e["run_index"] == run]
                self.assertCountEqual(arms_seen, ["x", "y"])
        self.assertEqual([e["order_index"] for e in first], list(range(18)))

    def test_probe_seed_is_stable_and_shared(self) -> None:
        import run_pilot

        self.assertEqual(
            run_pilot.probe_seed("PROBE-FORMS", 0), run_pilot.probe_seed("PROBE-FORMS", 0)
        )
        self.assertNotEqual(
            run_pilot.probe_seed("PROBE-FORMS", 0), run_pilot.probe_seed("PROBE-FORMS", 1)
        )


if __name__ == "__main__":
    unittest.main()
