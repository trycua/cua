# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The raw-linux prototype's background-use lint: what it blocks, warns about and lets through."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE / "tools" / "raw_linux"))
import rawlint  # noqa: E402
import runner  # noqa: E402

ESC = 'raw.escalate_foreground("the toolbar only appears on a real hover")\n'


def rules(source: str) -> list[tuple[str, str]]:
    return sorted((f.rule, f.severity) for f in rawlint.lint(source).findings)


class RawLintTest(unittest.TestCase):
    def assertBlocked(self, source: str, rule: str) -> None:
        report = rawlint.lint(source)
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

    def test_pointer_focus_and_key_theft_block_without_escalation(self):
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

    def test_escalation_admits_pointer_focus_and_keys_but_never_grabs(self):
        report = rawlint.lint(ESC + 'raw.xdotool("mousemove", 5, 5)\nreturn raw.pointer()')
        self.assertTrue(report.ok)
        self.assertEqual(report.escalation_reason, "the toolbar only appears on a real hover")
        self.assertIn("FOREGROUND (declared)", report.render())
        self.assertBlocked(ESC + "raw.x11.XGrabKeyboard(raw.dpy, raw.root, 1, 1, 1, 0)", "BG4")
        self.assertBlocked(ESC + "raw.x11.XGrabServer(raw.dpy)", "BG4")

    def test_escalation_needs_a_literal_reason(self):
        self.assertBlocked("r = 'x'\nraw.escalate_foreground(r)\nraw.xdotool('click', 1)", "GUI2")
        self.assertBlocked("raw.escalate_foreground('')\nraw.xdotool('click', 1)", "GUI2")

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

    def test_xdotool_runs_one_command_per_call(self):
        # a chain would let a later key command go to the focused window (verifier round 5)
        src = 'raw.xdotool("key", "--window", "123", "a", "key", "b")\nreturn raw.pointer()'
        self.assertBlocked(src, "GUI2")
        self.assertTrue(rawlint.lint('raw.xdotool("search", "--name", "BenchLab")').ok)
        self.assertTrue(
            rawlint.lint(
                'raw.xdotool("type", "--window", "5", "key words")\nreturn raw.pointer()'
            ).ok
        )
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
            self.assertIn(rule.severity, {"block", "escalatable", "warn"})
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

    def test_guarded_getattr_refuses_private_and_escape_names(self):
        class Box:
            _lib = "native"
            GLib = "glib"
            from_address = "memory"

        for name in ("_lib", "__class__", "GLib", "from_address"):
            with self.assertRaises(AttributeError, msg=name):
                runner._guarded_getattr(Box(), name)

    def test_lint_refused_script_reports_no_escalation(self):
        src = 'if False:\n    raw.escalate_foreground("never runs")\nraw.x11.XGrabServer(raw.dpy)'
        out = runner.run(src, Path("/nonexistent"))
        self.assertTrue(out["lint_blocked"])
        self.assertIsNone(out["escalated"])
        self.assertIn("BG4", out["lint_text"])


if __name__ == "__main__":
    unittest.main()
