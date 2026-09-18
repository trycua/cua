from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import build_candidates, parse_visual_regions
from jev_adapter import choose_with_typesafe


class FakeClient:
    request = None

    def system_one(self, **request):
        FakeClient.request = request
        answer = SimpleNamespace(
            choice="submit-form",
            confidence=0.9,
            probabilities={"submit-form": 0.9},
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
        snapshot["refs"][0]["value"] = "expected"
        payload = json.loads(
            (
                Path(__file__).resolve().parents[2]
                / "fixtures/parse-visual-regions-submit-v1.json"
            ).read_text(encoding="utf-8")
        )
        visual = parse_visual_regions(
            payload,
            expected_capture_id="capture-submit",
            expected_pid=7,
            expected_window_id=9,
        )
        candidates = build_candidates(snapshot, "expected", visual, capture_bound_click=True)
        selected, confidence, probabilities = choose_with_typesafe(
            FakeClient(), candidates, snapshot, visual, []
        )

        self.assertEqual(selected, "submit-form")
        self.assertEqual(confidence, 0.9)
        self.assertEqual(probabilities[selected], 0.9)
        question = FakeClient.request["questions"]["driver_action"]
        self.assertEqual(
            set(question.criteria),
            {"submit-form", "reobserve", "abstain"},
        )
        sent_visual = FakeClient.request["state"]["observation"]["visual"]
        self.assertEqual(sent_visual["capture_id"], "capture-submit")
        self.assertEqual(sent_visual["regions"][0]["id"], "submit-text")

    def test_live_adapter_rejects_id_outside_the_supplied_table(self) -> None:
        snapshot = {"target_id": "target", "tab_id": "tab", "refs": []}
        candidates = build_candidates(snapshot, "expected")

        class UnknownClient(FakeClient):
            def system_one(self, **request):
                return SimpleNamespace(
                    choices={
                        "driver_action": SimpleNamespace(
                            choice="invented",
                            confidence=1.0,
                            probabilities={"invented": 1.0},
                        )
                    }
                )

        with self.assertRaisesRegex(ValueError, "unknown candidate"):
            choose_with_typesafe(UnknownClient(), candidates, snapshot, None, [])


if __name__ == "__main__":
    unittest.main()
