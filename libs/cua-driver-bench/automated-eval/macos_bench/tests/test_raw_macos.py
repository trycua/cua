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

    def test_every_rule_cites_a_source(self):
        for rule in lint.RULES.values():
            self.assertTrue(rule.source.strip(), rule.id)
            self.assertIn(rule.severity, {"block", "granted", "warn"})


if __name__ == "__main__":
    unittest.main()
