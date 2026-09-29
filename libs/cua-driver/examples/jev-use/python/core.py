"""Runner primitives shared by every jev-use task.

Visual-region parsing, candidate validation, and the mock chooser live here.
Candidate sources are in ``sources.py`` and task specs in ``tasks.py``; the
names re-exported below keep existing imports from ``core`` working.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Any, Literal, Mapping

from action_policy import (
    AffineCoefficients,
    CoordinateMappingError,
    action_coordinate_mapping,
    map_screenshot_point,
)
from sources import Candidate
from tasks import (
    FIELD_NAME,
    HISTORY_OUTCOMES,
    REDACTED_TOKEN,
    SUBMIT_IDS,
    SUBMIT_NAME,
    Outcome,
    build_candidates,
    classify,
    form_state,
    history_entry,
    redact_token,
    visual_submit_region,
)

__all__ = [
    "FIELD_NAME",
    "HISTORY_OUTCOMES",
    "REDACTED_TOKEN",
    "SUBMIT_IDS",
    "SUBMIT_NAME",
    "Candidate",
    "Outcome",
    "VisualDelivery",
    "VisualObservation",
    "VisualObservationError",
    "VisualRegion",
    "build_candidates",
    "choose_mock",
    "classify",
    "form_state",
    "has_executable_candidate",
    "history_entry",
    "parse_visual_regions",
    "redact_token",
    "validate_choice",
    "visual_submit_region",
]

VisualDelivery = Literal["background", "foreground"]


@dataclass(frozen=True)
class VisualRegion:
    id: str
    kind: Literal["text", "icon"]
    text: str | None
    label: str | None
    confidence: float
    interactive: bool
    x: int
    y: int
    width: int
    height: int


@dataclass(frozen=True)
class VisualObservation:
    capture_id: str
    screenshot_reference: str
    screenshot_width: int
    screenshot_height: int
    pid: int
    window_id: int
    # Driver's screenshot-to-action affine ``(m11, m12, m21, m22, tx, ty)``.
    # It is validated but never applied here: a capture-bound click sends the
    # original screenshot point and ``capture_id`` and Driver maps it once.
    screenshot_to_action: AffineCoefficients
    regions: tuple[VisualRegion, ...]

    def screenshot_center(self, region: VisualRegion) -> tuple[float, float]:
        return (region.x + region.width / 2, region.y + region.height / 2)


class VisualObservationError(ValueError):
    def __init__(self, message: str, code: str = "invalid_visual_result") -> None:
        super().__init__(message)
        self.code = code


def _nonempty(value: Any) -> str:
    if not isinstance(value, str) or not value.strip():
        raise VisualObservationError("visual result contains an empty string")
    return value


def _positive_int(value: Any) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise VisualObservationError("visual result contains an invalid positive integer")
    return value


def _pixel_int(value: Any) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise VisualObservationError("visual result contains an invalid pixel coordinate")
    return value


def parse_visual_regions(
    payload: Mapping[str, Any],
    *,
    expected_capture_id: str,
    expected_pid: int,
    expected_window_id: int,
) -> VisualObservation:
    if payload.get("schema") != "cua.visual_regions_v1":
        raise VisualObservationError("unsupported visual region schema")
    capture = payload.get("capture")
    if not isinstance(capture, dict) or capture.get("capture_id") != expected_capture_id:
        raise VisualObservationError(
            "visual result is stale or capture-mismatched", code="capture_mismatch"
        )
    source = capture.get("source")
    if (
        not isinstance(source, dict)
        or source.get("kind") != "window"
        or source.get("pid") != expected_pid
        or source.get("window_id") != expected_window_id
    ):
        raise VisualObservationError(
            "visual result has a mismatched window target", code="capture_mismatch"
        )
    screenshot = capture.get("screenshot")
    if not isinstance(screenshot, dict) or screenshot.get("mime_type") != "image/png":
        raise VisualObservationError("visual result has invalid screenshot provenance")
    screenshot_reference = _nonempty(screenshot.get("reference"))
    screenshot_width = _positive_int(screenshot.get("width"))
    screenshot_height = _positive_int(screenshot.get("height"))

    try:
        screenshot_to_action = action_coordinate_mapping(capture.get("action_coordinate_space"))
        for corner_x, corner_y in (
            (0, 0),
            (screenshot_width, 0),
            (0, screenshot_height),
            (screenshot_width, screenshot_height),
        ):
            map_screenshot_point(screenshot_to_action, corner_x, corner_y)
    except CoordinateMappingError as error:
        raise VisualObservationError(f"visual result has {error}") from error

    raw_regions = payload.get("regions")
    if not isinstance(raw_regions, list):
        raise VisualObservationError("visual result has no region list")
    regions: list[VisualRegion] = []
    ids: set[str] = set()
    for raw in raw_regions:
        if not isinstance(raw, dict):
            raise VisualObservationError("visual result contains a malformed region")
        region_id = _nonempty(raw.get("id"))
        if region_id in ids:
            raise VisualObservationError("visual result contains duplicate region IDs")
        ids.add(region_id)
        kind = raw.get("kind")
        if kind not in {"text", "icon"}:
            raise VisualObservationError("visual result contains an unsupported region kind")
        bounds = raw.get("bounds")
        if not isinstance(bounds, dict):
            raise VisualObservationError("visual result contains malformed bounds")
        x = _pixel_int(bounds.get("x"))
        y = _pixel_int(bounds.get("y"))
        width = _positive_int(bounds.get("width"))
        height = _positive_int(bounds.get("height"))
        if x + width > screenshot_width or y + height > screenshot_height:
            raise VisualObservationError("visual region is outside its source screenshot")
        confidence = raw.get("confidence")
        if (
            not isinstance(confidence, (int, float))
            or isinstance(confidence, bool)
            or not math.isfinite(float(confidence))
            or not 0 <= float(confidence) <= 1
        ):
            raise VisualObservationError("visual result contains invalid confidence")
        text = raw.get("text")
        label = raw.get("label")
        if text is not None:
            text = _nonempty(text)
        if label is not None:
            label = _nonempty(label)
        if (kind == "text" and text is None) or (kind == "icon" and label is None):
            raise VisualObservationError("visual region is missing content required by its kind")
        if not isinstance(raw.get("interactive"), bool):
            raise VisualObservationError("visual region has malformed interactivity")
        regions.append(
            VisualRegion(
                id=region_id,
                kind=kind,
                text=text,
                label=label,
                confidence=float(confidence),
                interactive=raw["interactive"],
                x=x,
                y=y,
                width=width,
                height=height,
            )
        )

    return VisualObservation(
        capture_id=expected_capture_id,
        screenshot_reference=screenshot_reference,
        screenshot_width=screenshot_width,
        screenshot_height=screenshot_height,
        pid=expected_pid,
        window_id=expected_window_id,
        screenshot_to_action=screenshot_to_action,
        regions=tuple(regions),
    )


def has_executable_candidate(candidates: list[Candidate]) -> bool:
    return any(candidate.tool is not None for candidate in candidates)


def choose_mock(candidates: list[Candidate]) -> tuple[str | None, float, dict[str, float]]:
    ids = {candidate.id for candidate in candidates}
    selected = (
        "type-verification-value"
        if "type-verification-value" in ids
        else "submit-form"
        if "submit-form" in ids
        else "submit-form-foreground"
        if "submit-form-foreground" in ids
        else "reobserve"
        if "reobserve" in ids
        else None
    )
    probabilities = {candidate.id: float(candidate.id == selected) for candidate in candidates}
    return selected, 1.0 if selected else 0.0, probabilities


def validate_choice(
    choice: str,
    candidates: list[Candidate],
    *,
    current_capture_id: str | None = None,
) -> Candidate:
    if not isinstance(choice, str) or not choice:
        raise ValueError("provider selected a malformed candidate ID")
    ids = [candidate.id for candidate in candidates]
    if len(ids) != len(set(ids)):
        raise ValueError("candidate set contains duplicate IDs")
    candidate = next((item for item in candidates if item.id == choice), None)
    if candidate is None:
        raise ValueError(f"provider selected unknown candidate: {choice}")
    if candidate.capture_id is not None and candidate.capture_id != current_capture_id:
        raise ValueError("provider selected a stale or capture-mismatched candidate")
    return candidate
