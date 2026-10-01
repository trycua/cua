"""The uniform interface any model under test implements, regardless of
architecture. The runner and scorer only ever talk to this interface, never
to a model's own code directly.
"""
from __future__ import annotations

from abc import ABC, abstractmethod

from ..task import CuaTask


def option_key(element_id: str, action: str) -> str:
    """Stable key for one (element, action) option within a task. Each element
    has at most one option per action in this benchmark's option-set design
    (see datagen/generator.py's docstring), so element_id+action is unique."""
    return f"{element_id}::{action}"


class ModelAdapter(ABC):
    """A model under test. `predict` must return a probability for *every*
    option in `task.options` (see eval.scoring.validate_distribution for the
    exact contract: covers exactly the option set, non-negative, sums to ~1
    per element -- fail-closed: anything else scores as wrong, not as an
    error the runner swallows)."""

    name: str = "adapter"

    @abstractmethod
    def predict(self, task: CuaTask, modality: str) -> dict[str, float]:
        """Return {option_key(element_id, action): probability, ...} covering
        every option in `task.options`. `modality` is "text" or "multimodal";
        the adapter must not be handed the artifact the other modality would
        use (the runner enforces this by stripping it before calling here)."""
        raise NotImplementedError

    def predict_logits(self, task: CuaTask, modality: str) -> dict[str, float] | None:
        """Optional: return PRE-softmax per-option scores (same keys as
        `predict`) for adapters that have them, enabling real temperature
        scaling (Guo et al. 2017) on genuine logits instead of an
        approximation. Default `None` means "not available" -- a caller
        wanting temperature calibration should fall back to treating
        log(predict()) as a logit surrogate when this returns None, a
        documented approximation rather than a real logit. A future adapter
        can override this to return the model's actual pre-softmax scores."""
        return None


class RandomAdapter(ModelAdapter):
    """Trivial baseline: uniform-random probability over each element's own
    options. Useful for smoke-testing the runner/scorer without a real
    model."""

    name = "random"

    def __init__(self, seed: int = 0) -> None:
        import random
        self._rng = random.Random(seed)

    def predict(self, task: CuaTask, modality: str) -> dict[str, float]:
        by_element: dict[str, list[str]] = {}
        for opt in task.options:
            by_element.setdefault(opt.element_id, []).append(opt.action)
        out: dict[str, float] = {}
        for element_id, actions in by_element.items():
            weights = [self._rng.random() for _ in actions]
            total = sum(weights) or 1.0
            for action, w in zip(actions, weights):
                out[option_key(element_id, action)] = w / total
        return out


class OracleAdapter(ModelAdapter):
    """Always predicts the gold answer with probability 1. Used by tests to
    verify the scorer reports perfect accuracy/calibration on a known-correct
    input."""

    name = "oracle"

    def predict(self, task: CuaTask, modality: str) -> dict[str, float]:
        by_element: dict[str, list[str]] = {}
        for opt in task.options:
            by_element.setdefault(opt.element_id, []).append(opt.action)
        out: dict[str, float] = {}
        for element_id, actions in by_element.items():
            if element_id not in task.expected:
                # `task.expected` is documented (task.py) to cover every
                # element in `task.options`. OracleAdapter exists specifically
                # so tests can assert perfect accuracy on a known-correct
                # input -- silently falling back to a uniform distribution
                # here would instead make a malformed task look like an
                # ordinary uniform-guess result, defeating the point of using
                # Oracle as a ground-truth check.
                raise KeyError(
                    f"task {task.id!r} has no expected answer for element "
                    f"{element_id!r} (task.expected keys: {sorted(task.expected)}) "
                    f"-- malformed task"
                )
            gold = task.expected[element_id]
            if gold not in actions:
                raise ValueError(
                    f"task {task.id!r}: expected action {gold!r} for element "
                    f"{element_id!r} is not among its own options {actions} "
                    f"-- malformed task"
                )
            for action in actions:
                out[option_key(element_id, action)] = 1.0 if action == gold else 0.0
        return out
