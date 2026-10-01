"""Browser-runner provider parity on the existing bounded decision seam.

The runner still owns candidate construction, action arguments, freshness, and
verification. Providers receive only a validated closed-choice request.
"""

from __future__ import annotations

from typing import Any, Literal

from choose_action import REQUEST_SCHEMA, validate_request
from decision_models import DecisionRequest, TypeSafeDecisionModel, choose
from jev_adapter import choose_mock_for_task, visual_decision_state
from s1_service import choose_s1_service
from tasks import Task, TaskSources

BrowserProvider = Literal["mock", "live", "typesafe", "s1"]


def backend_name(provider: BrowserProvider) -> str:
    return "typesafe" if provider in {"live", "typesafe"} else provider


def browser_decision_request(
    task: Task,
    sources: TaskSources,
    candidates: list[Any],
    history: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build the bounded provider request from one fresh browser decision step."""
    page = sources.require_page().snapshot
    visual = sources.visual.observation if sources.visual is not None else None
    capture_id = (
        visual.capture_id
        if visual is not None
        else page.get("capture_id")
        if isinstance(page.get("capture_id"), str) and page.get("capture_id")
        else f"browser:{page['target_id']}:{page['tab_id']}"
    )
    visual_state = visual_decision_state(visual)
    regions = visual_state["regions"] if visual_state is not None else []
    compact_history = [
        {
            key: item[key]
            for key in ("selected_id", "outcome")
            if isinstance(item.get(key), str) and item.get(key)
        }
        for item in history
    ]
    request = {
        "schema": REQUEST_SCHEMA,
        "goal": task.goal,
        "capture_id": capture_id,
        "regions": task.redact(regions),
        "history": compact_history,
        "candidates": [
            {"id": candidate.id, "description": candidate.description}
            for candidate in candidates
        ],
    }
    return validate_request(request)


def choose_browser_provider(
    provider: BrowserProvider,
    task: Task,
    sources: TaskSources,
    candidates: list[Any],
    history: list[dict[str, Any]],
) -> tuple[str | None, float, dict[str, float], str]:
    """Choose one supplied ID and report the backend that handled the request."""
    backend = backend_name(provider)
    if provider == "mock":
        choice, confidence, probabilities = choose_mock_for_task(
            task, sources, candidates, history
        )
        return choice, confidence, probabilities, backend

    request = browser_decision_request(task, sources, candidates, history)
    if provider == "s1":
        choice, confidence, probabilities = choose_s1_service(request)
        return choice, confidence, probabilities, backend

    decision_request = DecisionRequest.from_validated(request)
    from typesafe_sdk import TypeSafeClient

    with TypeSafeClient() as client:
        result = choose(TypeSafeDecisionModel(client), decision_request)
    if result.kind == "error":
        raise RuntimeError(f"{backend} decision failed")
    return (
        result.selected_id,
        float(result.confidence or 0.0),
        dict(result.probabilities),
        backend,
    )