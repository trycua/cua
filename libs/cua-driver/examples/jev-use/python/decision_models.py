"""Closed-candidate decision models for the jev-use example.

Models only select caller-supplied IDs. Candidate construction, capture
freshness, Driver actions, and independent verification stay with the caller.
"""

from __future__ import annotations

import math
import json
import re
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
    capture_id: str
    selected_id: str | None
    model: str
    confidence: float | None
    probabilities: Mapping[str, float]
    reason: str | None = None

    def to_wire(self) -> dict[str, Any]:
        return {
            "schema": "cua.decision_choice_v1",
            "kind": self.kind,
            "capture_id": self.capture_id,
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
        top = max(probabilities.values())
        winners = [candidate_id for candidate_id in allowed if probabilities[candidate_id] == top]
        if len(winners) != 1:
            raise ValueError("a tied top score is not actionable")
        selected = scores.selected_id or winners[0]
        if selected != winners[0]:
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
            capture_id=request.capture_id,
            selected_id=selected,
            model=scores.model,
            confidence=confidence,
            probabilities=probabilities,
        )
    except Exception as error:
        reason = "option_limit" if isinstance(error, OptionLimitError) else "model_error"
        model_name = getattr(model, "name", "unknown")
        if not isinstance(model_name, str) or not 0 < len(model_name) <= 128:
            model_name = "unknown"
        return DecisionResult("error", request.capture_id, None, model_name, None, {}, reason)


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
    lines = [f"Visual-region-derived observation for capture {json.dumps(request.capture_id)}:"]
    for region in request.regions:
        bounds = region["bounds"]
        label = region.get("text") or region.get("label")
        lines.append(
            f"{json.dumps(region['id'])}: {region['kind']} {label!r} at "
            f"({bounds['x']},{bounds['y']},{bounds['width']},{bounds['height']}) "
            f"confidence={json.dumps(region['confidence'])} "
            f"interactive={json.dumps(region['interactive'])}"
        )
    if request.history:
        lines.append("Prior bounded decisions:")
        lines.extend(json.dumps(item, ensure_ascii=False) for item in request.history)
    return "\n".join(lines)


S1_MODALITIES = ("text", "multimodal")
_HF_SNAPSHOT_PARENT = "snapshots"
_REVISION_PATTERN = re.compile(r"[0-9a-f]{40}")
_UNSAFE_IDENTITY_CHARS = re.compile(r"[^A-Za-z0-9._/-]")


def _hf_local_dir_revision(root: Path, modality: str) -> str | None:
    """Read the commit that `hf download --local-dir` recorded for the adapter.

    `huggingface_hub` writes `<local-dir>/.cache/huggingface/download/<file>.metadata`
    whose first line is the resolved commit hash. Only the selected modality's
    adapter files are consulted, and they must agree.
    """
    metadata_dir = root / ".cache" / "huggingface" / "download"
    revisions: set[str] = set()
    for prefix in (Path(modality), Path()):
        for name in ("adapter_config.json", "adapter_model.safetensors"):
            candidate = metadata_dir / prefix / f"{name}.metadata"
            try:
                first_line = candidate.read_text(encoding="utf-8").splitlines()[0].strip()
            except (OSError, IndexError, UnicodeDecodeError):
                continue
            if _REVISION_PATTERN.fullmatch(first_line):
                revisions.add(first_line)
        if revisions:
            break
    return revisions.pop() if len(revisions) == 1 else None


def s1_model_identity(
    adapter_path: Path,
    modality: str,
    *,
    repo_id: str | None = None,
    revision: str | None = None,
) -> str:
    """Name the S1 checkpoint that produced a decision, for evidence logs.

    The result is `<adapter>[@<revision>]:<modality>`, for example
    `cua-ai/cua-s1-4b-0.2@16818868b0cc7813808aae4e87b417657046ab79:text`.
    `repo_id` and `revision` override detection. Otherwise the adapter name
    and revision come from a Hugging Face cache snapshot path
    (`models--<org>--<name>/snapshots/<commit>`), from the metadata that
    `hf download --local-dir` leaves in the adapter directory, or, failing
    both, from the adapter directory name with no revision. Detection never
    claims a Hub repository or revision it cannot read locally.
    """
    if modality not in S1_MODALITIES:
        raise ValueError(f"unknown S1 modality: {modality!r}")
    root = Path(adapter_path).expanduser()
    if root.name in S1_MODALITIES:
        root = root.parent
    label = repo_id.strip() if repo_id and repo_id.strip() else None
    detected_revision: str | None = None
    parts = root.parts
    if len(parts) >= 3 and parts[-2] == _HF_SNAPSHOT_PARENT and parts[-3].startswith("models--"):
        if label is None:
            label = parts[-3].removeprefix("models--").replace("--", "/")
        if _REVISION_PATTERN.fullmatch(parts[-1]):
            detected_revision = parts[-1]
    else:
        detected_revision = _hf_local_dir_revision(root, modality)
    if label is None:
        label = root.name or "cua-s1-adapter"
    chosen_revision = revision.strip() if revision and revision.strip() else detected_revision
    label = _UNSAFE_IDENTITY_CHARS.sub("_", label)[:64]
    identity = label
    if chosen_revision:
        identity += "@" + _UNSAFE_IDENTITY_CHARS.sub("_", chosen_revision)[:40]
    return f"{identity}:{modality}"


class S1DecisionModel:
    # Default identity for callers that construct the adapter directly without
    # naming the checkpoint. `choose_decision.py` passes a derived identity.
    name = "cua-s1-4b-local"

    def __init__(
        self,
        scorer: Any,
        *,
        modality: Literal["text", "multimodal"] = "text",
        screenshot_path: Path | None = None,
        screenshot_capture_id: str | None = None,
        name: str | None = None,
    ) -> None:
        if name is not None:
            if not isinstance(name, str) or not 0 < len(name) <= 128:
                raise ValueError("S1 model identity must be a bounded string")
            self.name = name
        self.scorer = scorer
        self.modality = modality
        self.screenshot_path = screenshot_path
        self.screenshot_capture_id = screenshot_capture_id

    def score(self, request: DecisionRequest) -> ModelScores:
        if len(request.candidates) > 26:
            raise OptionLimitError("S1 uses one letter per option, at most 26")
        if self.modality == "multimodal" and (
            self.screenshot_path is None
            or not self.screenshot_path.is_file()
            or self.screenshot_capture_id != request.capture_id
        ):
            raise ValueError("multimodal S1 requires a screenshot bound to the request capture")
        scorer_modality = getattr(self.scorer, "modality", self.modality)
        if scorer_modality != self.modality:
            raise ValueError("S1 scorer modality does not match the decision adapter")
        options = [
            _S1Option(
                item["id"],
                "Decision",
                json.dumps(item["description"], ensure_ascii=False)[1:-1],
                "select",
            )
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
