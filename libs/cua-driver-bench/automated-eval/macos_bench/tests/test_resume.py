# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Kill a short run at several points, then resume it with the same command. The runner is the real one in a
child process (tests/fake_runner.py stubs only the GUI, daemons and model). After the resume every scheduled
trial has exactly one final row, every row written before the kill is kept, and only the cut-short attempt is
redone. No GUI, no model."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import bench_core as core  # noqa: E402

FAKE = HERE / "fake_runner.py"
ARMS = ["pi-cua-driver", "pi-cua-driver-main"]
TASKS = ["MB-09", "IR-01"]
TRIALS = {f"{t}-r{r}-{a}" for t in TASKS for r in (1, 2) for a in ARMS}  # 2 tasks x 2 runs x 2 arms
FIRST = "MB-09-r1-pi-cua-driver"  # run 0 of the task at position 0 starts with arms[0]


class Run:
    def __init__(self, root: Path, **env: str) -> None:
        self.root = root
        self.dir = root / "runs" / "R"
        self.env = {**os.environ, "CDB_BENCH_WORK": str(root / "work"), **env}

    def argv(self, *extra: str) -> list[str]:
        return [
            sys.executable, str(FAKE), "run", "--arms", *ARMS, "--tasks", *TASKS,
            "--phase1-runs", "2", "--phase2-runs", "0", "--runs-dir", str(self.root / "runs"),
            "--run-id", "R", "--ledger", str(self.root / "spend.jsonl"), "--allow-unknown-quota",
            "--disk-floor-gb", "0.001", "--no-recorder", *extra,
        ]  # fmt: skip

    def start(self, *extra: str, **env: str) -> subprocess.Popen:
        log = (self.root / "out.log").open("a")
        return subprocess.Popen(
            self.argv(*extra), env={**self.env, **env}, stdout=log, stderr=subprocess.STDOUT
        )

    def run(self, *extra: str, **env: str) -> int:
        proc = self.start(*extra, **env)
        try:
            return proc.wait(timeout=120)
        finally:
            if proc.poll() is None:
                proc.kill()

    def rows(self) -> list[dict]:
        return core.load_rows(self.dir / "results.jsonl")


def keys(rows: list[dict]) -> list[tuple[str, int]]:
    return [(r["trial_id"], r["attempt"]) for r in rows]


class KillAndResumeTest(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)

    def assert_complete(self, run: Run, kept: list[dict]) -> list[dict]:
        rows = run.rows()
        self.assertTrue((run.dir / "DONE").exists(), (run.root / "out.log").read_text()[-2000:])
        self.assertEqual(len(keys(rows)), len(set(keys(rows))), "an attempt was recorded twice")
        self.assertEqual({r["trial_id"] for r in rows if r["final"]}, TRIALS)
        self.assertEqual(sum(1 for r in rows if r["final"]), len(TRIALS))
        after = {(r["trial_id"], r["attempt"]): r for r in rows}
        # every row from before the kill survives; only finalize's block flag may move
        for row in kept:
            same = {k: v for k, v in row.items() if k != "block_incomplete"}
            self.assertEqual({k: after[(row["trial_id"], row["attempt"])][k] for k in same}, same)
        return rows

    def test_sigkill_mid_attempt_redoes_only_that_attempt(self) -> None:
        # The CUA-3543 case: the host died with an attempt directory written and no row.
        run = Run(self.root)
        victim = "IR-01-r1-pi-cua-driver-main"
        self.assertEqual(run.run(FAKE_KILL=f"{victim}:1:agent"), -signal.SIGKILL)
        before = run.rows()
        self.assertEqual(len(before), 4)  # the MB-09 block, nothing from the victim
        self.assertTrue((run.dir / "trials" / victim / "a1" / "artifacts" / "started").exists())
        self.assertEqual(run.run(), 0)
        rows = self.assert_complete(run, before)
        self.assertEqual(len(rows), len(TRIALS))
        redone = [r for r in rows if r["trial_id"] == victim]
        self.assertEqual([r["attempt"] for r in redone], [1])
        self.assertNotEqual(redone[0]["writer_pid"], before[0]["writer_pid"])

    def test_sigkill_mid_row_write_drops_the_torn_line(self) -> None:
        run = Run(self.root)
        self.assertEqual(run.run(FAKE_KILL=f"{FIRST}:1:append"), -signal.SIGKILL)
        text = (run.dir / "results.jsonl").read_text()
        self.assertTrue(text and not text.endswith("\n"))
        self.assertEqual(run.run(), 0)
        rows = self.assert_complete(run, [])
        self.assertEqual(len(rows), len(TRIALS))

    def test_sigkill_during_a_retry_keeps_the_recorded_attempt_and_its_budget(self) -> None:
        # a1 failed with mcp_start (non-final, recorded); the runner died in the a2 retry. The resume runs a2
        # (not a fresh a1), and since the one infra retry is spent, a second mcp_start excludes the trial.
        plan = json.dumps({FIRST: ["mcp_start", "mcp_start", None]})
        run = Run(self.root, FAKE_PLAN=plan)
        self.assertEqual(run.run(FAKE_KILL=f"{FIRST}:2:agent"), -signal.SIGKILL)
        before = run.rows()
        self.assertEqual(keys(before), [(FIRST, 1)])
        self.assertEqual(run.run(), 0)
        rows = self.assert_complete(run, before)
        mine = [r for r in rows if r["trial_id"] == FIRST]
        self.assertEqual([(r["attempt"], r["final"]) for r in mine], [(1, False), (2, True)])
        self.assertTrue(mine[-1]["excluded"])

    def test_sigterm_stops_cleanly_and_resumes(self) -> None:
        run = Run(self.root, FAKE_SLEEP="0.4")
        proc = run.start()
        deadline = time.monotonic() + 60
        while len(run.rows()) < 2 and time.monotonic() < deadline:
            time.sleep(0.05)
        proc.send_signal(signal.SIGTERM)
        self.assertEqual(proc.wait(timeout=60), 0)
        self.assertTrue((run.dir / "STOPPED_USER").exists())
        before = run.rows()
        self.assertLess(len(before), len(TRIALS))
        self.assertEqual(run.run(), 0)
        self.assert_complete(run, before)
        self.assertFalse((run.dir / "STOPPED_USER").exists())

    def test_external_sigkill_at_any_point_then_one_resume(self) -> None:
        # Kill from outside at fixed delays across the run (setup, mid-attempt, between rows), resume once each.
        for delay in (0.0, 0.5, 0.9, 1.3, 1.7):
            with self.subTest(delay=delay):
                root = self.root / f"d{delay}"
                root.mkdir()
                run = Run(root, FAKE_SLEEP="0.3")  # 8 trials: every delay lands before DONE
                proc = run.start()
                time.sleep(delay)
                proc.kill()
                proc.wait(timeout=30)
                self.assertFalse((run.dir / "DONE").exists())
                before = run.rows()
                self.assertEqual(run.run(), 0)
                self.assert_complete(run, before)

    def test_smoke_run_resumes_like_a_real_one(self) -> None:
        run = Run(self.root)
        victim = f"SMOKE-IR-01-r1-{ARMS[1]}"
        self.assertEqual(run.run("--smoke", FAKE_KILL=f"{victim}:1:agent"), -signal.SIGKILL)
        before = run.rows()
        self.assertEqual(len(before), 4)
        self.assertEqual(run.run("--smoke"), 0)
        rows = run.rows()
        self.assertEqual(keys(rows)[:4], keys(before))
        self.assertEqual(len(rows), len(TRIALS))
        self.assertEqual({r["trial_id"] for r in rows}, {f"SMOKE-{t}" for t in TRIALS})
        self.assertEqual(run.run(), 6)  # smoke rows are not a real run's rows

    def test_preflight_never_replaces_a_runs_record(self) -> None:
        run = Run(self.root)
        self.assertEqual(
            run.run(FAKE_KILL=f"{FIRST}:2:agent", FAKE_PLAN=json.dumps({FIRST: ["mcp_start"]})), -9
        )
        manifest = (run.dir / "manifest.json").read_bytes()
        argv = run.argv("--no-gui-only-enforce")
        argv[argv.index("run")] = "preflight"
        done = subprocess.run(argv, env=run.env, capture_output=True, text=True, timeout=120)
        self.assertEqual(done.returncode, 2, done.stderr[-500:])
        self.assertEqual((run.dir / "manifest.json").read_bytes(), manifest)

    def test_a_video_left_without_a_record_withholds_done(self) -> None:
        run = Run(self.root, FAKE_VIDEO_FAILS="1")
        self.assertEqual(run.run(), 0)
        self.assertFalse((run.dir / "DONE").exists())
        self.assertTrue((run.dir / "STOPPED_COMPRESS").exists())

    def test_second_start_while_running_is_refused(self) -> None:
        run = Run(self.root, FAKE_SLEEP="2")
        first = run.start()
        try:
            deadline = time.monotonic() + 60
            while not (run.dir / "heartbeat.json").exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertEqual(run.run(), 5)
        finally:
            first.kill()
            first.wait(timeout=30)

    def test_resume_that_would_reschedule_a_recorded_trial_is_refused_and_changes_nothing(
        self,
    ) -> None:
        run = Run(self.root)
        # Killed while writing the first IR-01 run 2 row: six rows recorded, then a torn line.
        self.assertEqual(run.run(FAKE_KILL=f"IR-01-r2-{ARMS[0]}:1:append"), -signal.SIGKILL)
        self.assertEqual(len(run.rows()), 6)
        state = {p.name: p.read_bytes() for p in run.dir.iterdir() if p.is_file()}
        changes = {
            "arm order": (["--arms", *reversed(ARMS)], {}),
            "agent model": ([], {"CDB_PI_MODEL": "another-model"}),
            "run 2 moved to phase 2": (["--phase1-runs", "1", "--phase2-runs", "1"], {}),
            "phase 1 grown": (["--phase1-runs", "3"], {}),  # recorded IR-01 run 1 moves later
            "task dropped": (["--tasks", "IR-01"], {}),
            "turn cap": (["--max-turns", "5"], {}),
            "time limit": (["--timeout-s", "60"], {}),
            "guard only records": (["--no-gui-only-enforce"], {}),
            "budget not told": (["--no-tell-budget"], {}),
        }
        for name, (argv, env) in changes.items():
            with self.subTest(name):
                self.assertEqual(run.run(*argv, **env), 6)
                now = {p.name: p.read_bytes() for p in run.dir.iterdir() if p.is_file()}
                self.assertEqual(now, state)
        legacy = (run.dir / "results.jsonl").read_text()
        rows = [json.loads(line) for line in legacy.splitlines()[:-1]]
        stripped = "".join(json.dumps({**r, "order_index": None}) + "\n" for r in rows)
        (run.dir / "results.jsonl").write_text(stripped + legacy.splitlines(True)[-1])
        self.assertEqual(
            run.run(), 6
        )  # a row that cannot show its slot is not resumed into a schedule
        (run.dir / "results.jsonl").write_text(legacy)
        recorded = (run.dir / "results.jsonl").read_bytes()
        other = [
            {**r, "bench_src_sha256": "0" * 64} for r in run.rows()
        ]  # another runner or task source
        (run.dir / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in other))
        self.assertEqual(run.run(), 6)
        (run.dir / "results.jsonl").write_bytes(recorded)
        self.assertEqual(run.run("--phase2-runs", "1"), 0)  # extending the schedule is a resume
        self.assertEqual(sum(1 for r in run.rows() if r["final"]), len(TRIALS) + 4)


if __name__ == "__main__":
    unittest.main()
