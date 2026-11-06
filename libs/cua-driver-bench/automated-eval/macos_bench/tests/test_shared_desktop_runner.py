# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""run_bench.run_attempt for PA-01 (SHARED-DESKTOP.md) with the agents faked: N agents start at the same
moment, each gets its own copy's prompt, and the trial row carries the parallel checker's verdict, cross-talk,
throughput and the agents' aggregate. A violation by one agent ends the trial. No GUI, no model."""

from __future__ import annotations

import argparse
import contextlib
import json
import re
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "probes" / "_common"))

import bench_core as core  # noqa: E402
import benchlab_common as C  # noqa: E402
import run_bench as rb  # noqa: E402

REAL_RUN = subprocess.run


def make_ctx(tmp: str, instances: int) -> rb.Ctx:
    args = argparse.Namespace(
        ledger_who="test", model="m", max_budget_usd=6.0, budget_cap_usd=None, seven_day_stop=0.95,
        smoke=False, phase1_runs=1, phase2_runs=0, schedule_seed=1, build_dir=tmp, offline=False,
        disk_floor_gb=rb.DISK_FLOOR_GB, timeout_s=60, max_turns=5, effort="low", idle_min=0,
        no_sentinel=True, check_occlusion=False, tell_budget=False, bench_v2=True, condition="background",
        gui_only_enforce=True, instances=instances, tool_search="default",
    )  # fmt: skip
    run_dir = Path(tmp) / "run"
    run_dir.mkdir()
    ctx = rb.Ctx(
        args, run_dir, core.Ledger(Path(tmp) / "spend.jsonl"), {}, ["PA-01"], ["cc-codex-cu"],
        full_task_ids=["PA-01"],
    )  # fmt: skip
    ctx.tasks = rb.load_tasks(HERE.parent / "probes")
    mcp = Path(tmp) / "mcp.json"
    mcp.write_text("{}")
    ctx.mcp["cc-codex-cu"] = (mcp, "server")
    return ctx


INIT = "0\t" + json.dumps({"type": "system", "subtype": "init", "tools": [], "model": "m"}) + "\n"


def done(cost: float | None = 0.1) -> str:
    result = {"type": "result", "subtype": "success", "result": "DONE", "is_error": False}
    if cost is not None:
        result["total_cost_usd"] = cost
    return INIT + "1\t" + json.dumps(result) + "\n"


def desk_logs(seed: int, label: str, code: str, shelf: str, root: Path, t: float) -> None:
    events = [
        {"type": "app_start", "details": {"mode": "desk", "seed": seed, "instance": label}},
        {"type": "field_edit", "details": {"field": "code", "value": code}},
        {"type": "shelf_select", "details": {"shelf": shelf}},
        {"type": "submit", "details": {"code": code, "shelf": shelf}},
    ]
    for k, e in enumerate(events, start=1):
        e.update(seq=k, t=t + 100 * k)
    state = {"mode": "desk", "seed": seed, "seq": len(events), "instance": label, "code": code,
             "shelf": shelf, "submit_count": 1, "submitted": {"code": code, "shelf": shelf}}  # fmt: skip
    d = root / f"desk-{label}"
    d.mkdir(parents=True, exist_ok=True)
    (d / "state.json").write_text(json.dumps(state))
    (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))


class ParallelRunAttempt(unittest.TestCase):
    def run_trial(self, n: int, behave) -> tuple[dict, list[str]]:
        entry = {
            "trial_id": "PA-01-r1-cc-codex-cu", "phase": 1, "block": "P1-PA-01", "run_index": 0, "run": 1,
            "task": "PA-01", "arm": "cc-codex-cu", "task_pos": 0, "arm_slot": 0, "first_arm": True,
            "schedule_seed": 1, "order_index": 0,
        }  # fmt: skip
        seed = core.probe_seed("PA-01", 0)
        cells = {c["label"]: c for c in C.derive_parallel(seed, n)["cells"]}
        prompts: list[str] = []
        barrier = threading.Barrier(n, timeout=10)
        with tempfile.TemporaryDirectory() as tmp, contextlib.ExitStack() as stack:
            ctx = make_ctx(tmp, n)
            artifacts = ctx.run_dir / "trials" / entry["trial_id"] / "a1" / "artifacts"

            def run(cmd, *a, **kw):
                if cmd and cmd[0] == sys.executable:
                    return REAL_RUN(cmd, *a, **kw)
                return subprocess.CompletedProcess(cmd, 1, b"", b"")  # pgrep: no side-door process

            def fake_claude(
                argv, env, cwd, prompt, allowed, timeout_s, stream, stderr, elic, abort=None
            ):
                prompts.append(prompt)
                barrier.wait()  # every agent is running at the same moment
                label = re.search(r'"BenchLab Desk ([A-J])"', prompt)[1]
                behave(label, cells, seed, artifacts, stream, abort)
                return {
                    "returncode": 0,
                    "timed_out": False,
                    "wall_s": 1.0,
                    "result_seen": True,
                    "elicitations": [],
                }

            for owner, name, result in (
                (rb.desktop, "capture_screenshot", None),
                (rb, "kill_bench_apps", None),
                (rb, "reset_needs_apps", None),
                (rb.pilot, "hid_idle_seconds", 1000),
                (rb.pilot, "sweep_processes", 0),
                (rb.ca, "prepare_cwd", Path(tmp)),
                (rb.ca, "claude_argv", ["claude"]),
                (rb.ca, "system_prompt_for", "SYSTEM"),
                (rb, "apps_running", {}),
                (rb, "launch_desks", []),
                (rb, "frontmost_bundle", None),
                (rb.v2, "terminal_tty_inputs", {}),
            ):
                stack.enter_context(mock.patch.object(owner, name, return_value=result))
            stack.enter_context(mock.patch.object(rb.subprocess, "run", side_effect=run))
            sentinel = stack.enter_context(mock.patch.object(rb.pilot, "Sentinel"))
            sentinel.return_value.enabled = False
            sentinel.return_value.summary.return_value = {}
            stack.enter_context(mock.patch.object(rb.pilot, "IdleSampler"))
            stack.enter_context(
                mock.patch.object(rb.claude_driver, "run_claude", side_effect=fake_claude)
            )
            row = rb.run_attempt(ctx, entry, 1)
        return row, prompts

    def test_all_agents_do_their_copy(self) -> None:
        def behave(label, cells, seed, artifacts, stream, abort):
            c = cells[label]
            desk_logs(seed, label, c["code"], c["shelf"], artifacts, time.time() * 1000)
            stream.write_text(done())

        row, prompts = self.run_trial(4, behave)
        self.assertEqual(len(prompts), 4)
        self.assertEqual(len({p for p in prompts}), 4, "each agent gets its own copy's brief")
        self.assertTrue(
            all("moving the pointer" not in p and "User activity" in p for p in prompts)
        )
        self.assertTrue(row["passed"], json.dumps(row.get("checks"))[:1500])
        self.assertEqual(row["instances"], 4)
        self.assertEqual((row["parallel"]["passed"], row["parallel"]["cross_talk"]), (4, 0))
        self.assertGreater(row["parallel"]["throughput_per_min"], 0)
        self.assertEqual(row["agents"]["agents"], 4)
        self.assertAlmostEqual(row["cost_usd"], 0.4)
        self.assertFalse(row["gui_only"]["violation"])

    def test_cross_talk_fails_the_trial(self) -> None:
        def behave(label, cells, seed, artifacts, stream, abort):
            labels = sorted(cells)
            c = cells[label]
            code = c["code"]
            if label == labels[0]:  # its field also received another agent's letters
                other = cells[labels[1]]
                code_events = [other["letters"], ""]
            else:
                code_events = []
            t = time.time() * 1000
            desk_logs(seed, label, code, c["shelf"], artifacts, t)
            if code_events:
                d = artifacts / f"desk-{label}"
                events = [json.loads(x) for x in (d / "events.jsonl").read_text().splitlines()]
                extra = [
                    {"type": "field_edit", "details": {"field": "code", "value": v}}
                    for v in code_events
                ]
                events = events[:1] + extra + events[1:]
                for k, e in enumerate(events, start=1):
                    e.update(seq=k, t=t + 100 * k)
                (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
                state = json.loads((d / "state.json").read_text())
                state["seq"] = len(events)
                (d / "state.json").write_text(json.dumps(state))
            stream.write_text(done())

        row, _ = self.run_trial(3, behave)
        self.assertFalse(row["passed"])
        self.assertEqual(row["parallel"]["cross_talk"], 1)
        self.assertEqual(row["parallel"]["passed"], 2)

    def test_one_agents_violation_ends_all(self) -> None:
        def behave(label, cells, seed, artifacts, stream, abort):
            c = cells[label]
            desk_logs(seed, label, c["code"], c["shelf"], artifacts, time.time() * 1000)
            if label == sorted(cells)[0]:
                use = {"type": "assistant", "message": {"content": [
                    {"type": "tool_use", "id": "t1", "name": "mcp__x__run", "input": {"code": "osascript -e 1"}}]}}  # fmt: skip
                stream.write_text(INIT + "1\t" + json.dumps(use) + "\n")
                deadline = time.monotonic() + 5
                while abort is not None and not abort.is_set() and time.monotonic() < deadline:
                    time.sleep(0.05)
            else:
                deadline = time.monotonic() + 5
                while abort is not None and not abort.is_set() and time.monotonic() < deadline:
                    time.sleep(0.05)  # a long-running agent: ended by the other agent's violation
                stream.write_text(done(None))

        started = time.monotonic()
        row, _ = self.run_trial(2, behave)
        self.assertLess(time.monotonic() - started, 5, "the violation stopped every agent early")
        self.assertTrue(row["gui_only"]["violation"])
        self.assertTrue(row["gui_only"]["stopped_trial"])
        self.assertFalse(row["passed"])
        self.assertEqual(row["status"], "gui_only_violation")


if __name__ == "__main__":
    unittest.main()
