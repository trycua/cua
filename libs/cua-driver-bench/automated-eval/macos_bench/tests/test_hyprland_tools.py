# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Hyprland (Wayland) port contracts that hold on every OS (no GUI, no model): the GUI-only guard, the brief,
the launch descriptors, the window rules, window geometry, and that the Hyprland sentinel writes every event
the X11 sentinel writes."""

from __future__ import annotations

import importlib.util
import json
import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "launch"))
import bench_v2 as v2  # noqa: E402
import hyprland_session as HS  # noqa: E402

G = HS.hyprland_module("guard")
H = HS.hyprland_module("hypr")
BENCHLAB_TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")
SHARED_TASKS = ("PA-01", "UM-01")  # selected by id, not part of the registered core tasks


def _emitted(path: Path) -> set[str]:
    return set(re.findall(r'self\.emit\(\s*"([a-z_A-Z]+)"', path.read_text("utf-8")))


class PortTest(unittest.TestCase):
    def test_sentinel_events_cover_linux(self) -> None:
        linux = _emitted(BENCH / "linux/sentinel.py") - {
            "ax_focus"
        }  # AT-SPI focus trace: diagnostic only
        self.assertLessEqual(linux, _emitted(BENCH / "hyprland/sentinel.py"))

    def test_every_benchlab_task_has_a_hyprland_descriptor(self) -> None:
        manifest = {t["id"]: t for t in v2.load_taskset()["tasks"]}
        for tid in BENCHLAB_TASKS:
            spec = json.loads((BENCH / "probes" / tid / "task.json").read_text("utf-8"))
            launch = json.loads(
                (BENCH / "probes" / tid / "launch.hyprland.json").read_text("utf-8")
            )
            self.assertEqual(launch["platform"], "linux-hyprland")
            self.assertEqual(launch["app_args"], spec["app_args"], tid)
            self.assertEqual(launch["conditions"], manifest[tid]["conditions"], tid)
            self.assertIn("linux", manifest[tid]["os"])
            self.assertIn("launch/hyprland_session.py", launch["launcher"])
        self.assertEqual(set(HS.TASKS), set(BENCHLAB_TASKS) | set(SHARED_TASKS))

    def test_window_rules(self) -> None:
        sent = []

        class Fake:
            @staticmethod
            def request(command: str) -> str:
                sent.append(command)
                return "ok"

        HS.apply_rules(Fake)
        self.assertEqual(len(sent), 3)
        self.assertTrue(
            all(c.startswith("eval hl.window_rule({") and c.endswith("})") for c in sent)
        )
        dialogs = next(c for c in sent if "cdb-benchlab-dialogs" in c)
        x, y = map(int, re.search(r'move = "(\d+) (\d+)"', dialogs).groups())
        ux, uy, uw, uh = v2.USER_WINDOW_FRAME
        lx, ly, lw, lh = HS.LAB_FRAME
        # Dialogs open inside BenchLab's frame, above the top edge of the user window.
        self.assertTrue(lx <= x < lx + lw and ly <= y < uy, (x, y))


class OverlayTest(unittest.TestCase):
    def test_agent_cursor_layer_is_the_tools_own(self) -> None:
        sys.path.insert(0, str(BENCH / "swift"))
        import summarize_sentinel

        for name in ("cua-driver:layer:cua-agent-cursor", "pid123:layer:cua-agent-cursor"):
            self.assertTrue(summarize_sentinel.OVERLAY_OWNER_RE.search(name), name)
        self.assertFalse(summarize_sentinel.OVERLAY_OWNER_RE.search("quickshell:layer:omarchy-bar"))


class BriefTest(unittest.TestCase):
    def test_platform_words_and_background_paragraph(self) -> None:
        for tid in BENCHLAB_TASKS:
            fg = HS.render_brief(tid, 123456, "foreground")
            bg = HS.render_brief(tid, 123456, "background")
            self.assertNotIn("your Mac", fg, tid)
            self.assertIn("on this Linux computer", fg, tid)
            self.assertNotIn(v2.BACKGROUND_BRIEF, fg)
            self.assertTrue(bg.rstrip().endswith(v2.BACKGROUND_BRIEF), tid)
            self.assertNotIn("{{", bg, tid)


class GeometryTest(unittest.TestCase):
    def test_overlaps(self) -> None:
        user = {"at": [420, 340], "size": [900, 520]}
        self.assertTrue(H.overlaps({"at": [40, 40], "size": [760, 540]}, user))
        self.assertFalse(H.overlaps({"at": [80, 120], "size": [296, 105]}, user))
        self.assertFalse(H.overlaps({"at": [1521, 0], "size": [399, 1080]}, user))

    def test_identity(self) -> None:
        self.assertEqual(H.identity({"class": "BenchLab", "title": "x"}), "BenchLab")
        self.assertEqual(H.identity({"class": "", "initialClass": "", "title": "t"}), "t")


class GuardTest(unittest.TestCase):
    ROWS = [
        (1, 0, "systemd"),
        (100, 1, "Hyprland"),
        (200, 1, "python3"),  # the runner
        (210, 200, "python3"),
        (220, 210, "kitty"),  # under the runner: not a violation
        (230, 100, "alacritty"),  # started from the desktop: a violation
        (240, 100, "bash"),  # a shell alone is not counted
        (250, 999, "foot"),  # parent gone, but seen under the runner before: the runner's
        (260, 100, "ghostty"),
    ]

    def test_new_side_doors(self) -> None:
        hits = G.new_side_doors(self.ROWS, baseline={1, 100}, runner=200, own={250})
        self.assertEqual(sorted(hits), [(230, "Alacritty"), (260, "Ghostty")])
        self.assertEqual(
            G.new_side_doors(self.ROWS, baseline={p for p, _, _ in self.ROWS}, runner=200), []
        )

    def test_tool_input_patterns(self) -> None:
        hard, _ = G.scan_tool_use("mcp__x__launch_app", {"name": "kitty"})
        self.assertEqual(hard, ["mcp__x__launch_app:kitty"])
        hard, _ = G.scan_tool_use("mcp__x__type_text", {"text": "hyprctl dispatch exec foot"})
        self.assertIn("mcp__x__type_text:hyprctl", hard)
        self.assertIn("mcp__x__type_text:foot", hard)
        self.assertEqual(
            G.scan_tool_use("mcp__x__type_text", {"text": "Football club footer"}), ([], [])
        )
        self.assertEqual(G.scan_tool_use("Bash", {"command": "ls"})[0], ["builtin:Bash"])

    def test_front_grace(self) -> None:
        watch = G.FrontWatch()
        self.assertIsNone(watch.sample("com.mitchellh.ghostty"))
        self.assertIsNone(watch.sample("com.mitchellh.ghostty"))
        self.assertEqual(watch.sample("com.mitchellh.ghostty"), "Ghostty")
        self.assertIsNone(watch.sample("BenchLab"))
        for _ in range(2):
            self.assertIsNone(watch.sample("org.omarchy.btop"))
        self.assertEqual(watch.sample("org.omarchy.btop"), "Omarchy TUI")


class SharedDesktopTest(unittest.TestCase):
    def descriptor(self, tid: str) -> dict:
        return json.loads((BENCH / "probes" / tid / "launch.hyprland.json").read_text("utf-8"))

    def test_descriptors_follow_the_task_files(self) -> None:
        for tid in SHARED_TASKS:
            spec = json.loads((BENCH / "probes" / tid / "task.json").read_text("utf-8"))
            launch = self.descriptor(tid)
            self.assertEqual(launch["platform"], "linux-hyprland")
            self.assertEqual(launch["task_id"], tid)
            self.assertEqual(launch["app_args"], spec["app_args"])
            self.assertEqual(launch["conditions"], spec["conditions"])
            self.assertEqual(launch["launcher"][launch["launcher"].index("--task") + 1], tid)
        pa = self.descriptor("PA-01")
        self.assertEqual(pa["launcher"][pa["launcher"].index("--instances") + 1], "${instances}")
        self.assertIn("{instance}", pa["agent_command_placeholders"])
        um = self.descriptor("UM-01")
        self.assertEqual(um["conditions"], ["background"])
        self.assertIn("--sentinel", um["evaluate"])
        self.assertEqual(um["sentinel_args"][0], "--mouse-user")

    def test_briefs(self) -> None:
        um = HS.render_brief("UM-01", 123456, "background", mouse_user=True)
        self.assertTrue(um.rstrip().endswith(v2.BACKGROUND_BRIEF_MOUSE))
        self.assertIn("on this Linux computer", um)
        self.assertNotIn("your Mac", um)

    def test_agent_command_placeholders(self) -> None:
        subs = {"{brief}": "/o/desk-C/brief.md", "{out}": "/o/desk-C", "{instance}": "C",
                "{title}": "BenchLab Desk C", "{seed}": "7", "{mode}": "desk"}  # fmt: skip
        argv = HS.substitute(
            ["agent", "--brief", "{brief}", "--cwd={out}", "{title}:{instance}"], subs
        )
        self.assertEqual(
            argv, ["agent", "--brief", "/o/desk-C/brief.md", "--cwd=/o/desk-C", "BenchLab Desk C:C"]
        )

    def test_screen_check(self) -> None:
        class Fake:
            def __init__(self, monitors):
                self.monitors = monitors

            def query(self, what):
                return self.monitors

        big, small = {"width": 1920, "height": 1080}, {"width": 1280, "height": 800}
        self.assertFalse(HS.screen_too_small(Fake([big]), *HS.PA_MIN_SCREEN))
        self.assertTrue(HS.screen_too_small(Fake([small]), *HS.PA_MIN_SCREEN))
        self.assertTrue(HS.screen_too_small(Fake([]), 1, 1))
        # the foreground witness window stays clear of the grid and on the screen
        wx, wy, ww, wh = HS.PA_WITNESS_FRAME
        self.assertTrue(wy >= 60 + 440 + 400 and wx + ww <= 1920 and wy + wh <= 1080)

    def test_trial_args(self) -> None:
        self.assertEqual(HS.shared_desktop.instances_for({"instances": 10}, 4), 4)
        with self.assertRaises(ValueError):
            HS.shared_desktop.instances_for({"instances": 10}, 11)


class MouseUserToolsTest(unittest.TestCase):
    def test_click_tool_choice(self) -> None:
        both = {"wlrctl", "ydotool"}
        self.assertEqual(
            H.click_command(lambda n: n if n in both else None),
            ["wlrctl", "pointer", "click", "left"],
        )
        self.assertEqual(
            H.click_command(lambda n: n if n == "ydotool" else None), ["ydotool", "click", "0xC0"]
        )
        self.assertIsNone(H.click_command(lambda n: None))

    def test_cursor_commands(self) -> None:
        classic, lua = H.cursor_commands(100.4, 200.6)
        self.assertEqual(classic, "movecursor 100 201")
        self.assertEqual(lua, "hl.dsp.cursor.move({ x = 100, y = 201 })")

    def test_move_cursor_keeps_the_form_that_moves_the_pointer(self) -> None:
        sent, pointer = [], [0.0, 0.0]

        def dispatch(command: str) -> str:
            sent.append(command)
            if command.startswith("hl."):  # only the Lua form really moves the pointer here
                pointer[:] = [50.0, 60.0]
            return "ok"

        saved = H.dispatch, H.cursor, H._cursor_form
        H.dispatch, H.cursor, H._cursor_form = dispatch, lambda: tuple(pointer), None
        try:
            self.assertTrue(H.move_cursor(50, 60))
            self.assertEqual(len(sent), 2)  # the classic form was tried first and did nothing
            self.assertTrue(H.move_cursor(50, 60))
            self.assertEqual(len(sent), 3)  # then only the working form
            self.assertTrue(sent[-1].startswith("hl."))
            pointer[:] = [0.0, 0.0]
            H._cursor_form = None
            H.dispatch = lambda command: "error"
            self.assertFalse(H.move_cursor(50, 60))
        finally:
            H.dispatch, H.cursor, H._cursor_form = saved


if __name__ == "__main__":
    unittest.main()
