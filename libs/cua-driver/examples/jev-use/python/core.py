from __future__ import annotations

import math
from dataclasses import dataclass
from types import MappingProxyType
from typing import Any, Literal, Mapping

Outcome = Literal["verified", "refuted", "unknown", "abstained", "budget_exhausted"]


def _freeze(value: Any) -> Any:
    if isinstance(value, dict):
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(_freeze(item) for item in value)
    return value


@dataclass(frozen=True)
class Candidate:
    id: str
    description: str
    tool: str | None
    arguments: Mapping[str, Any]
    capture_id: str | None = None
    screenshot_reference: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "arguments", _freeze(dict(self.arguments)))


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
    action_origin_x: float
    action_origin_y: float
    action_units_per_pixel_x: float
    action_units_per_pixel_y: float
    regions: tuple[VisualRegion, ...]

    def screenshot_center(self, region: VisualRegion) -> tuple[float, float]:
        return (region.x + region.width / 2, region.y + region.height / 2)


class VisualObservationError(ValueError):
    pass


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
        raise VisualObservationError("visual result is stale or capture-mismatched")
    source = capture.get("source")
    if (
        not isinstance(source, dict)
        or source.get("kind") != "window"
        or source.get("pid") != expected_pid
        or source.get("window_id") != expected_window_id
    ):
        raise VisualObservationError("visual result has a mismatched window target")
    screenshot = capture.get("screenshot")
    if not isinstance(screenshot, dict) or screenshot.get("mime_type") != "image/png":
        raise VisualObservationError("visual result has invalid screenshot provenance")
    screenshot_reference = _nonempty(screenshot.get("reference"))
    screenshot_width = _positive_int(screenshot.get("width"))
    screenshot_height = _positive_int(screenshot.get("height"))

    coordinate_space = capture.get("action_coordinate_space")
    if not isinstance(coordinate_space, dict):
        raise VisualObservationError("visual result has no action coordinate space")
    if coordinate_space.get("kind") == "screenshot_pixels":
        origin_x, origin_y, scale_x, scale_y = 0.0, 0.0, 1.0, 1.0
    elif coordinate_space.get("kind") == "scaled_top_left":
        values = (
            coordinate_space.get("action_origin_x"),
            coordinate_space.get("action_origin_y"),
            coordinate_space.get("action_units_per_pixel_x"),
            coordinate_space.get("action_units_per_pixel_y"),
        )
        if any(not isinstance(value, (int, float)) or isinstance(value, bool) for value in values):
            raise VisualObservationError("visual result has malformed coordinate mapping")
        origin_x, origin_y, scale_x, scale_y = (float(value) for value in values)
        if not all(math.isfinite(value) for value in (origin_x, origin_y, scale_x, scale_y)):
            raise VisualObservationError("visual result has non-finite coordinate mapping")
        if scale_x <= 0 or scale_y <= 0:
            raise VisualObservationError("visual result has non-positive coordinate scale")
    else:
        raise VisualObservationError("visual result has unsupported coordinate mapping")

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
        action_origin_x=origin_x,
        action_origin_y=origin_y,
        action_units_per_pixel_x=scale_x,
        action_units_per_pixel_y=scale_y,
        regions=tuple(regions),
    )


def _reserved_candidates() -> list[Candidate]:
    return [
        Candidate(
            "reobserve",
            "Discard this decision set and obtain a fresh Driver observation.",
            None,
            {},
        ),
        Candidate(
            "abstain",
            "Stop without acting if none of the proposed actions is safe for the observed state.",
            None,
            {},
        ),
    ]


def build_candidates(
    snapshot: dict[str, Any],
    token: str,
    visual: VisualObservation | None = None,
    *,
    capture_bound_click: bool = False,
) -> list[Candidate]:
    common = {
        "target_id": snapshot["target_id"],
        "tab_id": snapshot["tab_id"],
    }
    refs = snapshot.get("refs", [])
    field = next(
        (
            ref
            for ref in refs
            if ref.get("role") == "textbox" and ref.get("name") == "verification value"
        ),
        None,
    )
    button = next(
        (ref for ref in refs if ref.get("role") == "button" and ref.get("name") == "Submit"),
        None,
    )
    candidates: list[Candidate] = []
    if field and field.get("value") != token:
        candidates.append(
            Candidate(
                "type-verification-value",
                "Replace the verification field with the required token.",
                "browser_type",
                {**common, "ref": field["ref"], "text": token, "replace": True},
            )
        )
    elif field and field.get("value") == token and button:
        candidates.append(
            Candidate(
                "submit-form",
                "Submit the form now that the verification field contains the token.",
                "browser_click",
                {**common, "ref": button["ref"], "input_route": "dom_event"},
            )
        )
    elif (
        field
        and field.get("value") == token
        and visual
        and capture_bound_click
    ):
        matches = [
            region
            for region in visual.regions
            if region.confidence >= 0.8
            and _ascii_lower(region.text or region.label or "") == "submit"
        ]
        if len(matches) == 1:
            x, y = visual.screenshot_center(matches[0])
            candidates.append(
                Candidate(
                    "submit-form",
                    "Submit the form using the unique validated visual Submit region.",
                    "click",
                    {
                        "pid": visual.pid,
                        "window_id": visual.window_id,
                        "x": x,
                        "y": y,
                        "capture_id": visual.capture_id,
                        "delivery_mode": "background",
                    },
                    capture_id=visual.capture_id,
                    screenshot_reference=visual.screenshot_reference,
                )
            )
    return candidates + _reserved_candidates()


def _ascii_lower(value: str) -> str:
    return "".join(chr(ord(char) + 32) if "A" <= char <= "Z" else char for char in value)


def choose_mock(candidates: list[Candidate]) -> tuple[str | None, float, dict[str, float]]:
    ids = {candidate.id for candidate in candidates}
    selected = (
        "type-verification-value"
        if "type-verification-value" in ids
        else "submit-form" if "submit-form" in ids else "reobserve" if "reobserve" in ids else None
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


def classify(submitted: str | None, token: str, *, steps: int, max_steps: int) -> Outcome:
    if submitted == token:
        return "verified"
    if submitted is not None:
        return "refuted"
    if steps >= max_steps:
        return "budget_exhausted"
    return "unknown"
