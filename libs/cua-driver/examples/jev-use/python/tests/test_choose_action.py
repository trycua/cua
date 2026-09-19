from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from choose_action import choose_request, validate_request


def request() -> dict:
    return {
        "schema": "cua.jev_choice_request_v1",
        "goal": "Submit the verified form.",
        "capture_id": "capture-1",
        "regions": [
            {
                "id": "submit",
                "kind": "text",
                "bounds": {"x": 10, "y": 20, "width": 80, "height": 30},
                "text": "Submit",
                "confidence": 0.98,
                "interactive": True,
            }
        ],
        "history": [{"selected_id": "type-value", "outcome": "observed"}],
        "candidates": [
            {"id": "submit-form", "description": "Submit the validated form."},
            {"id": "reobserve", "description": "Obtain a fresh observation."},
            {"id": "abstain", "description": "Stop without acting."},
        ],
    }


class ChooseActionTest(unittest.TestCase):
    def test_mock_cli_emits_only_the_allowlisted_response(self) -> None:
        script = Path(__file__).resolve().parents[1] / "choose_action.py"
        result = subprocess.run(
            [sys.executable, str(script), "--mock"],
            input=json.dumps(request()),
            text=True,
            capture_output=True,
            check=True,
        )
        response = json.loads(result.stdout)
        self.assertEqual(
            set(response),
            {"schema", "selected_id", "model", "confidence", "probabilities"},
        )
        self.assertEqual(response["schema"], "cua.jev_choice_v1")
        self.assertEqual(response["selected_id"], "submit-form")
        self.assertEqual(response["model"], "mock")
        self.assertEqual(result.stderr, "")

    def test_live_client_receives_bounded_state_and_returns_model(self) -> None:
        class FakeClient:
            sent = None

            def system_one(self, **payload):
                self.sent = payload
                return SimpleNamespace(
                    model="jev-test",
                    choices={
                        "candidate": SimpleNamespace(
                            choice="reobserve",
                            confidence=0.75,
                            probabilities={"reobserve": 0.75, "abstain": 0.25},
                        )
                    },
                )

        client = FakeClient()
        response = choose_request(request(), client=client)
        self.assertEqual(response["selected_id"], "reobserve")
        self.assertEqual(response["model"], "jev-test")
        criteria = client.sent["questions"]["candidate"].criteria
        self.assertEqual(set(criteria), {"submit-form", "reobserve", "abstain"})
        observation = client.sent["state"]["observation"]
        self.assertEqual(observation["capture_id"], "capture-1")
        self.assertEqual(observation["regions"][0]["id"], "submit")
        self.assertNotIn("arguments", json.dumps(client.sent["state"]))

    def test_request_rejects_tool_arguments_screenshot_bytes_and_missing_reserved_ids(self) -> None:
        for field in ("arguments", "screenshot", "image_base64"):
            invalid = request()
            invalid["history"] = [{field: "secret"}]
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "forbidden"):
                validate_request(invalid)

        invalid = request()
        invalid["candidates"] = invalid["candidates"][:2]
        with self.assertRaisesRegex(ValueError, "reobserve and abstain"):
            validate_request(invalid)

    def test_provider_cannot_select_or_score_an_unknown_id(self) -> None:
        class UnknownClient:
            def system_one(self, **payload):
                return SimpleNamespace(
                    model="jev-test",
                    choices={
                        "candidate": SimpleNamespace(
                            choice="invented",
                            confidence=1.0,
                            probabilities={"invented": 1.0},
                        )
                    },
                )

        with self.assertRaisesRegex(ValueError, "unknown candidate"):
            choose_request(request(), client=UnknownClient())


if __name__ == "__main__":
    unittest.main()
