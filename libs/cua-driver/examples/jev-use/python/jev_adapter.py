from __future__ import annotations

from typing import Any, Protocol

from core import Candidate, VisualObservation, choose_mock


class TypeSafeClientLike(Protocol):
    def system_one(self, **request: Any) -> Any: ...


def _candidate_criteria(candidates: list[Candidate]) -> dict[str, str]:
    criteria = {candidate.id: candidate.description for candidate in candidates}
    if len(criteria) != len(candidates):
        raise ValueError("candidate set contains duplicate IDs")
    return criteria


def visual_decision_state(visual: VisualObservation | None) -> dict[str, Any] | None:
    if visual is None:
        return None
    return {
        "schema": "cua.visual_regions_v1",
        "capture_id": visual.capture_id,
        "screenshot_reference": visual.screenshot_reference,
        "source": {
            "kind": "window",
            "pid": visual.pid,
            "window_id": visual.window_id,
        },
        "regions": [
            {
                "id": region.id,
                "kind": region.kind,
                "text": region.text,
                "label": region.label,
                "confidence": region.confidence,
                "interactive": region.interactive,
                "bounds": {
                    "x": region.x,
                    "y": region.y,
                    "width": region.width,
                    "height": region.height,
                },
            }
            for region in visual.regions
        ],
    }


def choose_with_typesafe(
    client: TypeSafeClientLike,
    candidates: list[Candidate],
    snapshot: dict[str, Any],
    visual: VisualObservation | None,
    history: list[dict[str, Any]],
) -> tuple[str, float, dict[str, float]]:
    from typesafe_sdk import Choice

    criteria = _candidate_criteria(candidates)
    state = {
        "goal": "Enter the verification token, then submit the form.",
        "observation": {
            "page": snapshot.get("page"),
            "outline": snapshot.get("outline"),
            "visual": visual_decision_state(visual),
        },
        "history": history,
    }
    response = client.system_one(
        state=state,
        questions={
            "driver_action": Choice(
                instructions="Which complete executable action should Cua Driver run next?",
                criteria=criteria,
            )
        },
    )
    answer = response.choices["driver_action"]
    if answer.choice not in criteria:
        raise ValueError(f"Jev selected unknown candidate: {answer.choice}")
    return answer.choice, answer.confidence, answer.probabilities


def choose_live(
    candidates: list[Candidate],
    snapshot: dict[str, Any],
    visual: VisualObservation | None,
    history: list[dict[str, Any]],
) -> tuple[str, float, dict[str, float]]:
    from typesafe_sdk import TypeSafeClient

    with TypeSafeClient() as client:
        return choose_with_typesafe(client, candidates, snapshot, visual, history)


def choose_mock_adapter(
    candidates: list[Candidate],
    _snapshot: dict[str, Any],
    _visual: VisualObservation | None,
    _history: list[dict[str, Any]],
) -> tuple[str | None, float, dict[str, float]]:
    return choose_mock(candidates)
