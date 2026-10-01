from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))

import browser_provider
from browser_provider import backend_name, browser_decision_request, choose_browser_provider
from tasks import FixtureFormTask, fixture_sources


class BrowserProviderTest(unittest.TestCase):
    def sources(self, token: str):
        snapshot = {
            "target_id": "target",
            "tab_id": "tab",
            "capture_id": "browser-capture-1",
            "refs": [
                {
                    "role": "textbox",
                    "name": "verification value",
                    "ref": "p1:0",
                    "value": "",
                },
                {"role": "button", "name": "Submit", "ref": "p1:1"},
            ],
        }
        return fixture_sources(snapshot)

    def test_browser_request_is_bounded_and_contains_no_action_arguments_or_secret(self):
        token = "secret-proof-token"
        task = FixtureFormTask(token)
        sources = self.sources(token)
        candidates = task.candidates(sources)
        request = browser_decision_request(task, sources, candidates, [])
        self.assertEqual(request["schema"], "cua.jev_choice_request_v1")
        self.assertEqual(request["capture_id"], "browser-capture-1")
        self.assertEqual(
            [item["id"] for item in request["candidates"]],
            ["type-verification-value", "reobserve", "abstain"],
        )
        wire = json.dumps(request)
        self.assertNotIn(token, wire)
        self.assertNotIn("arguments", wire)
        self.assertNotIn("browser_type", wire)

    def test_mock_uses_existing_task_policy_and_reports_actual_backend(self):
        token = "proof-token"
        task = FixtureFormTask(token)
        sources = self.sources(token)
        candidates = task.candidates(sources)
        choice, confidence, probabilities, backend = choose_browser_provider(
            "mock", task, sources, candidates, []
        )
        self.assertEqual(choice, "type-verification-value")
        self.assertEqual(confidence, 1.0)
        self.assertEqual(backend, "mock")
        self.assertEqual(probabilities[choice], 1.0)
        self.assertEqual(backend_name("live"), "typesafe")
        self.assertEqual(backend_name("typesafe"), "typesafe")
        self.assertEqual(backend_name("s1"), "s1")

    def test_s1_receipt_reports_s1_and_uses_bounded_request(self):
        task = FixtureFormTask("proof-token")
        sources = self.sources("proof-token")
        candidates = task.candidates(sources)
        expected = (
            "type-verification-value",
            0.7,
            {"type-verification-value": 0.7, "reobserve": 0.2, "abstain": 0.1},
        )
        with patch.object(browser_provider, "choose_s1_service", return_value=expected) as choose_s1:
            choice, confidence, probabilities, backend = choose_browser_provider(
                "s1", task, sources, candidates, []
            )
        self.assertEqual((choice, confidence, probabilities), expected)
        self.assertEqual(backend, "s1")
        request = choose_s1.call_args.args[0]
        self.assertEqual(request["schema"], "cua.jev_choice_request_v1")
        self.assertNotIn("arguments", json.dumps(request))

    def test_live_and_typesafe_aliases_report_actual_typesafe_backend(self):
        task = FixtureFormTask("proof-token")
        sources = self.sources("proof-token")
        candidates = task.candidates(sources)
        result = SimpleNamespace(
            kind="ok",
            selected_id="type-verification-value",
            confidence=0.8,
            probabilities={
                "type-verification-value": 0.8,
                "reobserve": 0.1,
                "abstain": 0.1,
            },
        )
        fake_client = MagicMock()
        fake_client.__enter__.return_value = object()
        fake_client.__exit__.return_value = False
        with (
            patch("typesafe_sdk.TypeSafeClient", return_value=fake_client),
            patch.object(browser_provider, "TypeSafeDecisionModel", return_value=object()),
            patch.object(browser_provider, "choose", return_value=result) as choose_model,
        ):
            for provider in ("live", "typesafe"):
                choice, confidence, probabilities, backend = choose_browser_provider(
                    provider, task, sources, candidates, []
                )
                self.assertEqual(choice, "type-verification-value")
                self.assertEqual(confidence, 0.8)
                self.assertEqual(probabilities["type-verification-value"], 0.8)
                self.assertEqual(backend, "typesafe")
        self.assertEqual(choose_model.call_count, 2)


if __name__ == "__main__":
    unittest.main()