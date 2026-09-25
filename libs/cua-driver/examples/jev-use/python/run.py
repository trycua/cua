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
from urllib.request import Request, urlopen

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from driver_env import driver_environment
from core import (
    VisualObservation,
    VisualObservationError,
    build_candidates,
    classify,
    parse_visual_regions,
    validate_choice,
)
from jev_adapter import choose_live, choose_mock_adapter


def fixture_state(fixture_url: str) -> dict[str, str | None]:
    with urlopen(f"{fixture_url.rstrip('/')}/state", timeout=2) as response:
        return json.loads(response.read())


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


def reset_fixture(fixture_url: str) -> None:
    request = Request(f"{fixture_url.rstrip('/')}/reset", method="POST", data=b"")
    with urlopen(request, timeout=2) as response:
        if response.status != 204:
            raise RuntimeError(f"fixture reset failed: HTTP {response.status}")


class DriverToolError(RuntimeError):
    """A Driver tool returned an error result, optionally with a stable error code."""

    def __init__(self, message: str, code: str | None = None) -> None:
        super().__init__(message)
        self.code = code


class Driver:
    def __init__(self, session: ClientSession, label: str) -> None:
        self.session = session
        self.label = label

    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        result = await self.session.call_tool(name, {**arguments, "session": self.label})
        if result.isError:
            structured = getattr(result, "structuredContent", None)
            code = structured.get("code") if isinstance(structured, dict) else None
            raise DriverToolError(
                f"{name} failed: {getattr(result, 'content', None)}",
                code if isinstance(code, str) and code else None,
            )
        data = result.structuredContent
        if not isinstance(data, dict):
            raise RuntimeError(f"{name} returned no structured result")
        if data.get("status") == "refused" or data.get("refusal"):
            raise RuntimeError(f"{name} refused: {data.get('refusal', data)}")
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


def visual_status(
    status: str,
    *,
    error_code: str | None = None,
    visual: VisualObservation | None = None,
) -> dict[str, Any]:
    """Build the redacted per-step visual record written to the JSONL log.

    It never contains screenshots, screenshot references, region text, or secrets.
    """
    record: dict[str, Any] = {"status": status}
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


async def run(args: argparse.Namespace) -> str:
    token = args.token or f"jev-{uuid.uuid4().hex[:10]}"
    label = f"jev-python-{uuid.uuid4().hex[:8]}"
    history: list[dict[str, Any]] = []
    log_path = Path(args.log) if args.log else None
    if log_path:
        log_path.write_text("", encoding="utf-8")
    reset_fixture(args.fixture_url)

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

            for step in range(1, args.max_steps + 1):
                oracle = fixture_state(args.fixture_url)
                current = classify(oracle.get("submitted"), token, steps=step - 1, max_steps=args.max_steps)
                if current in {"verified", "refuted"}:
                    write_event(log_path, {"event": "outcome", "outcome": current, "token": token})
                    return current

                started = time.perf_counter()
                snapshot = await driver.call(
                    "get_browser_state",
                    {
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "snapshot_format": "semantic_v2",
                    },
                )
                visual, visual_record = await observe_visual(
                    driver,
                    pid,
                    int(window["window_id"]),
                    available_tools,
                    capture_bound_click,
                )
                candidates = build_candidates(
                    snapshot,
                    token,
                    visual,
                    capture_bound_click=capture_bound_click,
                )
                if not candidates:
                    write_event(
                        log_path,
                        {"event": "outcome", "outcome": "abstained", "step": step, "visual": visual_record},
                    )
                    return "abstained"

                if args.provider == "mock":
                    choice, confidence, probabilities = choose_mock_adapter(
                        candidates, snapshot, visual, history
                    )
                else:
                    choice, confidence, probabilities = await asyncio.to_thread(
                        choose_live, candidates, snapshot, visual, history
                    )
                if choice is None:
                    return "abstained"
                candidate = validate_choice(
                    choice,
                    candidates,
                    current_capture_id=visual.capture_id if visual else None,
                )
                decision_ms = round((time.perf_counter() - started) * 1000, 2)

                if candidate.id == "reobserve":
                    event = {
                        "event": "step",
                        "step": step,
                        "candidate": candidate.id,
                        "confidence": confidence,
                        "probabilities": probabilities,
                        "decision_ms": decision_ms,
                        "action_ms": 0.0,
                        "dry_run": args.dry_run,
                        "tool": None,
                        "visual": visual_record,
                    }
                    history.append(event)
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
                    "decision_ms": decision_ms,
                    "action_ms": action_ms,
                    "dry_run": args.dry_run,
                    "tool": candidate.tool,
                    "visual": visual_record,
                }
                history.append(event)
                write_event(log_path, event)
                if args.dry_run:
                    return "unknown"
                if candidate.id == "submit-form":
                    for _ in range(20):
                        oracle = fixture_state(args.fixture_url)
                        outcome = classify(
                            oracle.get("submitted"), token, steps=step, max_steps=args.max_steps
                        )
                        if outcome in {"verified", "refuted"}:
                            write_event(
                                log_path, {"event": "outcome", "outcome": outcome, "token": token}
                            )
                            return outcome
                        await asyncio.sleep(0.1)

            oracle = fixture_state(args.fixture_url)
            outcome = classify(
                oracle.get("submitted"), token, steps=args.max_steps, max_steps=args.max_steps
            )
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
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    outcome = asyncio.run(run(args))
    raise SystemExit(0 if outcome == "verified" or args.dry_run else 1)


if __name__ == "__main__":
    main()
