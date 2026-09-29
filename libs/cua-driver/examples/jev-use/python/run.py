from __future__ import annotations

import argparse
import asyncio
import json
import os
import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from driver_env import driver_environment
from core import (
    Candidate,
    VisualDelivery,
    VisualObservation,
    VisualObservationError,
    has_executable_candidate,
    parse_visual_regions,
    validate_choice,
)
from jev_adapter import choose_live_for_task, choose_mock_for_task
from tasks import (
    FixtureFormTask,
    Task,
    TaskSources,
    fixture_sources,
    fixture_state,
    reset_fixture,
)

__all__ = ["fixture_state", "reset_fixture"]


def validate_fixture_url(value: str) -> str:
    parsed = urlsplit(value)
    if (
        parsed.scheme != "http"
        or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}
        or parsed.username
        or parsed.password
        or parsed.path not in {"", "/"}
        or parsed.query
        or parsed.fragment
    ):
        raise argparse.ArgumentTypeError(
            "fixture URL must be an HTTP loopback origin such as http://127.0.0.1:8765/"
        )
    return value.rstrip("/") + "/"


def select_tab_id(tabs: list[dict[str, Any]]) -> str:
    if not tabs:
        raise RuntimeError("isolated browser has no tabs")
    selected = next((tab for tab in tabs if tab.get("active")), tabs[0])
    return str(selected["tab_id"])


class DriverToolError(RuntimeError):
    """A Driver tool returned an error result, optionally with a stable error code."""

    def __init__(
        self,
        message: str,
        code: str | None = None,
        recommended_delivery: str | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.recommended_delivery = recommended_delivery


class Driver:
    def __init__(self, session: ClientSession, label: str) -> None:
        self.session = session
        self.label = label

    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        result = await self.session.call_tool(name, {**arguments, "session": self.label})
        if result.isError:
            structured = getattr(result, "structuredContent", None)
            structured = structured if isinstance(structured, dict) else {}
            code = structured.get("code")
            refusal = structured.get("refusal")
            if not code and isinstance(refusal, dict):
                code = refusal.get("code")
            escalation = structured.get("escalation")
            recommended = escalation.get("recommended") if isinstance(escalation, dict) else None
            raise DriverToolError(
                f"{name} failed: {getattr(result, 'content', None)}",
                code if isinstance(code, str) and code else None,
                recommended if isinstance(recommended, str) and recommended else None,
            )
        data = result.structuredContent
        if not isinstance(data, dict):
            raise RuntimeError(f"{name} returned no structured result")
        if data.get("status") == "refused" or data.get("refusal"):
            refusal = data.get("refusal")
            code = refusal.get("code") if isinstance(refusal, dict) else None
            # DriverToolError is a RuntimeError, so existing handlers still match.
            raise DriverToolError(
                f"{name} refused: {data.get('refusal', data)}",
                code if isinstance(code, str) and code else None,
            )
        return data


def supports_capture_bound_click(tools: list[Any]) -> bool:
    for tool in tools:
        if getattr(tool, "name", None) != "click":
            continue
        schema = getattr(tool, "inputSchema", None)
        if not isinstance(schema, dict):
            schema = getattr(tool, "input_schema", None)
        properties = schema.get("properties") if isinstance(schema, dict) else None
        return isinstance(properties, dict) and "capture_id" in properties
    return False


def background_refusal_code(candidate: Candidate, error: BaseException) -> str | None:
    """Return Driver's code when it refused a background visual click.

    Only a structured refusal counts: a ``background_*`` error code or an explicit
    ``escalation.recommended == "foreground"``. Anything else stays a failure.
    """
    if candidate.tool != "click" or candidate.arguments.get("delivery_mode") != "background":
        return None
    if not isinstance(error, DriverToolError):
        return None
    if error.code and error.code.startswith("background_"):
        return error.code
    if error.recommended_delivery == "foreground":
        return error.code or "foreground_recommended"
    return None


def visual_status(
    status: str,
    *,
    error_code: str | None = None,
    visual: VisualObservation | None = None,
    reason: str | None = None,
) -> dict[str, Any]:
    """Build the redacted per-step visual record written to the JSONL log.

    It never contains screenshots, screenshot references, region text, or secrets.
    """
    record: dict[str, Any] = {"status": status}
    if reason:
        record["reason"] = reason
    if error_code:
        record["error_code"] = error_code
    if visual is not None:
        record["capture_id"] = visual.capture_id
        record["region_count"] = len(visual.regions)
    return record


async def observe_visual(
    driver: Driver,
    pid: int,
    window_id: int,
    available_tools: set[str],
    capture_bound_click: bool,
) -> tuple[VisualObservation | None, dict[str, Any]]:
    """Take an optional visual observation and report what happened.

    Failures never stop the run: the caller continues on the page-structure path,
    but the returned status makes the fallback observable.
    """
    if not capture_bound_click:
        return None, visual_status("unavailable", error_code="capture_bound_click_unsupported")
    if not {"get_window_state", "parse_visual_regions"}.issubset(available_tools):
        return None, visual_status("unavailable", error_code="tool_not_advertised")
    try:
        capture = await driver.call(
            "get_window_state",
            {
                "pid": pid,
                "window_id": window_id,
                "include_accessibility_tree": False,
            },
        )
        capture_id = capture.get("capture_id")
        if not isinstance(capture_id, str):
            return None, visual_status("error", error_code="capture_missing")
        result = await driver.call(
            "parse_visual_regions",
            {
                "capture_id": capture_id,
                "options": {"kinds": ["text", "icon"], "min_confidence": 0.8, "max_regions": 100},
            },
        )
        visual = parse_visual_regions(
            result,
            expected_capture_id=capture_id,
            expected_pid=pid,
            expected_window_id=window_id,
        )
    except DriverToolError as error:
        if error.code == "not_installed":
            return None, visual_status("not_installed", error_code="not_installed")
        return None, visual_status("error", error_code=error.code or "driver_error")
    except VisualObservationError as error:
        return None, visual_status("error", error_code=error.code)
    except (RuntimeError, KeyError, TypeError):
        return None, visual_status("error", error_code="driver_error")
    return visual, visual_status("ok", visual=visual)


async def optional_visual_observation(
    driver: Driver,
    pid: int,
    window_id: int,
    available_tools: set[str],
    capture_bound_click: bool,
) -> VisualObservation | None:
    visual, _ = await observe_visual(
        driver, pid, window_id, available_tools, capture_bound_click
    )
    return visual


async def task_candidates_for_step(
    driver: Driver,
    task: Task,
    snapshot: dict[str, Any],
    pid: int,
    window_id: int,
    available_tools: set[str],
    capture_bound_click: bool,
    *,
    visual_mode: str = "auto",
    visual_delivery: VisualDelivery = "background",
    phase_timings: dict[str, float] | None = None,
) -> tuple[list[Candidate], TaskSources, dict[str, Any]]:
    """Build one step's sources and candidates, parsing visual regions only when useful.

    In ``auto`` mode the capture and parse run only when the page structure
    offers no executable candidate, because only then can a visual region add
    one. ``always`` restores the per-step parse; ``off`` never parses.
    """
    # Whether a control missing from the page structure can still be found
    # through a capture-bound visual region; reported in the task state.
    visual_path = capture_bound_click and visual_mode != "off"
    candidate_build_started = time.perf_counter()
    sources = fixture_sources(
        snapshot,
        None,
        capture_bound_click=capture_bound_click,
        visual_delivery=visual_delivery,
        visual_path=visual_path,
    )
    candidates = task.candidates(sources)
    candidate_build_ms = (time.perf_counter() - candidate_build_started) * 1000
    visual_observe_ms = 0.0

    def record_phase_timings() -> None:
        if phase_timings is not None:
            phase_timings.update(
                {
                    "visual_observe_ms": round(visual_observe_ms, 2),
                    "candidate_build_ms": round(candidate_build_ms, 2),
                }
            )

    if visual_mode == "off":
        record_phase_timings()
        return candidates, sources, visual_status("skipped", reason="disabled")
    if visual_mode == "auto" and has_executable_candidate(candidates):
        record_phase_timings()
        return candidates, sources, visual_status("skipped", reason="page_structure_candidate")

    visual_started = time.perf_counter()
    visual, record = await observe_visual(
        driver, pid, window_id, available_tools, capture_bound_click
    )
    visual_observe_ms = (time.perf_counter() - visual_started) * 1000
    if visual is not None:
        candidate_build_started = time.perf_counter()
        sources = fixture_sources(
            snapshot,
            visual,
            capture_bound_click=capture_bound_click,
            visual_delivery=visual_delivery,
            visual_path=visual_path,
        )
        candidates = task.candidates(sources)
        candidate_build_ms += (time.perf_counter() - candidate_build_started) * 1000
    record_phase_timings()
    return candidates, sources, record


async def candidates_for_step(
    driver: Driver,
    snapshot: dict[str, Any],
    token: str,
    pid: int,
    window_id: int,
    available_tools: set[str],
    capture_bound_click: bool,
    *,
    visual_mode: str = "auto",
    visual_delivery: VisualDelivery = "background",
) -> tuple[list[Candidate], VisualObservation | None, dict[str, Any]]:
    """Build one step's fixture-task candidates; see ``task_candidates_for_step``."""
    candidates, sources, record = await task_candidates_for_step(
        driver,
        FixtureFormTask(token),
        snapshot,
        pid,
        window_id,
        available_tools,
        capture_bound_click,
        visual_mode=visual_mode,
        visual_delivery=visual_delivery,
    )
    visual = sources.visual.observation if sources.visual is not None else None
    return candidates, visual, record


async def wait_for_window(driver: Driver, pid: int) -> dict[str, Any]:
    for _ in range(40):
        windows = (await driver.call("list_windows", {"pid": pid})).get("windows", [])
        visible = [window for window in windows if window.get("is_on_screen")]
        if visible:
            return max(
                visible,
                key=lambda window: window["bounds"]["width"] * window["bounds"]["height"],
            )
        await asyncio.sleep(0.25)
    raise RuntimeError("isolated browser window did not become ready")


def write_event(log_path: Path | None, event: dict[str, Any]) -> None:
    line = json.dumps(event, sort_keys=True)
    print(line)
    if log_path:
        with log_path.open("a", encoding="utf-8") as stream:
            stream.write(line + "\n")


def decision_timing_fields(
    *,
    decision_ms: float,
    semantic_observe_ms: float,
    visual_observe_ms: float,
    candidate_build_ms: float,
    provider_decision_ms: float,
) -> dict[str, float]:
    """Stable phase fields for one observe → decide boundary."""
    return {
        "decision_ms": decision_ms,
        "semantic_observe_ms": semantic_observe_ms,
        "visual_observe_ms": visual_observe_ms,
        "candidate_build_ms": candidate_build_ms,
        "provider_decision_ms": provider_decision_ms,
    }


async def run(args: argparse.Namespace) -> str:
    token = args.token or f"jev-{uuid.uuid4().hex[:10]}"
    task: Task = FixtureFormTask(token, args.fixture_url, args.max_steps)
    label = f"jev-python-{uuid.uuid4().hex[:8]}"
    # Compact what-happened record for the decision model. The full telemetry
    # events (timings, probabilities) go only to the JSONL log.
    history: list[dict[str, Any]] = []
    visual_delivery: VisualDelivery = "background"
    log_path = Path(args.log) if args.log else None
    if log_path:
        log_path.write_text("", encoding="utf-8")
    task.reset()

    params = StdioServerParameters(
        command=os.getenv("CUA_DRIVER_BIN", "cua-driver"), args=["mcp"], env=driver_environment()
    )
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            advertised_tools = (await session.list_tools()).tools
            available_tools = {tool.name for tool in advertised_tools}
            capture_bound_click = supports_capture_bound_click(advertised_tools)
            driver = Driver(session, label)
            prepared = await driver.call(
                "browser_prepare",
                {"allow_launch": True, "profile": {"mode": "isolated_new"}},
            )
            pid = int(prepared["prepared_pid"])
            window = await wait_for_window(driver, pid)
            bound = await driver.call(
                "get_browser_state", {"pid": pid, "window_id": window["window_id"]}
            )
            target_id = bound["target_id"]
            tab_id = select_tab_id(bound["tabs"])
            await driver.call(
                "browser_navigate",
                {"target_id": target_id, "tab_id": tab_id, "url": args.fixture_url},
            )

            for step in range(1, task.max_steps + 1):
                current = task.classify(task.read_oracle(), steps=step - 1)
                if current in {"verified", "refuted"}:
                    write_event(log_path, {"event": "outcome", "outcome": current, "token": token})
                    return current

                started = time.perf_counter()
                phase_started = time.perf_counter()
                snapshot = await driver.call(
                    "get_browser_state",
                    {
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "snapshot_format": "semantic_v2",
                    },
                )
                semantic_observe_ms = round((time.perf_counter() - phase_started) * 1000, 2)
                candidate_phase: dict[str, float] = {}
                candidates, sources, visual_record = await task_candidates_for_step(
                    driver,
                    task,
                    snapshot,
                    pid,
                    int(window["window_id"]),
                    available_tools,
                    capture_bound_click,
                    visual_mode=args.visual_observation,
                    visual_delivery=visual_delivery,
                    phase_timings=candidate_phase,
                )
                if not candidates:
                    write_event(
                        log_path,
                        {"event": "outcome", "outcome": "abstained", "step": step, "visual": visual_record},
                    )
                    return "abstained"

                visual = sources.visual.observation if sources.visual is not None else None
                provider_started = time.perf_counter()
                if args.provider == "mock":
                    choice, confidence, probabilities = choose_mock_for_task(
                        task, sources, candidates, history
                    )
                else:
                    choice, confidence, probabilities = await asyncio.to_thread(
                        choose_live_for_task, task, sources, candidates, history
                    )
                provider_decision_ms = round(
                    (time.perf_counter() - provider_started) * 1000, 2
                )
                if choice is None:
                    return "abstained"
                candidate = validate_choice(
                    choice,
                    candidates,
                    current_capture_id=visual.capture_id if visual else None,
                )
                decision_ms = round((time.perf_counter() - started) * 1000, 2)
                timing = decision_timing_fields(
                    decision_ms=decision_ms,
                    semantic_observe_ms=semantic_observe_ms,
                    visual_observe_ms=candidate_phase.get("visual_observe_ms", 0.0),
                    candidate_build_ms=candidate_phase.get("candidate_build_ms", 0.0),
                    provider_decision_ms=provider_decision_ms,
                )

                if candidate.id == "reobserve":
                    event = {
                        "event": "step",
                        "step": step,
                        "candidate": candidate.id,
                        "confidence": confidence,
                        "probabilities": probabilities,
                        **timing,
                        "action_ms": 0.0,
                        "total_step_ms": round((time.perf_counter() - started) * 1000, 2),
                        "dry_run": args.dry_run,
                        "tool": None,
                        "visual": visual_record,
                    }
                    history.append(task.history_entry(step, candidate.id))
                    write_event(log_path, event)
                    continue

                if candidate.id == "abstain":
                    write_event(
                        log_path,
                        {
                            "event": "outcome",
                            "outcome": "abstained",
                            "step": step,
                            "confidence": confidence,
                            "probabilities": probabilities,
                            "visual": visual_record,
                        },
                    )
                    return "abstained"

                if not args.dry_run:
                    action_started = time.perf_counter()
                    try:
                        assert candidate.tool is not None
                        await driver.call(candidate.tool, candidate.arguments)
                    except Exception as error:
                        refusal = background_refusal_code(candidate, error)
                        if refusal is not None:
                            # Do not retry background. The next step takes a fresh
                            # capture and offers a distinct foreground candidate.
                            visual_delivery = "foreground"
                            event = {
                                "event": "step",
                                "step": step,
                                "candidate": candidate.id,
                                "confidence": confidence,
                                "probabilities": probabilities,
                                **timing,
                                "action_ms": round((time.perf_counter() - action_started) * 1000, 2),
                                "total_step_ms": round((time.perf_counter() - started) * 1000, 2),
                                "dry_run": args.dry_run,
                                "tool": candidate.tool,
                                "delivery_mode": "background",
                                "action_error": refusal,
                                "escalation": {
                                    "from": "background",
                                    "to": "foreground",
                                    "reason": refusal,
                                },
                                "visual": visual_record,
                            }
                            history.append(task.history_entry(step, candidate.id, refusal=refusal))
                            write_event(log_path, event)
                            continue
                        write_event(
                            log_path,
                            {
                                "event": "outcome",
                                "outcome": "unknown",
                                "step": step,
                                "phase": "action",
                                "error": type(error).__name__,
                                "tool": candidate.tool,
                                "visual": visual_record,
                            },
                        )
                        return "unknown"
                    action_ms = round((time.perf_counter() - action_started) * 1000, 2)
                else:
                    action_ms = 0.0
                event = {
                    "event": "step",
                    "step": step,
                    "candidate": candidate.id,
                    "confidence": confidence,
                    "probabilities": probabilities,
                    **timing,
                    "action_ms": action_ms,
                    "total_step_ms": round((time.perf_counter() - started) * 1000, 2),
                    "dry_run": args.dry_run,
                    "tool": candidate.tool,
                    "delivery_mode": candidate.arguments.get("delivery_mode"),
                    "visual": visual_record,
                }
                history.append(task.history_entry(step, candidate.id))
                write_event(log_path, event)
                if args.dry_run:
                    return "unknown"
                if candidate.id in task.completion_candidate_ids:
                    for _ in range(20):
                        outcome = task.classify(task.read_oracle(), steps=step)
                        if outcome in {"verified", "refuted"}:
                            write_event(
                                log_path, {"event": "outcome", "outcome": outcome, "token": token}
                            )
                            return outcome
                        await asyncio.sleep(0.1)

            outcome = task.classify(task.read_oracle(), steps=task.max_steps)
            write_event(log_path, {"event": "outcome", "outcome": outcome, "token": token})
            return outcome


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", choices=("mock", "live"), default="mock")
    parser.add_argument(
        "--fixture-url", type=validate_fixture_url, default="http://127.0.0.1:8765/"
    )
    parser.add_argument("--token")
    parser.add_argument("--max-steps", type=int, default=4)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--log", help="optional JSONL output path")
    parser.add_argument(
        "--visual-observation",
        choices=("auto", "always", "off"),
        default="auto",
        help=(
            "auto parses visual regions only when the page structure offers no "
            "executable candidate; always parses every step; off never parses"
        ),
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    outcome = asyncio.run(run(args))
    raise SystemExit(0 if outcome == "verified" or args.dry_run else 1)


if __name__ == "__main__":
    main()
