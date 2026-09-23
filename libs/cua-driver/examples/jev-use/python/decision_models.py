"""Closed-candidate decision models for the jev-use example.

Models only select caller-supplied IDs. Candidate construction, capture
freshness, Driver actions, and independent verification stay with the caller.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Literal, Mapping, Protocol

from jev_adapter import choose_bounded_with_typesafe

DecisionKind = Literal["selected", "reobserve", "abstain", "error"]


@dataclass(frozen=True)
class DecisionRequest:
    goal: str
    capture_id: str
    regions: tuple[Mapping[str, Any], ...]
    history: tuple[Mapping[str, str], ...]
    candidates: tuple[Mapping[str, str], ...]

    @classmethod
    def from_validated(cls, request: Mapping[str, Any]) -> "DecisionRequest":
        return cls(
            goal=request["goal"],
            capture_id=request["capture_id"],
            regions=tuple(request["regions"]),
            history=tuple(request["history"]),
            candidates=tuple(MappingProxyType(dict(item)) for item in request["candidates"]),
        )

    @property
    def criteria(self) -> dict[str, str]:
        return {item["id"]: item["description"] for item in self.candidates}


@dataclass(frozen=True)
class ModelScores:
    probabilities: Mapping[str, float]
    model: str
    selected_id: str | None = None
    confidence: float | None = None


@dataclass(frozen=True)
class DecisionResult:
    kind: DecisionKind
    selected_id: str | None
    model: str
    confidence: float | None
    probabilities: Mapping[str, float]
    reason: str | None = None

    def to_wire(self) -> dict[str, Any]:
        return {
            "schema": "cua.decision_choice_v1",
            "kind": self.kind,
            "selected_id": self.selected_id,
            "model": self.model,
            "confidence": self.confidence,
            "probabilities": dict(self.probabilities),
            "reason": self.reason,
        }


class DecisionModel(Protocol):
    name: str

    def score(self, request: DecisionRequest) -> ModelScores: ...


def choose(model: DecisionModel, request: DecisionRequest) -> DecisionResult:
    """Reject malformed scores without converting them into an action."""
    try:
        allowed = request.criteria
        scores = model.score(request)
        if not isinstance(scores.model, str) or not 0 < len(scores.model) <= 128:
            raise ValueError("model identity must be a bounded string")
        if set(scores.probabilities) != set(allowed):
            raise ValueError("probability IDs do not match the candidate IDs")
        probabilities = dict(scores.probabilities)
        if any(
            not isinstance(value, (int, float))
            or isinstance(value, bool)
            or not math.isfinite(value)
            or not 0 <= value <= 1
            for value in probabilities.values()
        ):
            raise ValueError("probabilities must be finite numbers in [0, 1]")
        if abs(sum(probabilities.values()) - 1) > 0.02:
            raise ValueError("probability mass must be approximately one")
        argmax = max(allowed, key=lambda candidate_id: probabilities[candidate_id])
        selected = scores.selected_id or argmax
        if selected not in allowed or probabilities[selected] != probabilities[argmax]:
            raise ValueError("selection must be an allowed argmax")
        confidence = scores.confidence if scores.confidence is not None else probabilities[selected]
        if (
            not isinstance(confidence, (int, float))
            or isinstance(confidence, bool)
            or not math.isfinite(confidence)
            or not 0 <= confidence <= 1
        ):
            raise ValueError("confidence must be a finite number in [0, 1]")
        kind: DecisionKind = (
            "reobserve"
            if selected == "reobserve"
            else "abstain"
            if selected == "abstain"
            else "selected"
        )
        return DecisionResult(
            kind=kind,
            selected_id=selected,
            model=scores.model,
            confidence=confidence,
            probabilities=probabilities,
        )
    except Exception as error:
        reason = "option_limit" if isinstance(error, OptionLimitError) else "model_error"
        return DecisionResult("error", None, model.name, None, {}, reason)


class OptionLimitError(ValueError):
    pass


class TypeSafeDecisionModel:
    name = "typesafe-jev"

    def __init__(self, client: Any) -> None:
        self.client = client

    def score(self, request: DecisionRequest) -> ModelScores:
        choice = choose_bounded_with_typesafe(
            self.client,
            goal=request.goal,
            observation={
                "capture_id": request.capture_id,
                "regions": list(request.regions),
                "history": list(request.history),
            },
            criteria=request.criteria,
        )
        if choice.selected_id not in request.criteria:
            raise ValueError("Jev selected an unknown candidate")
        if choice.probabilities.get(choice.selected_id) != max(choice.probabilities.values()):
            raise ValueError("Jev selection disagrees with its scores")
        return ModelScores(
            choice.probabilities,
            choice.model or self.name,
            selected_id=choice.selected_id,
            confidence=choice.confidence,
        )


@dataclass(frozen=True)
class _S1Option:
    element_id: str
    role: str
    label: str
    action: str
    entity_id: str | None = None


def visual_regions_as_text(request: DecisionRequest) -> str:
    """Describe OmniParser regions; this is not an accessibility tree."""
    lines = [f"Visual-region-derived observation for capture {request.capture_id}:"]
    for region in request.regions:
        bounds = region["bounds"]
        label = region.get("text") or region.get("label")
        lines.append(
            f"{region['id']}: {region['kind']} {label!r} at "
            f"({bounds['x']},{bounds['y']},{bounds['width']},{bounds['height']})"
        )
    return "\n".join(lines)


class S1DecisionModel:
    name = "cua-s1-4b-0.2"

    def __init__(
        self,
        scorer: Any,
        *,
        modality: Literal["text", "multimodal"] = "text",
        screenshot_path: Path | None = None,
    ) -> None:
        self.scorer = scorer
        self.modality = modality
        self.screenshot_path = screenshot_path

    def score(self, request: DecisionRequest) -> ModelScores:
        if len(request.candidates) > 26:
            raise OptionLimitError("S1 uses one letter per option, at most 26")
        if self.modality == "multimodal" and (
            self.screenshot_path is None or not self.screenshot_path.is_file()
        ):
            raise ValueError("multimodal S1 requires an existing local screenshot")
        options = [
            _S1Option(item["id"], "Decision", item["description"], "select")
            for item in request.candidates
        ]
        kwargs: dict[str, Any] = {
            "app": "Cua Driver",
            "task_family": "closed-candidate decision",
            "goal": request.goal,
            "modality": self.modality,
        }
        if self.modality == "text":
            kwargs["ax_tree"] = visual_regions_as_text(request)
        else:
            kwargs["screenshot"] = self.screenshot_path
        scores = self.scorer.forward(options, **kwargs)
        probabilities: dict[str, float] = {}
        for score in scores:
            if score.element_id in probabilities:
                raise ValueError("S1 returned a duplicate candidate ID")
            probabilities[score.element_id] = score.probability
        return ModelScores(probabilities, self.name)
