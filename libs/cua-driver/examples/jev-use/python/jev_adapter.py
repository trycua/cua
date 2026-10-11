from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Any, Mapping, Protocol

from core import Candidate, VisualObservation, choose_mock
from tasks import FIXTURE_GOAL, FixtureFormTask, Task, TaskSources, fixture_sources


class TypeSafeClientLike(Protocol):
    def system_one(self, **request: Any) -> Any: ...


@dataclass(frozen=True)
class ProviderChoice:
    selected_id: str
    confidence: float
    probabilities: dict[str, float]
    model: str | None


def choose_bounded_with_typesafe(
    client: TypeSafeClientLike,
    *,
    goal: str,
    observation: Mapping[str, Any],
    criteria: Mapping[str, str],
) -> ProviderChoice:
    from typesafe_sdk import Choice

    response = client.system_one(
        state={"observation": dict(observation)},
        questions={
            "candidate": Choice(
                instructions=goal,
                criteria=dict(criteria),
            )
        },
    )
    answer = response.choices["candidate"]
    if answer.choice not in criteria:
        raise ValueError(f"Jev selected unknown candidate: {answer.choice}")
    confidence = float(answer.confidence)
    if not math.isfinite(confidence) or not 0 <= confidence <= 1:
        raise ValueError("Jev returned invalid confidence")
    probabilities: dict[str, float] = {}
    for candidate_id, value in answer.probabilities.items():
        if candidate_id not in criteria:
            raise ValueError(f"Jev returned probability for unknown candidate: {candidate_id}")
        probability = float(value)
        if not math.isfinite(probability) or not 0 <= probability <= 1:
            raise ValueError("Jev returned invalid probability")
        probabilities[candidate_id] = probability
    model_value = getattr(response, "model", None)
    model = model_value if isinstance(model_value, str) and model_value.strip() else None
    return ProviderChoice(answer.choice, confidence, probabilities, model)


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


GOAL = FIXTURE_GOAL


def task_decision_state(
    task: Task, sources: TaskSources, history: list[dict[str, Any]]
) -> dict[str, Any]:
    """Build the compact, deterministic, secret-redacted state sent to Jev.

    ``form`` is the task's state summary, which states what the runner verified
    from its candidate sources, so the model does not have to infer it from the
    outline. Every secret task parameter is replaced everywhere, including the
    outline and visual text.
    """
    snapshot = sources.require_page().snapshot
    visual = sources.visual.observation if sources.visual is not None else None
    return {
        "goal": task.goal,
        "observation": {
            "page": task.redact(snapshot.get("page")),
            "form": task.state_summary(sources),
            "outline": task.redact(snapshot.get("outline")),
            "visual": task.redact(visual_decision_state(visual)),
        },
        "history": [dict(item) for item in history],
    }


def decision_state(
    snapshot: Mapping[str, Any],
    visual: VisualObservation | None,
    history: list[dict[str, Any]],
    token: str,
    visual_path: bool = False,
) -> dict[str, Any]:
    """Build the fixture task's decision state; see ``task_decision_state``."""
    return task_decision_state(
        FixtureFormTask(token),
        fixture_sources(snapshot, visual, visual_path=visual_path),
        history,
    )


DRIVER_ACTION_INSTRUCTIONS = "Which complete executable action should Cua Driver run next?"


def choose_for_task(
    client: TypeSafeClientLike,
    task: Task,
    sources: TaskSources,
    candidates: list[Candidate],
    history: list[dict[str, Any]],
) -> tuple[str, float, dict[str, float]]:
    from typesafe_sdk import Choice

    criteria = _candidate_criteria(candidates)
    response = client.system_one(
        state=task_decision_state(task, sources, history),
        questions={
            "driver_action": Choice(
                instructions=DRIVER_ACTION_INSTRUCTIONS,
                criteria=criteria,
            )
        },
    )
    answer = response.choices["driver_action"]
    if answer.choice not in criteria:
        raise ValueError(f"Jev selected unknown candidate: {answer.choice}")
    return answer.choice, answer.confidence, answer.probabilities


def choose_with_typesafe(
    client: TypeSafeClientLike,
    candidates: list[Candidate],
    snapshot: dict[str, Any],
    visual: VisualObservation | None,
    history: list[dict[str, Any]],
    token: str,
    visual_path: bool = False,
) -> tuple[str, float, dict[str, float]]:
    return choose_for_task(
        client,
        FixtureFormTask(token),
        fixture_sources(snapshot, visual, visual_path=visual_path),
        candidates,
        history,
    )


def choose_live_for_task(
    task: Task,
    sources: TaskSources,
    candidates: list[Candidate],
    history: list[dict[str, Any]],
) -> tuple[str, float, dict[str, float]]:
    from typesafe_sdk import TypeSafeClient

    with TypeSafeClient() as client:
        return choose_for_task(client, task, sources, candidates, history)


def choose_mock_for_task(
    task: Task,
    _sources: TaskSources,
    candidates: list[Candidate],
    _history: list[dict[str, Any]],
) -> tuple[str | None, float, dict[str, float]]:
    """Deterministic mock provider.

    A task may declare ``mock_preferences``: the first preferred ID present in
    the candidate set wins (a refused control's ``<id>:foreground`` variant
    counts as its ID), otherwise ``reobserve``. Tasks without preferences keep
    the fixed browser-fixture choice order.
    """
    preferences = getattr(task, "mock_preferences", ())
    if not preferences:
        return choose_mock(candidates)
    ids = [candidate.id for candidate in candidates]
    selected = next(
        (
            candidate_id
            for preferred in preferences
            for candidate_id in (preferred, f"{preferred}:foreground")
            if candidate_id in ids
        ),
        "reobserve" if "reobserve" in ids else None,
    )
    probabilities = {candidate_id: float(candidate_id == selected) for candidate_id in ids}
    return selected, 1.0 if selected else 0.0, probabilities


def choose_live(
    candidates: list[Candidate],
    snapshot: dict[str, Any],
    visual: VisualObservation | None,
    history: list[dict[str, Any]],
    token: str,
    visual_path: bool = False,
) -> tuple[str, float, dict[str, float]]:
    return choose_live_for_task(
        FixtureFormTask(token),
        fixture_sources(snapshot, visual, visual_path=visual_path),
        candidates,
        history,
    )


def choose_mock_adapter(
    candidates: list[Candidate],
    _snapshot: dict[str, Any],
    _visual: VisualObservation | None,
    _history: list[dict[str, Any]],
    _token: str,
    _visual_path: bool = False,
) -> tuple[str | None, float, dict[str, float]]:
    return choose_mock(candidates)
