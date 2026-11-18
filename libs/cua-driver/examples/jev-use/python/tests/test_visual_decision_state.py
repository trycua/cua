from __future__ import annotations

import json
import sys
import unittest
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import (
    SUBMIT_IDS,
    build_candidates,
    choose_mock,
    form_state,
    has_executable_candidate,
    history_entry,
    parse_visual_regions,
    validate_choice,
    visual_submit_region,
)
from jev_adapter import choose_with_typesafe, decision_state

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"
VISUAL = json.loads((FIXTURES / "jev-visual-replay-v1.json").read_text(encoding="utf-8"))
PAGE = json.loads((FIXTURES / "jev-page-structure-replay-v1.json").read_text(encoding="utf-8"))
TOKEN = VISUAL["token"]
BEFORE = VISUAL["snapshots"]["before_typing"]
AFTER = VISUAL["snapshots"]["after_typing"]
PAYLOAD = VISUAL["visual_regions"]


def observation():
    source = PAYLOAD["capture"]["source"]
    return parse_visual_regions(
        PAYLOAD,
        expected_capture_id=PAYLOAD["capture"]["capture_id"],
        expected_pid=source["pid"],
        expected_window_id=source["window_id"],
    )


class ReplayClient:
    """Return recorded live choices in order and keep every request."""

    def __init__(self, recorded: list[dict]) -> None:
        self.recorded = list(recorded)
        self.requests: list[dict] = []

    def system_one(self, **request):
        self.requests.append(request)
        step = self.recorded.pop(0)
        answer = SimpleNamespace(
            choice=step["selected_id"],
            confidence=max(step["probabilities"].values()),
            probabilities=step["probabilities"],
        )
        return SimpleNamespace(choices={"driver_action": answer})


class VisualFormStateTest(unittest.TestCase):
    def test_fixture_exposes_submit_only_as_text_and_as_one_visual_region(self) -> None:
        for snapshot in (BEFORE, AFTER):
            self.assertFalse(any(ref["role"] == "button" for ref in snapshot["refs"]))
            self.assertIn('statictext "Submit"', snapshot["outline"])
        region = visual_submit_region(observation())
        self.assertIsNotNone(region)
        self.assertEqual(region.text, "Submit")

    def test_submit_state_names_the_visual_path_instead_of_a_missing_button(self) -> None:
        visual = observation()
        self.assertEqual(form_state(BEFORE, TOKEN)["submit_button"], "not_in_page_structure")
        self.assertEqual(
            form_state(BEFORE, TOKEN, visual_path=True),
            {"verification_field": "empty", "submit_button": "visual_check_pending"},
        )
        self.assertEqual(
            form_state(AFTER, TOKEN, visual, visual_path=True),
            {"verification_field": "contains_required_token", "submit_button": "visual_only"},
        )
        without_submit = replace(
            visual, regions=tuple(r for r in visual.regions if r.text != "Submit")
        )
        self.assertEqual(
            form_state(AFTER, TOKEN, without_submit, visual_path=True)["submit_button"],
            "not_found_visually",
        )
        submit = visual_submit_region(visual)
        duplicated = replace(visual, regions=visual.regions + (replace(submit, id="dup"),))
        self.assertEqual(
            form_state(AFTER, TOKEN, duplicated, visual_path=True)["submit_button"],
            "not_found_visually",
        )
        # A page-structure Submit ref always wins, with or without a visual path.
        page_after = PAGE["snapshots"]["after_typing"]
        self.assertEqual(
            form_state(page_after, PAGE["token"], visual, visual_path=True)["submit_button"],
            "available",
        )

    def test_pre_fix_step_one_state_contradicted_the_outline(self) -> None:
        # Pins the regression: the outline shows Submit text, the form said the
        # button was missing, and reobserve invited "incomplete" observations.
        old = VISUAL["pre_fix_step1"]
        self.assertEqual(
            old["state"]["observation"]["form"]["submit_button"], "not_in_page_structure"
        )
        self.assertIn('statictext "Submit"', old["state"]["observation"]["outline"])
        self.assertIn("incomplete", old["criteria"]["reobserve"])

        new_state = decision_state(BEFORE, None, [], TOKEN, visual_path=True)
        self.assertEqual(new_state["observation"]["form"]["submit_button"], "visual_check_pending")
        expected = json.loads(json.dumps(old["state"]))
        expected["observation"]["form"]["submit_button"] = "visual_check_pending"
        self.assertEqual(new_state, expected)

        criteria = {c.id: c.description for c in build_candidates(BEFORE, TOKEN)}
        self.assertEqual(list(criteria), list(old["criteria"]))
        self.assertNotIn("incomplete", criteria["reobserve"])
        self.assertIn("visual-only", criteria["reobserve"])
        self.assertIn("still pending", criteria["reobserve"])


class DeterministicPathsTest(unittest.TestCase):
    def test_page_structure_fixture_types_then_submits_with_browser_click(self) -> None:
        token = PAGE["token"]
        first = build_candidates(PAGE["snapshots"]["before_typing"], token)
        self.assertEqual(choose_mock(first)[0], "type-verification-value")
        second = build_candidates(PAGE["snapshots"]["after_typing"], token)
        choice = validate_choice(choose_mock(second)[0], second)
        self.assertEqual((choice.id, choice.tool), ("submit-form", "browser_click"))

    def test_visual_fixture_types_then_submits_with_a_capture_bound_click(self) -> None:
        first = build_candidates(BEFORE, TOKEN, capture_bound_click=True)
        choice = validate_choice(choose_mock(first)[0], first)
        self.assertEqual((choice.id, choice.tool), ("type-verification-value", "browser_type"))

        # After typing the page structure offers no action, so the runner parses.
        self.assertFalse(
            has_executable_candidate(build_candidates(AFTER, TOKEN, capture_bound_click=True))
        )
        visual = observation()
        second = build_candidates(AFTER, TOKEN, visual, capture_bound_click=True)
        choice = validate_choice(
            choose_mock(second)[0], second, current_capture_id=visual.capture_id
        )
        submit = visual_submit_region(visual)
        self.assertEqual((choice.id, choice.tool), ("submit-form", "click"))
        self.assertEqual(choice.arguments["capture_id"], visual.capture_id)
        self.assertEqual(choice.arguments["delivery_mode"], "background")
        self.assertEqual(
            (choice.arguments["x"], choice.arguments["y"]), visual.screenshot_center(submit)
        )

        third = build_candidates(
            AFTER, TOKEN, visual, capture_bound_click=True, visual_delivery="foreground"
        )
        choice = validate_choice(choose_mock(third)[0], third, current_capture_id=visual.capture_id)
        self.assertEqual(
            (choice.id, choice.arguments["delivery_mode"]), ("submit-form-foreground", "foreground")
        )


class VisualReplayTest(unittest.TestCase):
    def test_recorded_visual_run_choices_replay_through_the_new_state(self) -> None:
        visual = observation()
        for name, run in VISUAL["recorded_live_runs"].items():
            with self.subTest(run=name):
                client = ReplayClient(run["steps"])
                history: list[dict] = []
                typed = False
                delivery = "background"
                outcome = "budget_exhausted"
                for recorded in run["steps"]:
                    step = recorded["step"]
                    snapshot = AFTER if typed else BEFORE
                    current = visual if typed else None
                    candidates = build_candidates(
                        snapshot,
                        TOKEN,
                        current,
                        capture_bound_click=True,
                        visual_delivery=delivery,
                    )
                    self.assertEqual(set(recorded["probabilities"]), {c.id for c in candidates})
                    choice, _, _ = choose_with_typesafe(
                        client, candidates, snapshot, current, history, TOKEN, True
                    )
                    candidate = validate_choice(
                        choice,
                        candidates,
                        current_capture_id=current.capture_id if current else None,
                    )
                    request = client.requests[-1]
                    form = request["state"]["observation"]["form"]
                    self.assertNotIn(TOKEN, json.dumps(request["state"], sort_keys=True))
                    self.assertEqual(
                        form,
                        {
                            "verification_field": "contains_required_token" if typed else "empty",
                            "submit_button": "visual_only" if typed else "visual_check_pending",
                        },
                    )
                    criteria = request["questions"]["driver_action"].criteria
                    self.assertNotIn("incomplete", criteria["reobserve"])
                    for item in request["state"]["history"]:
                        self.assertEqual(set(item), {"step", "selected_id", "outcome"})
                    refusal = recorded.get("action_error")
                    history.append(history_entry(step, candidate.id, refusal=refusal))
                    if candidate.id == "type-verification-value":
                        typed = True
                    elif candidate.id in SUBMIT_IDS:
                        self.assertEqual(candidate.tool, "click")
                        if refusal:
                            delivery = "foreground"
                        else:
                            outcome = "verified"
                            break
                self.assertEqual(outcome, run["outcome"])


if __name__ == "__main__":
    unittest.main()
