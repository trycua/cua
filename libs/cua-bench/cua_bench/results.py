"""Result and trajectory formats shared with Harbor (harbor-framework/harbor).

cua-bench's own files keep their layout; these add what Harbor tooling reads:

* ``trajectory.json`` per variant: an ATIF-v1.8 trajectory (Agent Trajectory
  Interchange Format) built from the variant's trace events, screenshots as
  image content parts under ``imgs/``;
* the Harbor ``TrialResult`` fields in ``result.json`` (``id``,
  ``task_name``, ``trial_name``, ``verifier_result.rewards``,
  ``exception_info``, ``agent_info``, per-phase timing), see
  :func:`harbor_trial_fields`;
* ``pass_at_k`` for ``--attempts`` in ``summary.json`` (:func:`pass_at_k`,
  Harbor's definition: binary rewards, unbiased estimator).
"""

from __future__ import annotations

import json
import math
import traceback
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path
from typing import Any, Iterable, Optional, TypedDict

ATIF_VERSION = "ATIF-v1.8"


class Span(TypedDict):
    """A phase's start and end (ISO 8601, UTC)."""

    started_at: str
    finished_at: str


class HarborTrialFields(TypedDict):
    """The Harbor ``TrialResult`` keys of ``result.json`` (additive)."""

    #: A random id for this trial.
    id: str
    #: The task directory name.
    task_name: str
    #: ``<task>_v<variant>``, with ``_a<attempt>`` for repeats.
    trial_name: str
    #: ``file://`` URI of the variant's output directory.
    trial_uri: Optional[str]
    #: Always ``cua-bench``.
    source: str
    #: The agent label, version and model (``{name, provider}``).
    agent_info: dict
    #: ``{"rewards": {"reward": <float>, ...}}`` (numeric evaluation keys included), or ``null``.
    verifier_result: Optional[dict]
    #: Exception type, message, traceback and time when the variant raised, else ``null``.
    exception_info: Optional[dict]
    #: When the trial started.
    started_at: Optional[str]
    #: When the trial finished.
    finished_at: Optional[str]
    #: Sandbox start and task setup.
    environment_setup: Optional[Span]
    #: Always ``null`` (agents run in the cb process).
    agent_setup: None
    #: The agent's (or the oracle's) run.
    agent_execution: Optional[Span]
    #: The evaluation.
    verifier: Optional[Span]


class SummaryResult(TypedDict):
    """One row of ``summary.json`` ``results``: a variant's outcome."""

    task: str
    variant: int
    status: str
    reward: Optional[float]
    pool: Optional[str]
    duration_s: float
    error: Optional[str]
    #: ``container`` or ``vm``.
    kind: Optional[str]
    #: The engine (gvisor, runc, qemu, lume, kubevirt) when one was chosen,
    #: else ``null`` (the SDK picked).
    runtime: Optional[str]
    image_variant: Optional[str]
    image_digest: Optional[str]
    attempt: int
    retries: int


class SummaryStats(TypedDict):
    """Harbor-compatible job statistics."""

    #: Variants that completed.
    n_completed: int
    #: Variants with no reward that did not complete (the run broke).
    n_errored: int
    #: Infrastructure retries across the run.
    n_retries: int


class SummaryTarget(TypedDict):
    """Where variants ran: one row per (on, kind, runtime, image variant, image)."""

    on: str
    kind: Optional[str]
    runtime: Optional[str]
    image_variant: Optional[str]
    #: The pinned digest when known, else the image reference.
    image: Optional[str]
    #: Variants that ran there.
    count: int


class Summary(TypedDict):
    """``summary.json``: the run's totals, written next to each variant's directory."""

    #: Layout version; a rename or removal bumps it, new keys do not.
    schema_version: int
    #: Variants run.
    total: int
    #: Variants that completed.
    completed: int
    #: Variants that failed or were cancelled.
    failed: int
    #: Mean reward over variants with a reward, or ``null``.
    avg_reward: Optional[float]
    #: One row per variant (``SummaryResult``).
    results: list[SummaryResult]
    #: The number of trials (Harbor), equal to ``total``.
    n_total_trials: int
    #: Harbor-compatible statistics (``SummaryStats``).
    stats: SummaryStats
    #: ``{"<k>": <pass@k>}`` with ``--attempts`` above 1 (binary rewards, unbiased estimator), else ``null``.
    pass_at_k: Optional[dict]
    #: Where the variants ran (``SummaryTarget``).
    targets: list[SummaryTarget]


def _now_utc() -> datetime:
    return datetime.now(timezone.utc)


def _iso(moment: Optional[datetime]) -> Optional[str]:
    return moment.isoformat() if moment is not None else None


def _iso_or_none(value: Any) -> Optional[str]:
    """ATIF timestamps must be ISO 8601; anything else is dropped."""
    if not isinstance(value, str) or not value:
        return None
    try:
        datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return value


def _save_image(image: Any, directory: Path, name: str) -> Optional[str]:
    """Write a PIL image or PNG bytes; return the path relative to the variant dir."""
    try:
        directory.mkdir(parents=True, exist_ok=True)
        target = directory / name
        if isinstance(image, (bytes, bytearray)):
            target.write_bytes(bytes(image))
        else:
            buf = BytesIO()
            image.save(buf, format="PNG")
            target.write_bytes(buf.getvalue())
        return f"{directory.name}/{name}"
    except Exception:  # noqa: BLE001 - an image is optional
        return None


def _image_parts(images: Iterable[Any], img_dir: Path, prefix: str) -> list[dict]:
    parts = []
    for index, image in enumerate(images or []):
        rel = _save_image(image, img_dir, f"{prefix}_{index}.png")
        if rel:
            parts.append({"type": "image", "source": {"media_type": "image/png", "path": rel}})
    return parts


def _tool_calls(output: Any, step: int) -> tuple[list[dict], str]:
    """cua-agent style output items to ATIF tool calls and message text."""
    calls, texts = [], []
    for index, item in enumerate(output or []):
        if not isinstance(item, dict):
            continue
        kind = item.get("type")
        if kind == "message":
            for part in item.get("content") or []:
                if isinstance(part, dict) and part.get("text"):
                    texts.append(str(part["text"]))
        elif kind in ("computer_call", "function_call"):
            arguments = item.get("action") if kind == "computer_call" else item.get("arguments")
            if isinstance(arguments, str):
                try:
                    arguments = json.loads(arguments)
                except ValueError:
                    arguments = {"raw": arguments}
            calls.append(
                {
                    "tool_call_id": str(
                        item.get("call_id") or item.get("id") or f"call_{step}_{index}"
                    ),
                    "function_name": "computer"
                    if kind == "computer_call"
                    else str(item.get("name")),
                    "arguments": arguments if isinstance(arguments, dict) else {"value": arguments},
                }
            )
    return calls, "\n".join(texts)


def build_atif(
    rows: list[dict],
    output_dir: Path,
    *,
    session_id: Optional[str],
    agent_name: str,
    model_name: Optional[str],
    description: str,
    evaluation: Any = None,
    reward: Optional[float] = None,
    agent_version: str = "cua-bench",
) -> dict:
    """An ATIF trajectory dict from cua-bench trace rows (``Tracing._rows``)."""
    img_dir = output_dir / "imgs"
    steps: list[dict] = []
    prompt_tokens = completion_tokens = 0
    saw_usage = False

    def add(source: str, message: Any, timestamp: Optional[str], **extra: Any) -> None:
        step = {"step_id": len(steps) + 1, "timestamp": _iso_or_none(timestamp), "source": source}
        step["message"] = message
        step.update({k: v for k, v in extra.items() if v not in (None, [], {})})
        steps.append(step)

    for index, row in enumerate(rows):
        name = str(row.get("event_name", ""))
        try:
            data = json.loads(row.get("data_json") or "{}")
        except ValueError:
            data = {}
        stamp = row.get("timestamp")
        images = _image_parts(row.get("data_images") or [], img_dir, f"atif_{index}")
        if name == "reset":
            add("user", [{"type": "text", "text": description}, *images], stamp)
        elif name in ("agent_step", "agent_thinking"):
            calls, text = _tool_calls(data.get("output"), index)
            usage = data.get("usage") or {}
            metrics = {}
            if isinstance(usage, dict) and usage:
                saw_usage = True
                p = int(usage.get("prompt_tokens") or usage.get("input_tokens") or 0)
                c = int(usage.get("completion_tokens") or usage.get("output_tokens") or 0)
                prompt_tokens += p
                completion_tokens += c
                metrics = {"prompt_tokens": p, "completion_tokens": c}
            observation = {"results": [{"content": images}]} if images else None
            add(
                "agent",
                text or data.get("thinking") or "",
                stamp,
                model_name=data.get("model") or model_name,
                reasoning_content=data.get("thinking") if name == "agent_thinking" else None,
                tool_calls=calls,
                observation=observation,
                metrics=metrics,
            )
        elif name in ("step:after", "solve"):
            action = data.get("action") or ("oracle solution" if name == "solve" else name)
            calls = [
                {
                    "tool_call_id": f"call_{index}",
                    "function_name": "computer" if name != "solve" else "solve_task",
                    "arguments": {"action": action},
                }
            ]
            observation = {"results": [{"content": images}]} if images else None
            add(
                "agent", "", stamp, model_name=model_name, tool_calls=calls, observation=observation
            )
        elif name == "step:before":
            continue
        elif name == "evaluate":
            add(
                "system",
                f"evaluate() returned {json.dumps(data.get('result'), default=repr)}",
                stamp,
            )
        else:
            add(
                "system",
                [
                    {"type": "text", "text": f"{name}: {json.dumps(data, default=repr)[:2000]}"},
                    *images,
                ],
                stamp,
            )

    if not steps:
        add("user", description, None)
    final = {"total_steps": sum(1 for s in steps if s["source"] == "agent")}
    if saw_usage:
        final["total_prompt_tokens"] = prompt_tokens
        final["total_completion_tokens"] = completion_tokens
    final["extra"] = {"reward": reward, "evaluation": _jsonable(evaluation)}
    agent = {"name": agent_name, "version": agent_version}
    if model_name:
        agent["model_name"] = model_name
    return {
        "schema_version": ATIF_VERSION,
        "session_id": session_id,
        "agent": agent,
        "steps": steps,
        "final_metrics": final,
    }


def write_atif(rows: list[dict], output_dir: Path, **kwargs: Any) -> Path:
    path = Path(output_dir) / "trajectory.json"
    path.write_text(json.dumps(build_atif(rows, Path(output_dir), **kwargs), indent=2))
    return path


def _jsonable(value: Any) -> Any:
    try:
        json.dumps(value)
        return value
    except (TypeError, ValueError):
        return repr(value)


def harbor_trial_fields(
    *,
    task: str,
    variant: int,
    attempt: int,
    output_dir: str,
    reward: Optional[float],
    evaluation: Any,
    error: Optional[BaseException],
    started_at: datetime,
    finished_at: datetime,
    setup_started: Optional[datetime],
    timing: dict,
    agent_label: str,
    model: Optional[str],
) -> HarborTrialFields:
    """Harbor ``TrialResult`` fields (additive keys of cua-bench's result.json)."""
    import uuid

    rewards = None
    if reward is not None:
        rewards = {"reward": reward}
        if isinstance(evaluation, dict):
            numeric = {k: v for k, v in evaluation.items() if isinstance(v, (int, float))}
            rewards.update(numeric)
    provider, _, name = (model or "").rpartition("/")
    trial = f"{task}_v{variant}" + (f"_a{attempt}" if attempt else "")

    def phase(key: str) -> Optional[dict]:
        span = timing.get(key)
        if not span:
            return None
        return {"started_at": _iso(span[0]), "finished_at": _iso(span[1])}

    setup = timing.get("setup")
    environment_setup = None
    if setup_started is not None:
        environment_setup = {
            "started_at": _iso(setup_started),
            "finished_at": _iso(setup[1]) if setup else _iso(finished_at),
        }
    exception_info = None
    if error is not None:
        exception_info = {
            "exception_type": type(error).__name__,
            "exception_message": str(error),
            "exception_traceback": "".join(
                traceback.format_exception(type(error), error, error.__traceback__)
            ),
            "occurred_at": _iso(finished_at),
        }
    return {
        "id": str(uuid.uuid4()),
        "task_name": task,
        "trial_name": trial,
        "trial_uri": Path(output_dir).resolve().as_uri() if output_dir else None,
        "source": "cua-bench",
        "agent_info": {
            "name": agent_label,
            "version": "cua-bench",
            "model_info": {"name": name or model, "provider": provider or None} if model else None,
        },
        "verifier_result": {"rewards": rewards} if rewards is not None else None,
        "exception_info": exception_info,
        "started_at": _iso(started_at),
        "finished_at": _iso(finished_at),
        "environment_setup": environment_setup,
        "agent_setup": None,
        "agent_execution": phase("agent"),
        "verifier": phase("verifier"),
    }


def _pass_at_k_for_task(n: int, c: int, k: int) -> float:
    if n - c < k:
        return 1.0
    return 1.0 - math.comb(n - c, k) / math.comb(n, k)


def eligible_k(max_k: int) -> list[int]:
    """k = 1, powers of two from 2, and multiples of 5 (Harbor's set plus 1)."""
    ks = {1}
    k = 2
    while k <= max_k:
        ks.add(k)
        k *= 2
    k = 5
    while k <= max_k:
        ks.add(k)
        k += 5
    return sorted(k for k in ks if k <= max_k)


def pass_at_k(results: Iterable[Any]) -> Optional[dict[str, float]]:
    """pass@k over task variants with several attempts; binary rewards only.

    ``None`` when there is one attempt per variant or a reward is not 0/1
    (Harbor's rule). A missing reward (an errored attempt) counts as 0.
    """
    groups: dict[tuple[str, int], list[int]] = {}
    for result in results:
        reward = getattr(result, "reward", None)
        if reward is not None and reward not in (0, 1, 0.0, 1.0):
            return None
        key = (result.task, result.variant)
        groups.setdefault(key, []).append(1 if reward == 1 else 0)
    if not groups:
        return None
    n_min = min(len(v) for v in groups.values())
    if n_min < 2:
        return None
    return {
        str(k): sum(_pass_at_k_for_task(len(v), sum(v), k) for v in groups.values()) / len(groups)
        for k in eligible_k(n_min)
    }
