from __future__ import annotations

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import build_candidates
from run import choose_live


class FakeClient:
    request = None

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return None

    def system_one(self, **request):
        FakeClient.request = request
        answer = SimpleNamespace(
            choice="type-verification-value",
            confidence=0.9,
            probabilities={"type-verification-value": 0.9},
        )
        return SimpleNamespace(choices={"driver_action": answer})


class LiveAdapterTest(unittest.TestCase):
    def test_live_adapter_uses_one_choice_over_candidate_ids(self) -> None:
        snapshot = {
            "target_id": "target",
            "tab_id": "tab",
            "page": {"url": "http://fixture.test/"},
            "outline": "textbox verification value",
            "refs": [
                {
                    "role": "textbox",
                    "name": "verification value",
                    "ref": "p1:0",
                    "value": None,
                }
            ],
        }
        candidates = build_candidates(snapshot, "expected")
        with patch("typesafe_sdk.TypeSafeClient", FakeClient):
            selected, confidence, probabilities = choose_live(candidates, snapshot, [])

        self.assertEqual(selected, "type-verification-value")
        self.assertEqual(confidence, 0.9)
        self.assertEqual(probabilities[selected], 0.9)
        question = FakeClient.request["questions"]["driver_action"]
        self.assertEqual(set(question.criteria), {"type-verification-value", "abstain"})


if __name__ == "__main__":
    unittest.main()
