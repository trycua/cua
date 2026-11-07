# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Windows port contracts that hold on every OS (no GUI, no model): the GUI-only guard, the brief, the
launch descriptors, parity of the BenchLab modes and events with the Linux port, and that a log written in
the Windows sentinel's shape measures all eight A22 criteria."""

from __future__ import annotations

import ast
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "windows"))
sys.path.insert(0, str(BENCH / "launch"))
sys.path.insert(0, str(BENCH / "swift"))
import bench_v2 as v2  # noqa: E402
import guard as G  # noqa: E402
import summarize_sentinel  # noqa: E402
import windows_session as WS  # noqa: E402

BENCHLAB_TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")
CDB_TASKS = ("CDB-G02", "CDB-G03", "CDB-G04")


def _emitted(path: Path) -> set[str]:
    return set(re.findall(r'self\.emit\(\s*"([a-z_A-Z]+)"', path.read_text("utf-8")))


def _methods(path: Path, cls: str) -> set[str]:
    tree = ast.parse(path.read_text("utf-8"))
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name == cls:
            return {n.name for n in node.body if isinstance(n, ast.FunctionDef)}
    return set()


def _modes(path: Path) -> tuple:
    tree = ast.parse(path.read_text("utf-8"))
    for node in tree.body:
        if isinstance(node, ast.Assign) and getattr(node.targets[0], "id", "") == "MODES":
            return ast.literal_eval(node.value)
    return ()


class ParityTest(unittest.TestCase):
    """The Windows BenchLab and sentinel write what the Linux ones write, so one set of checkers serves both."""

    def test_same_modes(self) -> None:
        self.assertEqual(_modes(BENCH / "windows/benchlab.py"), _modes(BENCH / "linux/benchlab.py"))
        for mode in _modes(BENCH / "windows/benchlab.py"):
            self.assertIn(mode, _methods(BENCH / "windows/benchlab.py", "Lab"))

    def test_same_benchlab_events(self) -> None:
        self.assertEqual(
            _emitted(BENCH / "windows/benchlab.py"), _emitted(BENCH / "linux/benchlab.py")
        )

    def test_sentinel_events_cover_linux(self) -> None:
        linux = _emitted(BENCH / "linux/sentinel.py") - {
            "ax_focus"
        }  # AT-SPI focus trace: diagnostic only
        self.assertLessEqual(linux, _emitted(BENCH / "windows/sentinel.py"))

    def test_every_benchlab_task_has_a_windows_descriptor(self) -> None:
        manifest = {t["id"]: t for t in v2.load_taskset()["tasks"]}
        for tid in BENCHLAB_TASKS:
            spec = json.loads((BENCH / "probes" / tid / "task.json").read_text("utf-8"))
            launch = json.loads((BENCH / "probes" / tid / "launch.windows.json").read_text("utf-8"))
            self.assertEqual(launch["platform"], "windows")
            self.assertEqual(launch["app_args"], spec["app_args"], tid)
            self.assertEqual(launch["conditions"], manifest[tid]["conditions"], tid)
            self.assertIn("windows", manifest[tid]["os"])
        self.assertEqual(set(WS.TASKS), set(BENCHLAB_TASKS) | set(CDB_TASKS))


class BriefTest(unittest.TestCase):
    def test_platform_words_and_background_paragraph(self) -> None:
        for tid in BENCHLAB_TASKS:
            seed = 123456
            fg = WS.render_brief(tid, seed, "foreground")
            bg = WS.render_brief(tid, seed, "background")
            self.assertNotIn("your Mac", fg, tid)
            self.assertIn("on this Windows PC", fg, tid)
            self.assertNotIn(v2.BACKGROUND_BRIEF, fg)
            self.assertTrue(bg.rstrip().endswith(v2.BACKGROUND_BRIEF), tid)
            self.assertNotIn("{{", bg, tid)


class GuardTest(unittest.TestCase):
    ROWS = [
        (4, 0, "System"),
        (100, 4, "explorer.exe"),
        (200, 100, "runner.exe"),
        (210, 200, "claude.exe"),
        (220, 210, "cmd.exe"),  # an npm shim under the runner: not a violation
        (230, 100, "powershell.exe"),  # started from the desktop: a violation
        (240, 100, "WindowsTerminal.exe"),
        (250, 999, "pwsh.exe"),  # parent gone, but seen under the runner before: the runner's
        (260, 100, "notepad.exe"),
    ]

    def test_names(self) -> None:
        self.assertEqual(
            G.name_of(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"), "powershell"
        )
        self.assertEqual(G.name_of("CMD.EXE"), "cmd")
        self.assertEqual(G.name_of("wt"), "wt")

    def test_descendants(self) -> None:
        self.assertEqual(G.descendants(self.ROWS, 200), {200, 210, 220})

    def test_shell_started_by_the_tool_binary_is_a_helper(self) -> None:
        rows = [
            (10, 1, "cua-driver.exe"),
            (11, 10, "powershell.exe"),
            (20, 1, "python.exe"),
            (21, 20, "powershell.exe"),
        ]
        self.assertEqual(G.started_by_tool(rows, 11, ["cua-driver.exe"]), "cua-driver")
        self.assertIsNone(
            G.started_by_tool(rows, 21, ["cua-driver.exe"])
        )  # agent code in an interpreter
        self.assertIsNone(G.started_by_tool(rows, 11, []))

    def test_new_side_doors(self) -> None:
        hits = G.new_side_doors(self.ROWS, baseline={4, 100}, runner=200, own={250})
        self.assertEqual(sorted(hits), [(230, "Windows PowerShell"), (240, "Windows Terminal")])
        self.assertEqual(
            G.new_side_doors(self.ROWS, baseline={p for p, _, _ in self.ROWS}, runner=200), []
        )

    def test_tool_input_patterns(self) -> None:
        hard, _ = G.scan_tool_use(
            "mcp__x__exec", {"code": "subprocess.run(['powershell', '-c', 'dir'])"}
        )
        self.assertIn("mcp__x__exec:subprocess", hard)
        self.assertIn("mcp__x__exec:powershell", hard)
        hard, _ = G.scan_tool_use("mcp__x__type_text", {"text": "Start-Process notepad"})
        self.assertEqual(hard, ["mcp__x__type_text:Start-Process"])
        self.assertEqual(G.scan_tool_use("mcp__x__click", {"element": "Apply"}), ([], []))
        self.assertEqual(G.scan_tool_use("Bash", {"command": "ls"})[0], ["builtin:Bash"])

    def test_front_grace(self) -> None:
        watch = G.FrontWatch()
        self.assertIsNone(watch.sample(r"C:\x\cmd.exe"))
        self.assertIsNone(watch.sample(r"C:\x\cmd.exe"))
        self.assertEqual(watch.sample(r"C:\x\cmd.exe"), "Command Prompt")
        self.assertIsNone(watch.sample(r"C:\x\BenchLab.exe"))
        self.assertIsNone(watch.sample(r"C:\x\cmd.exe"))


def _sentinel_log(path: Path, disturb: bool) -> None:
    """A log in the exact shape windows/sentinel.py writes."""
    lines = [{"t": 0, "ev": "ready", "hwnd": 42}]
    me = {"bid": "ai.cua.benchsentinel", "pid": 7}
    lab = {"bid": "BenchLab", "pid": 9}

    def sample(t, front=me, mouse=(500, 500), clip=10, above=(), sheet=False, ev="s"):
        return {"t": t, "ev": ev, "front": front, "mouse": list(mouse), "clip": clip, "sheet": sheet,
                "above": list(above), "above_n": len(above), "onspace": True}  # fmt: skip

    lines.append(sample(1, ev="armed"))
    for i in range(2, 40):
        if disturb and i == 20:
            lines.append(sample(i, front=lab, ev="front"))
            lines.append({"t": i, "ev": "didResignActive"})
            lines.append({"t": i, "ev": "didResignKey"})
            lines.append(sample(i, front=lab, mouse=(700, 650), clip=11, above=("BenchLab",)))
            lines.append({"t": i, "ev": "user_blocked"})
            continue
        lines.append(sample(i, clip=11 if disturb and i > 20 else 10))
        if i % 3 == 0:
            lines.append({"t": i, "ev": "user_type"})
    lines.append({"t": 40, "ev": "user_text", "intact": True})
    lines.append(sample(41, clip=11 if disturb else 10, ev="disarmed"))
    path.write_text("\n".join(json.dumps(x) for x in lines) + "\n", "utf-8")


class SentinelLogTest(unittest.TestCase):
    def test_clean_log_measures_all_eight_criteria(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "s.jsonl"
            _sentinel_log(log, disturb=False)
            d = summarize_sentinel.summarize(str(log))
        score = v2.multiplayer_score(d, True, True, "background")
        self.assertEqual(score["unmeasured"], [])
        self.assertTrue(all(score["criteria"].values()), score["criteria"])
        self.assertTrue(score["background_completion"])

    def test_disturbance_is_seen(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "s.jsonl"
            _sentinel_log(log, disturb=True)
            d = summarize_sentinel.summarize(str(log))
        criteria = v2.multiplayer_score(d, True, True, "background")["criteria"]
        for key in ("key_focus", "window_raised", "typing", "pointer", "clipboard", "activation"):
            self.assertFalse(criteria[key], key)
        self.assertTrue(criteria["space"])
        self.assertTrue(criteria["dialog"])


if __name__ == "__main__":
    unittest.main()
