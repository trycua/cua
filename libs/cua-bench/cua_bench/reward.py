"""Structured extraction of task reward/score from evaluation output.

Historically reward was recovered by scanning raw stdout with a brittle regular
expression (``✓ Evaluation result: [0.8]``). That breaks whenever the
evaluation harness changes its formatting, or when a result is emitted as JSON.

``parse_reward`` only trusts an explicit, machine-readable envelope that the
evaluator prefixes with the ``CUA_BENCH_RESULT`` marker:

    CUA_BENCH_RESULT {"reward": 0.9}

The JSON is limited to a top-level ``reward`` or ``score`` key. Arbitrary JSON
an agent or tool happens to print (a model confidence ``score``, an OCR score,
an API response) is ignored, so it can never be mistaken for the task reward.
Evaluators produce the envelope with :func:`emit_result_envelope` and pass their
raw ``evaluate`` result through :func:`coerce_reward` first.
When several envelopes are present the last one wins. When no envelope is
present the legacy ``Evaluation result:`` text form is used. When neither is
present a clear ``ValueError`` is raised instead of silently returning ``None``
or 0.0, so a parse failure can never be mistaken for a zero reward.
"""

from __future__ import annotations

import json
import re
from typing import Any, Iterator, Optional

# Single source of truth for the reward threshold that classifies a task as
# successful. Used by Environment.evaluate() telemetry and run_single_task().
DEFAULT_SUCCESS_THRESHOLD = 0.5

# Explicit marker the evaluator prefixes to its machine-readable envelope.
# Only JSON attached to this marker is treated as a task result.
_RESULT_MARKER = "CUA_BENCH_RESULT"

# Keys accepted inside a structured envelope, checked in order.
_REWARD_JSON_KEYS = ("reward", "score")

# Legacy human-readable form. Brackets are optional because producers have
# emitted both ``Evaluation result: [0.8]`` and ``Evaluation result: 0.8``.
_EVAL_RESULT_RE = re.compile(
    r"Evaluation result:\s*\[?\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s*\]?"
)


def _reward_from_payload(payload: Any) -> Optional[float]:
    """Return the top-level reward/score from a decoded envelope, if present."""
    if not isinstance(payload, dict):
        return None
    for key in _REWARD_JSON_KEYS:
        if key in payload:
            try:
                return float(payload[key])
            except (TypeError, ValueError):
                return None
    return None


def coerce_reward(result: Any) -> Optional[float]:
    """Coerce an evaluation result into a scalar reward, if one is present.

    Evaluation functions return a scalar, a one-element list of scores, or a
    ``{"reward"/"score": ...}`` mapping. Returns ``None`` when no numeric reward
    can be recovered (for example a bare boolean success flag).
    """
    if isinstance(result, bool):
        return None
    if isinstance(result, (int, float)):
        return float(result)
    if isinstance(result, dict):
        return _reward_from_payload(result)
    if isinstance(result, (list, tuple)):
        for item in result:
            value = coerce_reward(item)
            if value is not None:
                return value
    return None


def emit_result_envelope(reward: float) -> str:
    """Format a machine-readable result line for :func:`parse_reward`.

    Returns ``CUA_BENCH_RESULT {"reward": <reward>}``. Evaluators should print
    this next to the human-readable result so reward extraction no longer
    depends on log formatting.
    """
    return f"{_RESULT_MARKER} {json.dumps({'reward': float(reward)})}"


def _decode_marker_payloads(stdout: str) -> Iterator[Any]:
    """Yield decoded JSON objects attached to ``CUA_BENCH_RESULT`` markers."""
    for line in stdout.splitlines():
        index = line.find(_RESULT_MARKER)
        if index == -1:
            continue
        remainder = line[index + len(_RESULT_MARKER) :].lstrip(":= \t")
        start = remainder.find("{")
        end = remainder.rfind("}")
        if start == -1 or end <= start:
            continue
        try:
            yield json.loads(remainder[start : end + 1])
        except (json.JSONDecodeError, ValueError):
            continue


def _structured_reward(stdout: str) -> Optional[float]:
    """Return the reward from the last ``CUA_BENCH_RESULT`` envelope carrying one."""
    reward: Optional[float] = None
    for payload in _decode_marker_payloads(stdout):
        found = _reward_from_payload(payload)
        if found is not None:
            reward = found
    return reward


def parse_reward(stdout: str) -> float:
    """Extract the reward from evaluation output.

    Prefers the explicit ``CUA_BENCH_RESULT`` JSON envelope (taking the last one
    emitted), then falls back to the legacy ``Evaluation result:`` text form.
    Raises ``ValueError`` when neither is present.
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
        "No reward found in evaluation output: expected a 'CUA_BENCH_RESULT' "
        "JSON envelope with a 'reward' or 'score' key, or an 'Evaluation result:' line."
    )


def try_parse_reward(stdout: str) -> Optional[float]:
    """Like :func:`parse_reward` but returns ``None`` instead of raising."""
    try:
        return parse_reward(stdout)
    except ValueError:
        return None
