"""Run one native jev-use task against a desktop window (RFC #4268, Phase 1).

The runner attaches to an already running application (``--pid``), finds the
task's window by title, and loops: read the app-owned oracle, observe the
window once with ``get_window_state`` (tree and screenshot together), build
the closed candidate set, send a validated ``cua.jev_choice_request_v2`` to
the chooser, dispatch the one selected element-bound action, and read the
oracle again. The chooser only ever selects a supplied ID.

Built-in tasks drive the AppKit, WPF, WinUI3, or GTK3 harness launched in task
mode (``CUA_APPKIT_TASK_STATE``, ``CUA_WPF_TASK_STATE``,
``CUA_WINUI3_TASK_STATE``, or ``CUA_GTK3_TASK_STATE`` set to ``<path>``); ``verify_native.py`` launches it, runs this runner, and
checks the state file independently.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
import uuid
from pathlib import Path
from typing import Any

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from choose_action import validate_request
from core import Candidate, VisualObservationError, parse_visual_regions, validate_choice
from decision_models import DecisionRequest, TypeSafeDecisionModel, choose
from driver_env import driver_environment
from jev_adapter import choose_mock_for_task
from native import NativeObservation, NativeObservationError
from native_roles import Platform
from native_tasks import (
    NATIVE_TASK_IDS,
    NativeTask,
    OracleError,
    native_task,
    native_choice_request,
    visual_fallback_reason,
)
from s1_service import S1ServiceError, choose_s1_service, s1_service_url
from run import (
    Driver,
    DriverToolError,
    background_refusal_code,
    decision_timing_fields,
    supports_capture_bound_click,
)
from sources import NativeAccessibilitySource, VisualRegionSource
from tasks import TaskSources

STALE_TOKEN_CODES = frozenset({"stale_element_token"})
# A partial or truncated tree is observed once more with this walk budget.
REOBSERVE_TIMEOUT_MS = 5_000


def host_platform() -> Platform:
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("win"):
        return "windows"
    return "linux"


def is_stale_token_error(error: BaseException) -> bool:
    if isinstance(error, DriverToolError) and error.code in STALE_TOKEN_CODES:
        return True
    return "element_token is stale" in str(error)


def add_phase(phase_timings: dict[str, float] | None, name: str, started: float) -> float:
    """Add the time since ``started`` to a step phase; return the elapsed milliseconds."""
    elapsed_ms = (time.perf_counter() - started) * 1000
    if phase_timings is not None:
        phase_timings[name] = phase_timings.get(name, 0.0) + elapsed_ms
    return elapsed_ms


def native_timing_fields(
    started: float, phase_timings: dict[str, float], provider_decision_ms: float
) -> dict[str, Any]:
    """The browser runner's decision-phase fields for one native step.

    ``provider_decision_ms`` is ``decide_ms``. ``visual_observe_ms`` is the
    ``parse_visual_regions`` phase only: the native screenshot and ``capture_id``
    come from the same ``get_window_state`` call that produced the semantic
    observation, so that capture is inside ``semantic_observe_ms``.
    """
    return {
        **decision_timing_fields(
            decision_ms=round((time.perf_counter() - started) * 1000, 2),
            semantic_observe_ms=round(phase_timings.get("semantic_observe_ms", 0.0), 2),
            visual_observe_ms=round(phase_timings.get("visual_observe_ms", 0.0), 2),
            candidate_build_ms=round(phase_timings.get("candidate_build_ms", 0.0), 2),
            provider_decision_ms=provider_decision_ms,
        ),
        "visual_observe_scope": "parse_only",
    }


def write_event(log_path: Path | None, event: dict[str, Any]) -> None:
    line = json.dumps(event, sort_keys=True)
    print(line)
    if log_path:
        with log_path.open("a", encoding="utf-8") as stream:
            stream.write(line + "\n")


async def find_window(driver: Driver, pid: int, title: str) -> dict[str, Any]:
    for _ in range(40):
        windows = (await driver.call("list_windows", {"pid": pid})).get("windows", [])
        matches = [
            window
            for window in windows
            if window.get("title") == title and window.get("is_on_screen") is not False
        ]
        if matches:
            return matches[0]
        await asyncio.sleep(0.25)
    raise RuntimeError(f"window {title!r} of pid {pid} did not appear")


async def observe(
    driver: Driver, task: NativeTask, pid: int, window_id: int, timeout_ms: int | None = None
) -> NativeObservation:
    arguments: dict[str, Any] = {
        "pid": pid,
        "window_id": window_id,
        "include_accessibility_tree": True,
        "include_screenshot": True,
        **task.scope.window_state_arguments(),
    }
    if timeout_ms is not None:
        arguments["timeout_ms"] = timeout_ms
    payload = await driver.call("get_window_state", arguments)
    return NativeObservation.from_window_state(
        payload, expected_pid=pid, expected_window_id=window_id
    )


async def observe_step(
    driver: Driver,
    task: NativeTask,
    pid: int,
    window_id: int,
    platform: Platform,
    foreground_ids: frozenset[str],
    phase_timings: dict[str, float] | None = None,
) -> tuple[TaskSources, dict[str, Any]]:
    """Take one observation, reobserving once with a larger walk budget if needed.

    A truncated tree, or a partial tree with no native candidate, is observed
    once more with ``REOBSERVE_TIMEOUT_MS``. The newer observation replaces the
    older one entirely, so every token in the step comes from one snapshot.

    ``observe_ms`` (kept for compatibility) spans this whole function. When
    ``phase_timings`` is given, the ``get_window_state`` calls (plus the
    ``NativeObservation`` validation) are added to ``semantic_observe_ms`` and the
    source construction and preliminary plan to ``candidate_build_ms``.
    """
    started = time.perf_counter()
    record: dict[str, Any] = {"reobserved": False}

    def build(observation: NativeObservation) -> TaskSources:
        ax = NativeAccessibilitySource.from_observation(
            observation, platform, redact=task.redact_text, text_method=task.text_method
        )
        return TaskSources(ax=ax, foreground_ids=foreground_ids)

    observe_started = time.perf_counter()
    first = await observe(driver, task, pid, window_id)
    add_phase(phase_timings, "semantic_observe_ms", observe_started)
    build_started = time.perf_counter()
    sources = build(first)
    observation = sources.ax.observation  # type: ignore[union-attr]
    native_count = sum(
        1 for candidate in task.plan(sources).candidates if candidate.source == "ax"
    )
    add_phase(phase_timings, "candidate_build_ms", build_started)
    if observation.truncated or (observation.partial and native_count == 0):
        record = {"reobserved": True, "reason": "truncated" if observation.truncated else "partial_empty"}
        observe_started = time.perf_counter()
        second = await observe(driver, task, pid, window_id, timeout_ms=REOBSERVE_TIMEOUT_MS)
        add_phase(phase_timings, "semantic_observe_ms", observe_started)
        build_started = time.perf_counter()
        sources = build(second)
        add_phase(phase_timings, "candidate_build_ms", build_started)
    observation = sources.ax.observation  # type: ignore[union-attr]
    record.update(
        {
            "snapshot_id": observation.snapshot_id,
            "capture_id": observation.capture_id,
            "elements_complete": observation.complete,
            "truncated": observation.truncated,
            "controls": len(sources.ax.controls),  # type: ignore[union-attr]
            "excluded": dict(sources.ax.native.excluded),  # type: ignore[union-attr]
            "observe_ms": round((time.perf_counter() - started) * 1000, 2),
        }
    )
    return sources, record


async def maybe_visual(
    driver: Driver,
    task: NativeTask,
    sources: TaskSources,
    available_tools: set[str],
    capture_bound_click: bool,
    phase_timings: dict[str, float] | None = None,
) -> tuple[TaskSources, dict[str, Any]]:
    """Parse visual regions from the same capture, only under the fallback rule."""
    candidate_started = time.perf_counter()
    native_count = sum(1 for c in task.plan(sources).candidates if c.source == "ax")
    reason = visual_fallback_reason(sources, task, native_count)
    add_phase(phase_timings, "candidate_build_ms", candidate_started)
    if reason is None:
        return sources, {"status": "skipped"}
    if not capture_bound_click or "parse_visual_regions" not in available_tools:
        return sources, {"status": "unavailable", "reason": reason}
    observation = sources.ax.observation  # type: ignore[union-attr]
    started = time.perf_counter()
    try:
        result = await driver.call(
            "parse_visual_regions",
            {
                "capture_id": observation.capture_id,
                "options": {
                    "kinds": ["text", "icon"],
                    "min_confidence": task.visual_min_confidence,
                    "max_regions": 100,
                },
            },
        )
        visual = parse_visual_regions(
            result,
            expected_capture_id=observation.capture_id or "",
            expected_pid=observation.pid,
            expected_window_id=observation.window_id,
        )
    except DriverToolError as error:
        # A failed parse still cost time; ``parse_ms`` is reported only on success.
        add_phase(phase_timings, "visual_observe_ms", started)
        return sources, {"status": "error", "reason": reason, "error_code": error.code or "driver_error"}
    except VisualObservationError as error:
        add_phase(phase_timings, "visual_observe_ms", started)
        return sources, {"status": "error", "reason": reason, "error_code": error.code}
    visual_sources = TaskSources(
        ax=sources.ax,
        visual=VisualRegionSource(
            visual, "background", capture_bound_click, min_confidence=task.visual_min_confidence
        ),
        visual_path=True,
        foreground_ids=sources.foreground_ids,
    )
    # One measurement feeds both fields, so visual_observe_ms == parse_ms on success.
    parse_ms = round(add_phase(phase_timings, "visual_observe_ms", started), 2)
    return (
        visual_sources,
        {"status": "ok", "reason": reason, "region_count": len(visual.regions), "parse_ms": parse_ms},
    )


def choose_live(request: dict[str, Any]) -> tuple[str | None, float, dict[str, float]]:
    from typesafe_sdk import TypeSafeClient

    with TypeSafeClient() as client:
        result = choose(
            TypeSafeDecisionModel(client), DecisionRequest.from_validated(validate_request(request))
        )
    if result.kind == "error":
        raise RuntimeError(f"provider decision failed: {result.reason}")
    return result.selected_id, result.confidence or 0.0, dict(result.probabilities)


def assert_in_scope(candidate: Candidate, pid: int, window_id: int) -> None:
    """Refuse any action addressed outside the task's one window."""
    if candidate.tool is None:
        return
    if candidate.arguments.get("pid") != pid or candidate.arguments.get("window_id") != window_id:
        raise RuntimeError("candidate addresses a window outside the task scope")


async def poll_oracle(task: NativeTask, steps: int) -> str:
    outcome = "unknown"
    for _ in range(20):
        outcome = task.classify(task.read_oracle(), steps=steps)
        if outcome in {"verified", "refuted"}:
            return outcome
        await asyncio.sleep(0.1)
    return outcome


async def run_task(args: argparse.Namespace, task: NativeTask) -> str:
    platform: Platform = args.platform or host_platform()
    log_path = Path(args.log) if args.log else None
    if log_path:
        log_path.write_text("", encoding="utf-8")
    history: list[dict[str, Any]] = []
    foreground_ids: set[str] = set()
    label = f"jev-native-python-{uuid.uuid4().hex[:8]}"
    params = StdioServerParameters(
        command=os.getenv("CUA_DRIVER_BIN", "cua-driver"), args=["mcp"], env=driver_environment()
    )
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            advertised = (await session.list_tools()).tools
            available_tools = {tool.name for tool in advertised}
            capture_bound_click = supports_capture_bound_click(advertised)
            driver = Driver(session, label)
            window = await find_window(driver, args.pid, task.scope.window_title)
            window_id = int(window["window_id"])
            write_event(
                log_path,
                {"event": "start", "task": task.id, "language": "python", "provider": args.provider,
                 "platform": platform, "pid": args.pid, "window_id": window_id},
            )
            for step in range(1, task.max_steps + 1):
                current = task.classify(task.read_oracle(), steps=step - 1)
                if current in {"verified", "refuted"}:
                    write_event(log_path, {"event": "outcome", "outcome": current, "step": step - 1})
                    return current

                started = time.perf_counter()
                phase_timings: dict[str, float] = {}
                sources, observed = await observe_step(
                    driver, task, args.pid, window_id, platform, frozenset(foreground_ids),
                    phase_timings,
                )
                sources, visual_record = await maybe_visual(
                    driver, task, sources, available_tools, capture_bound_click, phase_timings
                )
                plan_started = time.perf_counter()
                plan = task.plan(sources)
                add_phase(phase_timings, "candidate_build_ms", plan_started)
                request = native_choice_request(task, sources, plan, history)
                validate_request(request)

                decide_started = time.perf_counter()
                if args.provider == "mock":
                    choice, confidence, probabilities = choose_mock_for_task(
                        task, sources, plan.candidates, history
                    )
                elif args.provider == "s1":
                    try:
                        choice, confidence, probabilities = await asyncio.to_thread(
                            choose_s1_service, request
                        )
                    except S1ServiceError as error:
                        # A tied or malformed score is not an action; fail closed
                        # with a logged outcome instead of a traceback.
                        write_event(log_path, {"event": "outcome", "outcome": "unknown",
                                               "phase": "decide", "step": step,
                                               "candidate_count": len(plan.candidates),
                                               "expected_ids": task.expected_next(history),
                                               "error": "S1ServiceError", "reason": str(error)[:128]})
                        return "unknown"
                else:
                    choice, confidence, probabilities = await asyncio.to_thread(choose_live, request)
                decide_ms = round((time.perf_counter() - decide_started) * 1000, 2)
                # Measurement (#4312): the declared steps due now, and whether the
                # candidate set offered one. IDs only; no values.
                expected_ids = task.expected_next(history)
                offered = {candidate.id.removesuffix(":foreground") for candidate in plan.candidates}
                base_event = {
                    "event": "step",
                    "step": step,
                    "observation": observed,
                    "visual": visual_record,
                    "compose": plan.stats.to_log(),
                    "candidate_count": len(plan.candidates),
                    "schema": request["schema"],
                    "confidence": confidence,
                    "probabilities": probabilities,
                    "decide_ms": decide_ms,
                    "expected_ids": expected_ids,
                    "expected_offered": bool(set(expected_ids) & offered),
                }
                if choice is None:
                    base_event.update(native_timing_fields(started, phase_timings, decide_ms))
                    write_event(log_path, {**base_event, "event": "outcome", "outcome": "abstained"})
                    return "abstained"
                visual = sources.visual.observation if sources.visual is not None else None
                candidate = validate_choice(
                    choice, plan.candidates, current_capture_id=visual.capture_id if visual else None
                )
                assert_in_scope(candidate, args.pid, window_id)
                base_event.update(native_timing_fields(started, phase_timings, decide_ms))
                base_event["candidate"] = candidate.id
                base_event["source"] = candidate.source

                if candidate.id == "reobserve":
                    history.append(task.history_entry(step, candidate.id))
                    write_event(log_path, {**base_event, "tool": None, "act_ms": 0.0, "action_ms": 0.0,
                                           "total_step_ms": round((time.perf_counter() - started) * 1000, 2)})
                    continue
                if candidate.id == "abstain":
                    write_event(log_path, {**base_event, "event": "outcome", "outcome": "abstained"})
                    return "abstained"

                act_started = time.perf_counter()
                try:
                    assert candidate.tool is not None
                    await driver.call(candidate.tool, dict(candidate.arguments))
                except Exception as error:
                    act_ms = round((time.perf_counter() - act_started) * 1000, 2)
                    if is_stale_token_error(error):
                        # Nothing else is dispatched; the next step observes again.
                        history.append(task.history_entry(step, candidate.id, stale=True))
                        write_event(log_path, {**base_event, "tool": candidate.tool,
                                               "act_ms": act_ms, "action_ms": act_ms,
                                               "total_step_ms": round((time.perf_counter() - started) * 1000, 2),
                                               "action_error": "stale_element_token"})
                        continue
                    refusal = background_refusal_code(candidate, error)
                    if refusal is not None:
                        foreground_ids.add(candidate.id)
                        history.append(task.history_entry(step, candidate.id, refusal=refusal))
                        write_event(log_path, {**base_event, "tool": candidate.tool, "act_ms": act_ms,
                                               "action_ms": act_ms,
                                               "total_step_ms": round((time.perf_counter() - started) * 1000, 2),
                                               "action_error": refusal,
                                               "escalation": {"from": "background", "to": "foreground",
                                                              "allowed": task.allow_foreground}})
                        continue
                    write_event(log_path, {**base_event, "event": "outcome", "outcome": "unknown",
                                           "phase": "action", "error": type(error).__name__,
                                           "tool": candidate.tool})
                    return "unknown"
                act_ms = round((time.perf_counter() - act_started) * 1000, 2)
                history.append(
                    task.history_entry(step, candidate.id, outcome=plan.outcomes.get(candidate.id))
                )
                write_event(log_path, {**base_event, "tool": candidate.tool, "act_ms": act_ms,
                                       "action_ms": act_ms,
                                       "total_step_ms": round((time.perf_counter() - started) * 1000, 2),
                                       "delivery_mode": candidate.arguments.get("delivery_mode")})
                outcome = await poll_oracle(task, step)
                if outcome in {"verified", "refuted"}:
                    write_event(log_path, {"event": "outcome", "outcome": outcome, "step": step})
                    return outcome

            outcome = task.classify(task.read_oracle(), steps=task.max_steps)
            write_event(log_path, {"event": "outcome", "outcome": outcome, "step": task.max_steps})
            return outcome


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--task", choices=NATIVE_TASK_IDS, required=True)
    parser.add_argument(
        "--provider", choices=("mock", "live", "s1"), default="mock",
        help="s1 reads the loopback decide URL from CUA_S1_DECISION_URL",
    )
    parser.add_argument("--pid", type=int, required=True, help="the running harness process")
    parser.add_argument("--state-file", required=True, help="the harness task-state file path")
    parser.add_argument("--note-text", help="note text for the save-note tasks")
    parser.add_argument("--allow-foreground", action="store_true")
    parser.add_argument("--platform", choices=("macos", "windows", "linux"))
    parser.add_argument("--log", help="optional JSONL output path")
    return parser.parse_args(argv)


def task_from_args(args: argparse.Namespace) -> NativeTask:
    options: dict[str, Any] = {"pid": args.pid, "allow_foreground": args.allow_foreground}
    if args.note_text:
        options["note_text"] = args.note_text
    return native_task(args.task, Path(args.state_file), **options)


def main() -> None:
    args = parse_args()
    if args.provider == "s1":
        s1_service_url()  # fail before touching the app when the service is not configured
    try:
        outcome = asyncio.run(run_task(args, task_from_args(args)))
    except (OracleError, NativeObservationError) as error:
        write_event(Path(args.log) if args.log else None,
                    {"event": "outcome", "outcome": "unknown", "error": type(error).__name__})
        raise SystemExit(1) from None
    raise SystemExit(0 if outcome == "verified" else 1)


if __name__ == "__main__":
    main()
