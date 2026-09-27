from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import (
    REDACTED_TOKEN,
    VisualObservation,
    VisualRegion,
    build_candidates,
    form_state,
    history_entry,
    redact_token,
    validate_choice,
)
from jev_adapter import choose_mock_adapter, choose_with_typesafe, decision_state

FIXTURE = json.loads(
    (Path(__file__).resolve().parents[2] / "fixtures/jev-page-structure-replay-v1.json").read_text(
        encoding="utf-8"
    )
)
TOKEN = FIXTURE["token"]
BEFORE = FIXTURE["snapshots"]["before_typing"]
AFTER = FIXTURE["snapshots"]["after_typing"]


def with_field_value(value: str | None) -> dict:
    snapshot = json.loads(json.dumps(BEFORE))
    for ref in snapshot["refs"]:
        if ref["role"] == "textbox" and ref["name"] == "verification value":
            ref["value"] = value
    return snapshot


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


class FormStateTest(unittest.TestCase):
    def test_form_state_reports_field_status_without_the_value(self) -> None:
        self.assertEqual(
            form_state(BEFORE, TOKEN),
            {"verification_field": "empty", "submit_button": "available"},
        )
        self.assertEqual(form_state(AFTER, TOKEN)["verification_field"], "contains_required_token")
        self.assertEqual(
            form_state(with_field_value("something-else"), TOKEN)["verification_field"],
            "contains_other_value",
        )
        no_button = {**AFTER, "refs": [ref for ref in AFTER["refs"] if ref["role"] != "button"]}
        self.assertEqual(form_state(no_button, TOKEN)["submit_button"], "not_in_page_structure")
        self.assertEqual(
            form_state({"refs": []}, TOKEN),
            {"verification_field": "not_found", "submit_button": "not_in_page_structure"},
        )

    def test_redaction_is_nested_and_deterministic(self) -> None:
        value = {"a": [f"x {TOKEN} y", {"b": TOKEN}], "n": 3, "none": None}
        redacted = redact_token(value, TOKEN)
        self.assertEqual(
            redacted,
            {"a": [f"x {REDACTED_TOKEN} y", {"b": REDACTED_TOKEN}], "n": 3, "none": None},
        )
        self.assertEqual(redact_token(value, TOKEN), redacted)
        self.assertEqual(redact_token("unchanged", ""), "unchanged")


class SubmitStepObservationTest(unittest.TestCase):
    def test_pre_fix_submit_state_hid_readiness_and_leaked_the_token(self) -> None:
        # Pins the root-cause evidence: the old state had no form summary, sent the
        # raw token in the outline, and fed raw telemetry back as history.
        old = FIXTURE["pre_fix_submit_state"]
        self.assertNotIn("form", old["observation"])
        self.assertIn(TOKEN, old["observation"]["outline"])
        self.assertIn("status=waiting", old["observation"]["outline"])
        self.assertIn("probabilities", old["history"][0])

    def test_after_typing_the_observation_states_the_filled_form(self) -> None:
        history = [history_entry(1, "type-verification-value")]
        state = decision_state(AFTER, None, history, TOKEN)
        encoded = json.dumps(state, sort_keys=True)

        self.assertNotIn(TOKEN, encoded)
        self.assertEqual(
            state["observation"]["form"],
            {"verification_field": "contains_required_token", "submit_button": "available"},
        )
        self.assertIn(
            f'textbox "verification value": {REDACTED_TOKEN}', state["observation"]["outline"]
        )
        self.assertEqual(
            state["history"],
            [
                {
                    "step": 1,
                    "selected_id": "type-verification-value",
                    "outcome": "typed the required token into the verification field",
                }
            ],
        )
        self.assertEqual(
            encoded, json.dumps(decision_state(AFTER, None, history, TOKEN), sort_keys=True)
        )

        candidates = build_candidates(AFTER, TOKEN)
        criteria = {candidate.id: candidate.description for candidate in candidates}
        self.assertEqual(list(criteria), ["submit-form", "reobserve", "abstain"])
        self.assertIn("already contains the required token", criteria["submit-form"])
        self.assertIn("stale, incomplete", criteria["reobserve"])
        self.assertNotIn(TOKEN, json.dumps(criteria))

    def test_visual_text_is_redacted_too(self) -> None:
        region = VisualRegion("r1", "text", TOKEN, None, 0.9, False, 1, 1, 10, 10)
        visual = VisualObservation("cap", "ref", 100, 100, 7, 9, 0.0, 0.0, 1.0, 1.0, (region,))
        state = decision_state(AFTER, visual, [], TOKEN)
        self.assertEqual(state["observation"]["visual"]["regions"][0]["text"], REDACTED_TOKEN)
        self.assertNotIn(TOKEN, json.dumps(state))

    def test_background_refusal_history_is_compact(self) -> None:
        entry = history_entry(2, "submit-form", refusal="background_unsupported")
        self.assertEqual(set(entry), {"step", "selected_id", "outcome"})
        self.assertIn("background_unsupported", entry["outcome"])


class ProviderPathTest(unittest.TestCase):
    def test_mock_provider_types_then_submits_over_recorded_snapshots(self) -> None:
        first = build_candidates(BEFORE, TOKEN)
        self.assertEqual(
            choose_mock_adapter(first, BEFORE, None, [], TOKEN)[0], "type-verification-value"
        )
        second = build_candidates(AFTER, TOKEN)
        history = [history_entry(1, "type-verification-value")]
        self.assertEqual(choose_mock_adapter(second, AFTER, None, history, TOKEN)[0], "submit-form")

    def test_recorded_live_choices_replay_through_the_new_state(self) -> None:
        for name, run in FIXTURE["recorded_live_runs"].items():
            with self.subTest(run=name):
                client = ReplayClient(run["steps"])
                history: list[dict] = []
                outcome = "budget_exhausted"
                for step in range(1, 5):
                    snapshot = BEFORE if step == 1 else AFTER
                    candidates = build_candidates(snapshot, TOKEN)
                    recorded = run["steps"][step - 1]
                    # The recorded choice was made over the same bounded table.
                    self.assertEqual(
                        set(recorded["probabilities"]), {candidate.id for candidate in candidates}
                    )
                    choice, _, _ = choose_with_typesafe(
                        client, candidates, snapshot, None, history, TOKEN
                    )
                    candidate = validate_choice(choice, candidates)
                    request = client.requests[-1]
                    self.assertNotIn(TOKEN, json.dumps(request["state"], sort_keys=True))
                    self.assertEqual(
                        request["state"]["observation"]["form"]["verification_field"],
                        "empty" if step == 1 else "contains_required_token",
                    )
                    for item in request["state"]["history"]:
                        self.assertEqual(set(item), {"step", "selected_id", "outcome"})
                    self.assertIn("reobserve", request["questions"]["driver_action"].criteria)
                    self.assertIn("abstain", request["questions"]["driver_action"].criteria)
                    history.append(history_entry(step, candidate.id))
                    if candidate.id == "submit-form":
                        outcome = "verified"
                        break
                self.assertEqual(outcome, run["outcome"])


if __name__ == "__main__":
    unittest.main()
