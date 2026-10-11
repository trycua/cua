"""Scoring: distribution validation (fail-closed), argmax-vs-expected accuracy,
calibration (ECE), and a composite score combining accuracy/calibration/speed.
"""
from __future__ import annotations

from dataclasses import dataclass

from ..task import CuaTask
from .adapter import option_key

VALID_TOL = 1e-3


@dataclass
class TaskResult:
    task_id: str
    valid: bool                # distribution passed validate_distribution
    correct: bool               # every element's argmax action matches expected (False if invalid)
    per_element_correct: dict[str, bool]
    confidence: float            # predicted probability mass on the argmax action, averaged over elements
    latency_s: float | None = None
    reason: str | None = None    # set when invalid, explains why
    per_element_predicted: dict[str, str] = None    # eid -> argmax action (empty dict when invalid)
    per_element_gold: dict[str, str] = None          # eid -> gold action (empty dict when invalid)

    def __post_init__(self) -> None:
        # dataclass default_factory can't be used directly here since older call
        # sites (and jsonl round-tripping via TaskResult(**d)) may pass None
        # explicitly for these two fields -- normalize to {} either way.
        if self.per_element_predicted is None:
            self.per_element_predicted = {}
        if self.per_element_gold is None:
            self.per_element_gold = {}


def _elements_and_actions(task: CuaTask) -> dict[str, list[str]]:
    by_element: dict[str, list[str]] = {}
    for opt in task.options:
        by_element.setdefault(opt.element_id, []).append(opt.action)
    return by_element


def validate_distribution(task: CuaTask, probs: dict[str, float]) -> tuple[bool, str | None]:
    """Fail-closed: an adapter's output must cover EXACTLY the task's option
    set (no missing keys, no extras), all probabilities non-negative, and each
    element's own options must sum to ~1 (each element's action choice is a
    separate categorical decision, not one distribution over the whole page).
    Anything else is invalid; invalid counts as fully wrong in `score_task`,
    never as a runner error to skip."""
    by_element = _elements_and_actions(task)
    expected_keys = {option_key(eid, a) for eid, actions in by_element.items() for a in actions}
    got_keys = set(probs.keys())
    if got_keys != expected_keys:
        missing = expected_keys - got_keys
        extra = got_keys - expected_keys
        return False, f"option-key mismatch: missing={sorted(missing)[:5]} extra={sorted(extra)[:5]}"
    if any(v < -1e-9 for v in probs.values()):
        return False, "negative probability"
    for eid, actions in by_element.items():
        total = sum(probs[option_key(eid, a)] for a in actions)
        if abs(total - 1.0) > VALID_TOL:
            return False, f"element {eid} options sum to {total:.4f}, not ~1"
    return True, None


def score_task(task: CuaTask, probs: dict[str, float], latency_s: float | None = None) -> TaskResult:
    valid, reason = validate_distribution(task, probs)
    by_element = _elements_and_actions(task)
    if not valid:
        return TaskResult(task.id, False, False, {eid: False for eid in by_element}, 0.0, latency_s, reason)

    per_element_correct: dict[str, bool] = {}
    per_element_predicted: dict[str, str] = {}
    per_element_gold: dict[str, str] = {}
    confidences = []
    for eid, actions in by_element.items():
        scored = {a: probs[option_key(eid, a)] for a in actions}
        argmax_action = max(scored, key=scored.get)
        if eid not in task.expected:
            # `task.expected` is documented (task.py) to cover every element
            # in `task.options` -- a missing entry means the task itself is
            # malformed (a datagen bug), not that the model happened to get
            # this element wrong. Silently treating it as `gold=None` would
            # score it as simply "incorrect" instead of surfacing the real
            # upstream bug, and would silently corrupt accuracy numbers.
            raise KeyError(
                f"task {task.id!r} has no expected answer for element {eid!r} "
                f"(task.expected keys: {sorted(task.expected)}) -- malformed task"
            )
        gold = task.expected[eid]
        per_element_correct[eid] = argmax_action == gold
        per_element_predicted[eid] = argmax_action
        per_element_gold[eid] = gold
        confidences.append(scored[argmax_action])
    correct = all(per_element_correct.values())
    confidence = sum(confidences) / len(confidences) if confidences else 0.0
    return TaskResult(task.id, True, correct, per_element_correct, confidence, latency_s, None,
                      per_element_predicted, per_element_gold)


def accuracy(results: list[TaskResult]) -> float:
    if not results:
        return 0.0
    return sum(1 for r in results if r.correct) / len(results)


def element_accuracy(results: list[TaskResult]) -> float:
    total = correct = 0
    for r in results:
        total += len(r.per_element_correct)
        correct += sum(1 for v in r.per_element_correct.values() if v)
    return correct / total if total else 0.0


def expected_calibration_error(results: list[TaskResult], n_bins: int = 10) -> float:
    """Standard ECE over per-task confidence (argmax probability) vs. whether
    that argmax was correct. Invalid-distribution results contribute
    confidence=0.0/correct=False (from `score_task`), which pulls a
    frequently-invalid adapter's calibration score down too -- fail-closed
    applies to calibration, not just accuracy."""
    if not results:
        return 0.0
    bins = [[] for _ in range(n_bins)]
    for r in results:
        idx = min(n_bins - 1, int(r.confidence * n_bins))
        bins[idx].append(r)
    n = len(results)
    ece = 0.0
    for bucket in bins:
        if not bucket:
            continue
        avg_conf = sum(r.confidence for r in bucket) / len(bucket)
        avg_acc = sum(1 for r in bucket if r.correct) / len(bucket)
        ece += (len(bucket) / n) * abs(avg_conf - avg_acc)
    return ece


def composite_score(results: list[TaskResult], weight_accuracy: float = 0.6,
                    weight_calibration: float = 0.25, weight_speed: float = 0.15,
                    speed_budget_s: float = 1.0) -> float:
    """Combines accuracy, calibration (1 - ECE), and speed (fraction of tasks
    answered within `speed_budget_s`) into one number for leaderboard sorting.
    Weights are a deliberate, documented default (accuracy dominant, since a
    fast/well-calibrated wrong answer is still wrong) -- not derived from any
    external benchmark; tune as real runs accumulate."""
    acc = accuracy(results)
    cal = 1.0 - expected_calibration_error(results)
    timed = [r for r in results if r.latency_s is not None]
    speed = (sum(1 for r in timed if r.latency_s <= speed_budget_s) / len(timed)) if timed else 1.0
    return weight_accuracy * acc + weight_calibration * cal + weight_speed * speed
