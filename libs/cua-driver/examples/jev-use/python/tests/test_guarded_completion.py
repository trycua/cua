from __future__ import annotations

import sys
import unittest
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))

from guarded_completion import plan_guarded_completion, resolve_guarded_completion
from sources import Candidate
from tasks import FixtureFormTask, fixture_sources


def snapshot(value: str, submit_refs: list[str]) -> dict:
    refs = [
        {
            "role": "textbox",
            "name": "verification value",
            "ref": "p1:0",
            "value": value,
        }
    ]
    refs.extend({"role": "button", "name": "Submit", "ref": ref} for ref in submit_refs)
    return {"target_id": "target", "tab_id": "tab", "refs": refs}


class GuardedCompletionTest(unittest.TestCase):
    def setUp(self) -> None:
        self.task = FixtureFormTask("proof")

    def initial(self):
        sources = fixture_sources(snapshot("", ["p1:1"]))
        selected = self.task.candidates(sources)[0]
        self.assertEqual(selected.id, "type-verification-value")
        return sources, selected

    def test_fresh_unique_completion_uses_only_the_fresh_ref(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        self.assertIsNotNone(plan)
        assert plan is not None

        fresh_sources = fixture_sources(snapshot("proof", ["p2:1"]))
        candidates = self.task.candidates(fresh_sources)
        result = resolve_guarded_completion(
            plan, self.task, fresh_sources, candidates, session="session-a"
        )
        self.assertEqual(
            result.telemetry,
            {
                "status": "accepted",
                "prior_ref": "p1:1",
                "fresh_ref": "p2:1",
                "verification_field": "contains_required_token",
                "submit_matches": 1,
                "session": "session-a",
            },
        )
        completion = result.candidate
        self.assertIsNotNone(completion)
        assert completion is not None
        self.assertEqual(completion.id, "submit-form")
        self.assertEqual(completion.arguments["ref"], "p2:1")
        self.assertNotEqual(completion.arguments["ref"], plan.prior_ref)

    def assert_declined(self, result, reason):
        self.assertIsNone(result.candidate)
        self.assertEqual(result.telemetry, {"status": "declined", "reason": reason})

    def test_wrong_session_fails_closed(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        assert plan is not None
        fresh_sources = fixture_sources(snapshot("proof", ["p2:1"]))
        for session in ("session-b", ""):
            with self.subTest(session=session):
                self.assert_declined(
                    resolve_guarded_completion(
                        plan,
                        self.task,
                        fresh_sources,
                        self.task.candidates(fresh_sources),
                        session=session,
                    ),
                    "session_mismatch",
                )

    def test_ambiguous_or_missing_target_fails_closed(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        assert plan is not None
        for refs in ([], ["p2:1", "p2:2"]):
            with self.subTest(refs=refs):
                fresh_sources = fixture_sources(snapshot("proof", refs))
                self.assert_declined(
                    resolve_guarded_completion(
                        plan,
                        self.task,
                        fresh_sources,
                        self.task.candidates(fresh_sources),
                        session="session-a",
                    ),
                    "submit_not_unique",
                )

    def test_unverified_first_postcondition_fails_closed(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        assert plan is not None
        fresh_sources = fixture_sources(snapshot("other", ["p2:1"]))
        self.assert_declined(
            resolve_guarded_completion(
                plan,
                self.task,
                fresh_sources,
                self.task.candidates(fresh_sources),
                session="session-a",
            ),
            "field_not_proven",
        )

    def test_old_or_tampered_ref_never_gains_authority(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        assert plan is not None
        reused_sources = fixture_sources(snapshot("proof", [plan.prior_ref]))
        self.assert_declined(
            resolve_guarded_completion(
                plan,
                self.task,
                reused_sources,
                self.task.candidates(reused_sources),
                session="session-a",
            ),
            "ref_reused",
        )

        fresh_sources = fixture_sources(snapshot("proof", ["p2:1"]))
        bad = Candidate(
            "submit-form",
            "tampered",
            "browser_click",
            {"target_id": "target", "tab_id": "tab", "ref": plan.prior_ref},
            source="page",
        )
        self.assert_declined(
            resolve_guarded_completion(plan, self.task, fresh_sources, [bad], session="session-a"),
            "candidate_mismatch",
        )

    def test_wrong_task_or_missing_page_has_a_specific_reason(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        assert plan is not None
        fresh = fixture_sources(snapshot("proof", ["p2:1"]))
        candidates = self.task.candidates(fresh)
        wrong_task = SimpleNamespace(id="other-task")
        self.assert_declined(
            resolve_guarded_completion(plan, wrong_task, fresh, candidates, session="session-a"),
            "task_mismatch",
        )
        self.task = FixtureFormTask("proof")
        self.assert_declined(
            resolve_guarded_completion(
                plan, self.task, replace(fresh, page=None), candidates, session="session-a"
            ),
            "page_missing",
        )

    def test_completion_candidate_must_be_unique_and_executable(self) -> None:
        sources, selected = self.initial()
        plan = plan_guarded_completion(self.task, sources, selected, session="session-a")
        assert plan is not None
        fresh = fixture_sources(snapshot("proof", ["p2:1"]))
        good = next(c for c in self.task.candidates(fresh) if c.id == "submit-form")
        for candidates in (
            [],
            [good, good],
            [replace(good, tool="browser_type")],
            [replace(good, source="visual")],
            [replace(good, id="other")],
        ):
            with self.subTest(candidates=candidates):
                self.assert_declined(
                    resolve_guarded_completion(
                        plan, self.task, fresh, candidates, session="session-a"
                    ),
                    "candidate_not_unique",
                )

    def test_plan_requires_unique_initial_logical_target(self) -> None:
        for refs in ([], ["p1:1", "p1:2"]):
            with self.subTest(refs=refs):
                sources = fixture_sources(snapshot("", refs))
                selected = self.task.candidates(sources)[0]
                self.assertIsNone(
                    plan_guarded_completion(self.task, sources, selected, session="session-a")
                )


if __name__ == "__main__":
    unittest.main()