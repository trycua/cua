from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import build_candidates, choose_mock, classify, validate_choice


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

    def test_outcome_requires_oracle_match(self) -> None:
        self.assertEqual(classify("expected", "expected", steps=1, max_steps=4), "verified")
        self.assertEqual(classify("wrong", "expected", steps=1, max_steps=4), "refuted")
        self.assertEqual(classify(None, "expected", steps=4, max_steps=4), "budget_exhausted")


if __name__ == "__main__":
    unittest.main()
