# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The raw-linux prototype's background-use lint: what it blocks, warns about and lets through."""

from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE / "tools" / "raw_linux"))
import rawlint  # noqa: E402
import runner  # noqa: E402

def granted(source: str) -> "rawlint.Report":
    """Lint as on a task whose definition grants foreground (MB-09/10/11)."""
    return rawlint.lint(source, granted=True)


def rules(source: str) -> list[tuple[str, str]]:
    return sorted((f.rule, f.severity) for f in rawlint.lint(source).findings)


class RawLintTest(unittest.TestCase):
    def assertBlocked(self, source: str, rule: str, grant: bool = False) -> None:
        report = rawlint.lint(source, granted=grant)
        self.assertFalse(report.ok, source)
        self.assertIn(rule, {f.rule for f in report.blocked}, source)
        text = report.render()
        self.assertIn("NOT run", text)
        self.assertIn(rule, text)
        self.assertIn("source:", text)
        self.assertIn("instead:", text)

    def test_atspi_action_with_read_back_is_clean(self):
        src = (
            'b = raw.find(raw.app("BenchLab"), role="button", name="Apply")[0]\n'
            "b.get_action_iface().do_action(0)\n"
            "return raw.describe(b)\n"
        )
        self.assertEqual(rules(src), [])

    def test_pointer_focus_and_key_theft_block_without_the_tasks_grant(self):
        cases = {
            "raw.xtst.XTestFakeMotionEvent(raw.dpy, -1, 5, 5, 0)": "BG1",
            "raw.xtst.XTestFakeButtonEvent(raw.dpy, 1, 1, 0)": "BG1",
            "raw.x11.XWarpPointer(raw.dpy, 0, raw.root, 0, 0, 0, 0, 5, 5)": "BG1",
            'Atspi.generate_mouse_event(5, 5, "b1c")': "BG1",
            'raw.xdotool("mousemove", 5, 5)': "BG1",
            'raw.xdotool("click", 1)': "BG1",
            "raw.x11.XSetInputFocus(raw.dpy, 5, 1, 0)": "BG2",
            "raw.x11.XRaiseWindow(raw.dpy, 5)": "BG2",
            'raw.x11.XInternAtom(raw.dpy, b"_NET_ACTIVE_WINDOW", 0)': "BG2",
            'raw.xdotool("windowactivate", "5")': "BG2",
            'x = "wmctrl -a BenchLab"': "BG2",
            'raw.xdotool("key", "Return")': "BG3",
            'raw.xdotool("type", "hello")': "BG3",
            "raw.xtst.XTestFakeKeyEvent(raw.dpy, 36, 1, 0)": "BG3",
            'getattr(raw.xtst, "XTestFakeKeyEvent")(raw.dpy, 36, 1, 0)': "BG3",
        }
        for src, rule in cases.items():
            with self.subTest(src=src):
                self.assertBlocked(src + "\nreturn raw.pointer()", rule)

    def test_the_tasks_grant_admits_pointer_focus_and_keys_but_never_grabs(self):
        report = granted('raw.xdotool("mousemove", 5, 5)\nreturn raw.pointer()')
        self.assertTrue(report.ok)
        self.assertIn("FOREGROUND (granted by the task)", report.render())
        self.assertBlocked("raw.x11.XGrabKeyboard(raw.dpy, raw.root, 1, 1, 1, 0)", "BG4", grant=True)
        self.assertBlocked("raw.x11.XGrabServer(raw.dpy)", "BG4", grant=True)

    def test_the_script_cannot_grant_itself_foreground(self):
        # the old self-declaration is gone: it is an unknown helper, and the pointer call stays blocked
        src = 'raw.escalate_foreground("I need the pointer")\nraw.xdotool("click", 1)\nreturn raw.pointer()'
        self.assertBlocked(src, "BG1")
        self.assertIn("this task does not grant foreground", rawlint.lint(src).render())

    def test_every_route_to_cua_driver_blocks_with_or_without_the_grant(self):
        for src in (
            'raw.driver("click", {"pid": 1})',
            'raw.driver("get_window_state", {"pid": 1})',
            'd = raw.driver\nd("list_windows", {})',
            'getattr(raw, "driver")("list_windows", {})',
            'raw.xdotool("search", "--name", "cua-driver")',
            'x = "/tmp/cdb-bench-cua-main.sock"',
            "import cua_driver",
            "from cua_driver import tools",
        ):
            for grant in (False, True):
                with self.subTest(src=src, grant=grant):
                    self.assertBlocked(src + "\nreturn 1", "DRV1", grant=grant)
        # a local that happens to be called "driver" is not a route
        self.assertNotIn("DRV1", {f.rule for f in rawlint.lint("driver = 3\nreturn driver").findings})
        self.assertNotIn("DRV1", {f.rule for f in rawlint.lint('x = "driver"\nreturn x').findings})
        self.assertNotIn("BG10", rawlint.RULES)
        self.assertFalse(hasattr(rawlint, "DRIVER_TOOLS"))

    def test_runtime_refuses_raw_driver_under_drv1(self):
        """The runtime refuses raw.driver however it was reached (a name the lint could not see)."""
        fake = mock.MagicMock()
        with mock.patch.dict(sys.modules, {"gi": fake, "gi.repository": fake}):
            sys.modules.pop("raw", None)
            with mock.patch("ctypes.CDLL"), mock.patch("ctypes.util.find_library", return_value="x"):
                import raw  # noqa: PLC0415

                try:
                    with self.assertRaises(raw.Blocked) as caught:
                        raw.Raw.__getattr__(object.__new__(raw.Raw), "driver")
                    self.assertIn("DRV1", str(caught.exception))
                    with self.assertRaises(AttributeError):
                        raw.Raw.__getattr__(object.__new__(raw.Raw), "no_such_helper")
                finally:
                    sys.modules.pop("raw", None)

    def test_foreground_helpers_need_the_tasks_grant(self):
        cases = {
            "raw.activate(5)": "BG2",
            'raw.key(5, "ctrl+s")': "BG3",
            'raw.type_text(5, "B7\\n")': "BG3",
            "raw.click(5, 10, 10)": "BG1",
            "raw.scroll(5, 10, 10, 3)": "BG1",
            "raw.drag(1, 2, 3, 4)": "BG1",
            'getattr(raw, "drag")(1, 2, 3, 4)': "BG1",
            "r = raw\nf = r.key\nf(5, 'a')": "BG3",
            "q = r = raw\nq.click(5, 1, 1)": "BG1",
            "r = raw\nq = r\ngetattr(q, 'type_text')(5, 'x')": "BG3",
        }
        for src, rule in cases.items():
            with self.subTest(src=src):
                self.assertBlocked(src + "\nreturn raw.screenshot()", rule)
                self.assertTrue(granted(src + "\nreturn raw.screenshot()").ok)
        # a helper action needs a read-back like a raw one; raw.cells is a read
        self.assertIn("VR1", {f.rule for f in granted('raw.key(5, "ctrl+s")').findings})
        self.assertEqual(granted('raw.type_text(5, "x")\nreturn raw.cells(a, "A1")').findings[0].rule, "BG3")
        self.assertNotIn("VR1", {f.rule for f in granted('raw.type_text(5, "x")\nreturn raw.cells(a, "A1")').findings})
        # reads and AT-SPI need no grant
        self.assertTrue(rawlint.lint('return raw.cells(raw.app("soffice"), "A1:F9")').ok)

    def test_gui_only_side_doors_block(self):
        for src in (
            "import os",
            "import subprocess",
            "from pathlib import Path",
            "open('/etc/hostname').read()",
            "import ctypes\nctypes.CDLL('libX11.so.6')",
            "eval('1')",
            "__import__('os')",
            "getattr(raw.x11, 'XWarp' + 'Pointer')",
            "raw.__class__.__init__",
            "raw.xdotool(sub)",
            'raw.xdotool("exec", "ls")',
            'raw.xdotool("search", "--name", "x", "behave", "%@", "focus", "exec", "ls")',
            # private handles of the runtime (verifier round 1)
            'raw.xtst._lib["XTestFake" + "MotionEvent"](raw.dpy, -1, 5, 5, 0)',
            "x = raw._x",
            'getattr(raw.xtst, "_lib")',
            # other GObject modules: GLib/Gio spawn processes, Gdk warps the pointer (verifier round 1)
            "import gi",
            "from gi.repository import GLib",
            "from gi.repository import Gdk",
            'GLib.spawn_command_line_sync("ls")',
            "raw.Atspi.GLib",
            "import ctypes\nctypes.pythonapi",
            "from ctypes import pythonapi",
            'setattr(raw, "x", 1)',
            # aliasing getattr hides later lookups (verifier round 2)
            'g = getattr\ng(ctypes, "cd" + "ll")',
            "f = [getattr][0]",
            "import ctypes\nctypes.CFUNCTYPE",
            "import ctypes\nctypes.memmove",
            "import ctypes\nctypes.util",
            # raw memory through ctypes types (verifier round 3)
            "import ctypes\nctypes.c_char.from_address(1)",
            "import ctypes\nctypes.c_int.from_buffer(b)",
            "import ctypes\nctypes.c_int.in_dll(x, 'y')",
        ):
            with self.subTest(src=src):
                self.assertFalse(rawlint.lint(src).ok, src)

    def test_xdotool_chains_need_the_grant(self):
        # a chain would let a later key command go to the focused window (verifier round 5)
        src = 'raw.xdotool("key", "--window", "123", "a", "key", "b")\nreturn raw.pointer()'
        self.assertBlocked(src, "BG9")
        self.assertIn("BG3", {f.rule for f in rawlint.lint(src).findings})
        # each chained command with its own --window is still a chain, but sends nothing to the focused window
        both = 'raw.xdotool("key", "--window", "1", "a", "key", "--window", "1", "b")\nreturn raw.pointer()'
        self.assertEqual({f.rule for f in rawlint.lint(both).findings}, {"BG6", "BG9"})
        # the pilot's escalated drag: mousemove + mousedown in one call
        drag = 'raw.xdotool("mousemove", "300", "797", "mousedown", "1")\nreturn raw.pointer()'
        self.assertTrue(granted(drag).ok)
        self.assertFalse(rawlint.lint(drag).ok)
        self.assertTrue(rawlint.lint('raw.xdotool("search", "--name", "BenchLab")').ok)
        # a chained click is an action: without a later read it gets the read-back warning
        clicks = granted('raw.xdotool("mousemove", "5", "5", "click", "1")')
        self.assertIn("VR1", {f.rule for f in clicks.findings})
        # an argument equal to a command name is read as a command (documented; errs toward refusing)
        self.assertFalse(granted('raw.xdotool("type", "--window", "5", "exec")').ok)
        self.assertLessEqual(set(rawlint.XDOTOOL), rawlint.XDOTOOL_COMMANDS)

    def test_warnings_run(self):
        src = 'raw.xdotool("type", "--window", "5", "hi")\nreturn raw.describe(x)'
        report = rawlint.lint(src)
        self.assertTrue(report.ok)
        self.assertEqual({f.rule for f in report.findings}, {"BG6"})
        self.assertIn("WARNING", report.render())
        noread = rawlint.lint("b.get_action_iface().do_action(0)")
        self.assertTrue(noread.ok)
        self.assertEqual({f.rule for f in noread.findings}, {"VR1"})
        # a read later on the same line counts (one-line scripts)
        oneline = rawlint.lint("Atspi.Action.do_action(b, 0); raw.sleep(0.3); return raw.dump(a)")
        self.assertEqual(oneline.findings, [])
        before = rawlint.lint("x = raw.dump(a); Atspi.Action.do_action(b, 0)")
        self.assertEqual({f.rule for f in before.findings}, {"VR1"})

    def test_every_rule_cites_a_source_and_an_alternative(self):
        for rule in rawlint.RULES.values():
            self.assertTrue(rule.source and rule.instead and rule.why, rule.id)
            self.assertIn(rule.severity, {"block", "granted", "warn"})
        used = (
            set(rawlint.NAMES.values())
            | set(rawlint.STRINGS.values())
            | set(rawlint.XDOTOOL.values())
        )
        self.assertLessEqual(used, set(rawlint.RULES))


class RunnerPolicyTest(unittest.TestCase):
    """What a script's restricted builtins give it (the parts that need no X display)."""

    def test_ctypes_is_not_importable(self):
        for fromlist in ((), ("c_int",), ("CDLL",), ("c_char_p",)):
            with self.assertRaises(ImportError):
                runner._guarded_import("ctypes", fromlist=fromlist)
        self.assertFalse(rawlint.lint("import ctypes").ok)
        self.assertFalse(rawlint.lint("from ctypes import c_char_p").ok)

    def test_only_listed_modules_import(self):
        for name in ("os", "gi", "gi.repository", "subprocess", "importlib", "operator"):
            with self.assertRaises(ImportError, msg=name):
                runner._guarded_import(name)
        self.assertIs(runner._guarded_import("json"), __import__("json"))

    def test_gi_reimport_of_a_loaded_module_only(self):
        # gi's lazy loader re-imports gi.repository.Atspi through the script's __import__ (seen on the VM smoke)
        sentinel = object()
        with mock.patch.dict(sys.modules, {"gi.repository.Atspi": sentinel}):
            self.assertIs(runner._guarded_import("gi.repository.Atspi"), sentinel)
            with self.assertRaises(ImportError):
                runner._guarded_import("gi.repository.Atspi", fromlist=("Atspi",))
        for name in ("gi.repository.GLib", "gi.repository.Gdk"):
            with self.assertRaises(ImportError):
                runner._guarded_import(name)
        self.assertFalse(rawlint.lint("import gi.repository.Atspi").ok)

    def test_guarded_getattr_refuses_private_and_escape_names(self):
        class Box:
            _lib = "native"
            GLib = "glib"
            from_address = "memory"

        for name in ("_lib", "__class__", "GLib", "from_address"):
            with self.assertRaises(AttributeError, msg=name):
                runner._guarded_getattr(Box(), name)

    def test_runner_takes_the_grant_from_the_harness_environment_only(self):
        src = 'raw.xdotool("click", 1)\nreturn raw.pointer()'
        with mock.patch.dict("os.environ", {"RAW_GRANT": "0"}):
            out = runner.run(src, Path("/nonexistent"))
        self.assertTrue(out["lint_blocked"])
        self.assertFalse(out["granted"])
        with mock.patch.dict("os.environ", {"RAW_GRANT": "yes"}):  # only the exact value "1" grants
            self.assertTrue(runner.run(src, Path("/nonexistent"))["lint_blocked"])
        out = runner.run("raw.x11.XGrabServer(raw.dpy)", Path("/nonexistent"))
        self.assertIn("BG4", out["lint_text"])


class TrialSettingsTest(unittest.TestCase):
    """raw_mcp reads the harness's per-trial file; a missing file is the safe default."""

    def test_missing_trial_file_grants_nothing(self):
        import raw_mcp

        with mock.patch.object(raw_mcp, "TRIAL", Path("/nonexistent/trial.json")):
            settings = raw_mcp.trial()
        self.assertFalse(settings["foreground_grant"])
        self.assertEqual(raw_mcp.runner_env(settings)["RAW_GRANT"], "0")

    def test_grant_reaches_the_runner_and_no_driver_does(self):
        import raw_mcp

        settings = {
            "foreground_grant": True,
            "coverage": {"LibreOffice Calc": "partial"},
            "driver": {"socket": "/tmp/s.sock", "bin": "/x/cua-driver", "session": "raw-t"},  # an old trial file
        }
        with mock.patch.dict("os.environ", {"RAW_DRIVER_SOCKET": "/tmp/s.sock"}):
            env = raw_mcp.runner_env(settings)
        self.assertEqual(env["RAW_GRANT"], "1")
        self.assertFalse([k for k in env if k.startswith("RAW_DRIVER_")])
        self.assertEqual(raw_mcp.runner_env({"foreground_grant": "true"})["RAW_GRANT"], "0")
        text = raw_mcp.description(settings)
        self.assertIn("LibreOffice Calc: partial", text)
        self.assertIn("This task grants foreground", text)
        self.assertIn("does not grant foreground", raw_mcp.description({}))
        self.assertNotIn("raw.driver(", text)
        self.assertIn("raw.cells", text)

    def test_stalled_apps_carry_to_later_scripts_of_the_same_trial_only(self):
        import raw_mcp

        with mock.patch.dict(raw_mcp.STALLED, {"t1": {"42", "7"}}, clear=True):
            self.assertEqual(raw_mcp.runner_env({"trial_id": "t1"})["RAW_ATSPI_SUSPECT"], "42,7")
            self.assertNotIn("RAW_ATSPI_SUSPECT", raw_mcp.runner_env({"trial_id": "t2"}))


class ForegroundHelperTest(unittest.TestCase):
    """The keyboard helpers send nothing unless the target holds the input focus (no display needed)."""

    def test_no_keys_when_focus_is_outside_the_target(self):
        fake = mock.MagicMock()
        with mock.patch.dict(sys.modules, {"gi": fake, "gi.repository": fake}):
            sys.modules.pop("raw", None)
            with mock.patch("ctypes.CDLL"), mock.patch("ctypes.util.find_library", return_value="x"):
                import raw  # noqa: PLC0415

                try:
                    helper = object.__new__(raw.Raw)
                    helper._x = mock.MagicMock()
                    helper._x.clients.return_value = [5]
                    helper._x.popups.return_value = ()
                    helper._restore_to = None
                    helper._popups_before = set()
                    sent = []
                    helper._run_xdotool = lambda *a, **k: sent.append(a)
                    raw.STATE.granted = True
                    for focus_ok, expect in ((False, 0), (True, 1)):
                        sent.clear()
                        helper._activate = lambda wid, ok=focus_ok: {"active": True, "focus_ok": ok}
                        for call in (lambda: helper.key(5, "ctrl+s"), lambda: helper.type_text(5, "abc")):
                            if focus_ok:
                                call()
                            else:
                                with self.assertRaises(RuntimeError):
                                    call()
                        self.assertEqual(len(sent), 2 * expect, focus_ok)
                    # a pointer helper needs the window active, not the key focus
                    sent.clear()
                    helper._activate = lambda wid: {"active": True, "focus_ok": False}
                    helper._x.geometry.return_value = (0, 0, 10, 10)
                    helper.click(5, 1, 1)
                    self.assertEqual(len(sent), 2)
                    raw.STATE.granted = False
                    with self.assertRaises(raw.Blocked):
                        helper.key(5, "a")
                finally:
                    sys.modules.pop("raw", None)


class AtspiTimeoutTest(unittest.TestCase):
    """The runner's AT-SPI timeouts from RAW_ATSPI_TIMEOUT_MS (no display needed)."""

    def load(self, value: str | None) -> tuple[int, int]:
        fake = mock.MagicMock()
        env = {} if value is None else {"RAW_ATSPI_TIMEOUT_MS": value}
        with mock.patch.dict(sys.modules, {"gi": fake, "gi.repository": fake}), mock.patch.dict("os.environ", env):
            if value is None:
                os.environ.pop("RAW_ATSPI_TIMEOUT_MS", None)
            sys.modules.pop("raw", None)
            with mock.patch("ctypes.CDLL"), mock.patch("ctypes.util.find_library", return_value="x"):
                import raw  # noqa: PLC0415

                try:
                    return raw.ATSPI_TIMEOUT_MS, raw.SUSPECT_TIMEOUT_MS
                finally:
                    sys.modules.pop("raw", None)

    def test_suspect_probe_never_exceeds_the_configured_timeout(self):
        self.assertEqual(self.load(None), (2000, 300))
        self.assertEqual(self.load("150"), (150, 150))
        self.assertEqual(self.load("0"), (0, 300))  # 0 keeps D-Bus's default; raw.app then never shortens it


class CellRefTest(unittest.TestCase):
    """raw.cells' A1 references (pure functions; no display needed)."""

    def test_refs_round_trip(self):
        fake = mock.MagicMock()
        with mock.patch.dict(sys.modules, {"gi": fake, "gi.repository": fake}):
            sys.modules.pop("raw", None)
            with mock.patch("ctypes.CDLL"), mock.patch("ctypes.util.find_library", return_value="x"):
                import raw  # noqa: PLC0415

                try:
                    self.assertEqual(raw._cell_ref("A1"), (0, 0))
                    self.assertEqual(raw._cell_ref("b7"), (6, 1))
                    self.assertEqual(raw._cell_ref("$AA$10"), (9, 26))
                    for bad in ("7", "A0", "", "A1B"):
                        with self.assertRaises(ValueError, msg=bad):
                            raw._cell_ref(bad)
                    for col in (0, 25, 26, 701, 702):
                        self.assertEqual(raw._cell_ref(raw._col_name(col) + "1"), (0, col))
                finally:
                    sys.modules.pop("raw", None)


class TaskGrantTest(unittest.TestCase):
    """The per-task grant in the task definitions: only tasks with no background route on the X11 lane get it."""

    def test_grants(self):
        import json

        import bench_v2 as v2

        specs = {p.parent.name: json.loads(p.read_text()) for p in (HERE / "probes").glob("*/task.json")}
        got = {t for t in v2.V2_TASKS if v2.foreground_grant(specs[t])}
        # hover/drag/tooltip (no background pointer route on Xvfb) and the spreadsheet tasks (no background keyboard
        # route on Xvfb, and Calc cells have no EditableText); the BenchLab interruption tasks have AT-SPI routes
        self.assertEqual(got, {"MB-09", "MB-10", "MB-11", "CDB-G02", "CDB-G03"})
        for t in v2.V2_TASKS:
            self.assertEqual(v2.foreground_grant_problems(t, specs[t]), [], t)
        self.assertFalse(v2.foreground_grant({"foreground_grant": {"granted": "yes", "reason": "x"}}))
        self.assertTrue(v2.foreground_grant_problems("X", {"foreground_grant": True}))


if __name__ == "__main__":
    unittest.main()
