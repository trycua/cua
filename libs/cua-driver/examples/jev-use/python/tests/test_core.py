from __future__ import annotations

import json
import sys
import unittest
from dataclasses import FrozenInstanceError
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import (
    Candidate,
    build_candidates,
    choose_mock,
    classify,
    has_executable_candidate,
    parse_visual_regions,
    validate_choice,
)


FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"


class CoreTest(unittest.TestCase):
    def snapshot(self, value: str | None = None):
        return {
            "target_id": "target",
            "tab_id": "tab",
            "refs": [
                {"role": "textbox", "name": "verification value", "ref": "p1:0", "value": value},
                {"role": "button", "name": "Submit", "ref": "p1:1"},
            ],
        }

    def test_mock_types_before_submit(self) -> None:
        candidates = build_candidates(self.snapshot(), "expected")
        choice, confidence, probabilities = choose_mock(candidates)
        self.assertEqual(choice, "type-verification-value")
        self.assertEqual(candidates[0].arguments["ref"], "p1:0")
        self.assertEqual(confidence, 1.0)
        self.assertEqual(probabilities[choice], 1.0)

    def test_mock_submits_after_value_matches(self) -> None:
        candidates = build_candidates(self.snapshot("expected"), "expected")
        choice, _, _ = choose_mock(candidates)
        self.assertEqual(choice, "submit-form")

    def test_choice_must_match_current_candidates(self) -> None:
        with self.assertRaises(ValueError):
            validate_choice("stale-action", build_candidates(self.snapshot(), "expected"))

    def test_choice_resolves_to_original_candidate_arguments(self) -> None:
        candidates = build_candidates(self.snapshot(), "expected")
        selected = validate_choice("type-verification-value", candidates)
        self.assertIs(selected, candidates[0])
        self.assertEqual(
            selected.arguments,
            {"target_id": "target", "tab_id": "tab", "ref": "p1:0", "text": "expected", "replace": True},
        )
        with self.assertRaises(TypeError):
            selected.arguments["ref"] = "changed"
        with self.assertRaises(FrozenInstanceError):
            selected.capture_id = "changed"

    def test_reserved_candidates_are_always_available(self) -> None:
        candidates = build_candidates({"target_id": "target", "tab_id": "tab", "refs": []}, "expected")
        self.assertEqual([candidate.id for candidate in candidates], ["reobserve", "abstain"])
        self.assertEqual(choose_mock(candidates)[0], "reobserve")

    def test_visual_fixture_builds_candidate_without_claiming_interactivity(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        payload["regions"][0]["interactive"] = False
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        candidates = build_candidates(page, "expected", visual, capture_bound_click=True)
        selected = validate_choice("submit-form", candidates, current_capture_id="capture-submit")

        self.assertEqual(selected.id, "submit-form")
        self.assertEqual(selected.capture_id, "capture-submit")
        self.assertEqual(selected.screenshot_reference, "png-sha256:submit-fixture")
        self.assertEqual(
            dict(selected.arguments),
            {
                "pid": 7,
                "window_id": 9,
                "x": 350.0,
                "y": 260.0,
                "capture_id": "capture-submit",
                "delivery_mode": "background",
            },
        )

    def test_submit_path_depends_on_the_dom_button_ref(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        with_ref = build_candidates(
            self.snapshot("expected"), "expected", visual, capture_bound_click=True
        )
        self.assertEqual(with_ref[0].id, "submit-form")
        self.assertEqual(with_ref[0].tool, "browser_click")
        self.assertIsNone(with_ref[0].capture_id)

        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        without_ref = build_candidates(page, "expected", visual, capture_bound_click=True)
        self.assertEqual(without_ref[0].id, "submit-form")
        self.assertEqual(without_ref[0].tool, "click")
        self.assertEqual(without_ref[0].capture_id, "capture-submit")
        self.assertEqual(
            [candidate.id for candidate in build_candidates(page, "expected", None, capture_bound_click=True)],
            ["reobserve", "abstain"],
        )

    def test_visual_input_does_not_change_semantic_candidate_set(self) -> None:
        # Salvaged from #4165: when the page structure already offers an
        # executable action, a visual observation cannot change the candidates,
        # so the runner may skip the parse without changing behavior.
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        for value in [None, "expected"]:
            without_visual = build_candidates(
                self.snapshot(value), "expected", None, capture_bound_click=True
            )
            with_visual = build_candidates(
                self.snapshot(value), "expected", visual, capture_bound_click=True
            )
            self.assertEqual(with_visual, without_visual)
            self.assertTrue(has_executable_candidate(without_visual))

    def test_foreground_escalation_is_a_distinct_visual_candidate(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        candidates = build_candidates(
            page, "expected", visual, capture_bound_click=True, visual_delivery="foreground"
        )
        ids = [candidate.id for candidate in candidates]
        self.assertEqual(ids, ["submit-form-foreground", "reobserve", "abstain"])
        self.assertEqual(candidates[0].tool, "click")
        self.assertEqual(candidates[0].arguments["delivery_mode"], "foreground")
        self.assertEqual(candidates[0].arguments["capture_id"], "capture-submit")
        self.assertIn("foreground", candidates[0].description)
        self.assertEqual(choose_mock(candidates)[0], "submit-form-foreground")
        # Page-structure refs still win after escalation.
        self.assertEqual(
            build_candidates(
                self.snapshot("expected"), "expected", visual,
                capture_bound_click=True, visual_delivery="foreground",
            )[0].tool,
            "browser_click",
        )

    def test_visual_ambiguity_offers_only_reserved_candidates(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-ambiguous-v1.json").read_text())
        for region in payload["regions"]:
            region["interactive"] = False
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-ambiguous",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        self.assertEqual(
            [
                candidate.id
                for candidate in build_candidates(
                    page, "expected", visual, capture_bound_click=True
                )
            ],
            ["reobserve", "abstain"],
        )

    def test_visual_non_submit_observation_offers_only_reserved_candidates(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        payload["regions"][0]["text"] = "Continue"
        payload["regions"][0]["interactive"] = True
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]

        self.assertEqual(
            [
                candidate.id
                for candidate in build_candidates(
                    page, "expected", visual, capture_bound_click=True
                )
            ],
            ["reobserve", "abstain"],
        )

    def test_visual_candidate_requires_capture_bound_click_contract(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        self.assertEqual(
            [candidate.id for candidate in build_candidates(page, "expected", visual)],
            ["reobserve", "abstain"],
        )

    def test_null_optionals_and_ascii_case_rules_are_shared(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-null-and-case-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-edge",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        candidates = build_candidates(page, "expected", visual, capture_bound_click=True)
        selected = validate_choice("submit-form", candidates, current_capture_id="capture-edge")
        self.assertEqual(selected.arguments["x"], 140.0)
        self.assertEqual(selected.arguments["capture_id"], "capture-edge")

    def test_visual_stale_malformed_and_duplicate_candidates_fail_closed(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        with self.assertRaisesRegex(ValueError, "stale"):
            parse_visual_regions(
                payload,
                expected_capture_id="new-capture",
                expected_pid=7,
                expected_window_id=9,
            )
        payload["regions"][0]["bounds"]["width"] = 900
        with self.assertRaisesRegex(ValueError, "outside"):
            parse_visual_regions(
                payload,
                expected_capture_id="capture-submit",
                expected_pid=7,
                expected_window_id=9,
            )
        duplicate = Candidate("duplicate", "one", None, {})
        with self.assertRaisesRegex(ValueError, "duplicate"):
            validate_choice("duplicate", [duplicate, duplicate])

    def test_retina_affine_mapping_keeps_the_original_screenshot_point(self) -> None:
        # Issue #4289: Driver reports a non-identity capture as ``affine``
        # (never ``scaled_top_left``). A Retina-like 2x capture of a window at
        # (100, 200) maps pixels to points with scale 0.5 plus that offset.
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        self.assertEqual(payload["capture"]["action_coordinate_space"]["kind"], "affine")
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        self.assertEqual(visual.screenshot_to_action, (0.5, 0.0, 0.0, 0.5, 100.0, 200.0))
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        selected = validate_choice(
            "submit-form",
            build_candidates(page, "expected", visual, capture_bound_click=True),
            current_capture_id="capture-submit",
        )
        # Driver applies the mapping for the capture ID, so the click carries
        # the region center in screenshot pixels, not mapped (275, 330).
        self.assertEqual(
            (selected.arguments["x"], selected.arguments["y"], selected.arguments["capture_id"]),
            (350.0, 260.0, "capture-submit"),
        )

        # The reporter's observed macOS Retina mapping from #4289.
        payload["capture"]["action_coordinate_space"] = {
            "kind": "affine",
            "m11": 1.530612244897959,
            "m12": 0.0,
            "m21": 0.0,
            "m22": 1.5304487179487178,
            "tx": 0.0,
            "ty": 0.0,
        }
        observed = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        self.assertEqual(observed.screenshot_to_action[0], 1.530612244897959)

    def test_identity_screenshot_pixels_mapping(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-ambiguous-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-ambiguous",
            expected_pid=7,
            expected_window_id=9,
        )
        self.assertEqual(visual.screenshot_to_action, (1.0, 0.0, 0.0, 1.0, 0.0, 0.0))

    def test_unrepresentable_coordinate_mappings_fail_closed(self) -> None:
        base = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        affine = base["capture"]["action_coordinate_space"]
        cases = {
            "non-invertible": {**affine, "m11": 0.0, "m22": 0.0},
            "singular": {**affine, "m11": 1.0, "m12": 2.0, "m21": 2.0, "m22": 4.0},
            "nan": {**affine, "m11": float("nan")},
            "infinite": {**affine, "tx": float("inf")},
            "missing coefficient": {key: value for key, value in affine.items() if key != "ty"},
            "boolean coefficient": {**affine, "m12": False},
            "overflowing": {**affine, "m11": 1e308, "m22": 1e308},
            "scaled_top_left": {
                "kind": "scaled_top_left",
                "action_origin_x": 100.0,
                "action_origin_y": 200.0,
                "action_units_per_pixel_x": 0.5,
                "action_units_per_pixel_y": 0.5,
            },
            "unknown kind": {"kind": "projective"},
            "absent": None,
        }
        for name, space in cases.items():
            with self.subTest(name):
                payload = json.loads(json.dumps(base))
                payload["capture"]["action_coordinate_space"] = space
                with self.assertRaisesRegex(ValueError, "coordinate"):
                    parse_visual_regions(
                        payload,
                        expected_capture_id="capture-submit",
                        expected_pid=7,
                        expected_window_id=9,
                    )

    def test_capture_bound_choice_rejects_a_newer_capture(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        page = self.snapshot("expected")
        page["refs"] = page["refs"][:1]
        with self.assertRaisesRegex(ValueError, "stale"):
            validate_choice(
                "submit-form",
                build_candidates(page, "expected", visual, capture_bound_click=True),
                current_capture_id="new-capture",
            )

    def test_outcome_requires_oracle_match(self) -> None:
        self.assertEqual(classify("expected", "expected", steps=1, max_steps=4), "verified")
        self.assertEqual(classify("wrong", "expected", steps=1, max_steps=4), "refuted")
        self.assertEqual(classify(None, "expected", steps=4, max_steps=4), "budget_exhausted")


if __name__ == "__main__":
    unittest.main()
