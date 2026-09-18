"""Factorized Jev decision recipe for the jev-use example.

Ports the factorized question recipe from Kevin's Hermes / oh-my-pi work
(Hermes ``build_jev_questions`` and oh-my-pi ``buildFactorizedQuestions``):
instead of one opaque judgment, the chooser answers one small ``choice``
question plus cheap ``noul`` gate questions. Small questions are easier to
grade, mock, and replay than one big one, and the gates give the loop a
principled way to stop or re-observe without trusting a single argmax.

Adapted to this example's decision shape: candidates are already complete
executable Driver actions, so the factorization is ``selection`` (which
candidate) plus two gates -- ``goal_achieved`` (stop; nothing left to do) and
``needs_reobserve`` (the state changed; observe again before acting).

Any validation problem -- unknown ids, non-finite values, low confidence --
returns ``None`` (fail-open to the caller), never a half-trusted decision.
"""

from __future__ import annotations

import hashlib
import json
import time
from dataclasses import dataclass
from typing import Any, Mapping

from jev_backends import (
    JevConfig,
    JevProtocolError,
    SystemOneHttpClient,
    validate_choice_answer,
    validate_criteria,
    validate_noul_answer,
)

SELECTION_QUESTION = "selection"
GOAL_ACHIEVED_QUESTION = "goal_achieved"
NEEDS_REOBSERVE_QUESTION = "needs_reobserve"

#: Gate threshold: a gate fires only when the model is this confident.
GATE_THRESHOLD = 0.7
#: Minimum choice confidence before the caller should trust the decision.
MIN_CONFIDENCE = 0.4


def build_factorized_questions(
    candidates: Mapping[str, str], *, goal: str
) -> dict[str, dict[str, Any]]:
    """Build the factorized question set for one decision step."""
    criteria = validate_criteria(candidates)
    if not isinstance(goal, str) or not goal.strip():
        raise ValueError("goal must be a non-empty string")
    return {
        SELECTION_QUESTION: {
            "type": "choice",
            "instructions": (
                "Which complete executable action should Cua Driver run next? "
                "Select exactly one supplied candidate ID."
            ),
            "criteria": criteria,
        },
        GOAL_ACHIEVED_QUESTION: {
            "type": "noul",
            "instructions": (
                "Is the goal already achieved in the observed state, so that "
                "no further action is needed?"
            ),
        },
        NEEDS_REOBSERVE_QUESTION: {
            "type": "noul",
            "instructions": (
                "Has the observed state changed (stale capture, ambiguous "
                "target, missing element) such that fresh observation is "
                "required before acting?"
            ),
        },
    }


@dataclass(frozen=True)
class FactorizedDecision:
    selected_id: str
    confidence: float
    probabilities: dict[str, float]
    goal_achieved: float
    needs_reobserve: float
    model: str | None
    backend: str


@dataclass(frozen=True)
class DecisionPacket:
    """Replayable decision evidence: what was chosen, how sure, on what state.

    Contains no screenshots, pixels, or secrets -- only ids, scores and a
    digest of the state the decision was made on, so packets are safe to log
    and to replay against fixtures.
    """

    decision: FactorizedDecision
    latency_ms: float
    state_digest: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "selected_id": self.decision.selected_id,
            "confidence": self.decision.confidence,
            "probabilities": dict(self.decision.probabilities),
            "goal_achieved": self.decision.goal_achieved,
            "needs_reobserve": self.decision.needs_reobserve,
            "model": self.decision.model,
            "backend": self.decision.backend,
            "latency_ms": self.latency_ms,
            "state_digest": self.state_digest,
        }


def state_digest(goal: str, candidate_ids: list[str], capture_id: str | None) -> str:
    """Stable digest identifying the decision input (no secret material)."""
    canonical = json.dumps(
        {
            "goal": goal,
            "candidates": sorted(candidate_ids),
            "capture_id": capture_id,
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return "sha256:" + hashlib.sha256(canonical.encode("utf-8")).hexdigest()[:16]


def parse_factorized_decision(
    answers: Mapping[str, Any],
    candidates: Mapping[str, str],
    *,
    backend: str = "mock",
    model: str | None = None,
    min_confidence: float = MIN_CONFIDENCE,
) -> FactorizedDecision | None:
    """Parse and gate one factorized answer set. ``None`` means fail-open.

    Gate order: ``goal_achieved`` first (stop), then ``needs_reobserve``
    (look again), otherwise the validated selection. Reserved ids must be
    present for the gates to fire on them.
    """
    try:
        criteria = validate_criteria(candidates)
    except ValueError:
        return None
    if not isinstance(answers, Mapping):
        return None
    try:
        selection = validate_choice_answer(
            SELECTION_QUESTION, answers.get(SELECTION_QUESTION), set(criteria)
        )
        goal_achieved = validate_noul_answer(
            GOAL_ACHIEVED_QUESTION, answers.get(GOAL_ACHIEVED_QUESTION)
        )
        needs_reobserve = validate_noul_answer(
            NEEDS_REOBSERVE_QUESTION, answers.get(NEEDS_REOBSERVE_QUESTION)
        )
    except JevProtocolError:
        return None
    if selection.confidence < min_confidence:
        return None
    selected_id = selection.choice
    if goal_achieved >= GATE_THRESHOLD and "abstain" in criteria:
        selected_id = "abstain"
    elif needs_reobserve >= GATE_THRESHOLD and "reobserve" in criteria:
        selected_id = "reobserve"
    return FactorizedDecision(
        selected_id=selected_id,
        confidence=selection.confidence,
        probabilities=selection.probabilities,
        goal_achieved=goal_achieved,
        needs_reobserve=needs_reobserve,
        model=model,
        backend=backend,
    )


def choose_factorized(
    config: JevConfig,
    *,
    goal: str,
    observation: Mapping[str, Any],
    candidates: Mapping[str, str],
    capture_id: str | None = None,
    transport: Any | None = None,
) -> DecisionPacket | None:
    """Run one factorized decision through the configured backend.

    Returns ``None`` fail-open when the backend is skipped (missing key,
    timeout, malformed output) so the caller can fall back or abstain.
    """
    try:
        criteria = validate_criteria(candidates)
        questions = build_factorized_questions(criteria, goal=goal)
    except ValueError:
        return None
    if config.backend == "mock":
        selected = next(
            (cid for cid in criteria if cid not in {"reobserve", "abstain"}),
            "reobserve" if "reobserve" in criteria else next(iter(criteria)),
        )
        decision = FactorizedDecision(
            selected_id=selected,
            confidence=1.0,
            probabilities={cid: float(cid == selected) for cid in criteria},
            goal_achieved=0.0,
            needs_reobserve=0.0,
            model="mock",
            backend="mock",
        )
        return DecisionPacket(
            decision=decision,
            latency_ms=0.0,
            state_digest=state_digest(goal, list(criteria), capture_id),
        )
    started = time.perf_counter()
    client = SystemOneHttpClient(config, transport=transport)
    try:
        response = client.ask(
            state={"goal": goal, "observation": dict(observation)},
            questions=questions,
        )
        decision = parse_factorized_decision(
            response["answers"],
            criteria,
            backend=config.backend,
            model=response["model"],
        )
    except Exception:
        return None
    if decision is None:
        return None
    return DecisionPacket(
        decision=decision,
        latency_ms=round((time.perf_counter() - started) * 1000, 2),
        state_digest=state_digest(goal, list(criteria), capture_id),
    )
