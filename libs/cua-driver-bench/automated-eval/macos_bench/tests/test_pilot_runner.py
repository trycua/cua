# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Unit tests for the pilot runner pieces that do not need a GUI or a model."""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import tomllib
import sys
import time
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))


import arms  # noqa: E402
import codex_events  # noqa: E402


def _x11_ready() -> bool:
    if sys.platform != "linux" or not os.environ.get("DISPLAY"):
        return False
    try:
        return (
            subprocess.run(
                ["xdpyinfo", "-display", os.environ["DISPLAY"]],
                capture_output=True,
                timeout=10,
            ).returncode
            == 0
        )
    except (OSError, subprocess.SubprocessError):
        return False


def _has_gi() -> bool:
    try:
        import gi  # noqa: F401
    except ImportError:
        return False
    return True


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

    def test_linux_environment_keeps_default_xauthority_after_home_swap(self) -> None:
        import desktop_runtime as desktop
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as tmp:
            orig = Path(tmp) / "orig"
            orig.mkdir()
            (orig / ".Xauthority").write_text("x")
            environ = {"HOME": str(orig), "DISPLAY": ":92"}
            with (
                patch.object(desktop, "detect_platform", return_value="linux"),
                patch.dict(os.environ, environ, clear=True),
            ):
                env = desktop.desktop_env(Path(tmp) / "private")
            self.assertEqual(env["XAUTHORITY"], str(orig / ".Xauthority"))
            self.assertEqual(env["HOME"], str(Path(tmp) / "private"))

    def test_macos_environment_uses_closed_path_and_locale(self) -> None:
        import desktop_runtime as desktop
        import run_pilot as pilot
        from unittest.mock import patch

        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(desktop, "detect_platform", return_value="macos"),
            patch.dict(os.environ, {"PATH": "/injected/bin", "LANG": "injected", "DISPLAY": ":92"}),
        ):
            env = desktop.desktop_env(Path("/tmp/home"))
            self.assertEqual(env["PATH"], "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
            self.assertEqual(env["LANG"], "en_US.UTF-8")
            self.assertNotIn("DISPLAY", env)
            home = Path(tmp) / "home"
            app_env = pilot.clean_app_env(home)
            self.assertEqual(app_env["PATH"], f"/opt/homebrew/bin:{arms.CLOSED_PATH}")
            self.assertEqual(app_env["TMPDIR"], str(home / "tmp"))
            self.assertTrue((home / "tmp").is_dir())

    def test_linux_environment_uses_trial_tmpdir(self) -> None:
        import desktop_runtime as desktop
        import run_pilot as pilot
        from unittest.mock import patch

        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(desktop, "detect_platform", return_value="linux"),
            patch.dict(os.environ, {"TMPDIR": "/runner/tmp"}),
        ):
            home = Path(tmp) / "home"
            env = pilot.clean_app_env(home)
            self.assertEqual(env["TMPDIR"], str(home / "tmp"))
            self.assertTrue((home / "tmp").is_dir())

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

    @unittest.skipUnless(_x11_ready() and _has_gi(), "headless X11 session with GTK required")
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

    @unittest.skipUnless(_x11_ready() and _has_gi(), "headless X11 session with GTK required")
    def test_consent_dialog_survives_window_close(self) -> None:
        script = r"""
import argparse, json, sys, tempfile
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk
import benchlab

def pump(n=60):
    for _ in range(n):
        while Gtk.events_pending():
            Gtk.main_iteration_do(False)
        import time; time.sleep(0.01)

def dlg():
    for w in Gtk.Window.list_toplevels():
        if w.get_title() == "Cookie preferences" and w.get_visible():
            return w
    return None

with tempfile.TemporaryDirectory() as tmp:
    lab = benchlab.Lab(argparse.Namespace(
        mode="irconsent", seed=7, state=Path(tmp) / "s.json", events=Path(tmp) / "e.jsonl"))
    lab.fields["email"].set_text("a@example.com")
    for _ in range(100):
        pump(5)
        if dlg():
            break
    d = dlg()
    out = {"opened": d is not None}
    d.close(); pump()
    out["after_close"] = [dlg() is not None, lab.state["consent_shown"], list(lab.state["consent_answers"])]
    d.response(-1); d.response(99); pump()
    out["after_invalid"] = [dlg() is not None, list(lab.state["consent_answers"])]
    d.response(0); pump()
    out["after_button"] = [dlg() is not None, list(lab.state["consent_answers"])]
    lab.signup(); pump()
    out["submit"] = [lab.state["submit_count"], lab.state["submits_blocked"]]
    print(json.dumps(out))
"""
        with tempfile.TemporaryDirectory() as tmp:
            proc = subprocess.run(
                [sys.executable, "-c", script, str(HERE.parent / "linux")],
                capture_output=True,
                text=True,
                timeout=60,
                cwd=tmp,
            )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout.strip().splitlines()[-1])
        self.assertTrue(out["opened"])
        self.assertEqual(out["after_close"], [True, True, []])
        self.assertEqual(out["after_invalid"], [True, []])
        self.assertFalse(out["after_button"][0])
        self.assertEqual([a["answer"] for a in out["after_button"][1]], ["reject"])
        self.assertEqual(out["submit"], [1, 0])

    @unittest.skipUnless(_x11_ready() and _has_gi(), "headless X11 session with GTK required")
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
            # at-spi2-core < 2.53 reports GTK buttons as "push button"; >= 2.53 as "button".
            buttons = {"button", "push button"}
            roles = buttons if role in buttons else {role}

            def walk(node):
                if node.get_name() == name and (role is None or node.get_role_name() in roles):
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
        _x11_ready() and os.environ.get("CUA_DRIVER_BIN"),
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

    @unittest.skipUnless(_x11_ready(), "headless X11 session required")
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
                windows = subprocess.check_output(
                    ["xdotool", "search", "--name", "^User activity$"], text=True, timeout=10
                ).splitlines()
                self.assertTrue(windows)
                sentinel.activate()
                # A display reused across runs may still hold the pointer at the move target below.
                subprocess.run(["xdotool", "mousemove", "10", "10"], check=True, timeout=10)
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


class SentinelVisibilityTest(unittest.TestCase):
    @unittest.skipUnless(_has_gi(), "GTK required")
    def test_only_visible_current_workspace_windows_count(self) -> None:
        from unittest import mock

        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "linux"))
        import sentinel

        props = {
            1: {"_NET_WM_DESKTOP": "0", "_NET_WM_STATE": "_NET_WM_STATE_ABOVE"},
            2: {"_NET_WM_DESKTOP": "1", "_NET_WM_STATE": ""},
            3: {"_NET_WM_DESKTOP": "4294967295", "_NET_WM_STATE": ""},
            4: {"_NET_WM_DESKTOP": "0", "_NET_WM_STATE": "_NET_WM_STATE_HIDDEN"},
        }
        with mock.patch.object(sentinel, "xprop", lambda _i, w, *_a: props[int(w, 16)]):
            seen = [w for w in props if sentinel.visible_here(w, "0")]
        self.assertEqual(seen, [1, 3])


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


class RecorderTest(unittest.TestCase):
    def setUp(self) -> None:
        import recorder

        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.video = Path(self.tmp.name) / "v.mp4"
        self.video.write_bytes(b"x")
        self.rec = recorder.CuaRecorder(Path("/nonexistent"), "sock", {})
        self.rec.active_dir = Path(self.tmp.name)
        self.outcomes: list = []
        self.calls: dict[str, int] = {}
        self.closed = 0
        test = self

        class Fake:
            def call_tool(self, name, args, timeout=0):
                test.calls[name] = test.calls.get(name, 0) + 1
                if name == "get_recording_state":
                    raise TimeoutError("state")
                outcome = test.outcomes.pop(0)
                if isinstance(outcome, type):
                    raise outcome("boom")
                return outcome

            def close(self):
                test.closed += 1

        def connect():
            self.rec.client = Fake()
            return self.rec.client

        self.rec._connect = connect

    def test_stop_retries_after_transient_failure(self) -> None:
        self.outcomes = [TimeoutError, {"last_video_path": str(self.video)}]
        self.assertIsNone(self.rec.stop())
        self.assertIsNotNone(self.rec.active_dir)
        self.assertTrue(self.rec.last_error.startswith("TimeoutError"))
        self.assertEqual(self.rec.stop(), self.video)
        self.assertIsNone(self.rec.active_dir)
        self.rec.close()
        self.assertEqual(self.calls["stop_recording"], 2)
        self.assertEqual(self.closed, 1)

    def test_close_retries_unconfirmed_stop(self) -> None:
        self.outcomes = [TimeoutError, TimeoutError, {"last_video_path": str(self.video)}]
        self.rec.stop()
        self.rec.stop()
        self.rec.close()
        self.assertEqual(self.calls["stop_recording"], 3)
        self.assertEqual(self.rec.last_video, self.video)
        self.assertIsNone(self.rec.active_dir)
        self.assertEqual(self.closed, 1)

    def test_state_lookup_failure_after_confirmed_stop_clears_state(self) -> None:
        self.outcomes = [{}]
        self.assertIsNone(self.rec.stop())
        self.assertIsNone(self.rec.active_dir)
        self.rec.close()
        self.assertEqual(self.calls["stop_recording"], 1)


class ArmsTest(unittest.TestCase):
    def test_gateway_common_config_matches_original_text(self) -> None:
        with mock.patch.dict(os.environ, {"CDB_CODEX_AUTH": "gateway"}):
            self.assertEqual(
                arms.common_config("m", "high"),
                """model = "m"
model_provider = "litellm"
model_reasoning_effort = "high"
approval_policy = "never"
sandbox_mode = "workspace-write"
mcp_optional_startup_grace_ms = 30000

""",
            )

    def test_arm_homes_differ_only_in_tool_layer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("skills/computer-use/SKILL.md", ".codex-plugin/computer-use-node-repl.md"):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture")
            source = root / ".codex/auth.json"
            source.parent.mkdir()
            source.write_bytes(b"private")
            source.chmod(0o000)
            before = source.stat().st_mtime_ns
            with mock.patch.multiple(
                arms,
                CODEX_APP_RESOURCES=root,
                CODEX_CU_PLUGIN=root,
                CUA_DRIVER_SKILL=root / "skills/computer-use",
            ):
                for arm in arms.ARMS:
                    configs = {}
                    for mode in ("gateway", "chatgpt"):
                        home = root / f"{arm}-{mode}"
                        with mock.patch.dict(
                            os.environ,
                            {
                                "CDB_CODEX_AUTH": mode,
                                "CDB_CODEX_AUTH_FILE": str(source),
                                "LITELLM_MASTER_KEY": "secret",
                                "CDB_PILOT_PASS_GATEWAY_KEY": "1",
                            },
                        ):
                            env = arms.render_home(arm, home, "m", "high")
                            text = (home / ".codex/config.toml").read_text()
                            self.assertTrue(text.startswith(arms.common_config("m", "high")))
                            self.assertEqual("LITELLM_MASTER_KEY" in env, mode == "gateway")
                            link = home / ".codex/auth.json"
                            self.assertEqual(link.is_symlink(), mode == "chatgpt")
                            if mode == "chatgpt":
                                self.assertEqual(os.readlink(link), str(source))
                            configs[mode] = tomllib.loads(text)
                    gateway = configs["gateway"]
                    self.assertEqual(gateway.pop("model_provider"), "litellm")
                    gateway.pop("model_providers")
                    self.assertNotIn("cli_auth_credentials_store", gateway)
                    self.assertEqual(configs["chatgpt"].pop("cli_auth_credentials_store"), "file")
                    self.assertEqual(gateway, configs["chatgpt"])
                    if arm == "codex-native-cu":
                        self.assertIn("notify", gateway)
                with mock.patch.dict(
                    os.environ, {"CDB_CODEX_AUTH": "chatgpt", "HOME": tmp}, clear=True
                ):
                    env = arms.render_home("cua-driver-mcp", root / "default", "m", "high")
                    link = Path(env["CODEX_HOME"]) / "auth.json"
                    self.assertTrue(link.is_symlink())
                    self.assertEqual(os.readlink(link), str(source))
                self.assertEqual(source.stat().st_mtime_ns, before)
                source.chmod(0o600)
                self.assertEqual(source.read_bytes(), b"private")
                source.unlink()
                with mock.patch.dict(
                    os.environ, {"CDB_CODEX_AUTH": "chatgpt", "CDB_CODEX_AUTH_FILE": str(source)}
                ):
                    with self.assertRaises(FileNotFoundError):
                        arms.render_home("cua-driver-mcp", root / "missing", "m", "high")

    def test_exec_flags_are_identical_across_arms(self) -> None:
        argv = arms.codex_argv(Path("/w"), "m")
        self.assertIn("workspace-write", argv)
        self.assertNotIn("--dangerously-bypass-approvals-and-sandbox", argv)

    def test_preamble_forbids_confirmation_requests(self) -> None:
        self.assertIn("do not ask for confirmation", arms.PREAMBLE)


class ScheduleTest(unittest.TestCase):
    def test_pilot_rows_and_manifest_record_auth_mode(self) -> None:
        import run_pilot

        args = argparse.Namespace(
            model="m", effort="high", idle_min=0, max_idle_wait_h=1, no_sentinel=True
        )
        task = argparse.Namespace(id="T", group="bench", tags=[], timeout_s=60)
        for mode, replaced in (("gateway", False), ("chatgpt", False), ("chatgpt", True)):

            def fail_setup(compare, task, trial_dir, workspace):
                home = trial_dir / "home/.codex"
                home.mkdir(parents=True)
                arms.link_chatgpt_auth(home)
                if replaced:
                    (home / "auth.json").unlink()
                    (home / "auth.json").write_text("FAKE_TOKEN")
                raise RuntimeError("unavailable fixture")

            with (
                tempfile.TemporaryDirectory() as tmp,
                mock.patch.dict(
                    os.environ,
                    {
                        "CDB_CODEX_AUTH": mode,
                        "CDB_CODEX_AUTH_FILE": str(Path(tmp) / "shared-auth.json"),
                    },
                ),
                mock.patch.object(run_pilot, "wait_for_idle", return_value=1000),
                mock.patch.object(run_pilot, "Sentinel") as sentinel,
                mock.patch.object(run_pilot, "prepare_bench", side_effect=fail_setup),
                mock.patch.object(run_pilot, "sweep_processes"),
                mock.patch.object(run_pilot.subprocess, "run") as command,
            ):
                source = Path(tmp) / "shared-auth.json"
                source.write_text("FAKE_TOKEN")
                sentinel.return_value.enabled = False
                sentinel.return_value.summary.return_value = {}
                command.return_value.stdout = "version"
                row = run_pilot.run_trial(
                    args, None, task, "codex-native-cu", 1, 0, Path(tmp), None
                )
                manifest = run_pilot.environment_manifest(args)
                self.assertEqual(row["codex_auth"], mode)
                self.assertEqual(manifest["codex_auth"], mode)
                saved = json.loads(next((Path(tmp) / "trials").glob("*/trial.json")).read_text())
                self.assertEqual(saved["codex_auth"], mode)
                if replaced:
                    self.assertIn("symlink replaced", row["notes"])
                for secret in (str(source), "FAKE_TOKEN", "http://127.0.0.1:4000/v1"):
                    self.assertNotIn(secret, json.dumps([row, manifest, saved]))

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


def _has_session_tools() -> bool:
    import shutil

    return sys.platform == "linux" and all(
        shutil.which(t) for t in ("xvfb-run", "openbox", "picom", "xprop", "gdbus")
    )


FAKE_DRIVER = """#!/usr/bin/python3
import json, sys, time
a = sys.argv[1:]
if "--version" in a:
    print("cua-driver 9.9.9")
elif a and a[0] == "serve":
    time.sleep(600)
elif a and a[0] == "status":
    pass
elif "health_report" in a:
    print(json.dumps({"build": {"version": "9.9.9", "git_sha": "abc", "exe_sha256": "x"},
        "checks": [{"name": "ax_capability", "status": "pass"},
                   {"name": "screen_capture_capability", "status": "pass"}]}))
"""


@unittest.skipUnless(_has_session_tools(), "xvfb-run, openbox, picom, xprop and gdbus required")
class LinuxLauncherTest(unittest.TestCase):
    script = HERE.parent / "launch" / "linux_session.sh"

    def launch(self, evidence: Path, *command: str) -> subprocess.CompletedProcess:
        env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
        return subprocess.run(
            ["bash", str(self.script), "--evidence", str(evidence), "--", *command],
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
        )

    def test_launcher_owns_and_reaps_its_session(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            done = self.launch(
                ev,
                "bash",
                "-c",
                'xprop -root _NET_SUPPORTING_WM_CHECK > "$CDB_SESSION_DIR/wm.txt"',
            )
            self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
            self.assertIn("window id", (ev / "wm.txt").read_text())
            address = (ev / "at-spi-address.txt").read_text()
            self.assertTrue("unix:path=" in address or "unix:abstract=" in address, address)
            entries = [json.loads(x) for x in (ev / "pids.jsonl").read_text().splitlines()]
            self.assertEqual({e["role"] for e in entries}, {"openbox", "picom"})
            for entry in entries:
                with self.assertRaises(ProcessLookupError):
                    os.kill(entry["pid"], 0)
            self.assertEqual((ev / "exit").read_text().strip(), "command=0 evidence=0")

    def test_status_passes_through(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            done = self.launch(ev, "bash", "-c", "exit 7")
            self.assertEqual(done.returncode, 7)
            self.assertIn("command=7", (ev / "exit").read_text())
            self.assertTrue((ev / "launch.log").exists())

    def test_surviving_child_fails_the_check(self) -> None:
        def leaked_pids(ev: Path) -> list[int]:
            try:
                lines = (ev / "pids.jsonl").read_text().splitlines()
            except OSError:
                return []
            return [json.loads(x)["pid"] for x in lines if json.loads(x).get("role") == "leak"]

        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            try:
                done = self.launch(
                    ev,
                    "bash",
                    "-c",
                    'setsid sleep 300 & echo "{\\"pid\\": $!, \\"pgid\\": $!, \\"role\\": \\"leak\\"}"'
                    ' >> "$CDB_BENCH_PID_LOG"; sleep 0.5',
                )
                leaked = leaked_pids(ev)
                self.assertNotEqual(done.returncode, 0)
                self.assertEqual(len(leaked), 1)
                os.kill(leaked[0], 0)
                self.assertIn("command=0 evidence=1", (ev / "exit").read_text())
            finally:
                for pid in leaked_pids(ev):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except OSError:
                        pass

    def test_sigterm_to_launcher_tears_down_and_reports(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
            proc = subprocess.Popen(
                ["bash", str(self.script), "--evidence", str(ev), "--", "bash", "-c",
                 'echo $$ > "$CDB_SESSION_DIR/cmd.pid"; sleep 300'],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 60
                while not (ev / "cmd.pid").exists() and time.monotonic() < deadline:
                    time.sleep(0.2)
                self.assertTrue((ev / "cmd.pid").exists())
                cmd = int((ev / "cmd.pid").read_text())
                proc.send_signal(signal.SIGTERM)
                self.assertNotEqual(proc.wait(60), 0)
                self.assertTrue((ev / "exit").exists())
                time.sleep(0.5)
                with self.assertRaises(ProcessLookupError):
                    os.kill(cmd, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()

    @unittest.skipUnless(_x11_ready() and _has_gi(), "headless X11 session with GTK required")
    def test_doctor_private_daemon_is_reaped(self) -> None:
        import desktop_runtime as desktop

        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "cua-driver"
            binary.write_text(FAKE_DRIVER)
            binary.chmod(0o755)
            out = Path(tmp) / "out"
            with mock.patch("sys.stdout"):
                code = desktop.main(["doctor", "--cua-bin", str(binary), "--out", str(out)])
            report = json.loads((out / "doctor.json").read_text())
            self.assertEqual(code, 0, report)
            self.assertTrue(report["ok"])
            self.assertEqual(report["daemon"], "private")
            self.assertTrue(report["at_spi_address"])
            with self.assertRaises(ProcessLookupError):
                os.kill(report["daemon_pid"], 0)


@unittest.skipUnless(sys.platform == "linux", "Linux desktop")
class DoctorExistingSocketTest(unittest.TestCase):
    def test_missing_display_and_socket_fails_without_starting_anything(self) -> None:
        import desktop_runtime as desktop

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out"
            with (
                mock.patch.dict(os.environ, {"DISPLAY": ""}),
                mock.patch.object(desktop, "start_driver", side_effect=AssertionError),
                mock.patch.object(desktop, "start_app", side_effect=AssertionError),
            ):
                code = desktop.main(
                    ["doctor", "--cua-bin", "/nonexistent/cua-driver",
                     "--socket", str(Path(tmp) / "missing.sock"), "--out", str(out)]
                )  # fmt: skip
            report = json.loads((out / "doctor.json").read_text())
            self.assertEqual(code, 1)
            self.assertEqual(report["daemon"], "existing")
            self.assertIsNone(report["daemon_pid"])
            display = next(c for c in report["checks"] if c["name"] == "display")
            self.assertEqual(display["status"], "fail")


if __name__ == "__main__":
    unittest.main()
