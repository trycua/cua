#!/usr/bin/env python3
"""Pre-registered statistical analysis for the macOS computer-use pilot.

Compares two arms on the same tasks with the same model:

    A = "cua-driver-mcp"   (Codex CLI + Cua Driver MCP)
    B = "codex-native-cu"  (Codex CLI + Codex built-in computer use)

Input is a JSONL file with one trial per line (schema "cdb-pilot-trial/1").
Output is a Markdown report and a JSON dump of every number in it.

Only the Python standard library is used. Everything is deterministic given
``--seed``.

Usage:
    analyze.py results.jsonl [--out-md PATH] [--out-json PATH]
               [--bootstrap 10000] [--seed 1234] [--pass-k 3]
               [--headline-groups bench,probe] [--coverage-tags hover]

Library use:
    from analyze import load_rows, analyze, render_markdown
    result = analyze(load_rows("results.jsonl"))
    print(render_markdown(result))

Pre-registered rules (see also the "Method notes" section of the report):

* ``excluded=true`` trials are verified infrastructure failures. They are
  dropped from every outcome statistic; their counts are reported per arm and
  per task.
* A non-excluded trial counts as a success only if ``passed`` is true AND
  ``status`` is "completed". Non-excluded timeout / agent_error / infra_error
  trials are failures. Failures are never dropped.
* Headline tasks: ``task_group`` in --headline-groups AND ``dimension_tags``
  disjoint from --coverage-tags. Coverage-tagged tasks are reported separately.
* Headline estimand: task-macro success (mean over tasks of per-task success
  rate; each task weighted equally regardless of attempts). The A-minus-B
  difference gets a hierarchical percentile bootstrap CI (resample tasks, then
  runs within each sampled task, independently per arm).
* Per-task Fisher exact p-values are descriptive only (no multiplicity
  correction, tiny n).
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Sequence

SCHEMA = "cdb-pilot-trial/1"
ANALYSIS_SCHEMA = "cdb-pilot-analysis/1"

ARM_A = "cua-driver-mcp"
ARM_B = "codex-native-cu"
ARMS = (ARM_A, ARM_B)

STATUSES = ("completed", "timeout", "infra_error", "agent_error")
TOOL_CLASSES = (
    "observe",
    "click",
    "type",
    "key",
    "scroll",
    "drag",
    "set_value",
    "other",
)
HID_KEYS = ("move", "down", "key", "scroll")

WILSON_Z = 1.959963984540054  # two-sided 95% normal quantile
CI_LEVEL = 0.95
PILOT_MIN_RUNS = 5
PILOT_SENTENCE = "pilot-sized: not decision-bearing"

VERDICT_A_BETTER = "A better"
VERDICT_B_BETTER = "B better"
VERDICT_NONE = "no resolvable difference"

SCOPE_HEADLINE = "headline"
SCOPE_COVERAGE = "coverage"
SCOPE_OTHER = "other"
SCOPE_LABELS = {
    SCOPE_HEADLINE: "headline",
    SCOPE_COVERAGE: "coverage-tagged (not headline)",
    SCOPE_OTHER: "other non-headline (task_group not in headline groups)",
}

NO_MULTIPLICITY_NOTE = (
    "Per-task Fisher exact p-values are descriptive only. No multiplicity "
    "correction is applied and n per cell is tiny, so these p-values must not "
    "be used as decision thresholds."
)


class TrialFormatError(ValueError):
    """Raised when an input line or row does not match the trial schema."""


# --------------------------------------------------------------------------
# Small statistics helpers (implemented from scratch)
# --------------------------------------------------------------------------


def wilson_interval(successes: int, n: int, z: float = WILSON_Z) -> tuple[float, float] | None:
    """Wilson score interval for a binomial proportion. None when n == 0."""
    if n <= 0:
        return None
    if successes < 0 or successes > n:
        raise ValueError(f"successes must be in [0, n]; got {successes}/{n}")
    p = successes / n
    z2 = z * z
    denom = 1.0 + z2 / n
    center = (p + z2 / (2.0 * n)) / denom
    half = z * math.sqrt(p * (1.0 - p) / n + z2 / (4.0 * n * n)) / denom
    low = max(0.0, center - half)
    high = min(1.0, center + half)
    if successes == 0:
        low = 0.0
    if successes == n:
        high = 1.0
    return (low, high)


def fisher_exact_two_sided(a: int, b: int, c: int, d: int) -> float:
    """Two-sided Fisher exact p-value for the 2x2 table [[a, b], [c, d]].

    Uses the "sum of all tables no more likely than the observed one" rule
    (the same definition as R's fisher.test). Probabilities are compared as
    exact integers, so ties are handled without floating-point tolerance.
    """
    for name, value in (("a", a), ("b", b), ("c", c), ("d", d)):
        if value < 0 or int(value) != value:
            raise ValueError(f"cell {name} must be a non-negative integer; got {value!r}")
    row1 = a + b
    row2 = c + d
    col1 = a + c
    total = row1 + row2
    if total == 0:
        raise ValueError("Fisher exact test needs at least one observation")

    def weight(x: int) -> int:
        # Proportional to the hypergeometric pmf of x in the top-left cell.
        return math.comb(col1, x) * math.comb(total - col1, row1 - x)

    low = max(0, row1 - (total - col1))
    high = min(row1, col1)
    observed = weight(a)
    tail = sum(w for w in (weight(x) for x in range(low, high + 1)) if w <= observed)
    return min(1.0, tail / math.comb(total, row1))


def pass_hat_k(successes: int, n: int, k: int) -> float | None:
    """Probability that k runs drawn without replacement all succeed.

    C(s, k) / C(n, k). None when n < k (not estimable).
    """
    if k < 1:
        raise ValueError("k must be >= 1")
    if n < k:
        return None
    return math.comb(successes, k) / math.comb(n, k)


def percentile(sorted_values: Sequence[float], q: float) -> float:
    """Linear-interpolation percentile (q in [0, 100]) of a sorted sequence."""
    if not sorted_values:
        raise ValueError("percentile of an empty sequence")
    if len(sorted_values) == 1:
        return float(sorted_values[0])
    pos = (len(sorted_values) - 1) * q / 100.0
    lo = int(math.floor(pos))
    hi = int(math.ceil(pos))
    if lo == hi:
        return float(sorted_values[lo])
    frac = pos - lo
    return float(sorted_values[lo] * (1.0 - frac) + sorted_values[hi] * frac)


def describe(values: Iterable[float | None]) -> dict[str, Any]:
    """n, median, mean, Q1, Q3, IQR, min, max (None-valued entries are skipped)."""
    vals = sorted(float(v) for v in values if v is not None)
    n = len(vals)
    if n == 0:
        return {
            "n": 0,
            "median": None,
            "mean": None,
            "q1": None,
            "q3": None,
            "iqr": None,
            "min": None,
            "max": None,
        }
    q1 = percentile(vals, 25)
    q3 = percentile(vals, 75)
    return {
        "n": n,
        "median": percentile(vals, 50),
        "mean": math.fsum(vals) / n,
        "q1": q1,
        "q3": q3,
        "iqr": q3 - q1,
        "min": vals[0],
        "max": vals[-1],
    }


def _mean(values: Sequence[float]) -> float | None:
    return math.fsum(values) / len(values) if values else None


def verdict(ci_low: float | None, ci_high: float | None) -> str:
    """Map the 95% CI of (A minus B) to a verdict.

    "A better" if the CI is entirely above 0, "B better" if entirely below 0,
    otherwise "no resolvable difference". The CI must be a real interval; the
    caller decides what to say when no comparison is possible.
    """
    if ci_low is None or ci_high is None:
        raise ValueError("verdict() needs a confidence interval; none available")
    if ci_low > ci_high:
        raise ValueError(f"invalid confidence interval [{ci_low}, {ci_high}]")
    if ci_low > 0:
        return VERDICT_A_BETTER
    if ci_high < 0:
        return VERDICT_B_BETTER
    return VERDICT_NONE


def _bootstrap(arm_vectors: dict[str, list[list[int]]], n_boot: int, seed: int) -> dict[str, Any]:
    """Hierarchical bootstrap of the task-macro success rate.

    ``arm_vectors[arm]`` is a list (one entry per task, aligned across arms)
    of 0/1 outcome vectors, one element per non-excluded run. Each replicate
    resamples tasks with replacement, then, for every sampled task and for
    each arm independently, resamples that arm's runs with replacement (runs
    are not paired by index). Percentile CI.
    """
    arms = list(arm_vectors)
    n_tasks = len(arm_vectors[arms[0]])
    if n_tasks == 0:
        raise ValueError("bootstrap needs at least one task")
    for arm in arms:
        if len(arm_vectors[arm]) != n_tasks or any(not v for v in arm_vectors[arm]):
            raise ValueError("every task needs at least one run for every bootstrapped arm")
    rng = random.Random(seed)
    task_index = range(n_tasks)
    reps: dict[str, list[float]] = {arm: [] for arm in arms}
    diffs: list[float] = []
    for _ in range(n_boot):
        sampled = rng.choices(task_index, k=n_tasks)
        macro: dict[str, float] = {}
        for arm in arms:
            vectors = arm_vectors[arm]
            total = 0.0
            for t in sampled:
                vec = vectors[t]
                total += sum(rng.choices(vec, k=len(vec))) / len(vec)
            macro[arm] = round(total / n_tasks, 12)
            reps[arm].append(macro[arm])
        if len(arms) == 2:
            diffs.append(round(macro[arms[0]] - macro[arms[1]], 12))

    alpha = (1.0 - CI_LEVEL) / 2.0 * 100.0

    def interval(values: list[float]) -> tuple[float, float]:
        ordered = sorted(values)
        return (
            round(percentile(ordered, alpha), 12),
            round(percentile(ordered, 100.0 - alpha), 12),
        )

    out: dict[str, Any] = {
        "method": "hierarchical percentile bootstrap (tasks, then runs within task per arm)",
        "n_boot": n_boot,
        "seed": seed,
        "ci_level": CI_LEVEL,
        "n_tasks": n_tasks,
        "arms": {},
    }
    for arm in arms:
        point = math.fsum(math.fsum(v) / len(v) for v in arm_vectors[arm]) / n_tasks
        lo, hi = interval(reps[arm])
        out["arms"][arm] = {"point": point, "ci_low": lo, "ci_high": hi}
    if len(arms) == 2:
        point_diff = out["arms"][arms[0]]["point"] - out["arms"][arms[1]]["point"]
        lo, hi = interval(diffs)
        out["diff"] = {"point": round(point_diff, 12), "ci_low": lo, "ci_high": hi}
    return out


# --------------------------------------------------------------------------
# Loading and validation
# --------------------------------------------------------------------------


def _is_num(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _short(value: Any) -> str:
    text = repr(value)
    return text if len(text) <= 60 else text[:57] + "..."


def _require_num(
    where: str, obj: dict[str, Any], key: str, *, nullable: bool = False, label: str | None = None
) -> None:
    name = label or key
    if key not in obj:
        raise TrialFormatError(f"{where}: missing required field '{name}'")
    value = obj[key]
    if value is None and nullable:
        return
    if not _is_num(value):
        raise TrialFormatError(f"{where}: field '{name}' must be a number, got {_short(value)}")


def _optional_num(where: str, obj: dict[str, Any], key: str, *, label: str | None = None) -> None:
    name = label or key
    if key in obj and obj[key] is not None and not _is_num(obj[key]):
        raise TrialFormatError(
            f"{where}: field '{name}' must be a number or null, got {_short(obj[key])}"
        )


def validate_row(row: Any, where: str) -> None:
    """Raise TrialFormatError (message starts with ``where``) if row is malformed."""
    if not isinstance(row, dict):
        raise TrialFormatError(f"{where}: expected a JSON object, got {type(row).__name__}")
    if "schema" not in row:
        raise TrialFormatError(f"{where}: missing required field 'schema'")
    if row["schema"] != SCHEMA:
        raise TrialFormatError(
            f"{where}: field 'schema' must be {SCHEMA!r}, got {_short(row['schema'])}"
        )
    for key in ("trial_id", "arm", "task", "task_group", "status"):
        if key not in row:
            raise TrialFormatError(f"{where}: missing required field '{key}'")
        if not isinstance(row[key], str) or not row[key]:
            raise TrialFormatError(
                f"{where}: field '{key}' must be a non-empty string, got {_short(row[key])}"
            )
    if row["arm"] not in ARMS:
        raise TrialFormatError(
            f"{where}: field 'arm' must be one of {list(ARMS)}, got {_short(row['arm'])}"
        )
    if row["status"] not in STATUSES:
        raise TrialFormatError(
            f"{where}: field 'status' must be one of {list(STATUSES)}, got {_short(row['status'])}"
        )
    for key in ("excluded", "passed"):
        if key not in row:
            raise TrialFormatError(f"{where}: missing required field '{key}'")
        if not isinstance(row[key], bool):
            raise TrialFormatError(
                f"{where}: field '{key}' must be true or false, got {_short(row[key])}"
            )
    reason = row.get("excluded_reason")
    if reason is not None and not isinstance(reason, str):
        raise TrialFormatError(
            f"{where}: field 'excluded_reason' must be a string or null, got {_short(reason)}"
        )
    tags = row.get("dimension_tags", [])
    if not isinstance(tags, list) or not all(isinstance(t, str) for t in tags):
        raise TrialFormatError(
            f"{where}: field 'dimension_tags' must be a list of strings, got {_short(tags)}"
        )
    for key in ("confirmation_requested", "evaluator_read_suspected"):
        if key in row and not isinstance(row[key], bool):
            raise TrialFormatError(
                f"{where}: field '{key}' must be true or false, got {_short(row[key])}"
            )
    for key in (
        "run_index",
        "seed",
        "order_index",
        "score",
        "wall_s",
        "agent_wall_s",
        "steps",
        "est_cost_usd",
    ):
        _optional_num(where, row, key)

    calls = row.get("tool_calls")
    if calls is not None:
        if not isinstance(calls, dict):
            raise TrialFormatError(
                f"{where}: field 'tool_calls' must be an object, got {_short(calls)}"
            )
        _optional_num(where, calls, "total", label="tool_calls.total")
        _optional_num(where, calls, "failed", label="tool_calls.failed")
        by_class = calls.get("by_class", {})
        if not isinstance(by_class, dict):
            raise TrialFormatError(
                f"{where}: field 'tool_calls.by_class' must be an object, got {_short(by_class)}"
            )
        for cls, count in by_class.items():
            if not _is_num(count):
                raise TrialFormatError(
                    f"{where}: field 'tool_calls.by_class.{cls}' must be a number, got {_short(count)}"
                )

    tokens = row.get("tokens")
    if tokens is not None:
        if not isinstance(tokens, dict):
            raise TrialFormatError(
                f"{where}: field 'tokens' must be an object, got {_short(tokens)}"
            )
        for key in ("input", "cached_input", "output", "reasoning"):
            _optional_num(where, tokens, key, label=f"tokens.{key}")

    latency = row.get("action_latency_ms")
    if latency is not None:
        if not isinstance(latency, dict):
            raise TrialFormatError(
                f"{where}: field 'action_latency_ms' must be an object, got {_short(latency)}"
            )
        for cls, entry in latency.items():
            if not isinstance(entry, dict):
                raise TrialFormatError(
                    f"{where}: field 'action_latency_ms.{cls}' must be an object, got {_short(entry)}"
                )
            for key in ("n", "median", "p90"):
                _optional_num(where, entry, key, label=f"action_latency_ms.{cls}.{key}")

    dist = row.get("disturbance")
    if dist is not None:
        if not isinstance(dist, dict):
            raise TrialFormatError(
                f"{where}: field 'disturbance' must be an object, got {_short(dist)}"
            )
        if not isinstance(dist.get("available"), bool):
            raise TrialFormatError(
                f"{where}: field 'disturbance.available' must be true or false, got {_short(dist.get('available'))}"
            )
        if dist["available"]:
            if not isinstance(dist.get("human_input_suspected"), bool):
                raise TrialFormatError(
                    f"{where}: field 'disturbance.human_input_suspected' must be true or false when "
                    f"disturbance.available is true, got {_short(dist.get('human_input_suspected'))}"
                )
            for key in (
                "front_changes",
                "key_loss",
                "keystrokes_leaked",
                "clicks_leaked",
                "pointer_max_deviation_px",
                "pointer_deviation_episodes",
            ):
                _require_num(where, dist, key, label=f"disturbance.{key}")
            _optional_num(where, dist, "scrolls_leaked", label="disturbance.scrolls_leaked")
            hid = dist.get("hid_events")
            if not isinstance(hid, dict):
                raise TrialFormatError(
                    f"{where}: field 'disturbance.hid_events' must be an object when disturbance.available is true, got {_short(hid)}"
                )
            for key in HID_KEYS:
                _require_num(where, hid, key, label=f"disturbance.hid_events.{key}")


def _validate_rows(rows: Sequence[Any], labels: Sequence[str]) -> None:
    seen: dict[str, str] = {}
    for row, where in zip(rows, labels):
        validate_row(row, where)
        trial_id = row["trial_id"]
        if trial_id in seen:
            raise TrialFormatError(
                f"{where}: duplicate trial_id {trial_id!r} (first seen at {seen[trial_id]})"
            )
        seen[trial_id] = where


def load_rows(path: str | Path) -> list[dict[str, Any]]:
    """Read and validate a trial JSONL file. Errors name the offending line number."""
    rows: list[dict[str, Any]] = []
    labels: list[str] = []
    with open(path, "r", encoding="utf-8") as handle:
        for lineno, raw in enumerate(handle, start=1):
            text = raw.strip()
            if not text:
                continue
            try:
                row = json.loads(text)
            except json.JSONDecodeError as exc:
                raise TrialFormatError(
                    f"line {lineno}: invalid JSON ({exc.msg} at column {exc.colno})"
                ) from exc
            rows.append(row)
            labels.append(f"line {lineno}")
    _validate_rows(rows, labels)
    return rows


# --------------------------------------------------------------------------
# Trial model
# --------------------------------------------------------------------------


@dataclass
class Trial:
    trial_id: str
    arm: str
    task: str
    group: str
    tags: tuple[str, ...]
    run_index: int | None
    status: str
    excluded: bool
    excluded_reason: str | None
    raw_passed: bool
    passed: bool  # effective: raw_passed and status == "completed"
    score: float | None
    wall_s: float | None
    agent_wall_s: float | None
    steps: float | None  # tool_calls.total
    steps_field: float | None  # top-level "steps" (may also count shell commands)
    by_class: dict[str, float]
    has_tool_calls: bool
    failed_calls: float | None
    tokens: dict[str, float | None]
    cost: float | None
    latency: dict[str, dict[str, Any]]
    disturbance: dict[str, Any] | None
    confirmation: bool
    evaluator_read: bool

    @property
    def sort_key(self) -> tuple[int, float, str]:
        return (
            0 if self.run_index is not None else 1,
            self.run_index if self.run_index is not None else 0,
            self.trial_id,
        )

    @property
    def tokens_total(self) -> float | None:
        a, b = self.tokens.get("input"), self.tokens.get("output")
        return a + b if a is not None and b is not None else None


def _num_or_none(value: Any) -> float | None:
    return float(value) if _is_num(value) else None


def _to_trial(row: dict[str, Any]) -> Trial:
    calls = row.get("tool_calls") if isinstance(row.get("tool_calls"), dict) else None
    tokens = row.get("tokens") if isinstance(row.get("tokens"), dict) else {}
    passed_raw = bool(row["passed"])
    return Trial(
        trial_id=row["trial_id"],
        arm=row["arm"],
        task=row["task"],
        group=row["task_group"],
        tags=tuple(row.get("dimension_tags") or ()),
        run_index=int(row["run_index"]) if _is_num(row.get("run_index")) else None,
        status=row["status"],
        excluded=row["excluded"],
        excluded_reason=row.get("excluded_reason"),
        raw_passed=passed_raw,
        passed=passed_raw and row["status"] == "completed",
        score=_num_or_none(row.get("score")),
        wall_s=_num_or_none(row.get("wall_s")),
        agent_wall_s=_num_or_none(row.get("agent_wall_s")),
        steps=_num_or_none(calls.get("total")) if calls else None,
        steps_field=_num_or_none(row.get("steps")),
        by_class={k: float(v) for k, v in (calls.get("by_class") or {}).items()} if calls else {},
        has_tool_calls=calls is not None,
        failed_calls=_num_or_none(calls.get("failed")) if calls else None,
        tokens={
            k: _num_or_none(tokens.get(k)) for k in ("input", "cached_input", "output", "reasoning")
        },
        cost=_num_or_none(row.get("est_cost_usd")),
        latency=row.get("action_latency_ms") or {},
        disturbance=row.get("disturbance"),
        confirmation=bool(row.get("confirmation_requested", False)),
        evaluator_read=bool(row.get("evaluator_read_suspected", False)),
    )


# --------------------------------------------------------------------------
# Analysis
# --------------------------------------------------------------------------

# metric name -> accessor on Trial
METRICS: dict[str, Any] = {
    "wall_s": lambda t: t.wall_s,
    "agent_wall_s": lambda t: t.agent_wall_s,
    "steps": lambda t: t.steps,
    "steps_field": lambda t: t.steps_field,
    "tokens_input": lambda t: t.tokens.get("input"),
    "tokens_cached_input": lambda t: t.tokens.get("cached_input"),
    "tokens_output": lambda t: t.tokens.get("output"),
    "tokens_reasoning": lambda t: t.tokens.get("reasoning"),
    "tokens_total": lambda t: t.tokens_total,
    "est_cost_usd": lambda t: t.cost,
}


def _rate_block(successes: int, n: int) -> dict[str, Any]:
    interval = wilson_interval(successes, n)
    return {
        "n": n,
        "successes": successes,
        "rate": successes / n if n else None,
        "wilson_low": interval[0] if interval else None,
        "wilson_high": interval[1] if interval else None,
    }


def _build_cell(arm: str, task: str, trials: list[Trial]) -> dict[str, Any]:
    ordered = sorted(trials, key=lambda t: t.sort_key)
    kept = [t for t in ordered if not t.excluded]
    excluded = [t for t in ordered if t.excluded]
    n = len(kept)
    successes = sum(1 for t in kept if t.passed)
    cell: dict[str, Any] = {
        "arm": arm,
        "task": task,
        "n_total": len(ordered),
        "n_excluded": len(excluded),
    }
    cell.update(_rate_block(successes, n))

    scored = [t.score for t in kept if t.score is not None]
    cell["score"] = {
        "n_scored": len(scored),
        "mean": _mean(scored),
        "mean_null_as_zero": (math.fsum(scored) / n) if n else None,
    }
    cell["metrics"] = {name: describe(get(t) for t in kept) for name, get in METRICS.items()}

    with_calls = [t for t in kept if t.has_tool_calls]
    classes = list(TOOL_CLASSES) + sorted(
        {c for t in with_calls for c in t.by_class} - set(TOOL_CLASSES)
    )
    cell["tool_calls"] = {
        "n_runs_with_data": len(with_calls),
        "sum_by_class": {c: math.fsum(t.by_class.get(c, 0.0) for t in with_calls) for c in classes},
        "mean_by_class": {
            c: (math.fsum(t.by_class.get(c, 0.0) for t in with_calls) / len(with_calls))
            if with_calls
            else None
            for c in classes
        },
        "failed_calls_sum": math.fsum(t.failed_calls for t in kept if t.failed_calls is not None),
    }
    cell["failures"] = {
        "completed_but_failed": sum(1 for t in kept if t.status == "completed" and not t.passed),
        "timeout": sum(1 for t in kept if t.status == "timeout"),
        "agent_error": sum(1 for t in kept if t.status == "agent_error"),
        "infra_error_not_excluded": sum(1 for t in kept if t.status == "infra_error"),
    }
    cell["confirmation_requested"] = sum(1 for t in kept if t.confirmation)
    cell["excluded_runs"] = [
        {
            "run_index": t.run_index,
            "trial_id": t.trial_id,
            "status": t.status,
            "reason": t.excluded_reason,
        }
        for t in excluded
    ]
    cell["raw"] = {
        "run_index": [t.run_index for t in kept],
        "trial_id": [t.trial_id for t in kept],
        "passed": [1 if t.passed else 0 for t in kept],
        "status": [t.status for t in kept],
        "wall_s": [t.wall_s for t in kept],
        "steps": [t.steps for t in kept],
        "score": [t.score for t in kept],
    }
    return cell


def _task_scope(
    group: str, tags: Sequence[str], headline_groups: set[str], coverage_tags: set[str]
) -> str:
    if set(tags) & coverage_tags:
        return SCOPE_COVERAGE
    if group in headline_groups:
        return SCOPE_HEADLINE
    return SCOPE_OTHER


def _outcome_vectors(
    cells: dict[str, dict[str, Any]], arm: str, tasks: Sequence[str]
) -> list[list[int]]:
    return [list(cells[arm][t]["raw"]["passed"]) for t in tasks]


def _arm_scope_block(
    arm: str, tasks: Sequence[str], cells: dict[str, dict[str, Any]], k: int
) -> dict[str, Any]:
    with_data = [t for t in tasks if cells[arm][t]["n"] > 0]
    rates = [cells[arm][t]["rate"] for t in with_data]
    runs = sum(cells[arm][t]["n"] for t in with_data)
    succ = sum(cells[arm][t]["successes"] for t in with_data)
    k_values = {t: pass_hat_k(cells[arm][t]["successes"], cells[arm][t]["n"], k) for t in with_data}
    usable = [v for v in k_values.values() if v is not None]
    macro = _mean(rates)
    return {
        "n_tasks_with_data": len(with_data),
        "tasks_with_data": with_data,
        "n_runs": runs,
        "successes": succ,
        "micro_rate": succ / runs if runs else None,
        "task_macro_success": macro,
        "pass_at_1": macro,  # mean over tasks of C(s,1)/C(n,1) == task-macro success
        "pass_k": {
            "k": k,
            "value": _mean(usable),
            "n_tasks_used": len(usable),
            "tasks_omitted_n_lt_k": [t for t, v in k_values.items() if v is None],
            "per_task": k_values,
        },
        "min_runs_per_task": min((cells[arm][t]["n"] for t in with_data), default=None),
    }


def _scope_summary(
    scope: str,
    tasks: Sequence[str],
    cells: dict[str, dict[str, Any]],
    present_arms: Sequence[str],
    *,
    k: int,
    n_boot: int,
    seed: int,
) -> dict[str, Any]:
    tasks = sorted(tasks)
    out: dict[str, Any] = {"scope": scope, "label": SCOPE_LABELS[scope], "tasks": tasks, "arms": {}}
    for arm in ARMS:
        out["arms"][arm] = _arm_scope_block(arm, tasks, cells, k)

    # Pilot-size flag: any task with < PILOT_MIN_RUNS non-excluded runs in a
    # considered arm. Considered arms = arms that have any data at all.
    considered = list(present_arms) if present_arms else list(ARMS)
    small = []
    for t in tasks:
        for arm in considered:
            n = cells[arm][t]["n"]
            if n < PILOT_MIN_RUNS:
                small.append({"task": t, "arm": arm, "n": n})
    out["pilot_sized"] = (not tasks) or bool(small)
    out["pilot_small_cells"] = small
    out["pilot_min_runs_required"] = PILOT_MIN_RUNS
    out["smallest_cell_n"] = min(
        (cells[arm][t]["n"] for t in tasks for arm in considered), default=None
    )

    comparison: dict[str, Any] = {"possible": False}
    if len(present_arms) < 2:
        comparison["reason"] = (
            "only one arm has non-excluded trials; no comparison is possible"
            if len(present_arms) == 1
            else "no arm has non-excluded trials; no comparison is possible"
        )
    else:
        paired = [t for t in tasks if cells[ARM_A][t]["n"] > 0 and cells[ARM_B][t]["n"] > 0]
        unpaired = [t for t in tasks if t not in paired]
        comparison["paired_tasks"] = paired
        comparison["tasks_without_both_arms"] = unpaired
        if not paired:
            comparison["reason"] = (
                "no task in this scope has non-excluded runs in both arms; no comparison is possible"
            )
        else:
            boot = _bootstrap(
                {
                    ARM_A: _outcome_vectors(cells, ARM_A, paired),
                    ARM_B: _outcome_vectors(cells, ARM_B, paired),
                },
                n_boot,
                seed,
            )
            comparison.update(
                {
                    "possible": True,
                    "n_tasks": len(paired),
                    "n_runs_a": sum(cells[ARM_A][t]["n"] for t in paired),
                    "n_runs_b": sum(cells[ARM_B][t]["n"] for t in paired),
                    "macro_a": boot["arms"][ARM_A]["point"],
                    "macro_b": boot["arms"][ARM_B]["point"],
                    "macro_a_ci": [boot["arms"][ARM_A]["ci_low"], boot["arms"][ARM_A]["ci_high"]],
                    "macro_b_ci": [boot["arms"][ARM_B]["ci_low"], boot["arms"][ARM_B]["ci_high"]],
                    "diff": boot["diff"]["point"],
                    "diff_ci_low": boot["diff"]["ci_low"],
                    "diff_ci_high": boot["diff"]["ci_high"],
                    "verdict": verdict(boot["diff"]["ci_low"], boot["diff"]["ci_high"]),
                    "bootstrap": boot,
                    "ci_degenerate": boot["diff"]["ci_low"] == boot["diff"]["ci_high"],
                }
            )
    out["comparison"] = comparison

    # Single-arm descriptive CIs (task-macro bootstrap per arm over its own
    # tasks), only needed when no paired comparison (which carries per-arm CIs).
    out["arm_bootstrap"] = {}
    for arm in () if comparison["possible"] else present_arms:
        own = out["arms"][arm]["tasks_with_data"]
        if own:
            boot = _bootstrap({arm: _outcome_vectors(cells, arm, own)}, n_boot, seed)
            out["arm_bootstrap"][arm] = boot["arms"][arm]
    return out


def _efficiency(trials: list[Trial], arms: Sequence[str]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for arm in arms:
        mine = [t for t in trials if t.arm == arm and not t.excluded]
        bases = {
            "success_only": [t for t in mine if t.passed],
            "all_non_excluded": mine,
        }
        out[arm] = {
            basis: {
                "n_trials": len(group),
                "metrics": {name: describe(get(t) for t in group) for name, get in METRICS.items()},
            }
            for basis, group in bases.items()
        }
    return out


def _latency(trials: list[Trial], arms: Sequence[str]) -> dict[str, Any]:
    """Time per tool call by class: median of per-trial medians (and of per-trial p90s)."""
    out: dict[str, Any] = {}
    kept = [t for t in trials if not t.excluded]
    classes = list(TOOL_CLASSES) + sorted({c for t in kept for c in t.latency} - set(TOOL_CLASSES))
    for arm in arms:
        mine = [t for t in kept if t.arm == arm]
        per_class: dict[str, Any] = {}
        for cls in classes:
            entries = [
                t.latency[cls]
                for t in mine
                if isinstance(t.latency.get(cls), dict)
                and _is_num(t.latency[cls].get("n"))
                and t.latency[cls]["n"] > 0
                and _is_num(t.latency[cls].get("median"))
            ]
            medians = [float(e["median"]) for e in entries]
            p90s = [float(e["p90"]) for e in entries if _is_num(e.get("p90"))]
            per_class[cls] = {
                "n_trials": len(entries),
                "total_timed_actions": int(sum(e["n"] for e in entries)),
                "median_of_trial_medians_ms": percentile(sorted(medians), 50) if medians else None,
                "trial_medians_q1_ms": percentile(sorted(medians), 25) if medians else None,
                "trial_medians_q3_ms": percentile(sorted(medians), 75) if medians else None,
                "median_of_trial_p90_ms": percentile(sorted(p90s), 50) if p90s else None,
                "n_trials_with_p90": len(p90s),
            }
        out[arm] = per_class
    return out


def _any_disturbance(d: dict[str, Any]) -> bool:
    hid = d["hid_events"]
    return bool(
        d["front_changes"] > 0
        or d["key_loss"] > 0
        or d["keystrokes_leaked"] > 0
        or d["clicks_leaked"] > 0
        or any(hid[k] > 0 for k in HID_KEYS)
        or d["pointer_deviation_episodes"] > 0
    )


def _disturbance(trials: list[Trial], arms: Sequence[str]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for arm in arms:
        kept = [t for t in trials if t.arm == arm and not t.excluded]
        available = [t for t in kept if t.disturbance and t.disturbance.get("available")]
        dropped = [t for t in available if t.disturbance["human_input_suspected"]]
        used = [t for t in available if not t.disturbance["human_input_suspected"]]

        def series(fn) -> list[float]:
            return [float(fn(t.disturbance)) for t in used]

        fields = {
            "front_changes": series(lambda d: d["front_changes"]),
            "key_loss": series(lambda d: d["key_loss"]),
            "keystrokes_leaked": series(lambda d: d["keystrokes_leaked"]),
            "clicks_leaked": series(lambda d: d["clicks_leaked"]),
            "hid_events_total": series(lambda d: sum(d["hid_events"][k] for k in HID_KEYS)),
            "pointer_max_deviation_px": series(lambda d: d["pointer_max_deviation_px"]),
            "pointer_deviation_episodes": series(lambda d: d["pointer_deviation_episodes"]),
        }
        for key in HID_KEYS:
            fields[f"hid_events_{key}"] = series(lambda d, key=key: d["hid_events"][key])
        if all(_is_num(t.disturbance.get("scrolls_leaked")) for t in used) and used:
            fields["scrolls_leaked"] = series(lambda d: d["scrolls_leaked"])

        any_count = sum(1 for t in used if _any_disturbance(t.disturbance))
        interval = wilson_interval(any_count, len(used))
        out[arm] = {
            "n_non_excluded": len(kept),
            "n_available": len(available),
            "n_unavailable": len(kept) - len(available),
            "n_dropped_human_input_suspected": len(dropped),
            "n_used": len(used),
            "fields": {
                name: {
                    "mean": _mean(vals),
                    "max": max(vals) if vals else None,
                    "sum": math.fsum(vals) if vals else None,
                }
                for name, vals in fields.items()
            },
            "any_disturbance": {
                "count": any_count,
                "n": len(used),
                "fraction": any_count / len(used) if used else None,
                "wilson_low": interval[0] if interval else None,
                "wilson_high": interval[1] if interval else None,
            },
        }
    return out


def analyze(
    rows: Sequence[dict[str, Any]],
    *,
    bootstrap: int = 10000,
    seed: int = 1234,
    pass_k: int = 3,
    headline_groups: Iterable[str] = ("bench", "probe"),
    coverage_tags: Iterable[str] = ("hover",),
) -> dict[str, Any]:
    """Run the full pre-registered analysis. Returns a JSON-serializable dict."""
    if bootstrap < 1:
        raise ValueError("bootstrap must be >= 1")
    if pass_k < 1:
        raise ValueError("pass_k must be >= 1")
    rows = list(rows)
    if not rows:
        raise ValueError("no trial rows to analyze")
    _validate_rows(rows, [f"row {i}" for i in range(1, len(rows) + 1)])
    headline_set = {g for g in headline_groups if g}
    coverage_set = {t for t in coverage_tags if t}

    trials = [_to_trial(r) for r in rows]
    by_task: dict[str, list[Trial]] = defaultdict(list)
    for t in trials:
        by_task[t.task].append(t)
    task_ids = sorted(by_task)

    # Task metadata (group is expected to be constant per task; tags are unioned).
    task_meta: dict[str, dict[str, Any]] = {}
    inconsistent: list[str] = []
    for task in task_ids:
        groups = sorted({t.group for t in by_task[task]})
        tags = sorted({tag for t in by_task[task] for tag in t.tags})
        tag_sets = {t.tags for t in by_task[task]}
        if len(groups) > 1 or len({frozenset(s) for s in tag_sets}) > 1:
            inconsistent.append(task)
        group = groups[0]
        task_meta[task] = {
            "task_group": group,
            "task_groups_seen": groups,
            "dimension_tags": tags,
            "scope": _task_scope(group, tags, headline_set, coverage_set),
        }

    cells: dict[str, dict[str, Any]] = {arm: {} for arm in ARMS}
    for arm in ARMS:
        for task in task_ids:
            cells[arm][task] = _build_cell(arm, task, [t for t in by_task[task] if t.arm == arm])

    present_arms = [arm for arm in ARMS if any(not t.excluded and t.arm == arm for t in trials)]
    mode = (
        "two-arm"
        if len(present_arms) == 2
        else ("single-arm" if len(present_arms) == 1 else "no-data")
    )

    scopes: dict[str, Any] = {}
    for scope in (SCOPE_HEADLINE, SCOPE_COVERAGE, SCOPE_OTHER):
        scope_tasks = [t for t in task_ids if task_meta[t]["scope"] == scope]
        scopes[scope] = _scope_summary(
            scope, scope_tasks, cells, present_arms, k=pass_k, n_boot=bootstrap, seed=seed
        )

    # Per-task paired table (all tasks, all scopes).
    paired: dict[str, Any] = {}
    for task in task_ids:
        ca, cb = cells[ARM_A][task], cells[ARM_B][task]
        row: dict[str, Any] = {
            "scope": task_meta[task]["scope"],
            "arm_a": {k: ca[k] for k in ("n", "successes", "rate", "wilson_low", "wilson_high")},
            "arm_b": {k: cb[k] for k in ("n", "successes", "rate", "wilson_low", "wilson_high")},
            "diff": None,
            "fisher_p_two_sided": None,
        }
        if ca["n"] > 0 and cb["n"] > 0:
            row["diff"] = ca["rate"] - cb["rate"]
            row["fisher_p_two_sided"] = fisher_exact_two_sided(
                ca["successes"],
                ca["n"] - ca["successes"],
                cb["successes"],
                cb["n"] - cb["successes"],
            )
        paired[task] = row

    headline_tasks = set(scopes[SCOPE_HEADLINE]["tasks"])
    headline_trials = [t for t in trials if t.task in headline_tasks]
    efficiency = {
        "headline": _efficiency(headline_trials, ARMS),
        "all": _efficiency(trials, ARMS),
    }
    latency = {"all_non_excluded": _latency(trials, ARMS)}
    disturbance = _disturbance(trials, ARMS)

    head = scopes[SCOPE_HEADLINE]
    cmp_ = head["comparison"]
    headline_verdict: dict[str, Any] = {
        "comparison_possible": cmp_["possible"],
        "pilot_sized": head["pilot_sized"],
        "pilot_sentence": PILOT_SENTENCE if head["pilot_sized"] else None,
        "n_headline_tasks": len(head["tasks"]),
    }
    if cmp_["possible"]:
        headline_verdict.update(
            {
                "verdict": cmp_["verdict"],
                "diff": cmp_["diff"],
                "ci_low": cmp_["diff_ci_low"],
                "ci_high": cmp_["diff_ci_high"],
                "n_tasks": cmp_["n_tasks"],
                "n_runs_a": cmp_["n_runs_a"],
                "n_runs_b": cmp_["n_runs_b"],
            }
        )
    else:
        headline_verdict["verdict"] = None
        headline_verdict["reason"] = cmp_.get("reason")

    # Data summary and quality flags.
    per_arm_summary: dict[str, Any] = {}
    for arm in ARMS:
        mine = [t for t in trials if t.arm == arm]
        excl = [t for t in mine if t.excluded]
        kept = [t for t in mine if not t.excluded]
        per_arm_summary[arm] = {
            "rows": len(mine),
            "excluded": len(excl),
            "analyzed": len(kept),
            "excluded_reasons": dict(
                Counter(t.excluded_reason or "(no reason given)" for t in excl)
            ),
            "excluded_by_task": dict(Counter(t.task for t in excl)),
            "status_counts_non_excluded": {
                s: sum(1 for t in kept if t.status == s) for s in STATUSES
            },
        }
    kept_all = [t for t in trials if not t.excluded]
    data_quality = {
        "passed_true_but_status_not_completed": sorted(
            t.trial_id for t in kept_all if t.raw_passed and t.status != "completed"
        ),
        "infra_error_not_excluded": {
            arm: sum(1 for t in kept_all if t.arm == arm and t.status == "infra_error")
            for arm in ARMS
        },
        "excluded_without_reason": sorted(
            t.trial_id for t in trials if t.excluded and not t.excluded_reason
        ),
        "human_input_suspected_non_excluded": {
            arm: sum(
                1
                for t in kept_all
                if t.arm == arm
                and t.disturbance
                and t.disturbance.get("available")
                and t.disturbance.get("human_input_suspected")
            )
            for arm in ARMS
        },
        "evaluator_read_suspected_non_excluded": {
            arm: sum(1 for t in kept_all if t.arm == arm and t.evaluator_read) for arm in ARMS
        },
        "confirmation_requested_non_excluded": {
            arm: sum(1 for t in kept_all if t.arm == arm and t.confirmation) for arm in ARMS
        },
        "tasks_with_inconsistent_group_or_tags": inconsistent,
        "tasks_with_unequal_runs_between_arms": [
            t for t in task_ids if cells[ARM_A][t]["n"] != cells[ARM_B][t]["n"]
        ],
    }

    return {
        "schema": ANALYSIS_SCHEMA,
        "parameters": {
            "bootstrap": bootstrap,
            "seed": seed,
            "pass_k": pass_k,
            "headline_groups": sorted(headline_set),
            "coverage_tags": sorted(coverage_set),
            "arm_a": ARM_A,
            "arm_b": ARM_B,
            "pilot_min_runs": PILOT_MIN_RUNS,
        },
        "mode": mode,
        "present_arms": present_arms,
        "data_summary": {"n_rows": len(trials), "per_arm": per_arm_summary},
        "tasks": task_meta,
        "cells": cells,
        "paired": paired,
        "scopes": scopes,
        "headline": headline_verdict,
        "efficiency": efficiency,
        "latency": latency,
        "disturbance": disturbance,
        "data_quality": data_quality,
        "notes": {"fisher": NO_MULTIPLICITY_NOTE},
    }


# --------------------------------------------------------------------------
# Markdown rendering
# --------------------------------------------------------------------------


def _f(value: float | None, nd: int = 2) -> str:
    if value is None or (isinstance(value, float) and math.isnan(value)):
        return "n/a"
    return f"{value:.{nd}f}"


def _ci(low: float | None, high: float | None, nd: int = 2) -> str:
    if low is None or high is None:
        return "n/a"
    return f"[{low:.{nd}f}, {high:.{nd}f}]"


def _frac(successes: int, n: int) -> str:
    return f"{successes}/{n}"


def _cell_text(text: Any) -> str:
    return str(text).replace("|", "\\|").replace("\n", " ")


def _table(headers: Sequence[str], rows: Sequence[Sequence[Any]]) -> str:
    lines = [
        "| " + " | ".join(_cell_text(h) for h in headers) + " |",
        "| " + " | ".join("---" for _ in headers) + " |",
    ]
    for row in rows:
        lines.append("| " + " | ".join(_cell_text(c) for c in row) + " |")
    return "\n".join(lines)


def _list(values: Sequence[Any], empty: str = "none") -> str:
    return ", ".join(str(v) for v in values) if values else empty


def _med_iqr(stats: dict[str, Any], nd: int) -> str:
    if not stats["n"]:
        return "n/a (n=0)"
    return (
        f"{_f(stats['median'], nd)} [{_f(stats['q1'], nd)}, {_f(stats['q3'], nd)}] (n={stats['n']})"
    )


def _fmt_series(values: Sequence[Any], nd: int | None = None) -> str:
    out = []
    for v in values:
        if v is None:
            out.append("n/a")
        elif nd is None:
            out.append(str(v))
        else:
            out.append(f"{v:.{nd}f}")
    return " ".join(out) if out else "-"


def render_markdown(result: dict[str, Any]) -> str:
    params = result["parameters"]
    cells = result["cells"]
    tasks = sorted(result["tasks"])
    mode = result["mode"]
    present = result["present_arms"]
    out: list[str] = []
    add = out.append

    add("# Pilot analysis: cua-driver-mcp vs codex-native-cu")
    add("")
    add(
        f"A = `{ARM_A}` (Codex CLI + Cua Driver MCP); B = `{ARM_B}` (Codex CLI + Codex built-in computer use)."
    )
    add("")
    add(
        f"Parameters: bootstrap={params['bootstrap']}, seed={params['seed']}, pass-k={params['pass_k']}, "
        f"headline groups={_list(params['headline_groups'])}, coverage tags={_list(params['coverage_tags'])}."
    )
    add("")

    # ---- data summary
    add("## 1. Data summary")
    add("")
    summary = result["data_summary"]
    rows = []
    for arm in ARMS:
        s = summary["per_arm"][arm]
        counts = s["status_counts_non_excluded"]
        rows.append(
            [
                arm,
                s["rows"],
                s["excluded"],
                s["analyzed"],
                counts["completed"],
                counts["timeout"],
                counts["agent_error"],
                counts["infra_error"],
                _list([f"{r} ({n})" for r, n in sorted(s["excluded_reasons"].items())]),
            ]
        )
    add(
        _table(
            [
                "arm",
                "rows",
                "excluded (verified infra failure)",
                "analyzed (non-excluded)",
                "status completed",
                "timeout",
                "agent_error",
                "infra_error (not excluded)",
                "excluded reasons",
            ],
            rows,
        )
    )
    add("")
    add(
        "Excluded trials are dropped from all outcome statistics. Non-excluded timeout, agent_error and "
        "infra_error trials count as failures. Per-task excluded counts are in the `excl` column of section 5."
    )
    add("")
    if mode == "single-arm":
        add(
            f"**Only one arm (`{present[0]}`) has non-excluded trials; no comparison is possible. Single-arm descriptive tables follow.**"
        )
        add("")
    elif mode == "no-data":
        add(
            "**No arm has non-excluded trials; there is nothing to analyze beyond the exclusion counts.**"
        )
        add("")

    # ---- headline
    head = result["scopes"][SCOPE_HEADLINE]
    hv = result["headline"]
    add("## 2. Headline: task-macro success")
    add("")
    add(f"Headline tasks ({len(head['tasks'])}): {_list(head['tasks'])}.")
    cov = result["scopes"][SCOPE_COVERAGE]
    oth = result["scopes"][SCOPE_OTHER]
    add(
        f"Coverage-tagged tasks, excluded from the headline ({len(cov['tasks'])}): {_list(cov['tasks'])}."
    )
    add(f"Other non-headline tasks ({len(oth['tasks'])}): {_list(oth['tasks'])}.")
    add("")
    add(_headline_block(head, hv, present))
    add("")
    if hv["pilot_sized"]:
        add(f"> {PILOT_SENTENCE}")
        add("")
        if head["tasks"]:
            small = head["pilot_small_cells"]
            add(
                f"Headline tasks need at least {PILOT_MIN_RUNS} non-excluded runs per arm; "
                f"{len(small)} task x arm cell(s) fall short (smallest cell n={head['smallest_cell_n']}): "
                + _list([f"{c['task']}/{c['arm']} n={c['n']}" for c in small])
                + "."
            )
        else:
            add("There are no headline tasks.")
    else:
        add(f"Every headline task has at least {PILOT_MIN_RUNS} non-excluded runs per arm.")
    add("")

    add("### Non-headline scopes (descriptive; no verdict)")
    add("")
    nh_rows = []
    for sc in (cov, oth):
        cmp_ = sc["comparison"]
        a, b = sc["arms"][ARM_A], sc["arms"][ARM_B]
        nh_rows.append(
            [
                sc["label"],
                len(sc["tasks"]),
                f"{_f(a['task_macro_success'])} (tasks={a['n_tasks_with_data']}, runs={a['n_runs']})",
                f"{_f(b['task_macro_success'])} (tasks={b['n_tasks_with_data']}, runs={b['n_runs']})",
                _f(cmp_["diff"], 3) if cmp_.get("possible") else "n/a",
                _ci(cmp_.get("diff_ci_low"), cmp_.get("diff_ci_high"), 3)
                if cmp_.get("possible")
                else "n/a",
                cmp_.get("n_tasks", "n/a") if cmp_.get("possible") else "n/a",
            ]
        )
    add(
        _table(
            [
                "scope",
                "tasks",
                f"A macro ({ARM_A})",
                f"B macro ({ARM_B})",
                "A - B (paired tasks)",
                "95% bootstrap CI",
                "paired tasks",
            ],
            nh_rows,
        )
    )
    add("")

    # ---- paired per task
    add("## 3. Per-task paired comparison (A minus B)")
    add("")
    if mode != "two-arm":
        add(
            "Not available: a per-task comparison needs non-excluded trials in both arms. See section 5 for per-arm values."
        )
    else:
        prow = []
        for task in tasks:
            p = result["paired"][task]
            a, b = p["arm_a"], p["arm_b"]
            prow.append(
                [
                    task,
                    p["scope"],
                    f"{_frac(a['successes'], a['n'])} = {_f(a['rate'])} {_ci(a['wilson_low'], a['wilson_high'])}",
                    f"{_frac(b['successes'], b['n'])} = {_f(b['rate'])} {_ci(b['wilson_low'], b['wilson_high'])}",
                    _f(p["diff"], 3),
                    _f(p["fisher_p_two_sided"], 4),
                ]
            )
        add(
            _table(
                [
                    "task",
                    "scope",
                    "A successes/n = rate [Wilson 95%]",
                    "B successes/n = rate [Wilson 95%]",
                    "A - B success rate",
                    "Fisher exact p (two-sided, descriptive)",
                ],
                prow,
            )
        )
        add("")
        add(f"Note: {NO_MULTIPLICITY_NOTE}")
    add("")

    # ---- pass@1 / pass^k
    add(f"## 4. pass@1 and pass^{params['pass_k']}")
    add("")
    add(
        f"pass^k is the probability that all k runs succeed, estimated per task as C(s,k)/C(n,k) and averaged "
        f"over tasks with n >= k (k={params['pass_k']}). pass@1 is the mean per-task success rate, which equals "
        "the task-macro success. micro = pooled successes / pooled runs (tasks with more runs weigh more)."
    )
    add("")
    prow = []
    for sc in (head, cov, oth):
        for arm in ARMS:
            blk = sc["arms"][arm]
            pk = blk["pass_k"]
            prow.append(
                [
                    sc["scope"],
                    arm,
                    blk["n_tasks_with_data"],
                    blk["n_runs"],
                    _f(blk["pass_at_1"], 3),
                    _f(blk["micro_rate"], 3),
                    f"{_f(pk['value'], 3)} (tasks used={pk['n_tasks_used']})",
                    _list(pk["tasks_omitted_n_lt_k"]),
                ]
            )
    add(
        _table(
            [
                "scope",
                "arm",
                "tasks",
                "runs",
                "pass@1 (= task-macro)",
                "micro success",
                f"pass^{params['pass_k']}",
                f"tasks omitted from pass^{params['pass_k']} (n < k)",
            ],
            prow,
        )
    )
    add("")

    # ---- per arm x task
    add("## 5. Per arm x task (non-excluded runs; failures never dropped)")
    add("")
    add("### 5a. Outcomes")
    add("")
    orow = []
    for task in tasks:
        meta = result["tasks"][task]
        for arm in ARMS:
            c = cells[arm][task]
            fl = c["failures"]
            orow.append(
                [
                    task,
                    f"{meta['task_group']}/{meta['scope']}",
                    arm,
                    c["n_excluded"],
                    c["n"],
                    c["successes"],
                    f"{_f(c['rate'])} {_ci(c['wilson_low'], c['wilson_high'])}",
                    f"{_f(c['score']['mean'], 3)} (n={c['score']['n_scored']})",
                    fl["completed_but_failed"],
                    fl["timeout"],
                    fl["agent_error"],
                    fl["infra_error_not_excluded"],
                    c["confirmation_requested"],
                ]
            )
    add(
        _table(
            [
                "task",
                "group/scope",
                "arm",
                "excl",
                "n runs",
                "successes",
                "success rate [Wilson 95%]",
                "mean score (n scored)",
                "failed, status completed",
                "timeout",
                "agent_error",
                "infra_error (not excluded)",
                "confirmation requested",
            ],
            orow,
        )
    )
    add("")
    add("Mean score is over runs with a non-null score only; `mean_null_as_zero` is in the JSON.")
    add("")

    dist_specs = [
        ("wall_s", "wall_s (seconds)", 1),
        ("steps", "steps (tool_calls.total)", 1),
        ("tokens_input", "tokens: input", 0),
        ("tokens_cached_input", "tokens: cached input", 0),
        ("tokens_output", "tokens: output", 0),
        ("est_cost_usd", "estimated cost (USD)", 4),
    ]
    for idx, (key, title, nd) in enumerate(dist_specs):
        add(f"### 5{chr(ord('b') + idx)}. {title}")
        add("")
        drow = []
        for task in tasks:
            for arm in ARMS:
                s = cells[arm][task]["metrics"][key]
                drow.append(
                    [
                        task,
                        arm,
                        s["n"],
                        _f(s["median"], nd),
                        _f(s["mean"], nd),
                        _f(s["q1"], nd),
                        _f(s["q3"], nd),
                        _f(s["min"], nd),
                        _f(s["max"], nd),
                    ]
                )
        add(_table(["task", "arm", "n", "median", "mean", "Q1", "Q3", "min", "max"], drow))
        add("")
    add(
        "Quartiles use linear interpolation (IQR = Q3 - Q1). Cost and token counts only include runs where the field is present (see n)."
    )
    add("")

    add("### 5h. Tool calls by class (mean per run; failed = summed over runs)")
    add("")
    classes = list(TOOL_CLASSES)
    extra = sorted(
        {c for arm in ARMS for t in tasks for c in cells[arm][t]["tool_calls"]["mean_by_class"]}
        - set(classes)
    )
    classes += extra
    trow = []
    for task in tasks:
        for arm in ARMS:
            c = cells[arm][task]
            tc = c["tool_calls"]
            trow.append(
                [task, arm, tc["n_runs_with_data"]]
                + [_f(tc["mean_by_class"].get(cls), 1) for cls in classes]
                + [_f(c["metrics"]["steps"]["mean"], 1), _f(tc["failed_calls_sum"], 0)]
            )
    add(_table(["task", "arm", "n runs"] + classes + ["total (mean)", "failed calls (sum)"], trow))
    add("")

    add("## 6. Raw per-run values (variance is visible; runs in run_index order)")
    add("")
    rrow = []
    for task in tasks:
        for arm in ARMS:
            c = cells[arm][task]
            raw = c["raw"]
            noncomp = [
                f"run {ri if ri is not None else '?'}={st}"
                for ri, st in zip(raw["run_index"], raw["status"])
                if st != "completed"
            ]
            excl = [
                f"run {e['run_index'] if e['run_index'] is not None else '?'}: {e['reason'] or 'no reason'}"
                for e in c["excluded_runs"]
            ]
            rrow.append(
                [
                    task,
                    arm,
                    _fmt_series(raw["passed"]),
                    _fmt_series(raw["wall_s"], 1),
                    _fmt_series(raw["steps"], 0),
                    _list(noncomp, "-"),
                    _list(excl, "-"),
                ]
            )
    add(
        _table(
            [
                "task",
                "arm",
                "passed (1/0)",
                "wall_s",
                "steps",
                "non-completed status",
                "excluded runs",
            ],
            rrow,
        )
    )
    add("")

    # ---- efficiency
    add("## 7. Efficiency")
    add("")
    add(
        "Medians with [Q1, Q3] and n. Conditional = successful runs only; unconditional = all non-excluded runs. "
        "Pooled across tasks, so the task mix of each arm affects these numbers; compare per-task values in section 5 "
        "before drawing conclusions. Total tokens = input + output (assumes cached input is a subset of input and "
        "reasoning a subset of output)."
    )
    add("")
    eff_specs = [
        ("wall_s", "wall_s (s)", 1),
        ("steps", "steps", 1),
        ("tokens_input", "tokens in", 0),
        ("tokens_cached_input", "tokens cached", 0),
        ("tokens_output", "tokens out", 0),
        ("tokens_total", "tokens total", 0),
        ("est_cost_usd", "cost (USD)", 4),
    ]
    for scope_key, title in (
        ("headline", "Headline tasks"),
        ("all", "All tasks (non-excluded trials)"),
    ):
        add(f"### {title}")
        add("")
        erow = []
        for arm in ARMS:
            for basis, label in (
                ("success_only", "success only"),
                ("all_non_excluded", "all non-excluded"),
            ):
                blk = result["efficiency"][scope_key][arm][basis]
                erow.append(
                    [arm, label, blk["n_trials"]]
                    + [_med_iqr(blk["metrics"][k], nd) for k, _, nd in eff_specs]
                )
        add(_table(["arm", "basis", "trials"] + [t for _, t, _ in eff_specs], erow))
        add("")

    # ---- latency
    add("## 8. Time per tool call by class")
    add("")
    add(
        "Per trial, `action_latency_ms` gives a median and p90 per action class. Here: the median of the per-trial "
        "medians (and the median of per-trial p90s) across trials, with the number of trials that timed that class. "
        "All non-excluded trials."
    )
    add("")
    lat = result["latency"]["all_non_excluded"]
    lat_classes = list(TOOL_CLASSES) + sorted(
        {c for arm in ARMS for c in lat[arm]} - set(TOOL_CLASSES)
    )
    lrow = []
    for cls in lat_classes:
        row = [cls]
        for arm in ARMS:
            e = lat[arm].get(cls, {"n_trials": 0})
            if not e["n_trials"]:
                row += ["n/a (0 trials)", "n/a"]
            else:
                row += [
                    f"{_f(e['median_of_trial_medians_ms'], 1)} ms (trials={e['n_trials']}, actions={e['total_timed_actions']})",
                    f"{_f(e['median_of_trial_p90_ms'], 1)} ms",
                ]
        lrow.append(row)
    add(
        _table(
            [
                "class",
                f"A median of trial medians ({ARM_A})",
                "A median of trial p90",
                f"B median of trial medians ({ARM_B})",
                "B median of trial p90",
            ],
            lrow,
        )
    )
    add("")

    # ---- disturbance
    add("## 9. Disturbance")
    add("")
    add(
        "Only non-excluded trials with `disturbance.available` true and `human_input_suspected` false are used. "
        "Cells show mean / max per trial. hid_events total = move + down + key + scroll. Any disturbance = "
        "front_changes, key_loss, keystrokes_leaked or clicks_leaked > 0, or any hid event, or "
        "pointer_deviation_episodes > 0."
    )
    add("")
    drow = []
    dist = result["disturbance"]
    for arm in ARMS:
        d = dist[arm]
        fl = d["fields"]

        def mm(name: str, nd: int = 2) -> str:
            f = fl.get(name)
            if f is None:
                return "n/a"
            return f"{_f(f['mean'], nd)} / {_f(f['max'], nd)}"

        any_ = d["any_disturbance"]
        drow.append(
            [
                arm,
                d["n_used"],
                d["n_dropped_human_input_suspected"],
                d["n_unavailable"],
                mm("front_changes"),
                mm("key_loss"),
                mm("keystrokes_leaked"),
                mm("clicks_leaked"),
                mm("scrolls_leaked"),
                mm("hid_events_total"),
                mm("pointer_max_deviation_px", 1),
                f"{any_['count']}/{any_['n']} = {_f(any_['fraction'])} {_ci(any_['wilson_low'], any_['wilson_high'])}",
            ]
        )
    add(
        _table(
            [
                "arm",
                "trials used",
                "dropped: human_input_suspected",
                "unavailable",
                "front_changes",
                "key_loss",
                "keystrokes_leaked",
                "clicks_leaked",
                "scrolls_leaked (not in 'any')",
                "hid_events total",
                "pointer_max_deviation_px",
                "any disturbance [Wilson 95%]",
            ],
            drow,
        )
    )
    add("")

    # ---- data quality
    dq = result["data_quality"]
    add("## 10. Data-quality flags")
    add("")
    qrow = [
        [
            "trials with passed=true but status != completed (counted as failures)",
            _list(dq["passed_true_but_status_not_completed"]),
        ],
        ["excluded trials with no excluded_reason", _list(dq["excluded_without_reason"])],
        [
            "infra_error trials NOT excluded (counted as failures)",
            _per_arm(dq["infra_error_not_excluded"]),
        ],
        [
            "human_input_suspected (non-excluded; dropped from disturbance only)",
            _per_arm(dq["human_input_suspected_non_excluded"]),
        ],
        [
            "evaluator_read_suspected (non-excluded; kept in all statistics)",
            _per_arm(dq["evaluator_read_suspected_non_excluded"]),
        ],
        [
            "confirmation_requested (non-excluded)",
            _per_arm(dq["confirmation_requested_non_excluded"]),
        ],
        [
            "tasks with inconsistent task_group or dimension_tags across rows",
            _list(dq["tasks_with_inconsistent_group_or_tags"]),
        ],
        [
            "tasks with unequal non-excluded run counts between arms",
            _list(dq["tasks_with_unequal_runs_between_arms"]),
        ],
    ]
    add(_table(["flag", "value"], qrow))
    add("")

    # ---- method notes
    add("## 11. Method notes")
    add("")
    for line in _method_notes(params):
        add(f"- {line}")
    add("")
    return "\n".join(out)


def _per_arm(mapping: dict[str, int]) -> str:
    return ", ".join(f"{arm}: {mapping[arm]}" for arm in ARMS)


def _headline_block(head: dict[str, Any], hv: dict[str, Any], present: Sequence[str]) -> str:
    lines: list[str] = []
    cmp_ = head["comparison"]
    if cmp_["possible"]:
        n_a, n_b = cmp_["n_runs_a"], cmp_["n_runs_b"]
        lines.append(
            _table(
                [
                    "arm",
                    "tasks (paired)",
                    "runs",
                    "task-macro success",
                    "95% hierarchical bootstrap CI",
                ],
                [
                    [
                        f"A: {ARM_A}",
                        cmp_["n_tasks"],
                        n_a,
                        _f(cmp_["macro_a"], 3),
                        _ci(*cmp_["macro_a_ci"], 3),
                    ],
                    [
                        f"B: {ARM_B}",
                        cmp_["n_tasks"],
                        n_b,
                        _f(cmp_["macro_b"], 3),
                        _ci(*cmp_["macro_b_ci"], 3),
                    ],
                ],
            )
        )
        lines.append("")
        lines.append(
            f"Difference A - B (task-macro success): **{_f(cmp_['diff'], 3)}**, 95% CI "
            f"**{_ci(cmp_['diff_ci_low'], cmp_['diff_ci_high'], 3)}**, n = {cmp_['n_tasks']} paired headline tasks "
            f"({n_a} runs in A, {n_b} runs in B)."
        )
        lines.append("")
        lines.append(
            f"Verdict: **{cmp_['verdict']}** (A = {ARM_A}, B = {ARM_B}; the verdict only reports whether the 95% CI of A - B "
            "excludes 0)."
        )
        if cmp_["ci_degenerate"]:
            lines.append("")
            lines.append(
                "Warning: the bootstrap CI has zero width. With so few tasks and runs the resampling distribution is "
                "degenerate; do not read this as certainty."
            )
        if cmp_["tasks_without_both_arms"]:
            lines.append("")
            lines.append(
                "Headline tasks left out of the comparison because one arm has no non-excluded runs: "
                + _list(cmp_["tasks_without_both_arms"])
                + "."
            )
        lines.append("")
        lines.append(
            "The CI is a percentile bootstrap (resample tasks with replacement, then runs within each sampled task, "
            "independently per arm). With a handful of tasks and runs it tends to be too narrow."
        )
    else:
        lines.append(f"No comparison is possible: {cmp_.get('reason', 'insufficient data')}.")
        lines.append("")
        rows = []
        for arm in ARMS:
            if arm not in present:
                rows.append([arm, 0, 0, "n/a", "n/a"])
                continue
            blk = head["arms"][arm]
            boot = head["arm_bootstrap"].get(arm)
            rows.append(
                [
                    arm,
                    blk["n_tasks_with_data"],
                    blk["n_runs"],
                    _f(blk["task_macro_success"], 3),
                    _ci(boot["ci_low"], boot["ci_high"], 3) if boot else "n/a",
                ]
            )
        lines.append(
            _table(
                [
                    "arm",
                    "headline tasks with data",
                    "runs",
                    "task-macro success",
                    "95% hierarchical bootstrap CI (descriptive)",
                ],
                rows,
            )
        )
    return "\n".join(lines)


def _method_notes(params: dict[str, Any]) -> list[str]:
    return [
        "Success = passed AND status == completed, among non-excluded trials. Excluded trials (verified infrastructure failures) are dropped from outcome statistics and counted per arm and per task.",
        "Task-macro success: mean over tasks of the per-task success rate; each task weighted equally regardless of attempts.",
        "Headline set: task_group in the headline groups and no dimension tag in the coverage tags. Tasks that fail this test are reported separately and never enter the headline.",
        "Difference CI: percentile bootstrap, tasks resampled with replacement, then runs within each sampled task resampled with replacement, independently per arm (runs are not paired by index). Only tasks with non-excluded runs in both arms enter the comparison.",
        "Wilson 95% intervals for proportions; Fisher exact two-sided p-values (sum of tables no more probable than the observed one). "
        + NO_MULTIPLICITY_NOTE,
        f"pass^k = mean over tasks (with n >= k) of C(s,k)/C(n,k); here k={params['pass_k']}.",
        f"The pilot-size warning is printed (as a block quote in section 2) whenever any headline task has fewer than {params['pilot_min_runs']} non-excluded runs in either arm.",
        "Latency: the median of per-trial medians is reported (not an n-weighted median of medians), with the number of trials.",
        "Quartiles use linear interpolation between order statistics.",
        "Results are deterministic given the seed.",
    ]


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _csv(value: str) -> list[str]:
    return [part.strip() for part in value.split(",") if part.strip()]


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Pre-registered analysis of the macOS computer-use pilot (cua-driver-mcp vs codex-native-cu).",
    )
    parser.add_argument("results", help="JSONL file, one cdb-pilot-trial/1 record per line")
    parser.add_argument(
        "--out-md", help="write the Markdown report here (default: print to stdout)"
    )
    parser.add_argument("--out-json", help="write all numbers as JSON here")
    parser.add_argument(
        "--bootstrap", type=int, default=10000, help="bootstrap replicates (default 10000)"
    )
    parser.add_argument("--seed", type=int, default=1234, help="bootstrap RNG seed (default 1234)")
    parser.add_argument("--pass-k", type=int, default=3, help="k for pass^k (default 3)")
    parser.add_argument(
        "--headline-groups",
        default="bench,probe",
        help="comma-separated task_group values in the headline (default bench,probe)",
    )
    parser.add_argument(
        "--coverage-tags",
        default="hover",
        help="comma-separated dimension tags that are coverage-only (default hover)",
    )
    return parser


def _write(path: str, text: str) -> None:
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        rows = load_rows(args.results)
        result = analyze(
            rows,
            bootstrap=args.bootstrap,
            seed=args.seed,
            pass_k=args.pass_k,
            headline_groups=_csv(args.headline_groups),
            coverage_tags=_csv(args.coverage_tags),
        )
    except (TrialFormatError, ValueError, OSError) as exc:
        print(f"analyze.py: error: {exc}", file=sys.stderr)
        return 2
    markdown = render_markdown(result)
    try:
        payload = json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n"
        if args.out_md:
            _write(args.out_md, markdown + "\n")
        else:
            print(markdown)
        if args.out_json:
            _write(args.out_json, payload)
    except (ValueError, OSError) as exc:
        print(f"analyze.py: error: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
