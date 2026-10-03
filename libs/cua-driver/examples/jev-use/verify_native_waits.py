#!/usr/bin/env python3
"""Fake-clock comparison of baseline vs candidate native verification waits.

Loads the immutable baseline ``poll_oracle`` by compiling the function AST from
``git show <sha>:libs/cua-driver/examples/jev-use/python/run_native.py`` (cwd is
the repository root). Loads the candidate poller from this checkout's production
``run_native.poll_oracle``. Does not reimplement either poller.

Nine delayed-effect cells (counter, choose-size, save-note) must all terminate
verified on both pollers. Printed timings are discrete fake-clock policy
latency, not a live, driver, or model benchmark.
"""

from __future__ import annotations

import argparse
import ast
import asyncio
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any, Mapping
from unittest.mock import patch

PACKAGE_ROOT = Path(__file__).resolve().parent
REPO_ROOT = PACKAGE_ROOT.parents[3]
JEV_PYTHON = PACKAGE_ROOT / "python"
DEFAULT_BASELINE_SHA = "62cdfbcd9356cf2e1b47ab09fee94eddd1d881cc"
BASELINE_POLL_PATH = "libs/cua-driver/examples/jev-use/python/run_native.py"
EXPECTED_CELLS = 9
EXPECTED_TERMINAL_VERIFIED = 18
EXPECTED_BASELINE_MS = 25500.0
EXPECTED_CANDIDATE_MS = 9300.0
EXPECTED_CELL_MS = {
    "counter_immediate": (4000.0, 0.0),
    "counter_delayed_150ms": (4200.0, 600.0),
    "counter_delayed_350ms": (4300.0, 1100.0),
    "choose_size_immediate": (2000.0, 0.0),
    "choose_size_delayed_150ms": (2200.0, 400.0),
    "choose_size_delayed_350ms": (2300.0, 700.0),
    "save_note_immediate": (2000.0, 2000.0),
    "save_note_delayed_150ms": (2200.0, 2200.0),
    "save_note_delayed_350ms": (2300.0, 2300.0),
}

sys.path.insert(0, str(JEV_PYTHON))

from core import Candidate
from native_tasks import NativeTask, appkit_task
from run_native import poll_oracle as candidate_poll_oracle


def load_baseline_poll_oracle(baseline_sha: str):
    """Extract and compile baseline poll_oracle from git show at baseline_sha."""
    raw_source = subprocess.check_output(
        ["git", "show", f"{baseline_sha}:{BASELINE_POLL_PATH}"],
        cwd=REPO_ROOT,
        text=True,
    )
    tree = ast.parse(raw_source)
    func_node = next(
        node
        for node in tree.body
        if isinstance(node, ast.AsyncFunctionDef) and node.name == "poll_oracle"
    )
    module = ast.Module(body=[func_node], type_ignores=[])
    code = compile(module, filename=f"<git_show_{baseline_sha}_poll_oracle>", mode="exec")
    namespace: dict[str, Any] = {"asyncio": asyncio}
    exec(code, namespace)
    source_bytes = ast.unparse(func_node).encode("utf-8")
    return namespace["poll_oracle"], hashlib.sha256(source_bytes).hexdigest()


class FakeClock:
    def __init__(self) -> None:
        self.time_ms = 0.0

    async def sleep(self, seconds: float) -> None:
        self.time_ms += seconds * 1000.0


class SimulatedAppOracle:
    def __init__(self, initial_state: dict[str, Any], clock: FakeClock) -> None:
        self.clock = clock
        self.state = dict(initial_state)
        self.scheduled_transitions: list[tuple[float, dict[str, Any]]] = []

    def set_delayed(self, delay_ms: float, updates: dict[str, Any]) -> None:
        ready_time = self.clock.time_ms + delay_ms
        self.scheduled_transitions.append((ready_time, updates))

    def read(self) -> dict[str, Any]:
        still_pending = []
        for ready_time, updates in self.scheduled_transitions:
            if self.clock.time_ms >= ready_time:
                self.state.update(updates)
            else:
                still_pending.append((ready_time, updates))
        self.scheduled_transitions = still_pending
        return dict(self.state)


def build_simulated_task(kind: str, oracle: SimulatedAppOracle) -> NativeTask:
    task = appkit_task(f"appkit-{kind}", Path("unused-oracle.json"), pid=42, note_text="test note")
    return NativeTask(
        id=task.id,
        goal=task.goal,
        scope=task.scope,
        allowed_actions=task.allowed_actions,
        oracle=oracle,
        check=task.check,
        parameters=task.parameters,
        allowed_risks=task.allowed_risks,
        allow_foreground=task.allow_foreground,
        max_steps=task.max_steps,
        text_method=task.text_method,
        visual_targets=task.visual_targets,
        visual_min_confidence=task.visual_min_confidence,
        mock_preferences=task.mock_preferences,
        steps=task.steps,
        cap_order=task.cap_order,
        intermediate_effect=task.intermediate_effect,
    )


async def execute_trial(
    kind: str,
    intermediate_delay_ms: float,
    terminal_delay_ms: float,
    clock: FakeClock,
    use_candidate: bool,
    baseline_poll_oracle,
) -> dict[str, Any]:
    with patch("asyncio.sleep", side_effect=clock.sleep):
        return await _execute_trial_inner(
            kind,
            intermediate_delay_ms,
            terminal_delay_ms,
            clock,
            use_candidate,
            baseline_poll_oracle,
        )


async def _execute_trial_inner(
    kind: str,
    intermediate_delay_ms: float,
    terminal_delay_ms: float,
    clock: FakeClock,
    use_candidate: bool,
    baseline_poll_oracle,
) -> dict[str, Any]:
    start_time = clock.time_ms
    step_latencies: list[dict[str, Any]] = []

    if kind == "counter":
        oracle = SimulatedAppOracle({"schema": "cua.appkit_task_state_v1", "pid": 42, "counter": 0}, clock)
        task = build_simulated_task("counter", oracle)
        cand = Candidate("ax:button:increment", "ax", "press", {"pid": 42})
        history: list[Mapping[str, Any]] = []

        for step in (1, 2, 3):
            pre_oracle = oracle.read()
            step_start = clock.time_ms
            is_terminal = step == 3
            delay = terminal_delay_ms if is_terminal else intermediate_delay_ms
            oracle.set_delayed(delay, {"counter": step})
            history.append(task.history_entry(step, cand.id))

            if use_candidate:
                res = await candidate_poll_oracle(
                    task, step, candidate=cand, pre_oracle=pre_oracle, history=history
                )
                outcome = res.outcome
                status = res.status
            else:
                outcome = await baseline_poll_oracle(task, step)
                status = outcome

            step_latencies.append(
                {
                    "step": step,
                    "is_terminal": is_terminal,
                    "outcome": outcome,
                    "status": status,
                    "virtual_latency_ms": clock.time_ms - step_start,
                }
            )
            if outcome in {"verified", "refuted"}:
                break

    elif kind == "choose-size":
        oracle = SimulatedAppOracle(
            {"schema": "cua.appkit_task_state_v1", "pid": 42, "size": "", "agreed": False},
            clock,
        )
        task = build_simulated_task("choose-size", oracle)
        cand1 = Candidate("ax:radio:large", "ax", "press", {"pid": 42})
        cand2 = Candidate("ax:checkbox:i-agree", "ax", "press", {"pid": 42})
        history = []

        pre1 = oracle.read()
        step1_start = clock.time_ms
        oracle.set_delayed(intermediate_delay_ms, {"size": "large"})
        history.append(task.history_entry(1, cand1.id))
        if use_candidate:
            res1 = await candidate_poll_oracle(
                task, 1, candidate=cand1, pre_oracle=pre1, history=history
            )
            outcome1, status1 = res1.outcome, res1.status
        else:
            outcome1 = await baseline_poll_oracle(task, 1)
            status1 = outcome1
        step_latencies.append(
            {
                "step": 1,
                "is_terminal": False,
                "outcome": outcome1,
                "status": status1,
                "virtual_latency_ms": clock.time_ms - step1_start,
            }
        )

        pre2 = oracle.read()
        step2_start = clock.time_ms
        oracle.set_delayed(terminal_delay_ms, {"agreed": True})
        history.append(task.history_entry(2, cand2.id))
        if use_candidate:
            res2 = await candidate_poll_oracle(
                task, 2, candidate=cand2, pre_oracle=pre2, history=history
            )
            outcome2, status2 = res2.outcome, res2.status
        else:
            outcome2 = await baseline_poll_oracle(task, 2)
            status2 = outcome2
        step_latencies.append(
            {
                "step": 2,
                "is_terminal": True,
                "outcome": outcome2,
                "status": status2,
                "virtual_latency_ms": clock.time_ms - step2_start,
            }
        )

    elif kind == "save-note":
        oracle = SimulatedAppOracle(
            {"schema": "cua.appkit_task_state_v1", "pid": 42, "note_saved": None},
            clock,
        )
        task = build_simulated_task("save-note", oracle)
        cand1 = Candidate("ax:text_input:note:set:note", "ax", "set_text", {"pid": 42})
        cand2 = Candidate("ax:button:save-note", "ax", "press", {"pid": 42})
        history = []

        pre1 = oracle.read()
        step1_start = clock.time_ms
        history.append(task.history_entry(1, cand1.id))
        if use_candidate:
            res1 = await candidate_poll_oracle(
                task, 1, candidate=cand1, pre_oracle=pre1, history=history
            )
            outcome1, status1 = res1.outcome, res1.status
        else:
            outcome1 = await baseline_poll_oracle(task, 1)
            status1 = outcome1
        step_latencies.append(
            {
                "step": 1,
                "is_terminal": False,
                "outcome": outcome1,
                "status": status1,
                "virtual_latency_ms": clock.time_ms - step1_start,
            }
        )

        pre2 = oracle.read()
        step2_start = clock.time_ms
        oracle.set_delayed(terminal_delay_ms, {"note_saved": "test note"})
        history.append(task.history_entry(2, cand2.id))
        if use_candidate:
            res2 = await candidate_poll_oracle(
                task, 2, candidate=cand2, pre_oracle=pre2, history=history
            )
            outcome2, status2 = res2.outcome, res2.status
        else:
            outcome2 = await baseline_poll_oracle(task, 2)
            status2 = outcome2
        step_latencies.append(
            {
                "step": 2,
                "is_terminal": True,
                "outcome": outcome2,
                "status": status2,
                "virtual_latency_ms": clock.time_ms - step2_start,
            }
        )
    else:
        raise ValueError(f"Unknown kind {kind}")

    total_elapsed = clock.time_ms - start_time
    terminal_outcome = step_latencies[-1]["outcome"]
    if terminal_outcome != "verified":
        raise AssertionError(f"{kind} terminal outcome {terminal_outcome!r} is not verified")

    return {
        "task_kind": kind,
        "terminal_outcome": terminal_outcome,
        "total_virtual_latency_ms": total_elapsed,
        "steps": step_latencies,
    }


def run_corpus(baseline_poll_oracle) -> list[dict[str, Any]]:
    trial_configs = [
        {"kind": "counter", "trial_id": "counter_immediate", "int_delay": 0.0, "term_delay": 0.0},
        {"kind": "counter", "trial_id": "counter_delayed_150ms", "int_delay": 150.0, "term_delay": 150.0},
        {"kind": "counter", "trial_id": "counter_delayed_350ms", "int_delay": 350.0, "term_delay": 250.0},
        {"kind": "choose-size", "trial_id": "choose_size_immediate", "int_delay": 0.0, "term_delay": 0.0},
        {"kind": "choose-size", "trial_id": "choose_size_delayed_150ms", "int_delay": 150.0, "term_delay": 150.0},
        {"kind": "choose-size", "trial_id": "choose_size_delayed_350ms", "int_delay": 350.0, "term_delay": 250.0},
        {"kind": "save-note", "trial_id": "save_note_immediate", "int_delay": 0.0, "term_delay": 0.0},
        {"kind": "save-note", "trial_id": "save_note_delayed_150ms", "int_delay": 150.0, "term_delay": 150.0},
        {"kind": "save-note", "trial_id": "save_note_delayed_350ms", "int_delay": 350.0, "term_delay": 250.0},
    ]
    all_trials = []
    for cfg in trial_configs:
        base_res = asyncio.run(
            execute_trial(
                cfg["kind"],
                cfg["int_delay"],
                cfg["term_delay"],
                FakeClock(),
                use_candidate=False,
                baseline_poll_oracle=baseline_poll_oracle,
            )
        )
        cand_res = asyncio.run(
            execute_trial(
                cfg["kind"],
                cfg["int_delay"],
                cfg["term_delay"],
                FakeClock(),
                use_candidate=True,
                baseline_poll_oracle=baseline_poll_oracle,
            )
        )
        all_trials.append(
            {
                "trial_id": cfg["trial_id"],
                "task_kind": cfg["kind"],
                "configured_delays_ms": {
                    "intermediate_delay_ms": cfg["int_delay"],
                    "terminal_delay_ms": cfg["term_delay"],
                },
                "baseline": base_res,
                "candidate": cand_res,
                "virtual_latency_difference_ms": (
                    base_res["total_virtual_latency_ms"] - cand_res["total_virtual_latency_ms"]
                ),
            }
        )
    return all_trials


def assert_results(trials: list[dict[str, Any]]) -> dict[str, Any]:
    if len(trials) != EXPECTED_CELLS:
        raise AssertionError(f"expected {EXPECTED_CELLS} cells, got {len(trials)}")
    verified = 0
    baseline_ms = 0.0
    candidate_ms = 0.0
    for trial in trials:
        expected = EXPECTED_CELL_MS[trial["trial_id"]]
        base = trial["baseline"]
        cand = trial["candidate"]
        if base["terminal_outcome"] == "verified":
            verified += 1
        if cand["terminal_outcome"] == "verified":
            verified += 1
        if base["total_virtual_latency_ms"] != expected[0]:
            raise AssertionError(
                f"{trial['trial_id']} baseline {base['total_virtual_latency_ms']} != {expected[0]}"
            )
        if cand["total_virtual_latency_ms"] != expected[1]:
            raise AssertionError(
                f"{trial['trial_id']} candidate {cand['total_virtual_latency_ms']} != {expected[1]}"
            )
        baseline_ms += base["total_virtual_latency_ms"]
        candidate_ms += cand["total_virtual_latency_ms"]
    if verified != EXPECTED_TERMINAL_VERIFIED:
        raise AssertionError(f"expected {EXPECTED_TERMINAL_VERIFIED} verified terminals, got {verified}")
    if baseline_ms != EXPECTED_BASELINE_MS:
        raise AssertionError(f"baseline virtual total {baseline_ms} != {EXPECTED_BASELINE_MS}")
    if candidate_ms != EXPECTED_CANDIDATE_MS:
        raise AssertionError(f"candidate virtual total {candidate_ms} != {EXPECTED_CANDIDATE_MS}")
    return {
        "cells": len(trials),
        "terminal_verified": verified,
        "baseline_virtual_ms": baseline_ms,
        "candidate_virtual_ms": candidate_ms,
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--baseline-sha",
        default=DEFAULT_BASELINE_SHA,
        help="immutable Git SHA used for git show of run_native.py (default: %(default)s)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="write JSON to this path; default prints JSON to stdout and never writes source",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    baseline_poll_oracle, baseline_fn_sha256 = load_baseline_poll_oracle(args.baseline_sha)
    trials = run_corpus(baseline_poll_oracle)
    totals = assert_results(trials)
    report = {
        "authority": (
            f"AST compiled baseline poll_oracle from git show {args.baseline_sha} "
            "vs production candidate poll_oracle"
        ),
        "disclaimer": (
            "JSON timings are virtual policy latency under simulated discrete fake-clock "
            "steps, not live speedup or a production end-to-end or model benchmark."
        ),
        "baseline_sha": args.baseline_sha,
        "baseline_poll_path": BASELINE_POLL_PATH,
        "baseline_poll_oracle_sha256": baseline_fn_sha256,
        "trial_count": len(trials),
        "assertions": totals,
        "trials": trials,
    }
    text = json.dumps(report, indent=2) + "\n"
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
