from __future__ import annotations

import argparse
import asyncio
import contextlib
import io
import json
import sys
import tempfile
import unittest
from contextlib import asynccontextmanager
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import urlencode
from urllib.request import Request, urlopen

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))
sys.path.insert(0, str(BASE))

import run
from verify_setup import fixture


@asynccontextmanager
async def transport(_params):
    yield None, None


class FixtureSession:
    """In-memory MCP transport; the HTTP fixture remains the outcome oracle."""

    def __init__(self, url, token):
        self.url = url
        self.token = token
        self.value = ""
        self.mutations = []

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args):
        pass

    async def initialize(self):
        pass

    async def list_tools(self):
        return SimpleNamespace(tools=[])

    async def call_tool(self, name, args):
        if name == "browser_prepare":
            data = {"prepared_pid": 1}
        elif name == "list_windows":
            data = {
                "windows": [
                    {"window_id": 1, "is_on_screen": True, "bounds": {"width": 100, "height": 100}}
                ]
            }
        elif name == "get_browser_state":
            data = {
                "target_id": "target",
                "tabs": [{"tab_id": "tab"}],
                "tab_id": "tab",
                "refs": [
                    {
                        "role": "textbox",
                        "name": "verification value",
                        "ref": "p2:0",
                        "value": self.value,
                    },
                    {"role": "button", "name": "Submit", "ref": "p2:1" if self.value else "p1:1"},
                ],
            }
        elif name == "browser_type":
            self.value = args["text"]
            self.mutations.append(name)
            data = {}
        elif name == "browser_click":
            self.mutations.append(name)
            with urlopen(
                Request(self.url + "submit", data=urlencode({"value": self.value}).encode()),
                timeout=2,
            ):
                pass
            data = {}
        else:
            self.assert_navigation(name)
            data = {}
        return SimpleNamespace(isError=False, structuredContent=data)

    @staticmethod
    def assert_navigation(name):
        assert name == "browser_navigate", name


class GuardedRunnerTest(unittest.TestCase):
    def execute(
        self,
        *,
        guarded=True,
        tamper=False,
        provider_failure=False,
        no_choice=False,
        reobserve=False,
        action_failure=False,
        expected="verified",
    ):
        token = "private-field-canary-4316"
        with tempfile.TemporaryDirectory() as directory, fixture() as url:
            session = FixtureSession(url, token)
            args = argparse.Namespace(
                token=token,
                fixture_url=url,
                max_steps=4,
                log=str(Path(directory) / "run.jsonl"),
                provider="mock",
                guarded_completion=guarded,
                visual_observation="off",
                dry_run=False,
            )
            if action_failure:
                original_call = session.call_tool

                async def call(name, args):
                    if name == "browser_click":
                        raise RuntimeError(token)
                    return await original_call(name, args)

                session.call_tool = call
            original = run.plan_guarded_completion

            def plan(*values, **options):
                result = original(*values, **options)
                return replace(result, session="foreign-session") if tamper and result else result

            original_choose = run.choose_mock_for_task
            forced = False

            def choose(*values):
                nonlocal forced
                if session.value and not forced:
                    forced = True
                    if provider_failure:
                        raise RuntimeError(token)
                    if no_choice:
                        return None, 0.0, {}
                    if reobserve:
                        return "reobserve", 1.0, {"reobserve": 1.0}
                return original_choose(*values)

            with (
                patch.object(run, "stdio_client", transport),
                patch.object(run, "ClientSession", return_value=session),
                patch.object(run, "plan_guarded_completion", side_effect=plan),
                patch.object(run, "choose_mock_for_task", side_effect=choose) as provider,
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(asyncio.run(run.run(args)), expected)
            text = Path(args.log).read_text()
            self.assertNotIn(token, text)
            return (
                [json.loads(line) for line in text.splitlines()],
                provider.call_count,
                session.mutations,
            )

    def test_accepted_proof_is_emitted_without_model_calibration_or_field_value(self):
        events, calls, mutations = self.execute()
        steps = [event for event in events if event["event"] == "step"]
        self.assertEqual(calls, 1)
        self.assertEqual(mutations, ["browser_type", "browser_click"])
        self.assertNotIn("guarded_completion", steps[0])
        self.assertEqual(
            [step["decision_route"] for step in steps], ["provider", "guarded-completion"]
        )
        accepted = steps[1]
        proof = accepted["guarded_completion"]
        self.assertEqual(
            proof,
            {
                "status": "accepted",
                "prior_ref": "p1:1",
                "fresh_ref": "p2:1",
                "verification_field": "contains_required_token",
                "submit_matches": 1,
                "session": proof["session"],
            },
        )
        self.assertTrue(proof["session"].startswith("jev-python-"))
        self.assertEqual(accepted["provider_decision_ms"], 0)
        self.assertIsNone(accepted["confidence"])
        self.assertIsNone(accepted["probabilities"])

    def test_decline_emits_reason_on_provider_fallback(self):
        events, calls, _ = self.execute(tamper=True)
        steps = [event for event in events if event["event"] == "step"]
        self.assertEqual(calls, 2)
        self.assertEqual([step["decision_route"] for step in steps], ["provider", "provider"])
        self.assertEqual(
            steps[1]["guarded_completion"], {"status": "declined", "reason": "session_mismatch"}
        )
        self.assertIsInstance(steps[1]["confidence"], float)
        self.assertIsInstance(steps[1]["probabilities"], dict)

    def test_provider_failure_keeps_declined_proof_visible(self):
        events, calls, _ = self.execute(tamper=True, provider_failure=True, expected="unknown")
        self.assertEqual(calls, 2)
        self.assertEqual(
            events[-1],
            {
                "event": "outcome",
                "outcome": "unknown",
                "step": 2,
                "phase": "provider",
                "decision_route": "provider",
                "error": "RuntimeError",
                "guarded_completion": {"status": "declined", "reason": "session_mismatch"},
                "visual": {"status": "skipped", "reason": "disabled"},
            },
        )

    def test_provider_no_choice_keeps_decline_visible(self):
        events, calls, _ = self.execute(tamper=True, no_choice=True, expected="abstained")
        self.assertEqual(calls, 2)
        self.assertEqual(
            events[-1]["guarded_completion"], {"status": "declined", "reason": "session_mismatch"}
        )
        self.assertEqual(events[-1]["decision_route"], "provider")

    def test_declined_plan_is_consumed_once_across_reobserve(self):
        events, calls, _ = self.execute(tamper=True, reobserve=True)
        steps = [event for event in events if event["event"] == "step"]
        self.assertEqual(calls, 3)
        self.assertEqual(
            [step["candidate"] for step in steps],
            ["type-verification-value", "reobserve", "submit-form"],
        )
        self.assertEqual(steps[1]["guarded_completion"]["reason"], "session_mismatch")
        self.assertNotIn("guarded_completion", steps[2])

    def test_action_failure_retains_proof_without_error_content(self):
        for tamper in (False, True):
            with self.subTest(tamper=tamper):
                events, _, _ = self.execute(tamper=tamper, action_failure=True, expected="unknown")
                self.assertEqual(events[-1]["phase"], "action")
                self.assertEqual(
                    events[-1]["guarded_completion"]["status"], "declined" if tamper else "accepted"
                )

    def test_default_off_never_records_an_attempted_guard(self):
        events, calls, _ = self.execute(guarded=False)
        self.assertEqual(calls, 2)
        self.assertTrue(all("guarded_completion" not in event for event in events))