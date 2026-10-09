"""Native step timing fields, Python runner (kvnloo/cua#75).

Runs ``run_native.py`` against ``fake_native_driver.py`` and checks the common timing fields
against the shared contract in ``fixtures/native/timing-contract-v1.json``. The TypeScript
runner is held to the same contract by ``typescript/native_timing.test.ts``.

The driver is scripted, so nothing here measures Driver speed: assertions are lower bounds from
the injected delays, alias equalities, and the nesting of phase intervals within a step.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = json.loads((ROOT / "fixtures/native/timing-contract-v1.json").read_text(encoding="utf-8"))
TOLERANCE = CONTRACT["tolerance_ms"]


def run_scenario(name: str, directory: Path) -> tuple[list[dict], list[str]]:
    scenario = CONTRACT["scenarios"][name]
    state, log, calls = directory / "state.json", directory / "events.jsonl", directory / "calls.txt"
    env = {
        **os.environ,
        "CUA_DRIVER_BIN": str(ROOT / "fake_native_driver.py"),
        "CUA_DRIVER_FAKE_SCENARIO": name,
        "CUA_DRIVER_FAKE_STATE_FILE": str(state),
        "CUA_DRIVER_FAKE_PID": str(scenario["pid"]),
        "CUA_DRIVER_FAKE_DELAYS_MS": json.dumps(CONTRACT["delays_ms"]),
        "CUA_DRIVER_FAKE_CALLS_FILE": str(calls),
    }
    result = subprocess.run(
        [sys.executable, str(ROOT / "python/run_native.py"), "--task", scenario["task"],
         "--pid", str(scenario["pid"]), "--state-file", str(state), "--provider", "mock",
         "--platform", scenario["platform"], "--log", str(log)],
        cwd=ROOT, env=env, capture_output=True, text=True, timeout=120,
    )
    if result.returncode != 0:
        raise AssertionError(f"run_native.py exited {result.returncode}: {result.stderr[-600:]}")
    events = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
    return events, calls.read_text(encoding="utf-8").split()


@unittest.skipIf(sys.platform == "win32", "the scripted driver is a POSIX executable script")
class NativeStepTimingTest(unittest.TestCase):
    def check_scenario(self, name: str) -> None:
        scenario = CONTRACT["scenarios"][name]
        delays = CONTRACT["delays_ms"]
        with tempfile.TemporaryDirectory() as directory:
            events, calls = run_scenario(name, Path(directory))
        steps = [event for event in events if event["event"] == "step"]
        self.assertEqual(events[-1]["outcome"], "verified")
        self.assertEqual(len(steps), scenario["steps"])
        # Timing must not change what the runner asks the Driver to do.
        self.assertEqual(calls, scenario["driver_calls"])

        for step in steps:
            for field in CONTRACT["common_fields"]:
                self.assertIsInstance(step[field], (int, float), field)
                self.assertGreaterEqual(step[field], 0, field)
            self.assertEqual(step["visual_observe_scope"], CONTRACT["visual_observe_scope"])
            # Legacy names are still emitted, in their original places.
            self.assertIsInstance(step["decide_ms"], (int, float))
            self.assertIsInstance(step["act_ms"], (int, float))
            self.assertIsInstance(step["observation"]["observe_ms"], (int, float))
            # Common names map onto the native ones.
            for common, legacy in CONTRACT["aliases"].items():
                self.assertEqual(step[common], step[legacy], f"{common} != {legacy}")

            observe_calls = 2 if step["observation"]["reobserved"] else 1
            self.assertGreaterEqual(step["semantic_observe_ms"] + TOLERANCE, delays["observe"] * observe_calls)
            self.assertGreaterEqual(step["action_ms"] + TOLERANCE, delays["act"])
            # observe_ms spans the semantic observation plus the source/plan work in observe_step.
            self.assertGreaterEqual(step["observation"]["observe_ms"] + TOLERANCE, step["semantic_observe_ms"])

            if step["visual"]["status"] == "ok":
                self.assertEqual(step["visual_observe_ms"], step["visual"]["parse_ms"])
                self.assertGreaterEqual(step["visual_observe_ms"] + TOLERANCE, delays["parse"])
            else:
                self.assertEqual(step["visual_observe_ms"], 0)
            self.assertEqual(step["visual"]["status"], scenario["visual_status"])

            named = (step["semantic_observe_ms"] + step["visual_observe_ms"]
                     + step["candidate_build_ms"] + step["provider_decision_ms"])
            self.assertGreaterEqual(step["decision_ms"] + TOLERANCE, named)
            self.assertGreaterEqual(step["total_step_ms"] + TOLERANCE, step["decision_ms"] + step["action_ms"])

    def test_form_task_reports_common_fields_without_a_visual_parse(self) -> None:
        self.check_scenario("counter")

    def test_visual_fallback_reports_parse_only_visual_time_and_both_observations(self) -> None:
        self.check_scenario("canvas")


if __name__ == "__main__":
    unittest.main()