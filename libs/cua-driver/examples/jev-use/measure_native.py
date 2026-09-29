"""Measure native jev-use decision accuracy by candidate-set size (#4312).

Two subcommands produce the same per-decision records and the same table:

``replay`` runs the native task specs offline over captured, sanitized
``get_window_state`` fixtures. For each task it walks the task's declared
steps: at every decision point it builds the real candidate set and
``cua.jev_choice_request_v2`` from the fixture, asks the provider, records the
decision, then applies the *correct* action's effect to the fixture (select a
radio, toggle a checkbox, fill a field) and continues, so every decision point
is scored even after a wrong choice. ``--cap-order`` compares strict
depth-first capping with relevance capping, and ``--order shuffled`` also
presents each set's executable candidates in a seeded random order, to see
whether position matters. Each repetition uses a distinct capture ID.

``logs`` reads the JSONL logs that the native runners write under
``verify_native.py`` (one ``start`` event, then ``step`` and ``outcome``
events carrying ``candidate_count``, ``expected_ids``, the chosen candidate,
confidence, and ``decide_ms``).

A decision is ``correct`` when the chosen candidate performs a declared step
that is due (its ``:foreground`` variant counts), ``wrong_action`` when it is
another executable candidate, or ``reobserve``, ``abstain``, or ``error``.
``target_missing`` counts decisions whose set did not offer any due step, for
example because the cap dropped it. Sizes are the full set sent to the
provider, including ``reobserve`` and ``abstain``, bucketed as ``~4`` (up to
8), ``~12`` (9 to 18), and ``~24`` (19 and more; the cap is 24 + 2).

Examples::

    python measure_native.py replay --fixture gtk3:24:fixtures/native/gtk3-window-state-density-24-v1.json \\
        --provider s1 --reps 5 --cap-order depth_first --cap-order relevance --out /tmp/replay
    python measure_native.py logs --platform linux /tmp/evidence/runs --out /tmp/live
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import random
import statistics
import sys
import time
from dataclasses import replace
from pathlib import Path
from typing import Any, Callable, Iterable, Mapping

BASE = Path(__file__).resolve().parent
sys.path.insert(0, str(BASE / "python"))

from choose_action import validate_request  # noqa: E402
from native import NativeObservation  # noqa: E402
from native_tasks import HARNESSES, TASK_KINDS, NativeStep, NativeTask, native_choice_request, native_task  # noqa: E402
from sources import NativeAccessibilitySource  # noqa: E402
from tasks import TaskSources  # noqa: E402

RESULTS = ("correct", "wrong_action", "reobserve", "abstain", "error")
BUCKETS = ("~4", "~12", "~24")
NOTE_TEXT = "jev-use native note"

Provider = Callable[[dict, NativeTask, TaskSources, NativeStep, list], tuple]


def bucket(candidate_count: int) -> str:
    if candidate_count <= 8:
        return "~4"
    if candidate_count <= 18:
        return "~12"
    return "~24"


def classify(selected: str | None, expected: Iterable[str], error: bool = False) -> str:
    if error:
        return "error"
    if selected is None or selected == "abstain":
        return "abstain"
    if selected == "reobserve":
        return "reobserve"
    return "correct" if selected.removesuffix(":foreground") in set(expected) else "wrong_action"


# -- providers ---------------------------------------------------------------


def mock_provider(request, task, sources, plan, history):
    from jev_adapter import choose_mock_for_task

    return choose_mock_for_task(task, sources, plan.candidates, history)


def s1_provider(request, task, sources, plan, history):
    from s1_service import choose_s1_service

    return choose_s1_service(request)


def live_provider(request, task, sources, plan, history):
    from decision_models import DecisionRequest, TypeSafeDecisionModel, choose
    from typesafe_sdk import TypeSafeClient

    with TypeSafeClient() as client:
        result = choose(TypeSafeDecisionModel(client), DecisionRequest.from_validated(validate_request(request)))
    if result.kind == "error":
        raise RuntimeError(f"provider decision failed: {result.reason}")
    return result.selected_id, result.confidence, dict(result.probabilities)


PROVIDERS: Mapping[str, Provider] = {"mock": mock_provider, "s1": s1_provider, "live": live_provider}


# -- replay ------------------------------------------------------------------


def _capture_id(*parts: Any) -> str:
    digest = hashlib.sha256(json.dumps(parts, sort_keys=True).encode()).hexdigest()
    return f"capture_replay_{digest[:32]}"


def apply_effect(elements: list[dict[str, Any]], sources: TaskSources, candidate_id: str, task: NativeTask) -> None:
    """Apply one correct action's effect to the fixture's elements in place."""
    assert sources.ax is not None
    base = candidate_id.removesuffix(":foreground")
    control_id, _, parameter = base.partition(":set:")
    control = next((item for item in sources.ax.controls if item.id == control_id), None)
    if control is None:
        raise ValueError(f"no control for {candidate_id}")
    element = next(item for item in elements if item.get("element_index") == control.element_index)
    if control.action == "set_text":
        element["value"] = next(p.value for p in task.parameters if p.name == parameter)
    elif control.action == "toggle":
        element["selected"] = not bool(element.get("selected"))
    elif control.action == "select":
        element["selected"] = True
    # A press changes no element the task set depends on (the counter label is
    # not actionable), so nothing else changes.


def shuffled(plan: NativeStep, seed: str) -> NativeStep:
    executable = [candidate for candidate in plan.candidates if candidate.tool is not None]
    reserved = [candidate for candidate in plan.candidates if candidate.tool is None]
    random.Random(seed).shuffle(executable)
    return replace(plan, candidates=executable + reserved)


def replay_task(
    task: NativeTask,
    payload: Mapping[str, Any],
    platform: str,
    provider: Provider,
    *,
    rep: int,
    order: str,
    context: Mapping[str, Any],
) -> list[dict[str, Any]]:
    elements = copy.deepcopy(list(payload["elements"]))
    history: list[dict[str, Any]] = []
    records = []
    for step in range(1, task.max_steps + 1):
        expected = task.expected_next(history)
        if not expected:
            break
        capture_id = _capture_id(dict(context), task.cap_order, order, rep, step)
        window_state = {**payload, "elements": elements, "capture_id": capture_id}
        observation = NativeObservation.from_window_state(
            window_state, expected_pid=payload["pid"], expected_window_id=payload["window_id"]
        )
        ax = NativeAccessibilitySource.from_observation(
            observation, platform, redact=task.redact_text, text_method=task.text_method  # type: ignore[arg-type]
        )
        sources = TaskSources(ax=ax)
        plan = task.plan(sources)
        if order == "shuffled":
            plan = shuffled(plan, capture_id)
        request = native_choice_request(task, sources, plan, history)
        validate_request(request)
        offered = {candidate.id.removesuffix(":foreground") for candidate in plan.candidates}
        started = time.perf_counter()
        error = None
        selected: str | None = None
        confidence: float | None = None
        try:
            selected, confidence, _ = provider(request, task, sources, plan, history)
        except Exception as exc:  # a provider failure is a scored decision
            error = f"{type(exc).__name__}: {str(exc)[:120]}"
        decide_ms = round((time.perf_counter() - started) * 1000, 2)
        executable = sum(1 for candidate in plan.candidates if candidate.tool is not None)
        records.append({
            **context,
            "mode": "replay",
            "task": task.id,
            "cap_order": task.cap_order,
            "order": order,
            "rep": rep,
            "step": step,
            "candidate_count": len(plan.candidates),
            "executable_count": executable,
            "dropped": plan.stats.dropped,
            "bucket": bucket(len(plan.candidates)),
            "expected_ids": expected,
            "expected_offered": bool(set(expected) & offered),
            "selected": selected,
            "result": classify(selected, expected, error is not None),
            "confidence": confidence,
            "decide_ms": decide_ms,
            "request_sha256": hashlib.sha256(
                json.dumps({**request, "capture_id": ""}, sort_keys=True).encode()
            ).hexdigest()[:16],
            **({"error": error} if error else {}),
        })
        # Teacher forcing: continue from the correct action, offered or not.
        correct = next((item for item in expected if item in offered), expected[0])
        apply_effect(elements, sources, correct, task)
        history.append(task.history_entry(step, correct, outcome=plan.outcomes.get(correct, "completed")))
    return records


def parse_fixture(spec: str) -> tuple[str, int | None, Path]:
    harness, density, path = spec.split(":", 2)
    if harness not in HARNESSES:
        raise argparse.ArgumentTypeError(f"unknown harness {harness}")
    return harness, (None if density in ("", "0", "base") else int(density)), Path(path)


def run_replay(args: argparse.Namespace) -> list[dict[str, Any]]:
    provider = PROVIDERS[args.provider]
    records: list[dict[str, Any]] = []
    for spec in args.fixture:
        harness, density, path = parse_fixture(spec)
        payload = json.loads(path.read_text(encoding="utf-8"))
        platform = HARNESSES[harness].platform
        for kind in args.task or TASK_KINDS:
            for cap_order in args.cap_order or ["relevance"]:
                for order in args.order or ["element"]:
                    for rep in range(1, args.reps + 1):
                        task = replace(
                            native_task(f"{harness}-{kind}", Path("/nonexistent"), note_text=NOTE_TEXT),
                            cap_order=cap_order,
                        )
                        context = {"platform": platform, "harness": harness, "density": density,
                                   "provider": args.provider, "language": "python", "fixture": path.name}
                        records.extend(
                            replay_task(task, payload, platform, provider, rep=rep, order=order, context=context)
                        )
                        print(json.dumps({k: records[-1][k] for k in ("harness", "density", "task", "cap_order",
                                                                         "order", "rep")}), flush=True)
    return records


# -- runner logs -------------------------------------------------------------


def decisions_from_log(path: Path, platform: str | None = None) -> list[dict[str, Any]]:
    """Per-decision records from one native runner JSONL log."""
    events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    start = next((event for event in events if event.get("event") == "start"), {})
    density = None
    stem = path.stem
    if "-d" in stem and stem.rsplit("-d", 1)[1].isdigit():
        density = int(stem.rsplit("-d", 1)[1])
    context = {
        "mode": "live",
        "platform": platform or start.get("platform"),
        "harness": str(start.get("task", "")).split("-", 1)[0],
        "density": density,
        "provider": start.get("provider"),
        "language": start.get("language"),
        "task": start.get("task"),
        "run": stem,
        "cap_order": "relevance",
        "order": "element",
    }
    records = []
    for event in events:
        if "candidate_count" not in event or "expected_ids" not in event:
            continue
        kind = event.get("event")
        if kind == "step":
            selected = event.get("candidate")
        elif kind == "outcome" and event.get("outcome") == "abstained":
            selected = "abstain"
        elif kind == "outcome" and event.get("phase") == "decide":
            selected = None
        else:
            continue
        error = kind == "outcome" and event.get("phase") == "decide"
        expected = event["expected_ids"]
        records.append({
            **context,
            "step": event.get("step"),
            "candidate_count": event["candidate_count"],
            "dropped": (event.get("compose") or {}).get("dropped"),
            "bucket": bucket(event["candidate_count"]),
            "expected_ids": expected,
            "expected_offered": event.get("expected_offered"),
            "selected": selected,
            "result": classify(selected, expected, error),
            "confidence": event.get("confidence"),
            "decide_ms": event.get("decide_ms"),
        })
    return records


def run_logs(args: argparse.Namespace) -> list[dict[str, Any]]:
    records = []
    for root in args.paths:
        files = [root] if root.is_file() else sorted(root.rglob("*.jsonl"))
        for path in files:
            if path.name in {"results.jsonl", "decisions.jsonl"}:
                continue
            records.extend(decisions_from_log(path, args.platform))
    return records


# -- table -------------------------------------------------------------------


def _median(values: list[float]) -> float | None:
    return round(statistics.median(values), 2) if values else None


def _p95(values: list[float]) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    return round(ordered[min(len(ordered) - 1, int(round(0.95 * (len(ordered) - 1))))], 2)


def aggregate(records: list[dict[str, Any]], keys: tuple[str, ...]) -> list[dict[str, Any]]:
    groups: dict[tuple, list[dict[str, Any]]] = {}
    for record in records:
        groups.setdefault(tuple(record.get(key) for key in keys), []).append(record)

    def sort_key(item: tuple) -> tuple:
        return tuple(
            BUCKETS.index(value) if key == "bucket" and value in BUCKETS else (str(value) if value is not None else "")
            for key, value in zip(keys, item)
        )

    rows = []
    for group in sorted(groups, key=sort_key):
        items = groups[group]
        counts = {result: sum(1 for item in items if item["result"] == result) for result in RESULTS}
        confidences = [item["confidence"] for item in items if isinstance(item.get("confidence"), (int, float))]
        latencies = [item["decide_ms"] for item in items if isinstance(item.get("decide_ms"), (int, float))]
        sizes = [item["candidate_count"] for item in items]
        rows.append({
            **dict(zip(keys, group)),
            "decisions": len(items),
            **counts,
            "accuracy": round(counts["correct"] / len(items), 4) if items else None,
            "target_missing": sum(1 for item in items if item.get("expected_offered") is False),
            "size_min": min(sizes),
            "size_max": max(sizes),
            "confidence_median": _median(confidences),
            "confidence_min": round(min(confidences), 2) if confidences else None,
            "decide_ms_median": _median(latencies),
            "decide_ms_p95": _p95(latencies),
        })
    return rows


def markdown(rows: list[dict[str, Any]], keys: tuple[str, ...]) -> str:
    header = [*keys, "size", "decisions", "correct", "accuracy", "wrong", "reobserve", "abstain", "error",
              "target missing", "conf. median", "conf. min", "decide ms median", "decide ms p95"]
    lines = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    for row in rows:
        size = f"{row['size_min']}" if row["size_min"] == row["size_max"] else f"{row['size_min']}–{row['size_max']}"
        accuracy = f"{row['accuracy'] * 100:.1f}%" if row["accuracy"] is not None else ""
        cells = [*(str(row[key]) if row[key] is not None else "base" if key == "density" else "" for key in keys),
                 size, str(row["decisions"]), str(row["correct"]), accuracy, str(row["wrong_action"]),
                 str(row["reobserve"]), str(row["abstain"]), str(row["error"]), str(row["target_missing"]),
                 str(row["confidence_median"] if row["confidence_median"] is not None else ""),
                 str(row["confidence_min"] if row["confidence_min"] is not None else ""),
                 str(row["decide_ms_median"] if row["decide_ms_median"] is not None else ""),
                 str(row["decide_ms_p95"] if row["decide_ms_p95"] is not None else "")]
        lines.append("| " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def write_outputs(records: list[dict[str, Any]], out: Path, keys: tuple[str, ...]) -> str:
    out.mkdir(parents=True, exist_ok=True)
    with (out / "decisions.jsonl").open("w", encoding="utf-8") as stream:
        for record in records:
            stream.write(json.dumps(record, sort_keys=True) + "\n")
    rows = aggregate(records, keys)
    (out / "table.json").write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    table = markdown(rows, keys)
    (out / "table.md").write_text(table, encoding="utf-8")
    return table


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    replay_parser = sub.add_parser("replay", help="replay captured fixtures offline through a provider")
    replay_parser.add_argument("--fixture", action="append", required=True,
                               help="HARNESS:DENSITY:PATH (DENSITY 0 or base for the ordinary task mode)")
    replay_parser.add_argument("--provider", choices=sorted(PROVIDERS), default="mock")
    replay_parser.add_argument("--task", action="append", choices=TASK_KINDS)
    replay_parser.add_argument("--cap-order", action="append", choices=("depth_first", "relevance"))
    replay_parser.add_argument("--order", action="append", choices=("element", "shuffled"))
    replay_parser.add_argument("--reps", type=int, default=5)
    replay_parser.add_argument("--out", type=Path, required=True)
    logs_parser = sub.add_parser("logs", help="score native runner JSONL logs")
    logs_parser.add_argument("paths", nargs="+", type=Path)
    logs_parser.add_argument("--platform", choices=("macos", "windows", "linux"))
    logs_parser.add_argument("--out", type=Path, required=True)
    for each in (replay_parser, logs_parser):
        each.add_argument("--group-by", default="platform,provider,bucket",
                          help="comma-separated record keys for the table rows")
    args = parser.parse_args(argv)
    if args.command == "replay" and args.provider == "s1":
        from s1_service import s1_service_url

        s1_service_url()
    records = run_replay(args) if args.command == "replay" else run_logs(args)
    keys = tuple(key.strip() for key in args.group_by.split(",") if key.strip())
    print(write_outputs(records, args.out, keys), end="")


if __name__ == "__main__":
    main()
