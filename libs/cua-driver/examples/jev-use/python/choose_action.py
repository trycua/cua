from __future__ import annotations

import json
import math
import re
import sys
from typing import Any, Mapping

from jev_adapter import ProviderChoice, choose_bounded_with_typesafe
from native_roles import ROLE_CLASSES

REQUEST_SCHEMA = "cua.jev_choice_request_v1"
# Additive over v1 (RFC #4268): per-candidate ``source``, an optional root
# ``snapshot_id``, an optional compact ``elements`` list of native controls, and
# an optional ``progress`` list of task steps counted from the runner's own
# performed actions (#4313).
REQUEST_SCHEMA_V2 = "cua.jev_choice_request_v2"
REQUEST_SCHEMAS = (REQUEST_SCHEMA, REQUEST_SCHEMA_V2)
RESPONSE_SCHEMA = "cua.jev_choice_v1"
MAX_INPUT_BYTES = 65_536
MAX_CANDIDATES = 32
MAX_REGIONS = 100
MAX_HISTORY = 16
MAX_ELEMENTS = 64
MAX_PROGRESS = 16
MAX_PROGRESS_COUNT = 64
ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,63}\Z")
RESERVED_IDS = frozenset({"reobserve", "abstain"})
CANDIDATE_SOURCES = frozenset({"page", "ax", "visual"})
ELEMENT_STATES = frozenset(
    {"enabled", "checked", "unchecked", "selected", "not_selected", "empty", "has_text"}
)
V1_ROOT_KEYS = frozenset({"schema", "goal", "capture_id", "regions", "history", "candidates"})
V2_OPTIONAL_ROOT_KEYS = frozenset({"snapshot_id", "elements", "progress"})


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


def _validate_elements(value: Any) -> list[dict[str, str]]:
    if not isinstance(value, list) or len(value) > MAX_ELEMENTS:
        raise ValueError(f"elements must be an array of at most {MAX_ELEMENTS} items")
    elements: list[dict[str, str]] = []
    for raw_value in value:
        raw = _record(raw_value, "element must be an object")
        if set(raw) != {"role_class", "label", "state"}:
            raise ValueError("element may contain only role_class, label, and state")
        if raw["role_class"] not in ROLE_CLASSES:
            raise ValueError("element role_class is not a supported role class")
        if raw["state"] not in ELEMENT_STATES:
            raise ValueError("element state is not a supported state")
        elements.append(
            {
                "role_class": raw["role_class"],
                "label": _bounded_string(raw["label"], "element label", 200),
                "state": raw["state"],
            }
        )
    return elements


def _count(value: Any, name: str, minimum: int) -> int:
    if (
        not isinstance(value, int)
        or isinstance(value, bool)
        or not minimum <= value <= MAX_PROGRESS_COUNT
    ):
        raise ValueError(f"{name} must be an integer from {minimum} to {MAX_PROGRESS_COUNT}")
    return value


def _validate_progress(value: Any) -> list[dict[str, Any]]:
    """Validate task steps and how often this run has performed each one.

    ``done`` is counted from the runner's own performed actions, never read
    from the application, so ``progress`` carries no application values.
    """
    if not isinstance(value, list) or len(value) > MAX_PROGRESS:
        raise ValueError(f"progress must be an array of at most {MAX_PROGRESS} items")
    progress: list[dict[str, Any]] = []
    for raw_value in value:
        raw = _record(raw_value, "progress item must be an object")
        if set(raw) != {"step", "done", "required"}:
            raise ValueError("progress item may contain only step, done, and required")
        progress.append(
            {
                "step": _bounded_string(raw["step"], "progress step", 200),
                "done": _count(raw["done"], "progress done", 0),
                "required": _count(raw["required"], "progress required", 1),
            }
        )
    return progress


def validate_request(value: Any) -> dict[str, Any]:
    """Validate a ``cua.jev_choice_request_v1`` or ``_v2`` request strictly.

    v1 is unchanged: exactly its six root keys and ``id``/``description``
    candidates, so any v2 field in a v1 request is rejected. v2 may add a root
    ``snapshot_id``, ``elements``, and ``progress`` and a per-candidate
    ``source``; reserved candidates never carry a source.
    """
    root = _record(value, "request must be a JSON object")
    schema = root.get("schema")
    if schema == REQUEST_SCHEMA:
        if set(root) != V1_ROOT_KEYS:
            raise ValueError(f"request must match {REQUEST_SCHEMA}")
    elif schema == REQUEST_SCHEMA_V2:
        if not V1_ROOT_KEYS.issubset(root) or not set(root).issubset(
            V1_ROOT_KEYS | V2_OPTIONAL_ROOT_KEYS
        ):
            raise ValueError(f"request must match {REQUEST_SCHEMA_V2}")
    else:
        raise ValueError(f"request must match {REQUEST_SCHEMA} or {REQUEST_SCHEMA_V2}")
    v2 = schema == REQUEST_SCHEMA_V2

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
        keys = set(raw)
        if v2:
            if not {"id", "description"}.issubset(keys) or not keys.issubset(
                {"id", "description", "source"}
            ):
                raise ValueError("candidate may contain only id, description, and source")
        elif keys != {"id", "description"}:
            raise ValueError("candidate may contain only id and description")
        candidate_id = _identifier(raw["id"], "candidate id")
        if candidate_id in candidate_ids:
            raise ValueError("candidate IDs must be unique")
        candidate_ids.add(candidate_id)
        candidate = {
            "id": candidate_id,
            "description": _bounded_string(raw["description"], "description", 1_000),
        }
        if "source" in raw:
            if candidate_id in RESERVED_IDS:
                raise ValueError("reserved candidates must not carry a source")
            if raw["source"] not in CANDIDATE_SOURCES:
                raise ValueError("candidate source must be page, ax, or visual")
            candidate["source"] = raw["source"]
        candidates.append(candidate)
    if not RESERVED_IDS.issubset(candidate_ids):
        raise ValueError("candidates must include reobserve and abstain")

    validated: dict[str, Any] = {
        "schema": schema,
        "goal": goal,
        "capture_id": capture_id,
        "regions": regions,
        "history": compact_history,
        "candidates": candidates,
    }
    if v2:
        snapshot_id = root.get("snapshot_id")
        validated["snapshot_id"] = (
            None if snapshot_id is None else _bounded_string(snapshot_id, "snapshot_id", 64)
        )
        validated["elements"] = _validate_elements(root.get("elements", []))
        validated["progress"] = _validate_progress(root.get("progress", []))
    return validated


def provider_observation(validated: Mapping[str, Any]) -> dict[str, Any]:
    """Build the observation a provider sees for a validated request.

    A v1 request keeps its exact v1 observation. A v2 request adds the native
    ``snapshot_id``, the compact ``elements``, and each candidate's ``source``,
    plus ``progress`` when the request carries any; none of these contain
    element tokens, values, or pixels.
    """
    observation: dict[str, Any] = {
        "capture_id": validated["capture_id"],
        "regions": validated["regions"],
        "history": validated["history"],
    }
    if validated.get("schema") == REQUEST_SCHEMA_V2:
        observation["snapshot_id"] = validated.get("snapshot_id")
        observation["elements"] = validated.get("elements", [])
        observation["candidate_sources"] = {
            item["id"]: item["source"] for item in validated["candidates"] if "source" in item
        }
        if validated.get("progress"):
            observation["progress"] = validated["progress"]
    return observation


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
                    observation=provider_observation(validated),
                    criteria=criteria,
                )
        else:
            choice = choose_bounded_with_typesafe(
                client,
                goal=validated["goal"],
                observation=provider_observation(validated),
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
