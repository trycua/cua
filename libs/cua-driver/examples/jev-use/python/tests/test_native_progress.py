"""Task progress in cua.jev_choice_request_v2 (#4313).

A native task declares the steps it requires. The request reports how often
this run has performed each step, counted only from the runner's own
successful actions, and a step's candidate names any earlier step that is not
done yet. Nothing in ``progress`` is read from the application.
"""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from choose_action import provider_observation, validate_request
from decision_models import DecisionRequest, native_elements_as_text, progress_as_text
from native import NativeObservation
from native_tasks import (
    CANVAS_TASK_ID,
    HARNESSES,
    TaskStep,
    native_choice_request,
    native_task,
    performed_counts,
)
from sources import NativeAccessibilitySource
from tasks import TaskSources

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "fixtures/native"


def v2_request() -> dict:
    return json.loads((ROOT / "fixtures/jev-choice-request-v2.json").read_text(encoding="utf-8"))


def v1_request() -> dict:
    return json.loads((ROOT / "fixtures/jev-choice-request-v1.json").read_text(encoding="utf-8"))


def sources(task, harness: str, name: str = "initial") -> TaskSources:
    payload = json.loads((FIXTURES / f"{harness}-window-state-{name}-v1.json").read_text("utf-8"))
    observation = NativeObservation.from_window_state(
        payload, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
    )
    ax = NativeAccessibilitySource.from_observation(
        observation, HARNESSES[harness].platform, redact=task.redact_text, text_method=task.text_method
    )
    return TaskSources(ax=ax)


def request_for(task_id: str, history: list[dict], note_text: str = "jev-use native note") -> dict:
    task = native_task(task_id, Path("/tmp/none.json"), note_text=note_text)
    task_sources = sources(task, task_id.split("-")[0])
    return native_choice_request(task, task_sources, task.plan(task_sources), history)


def performed(task, step: int, candidate_id: str) -> dict:
    return task.history_entry(step, candidate_id, outcome="done")


class ProgressContractTest(unittest.TestCase):
    def test_v2_accepts_progress_and_the_observation_carries_it(self) -> None:
        request = v2_request()
        request["progress"] = [{"step": 'Press "Increment"', "done": 2, "required": 3}]
        validated = validate_request(request)
        self.assertEqual(validated["progress"], request["progress"])
        self.assertEqual(provider_observation(validated)["progress"], request["progress"])

    def test_v2_without_progress_keeps_its_observation(self) -> None:
        validated = validate_request(v2_request())
        self.assertEqual(validated["progress"], [])
        self.assertNotIn("progress", provider_observation(validated))

    def test_v1_rejects_progress(self) -> None:
        request = v1_request()
        request["progress"] = []
        with self.assertRaises(ValueError):
            validate_request(request)

    def test_v2_rejects_malformed_progress(self) -> None:
        item = {"step": "Press", "done": 0, "required": 1}
        mutations = {
            "not a list": {"step": "Press"},
            "too many": [item] * 17,
            "extra key": [{**item, "value": "secret"}],
            "missing key": [{"step": "Press", "done": 0}],
            "empty step": [{**item, "step": " "}],
            "long step": [{**item, "step": "x" * 201}],
            "negative done": [{**item, "done": -1}],
            "boolean done": [{**item, "done": True}],
            "float done": [{**item, "done": 1.0}],
            "zero required": [{**item, "required": 0}],
            "huge required": [{**item, "required": 65}],
        }
        for name, progress in mutations.items():
            request = copy.deepcopy(v2_request())
            request["progress"] = progress
            with self.subTest(name=name), self.assertRaises(ValueError):
                validate_request(request)

    def test_s1_text_renders_progress_after_history(self) -> None:
        request = v2_request()
        request["progress"] = [
            {"step": 'Press "Increment"', "done": 2, "required": 3},
            {"step": 'Press "Save note"', "done": 1, "required": 1},
        ]
        text = native_elements_as_text(DecisionRequest.from_validated(validate_request(request)))
        self.assertIn('- Press "Increment": done 2 of 3 times; 1 remaining', text)
        self.assertIn('- Press "Save note": done 1 of 1 times; complete', text)
        self.assertLess(text.index("Prior bounded decisions:"), text.index("Task progress"))

    def test_s1_text_without_progress_is_unchanged(self) -> None:
        request = DecisionRequest.from_validated(validate_request(v2_request()))
        self.assertNotIn("Task progress", native_elements_as_text(request))
        self.assertTrue(progress_as_text(()).startswith("Task progress"))

    def test_decision_request_observation_matches_the_validated_one(self) -> None:
        raw = v2_request()
        raw["progress"] = [{"step": "Press", "done": 0, "required": 1}]
        validated = validate_request(raw)
        request = DecisionRequest.from_validated(validated)
        self.assertEqual(request.provider_observation(), provider_observation(validated))


class TaskProgressTest(unittest.TestCase):
    def test_only_performed_actions_count(self) -> None:
        task = native_task("appkit-counter", Path("/tmp/none.json"))
        history = [
            performed(task, 1, "ax:button:increment"),
            task.history_entry(2, "ax:button:increment", refusal="background_unsupported"),
            task.history_entry(3, "ax:button:increment", stale=True),
            task.history_entry(4, "reobserve"),
            performed(task, 5, "ax:button:increment:foreground"),
        ]
        self.assertEqual(performed_counts(history), {"ax:button:increment": 2})
        self.assertEqual(
            task.progress(history),
            [{"step": 'Press the button labeled "Increment"', "done": 2, "required": 3}],
        )

    def test_counter_request_reports_increments_done(self) -> None:
        task = native_task("appkit-counter", Path("/tmp/none.json"))
        history = [performed(task, 1, "ax:button:increment"), performed(task, 2, "ax:button:increment")]
        request = request_for("appkit-counter", history)
        validate_request(request)
        self.assertEqual(request["progress"][0]["done"], 2)
        increment = {c["id"]: c for c in request["candidates"]}["ax:button:increment"]
        self.assertTrue(increment["description"].endswith("still requires this 1 more time(s)."))
        self.assertEqual(request["progress"][0]["required"], 3)
        # The runner-side marker never reaches the provider.
        self.assertEqual(set(request["history"][0]), {"selected_id", "outcome"})

    def test_save_is_described_as_waiting_for_the_note(self) -> None:
        for harness in HARNESSES:
            with self.subTest(harness=harness):
                request = request_for(f"{harness}-save-note", [], note_text="secret note")
                save = {c["id"]: c for c in request["candidates"]}["ax:button:save-note"]
                self.assertIn(
                    'The task requires this only after: Set the text field "Note" to the task '
                    'parameter "note" (not done yet).',
                    save["description"],
                )
                self.assertEqual([item["done"] for item in request["progress"]], [0, 0])
                self.assertNotIn("secret note", json.dumps(request))

    def test_save_precondition_clears_once_the_note_is_set(self) -> None:
        task = native_task("appkit-save-note", Path("/tmp/none.json"))
        request = request_for("appkit-save-note", [performed(task, 1, "ax:text_input:note:set:note")])
        save = {c["id"]: c for c in request["candidates"]}["ax:button:save-note"]
        self.assertEqual(
            save["description"],
            'Press the button labeled "Save note". The task still requires this 1 more time(s).',
        )
        self.assertEqual([item["done"] for item in request["progress"]], [1, 0])

    def test_a_completed_step_says_so(self) -> None:
        task = native_task("appkit-counter", Path("/tmp/none.json"))
        history = [performed(task, step, "ax:button:increment") for step in (1, 2, 3)]
        request = request_for("appkit-counter", history)
        increment = {c["id"]: c for c in request["candidates"]}["ax:button:increment"]
        self.assertTrue(
            increment["description"].endswith("This run already did this the 3 time(s) the task requires.")
        )

    def test_unordered_steps_have_no_precondition(self) -> None:
        request = request_for("appkit-choose-size", [])
        agree = {c["id"]: c for c in request["candidates"]}["ax:checkbox:i-agree"]
        self.assertNotIn("only after", agree["description"])
        self.assertTrue(agree["description"].endswith("still requires this 1 more time(s)."))

    def test_a_task_without_steps_sends_no_progress(self) -> None:
        task = native_task(CANVAS_TASK_ID, Path("/tmp/none.json"))
        self.assertEqual(task.steps, ())
        self.assertEqual(task.progress([]), [])

    def test_task_step_bounds(self) -> None:
        task = native_task("appkit-counter", Path("/tmp/none.json"))
        from dataclasses import replace

        with self.assertRaises(ValueError):
            replace(task, steps=(TaskStep("Press", "ax:button:increment", 0),))
        with self.assertRaises(ValueError):
            replace(task, steps=tuple(TaskStep("Press", f"id{i}") for i in range(17)))

    def test_recorded_requests_are_reproduced_exactly(self) -> None:
        golden = json.loads((FIXTURES / "native-progress-requests-v2.json").read_text("utf-8"))
        for case in golden["cases"]:
            with self.subTest(task=case["task"], steps=len(case["history"])):
                task = native_task(case["task"], Path("/tmp/none.json"))
                task_sources = sources(task, "appkit")
                request = native_choice_request(
                    task, task_sources, task.plan(task_sources), case["history"]
                )
                self.assertEqual(request, case["request"])
                validate_request(request)

    def test_same_progress_on_every_harness(self) -> None:
        requests = {
            harness: request_for(f"{harness}-save-note", []) for harness in HARNESSES
        }
        progress = {json.dumps(r["progress"]) for r in requests.values()}
        self.assertEqual(len(progress), 1)


if __name__ == "__main__":
    unittest.main()
