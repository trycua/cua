"""Narrow caller-side guarded completion for the built-in browser fixture.

This is intentionally recipe-local. A provider still chooses the first mutation.
After that mutation, the caller may skip exactly one provider decision only when
all of the completion facts are re-proven from a fresh semantic snapshot.

The plan binds the Cua session plus the logical completion target. It never
carries a page ref forward as action authority: the second action must use the
fresh ref minted by the post-mutation snapshot.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping

from sources import Candidate
from tasks import FIXTURE_TASK_ID, SUBMIT_NAME, Task, TaskSources

TYPE_CANDIDATE_ID = "type-verification-value"
SUBMIT_CANDIDATE_ID = "submit-form"


@dataclass(frozen=True)
class GuardedCompletionPlan:
    session: str
    first_candidate_id: str
    completion_candidate_id: str
    target_role: str
    target_name: str
    prior_ref: str


@dataclass(frozen=True)
class GuardedCompletionResult:
    candidate: Candidate | None
    telemetry: dict[str, Any]


def _matching_refs(
    snapshot: Mapping[str, Any],
    *,
    role: str,
    name: str,
) -> list[Mapping[str, Any]]:
    refs = snapshot.get("refs") or []
    if not isinstance(refs, list):
        return []
    return [
        ref
        for ref in refs
        if isinstance(ref, Mapping)
        and ref.get("role") == role
        and ref.get("name") == name
        and isinstance(ref.get("ref"), str)
        and bool(ref.get("ref"))
    ]


def plan_guarded_completion(
    task: Task,
    sources: TaskSources,
    selected: Candidate,
    *,
    session: str,
) -> GuardedCompletionPlan | None:
    """Bind one known completion after the provider selects the first mutation."""
    if (
        not session
        or task.id != FIXTURE_TASK_ID
        or selected.id != TYPE_CANDIDATE_ID
        or selected.tool != "browser_type"
        or selected.source != "page"
        or sources.page is None
    ):
        return None
    matches = _matching_refs(sources.page.snapshot, role="button", name=SUBMIT_NAME)
    if len(matches) != 1:
        return None
    return GuardedCompletionPlan(
        session=session,
        first_candidate_id=selected.id,
        completion_candidate_id=SUBMIT_CANDIDATE_ID,
        target_role="button",
        target_name=SUBMIT_NAME,
        prior_ref=str(matches[0]["ref"]),
    )


def resolve_guarded_completion(
    plan: GuardedCompletionPlan,
    task: Task,
    sources: TaskSources,
    candidates: list[Candidate],
    *,
    session: str,
) -> GuardedCompletionResult:
    """Return fresh authority and redacted proof, or a stable decline reason."""

    def declined(reason: str) -> GuardedCompletionResult:
        return GuardedCompletionResult(None, {"status": "declined", "reason": reason})

    if not session or session != plan.session:
        return declined("session_mismatch")
    if task.id != FIXTURE_TASK_ID:
        return declined("task_mismatch")
    if sources.page is None:
        return declined("page_missing")

    state = task.state_summary(sources)
    if state.get("verification_field") != "contains_required_token":
        return declined("field_not_proven")

    matches = _matching_refs(
        sources.page.snapshot,
        role=plan.target_role,
        name=plan.target_name,
    )
    if len(matches) != 1:
        return declined("submit_not_unique")
    fresh_ref = str(matches[0]["ref"])
    if fresh_ref == plan.prior_ref:
        return declined("ref_reused")

    executable = [
        candidate
        for candidate in candidates
        if candidate.id == plan.completion_candidate_id
        and candidate.tool == "browser_click"
        and candidate.source == "page"
    ]
    if len(executable) != 1:
        return declined("candidate_not_unique")
    candidate = executable[0]
    if candidate.arguments.get("ref") != fresh_ref:
        return declined("candidate_mismatch")
    return GuardedCompletionResult(
        candidate,
        {
            "status": "accepted",
            "prior_ref": plan.prior_ref,
            "fresh_ref": fresh_ref,
            "verification_field": "contains_required_token",
            "submit_matches": len(matches),
            "session": session,
        },
    )