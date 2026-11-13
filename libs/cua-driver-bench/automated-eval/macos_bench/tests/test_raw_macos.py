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

    def test_native_private_api_names_are_raw(self):
        # Zane, 13 Nov 2026 13:38 SGT: any native macOS API, public or private, counts as raw; only Cua Driver is
        # banned. Naming a SkyLight call is no finding (MB11 is retired) ...
        self.assertNotIn("MB11", lint.RULES)
        for source in (
            'name = "SLPSReleaseKeyFocusWithID"',
            'name = "SLPSStealKeyFocusReturningID"',
            'name = "SLPSPostEventRecordTo"',
            'name = "SLEventPostToPid"',
            'x = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight"',
        ):
            with self.subTest(source=source):
                found, ok = rules(source + READ)
                self.assertEqual(found, set())
                self.assertTrue(ok)
        # ... but a script still reaches the frameworks only through the raw runtime (GUI1), and Cua Driver stays out
        found, ok = rules(
            "import ctypes\nctypes.CDLL(p).SLEventPostToPid(1, e)" + READ, granted=True
        )
        self.assertIn("GUI1", found)
        self.assertFalse(ok)
        found, ok = rules('x = "cua-driver"' + READ, granted=True)
        self.assertIn("DRV1", found)
        self.assertFalse(ok)

    def test_window_key_route_is_a_plain_background_call(self):
        found, ok = rules(
            'raw.post_text(1, "scheduled", window_id=7)\nraw.post_key(1, "return", window_id=7)'
            + READ
        )
        self.assertTrue(ok)
        self.assertEqual(found, set())

    def test_popup_open_keys(self):
        # #7551: the runtime refuses these under MB7 when the target's focused element is a pop-up
        self.assertEqual(set(lint.POPUP_OPEN_KEYS), {"down", "up", "space"})
        self.assertIn("Down, Up or Space", lint.RULES["MB7"].why)
        self.assertIn("raw.select", lint.RULES["MB7"].instead)

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

    def run_killed(self, marker: bool, outcome=(None, "", "")) -> dict:
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
            return outcome

        server.run_runner = killed
        _, is_error, entry = server.run_script("return 1", 1000)
        return entry

    def test_killed_with_the_guard_open_is_refused(self):
        entry = self.run_killed(marker=True)
        self.assertTrue(any(b.startswith("MB7") for b in entry["runtime_blocked"]))

    def test_crashed_with_the_guard_open_is_refused(self):
        entry = self.run_killed(marker=True, outcome=(-11, "", "Segmentation fault"))
        self.assertTrue(any(b.startswith("MB7") for b in entry["runtime_blocked"]))

    def test_a_complete_result_keeps_its_own_findings(self):
        done = '{"ok": true, "value": 1, "runtime_blocked": [], "watch": {"changed": ["top"], "lines": ["x"]}}'
        entry = self.run_killed(marker=True, outcome=(0, done + "\n", ""))
        self.assertEqual(entry["runtime_blocked"], [])
        self.assertEqual(entry["watch_changed"], ["top"])

    def test_a_cut_off_result_is_refused(self):
        entry = self.run_killed(marker=True, outcome=(0, '{"ok": true, "watch": {"cha', ""))
        self.assertTrue(any(b.startswith("MB7") for b in entry["runtime_blocked"]))

    def test_a_result_without_the_hook_report_is_refused(self):
        # the runner's own fallback when run() raised: it carries no watch report
        entry = self.run_killed(
            marker=True, outcome=(0, '{"ok": false, "error": "runtime failure"}\n', "")
        )
        self.assertTrue(any(b.startswith("MB7") for b in entry["runtime_blocked"]))

    def test_killed_without_the_guard_is_only_a_timeout(self):
        self.assertEqual(self.run_killed(marker=False)["runtime_blocked"], [])

    def test_the_marker_name_matches_the_runtime(self):
        source = (Path(lint.__file__).parent / "raw_mac.py").read_text()
        import raw_mcp_mac

        self.assertIn(f'GUARD_MARKER = "{raw_mcp_mac.GUARD_MARKER}"', source)


if __name__ == "__main__":
    unittest.main()


class CalcReadBackTest(unittest.TestCase):
    """raw.set_cell's read-back: a time or number Calc reformatted counts only if it is the same value. The helpers
    are pure, so they are loaded from raw_mac.py's source without the macOS frameworks it imports."""

    def shows(self):
        import ast
        import re

        source = (Path(lint.__file__).parent / "raw_mac.py").read_text()
        wanted = {"_clock", "_number", "_calc_shows"}
        nodes = [
            n for n in ast.parse(source).body if isinstance(n, ast.FunctionDef) and n.name in wanted
        ]
        self.assertEqual({n.name for n in nodes}, wanted)
        from decimal import Decimal

        ns: dict = {"re": re, "Decimal": Decimal}
        exec(compile(ast.Module(nodes, []), "raw_mac.py", "exec"), ns)  # noqa: S102
        return ns["_calc_shows"]

    def test_read_back(self):
        shows = self.shows()
        for value, got, ok in (
            ("cancelled", "cancelled", True),
            ("13:00", "01:00:00 PM", True),
            ("11:00", "11:00:00 AM", True),
            ("12:00", "12:00:00 PM", True),
            ("13:00", "09:00:00 AM", False),  # a write that did not land on an old time
            ("'13:00", "13:00", True),
            ("'13:00", "01:00:00 PM", False),
            ("3", "4", False),
            ("1,000", "1000", True),
            ("1,5", "15", False),  # a decimal comma is not compared
            ("1.50", "1.5", True),
            ("12345678901234567890", "12345678901234567891", False),
            ("1e400", "1e401", False),
            ("", "", True),
            ("x", "", False),
        ):
            with self.subTest(value=value, got=got):
                self.assertEqual(shows(value, got), ok)


class WindowKeyTest(unittest.TestCase):
    """The window-key route's pure decisions, loaded from raw_mac.py's source with the AX calls stubbed."""

    def load(self, names: set[str], **env):
        import ast

        source = (Path(lint.__file__).parent / "raw_mac.py").read_text()
        nodes = [
            n for n in ast.parse(source).body if isinstance(n, ast.FunctionDef) and n.name in names
        ]
        self.assertEqual({n.name for n in nodes}, names)
        exec(compile(ast.Module(nodes, []), "raw_mac.py", "exec"), env)  # noqa: S102
        return env

    def test_settled_needs_a_focused_element(self):
        # a never-active LibreOffice reports its document as focused with no focused element and drops keys
        for focused_window, focused_element, settled in (
            (7, "el", True),
            (7, None, False),
            (8, "el", False),
            (None, None, False),
        ):
            env = self.load(
                {"_key_settled"},
                _focused_window_id=lambda pid, w=focused_window: w,
                _ax_get=lambda el, attr, e=focused_element: e,
                AX=type(
                    "AX", (), {"AXUIElementCreateApplication": staticmethod(lambda pid: "app")}
                ),
            )
            with self.subTest(window=focused_window, element=focused_element):
                self.assertEqual(env["_key_settled"](1, 7), settled)

    def test_calc_window_by_title_or_id(self):
        windows = {"w1": ("Untitled 1", 11), "w2": ("lighthouse-replan.ods", 22)}

        class Finder:
            def find(self, w, **kw):
                return [w]

        env = self.load(
            {"_calc_window"},
            AX=type("AX", (), {"AXUIElementCreateApplication": staticmethod(lambda pid: "app")}),
            _ax_get=lambda el, attr: list(windows) if attr == "AXWindows" else windows[el][0],
            _ax_window_id=lambda pid, w: windows[w][1],
            Raw=Finder,
            Any=object,
        )
        pick = env["_calc_window"]
        self.assertEqual(pick(1, None), "w1")
        self.assertEqual(pick(1, "lighthouse"), "w2")
        self.assertEqual(pick(1, 22), "w2")
        self.assertEqual(pick(1, 11), "w1")
        with self.assertRaises(LookupError):
            pick(1, 33)
        with self.assertRaises(LookupError):
            pick(1, True)  # a bool is not a window id (and no title contains "True")


class Utf16Test(unittest.TestCase):
    def test_ax_range_length(self):
        import ast

        source = (Path(lint.__file__).parent / "raw_mac.py").read_text()
        node = next(
            n
            for n in ast.parse(source).body
            if isinstance(n, ast.FunctionDef) and n.name == "_utf16_len"
        )
        env: dict = {}
        exec(compile(ast.Module([node], []), "raw_mac.py", "exec"), env)  # noqa: S102
        for text, n in (("13:00", 5), ("", 0), ("a\U0001f600b", 4), ("\u00e9", 1)):
            with self.subTest(text=text):
                self.assertEqual(env["_utf16_len"](text), n)
