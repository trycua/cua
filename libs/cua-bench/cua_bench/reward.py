"""Structured extraction of task reward/score from evaluation output.

Historically reward was recovered by scanning raw stdout with a brittle regular
expression (``✓ Evaluation result: [0.8]``). That breaks whenever the
evaluation harness changes its formatting, or when a result is emitted as JSON.

``parse_reward`` prefers a structured JSON envelope emitted by the evaluation
harness and only falls back to the legacy text form. When neither is present it
raises a clear ``ValueError`` instead of silently returning ``None`` or 0.0, so
a parse failure can never be mistaken for a zero reward.
"""

from __future__ import annotations

import json
import re
from typing import Any, Optional

# Single source of truth for the reward threshold that classifies a task as
# successful. Used by Environment.evaluate() telemetry and run_single_task().
DEFAULT_SUCCESS_THRESHOLD = 0.5

# Keys accepted inside a structured JSON envelope, checked in order.
_REWARD_JSON_KEYS = ("reward", "score")

# Legacy human-readable form. Brackets are optional because producers have
# emitted both ``Evaluation result: [0.8]`` and ``Evaluation result: 0.8``.
_EVAL_RESULT_RE = re.compile(
    r"Evaluation result:\s*\[?\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s*\]?"
)


def _reward_from_payload(payload: Any) -> Optional[float]:
    """Return a float reward from a decoded JSON payload, if present."""
    if isinstance(payload, dict):
        for key in _REWARD_JSON_KEYS:
            if key in payload:
                try:
                    return float(payload[key])
                except (TypeError, ValueError):
                    return None
        for value in payload.values():
            found = _reward_from_payload(value)
            if found is not None:
                return found
    elif isinstance(payload, (list, tuple)):
        for item in payload:
            found = _reward_from_payload(item)
            if found is not None:
                return found
    return None


def _structured_reward(stdout: str) -> Optional[float]:
    """Find the first JSON object in ``stdout`` that carries a reward/score."""
    for line in stdout.splitlines():
        start = line.find("{")
        end = line.rfind("}")
        if start == -1 or end <= start:
            continue
        try:
            payload = json.loads(line[start : end + 1])
        except (json.JSONDecodeError, ValueError):
            continue
        reward = _reward_from_payload(payload)
        if reward is not None:
            return reward
    return None


def parse_reward(stdout: str) -> float:
    """Extract the reward from evaluation output.

    Prefers a structured JSON envelope (any object containing a ``reward`` or
    ``score`` key), then falls back to the legacy ``Evaluation result:`` text
    form. Raises ``ValueError`` when neither is present.
    """
    if stdout:
        structured = _structured_reward(stdout)
        if structured is not None:
            return structured

        match = _EVAL_RESULT_RE.search(stdout)
        if match:
            try:
                return float(match.group(1))
            except ValueError as exc:  # pragma: no cover - regex constrains shape
                raise ValueError(
                    f"Could not parse reward {match.group(1)!r} from evaluation output"
                ) from exc

    raise ValueError(
        "No reward found in evaluation output: expected a JSON envelope with a "
        "'reward' or 'score' key, or an 'Evaluation result:' line."
    )


def try_parse_reward(stdout: str) -> Optional[float]:
    """Like :func:`parse_reward` but returns ``None`` instead of raising."""
    try:
        return parse_reward(stdout)
    except ValueError:
        return None
