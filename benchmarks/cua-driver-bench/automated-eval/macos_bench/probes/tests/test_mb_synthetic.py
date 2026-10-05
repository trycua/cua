"""Synthetic (no GUI, no app) checks of the MB-07 and MB-12 evaluators, which were not run against the live app.

MB-12 is exercised with synthetic sentinel summaries (clean, focus change, pointer moved, leaked key, unavailable).
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "_common"))
import benchlab_common as C  # noqa: E402
import synth  # noqa: E402

SEED = 4242


def run(probe: str, mode: str, state, events, sentinel=None):
    module = synth.load_module(probe, "evaluate.py")
    return C.evaluate_inputs(
        mode,
        SEED,
        state,
        events,
        0,
        module.evaluate,
        extra={"sentinel": sentinel} if sentinel is not None else {},
    )


def clean_sentinel(**over):
    s = {
        "available": True,
        "front_changes": 0,
        "front_changed_to": [],
        "key_loss": 0,
        "keystrokes_leaked": 0,
        "clicks_leaked": 0,
        "scrolls_leaked": 0,
        "pointer_max_deviation_px": 0.0,
        "pointer_deviation_episodes": 0,
        "hid_events": {"move": 0, "down": 0, "key": 0, "scroll": 0},
    }
    s.update(over)
    return s


class Mb07(unittest.TestCase):
    def test_pass_and_fail(self) -> None:
        state, events = synth.clipboard_run(SEED)
        self.assertTrue(run("MB-07", "clipboard", state, events)["passed"])
        exp = C.derive_clipboard(SEED)["result"]
        state, events = synth.clipboard_run(SEED, typed=str(exp + 1))
        res = run("MB-07", "clipboard", state, events)
        self.assertFalse(res["passed"])
        self.assertFalse(res["checks"]["result_correct"]["passed"])
        self.assertIn("pasteboard_changes", res["diagnostics"])


class Mb12(unittest.TestCase):
    def test_clean_background_run_passes(self) -> None:
        state, events = synth.forms_run(SEED)
        self.assertTrue(run("MB-12", "forms", state, events, clean_sentinel())["passed"])

    def test_disturbance_fails_but_task_checks_pass(self) -> None:
        state, events = synth.forms_run(SEED)
        for over, failed in (
            ({"front_changes": 1, "front_changed_to": ["ai.cua.benchlab"]}, "front_unchanged"),
            ({"key_loss": 1}, "front_unchanged"),
            ({"pointer_max_deviation_px": 40.0}, "pointer_still"),
            ({"keystrokes_leaked": 3}, "no_leaked_input"),
            ({"clicks_leaked": 1}, "no_leaked_input"),
        ):
            res = run("MB-12", "forms", state, events, clean_sentinel(**over))
            self.assertFalse(res["passed"], over)
            self.assertFalse(res["checks"][failed]["passed"], over)
            for task_check in ("customer_name", "category", "priority", "submitted_once"):
                self.assertTrue(res["checks"][task_check]["passed"], (over, task_check))

    def test_pointer_limit_is_five_pixels(self) -> None:
        state, events = synth.forms_run(SEED)
        self.assertTrue(
            run("MB-12", "forms", state, events, clean_sentinel(pointer_max_deviation_px=5.0))[
                "passed"
            ]
        )
        self.assertFalse(
            run("MB-12", "forms", state, events, clean_sentinel(pointer_max_deviation_px=5.5))[
                "passed"
            ]
        )

    def test_missing_or_unavailable_sentinel_fails(self) -> None:
        state, events = synth.forms_run(SEED)
        self.assertFalse(run("MB-12", "forms", state, events)["passed"])
        self.assertFalse(
            run("MB-12", "forms", state, events, clean_sentinel(available=False))["passed"]
        )

    def test_wrong_task_fails_even_when_undisturbed(self) -> None:
        state, events = synth.forms_run(
            SEED,
            fields={"priority": "Low" if C.derive_forms(SEED)["priority"] != "Low" else "High"},
        )
        res = run("MB-12", "forms", state, events, clean_sentinel())
        self.assertFalse(res["passed"])
        self.assertFalse(res["checks"]["priority"]["passed"])


if __name__ == "__main__":
    unittest.main()
