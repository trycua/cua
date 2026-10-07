from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import decision_timing_fields


class DecisionTimingTest(unittest.TestCase):
    def test_step_timing_field_names_and_values_are_stable(self) -> None:
        fields = decision_timing_fields(
            decision_ms=14.5,
            semantic_observe_ms=4.0,
            visual_observe_ms=3.0,
            candidate_build_ms=2.0,
            provider_decision_ms=5.0,
        )
        self.assertEqual(
            fields,
            {
                "decision_ms": 14.5,
                "semantic_observe_ms": 4.0,
                "visual_observe_ms": 3.0,
                "candidate_build_ms": 2.0,
                "provider_decision_ms": 5.0,
            },
        )


if __name__ == "__main__":
    unittest.main()
