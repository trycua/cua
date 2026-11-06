# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Hermetic tests for the Linux launcher support: prompts, disk floor, pid ledger, evidence, pins."""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import json
import math
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))

import claude_arms as ca  # noqa: E402
import desktop_runtime as desktop  # noqa: E402
import run_bench as rb  # noqa: E402
from test_runner_flow import make_ctx  # noqa: E402


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, BENCH / "tools" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


evidence = _load("check_run_evidence")
pins_tool = _load("make_linux_pins")

MACOS_PROMPT_SHA256 = "1f68c4cc0780099eb70e65d0e404c7e77bcdcc9909f622a7d1483f66e82bf06d"


class PlatformConstantsTest(unittest.TestCase):
    def test_prompts(self) -> None:
        self.assertEqual(
            hashlib.sha256(ca.MACOS_SYSTEM_PROMPT.encode()).hexdigest(), MACOS_PROMPT_SHA256
        )
        self.assertEqual(
            ca.LINUX_SYSTEM_PROMPT,
            ca.MACOS_SYSTEM_PROMPT.replace("a macOS desktop", "a Linux desktop"),
        )
        self.assertIn("a Linux desktop", ca.LINUX_SYSTEM_PROMPT)
        want = (
            ca.LINUX_SYSTEM_PROMPT if sys.platform.startswith("linux") else ca.MACOS_SYSTEM_PROMPT
        )
        self.assertEqual(ca.SYSTEM_PROMPT, want)

    def test_executable_and_path(self) -> None:
        if sys.platform == "darwin":
            self.assertEqual(ca.CLAUDE_BIN, Path("/opt/homebrew/bin/claude"))
            self.assertEqual(ca.CLAUDE_PATH, "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        else:
            self.assertTrue(ca.CLAUDE_PATH.startswith(f"{ca.CLAUDE_BIN.parent}:"))
            self.assertNotIn("homebrew", ca.CLAUDE_PATH)

    def test_env_allowlist_unchanged(self) -> None:
        self.assertEqual(ca.CLAUDE_ENV_ALLOW, ("HOME", "PATH", "USER", "LANG", "TMPDIR", "TERM"))


class LinuxSingleBuildTest(unittest.TestCase):
    def test_explicit_binary_keeps_the_run_steps_only_arms_without_run_script(self) -> None:
        # One explicit driver backs every arm on Linux; it ships run_script, which only the script arm may expose.
        with tempfile.TemporaryDirectory() as tmp:
            saved = dict(ca.CUA_BUILDS)
            saved_globals = (ca.PINS_FILE, ca.CUA_BIN, ca.CUA_SKILLS)
            try:
                args = rb.build_parser().parse_args(
                    ["preflight", "--bench-v2", "--cua-bin", "/bin/true", "--cua-skills", tmp,
                     "--pins", str(Path(tmp) / "p.json"), "--runs-dir", tmp,
                     "--arms", *ca.CUA_ARMS]
                )  # fmt: skip
                with mock.patch.object(rb.Ctx, "__init__", return_value=None):
                    rb.make_ctx(args)
                for arm, build in ca.CUA_BUILDS.items():
                    self.assertEqual(
                        ca.SCRIPT_FLAG in build.daemon_env, arm == "cc-cua-driver-script", arm
                    )
                    self.assertEqual(
                        ca.NO_SCRIPT_FLAG in build.daemon_env, arm != "cc-cua-driver-script", arm
                    )
            finally:
                ca.CUA_BUILDS.clear()
                ca.CUA_BUILDS.update(saved)
                ca.PINS_FILE, ca.CUA_BIN, ca.CUA_SKILLS = saved_globals


def _x11_ready() -> bool:
    if sys.platform != "linux" or not os.environ.get("DISPLAY"):
        return False
    try:
        return subprocess.run(["xdpyinfo"], capture_output=True, timeout=10).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def _benchlab_running() -> bool:
    return (
        subprocess.run(
            ["pgrep", "-f", r"^/usr/bin/python3 \S*linux/benchlab\.py"], capture_output=True
        ).returncode
        == 0
    )


@unittest.skipUnless(_x11_ready(), "headless X11 session required")
class LinuxPreflightCaptureTest(unittest.TestCase):
    """Preflight on a real desktop: the capture must be a valid, non-uniform frame, and nothing is left running."""

    def test_preflight_captures_a_window_and_cleans_up(self) -> None:
        arm = rb.cxn.MCP_ARMS[0]
        frames: list[str] = []

        def capture(ctx, name):
            # cursor-free, like the driver's own capture: an empty desktop must come out uniform
            shot = ctx.run_dir / "shot.png"
            subprocess.run(
                ["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
                 "-i", os.environ["DISPLAY"], "-frames:v", "1", str(shot)],
                check=True, timeout=30,
            )  # fmt: skip
            frames.append("running" if _benchlab_running() else "absent")
            try:  # the runner's own validity check: raises on a uniform frame
                return f"{name} real capture", "pass", rb.rec.capture_detail(shot)
            except RuntimeError as error:
                return f"{name} real capture", "fail", str(error)

        self.assertFalse(_benchlab_running())
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp, offline=True, allow_non_terminal=True)
            ctx.task_ids = []
            ctx.arm_names = [arm]
            ctx.mcp[arm] = (Path(tmp) / "mcp.json", rb.cxn.SERVER_OF[arm])
            cli = Path(tmp) / "codex"
            cli.touch()
            with (
                mock.patch.object(rb, "terminal_ancestor", return_value=True),
                mock.patch.object(rb.rec, "free_gb", return_value=1000),
                mock.patch.object(rb, "check_pins", return_value=[]),
                mock.patch.object(rb, "registered_bench_copies", return_value=[]),
                mock.patch.object(rb.cxn, "CODEX_CLI", cli),
                mock.patch.object(
                    rb.cxn, "server_entry", return_value={"command": "mcp", "args": [], "env": {}}
                ),
                mock.patch.object(rb.ca, "cu_helper_observed", return_value={}),
                mock.patch.object(rb.ca, "check_cu_helper_pins", return_value=[]),
                mock.patch.object(rb, "cu_helper_live_checks", return_value=[]),
                mock.patch.object(rb, "native_codex_checks", return_value=[]),
                mock.patch.object(rb, "real_capture_check", side_effect=capture),
            ):
                rows = rb.preflight(ctx)
        by_name = {name: (status, detail) for name, status, detail in rows}
        status, detail = by_name[f"{arm} real capture"]
        self.assertEqual(status, "pass", detail)
        self.assertEqual(frames, ["running"])
        self.assertFalse(_benchlab_running())


def _has_gtk_and_atspi() -> bool:
    try:
        import gi

        gi.require_version("Gtk", "3.0")
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi, Gtk  # noqa: F401
    except (ImportError, ValueError):
        return False
    return True


@unittest.skipUnless(
    _x11_ready() and _has_gtk_and_atspi(), "headless X11 session with GTK required"
)
class LinuxSharedDesktopTest(unittest.TestCase):
    """BenchLab's `desk` mode and the sentinel's mouse user (PA-01, UM-01) on a real X11 desktop."""

    SEED = 7

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="cdb-desk-"))
        self.procs: list[subprocess.Popen] = []
        for path in (BENCH / "probes" / "_common", BENCH / "swift"):
            if str(path) not in sys.path:
                sys.path.insert(0, str(path))

    def tearDown(self) -> None:
        for proc in self.procs:
            if proc.poll() is None:
                proc.terminate()
        for proc in self.procs:
            with contextlib.suppress(subprocess.TimeoutExpired):
                proc.wait(timeout=10)
            if proc.poll() is None:
                proc.kill()
        import shutil

        shutil.rmtree(self.tmp, ignore_errors=True)

    # --- helpers
    def spawn(self, script: str, *argv: str, name: str) -> subprocess.Popen:
        proc = subprocess.Popen(
            [sys.executable, str(BENCH / "linux" / script), *argv],
            stdout=(self.tmp / f"{name}.log").open("w"),
            stderr=subprocess.STDOUT,
        )
        self.procs.append(proc)
        return proc

    def wait_for(self, predicate, limit: float = 20.0, what: str = "condition") -> None:
        end = time.monotonic() + limit
        while time.monotonic() < end:
            if predicate():
                return
            time.sleep(0.1)
        self.fail(f"timed out waiting for {what}")

    def window_ids(self, title: str) -> list[str]:
        found = subprocess.run(
            ["xdotool", "search", "--name", f"^{title}$"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return found.stdout.split()

    def geometry(self, title: str) -> dict[str, int]:
        window = self.window_ids(title)[0]
        out = subprocess.run(
            ["xdotool", "getwindowgeometry", "--shell", window],
            capture_output=True, text=True, timeout=10, check=True,
        ).stdout  # fmt: skip
        return {k: int(v) for k, v in (line.split("=") for line in out.split())}

    def start_desk(
        self,
        label: str,
        frame: str | None = None,
        name: str = "desk",
        extra: tuple[str, ...] = (),
        title: str | None = None,
    ) -> tuple[Path, Path]:
        state, events = self.tmp / f"{name}-state.json", self.tmp / f"{name}-events.jsonl"
        argv = ["--mode", "desk", "--seed", str(self.SEED), "--state", str(state),
                "--events", str(events), "--instance", label]  # fmt: skip
        if frame:
            argv += ["--frame", frame]
        self.spawn("benchlab.py", *argv, *extra, name=name)
        self.wait_for(lambda: state.exists(), what="BenchLab desk state")
        self.wait_for(
            lambda: self.window_ids(title or f"BenchLab Desk {label}"), what="the desk window"
        )
        time.sleep(0.5)
        return state, events

    def evaluate_desk(
        self, cell: dict, others: list[dict], state: Path, events: Path, hard: bool = False
    ) -> dict:
        import benchlab_common as C

        loaded = C.load_state(state)
        evs, bad = C.load_events(events)

        def check(seed, st, ev, checks):
            C.check_desk(cell, others, st, ev, checks, hard=hard)

        return C.evaluate_inputs("desk", self.SEED, loaded, evs, bad, check)

    def control(
        self, name: str, role: str, window: str, limit: float = 10, missing_ok: bool = False
    ):
        """Screen centre of the accessible control `name` of `role` inside the window titled `window`; with
        `missing_ok`, None when it is not in the tree within `limit` seconds (hidden controls are absent)."""
        import gi

        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi

        roles = {"button", "push button"} if role == "button" else {role}

        def walk(node, inside):
            inside = inside or node.get_name() == window
            if inside and node.get_name() == name and node.get_role_name() in roles:
                return node
            for i in range(node.get_child_count()):
                child = node.get_child_at_index(i)
                found = walk(child, inside) if child is not None else None
                if found is not None:
                    return found
            return None

        end = time.monotonic() + limit
        while time.monotonic() < end:
            node = walk(Atspi.get_desktop(0), False)
            if node is not None:
                box = node.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
                if box.width > 0 and box.height > 0:
                    return box.x + box.width // 2, box.y + box.height // 2
            time.sleep(0.1)
        if missing_ok:
            return None
        self.fail(f"accessible control not found: {name} / {role} in {window}")

    def xdo(self, *argv: object) -> None:
        subprocess.run(["xdotool", *map(str, argv)], check=True, timeout=10)
        time.sleep(0.15)

    def drive(self, label: str, code: str, shelf: str | None) -> None:
        title = f"BenchLab Desk {label}"
        x, y = self.control("Code", "text", title)
        self.xdo("mousemove", x, y, "click", 1)
        self.xdo("key", "ctrl+a")
        self.xdo("type", "--clearmodifiers", code)
        if shelf:
            x, y = self.control(shelf, "radio button", title)
            self.xdo("mousemove", x, y, "click", 1)
        x, y = self.control("Submit", "button", title)
        self.xdo("mousemove", x, y, "click", 1)

    # --- BenchLab desk
    def test_desk_title_frame_and_a_scripted_solve(self) -> None:
        import benchlab_common as C

        cell = C.derive_um_desk(self.SEED)
        state, events = self.start_desk(cell["label"], ",".join(map(str, cell["frame"])))
        geometry = self.geometry(f"BenchLab Desk {cell['label']}")
        x, y, w, h = cell["frame"]
        self.assertEqual((geometry["WIDTH"], geometry["HEIGHT"]), (w, h))
        self.assertLessEqual(abs(geometry["X"] - x), 40)
        self.assertLessEqual(abs(geometry["Y"] - y), 60)
        start = json.loads(state.read_text())
        self.assertEqual(
            (
                start["mode"],
                start["instance"],
                start["code"],
                start["shelf"],
                start["submit_count"],
            ),
            ("desk", cell["label"], "", None, 0),
        )
        self.assertIsNone(start["submitted"])
        noop = self.evaluate_desk(cell, [], state, events)
        self.assertFalse(noop["passed"])
        self.assertFalse(noop["checks"]["code_correct"]["passed"])
        self.assertFalse(noop["checks"]["submitted_once"]["passed"])

        self.drive(cell["label"], cell["code"], cell["shelf"])
        result = self.evaluate_desk(cell, [], state, events)
        self.assertTrue(result["passed"], result["checks"])
        done = json.loads(state.read_text())
        self.assertEqual(done["submitted"], {"code": cell["code"], "shelf": cell["shelf"]})
        self.assertEqual(done["submit_count"], 1)

    def test_enter_does_not_submit_and_a_foreign_code_is_caught(self) -> None:
        import benchlab_common as C

        cells = C.derive_parallel(self.SEED, 2)["cells"]
        mine, other = cells
        state, events = self.start_desk(mine["label"], ",".join(map(str, mine["frame"])))
        title = f"BenchLab Desk {mine['label']}"
        x, y = self.control("Code", "text", title)
        self.xdo("mousemove", x, y, "click", 1)
        self.xdo(
            "type", "--clearmodifiers", other["code"]
        )  # another agent's code lands in this desk
        self.xdo("key", "Return")
        self.assertEqual(json.loads(state.read_text())["submit_count"], 0)
        x, y = self.control("Submit", "button", title)
        self.xdo("mousemove", x, y, "click", 1)
        result = self.evaluate_desk(mine, [other], state, events)
        self.assertFalse(result["passed"])
        self.assertFalse(result["checks"]["no_foreign_input"]["passed"])

    # --- the mouse user
    def run_mouse_user(
        self, seconds: float, during=None, cover: bool = False, profile: str = "um01"
    ) -> dict:
        import benchlab_common as C
        import summarize_sentinel

        script = self.tmp / f"mouse-user-{len(self.procs)}.json"
        C.write_mouse_script(self.SEED, script, profile)
        log = script.with_suffix(".jsonl")
        sentinel = self.spawn(
            "sentinel.py", "--log", str(log), "--mouse-user", str(script), "--frame", "420,340,900,520",
            name=log.stem,
        )  # fmt: skip
        self.wait_for(lambda: log.exists() and '"ready"' in log.read_text(), what="the sentinel")
        self.wait_for(lambda: self.window_ids("User activity"), what="the user window")
        time.sleep(1.0)
        if cover:
            code = (
                "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk\n"
                "w = Gtk.Window(title='Cover'); w.set_default_size(1100, 700); w.move(380, 300)\n"
                "w.set_keep_above(True); w.connect('destroy', Gtk.main_quit); w.show_all(); Gtk.main()\n"
            )
            self.procs.append(subprocess.Popen([sys.executable, "-c", code]))
            self.wait_for(lambda: self.window_ids("Cover"), what="the covering window")
            time.sleep(0.5)
        os.kill(sentinel.pid, signal.SIGUSR2)  # arm
        started = time.monotonic()
        if during:
            during(started)
        time.sleep(max(0.0, seconds - (time.monotonic() - started)))
        os.kill(sentinel.pid, signal.SIGUSR2)  # disarm
        time.sleep(0.5)
        lines = [json.loads(ln) for ln in log.read_text().splitlines() if ln.strip()]
        self.assertFalse([ln for ln in lines if ln.get("ev") == "mu_error"], lines[-5:])
        start = next(ln for ln in lines if ln.get("ev") == "mu_start")
        self.assertEqual((start["platform"], start["tagging"]), ("linux-x11", False))
        self.assertTrue(any(ln.get("ev") == "mu_stop" for ln in lines))
        summary = summarize_sentinel.summarize(str(log))
        self.assertIsNotNone(summary["mouse_user"])
        return summary

    def test_mouse_user_clicks_land_and_the_pointer_follows_the_script(self) -> None:
        mu = self.run_mouse_user(7.0)["mouse_user"]
        self.assertGreaterEqual(mu["clicks"], 2, mu)
        self.assertEqual(mu["clicks_landed"], mu["clicks"], mu)
        self.assertLessEqual(mu["max_displacement_px"], 3, mu)
        self.assertGreater(mu["checks"], 30, mu)
        self.assertIsNone(mu["foreign_pointer_events"])  # nothing is tagged on X11

    def test_a_pointer_warp_shows_as_displacement(self) -> None:
        def warp(started: float) -> None:
            for _ in range(3):
                time.sleep(0.9)
                subprocess.run(["xdotool", "mousemove", "5", "5"], check=True, timeout=10)

        mu = self.run_mouse_user(7.0, during=warp)["mouse_user"]
        self.assertGreater(mu["max_displacement_px"], 3, mu)
        self.assertGreaterEqual(mu["displaced_checks"], 1, mu)

    def test_clicks_into_a_covered_canvas_are_lost(self) -> None:
        mu = self.run_mouse_user(7.0, cover=True)["mouse_user"]
        self.assertGreaterEqual(mu["clicks"], 2, mu)
        self.assertEqual(mu["clicks_landed"], 0, mu)
        self.assertGreaterEqual(mu["clicks_lost"], 1, mu)

    # --- hard variants: BenchLab --title / --confirm
    def test_desk_title_and_the_inline_confirm_step(self) -> None:
        import benchlab_common as C

        cell = C.derive_crowded(self.SEED, 2)["cells"][0]
        args = C.crowded_app_args(cell)
        self.assertIn("--confirm", args)
        title = cell["title"]
        self.assertEqual(title, "BenchLab Desk")  # one title for every copy
        state, events = self.start_desk(
            cell["label"], ",".join(map(str, cell["frame"])), extra=("--title", title, "--confirm"),
            title=title,
        )  # fmt: skip
        start = json.loads(state.read_text())
        self.assertIs(start["confirm_pending"], False)
        self.assertIsNone(self.control("Confirm", "button", title, limit=1, missing_ok=True))
        self.assertIsNone(self.control("Cancel", "button", title, limit=1, missing_ok=True))

        # a premature Submit and Cancel: nothing is submitted
        x, y = self.control("Submit", "button", title)
        self.xdo("mousemove", x, y, "click", 1)
        pending = json.loads(state.read_text())
        self.assertIs(pending["confirm_pending"], True)
        self.assertEqual(pending["submit_count"], 0)
        self.assertIsNotNone(self.control("Confirm", "button", title))
        x, y = self.control("Cancel", "button", title)
        self.xdo("mousemove", x, y, "click", 1)
        cancelled = json.loads(state.read_text())
        self.assertIs(cancelled["confirm_pending"], False)
        self.assertEqual(cancelled["submit_count"], 0)
        self.assertIsNone(self.control("Confirm", "button", title, limit=1, missing_ok=True))

        # the real thing: fill, Submit, Confirm
        x, y = self.control("Code", "text", title)
        self.xdo("mousemove", x, y, "click", 1)
        self.xdo("type", "--clearmodifiers", cell["code"])
        x, y = self.control(cell["shelf"], "radio button", title)
        self.xdo("mousemove", x, y, "click", 1)
        x, y = self.control("Submit", "button", title)
        self.xdo("mousemove", x, y, "click", 1)
        self.assertEqual(json.loads(state.read_text())["submit_count"], 0)
        x, y = self.control("Confirm", "button", title)
        self.xdo("mousemove", x, y, "click", 1)
        done = json.loads(state.read_text())
        self.assertEqual(done["submit_count"], 1)
        self.assertEqual(done["submitted"], {"code": cell["code"], "shelf": cell["shelf"]})
        self.assertIs(done["confirm_pending"], False)
        types = [json.loads(ln)["type"] for ln in events.read_text().splitlines()]
        self.assertEqual(
            [t for t in types if t.startswith("confirm") or t == "submit"],
            ["confirm_open", "confirm_cancel", "confirm_open", "submit"],
        )
        result = self.evaluate_desk(cell, [], state, events, hard=True)
        self.assertTrue(result["passed"], result["checks"])

    def test_desk_without_confirm_is_unchanged(self) -> None:
        import benchlab_common as C

        cell = C.derive_um_desk(self.SEED)
        state, events = self.start_desk(cell["label"], ",".join(map(str, cell["frame"])))
        self.assertNotIn("confirm_pending", json.loads(state.read_text()))
        title = f"BenchLab Desk {cell['label']}"
        self.assertIsNone(self.control("Confirm", "button", title, limit=1, missing_ok=True))

    # --- the busy user (UM-02): clicks and drags with the button held
    def test_busy_user_clicks_and_drags_land(self) -> None:
        mu = self.run_mouse_user(5.0, profile="um02")["mouse_user"]
        self.assertGreaterEqual(mu["clicks"], 8, mu)
        self.assertGreaterEqual(mu["drags"], 2, mu)
        self.assertEqual(mu["clicks_landed"], mu["clicks"], mu)
        self.assertEqual(mu["drags_landed"], mu["drags"], mu)
        self.assertLessEqual(mu["max_displacement_px"], 3, mu)
        self.assertGreater(mu["checks"], 150, mu)  # about 4 x UM-01's event rate

    def test_busy_user_a_warp_is_displacement(self) -> None:
        def warp(started: float) -> None:
            time.sleep(1.0)
            subprocess.run(["xdotool", "mousemove", "5", "5"], check=True, timeout=10)

        mu = self.run_mouse_user(5.0, during=warp, profile="um02")["mouse_user"]
        self.assertGreater(mu["max_displacement_px"], 3, mu)

    def test_busy_user_into_a_covered_canvas_is_lost(self) -> None:
        mu = self.run_mouse_user(5.0, cover=True, profile="um02")["mouse_user"]
        self.assertGreaterEqual(mu["clicks_lost"], 1, mu)
        self.assertGreaterEqual(mu["drags"], 1, mu)
        self.assertEqual(mu["drags_landed"], 0, mu)

    def test_busy_user_a_stolen_button_up_cuts_the_drag(self) -> None:
        def steal(started: float) -> None:
            end = time.monotonic() + 4.0
            time.sleep(0.5)
            while (
                time.monotonic() < end
            ):  # a second synthetic click, with a warp, while the button is held
                subprocess.run(
                    ["xdotool", "mousemove", "5", "5", "click", "1"], check=True, timeout=10
                )
                time.sleep(0.1)

        mu = self.run_mouse_user(5.0, during=steal, profile="um02")["mouse_user"]
        self.assertGreaterEqual(mu["drags_cut"], 1, mu)


class LinuxCaptureSubjectOffLinuxTest(unittest.TestCase):
    def test_not_used_on_macos(self) -> None:
        with (
            mock.patch.object(rb.desktop, "detect_platform", return_value="macos"),
            mock.patch.object(rb.desktop, "start_app", side_effect=AssertionError),
            tempfile.TemporaryDirectory() as tmp,
        ):
            with rb.capture_subject(Path(tmp)):
                pass


class DiskFloorTest(unittest.TestCase):
    def test_flag_lowers_the_floor(self) -> None:
        with (
            tempfile.TemporaryDirectory() as tmp,
            mock.patch.object(rb.rec, "free_gb", return_value=10.0),
        ):
            ctx = make_ctx(tmp, disk_floor_gb=5.0, no_quota=True)
            ctx.quota = {}
            with mock.patch.object(rb.core, "quota_gate", return_value={"action": "go"}):
                rb.check_gates(ctx)
            ctx.args.disk_floor_gb = rb.DISK_FLOOR_GB
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "DISK")

    def test_invalid_values_rejected(self) -> None:
        for bad in ("0", "-1", "nan", "inf"):
            with self.subTest(bad), self.assertRaises(SystemExit):
                with mock.patch("sys.stderr"):
                    rb.build_parser().parse_args(["preflight", "--disk-floor-gb", bad])
        self.assertTrue(math.isfinite(rb.positive_gb("5")))
        args = rb.build_parser().parse_args(["preflight"])
        self.assertEqual(args.disk_floor_gb, rb.DISK_FLOOR_GB)


class RecordPidTest(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_start_app_records_pid_without_argv(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            with mock.patch.dict(os.environ, {"CDB_BENCH_PID_LOG": str(ledger)}):
                proc = desktop.start_app(["sleep", "30"], Path(tmp) / "home", Path(tmp) / "log")
            try:
                entry = json.loads(ledger.read_text())
                self.assertEqual(entry["pid"], proc.pid)
                self.assertEqual(entry["pgid"], proc.pid)
                self.assertEqual(set(entry), {"pid", "pgid", "role", "start"})
                self.assertEqual(entry["role"], "sleep")
            finally:
                desktop.stop_group(proc.pid)
                proc.wait(5)

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_ledger_write_failure_reaps_the_new_process(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            made: list[subprocess.Popen] = []
            real = subprocess.Popen

            def spy(*a, **k):
                made.append(real(*a, **k))
                return made[-1]

            env = {"CDB_BENCH_PID_LOG": str(Path(tmp) / "missing" / "pids.jsonl")}
            with (
                mock.patch.dict(os.environ, env),
                mock.patch.object(desktop.subprocess, "Popen", spy),
                self.assertRaises(OSError),
            ):
                desktop.start_app(["sleep", "30"], Path(tmp) / "home", Path(tmp) / "log")
            self.assertEqual(len(made), 1)
            self.assertIsNotNone(made[0].poll())

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_sigterm_reaps_private_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp) / "pid"
            fake = Path(tmp) / "driver"
            fake.write_text(
                f'#!/bin/sh\n[ "$1" = serve ] && {{ echo $$ > {pidfile}; exec sleep 300; }}\n'
                "exit 1\n"
            )
            fake.chmod(0o755)
            proc = subprocess.Popen(
                [sys.executable, str(BENCH / "desktop_runtime.py"), "doctor",
                 "--cua-bin", str(fake), "--out", str(Path(tmp) / "out")]
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.1)
                time.sleep(0.5)
                daemon = int(pidfile.read_text())
                proc.send_signal(signal.SIGTERM)
                proc.wait(20)
                time.sleep(0.5)
                with self.assertRaises(ProcessLookupError):
                    os.kill(daemon, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_sigterm_at_startup_handoff_reaps_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp) / "pid"
            fake = Path(tmp) / "driver"
            fake.write_text(
                f'#!/bin/sh\n[ "$1" = serve ] && {{ echo $$ > {pidfile}; exec sleep 300; }}\n'
                "exit 1\n"
            )
            fake.chmod(0o755)
            real = desktop.start_app

            def start_then_term(*a, **k):
                proc = real(*a, **k)
                os.kill(os.getpid(), signal.SIGTERM)  # lands before the caller owns proc
                return proc

            with (
                mock.patch.object(desktop, "start_app", start_then_term),
                self.assertRaises(SystemExit) as raised,
            ):
                desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(Path(tmp) / "out")])
            self.assertEqual(raised.exception.code, 143)
            daemon = int(pidfile.read_text())
            time.sleep(0.5)
            with self.assertRaises(ProcessLookupError):
                os.kill(daemon, 0)

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_cancel_then_retry_starts_cleanly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp) / "pid"
            fake = Path(tmp) / "driver"
            fake.write_text(
                f'#!/bin/sh\n[ "$1" = serve ] && {{ echo $$ > {pidfile}; exec sleep 300; }}\n'
                "exit 0\n"
            )
            fake.chmod(0o755)
            real = desktop.start_app

            def start_then_term(*a, **k):
                proc = real(*a, **k)
                os.kill(os.getpid(), signal.SIGTERM)
                return proc

            handler = signal.getsignal(signal.SIGTERM)
            with (
                mock.patch.object(desktop, "start_app", start_then_term),
                self.assertRaises(SystemExit) as raised,
            ):
                desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(Path(tmp) / "o1")])
            self.assertEqual(raised.exception.code, 143)
            self.assertIs(signal.getsignal(signal.SIGTERM), handler)

            state = Path(tmp) / "state"
            (state / "home").mkdir(parents=True)
            proc = desktop.start_driver(fake, str(state / "d.sock"), state, overlay=False)
            try:
                self.assertIsNone(proc.poll())
            finally:
                desktop.stop_group(proc.pid)
                proc.wait(5)

            with mock.patch("sys.stdout"):
                code = desktop.main(
                    ["doctor", "--cua-bin", str(fake), "--out", str(Path(tmp) / "o2")]
                )
            self.assertIn(code, (0, 1))
            report = json.loads((Path(tmp) / "o2" / "doctor.json").read_text())
            self.assertEqual(report["daemon"], "private")
            time.sleep(0.5)
            with self.assertRaises(ProcessLookupError):
                os.kill(report["daemon_pid"], 0)

    def test_cancel_helpers_are_inert_outside_doctor(self) -> None:
        with desktop.deferred_cancel():
            desktop.check_cancel()
        desktop.check_cancel()

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_second_sigterm_during_teardown_still_reaps_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile, termed = Path(tmp) / "pid", Path(tmp) / "termed"
            fake = Path(tmp) / "driver"
            fake.write_text(
                "#!/bin/sh\n"
                f'[ "$1" = serve ] && {{ echo $$ > {pidfile}; '
                f"trap 'touch {termed}' TERM; while :; do sleep 0.1; done; }}\n"
                "exit 1\n"
            )
            fake.chmod(0o755)
            proc = subprocess.Popen(
                [sys.executable, str(BENCH / "desktop_runtime.py"), "doctor",
                 "--cua-bin", str(fake), "--out", str(Path(tmp) / "out")]
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.1)
                time.sleep(0.5)
                daemon = int(pidfile.read_text())
                proc.send_signal(signal.SIGTERM)
                deadline = time.monotonic() + 10
                while not termed.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                proc.send_signal(signal.SIGTERM)
                proc.wait(20)
                time.sleep(0.5)
                with self.assertRaises(ProcessLookupError):
                    os.kill(daemon, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()

    def test_noop_without_variable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ):
            os.environ.pop("CDB_BENCH_PID_LOG", None)
            self.assertIsNone(desktop.record_pid(os.getpid(), "x"))

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_cdb_start_apps_records_before_readiness_failure(self) -> None:
        import cdb_adapter

        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            task = cdb_adapter.CdbTask.__new__(cdb_adapter.CdbTask)
            task.artifacts = Path(tmp)
            task.bundle = Path(tmp)
            task.procs, task.log = [], []
            task.sub = lambda text, **_: text
            app = {
                "id": "stub",
                "command": ["sleep", "30"],
                "ready": {"http": "http://127.0.0.1:1/", "timeout_seconds": 0.2},
            }
            task.apps = lambda: [app]
            try:
                with mock.patch.dict(os.environ, {"CDB_BENCH_PID_LOG": str(ledger)}):
                    with self.assertRaises(RuntimeError):
                        task.start_apps()
                self.assertEqual(json.loads(ledger.read_text())["role"], "stub")
            finally:
                for proc in task.procs:
                    desktop.stop_group(proc.pid)
                    proc.wait(5)


def write_run(root: Path, *, arm: str = "cc-cua-driver", recorder: str = "none", argv=None) -> Path:
    run = root / "run"
    attempt = run / "trials" / "T1" / "a1"
    (attempt / "artifacts").mkdir(parents=True)
    (attempt / "trial.json").write_text("{}")
    (attempt / "artifacts" / "failure.png").write_bytes(b"png")
    (attempt / "artifacts" / "driver.log").write_text("log")
    (attempt / "artifacts" / "sentinel.jsonl").write_text('{"event": 1}\n')
    row = {"trial_id": "T1", "attempt": 1, "arm": arm, "passed": False, "recording_started": False}
    (run / "results.jsonl").write_text(json.dumps(row) + "\n")
    (run / "manifest.json").write_text(json.dumps({"recorder": recorder, "argv": argv or []}))
    return run


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux doctor")
class SpawnOwnershipTest(unittest.TestCase):
    """An unwritable ledger must fail the launch and leave no live child."""

    def setUp(self) -> None:
        if sys.platform != "linux":
            self.skipTest("Linux only")
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.made: list[subprocess.Popen] = []
        real = subprocess.Popen

        def spy(*a, **k):
            self.made.append(real(*a, **k))
            return self.made[-1]

        self.spy = spy
        self.addCleanup(self._tmp.cleanup)
        self.addCleanup(self._reap)

    def _reap(self) -> None:
        for child in self.made:
            if child.poll() is None:
                with contextlib.suppress(OSError):
                    os.killpg(child.pid, signal.SIGKILL)
            with contextlib.suppress(Exception):
                child.wait(5)

    def ledger(self, writable: bool = False) -> dict[str, str]:
        if writable:
            return {"CDB_BENCH_PID_LOG": str(self.tmp / "pids.jsonl")}
        return {"CDB_BENCH_PID_LOG": str(self.tmp / "missing" / "pids.jsonl")}

    def check(self, count: int = 1) -> None:
        self.assertEqual(len(self.made), count)
        for child in self.made:
            self.assertIsNotNone(child.poll())

    def test_agent_launch(self) -> None:
        import claude_driver

        with (
            mock.patch.dict(os.environ, self.ledger()),
            mock.patch.object(claude_driver.ca, "open_token_fd", return_value=None),
            mock.patch.object(claude_driver.subprocess, "Popen", self.spy),
            self.assertRaises(OSError),
        ):
            claude_driver.run_claude(
                ["sleep", "30"],
                dict(os.environ),
                self.tmp,
                "p",
                set(),
                5.0,
                self.tmp / "s",
                self.tmp / "e",
                self.tmp / "l",
            )
        self.check()

    def test_app_server_launch(self) -> None:
        import codex_native

        with (
            mock.patch.dict(os.environ, self.ledger()),
            mock.patch.object(codex_native.subprocess, "Popen", self.spy),
            self.assertRaises(OSError),
        ):
            codex_native.run_codex(
                ["sleep", "30"],
                dict(os.environ),
                self.tmp,
                "p",
                5.0,
                1,
                self.tmp / "r",
                self.tmp / "s",
                self.tmp / "e",
            )
        self.check()

    def test_pilot_run_agent(self) -> None:
        import run_pilot

        with (
            mock.patch.dict(os.environ, self.ledger()),
            mock.patch.object(run_pilot.arms, "render_home", return_value=dict(os.environ)),
            mock.patch.object(run_pilot.arms, "codex_argv", return_value=["sleep", "30"]),
            mock.patch.object(run_pilot.subprocess, "Popen", self.spy),
            self.assertRaises(OSError),
        ):
            (self.tmp / "t").mkdir()
            run_pilot.run_agent("a", "p", self.tmp, self.tmp / "t", 5, "m", "low")
        self.check()

    def launch_apps(self, apps: list[dict]) -> None:
        import run_pilot

        compare = mock.Mock()
        compare.expand_placeholders = lambda raw, variables: raw
        compare._resolve_executable = lambda command: command
        with mock.patch.object(run_pilot.subprocess, "Popen", self.spy):
            run_pilot.launch_apps(
                compare,
                {"apps": apps},
                {"bundle": str(self.tmp)},
                dict(os.environ),
                self.tmp / "logs",
            )

    def test_launch_apps_first_registration_fails(self) -> None:
        with mock.patch.dict(os.environ, self.ledger()), self.assertRaises(OSError):
            self.launch_apps([{"id": "a", "command": ["sleep", "30"]}])
        self.check()

    def test_launch_apps_later_registration_fails(self) -> None:
        real = desktop.record_pid
        calls = [0]

        def once(pid: int, role: str) -> None:
            calls[0] += 1
            if calls[0] > 1:
                raise OSError("ledger")
            real(pid, role)

        apps = [{"id": "a", "command": ["sleep", "30"]}, {"id": "b", "command": ["sleep", "30"]}]
        with (
            mock.patch.dict(os.environ, self.ledger(True)),
            mock.patch.object(desktop, "record_pid", once),
            self.assertRaises(OSError),
        ):
            self.launch_apps(apps)
        self.check(2)

    def test_launch_apps_readiness_fails(self) -> None:
        apps = [
            {"id": "a", "command": ["sleep", "30"]},
            {
                "id": "b",
                "command": ["sleep", "30"],
                "ready": {"http": "http://127.0.0.1:9/", "timeout_seconds": 1},
            },
        ]
        with mock.patch.dict(os.environ, self.ledger(True)), self.assertRaises(TimeoutError):
            self.launch_apps(apps)
        self.check(2)

    def test_record_or_reap_disabled_and_enabled(self) -> None:
        proc = subprocess.Popen(["sleep", "30"], start_new_session=True)
        self.made.append(proc)
        with mock.patch.dict(os.environ, self.ledger(True)):
            desktop.record_or_reap(proc, "sleep")
        self.assertIsNone(proc.poll())
        self.assertEqual(len((self.tmp / "pids.jsonl").read_text().splitlines()), 1)
        env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
        with mock.patch.dict(os.environ, env, clear=True):
            desktop.record_or_reap(proc, "sleep")
        self.assertEqual(len((self.tmp / "pids.jsonl").read_text().splitlines()), 1)

    def test_record_or_reap_reraises_same_exception_and_reaps_group(self) -> None:
        proc = subprocess.Popen(["sh", "-c", "sleep 30 & wait"], start_new_session=True)
        self.made.append(proc)
        time.sleep(0.3)
        boom = KeyboardInterrupt()
        with (
            mock.patch.object(desktop, "record_pid", side_effect=boom),
            self.assertRaises(KeyboardInterrupt) as caught,
        ):
            desktop.record_or_reap(proc, "x")
        self.assertIs(caught.exception, boom)
        self.assertIsNotNone(proc.poll())
        with self.assertRaises(ProcessLookupError):
            os.killpg(proc.pid, 0)


class DoctorStartupLogTest(unittest.TestCase):
    def test_failed_start_keeps_driver_log(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "driver"
            fake.write_text("#!/bin/sh\necho startup-boom >&2\nexit 3\n")
            fake.chmod(0o755)
            out = Path(tmp) / "out"
            self.assertEqual(desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(out)]), 1)
            report = json.loads((out / "doctor.json").read_text())
            self.assertTrue(any(c["name"] == "driver-start" for c in report["checks"]))
            self.assertIn("startup-boom", (out / "driver.log").read_text())


class LedgerReuseTest(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_recycled_pid_is_not_the_recorded_child(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            stale = {"pid": os.getpid(), "pgid": os.getpid(), "role": "old", "start": 1}
            ledger.write_text(json.dumps(stale) + "\n")
            self.assertEqual(evidence.check_ledger(ledger), [])
            live = dict(stale, start=desktop.proc_start(os.getpid()))
            ledger.write_text(json.dumps(live) + "\n")
            self.assertTrue(any("still alive" in p for p in evidence.check_ledger(ledger)))


class EvidenceTest(unittest.TestCase):
    def check(self, run: Path) -> list[str]:
        return evidence.check_run(run)

    def test_complete_failed_task_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self.check(write_run(Path(tmp))), [])

    def test_each_gap_is_named(self) -> None:
        cases = {
            "screenshot": lambda r: (r / "trials/T1/a1/artifacts/failure.png").unlink(),
            "driver.log": lambda r: (r / "trials/T1/a1/artifacts/driver.log").unlink(),
            "sentinel": lambda r: (r / "trials/T1/a1/artifacts/sentinel.jsonl").write_text(""),
            "orphan": lambda r: (r / "trials/T2/a1").mkdir(parents=True),
            "duplicate": lambda r: (r / "results.jsonl").write_text(
                (r / "results.jsonl").read_text() * 2
            ),
            "malformed": lambda r: (r / "results.jsonl").write_text("{nope\n"),
        }
        for word, mutate in cases.items():
            with self.subTest(word), tempfile.TemporaryDirectory() as tmp:
                run = write_run(Path(tmp))
                mutate(run)
                self.assertTrue(any(word in p for p in self.check(run)), word)

    def test_recorder_requires_started_and_video(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp), recorder="cua")
            self.assertTrue(any("recording not started" in p for p in self.check(run)))
            row = json.loads((run / "results.jsonl").read_text())
            row["recording_started"] = True
            (run / "results.jsonl").write_text(json.dumps(row) + "\n")
            self.assertTrue(any("video missing" in p for p in self.check(run)))
            video = run / "trials/T1/a1/video"
            video.mkdir()
            (video / "video.mp4").write_bytes(b"x")
            self.assertTrue(any("video missing" in p for p in self.check(run)))
            (video / "video-720p.mp4").write_bytes(b"x")
            (video / "video.json").write_text(json.dumps({"error": "OSError: boom"}))
            self.assertTrue(any("compression failed" in p for p in self.check(run)))
            (video / "video.json").write_text("{}")
            self.assertEqual(self.check(run), [])

    def test_bad_json_shapes_are_reported(self) -> None:
        for row in ({"trial_id": [], "attempt": 1, "arm": "x"}, {"trial_id": "T1", "attempt": 1}):
            with tempfile.TemporaryDirectory() as tmp:
                run = write_run(Path(tmp))
                (run / "results.jsonl").write_text(json.dumps(row) + "\n")
                self.assertTrue(any("malformed row" in p for p in self.check(run)))
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp))
            (run / "manifest.json").write_text("[]")
            self.assertTrue(any("manifest.json" in p for p in self.check(run)))
            self.assertEqual(evidence.main([str(run)]), 1)
            self.assertTrue((run / "evidence-check.json").is_file())

    def test_session_layout_ledger_is_found(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            session = Path(tmp)
            run = write_run(session / "runs")
            run = run.rename(session / "runs" / "RUN")
            live = subprocess.Popen(["sleep", "30"])
            try:
                (session / "pids.jsonl").write_text(
                    json.dumps({"pid": live.pid, "pgid": None, "role": "s"}) + "\n"
                )
                self.assertTrue(any("still alive" in p for p in self.check(run)))
            finally:
                live.kill()
                live.wait()

    @unittest.skipUnless(os.path.isdir("/proc/self"), "needs /proc")
    def test_zombie_leader_with_live_child_is_alive(self) -> None:
        leader = subprocess.Popen(
            ["bash", "-c", "sleep 30 & echo $!; exit 0"],
            stdout=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        child = int(leader.stdout.readline())
        try:
            time.sleep(0.3)  # leader exits, stays a zombie until waited
            with tempfile.TemporaryDirectory() as tmp:
                ledger = Path(tmp) / "pids.jsonl"
                ledger.write_text(
                    json.dumps({"pid": leader.pid, "pgid": leader.pid, "role": "l"}) + "\n"
                )
                self.assertTrue(
                    any("group still alive" in p for p in evidence.check_ledger(ledger))
                )
        finally:
            os.kill(child, signal.SIGKILL)
            leader.wait()

    def test_opt_outs_lift_requirements(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp), argv=["--no-sentinel"])
            (run / "trials/T1/a1/artifacts/sentinel.jsonl").unlink()
            self.assertEqual(self.check(run), [])

    def test_ledger(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            done = subprocess.Popen(["true"])
            done.wait()
            ledger.write_text(json.dumps({"pid": done.pid, "pgid": None, "role": "x"}) + "\n")
            self.assertEqual(evidence.check_ledger(ledger), [])
            live = subprocess.Popen(["sleep", "30"], start_new_session=True)
            try:
                ledger.write_text(
                    json.dumps({"pid": live.pid, "pgid": live.pid, "role": "s"}) + "\n"
                )
                self.assertTrue(evidence.check_ledger(ledger))
                self.assertIsNone(live.poll())
                with mock.patch.object(evidence.os, "kill", side_effect=PermissionError):
                    self.assertTrue(evidence.check_ledger(ledger))
                seen = []
                with (
                    mock.patch.object(evidence.os, "kill", lambda p, s: seen.append(s)),
                    mock.patch.object(evidence.os, "killpg", lambda p, s: seen.append(s)),
                ):
                    evidence.check_ledger(ledger)
                self.assertEqual(set(seen), {0})
            finally:
                os.killpg(live.pid, signal.SIGKILL)
                live.wait()
            ledger.write_text("not json\n")
            self.assertTrue(any("malformed" in p for p in evidence.check_ledger(ledger)))
            ledger.write_text('{"pid": 0}\n')
            self.assertTrue(evidence.check_ledger(ledger))


class PinsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.bin = root / "cua-driver"
        self.bin.write_text('#!/bin/sh\necho "cua-driver 1.2.3"\n')
        self.bin.chmod(0o755)
        self.skills = root / "skills"
        self.skills.mkdir()
        (self.skills / "SKILL.md").write_text("skill")
        self.report = {
            "ok": True,
            "build": {
                "version": "1.2.3",
                "git_sha": "abc123",
                "exe_sha256": ca.sha256_file(self.bin),
            },
        }
        self.doctor = root / "doctor.json"
        self.doctor.write_text(json.dumps(self.report))
        self.out = root / "pins" / "linux.json"

    def run_tool(self) -> int:
        return pins_tool.main(
            [
                "--cua-bin",
                str(self.bin),
                "--cua-skills",
                str(self.skills),
                "--doctor-json",
                str(self.doctor),
                "--out",
                str(self.out),
            ]  # fmt: skip
        )

    def test_written_pins_satisfy_the_runner(self) -> None:
        with mock.patch.object(pins_tool, "in_checkout", return_value=False):
            self.assertEqual(self.run_tool(), 0)
        pins = json.loads(self.out.read_text())
        digest = ca.sha256_file(self.bin)
        self.assertEqual(pins["platform"], "linux")
        main = pins["cua_main"]
        self.assertEqual({main["binary_sha256"], main["script_binary_sha256"]}, {digest})
        self.assertEqual(pins["cua_skills_tree_sha256"], ca.sha256_tree(self.skills))
        checked_in = json.loads((BENCH / "pins.json").read_text())
        self.assertEqual(pins["cdb_pack"], checked_in["cdb_pack"])
        health = {
            "build": self.report["build"],
            "checks": {"ax_capability": "pass", "screen_capture_capability": "pass"},
            "ok": True,
        }
        with (
            mock.patch.object(rb.ca, "cua_health", return_value=health),
            mock.patch.object(rb.desktop, "capabilities_ok", return_value=True),
        ):
            rows = rb.check_daemon(pins, "cc-cua-driver-script")
        self.assertEqual([r[1] for r in rows], ["pass", "pass", "pass"])

    def test_refusals(self) -> None:
        def refuses(mutate) -> None:
            mutate()
            with mock.patch.object(pins_tool, "in_checkout", return_value=False):
                with self.assertRaises(SystemExit):
                    self.run_tool()

        refuses(lambda: self.report["build"].update(exe_sha256="0" * 64) or self.write())
        self.report["build"]["exe_sha256"] = ca.sha256_file(self.bin)
        refuses(lambda: self.report["build"].update(version="9.9") or self.write())
        self.report["build"]["version"] = "1.2.3"
        refuses(lambda: self.report["build"].update(git_sha="") or self.write())
        self.report["build"]["git_sha"] = "abc123"
        refuses(lambda: (self.skills / "SKILL.md").unlink())
        (self.skills / "SKILL.md").write_text("skill")
        self.out.parent.mkdir(parents=True)
        self.out.write_text("old")
        refuses(lambda: None)
        self.assertEqual(self.out.read_text(), "old")

    def test_competing_writer_is_preserved(self) -> None:
        self.out.parent.mkdir(parents=True)
        real_link = os.link

        def racing(src, dst):
            self.out.write_text("competitor")
            return real_link(src, dst)

        with (
            mock.patch.object(pins_tool, "in_checkout", return_value=False),
            mock.patch.object(pins_tool.os, "link", racing),
            self.assertRaises(SystemExit),
        ):
            self.run_tool()
        self.assertEqual(self.out.read_text(), "competitor")
        self.assertEqual([p.name for p in self.out.parent.iterdir()], [self.out.name])

    def test_refuses_inside_checkout(self) -> None:
        self.out = BENCH / "should-not-exist.json"
        with self.assertRaises(SystemExit):
            self.run_tool()
        self.assertFalse(self.out.exists())

    def write(self) -> None:
        self.doctor.write_text(json.dumps(self.report))


if __name__ == "__main__":
    unittest.main()


# ---------------------------------------------------------------------------------------------------------
# The Linux mouse user's state machine against a simulated X server and canvas (no display, GTK or XTest):
# the schema-2 plan with click and drag legs, the button-up handler, drag resolution and stop-while-dragging.


def _load_sentinel():
    import types

    gi = types.ModuleType("gi")
    gi.require_version = lambda *a, **k: None
    repo = types.ModuleType("gi.repository")
    for name in ("Atspi", "Gdk", "GLib", "Gtk"):
        setattr(repo, name, mock.MagicMock())
    gi.repository = repo
    with mock.patch.dict(sys.modules, {"gi": gi, "gi.repository": repo}):
        spec = importlib.util.spec_from_file_location(
            "cdb_sentinel_sim", BENCH / "linux" / "sentinel.py"
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    return module


class SimWorld:
    """A fake clock, pointer, button and canvas around one MouseUser. Button events are queued and delivered
    after the tick that posted them, as the GTK main loop does."""

    ORIGIN = (100, 100)

    def __init__(self, sentinel_module, script: dict, covered: bool = False) -> None:
        import types

        self.sm = sentinel_module
        self.now = 1000.0
        self.queue: list[tuple[str, tuple[int, int]]] = []
        self.events: list[dict] = []
        self.pointer = (0, 0)
        self.held = False
        self.armed = True
        self.covered = covered
        world = self

        class FakeSentinel:
            canvas = None

            def emit(self, event: str, **details: object) -> bool:
                world.events.append({"ev": event, "t": world.now * 1000, **details})
                return False

        class FakeXTest:
            def move(self, x: int, y: int) -> None:
                world.pointer = (x, y)

            def button(self, down: bool) -> None:
                if down != world.held:  # an X server drops a repeated press or release
                    world.held = down
                    world.queue.append(("down" if down else "up", world.pointer))

            def pointer(self) -> tuple[int, int]:
                return world.pointer

            def close(self) -> None:
                pass

        self.clock = types.SimpleNamespace(monotonic=lambda: world.now, time=lambda: world.now)
        self.mu = sentinel_module.MouseUser(FakeSentinel(), script)
        self.mu.xt = FakeXTest()
        self.mu.origin = lambda: SimWorld.ORIGIN
        sentinel_module.Gtk.main_iteration_do = lambda *_: self.advance(0.002)

    def flush(self) -> None:
        import types

        queue, self.queue = self.queue, []
        for kind, (x, y) in queue:
            if self.covered or not self.armed:
                continue
            event = types.SimpleNamespace(x=x - self.ORIGIN[0], y=y - self.ORIGIN[1])
            (self.mu.rx if kind == "down" else self.mu.up)(event)

    def advance(self, dt: float) -> None:
        self.now += dt
        self.flush()

    def run(self, seconds: float, hook=None) -> None:
        end = self.now + seconds
        while self.now < end and self.mu.running:
            self.now += self.sm.MouseUser.TICK_MS / 1000
            self.mu.tick()
            self.flush()
            if hook:
                hook(self)

    def go(self, seconds: float, hook=None) -> "SimWorld":
        with mock.patch.object(self.sm, "time", self.clock):
            self.mu.start()
            self.run(seconds, hook)
        return self

    def stop(self) -> None:
        with mock.patch.object(self.sm, "time", self.clock):
            self.mu.stop()
            self.armed = (
                False  # the sentinel disarms right after stopping; late events are not forwarded
            )
            self.flush()

    def of(self, name: str) -> list[dict]:
        return [e for e in self.events if e["ev"] == name]


class LinuxMouseUserSimulationTest(unittest.TestCase):
    SEED = 7

    @classmethod
    def setUpClass(cls) -> None:
        if str(BENCH / "probes" / "_common") not in sys.path:
            sys.path.insert(0, str(BENCH / "probes" / "_common"))
        cls.sm = _load_sentinel()

    def script(self, profile: str) -> dict:
        import benchlab_common as C

        return C.derive_mouse_user(self.SEED, profile)

    def test_tick_is_fine_enough_for_an_8_ms_step(self) -> None:
        self.assertLessEqual(self.sm.MouseUser.TICK_MS, 4)

    def test_schema_one_script_is_all_click_legs_and_logs_no_button_up(self) -> None:
        script = self.script("um01")
        self.assertNotIn("plan", script)
        world = SimWorld(self.sm, script).go(8.0)
        self.assertEqual(world.mu.plan, [{"kind": "click", "dot": d} for d in script["sequence"]])
        results = world.of("mu_click_result")
        self.assertGreaterEqual(len(results), 5)
        self.assertTrue(all(r["ok"] and r["why"] == "landed" for r in results), results)
        self.assertTrue(all("kind" not in r for r in results))
        self.assertEqual(world.of("mu_drag"), [])
        self.assertEqual(world.of("mu_up") + world.of("mouseUp") + world.of("mouseDown"), [])
        start = world.of("mu_start")[0]
        self.assertEqual(sorted(start), ["ev", "platform", "t", "tagging", "tolerance_px"])
        self.assertLessEqual(max(e["d"] for e in world.of("mu")), 3)
        gaps = [b["t"] - a["t"] for a, b in zip(world.of("mu"), world.of("mu")[1:])]
        self.assertGreater(
            sum(1 for g in gaps if abs(g - 25) <= 1), len(gaps) / 2
        )  # UM-01 keeps 25 ms

    def test_busy_user_lands_every_click_and_drag_at_an_8_ms_step(self) -> None:
        script = self.script("um02")
        self.assertEqual(script["schema"], "cdb-mouse-user/2")
        world = SimWorld(self.sm, script).go(6.0)
        results = world.of("mu_click_result")
        drags = [r for r in results if r.get("kind") == "drag"]
        self.assertGreaterEqual(len(results), 12, len(results))
        self.assertGreaterEqual(len(drags), 3)
        self.assertTrue(all(r["ok"] and r["why"] == "landed" for r in results), results)
        self.assertEqual(
            [r["n"] for r in results], sorted({r["n"] for r in results}), "one n per action"
        )
        self.assertGreaterEqual(len(world.of("mu_drag")), len(drags))
        drag = world.of("mu_drag")[0]
        for key in ("n", "from", "to", "at_from", "at_to"):
            self.assertIn(key, drag)
        self.assertEqual(world.of("mouseDown") + world.of("mouseUp"), [])
        ups = world.of("mu_up")
        self.assertTrue(ups and all(u["own"] for u in ups), ups)
        self.assertLessEqual(max(e["d"] for e in world.of("mu")), 3)
        gaps = [b["t"] - a["t"] for a, b in zip(world.of("mu"), world.of("mu")[1:])]
        self.assertGreater(sum(1 for g in gaps if abs(g - 8) <= 1), len(gaps) / 2)
        start = world.of("mu_start")[0]
        self.assertEqual((start["click_timeout_ms"], start["drag_timeout_ms"]), (400, 600))
        # the legs follow the plan, drags with the button held between two dots
        plan_kinds = [leg["kind"] for leg in script["plan"]]
        self.assertIn("drag", plan_kinds)
        got = [("drag" if r.get("kind") == "drag" else "click") for r in results]
        self.assertEqual(got, [plan_kinds[i % len(plan_kinds)] for i in range(len(got))])

    def test_a_covered_canvas_loses_clicks_and_drags(self) -> None:
        world = SimWorld(self.sm, self.script("um02"), covered=True).go(5.0)
        world.stop()
        results = world.of("mu_click_result")
        self.assertTrue(results)
        self.assertTrue(all(not r["ok"] and r["why"] == "lost" for r in results), results)
        self.assertTrue(any(r.get("kind") == "drag" for r in results))

    def test_a_stolen_button_up_mid_drag_cuts_the_drag(self) -> None:
        done: list[int] = []

        def steal(world: SimWorld) -> None:
            mu = world.mu
            if not done and mu.state == "drag_move" and mu.k == 3:
                world.pointer = (
                    5,
                    5,
                )  # a pointer warp, then a second synthetic click while the button is held
                world.held = False
                world.queue.append(("up", (5, 5)))
                done.append(mu.pending["n"])

        world = SimWorld(self.sm, self.script("um02")).go(3.0, steal)
        self.assertTrue(done)
        cut = [r for r in world.of("mu_click_result") if r["n"] == done[0]]
        self.assertEqual([(r["ok"], r["why"], r["kind"]) for r in cut], [(False, "cut", "drag")])
        self.assertEqual(len(world.of("mouseUp")), 1)
        self.assertEqual([u["own"] for u in world.of("mu_up")].count(False), 1)
        later = [r for r in world.of("mu_click_result") if r["n"] > done[0]]
        self.assertTrue(later and all(r["ok"] for r in later), later)

    def test_a_drag_whose_release_never_reaches_the_canvas_is_cut(self) -> None:
        done: list[int] = []

        def drop(world: SimWorld) -> None:
            mu = world.mu
            if not done and mu.state == "drag_release":
                done.append(mu.pending["n"])
                world.covered = True  # the release goes elsewhere
            elif done and mu.state == "plan":
                world.covered = False

        world = SimWorld(self.sm, self.script("um02")).go(3.0, drop)
        res = [r for r in world.of("mu_click_result") if r["n"] == done[0]]
        self.assertEqual([(r["why"], r["kind"]) for r in res], [("cut", "drag")])

    def test_a_pointer_warp_during_a_drag_is_displacement_not_a_cut(self) -> None:
        done: list[int] = []

        def warp(world: SimWorld) -> None:
            if not done and world.mu.state == "drag_move" and world.mu.k == 3:
                world.pointer = (5, 5)
                done.append(1)

        world = SimWorld(self.sm, self.script("um02")).go(3.0, warp)
        self.assertGreater(max(e["d"] for e in world.of("mu")), 3)

    def test_stop_while_dragging_releases_the_button_and_drops_the_drag(self) -> None:
        seen: dict = {}

        def stop_mid_drag(world: SimWorld) -> None:
            mu = world.mu
            if not seen and mu.state == "drag_move" and mu.k == 3:
                seen["n"] = mu.pending["n"]
                world.stop()

        world = SimWorld(self.sm, self.script("um02")).go(3.0, stop_mid_drag)
        self.assertTrue(seen)
        self.assertFalse(world.held)
        self.assertFalse(world.mu.running)
        self.assertEqual(len(world.of("mu_stop")), 1)
        self.assertEqual([r for r in world.of("mu_click_result") if r["n"] == seen["n"]], [])
        self.assertEqual([d for d in world.of("mu_drag") if d["n"] == seen["n"]], [])
        self.assertEqual(world.of("mouseUp"), [])  # the release came after disarming: not a leak


class HardVariantLaunchersTest(unittest.TestCase):
    """PA-02 and UM-02 on the launchers and the validator, without a display."""

    def run_launcher(self, name: str, *argv: str) -> subprocess.CompletedProcess:
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE")
        }
        return subprocess.run(
            [sys.executable, str(BENCH / "launch" / name), *argv],
            capture_output=True, text=True, timeout=60, env=env,
        )  # fmt: skip

    def test_hyprland_rejects_the_hard_variants_as_a_harness_error(self) -> None:
        for task, condition in (("PA-02", "foreground"), ("UM-02", "background")):
            done = self.run_launcher(
                "hyprland_session.py", "--task", task, "--run-index", "0", "--condition", condition,
                "--out", tempfile.mkdtemp(prefix="cdb-hyprland-"), "--", "true",
            )  # fmt: skip
            self.assertEqual(done.returncode, 3, done.stderr)
            row = json.loads(done.stdout.strip().splitlines()[-1])
            self.assertIn("not supported on Hyprland", row["harness_error"])
            self.assertIsNone(row["passed"])  # never a fail

    def test_x11_validates_task_condition_and_instances_before_starting(self) -> None:
        out = tempfile.mkdtemp(prefix="cdb-x11-")
        for argv in (
            ["--task", "PA-02", "--instances", "17", "--condition", "foreground"],
            ["--task", "PA-02", "--instances", "0", "--condition", "foreground"],
            ["--task", "PA-01", "--instances", "11", "--condition", "foreground"],
            ["--task", "UM-02", "--condition", "foreground"],
            ["--task", "UM-02", "--instances", "2", "--condition", "background"],
        ):
            done = self.run_launcher(
                "x11_session.py", *argv, "--run-index", "0", "--out", out, "--", "true"
            )
            self.assertEqual(done.returncode, 2, (argv, done.stderr))
            self.assertIn("error", done.stderr)
        help_text = " ".join(self.run_launcher("x11_session.py", "-h", "--", "true").stdout.split())
        self.assertIn("PA-02 1..16", help_text)

    def test_x11_accepts_the_hard_tasks_and_the_variant_counts(self) -> None:
        for argv in (
            ["--task", "PA-02", "--instances", "16", "--condition", "foreground"],
            ["--task", "PA-02", "--instances", "8", "--condition", "background"],
            ["--task", "PA-02", "--instances", "1", "--condition", "background"],
            ["--task", "UM-02", "--condition", "background"],
        ):
            done = self.run_launcher(
                "x11_session.py", *argv, "--run-index", "0", "--out", tempfile.mkdtemp(prefix="cdb-x11-"), "--", "true"
            )  # fmt: skip
            self.assertEqual(
                done.returncode, 2, (argv, done.stderr)
            )  # past the parser: stops at "needs DISPLAY"
            self.assertIn("needs an X11 session", done.stderr)

    def test_x11_plans_the_crowded_desk_through_the_shared_helpers(self) -> None:
        for path in (BENCH / "probes" / "_common", BENCH / "swift", BENCH):
            if str(path) not in sys.path:
                sys.path.insert(0, str(path))
        import shared_desktop

        out = Path(tempfile.mkdtemp(prefix="cdb-pa2-"))
        agents = shared_desktop.pa_agents(
            7, 16, out, ("on your Mac", "on this Linux computer"), task="PA-02"
        )
        self.assertEqual(len(agents), 16)
        self.assertEqual({a["title"] for a in agents}, {"BenchLab Desk"})
        for a in agents:
            argv = a["app_args"]
            self.assertEqual(argv[argv.index("--title") + 1], "BenchLab Desk")
            self.assertIn("--confirm", argv)
        self.assertEqual(shared_desktop.min_screen("PA-02"), shared_desktop.C.PA2_MIN_SCREEN)
        self.assertEqual(shared_desktop.max_instances("PA-02"), 16)

    def test_validator_reads_the_hard_briefs(self) -> None:
        for path in (BENCH / "probes" / "_common", BENCH / "swift", BENCH):
            if str(path) not in sys.path:
                sys.path.insert(0, str(path))
        import shared_desktop

        validator = _load("validate_shared_linux")
        out = Path(tempfile.mkdtemp(prefix="cdb-brief-"))
        agents = shared_desktop.pa_agents(
            7, 16, out, ("on your Mac", "on this Linux computer"), task="PA-02"
        )
        import benchlab_common as C

        cells = C.derive_crowded(7, 16)["cells"]
        for agent, cell in zip(agents, cells):
            agent["dir"].mkdir(parents=True)
            brief = agent["dir"] / "brief.md"
            brief.write_text(agent["brief"], encoding="utf-8")
            values = validator.brief_values(brief)
            self.assertEqual(
                (values["title"], values["label"], values["code"], values["shelf"]),
                ("BenchLab Desk", cell["label"], cell["code"], cell["shelf"]),
            )
        um = BENCH / "probes" / "UM-02"
        render = importlib.util.spec_from_file_location("um02_render", um / "render_brief.py")
        module = importlib.util.module_from_spec(render)
        render.loader.exec_module(module)
        brief = out / "um02.md"
        brief.write_text(module.render(7), encoding="utf-8")
        desk = C.derive_busy_desk(7)
        values = validator.brief_values(brief)
        self.assertEqual(
            (values["title"], values["code"], values["shelf"]),
            (f"BenchLab Desk {desk['label']}", desk["code"], desk["shelf"]),
        )
