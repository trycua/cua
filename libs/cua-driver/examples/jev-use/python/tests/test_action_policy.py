from __future__ import annotations

import copy
import sys
import unittest
from pathlib import Path

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))

from action_policy import (
    ActionAuthorizationError,
    ExactRegionTextAction,
    authorize_exact_region_text_action,
)
from choose_decision import choose_request
from decision_models import ModelScores


def inputs(
    text: str = "Send", required: str = "Send"
) -> tuple[dict, dict, dict, ExactRegionTextAction]:
    region = {
        "id": "text-60",
        "kind": "text",
        "text": text,
        "bounds": {"x": 12, "y": 20, "width": 72, "height": 32},
        "confidence": 0.98,
        "interactive": False,
    }
    action = ExactRegionTextAction("region:text-60", "text-60", required)
    request = {
        "capture_id": "capture-1",
        "regions": [copy.deepcopy(region)],
        "candidates": [
            action.wire_candidate(),
            {"id": "reobserve", "description": "Observe again."},
            {"id": "abstain", "description": "Stop."},
        ],
    }
    decision = {
        "schema": "cua.decision_choice_v1",
        "kind": "selected",
        "capture_id": "capture-1",
        "selected_id": action.candidate_id,
        "probabilities": {action.candidate_id: 0.777, "reobserve": 0.123, "abstain": 0.1},
    }
    parsed = {
        "schema": "cua.visual_regions_v1",
        "capture": {
            "capture_id": "capture-1",
            "source": {"kind": "window", "pid": 10, "window_id": 20},
            "screenshot": {
                "reference": "png-sha256:synthetic",
                "width": 100,
                "height": 100,
                "mime_type": "image/png",
            },
            "action_coordinate_space": {"kind": "screenshot_pixels"},
        },
        "regions": [region],
    }
    return request, decision, parsed, action


class ActionPolicyTest(unittest.TestCase):
    def authorize(self, request, decision, parsed, action, capture_id="capture-1", source=None):
        source = source or {"kind": "window", "pid": 10, "window_id": 20}
        return authorize_exact_region_text_action(
            request,
            decision,
            parsed,
            action,
            current_capture_id=capture_id,
            expected_source=source,
        )

    def test_matching_text_authorizes(self) -> None:
        click = self.authorize(*inputs())
        self.assertEqual((click.capture_id, click.x, click.y), ("capture-1", 48.0, 36.0))

    def test_selected_wrong_text_never_reaches_dispatch(self) -> None:
        request, decision, parsed, action = inputs("Send", "Save")
        request.update(
            {"schema": "cua.jev_choice_request_v1", "goal": "Choose one.", "history": []}
        )

        class WrongSelectionModel:
            name = "synthetic-unsafe-model"

            def score(self, _request):
                return ModelScores(
                    {action.candidate_id: 0.777, "reobserve": 0.123, "abstain": 0.1},
                    self.name,
                )

        decision = choose_request(request, WrongSelectionModel())
        self.assertEqual(
            (decision["kind"], decision["selected_id"]), ("selected", action.candidate_id)
        )
        dispatched: list[str] = []
        with self.assertRaisesRegex(ActionAuthorizationError, "exact-text condition"):
            self.authorize(request, decision, parsed, action)
            dispatched.append("click")
        self.assertEqual(dispatched, [])

    def test_stale_or_changed_capture_refuses(self) -> None:
        for field in ("request", "decision", "parse", "current"):
            with self.subTest(field=field):
                request, decision, parsed, action = inputs()
                current = "capture-1"
                if field == "request":
                    request["capture_id"] = "old"
                elif field == "decision":
                    decision["capture_id"] = "old"
                elif field == "parse":
                    parsed["capture"]["capture_id"] = "old"
                else:
                    current = "new"
                with self.assertRaisesRegex(ActionAuthorizationError, "capture identity"):
                    self.authorize(request, decision, parsed, action, current)

    def test_action_and_offered_condition_must_match(self) -> None:
        request, decision, parsed, action = inputs()
        request["candidates"][0]["description"] = "An unrelated condition."
        with self.assertRaisesRegex(ActionAuthorizationError, "action condition"):
            self.authorize(request, decision, parsed, action)

        request, decision, parsed, action = inputs()
        decision["selected_id"] = "abstain"
        with self.assertRaisesRegex(ActionAuthorizationError, "did not select"):
            self.authorize(request, decision, parsed, action)

    def test_nonunique_low_confidence_and_offered_region_refuse(self) -> None:
        for field in ("duplicate", "low_confidence", "offered", "bounds"):
            with self.subTest(field=field):
                request, decision, parsed, action = inputs()
                if field == "duplicate":
                    other = copy.deepcopy(parsed["regions"][0])
                    other["id"] = "text-61"
                    parsed["regions"].append(other)
                elif field == "low_confidence":
                    parsed["regions"][0]["confidence"] = 0.7999
                    request["regions"][0]["confidence"] = 0.7999
                elif field == "offered":
                    request["regions"][0]["text"] = "Save"
                else:
                    parsed["regions"][0]["bounds"]["width"] = 0
                    request["regions"][0]["bounds"]["width"] = 0
                with self.assertRaises(ActionAuthorizationError):
                    self.authorize(request, decision, parsed, action)

    def test_case_is_exact_and_reserved_outcomes_are_not_actions(self) -> None:
        request, decision, parsed, action = inputs("send", "Send")
        with self.assertRaisesRegex(ActionAuthorizationError, "exact-text condition"):
            self.authorize(request, decision, parsed, action)
        request, decision, parsed, action = inputs()
        decision.update({"kind": "abstain", "selected_id": "abstain"})
        with self.assertRaisesRegex(ActionAuthorizationError, "did not select"):
            self.authorize(request, decision, parsed, action)

    def test_unsupported_action_and_malformed_confidence_refuse(self) -> None:
        request, decision, parsed, action = inputs()
        with self.assertRaisesRegex(ActionAuthorizationError, "unsupported action"):
            self.authorize(request, decision, parsed, object())
        parsed["regions"][0]["confidence"] = float("nan")
        with self.assertRaisesRegex(ActionAuthorizationError, "malformed visual confidence"):
            self.authorize(request, decision, parsed, action)

    def test_decision_scores_must_match_offered_candidates(self) -> None:
        for case in ("missing", "wrong_set", "non_argmax"):
            with self.subTest(case=case):
                request, decision, parsed, action = inputs()
                if case == "missing":
                    del decision["probabilities"]
                elif case == "wrong_set":
                    decision["probabilities"]["invented"] = 0.0
                else:
                    decision["probabilities"] = {
                        action.candidate_id: 0.2,
                        "reobserve": 0.7,
                        "abstain": 0.1,
                    }
                with self.assertRaisesRegex(ActionAuthorizationError, "decision scores"):
                    self.authorize(request, decision, parsed, action)

    def test_wire_description_preserves_the_enforced_threshold(self) -> None:
        with self.assertRaisesRegex(ValueError, "two decimal places"):
            ExactRegionTextAction("region:one", "one", "Send", 0.805)

    def test_source_geometry_and_provenance_are_checked(self) -> None:
        for case in ("source", "screenshot", "outside", "mapping"):
            with self.subTest(case=case):
                request, decision, parsed, action = inputs()
                if case == "source":
                    parsed["capture"]["source"]["window_id"] = 21
                elif case == "screenshot":
                    parsed["capture"]["screenshot"]["mime_type"] = "image/jpeg"
                elif case == "outside":
                    parsed["regions"][0]["bounds"]["x"] = 90
                    request["regions"][0]["bounds"]["x"] = 90
                else:
                    parsed["capture"]["action_coordinate_space"] = {
                        "kind": "affine",
                        "m11": 0,
                        "m12": 0,
                        "m21": 0,
                        "m22": 0,
                        "tx": 0,
                        "ty": 0,
                    }
                with self.assertRaises(ActionAuthorizationError):
                    self.authorize(request, decision, parsed, action)

    def test_primary_desktop_and_affine_coordinates(self) -> None:
        request, decision, parsed, action = inputs()
        source = {"kind": "primary_desktop", "display_id": "primary"}
        parsed["capture"]["source"] = source
        parsed["capture"]["action_coordinate_space"] = {
            "kind": "affine",
            "m11": 2,
            "m12": 0,
            "m21": 0,
            "m22": 2,
            "tx": 10,
            "ty": 20,
        }
        click = self.authorize(request, decision, parsed, action, source=source)
        self.assertEqual((click.x, click.y), (106.0, 92.0))

    def test_tied_scores_and_duplicate_offered_regions_refuse(self) -> None:
        request, decision, parsed, action = inputs()
        decision["probabilities"] = {
            action.candidate_id: 0.5,
            "reobserve": 0.5,
            "abstain": 0.0,
        }
        with self.assertRaisesRegex(ActionAuthorizationError, "decision scores"):
            self.authorize(request, decision, parsed, action)
        request, decision, parsed, action = inputs()
        request["regions"].append(copy.deepcopy(request["regions"][0]))
        with self.assertRaisesRegex(ActionAuthorizationError, "offered visual regions"):
            self.authorize(request, decision, parsed, action)


if __name__ == "__main__":
    unittest.main()
