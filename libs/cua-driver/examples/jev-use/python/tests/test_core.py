from __future__ import annotations

import json
import sys
import unittest
from dataclasses import FrozenInstanceError
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import Candidate, build_candidates, choose_mock, classify, parse_visual_regions, validate_choice


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

    def test_visual_fixture_builds_equivalent_capture_bound_submit_candidate(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
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

    def test_visual_ambiguity_offers_only_reserved_candidates(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-ambiguous-v1.json").read_text())
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
