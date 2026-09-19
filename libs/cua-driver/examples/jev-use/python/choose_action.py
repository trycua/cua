from __future__ import annotations

import json
import math
import re
import sys
from typing import Any, Mapping

from jev_adapter import ProviderChoice, choose_bounded_with_typesafe

REQUEST_SCHEMA = "cua.jev_choice_request_v1"
RESPONSE_SCHEMA = "cua.jev_choice_v1"
MAX_INPUT_BYTES = 65_536
MAX_CANDIDATES = 32
MAX_REGIONS = 100
MAX_HISTORY = 16
ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,63}\Z")


def _record(value: Any, message: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(message)
    return value


def _bounded_string(value: Any, name: str, limit: int) -> str:
    if not isinstance(value, str) or not value.strip() or len(value) > limit:
        raise ValueError(f"{name} must be a nonempty string of at most {limit} characters")
    return value


def _identifier(value: Any, name: str) -> str:
    value = _bounded_string(value, name, 64)
    if not ID_PATTERN.fullmatch(value):
        raise ValueError(f"{name} contains unsupported characters")
    return value


def validate_request(value: Any) -> dict[str, Any]:
    root = _record(value, "request must be a JSON object")
    expected_keys = {"schema", "goal", "capture_id", "regions", "history", "candidates"}
    if set(root) != expected_keys or root.get("schema") != REQUEST_SCHEMA:
        raise ValueError(f"request must match {REQUEST_SCHEMA}")

    goal = _bounded_string(root["goal"], "goal", 4_000)
    capture_id = _bounded_string(root["capture_id"], "capture_id", 256)

    raw_regions = root["regions"]
    if not isinstance(raw_regions, list) or len(raw_regions) > MAX_REGIONS:
        raise ValueError(f"regions must be an array of at most {MAX_REGIONS} items")
    regions: list[dict[str, Any]] = []
    region_ids: set[str] = set()
    for raw_value in raw_regions:
        raw = _record(raw_value, "region must be an object")
        allowed = {"id", "kind", "bounds", "text", "label", "confidence", "interactive"}
        required = {"id", "kind", "bounds", "confidence", "interactive"}
        if not set(raw).issubset(allowed) or not required.issubset(raw):
            raise ValueError("region has unsupported or missing fields")
        region_id = _bounded_string(raw["id"], "region id", 256)
        if region_id in region_ids:
            raise ValueError("region IDs must be unique")
        region_ids.add(region_id)
        kind = raw["kind"]
        if kind not in {"text", "icon"}:
            raise ValueError("region kind must be text or icon")
        bounds = _record(raw["bounds"], "region bounds must be an object")
        if set(bounds) != {"x", "y", "width", "height"}:
            raise ValueError("region bounds have unsupported or missing fields")
        for key in ("x", "y", "width", "height"):
            number = bounds[key]
            minimum = 0 if key in {"x", "y"} else 1
            if not isinstance(number, int) or isinstance(number, bool) or number < minimum:
                raise ValueError("region bounds must contain valid integers")
        text = raw.get("text")
        label = raw.get("label")
        if text is not None:
            text = _bounded_string(text, "region text", 1_000)
        if label is not None:
            label = _bounded_string(label, "region label", 1_000)
        if (kind == "text" and text is None) or (kind == "icon" and label is None):
            raise ValueError("region is missing content required by its kind")
        confidence = raw["confidence"]
        if (
            not isinstance(confidence, (int, float))
            or isinstance(confidence, bool)
            or not math.isfinite(float(confidence))
            or not 0 <= float(confidence) <= 1
        ):
            raise ValueError("region confidence must be between zero and one")
        if not isinstance(raw["interactive"], bool):
            raise ValueError("region interactive must be boolean")
        regions.append(
            {
                "id": region_id,
                "kind": kind,
                "bounds": dict(bounds),
                "text": text,
                "label": label,
                "confidence": float(confidence),
                "interactive": raw["interactive"],
            }
        )

    history = root["history"]
    if not isinstance(history, list) or len(history) > MAX_HISTORY:
        raise ValueError(f"history must be an array of at most {MAX_HISTORY} items")
    compact_history: list[dict[str, str]] = []
    for raw_value in history:
        raw = _record(raw_value, "history item must be an object")
        if not raw or not set(raw).issubset({"selected_id", "outcome"}):
            raise ValueError("history contains a forbidden field")
        item: dict[str, str] = {}
        if "selected_id" in raw:
            item["selected_id"] = _identifier(raw["selected_id"], "history selected_id")
        if "outcome" in raw:
            item["outcome"] = _bounded_string(raw["outcome"], "history outcome", 128)
        compact_history.append(item)

    raw_candidates = root["candidates"]
    if (
        not isinstance(raw_candidates, list)
        or not 2 <= len(raw_candidates) <= MAX_CANDIDATES
    ):
        raise ValueError(f"candidates must contain between 2 and {MAX_CANDIDATES} items")
    candidates: list[dict[str, str]] = []
    candidate_ids: set[str] = set()
    for raw_value in raw_candidates:
        raw = _record(raw_value, "candidate must be an object")
        if set(raw) != {"id", "description"}:
            raise ValueError("candidate may contain only id and description")
        candidate_id = _identifier(raw["id"], "candidate id")
        if candidate_id in candidate_ids:
            raise ValueError("candidate IDs must be unique")
        candidate_ids.add(candidate_id)
        candidates.append(
            {
                "id": candidate_id,
                "description": _bounded_string(raw["description"], "description", 1_000),
            }
        )
    if not {"reobserve", "abstain"}.issubset(candidate_ids):
        raise ValueError("candidates must include reobserve and abstain")

    return {
        "goal": goal,
        "capture_id": capture_id,
        "regions": regions,
        "history": compact_history,
        "candidates": candidates,
    }


def choose_request(
    request: Mapping[str, Any], *, client: Any | None = None, mock: bool = False
) -> dict[str, Any]:
    validated = validate_request(dict(request))
    criteria = {item["id"]: item["description"] for item in validated["candidates"]}
    if mock:
        selected_id = next(
            (
                candidate_id
                for candidate_id in criteria
                if candidate_id not in {"reobserve", "abstain"}
            ),
            "reobserve",
        )
        choice = ProviderChoice(
            selected_id=selected_id,
            confidence=1.0,
            probabilities={
                candidate_id: float(candidate_id == selected_id)
                for candidate_id in criteria
            },
            model="mock",
        )
    else:
        if client is None:
            from typesafe_sdk import TypeSafeClient

            with TypeSafeClient() as live_client:
                choice = choose_bounded_with_typesafe(
                    live_client,
                    goal=validated["goal"],
                    observation={
                        "capture_id": validated["capture_id"],
                        "regions": validated["regions"],
                        "history": validated["history"],
                    },
                    criteria=criteria,
                )
        else:
            choice = choose_bounded_with_typesafe(
                client,
                goal=validated["goal"],
                observation={
                    "capture_id": validated["capture_id"],
                    "regions": validated["regions"],
                    "history": validated["history"],
                },
                criteria=criteria,
            )
    return {
        "schema": RESPONSE_SCHEMA,
        "selected_id": choice.selected_id,
        "model": choice.model,
        "confidence": choice.confidence,
        "probabilities": choice.probabilities,
    }


def main() -> None:
    if sys.argv[1:] not in ([], ["--mock"]):
        raise SystemExit("usage: choose_action.py [--mock]")
    raw = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
    if len(raw) > MAX_INPUT_BYTES:
        raise SystemExit("request exceeds input limit")
    try:
        request = json.loads(raw)
        validate_request(request)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError, TypeError, KeyError) as error:
        raise SystemExit(f"invalid request: {error}") from None
    try:
        response = choose_request(request, mock=sys.argv[1:] == ["--mock"])
    except Exception:
        raise SystemExit("provider request failed") from None
    sys.stdout.write(json.dumps(response, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
