"""Browser-runner provider choices and the backend each one reports.

The runner still owns candidate construction, action arguments, freshness, and
verification. ``live`` keeps the TypeSafe request pinned by the provider-request
golden files; ``s1`` receives only the validated closed-choice request.
"""

from __future__ import annotations

from typing import Any

from choose_action import REQUEST_SCHEMA, validate_request
from jev_adapter import visual_decision_state
from tasks import Task, TaskSources

BACKENDS = {"mock": "mock", "live": "typesafe", "s1": "s1"}


def browser_decision_request(
    task: Task,
    sources: TaskSources,
    candidates: list[Any],
    history: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build the bounded provider request from one fresh browser decision step."""
    page = sources.require_page().snapshot
    visual = sources.visual.observation if sources.visual is not None else None
    if visual is not None:
        capture_id = visual.capture_id
    elif isinstance(page.get("capture_id"), str) and page["capture_id"]:
        capture_id = page["capture_id"]
    else:
        capture_id = f"browser:{page['target_id']}:{page['tab_id']}"
    visual_state = visual_decision_state(visual)
    regions = visual_state["regions"] if visual_state is not None else []
    request = {
        "schema": REQUEST_SCHEMA,
        "goal": task.goal,
        "capture_id": capture_id,
        "regions": task.redact(regions),
        "history": [
            {key: item[key] for key in ("selected_id", "outcome") if item.get(key)}
            for item in history
        ],
        "candidates": [
            {"id": candidate.id, "description": candidate.description} for candidate in candidates
        ],
    }
    return validate_request(request)
