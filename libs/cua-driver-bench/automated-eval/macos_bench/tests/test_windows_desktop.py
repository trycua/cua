# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Windows desktop contracts (no model): runs on a Windows interactive desktop with wxPython and pywinauto.

Through launch/windows_session.py, the real launcher:
* every BenchLab task is solvable through the GUI: the scripted oracle (windows/oracle.py, real input)
  passes the task's checker, with no GUI-only violation;
* an agent that does nothing fails the checker, and the sentinel measures all eight A22 criteria as held;
* a deliberate disturbance (focus steal, raise, pointer, clipboard) is seen by the sentinel;
* desk mode (PA-01): two copies of BenchLab at two grid cells, each driven by the oracle with its own cell's
  code and shelf, pass the PA-01 checker; a no-op run fails; one copy's code typed into the other is caught;
* the sentinel's mouse user (UM-01, `--mouse-user`): it moves and clicks the real pointer with every click
  landing and no displacement; a pointer yanked away mid-run, or a topmost window over the canvas, is seen.
"""

from __future__ import annotations

import importlib.util
import ctypes
import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "probes" / "_common"))
sys.path.insert(0, str(BENCH / "swift"))
import bench_v2 as v2  # noqa: E402
import benchlab_common as C  # noqa: E402
import shared_desktop  # noqa: E402
import summarize_sentinel  # noqa: E402

SESSION = BENCH / "launch" / "windows_session.py"
ORACLE = BENCH / "windows" / "oracle.py"
BENCHLAB = BENCH / "windows" / "benchlab.py"
SENTINEL = BENCH / "windows" / "sentinel.py"
PYTHONW = Path(sys.executable).with_name("pythonw.exe")
TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")


def _available() -> bool:
    return sys.platform == "win32" and all(
        importlib.util.find_spec(m) is not None for m in ("wx", "pywinauto")
    )


@unittest.skipUnless(_available(), "needs a Windows desktop with wxPython and pywinauto")
class WindowsDesktopTest(unittest.TestCase):
    def trial(self, task: str, condition: str, agent: list[str]) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / task
            done = subprocess.run(
                [sys.executable, str(SESSION), "--task", task, "--run-index", "0",
                 "--condition", condition, "--out", str(out), "--timeout", "150", "--", *agent],
                capture_output=True, text=True, timeout=300,
            )  # fmt: skip
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
            return json.loads((out / "row.json").read_text("utf-8"))

    def test_oracle_solves_every_task(self) -> None:
        for task in TASKS:
            with self.subTest(task=task):
                row = self.trial(
                    task,
                    "foreground",
                    [sys.executable, str(ORACLE), "--mode", "{mode}", "--seed", "{seed}"],
                )
                failed = [k for k, v in row["evaluation"]["checks"].items() if not v["passed"]]
                self.assertTrue(row["checker_passed"], failed)
                self.assertFalse(row["gui_only"]["violation"], row["gui_only"])
                self.assertTrue(row["passed"])

    def test_noop_fails_and_leaves_the_user_undisturbed(self) -> None:
        row = self.trial(
            "IR-02", "background", [sys.executable, "-c", "import time; time.sleep(8)"]
        )
        self.assertFalse(row["checker_passed"])
        self.assertTrue(row["sentinel_front_at_start"])
        self.assertEqual(row["a22"]["unmeasured"], [])
        self.assertTrue(row["a22"]["user_undisturbed"], row["a22"]["criteria"])

    def test_disturbance_is_seen(self) -> None:
        row = self.trial(
            "IR-02",
            "background",
            [sys.executable, str(ORACLE), "--mode", "{mode}", "--seed", "{seed}", "--disturb"],
        )
        criteria = row["a22"]["criteria"]
        for key in ("key_focus", "window_raised", "pointer", "clipboard", "activation"):
            self.assertFalse(criteria[key], key)
        self.assertFalse(row["a22"]["user_undisturbed"])


def _lines(path: Path) -> list[dict]:
    if not path.exists():
        return []
    out = []
    for raw in path.read_text("utf-8").splitlines():
        try:
            out.append(json.loads(raw))
        except json.JSONDecodeError:
            pass
    return out


def _wait(predicate, limit: float) -> bool:
    end = time.monotonic() + limit
    while time.monotonic() < end:
        if predicate():
            return True
        time.sleep(0.1)
    return False


@unittest.skipUnless(_available(), "needs a Windows desktop with wxPython and pywinauto")
class DeskTest(unittest.TestCase):
    """PA-01's BenchLab desk mode, two copies at two grid cells, driven through the GUI by the oracle."""

    SEED = 4242

    def setUp(self) -> None:
        self.procs: list[subprocess.Popen] = []
        self.addCleanup(self.stop)

    def stop(self) -> None:
        for proc in self.procs:
            if proc.poll() is None:
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)

    def start(self, root: Path, instances: int = 2) -> list[dict]:
        plan = shared_desktop.pa_agents(self.SEED, instances, root)
        for entry in plan:
            entry["dir"].mkdir(parents=True)
            self.procs.append(
                subprocess.Popen(
                    [str(PYTHONW), str(BENCHLAB), "--mode", "desk", *entry["app_args"]]
                )
            )
        for entry in plan:
            self.assertTrue(
                _wait(entry["state"].exists, 30), f"desk {entry['label']} did not start"
            )
        return plan

    def drive(self, entry: dict, **override: str) -> None:
        cmd = [sys.executable, str(ORACLE), "--mode", "desk", "--seed", str(self.SEED),
               "--instance", entry["label"]]  # fmt: skip
        for key, value in override.items():
            cmd += [f"--{key}", value]
        done = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def evaluate(self, root: Path, t0_ms: float) -> dict:
        shared_desktop.write_meta(root, 2, t0_ms, 360)
        result = root / "result.json"
        subprocess.run(shared_desktop.evaluate_argv(sys.executable, self.SEED, root, result),
                       capture_output=True, timeout=60, check=True)  # fmt: skip
        return json.loads(result.read_text("utf-8"))

    def test_desk_starts_empty_and_names_its_window(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            plan = self.start(Path(tmp), 1)
            state = json.loads(plan[0]["state"].read_text("utf-8"))
            self.assertEqual(state["mode"], "desk")
            self.assertEqual(state["instance"], plan[0]["label"])
            self.assertEqual((state["code"], state["shelf"]), ("", None))
            self.assertEqual((state["submit_count"], state["submitted"]), (0, None))
            events = _lines(plan[0]["events"])
            self.assertEqual(events[0]["type"], "app_start")
            self.assertEqual(events[0]["details"]["instance"], plan[0]["label"])
            from pywinauto import Desktop

            win = Desktop(backend="uia").window(title=plan[0]["title"])
            self.assertTrue(win.exists(timeout=10))
            radios = win.descendants(control_type="RadioButton")
            self.assertEqual([r.window_text() for r in radios], C.DESK_SHELVES)
            self.assertFalse(
                any(r.get_toggle_state() for r in radios), "a shelf is selected at start"
            )

    def test_oracle_passes_each_copy(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            plan = self.start(root)
            t0 = time.time() * 1000
            for entry in plan:
                self.drive(entry)
            result = self.evaluate(root, t0)
            failed = [k for k, v in result["checks"].items() if not v["passed"]]
            self.assertTrue(result["passed"], failed)
            self.assertEqual(result["diagnostics"]["cross_talk_instances"]["value"], 0)

    def test_noop_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.start(root)
            result = self.evaluate(root, time.time() * 1000)
            self.assertFalse(result["passed"])
            self.assertEqual(result["diagnostics"]["instances_passed"]["value"], 0)

    def test_foreign_code_in_a_copy_is_caught(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            plan = self.start(root)
            t0 = time.time() * 1000
            first, second = plan
            self.drive(first, code=self.code_of(second))
            self.drive(second)
            result = self.evaluate(root, t0)
            self.assertFalse(result["passed"])
            self.assertFalse(result["checks"][f"{first['label']}.no_foreign_input"]["passed"])
            self.assertTrue(result["checks"][f"{second['label']}.no_foreign_input"]["passed"])
            self.assertEqual(result["diagnostics"]["cross_talk_instances"]["value"], 1)

    def code_of(self, entry: dict) -> str:
        return C.derive_parallel(self.SEED, 2)["cells"][entry["cell"]]["code"]


@unittest.skipUnless(_available(), "needs a Windows desktop with wxPython and pywinauto")
class MouseUserTest(unittest.TestCase):
    """The sentinel's simulated mouse user (SendInput, tagged; a low-level hook for everything else)."""

    COVER = (
        "import sys, wx\n"
        "app = wx.App(False)\n"
        "f = wx.Frame(None, title='Cover', pos=(420, 340), size=(900, 520),\n"
        "             style=wx.STAY_ON_TOP | wx.NO_BORDER | wx.FRAME_NO_TASKBAR)\n"
        "f.Show()\n"
        "wx.CallLater(40000, app.ExitMainLoop)\n"
        "app.MainLoop()\n"
    )

    def setUp(self) -> None:
        self.procs: list[subprocess.Popen] = []
        self.addCleanup(self.stop)

    def stop(self) -> None:
        for proc in self.procs:
            if proc.poll() is None:
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)

    def run_user(self, seconds: float = 4.0, during=None, cover: bool = False) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            script, log, control = root / "mouse-user.json", root / "s.jsonl", root / "control"
            shared_desktop.write_mouse_script(9, script)
            control.write_text("", "utf-8")
            sentinel = subprocess.Popen(
                [str(PYTHONW), str(SENTINEL), "--mouse-user", str(script), "--log", str(log),
                 "--control", str(control), "--frame", ",".join(map(str, v2.USER_WINDOW_FRAME))]
            )  # fmt: skip
            self.procs.append(sentinel)
            self.assertTrue(_wait(lambda: any(x.get("ev") == "ready" for x in _lines(log)), 30))
            ready = next(x for x in _lines(log) if x["ev"] == "ready")
            self.assertEqual(ready["canvas"], list(C.MU_CANVAS))
            self.say(control, "activate")
            time.sleep(1.0)
            if cover:
                self.procs.append(subprocess.Popen([str(PYTHONW), "-c", self.COVER]))
                time.sleep(2.0)
            self.say(control, "arm")
            self.assertTrue(_wait(lambda: any(x.get("ev") == "armed" for x in _lines(log)), 5))
            start = time.monotonic()
            if during:
                time.sleep(seconds / 2)
                during()
            time.sleep(max(0.0, seconds - (time.monotonic() - start)))
            self.say(control, "disarm")
            self.assertTrue(_wait(lambda: any(x.get("ev") == "disarmed" for x in _lines(log)), 10))
            self.say(control, "quit")
            return summarize_sentinel.summarize(str(log))["mouse_user"]

    @staticmethod
    def say(control: Path, command: str) -> None:
        with control.open("a", encoding="utf-8") as c:
            c.write(command + "\n")

    def test_the_user_clicks_cleanly(self) -> None:
        mu = self.run_user()
        self.assertTrue(mu["tagging"], mu)
        self.assertGreaterEqual(mu["clicks"], 2, mu)
        self.assertEqual(mu["clicks_landed"], mu["clicks"], mu)
        self.assertLessEqual(mu["max_displacement_px"], 3, mu)
        self.assertEqual(mu["foreign_pointer_events"], 0, mu)

    def test_a_pointer_yanked_away_is_seen(self) -> None:
        mu = self.run_user(during=lambda: ctypes.windll.user32.SetCursorPos(5, 5))
        self.assertTrue(mu["max_displacement_px"] > 3 or mu["foreign_pointer_events"], mu)

    def test_a_window_over_the_canvas_loses_the_clicks(self) -> None:
        mu = self.run_user(cover=True)
        self.assertGreaterEqual(mu["clicks"], 2, mu)
        self.assertGreater(mu["clicks_lost"], 0, mu)
        self.assertLess(mu["clicks_landed"], mu["clicks"], mu)


if __name__ == "__main__":
    unittest.main()
