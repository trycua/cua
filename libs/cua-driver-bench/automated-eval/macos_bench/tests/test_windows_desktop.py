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
* the crowded desk (PA-02): up to sixteen copies with one shared window title and an inline Confirm step, all
  worked by one UI Automation oracle (windows/oracle.py --mode crowded): it passes at N = 16 and 8, a no-op
  fails, cross-talk, a premature Confirm and a Confirm left pending are caught.
* the busy user (UM-02, schema 2): the same mouse-user contracts with drag legs at 8 ms steps, plus a drag cut
  by a foreign click while the button is held.
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
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
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
            self.assertFalse(any(r.is_selected() for r in radios), "a shelf is selected at start")

    def test_oracle_passes_each_copy(self) -> None:
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
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
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            root = Path(tmp)
            self.start(root)
            result = self.evaluate(root, time.time() * 1000)
            self.assertFalse(result["passed"])
            self.assertEqual(result["diagnostics"]["instances_passed"]["value"], 0)

    def test_foreign_code_in_a_copy_is_caught(self) -> None:
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
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

    TASK = "UM-01"

    def setUp(self) -> None:
        self.procs: list[subprocess.Popen] = []
        self.addCleanup(self.stop)

    def stop(self) -> None:
        for proc in self.procs:
            if proc.poll() is None:
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)

    def run_user(self, seconds: float = 4.0, during=None, cover: bool = False) -> dict:
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            root = Path(tmp)
            script, log, control = root / "mouse-user.json", root / "s.jsonl", root / "control"
            shared_desktop.write_mouse_script(9, script, self.TASK)
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
            summary = summarize_sentinel.summarize(str(log))["mouse_user"]
            self.stop()  # end every process before the temporary directory goes
            return summary

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


@unittest.skipUnless(_available(), "needs a Windows desktop with wxPython and pywinauto")
class CrowdedDeskTest(unittest.TestCase):
    """PA-02: copies of BenchLab desk mode with one shared title and --confirm, worked by one oracle."""

    SEED = 4242

    def setUp(self) -> None:
        self.procs: list[subprocess.Popen] = []
        self.addCleanup(self.stop)

    def stop(self) -> None:
        for proc in self.procs:
            if proc.poll() is None:
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)

    def start(self, root: Path, instances: int) -> list[dict]:
        plan = shared_desktop.pa_agents(self.SEED, instances, root, task="PA-02")
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

    def oracle(self, instances: int, *extra: str) -> None:
        done = subprocess.run(
            [sys.executable, str(ORACLE), "--mode", "crowded", "--seed", str(self.SEED),
             "--instances", str(instances), *extra],
            capture_output=True, text=True, timeout=240,
        )  # fmt: skip
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def evaluate(self, root: Path, instances: int, t0_ms: float) -> dict:
        shared_desktop.write_meta(
            root, instances, t0_ms, C.PA2_BUDGET_S, copy_deadline_s=C.PA2_COPY_DEADLINE_S
        )
        result = root / "result.json"
        subprocess.run(
            shared_desktop.evaluate_argv(sys.executable, self.SEED, root, result, "PA-02"),
            capture_output=True, timeout=60, check=True,
        )  # fmt: skip
        return json.loads(result.read_text("utf-8"))

    def run_oracle(self, instances: int, *extra: str) -> dict:
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            root = Path(tmp)
            self.start(root, instances)
            t0 = time.time() * 1000
            self.oracle(instances, *extra)
            result = self.evaluate(root, instances, t0)
            self.stop()  # end every process before the temporary directory goes
            return result

    @staticmethod
    def failed(result: dict, label: str | None = None) -> list[str]:
        return [
            k
            for k, v in result["checks"].items()
            if not v["passed"] and (label is None or k.startswith(f"{label}."))
        ]

    def code_of(self, label: str) -> str:
        return next(c["code"] for c in C.derive_crowded(self.SEED)["cells"] if c["label"] == label)

    def test_copies_share_a_title_and_hide_confirm_until_submit(self) -> None:
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            plan = self.start(Path(tmp), 2)
            self.assertEqual({e["title"] for e in plan}, {C.PA2_WINDOW_TITLE})
            for entry in plan:
                state = json.loads(entry["state"].read_text("utf-8"))
                self.assertIs(state["confirm_pending"], False)
            from pywinauto import Desktop

            windows = Desktop(backend="uia").windows(title=C.PA2_WINDOW_TITLE)
            self.assertEqual(len(windows), 2)
            for win in windows:
                names = {b.window_text() for b in win.descendants(control_type="Button")}
                self.assertIn("Submit", names)
                self.assertFalse({"Confirm", "Cancel"} & names, names)

    def test_oracle_passes_all_sixteen_copies(self) -> None:
        result = self.run_oracle(16)
        self.assertTrue(result["passed"], self.failed(result))
        self.assertEqual(result["diagnostics"]["cross_talk_instances"]["value"], 0)
        self.assertEqual(result["diagnostics"]["instances_passed"]["value"], 16)

    def test_oracle_passes_eight_copies(self) -> None:
        result = self.run_oracle(8)
        self.assertTrue(result["passed"], self.failed(result))
        self.assertEqual(result["diagnostics"]["instances_passed"]["value"], 8)

    def test_noop_fails(self) -> None:
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            root = Path(tmp)
            self.start(root, 8)
            result = self.evaluate(root, 8, time.time() * 1000)
            self.stop()
            self.assertFalse(result["passed"])
            self.assertEqual(result["diagnostics"]["instances_passed"]["value"], 0)

    def test_cross_talk_is_caught(self) -> None:
        first, second = C.derive_crowded(self.SEED, 8)["cells"][:2]
        result = self.run_oracle(8, "--type", f"{first['label']}={second['code']}")
        self.assertFalse(result["passed"])
        self.assertGreaterEqual(result["diagnostics"]["cross_talk_instances"]["value"], 1)
        self.assertIn(f"{first['label']}.no_foreign_input", self.failed(result, first["label"]))
        self.assertEqual(self.failed(result, second["label"]), [])

    def test_a_premature_confirm_fails_that_copy(self) -> None:
        first, second = C.derive_crowded(self.SEED, 8)["cells"][:2]
        result = self.run_oracle(8, "--premature", first["label"])
        self.assertFalse(result["passed"])
        self.assertNotEqual(self.failed(result, first["label"]), [])
        self.assertEqual(self.failed(result, second["label"]), [])

    def test_a_pending_confirm_fails_that_copy(self) -> None:
        first, second = C.derive_crowded(self.SEED, 8)["cells"][:2]
        result = self.run_oracle(8, "--pending", first["label"])
        self.assertFalse(result["passed"])
        self.assertIn(f"{first['label']}.confirm_resolved", self.failed(result, first["label"]))
        self.assertEqual(self.failed(result, second["label"]), [])


@unittest.skipUnless(_available(), "needs a Windows desktop with wxPython and pywinauto")
class BusyUserTest(MouseUserTest):
    """UM-02: the schema-2 mouse user (8 ms steps, drag legs, button-up handling) on the canvas. The UM-01
    contracts above run again with this script."""

    TASK = "UM-02"

    def test_every_click_and_drag_landed(self) -> None:
        mu = self.run_user(seconds=6.0)
        self.assertTrue(mu["tagging"], mu)
        self.assertGreaterEqual(mu["drags"], 2, mu)
        self.assertEqual(mu["clicks_landed"], mu["clicks"], mu)
        self.assertEqual(mu["drags_landed"], mu["drags"], mu)
        self.assertEqual(mu["drags_cut"], 0, mu)
        self.assertLessEqual(mu["max_displacement_px"], 3, mu)
        self.assertEqual(mu["foreign_pointer_events"], 0, mu)

    def test_a_window_over_the_canvas_loses_clicks_and_drags(self) -> None:
        mu = self.run_user(seconds=6.0, cover=True)
        self.assertGreaterEqual(mu["drags"], 1, mu)
        self.assertGreater(mu["clicks_lost"], 0, mu)
        self.assertEqual(mu["drags_landed"], 0, mu)

    def test_a_foreign_click_while_the_button_is_held_cuts_a_drag(self) -> None:
        def stray_clicks() -> None:
            # Untagged button events at the pointer's place, for two seconds: some fall inside a drag.
            user32, end = ctypes.windll.user32, time.monotonic() + 2.0
            while time.monotonic() < end:
                user32.mouse_event(0x2, 0, 0, 0, 0)  # left down
                user32.mouse_event(0x4, 0, 0, 0, 0)  # left up
                time.sleep(0.037)

        mu = self.run_user(seconds=7.0, during=stray_clicks)
        self.assertGreaterEqual(mu["drags_cut"], 1, mu)
        self.assertFalse(mu["foreign_pointer_events"] == 0, mu)


if __name__ == "__main__":
    unittest.main()
