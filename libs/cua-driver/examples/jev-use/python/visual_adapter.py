from __future__ import annotations

from typing import Any

from core import Candidate


def _valid_box(box: dict[str, Any], width: int, height: int) -> bool:
    values = [box.get(key) for key in ("x", "y", "width", "height")]
    if not all(isinstance(value, int) for value in values):
        return False
    x, y, box_width, box_height = values
    return x >= 0 and y >= 0 and box_width > 0 and box_height > 0 and x + box_width <= width and y + box_height <= height


def _overlaps(left: dict[str, int], right: dict[str, int]) -> bool:
    return (
        left["x"] < right["x"] + right["width"]
        and right["x"] < left["x"] + left["width"]
        and left["y"] < right["y"] + right["height"]
        and right["y"] < left["y"] + left["height"]
    )


def build_visual_candidates(
    observation: dict[str, Any], response: dict[str, Any], *, minimum_confidence: float = 0.8
) -> list[Candidate]:
    fallback = [
        Candidate("reobserve", "Capture fresh evidence before proposing another action.", None, {}),
        Candidate("abstain", "Stop without acting when the evidence is unsafe or ambiguous.", None, {}),
    ]
    capture = response.get("capture", {})
    if response.get("schema") != "cua.visual_regions_v1" or any(
        capture.get(key) != observation.get(key)
        for key in ("source", "snapshot_id", "image_width", "image_height")
    ):
        return fallback

    width = observation.get("image_width")
    height = observation.get("image_height")
    source = observation.get("source", {})
    if (
        not isinstance(width, int)
        or not isinstance(height, int)
        or source.get("kind") != "window"
        or not isinstance(source.get("pid"), int)
        or not isinstance(source.get("window_id"), int)
    ):
        return fallback

    semantic = observation.get("semantic_elements", [])
    matches: list[dict[str, Any]] = []
    for region in response.get("regions", []):
        box = region.get("bounds", {})
        center = region.get("center", {})
        if (
            region.get("interactive") is not True
            or not isinstance(region.get("confidence"), (int, float))
            or region["confidence"] < minimum_confidence
            or not _valid_box(box, width, height)
            or not isinstance(center.get("x"), int)
            or not isinstance(center.get("y"), int)
            or not (box["x"] <= center["x"] < box["x"] + box["width"])
            or not (box["y"] <= center["y"] < box["y"] + box["height"])
        ):
            continue
        aligned = [
            element
            for element in semantic
            if str(element.get("name", "")).casefold() == str(region.get("text", "")).casefold()
            and _valid_box(element.get("bounds", {}), width, height)
            and _overlaps(box, element["bounds"])
        ]
        if len(aligned) == 1:
            matches.append(region)

    if len(matches) != 1:
        return fallback
    region = matches[0]
    action = Candidate(
        "click-visual-save",
        "Click the fresh visual region labelled Save.",
        "click",
        {
            "pid": source["pid"],
            "window_id": source["window_id"],
            "x": region["center"]["x"],
            "y": region["center"]["y"],
            "delivery_mode": "background",
        },
    )
    return [action, *fallback]
