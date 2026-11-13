# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The raw-macos background-use lint (no macOS frameworks needed): what is blocked, what the grant allows, and what
only warns. The runtime half (raw_mac.py) needs pyobjc and a desktop; it is smoke-tested in the bench guest."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools" / "raw_macos"))
import rawlint_mac as lint  # noqa: E402

READ = "\nreturn raw.describe(el)"


def rules(source: str, granted: bool = False) -> tuple[set[str], bool]:
    report = lint.lint(source, granted=granted)
    return {f.rule for f in report.findings}, report.ok


class LintTest(unittest.TestCase):
    def test_background_ax_routes_pass(self):
        found, ok = rules(
            'el = raw.find(raw.ax.app(1), role="AXButton", title="Apply")[0]\n'
            'raw.ax.set(el, "AXValue", "3")\nraw.ax.perform(el, "AXPress")' + READ
        )
        self.assertTrue(ok)
        self.assertEqual(found, set())

    def test_focus_and_raise_need_the_grant(self):
        for source, rule in (
            ('raw.ax.perform(w, "AXRaise")', "MB2"),
            ('raw.ax.set(w, "AXMain", True)', "MB2"),
            ('raw.ax.set(app, "AXFrontmost", True)', "MB2"),
            ('raw.ax.set(el, "AXFocused", True)', "MB3"),
            ("raw.pointer_move(10, 10)", "MB1"),
            ("raw.pointer_drag(1, 2, 3, 4)", "MB1"),
            ('raw.ax.perform(pop, "AXShowMenu")', "MB7"),
            ('raw.post_click(1, 10, 20, button="right")', "MB7"),
            ('raw.post_click(1, 10, 20, "right")', "MB7"),
        ):
            with self.subTest(source=source):
                found, ok = rules(source + READ)
                self.assertIn(rule, found)
                self.assertFalse(ok)
                _, granted_ok = rules(source + READ, granted=True)
                self.assertTrue(granted_ok)

    def test_side_doors_are_always_blocked(self):
        for source, rule in (
            ("import Quartz", "GUI1"),
            ("import subprocess", "GUI1"),
            ('x = "osascript -e beep"', "MB5"),
            ('x = "tell application \\"Finder\\" to activate"', "MB5"),
            ("open('/etc/passwd')", "GUI1"),
            ("eval('1')", "GUI2"),
            ("getattr(raw, name)", "GUI2"),
            ("raw.ax._x", "GUI2"),
            ('x = "SkyLight"', "MB11"),
            ("ws.openURL_(u)", "MB5"),
        ):
            with self.subTest(source=source):
                found, ok = rules(source, granted=True)
                self.assertIn(rule, found)
                self.assertFalse(ok)

    def test_warnings_run(self):
        for source, rule in (
            ('raw.post_key(1, ["cmd", "l"])', "MB9"),
            ("pb.setString_forType_(s, t)", "MB8"),
        ):
            with self.subTest(source=source):
                found, ok = rules(source + READ)
                self.assertIn(rule, found)
                self.assertTrue(ok)

    def test_read_back_warning(self):
        found, ok = rules('raw.ax.perform(el, "AXPress")')
        self.assertIn("VR1", found)
        self.assertTrue(ok)
        found, _ = rules('raw.ax.perform(el, "AXPress")\nreturn raw.ax.get(el, "AXValue")')
        self.assertNotIn("VR1", found)

    def test_local_names_are_not_api_calls(self):
        found, ok = rules("activate = 1\nsystem = 2\nreturn activate + system")
        self.assertTrue(ok)
        self.assertEqual(found, set())

    def test_no_call_path_to_cua_driver(self):
        """The arm is raw macOS APIs only: any route to Cua Driver is blocked, with or without the grant."""
        for source in (
            'raw.driver("get_window_state", {"pid": 1, "window_id": 2})',
            'raw.driver("click", {"pid": 1, "element_token": "s1:3"})',
            "d = raw.driver\nd('click', {})",
            "f = getattr(raw, 'driver')",
            'x = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"',
            'x = "~/.cua-driver/cua-driver.sock"',
            "import cua_driver",
        ):
            with self.subTest(source=source):
                found, ok = rules(source + READ, granted=True)
                self.assertFalse(ok)
                self.assertTrue(found & {"DRV1", "GUI1"}, found)

    def test_value_helpers_are_actions_that_want_a_read_back(self):
        for source in (
            'raw.set_text(el, "x")',
            'raw.select(el, "High")',
            'raw.set_cell(1, "H4", "x")',
            'raw.menu(1, ["File", "Save"])',
        ):
            with self.subTest(source=source):
                found, ok = rules(source)
                self.assertTrue(ok)
                self.assertIn("VR1", found)
                found, _ = rules(source + "\nreturn raw.text(el)")
                self.assertNotIn("VR1", found)

    def test_mb11_permits_only_the_key_focus_release(self):
        # Zane, 13 Nov 2026: SLPSReleaseKeyFocusWithID is allowed; every other SkyLight/CGS name stays blocked
        found, ok = rules('name = "SLPSReleaseKeyFocusWithID"' + READ)
        self.assertNotIn("MB11", found)
        self.assertTrue(ok)
        for source in (
            'x = "SLPSStealKeyFocusReturningID"',
            'x = "SLPSReleaseKeyFocusWithID SLPSSetFrontProcessWithOptions"',
            'x = "_SLPSSetFrontProcess"',
            'x = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight"',
            'x = "CGSOrderWindow"',
            'x = "SLPSReleaseKeyFocusWithIDExtra"',
            'x = "_SLPSReleaseKeyFocusWithID"',
        ):
            with self.subTest(source=source):
                found, ok = rules(source + READ, granted=True)
                self.assertIn("MB11", found)
                self.assertFalse(ok)
        # the name alone is not a route: loading SkyLight still takes ctypes, which GUI1 blocks
        found, ok = rules(
            "import ctypes\nctypes.CDLL(p).SLPSReleaseKeyFocusWithID(1)" + READ, granted=True
        )
        self.assertIn("GUI1", found)
        self.assertFalse(ok)

    def test_every_rule_cites_a_source(self):
        for rule in lint.RULES.values():
            self.assertTrue(rule.source.strip(), rule.id)
            self.assertIn(rule.severity, {"block", "granted", "warn"})


class MacGrantTest(unittest.TestCase):
    """The raw-macos arm's per-task grant: the hover/drag tasks and, in the foreground condition, the IR tasks with a
    native AppKit pop-up (Zane, 13 Nov 2026). Every other arm keeps the task's plain grant."""

    def test_grants(self):
        import json

        sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
        import bench_v2 as v2

        here = Path(__file__).resolve().parents[1]
        specs = {
            p.parent.name: json.loads(p.read_text()) for p in (here / "probes").glob("*/task.json")
        }
        mac = {t for t in v2.V2_TASKS if v2.foreground_grant(specs[t], macos=True)}
        self.assertEqual(mac, {"MB-09", "MB-10", "MB-11", "IR-01", "IR-02"})
        plain = {t for t in v2.V2_TASKS if v2.foreground_grant(specs[t])}
        self.assertEqual(plain, {"MB-09", "MB-10", "MB-11"})
        for t in v2.V2_TASKS:
            self.assertEqual(v2.foreground_grant_problems(t, specs[t]), [], t)
        bad = {"foreground_grant_macos": {"granted": True, "reason": ""}}
        self.assertTrue(v2.foreground_grant_problems("X", bad))
        self.assertFalse(
            v2.foreground_grant({"foreground_grant_macos": {"granted": True, "reason": "x"}})
        )


class GuardTimeoutTest(unittest.TestCase):
    """A call killed at its time limit while raw.select had a background pop-up menu open is refused under MB7, so a
    disturbance the guard could not finish checking still reaches the call log."""

    def run_killed(self, marker: bool) -> dict:
        import importlib
        import os
        import tempfile

        tmp = Path(tempfile.mkdtemp())
        os.environ["RAW_TMP"] = str(tmp)
        os.environ.pop("RAW_LOG", None)
        os.environ.pop("RAW_TRIAL", None)
        import raw_mcp_mac

        server = importlib.reload(raw_mcp_mac)

        def killed(script, shots, limit, env):
            if marker:
                (shots / server.GUARD_MARKER).write_text("BenchLab")
            return None, "", ""

        server.run_runner = killed
        _, is_error, entry = server.run_script("return 1", 1000)
        self.assertTrue(is_error)
        return entry

    def test_killed_with_the_guard_open_is_refused(self):
        entry = self.run_killed(marker=True)
        self.assertTrue(any(b.startswith("MB7") for b in entry["runtime_blocked"]))

    def test_killed_without_the_guard_is_only_a_timeout(self):
        self.assertEqual(self.run_killed(marker=False)["runtime_blocked"], [])

    def test_the_marker_name_matches_the_runtime(self):
        source = (Path(lint.__file__).parent / "raw_mac.py").read_text()
        import raw_mcp_mac

        self.assertIn(f'GUARD_MARKER = "{raw_mcp_mac.GUARD_MARKER}"', source)


if __name__ == "__main__":
    unittest.main()
