"""Caller-owned exact-text authorization for a capture-bound visual action.

This is an example policy, not a decision-model guarantee or Driver API.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from typing import Any, Mapping


class ActionAuthorizationError(ValueError):
    """The selected candidate is not authorized by the current observation."""


@dataclass(frozen=True)
class AuthorizedVisualClick:
    capture_id: str
    x: float
    y: float


@dataclass(frozen=True)
class ExactRegionTextAction:
    candidate_id: str
    region_id: str
    exact_text: str
    min_confidence: float = 0.8

    def __post_init__(self) -> None:
        if not all(
            isinstance(value, str) and value.strip()
            for value in (self.candidate_id, self.region_id, self.exact_text)
        ):
            raise ValueError("action identifiers and exact text must be nonempty strings")
        if (
            not isinstance(self.min_confidence, (int, float))
            or isinstance(self.min_confidence, bool)
            or not math.isfinite(self.min_confidence)
            or not 0 <= self.min_confidence <= 1
        ):
            raise ValueError("minimum confidence must be a finite value in [0, 1]")
        if round(float(self.min_confidence), 2) != float(self.min_confidence):
            raise ValueError("minimum confidence must use at most two decimal places")

    def wire_candidate(self) -> dict[str, str]:
        return {
            "id": self.candidate_id,
            "description": (
                "Select only if exactly one supplied text region has "
                f"id={json.dumps(self.region_id)}, "
                f"exact_text={json.dumps(self.exact_text, ensure_ascii=False)}, "
                f"and confidence at or above {self.min_confidence:.2f}."
            ),
        }


def authorize_exact_region_text_action(
    request: Mapping[str, Any],
    decision: Mapping[str, Any],
    parsed: Mapping[str, Any],
    action: ExactRegionTextAction,
    *,
    current_capture_id: str,
) -> AuthorizedVisualClick:
    """Raise unless the original parse still proves the selected action condition.

    The caller supplies the returned point and capture ID to Driver once, then
    independently verifies the result. This helper does not dispatch input.
    """
    if not all(isinstance(value, Mapping) for value in (request, decision, parsed)):
        raise ActionAuthorizationError("missing decision or visual observation")
    if not isinstance(action, ExactRegionTextAction):
        raise ActionAuthorizationError("unsupported action condition")
    capture = parsed.get("capture")
    capture_id = capture.get("capture_id") if isinstance(capture, Mapping) else None
    if (
        not isinstance(current_capture_id, str)
        or not current_capture_id
        or request.get("capture_id") != current_capture_id
        or decision.get("capture_id") != current_capture_id
        or capture_id != current_capture_id
    ):
        raise ActionAuthorizationError("capture identity changed")
    if parsed.get("schema") != "cua.visual_regions_v1":
        raise ActionAuthorizationError("unsupported visual observation")
    if (
        decision.get("schema") != "cua.decision_choice_v1"
        or decision.get("kind") != "selected"
        or decision.get("selected_id") != action.candidate_id
    ):
        raise ActionAuthorizationError("decision did not select this action")

    candidates = request.get("candidates")
    if (
        not isinstance(candidates, list)
        or sum(
            isinstance(candidate, Mapping) and candidate.get("id") == action.candidate_id
            for candidate in candidates
        )
        != 1
        or action.wire_candidate() not in candidates
    ):
        raise ActionAuthorizationError("action condition differs from the offered candidate")
    scores = decision.get("probabilities")
    candidate_ids = [item.get("id") for item in candidates if isinstance(item, Mapping)]
    if (
        len(candidate_ids) != len(candidates)
        or any(not isinstance(candidate_id, str) for candidate_id in candidate_ids)
        or len(set(candidate_ids)) != len(candidate_ids)
        or not isinstance(scores, Mapping)
        or set(scores) != set(candidate_ids)
        or any(
            not isinstance(score, (int, float))
            or isinstance(score, bool)
            or not math.isfinite(score)
            or not 0 <= score <= 1
            for score in scores.values()
        )
        or abs(sum(scores.values()) - 1) > 0.02
        or scores[action.candidate_id] != max(scores.values())
    ):
        raise ActionAuthorizationError("decision scores do not match the candidate set")

    regions = parsed.get("regions")
    offered_regions = request.get("regions")
    if not isinstance(regions, list) or not isinstance(offered_regions, list):
        raise ActionAuthorizationError("visual regions are unavailable")
    matches: list[Mapping[str, Any]] = []
    region_ids: set[str] = set()
    for region in regions:
        if not isinstance(region, Mapping) or not isinstance(region.get("id"), str):
            raise ActionAuthorizationError("malformed visual region")
        if region["id"] in region_ids:
            raise ActionAuthorizationError("duplicate visual region ID")
        region_ids.add(region["id"])
        confidence = region.get("confidence")
        if (
            not isinstance(confidence, (int, float))
            or isinstance(confidence, bool)
            or not math.isfinite(confidence)
            or not 0 <= confidence <= 1
        ):
            raise ActionAuthorizationError("malformed visual confidence")
        if (
            region.get("kind") == "text"
            and region.get("text") == action.exact_text
            and confidence >= action.min_confidence
        ):
            matches.append(region)
    if len(matches) != 1 or matches[0]["id"] != action.region_id:
        raise ActionAuthorizationError("exact-text condition is not uniquely satisfied")
    if (
        sum(
            isinstance(region, Mapping)
            and region.get("id") == action.region_id
            and region == matches[0]
            for region in offered_regions
        )
        != 1
    ):
        raise ActionAuthorizationError("offered region differs from the original parse")
    bounds = matches[0].get("bounds")
    if (
        not isinstance(bounds, Mapping)
        or any(
            not isinstance(bounds.get(key), int) or isinstance(bounds[key], bool)
            for key in ("x", "y", "width", "height")
        )
        or bounds["x"] < 0
        or bounds["y"] < 0
        or bounds["width"] <= 0
        or bounds["height"] <= 0
    ):
        raise ActionAuthorizationError("malformed visual bounds")
    return AuthorizedVisualClick(
        current_capture_id,
        bounds["x"] + bounds["width"] / 2,
        bounds["y"] + bounds["height"] / 2,
    )
