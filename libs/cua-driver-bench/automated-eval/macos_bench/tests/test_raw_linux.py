# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The raw-linux prototype's background-use lint: what it blocks, warns about and lets through."""

from __future__ import annotations

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

    def test_driver_fallback_background_calls_pass_and_foreground_needs_the_grant(self):
        ok = 'r = raw.driver("click", {"pid": 1, "element_token": "s1:2"})\nreturn raw.driver("get_window_state", {"pid": 1})'
        self.assertEqual(rules(ok), [])
        for src in (
            'raw.driver("click", {"pid": 1, "x": 5, "y": 5, "delivery_mode": "foreground"})',
            'raw.driver("move_cursor", {"x": 5, "y": 5, "scope": "desktop"})',
            'raw.driver("bring_to_front", {"pid": 1})',
        ):
            with self.subTest(src=src):
                self.assertBlocked(src + "\nreturn raw.pointer()", "BG10")
                self.assertTrue(granted(src + "\nreturn raw.pointer()").ok)
        for tool in ("launch_app", "kill_app", "run_script", "clipboard_write", "set_config", "browser_navigate"):
            with self.subTest(tool=tool):
                self.assertBlocked(f'raw.driver("{tool}", {{}})\nreturn 1', "GUI1", grant=True)
        # a driver action needs a read-back like a raw one
        self.assertIn("VR1", {f.rule for f in rawlint.lint('raw.driver("click", {"pid": 1, "element_token": "s1:2"})').findings})

    def test_driver_foreground_classifier(self):
        self.assertIsNone(rawlint.driver_foreground("click", {"pid": 1}))
        self.assertIsNone(rawlint.driver_foreground("type_text", {"delivery_mode": "background"}))
        self.assertTrue(rawlint.driver_foreground("hotkey", {"keys": ["a"], "scope": "desktop"}))
        self.assertTrue(rawlint.driver_foreground("drag", {"delivery_mode": "foreground"}))
        self.assertTrue(rawlint.driver_foreground("bring_to_front", {}))
        self.assertLessEqual(rawlint.DRIVER_FOREGROUND_TOOLS, rawlint.DRIVER_ACTIONS)

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
        self.assertNotIn("RAW_DRIVER_SOCKET", raw_mcp.runner_env(settings))

    def test_grant_and_driver_reach_the_runner(self):
        import raw_mcp

        settings = {
            "foreground_grant": True,
            "coverage": {"LibreOffice Calc": "partial"},
            "driver": {"socket": "/tmp/s.sock", "bin": "/x/cua-driver", "session": "raw-t"},
        }
        env = raw_mcp.runner_env(settings)
        self.assertEqual(
            (env["RAW_GRANT"], env["RAW_DRIVER_SOCKET"], env["RAW_DRIVER_BIN"], env["RAW_DRIVER_SESSION"]),
            ("1", "/tmp/s.sock", "/x/cua-driver", "raw-t"),
        )
        self.assertEqual(raw_mcp.runner_env({"foreground_grant": "true"})["RAW_GRANT"], "0")
        text = raw_mcp.description(settings)
        self.assertIn("LibreOffice Calc: partial", text)
        self.assertIn("This task grants foreground", text)
        self.assertIn("does not grant foreground", raw_mcp.description({}))


class TaskGrantTest(unittest.TestCase):
    """The per-task grant in the task definitions: only the real hover/drag tasks get it."""

    def test_grants(self):
        import json

        import bench_v2 as v2

        specs = {p.parent.name: json.loads(p.read_text()) for p in (HERE / "probes").glob("*/task.json")}
        got = {t for t in v2.V2_TASKS if v2.foreground_grant(specs[t])}
        self.assertEqual(got, {"MB-09", "MB-10", "MB-11"})
        for t in v2.V2_TASKS:
            self.assertEqual(v2.foreground_grant_problems(t, specs[t]), [], t)
        self.assertFalse(v2.foreground_grant({"foreground_grant": {"granted": "yes", "reason": "x"}}))
        self.assertTrue(v2.foreground_grant_problems("X", {"foreground_grant": True}))


if __name__ == "__main__":
    unittest.main()
