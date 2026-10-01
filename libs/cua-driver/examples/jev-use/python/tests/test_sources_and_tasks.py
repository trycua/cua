from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import REDACTED_TOKEN, parse_visual_regions
from sources import BrowserSemanticSource, CandidateSource, VisualRegionSource
from tasks import (
    FIXTURE_GOAL,
    SUBMIT_IDS,
    FixtureFormTask,
    Task,
    TaskSources,
    fixture_sources,
)

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"
PAGE = json.loads((FIXTURES / "jev-page-structure-replay-v1.json").read_text(encoding="utf-8"))
VISUAL = json.loads((FIXTURES / "jev-visual-replay-v1.json").read_text(encoding="utf-8"))
PAYLOAD = VISUAL["visual_regions"]


def observation():
    source = PAYLOAD["capture"]["source"]
    return parse_visual_regions(
        PAYLOAD,
        expected_capture_id=PAYLOAD["capture"]["capture_id"],
        expected_pid=source["pid"],
        expected_window_id=source["window_id"],
    )


class CandidateSourceTest(unittest.TestCase):
    def test_sources_implement_one_interface_with_distinct_kinds(self) -> None:
        page: CandidateSource = BrowserSemanticSource(PAGE["snapshots"]["before_typing"])
        visual: CandidateSource = VisualRegionSource(observation())
        self.assertEqual((page.kind, visual.kind), ("page", "visual"))

    def test_page_source_addresses_refs_with_exact_browser_arguments(self) -> None:
        snapshot = PAGE["snapshots"]["after_typing"]
        page = BrowserSemanticSource(snapshot)
        button = page.find("button", "Submit")
        self.assertIsNotNone(button)
        self.assertEqual((button.source, button.handle["ref"]), ("page", "p4:2"))
        candidate = page.click(button, candidate_id="c", description="d")
        self.assertEqual(candidate.tool, "browser_click")
        self.assertEqual(
            dict(candidate.arguments),
            {
                "target_id": snapshot["target_id"],
                "tab_id": snapshot["tab_id"],
                "ref": "p4:2",
                "input_route": "dom_event",
            },
        )
        self.assertIsNone(page.find("button", "Cancel"))

    def test_visual_source_clicks_only_capture_bound_and_never_types(self) -> None:
        visual = observation()
        source = VisualRegionSource(visual)
        submit = source.find("button", "submit")
        self.assertIsNotNone(submit)
        self.assertIsNone(source.click(submit, candidate_id="c", description="d"))
        self.assertIsNone(source.type_text(submit, "x", candidate_id="c", description="d"))
        bound = VisualRegionSource(visual, "foreground", capture_bound=True)
        candidate = bound.click(submit, candidate_id="c", description="d")
        self.assertEqual(candidate.tool, "click")
        self.assertEqual(candidate.capture_id, visual.capture_id)
        self.assertEqual(candidate.arguments["delivery_mode"], "foreground")
        self.assertEqual(
            (candidate.arguments["x"], candidate.arguments["y"]),
            visual.screenshot_center(submit.handle),
        )


class FixtureTaskSpecTest(unittest.TestCase):
    def test_built_in_task_declares_its_spec(self) -> None:
        task: Task = FixtureFormTask("secret-token", "http://127.0.0.1:9/", 3)
        self.assertEqual(task.goal, FIXTURE_GOAL)
        self.assertEqual(task.max_steps, 3)
        self.assertEqual(task.completion_candidate_ids, SUBMIT_IDS)
        self.assertEqual(task.allowed_action_kinds, {"browser_type", "browser_click", "click"})
        [parameter] = task.parameters
        self.assertTrue(parameter.secret)
        self.assertEqual((parameter.value, parameter.redaction), ("secret-token", REDACTED_TOKEN))
        self.assertEqual(
            task.redact({"a": ["x secret-token y"]}), {"a": [f"x {REDACTED_TOKEN} y"]}
        )

    def test_oracle_classification_uses_the_task_budget(self) -> None:
        task = FixtureFormTask("t", max_steps=2)
        self.assertEqual(task.classify({"submitted": "t"}, steps=0), "verified")
        self.assertEqual(task.classify({"submitted": "other"}, steps=0), "refuted")
        self.assertEqual(task.classify({"submitted": None}, steps=1), "unknown")
        self.assertEqual(task.classify({"submitted": None}, steps=2), "budget_exhausted")

    def test_candidates_outside_the_allowed_action_kinds_are_refused(self) -> None:
        task = FixtureFormTask(PAGE["token"])
        object.__setattr__(task, "allowed_action_kinds", frozenset({"browser_type"}))
        sources = fixture_sources(PAGE["snapshots"]["after_typing"])
        with self.assertRaisesRegex(ValueError, "browser_click"):
            task.candidates(sources)
        before = task.candidates(fixture_sources(PAGE["snapshots"]["before_typing"]))
        self.assertEqual(before[0].tool, "browser_type")

    def test_state_summary_reads_the_step_sources(self) -> None:
        task = FixtureFormTask(VISUAL["token"])
        after = VISUAL["snapshots"]["after_typing"]
        pending = TaskSources(BrowserSemanticSource(after), None, visual_path=True)
        self.assertEqual(task.state_summary(pending)["submit_button"], "visual_check_pending")
        parsed = TaskSources(
            BrowserSemanticSource(after), VisualRegionSource(observation()), visual_path=True
        )
        self.assertEqual(
            task.state_summary(parsed),
            {"verification_field": "contains_required_token", "submit_button": "visual_only"},
        )


if __name__ == "__main__":
    unittest.main()
