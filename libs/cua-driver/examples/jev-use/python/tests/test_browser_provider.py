from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))

from browser_provider import BACKENDS, browser_decision_request
from tasks import FixtureFormTask, fixture_sources

TOKEN = "secret-proof-token"


def fixture():
    snapshot = {
        "target_id": "target",
        "tab_id": "tab",
        "capture_id": "browser-capture-1",
        "refs": [
            {"role": "textbox", "name": "verification value", "ref": "p1:0", "value": ""},
            {"role": "button", "name": "Submit", "ref": "p1:1"},
        ],
    }
    task = FixtureFormTask(TOKEN)
    sources = fixture_sources(snapshot)
    return task, sources, task.candidates(sources)


class BrowserDecisionRequestTest(unittest.TestCase):
    def test_request_is_bounded_and_carries_no_action_arguments_or_secret(self):
        task, sources, candidates = fixture()
        history = [task.history_entry(1, "reobserve")]
        request = browser_decision_request(task, sources, candidates, history)
        self.assertEqual(request["schema"], "cua.jev_choice_request_v1")
        self.assertEqual(request["capture_id"], "browser-capture-1")
        self.assertEqual(
            [item["id"] for item in request["candidates"]],
            ["type-verification-value", "reobserve", "abstain"],
        )
        self.assertEqual(list(request["history"][0]), ["selected_id", "outcome"])
        wire = json.dumps(request)
        self.assertNotIn(TOKEN, wire)
        self.assertNotIn("arguments", wire)
        self.assertNotIn("browser_type", wire)
        self.assertNotIn("p1:0", wire)

    def test_capture_id_falls_back_to_the_browser_tab(self):
        task, sources, candidates = fixture()
        del sources.require_page().snapshot["capture_id"]
        request = browser_decision_request(task, sources, candidates, [])
        self.assertEqual(request["capture_id"], "browser:target:tab")

    def test_each_provider_names_its_backend(self):
        self.assertEqual(BACKENDS, {"mock": "mock", "live": "typesafe", "s1": "s1"})


class S1ArgumentTest(unittest.TestCase):
    def test_s1_without_service_url_fails_before_touching_the_browser(self):
        env = {"PATH": "/usr/bin:/bin", "CUA_DRIVER_BIN": "/nonexistent/cua-driver"}
        result = subprocess.run(
            [sys.executable, str(BASE / "python" / "run.py"), "--provider", "s1"],
            capture_output=True,
            text=True,
            env=env,
            timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CUA_S1_DECISION_URL is not set", result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
