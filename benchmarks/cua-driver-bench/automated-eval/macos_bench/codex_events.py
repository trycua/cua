"""Metrics from a timestamped Codex `exec --json` event log.

The runner stores each stdout line as ``<epoch_ms>\\t<json>`` so tool latency can be
measured identically for both arms from the same clock. Prompts, tool results and
screen content are never copied into the summary; only counts, names and timings.
"""

from __future__ import annotations

import json
import re
import statistics
from pathlib import Path
from typing import Any

# Cua Driver MCP tool names -> action class.
CUA_CLASSES = {
    "click": "click",
    "double_click": "click",
    "right_click": "click",
    "type_text": "type",
    "browser_type": "type",
    "set_value": "set_value",
    "press_key": "key",
    "hotkey": "key",
    "scroll": "scroll",
    "drag": "drag",
    "browser_click": "click",
    "browser_pointer": "click",
    "get_window_state": "observe",
    "get_desktop_state": "observe",
    "get_accessibility_tree": "observe",
    "get_browser_state": "observe",
    "list_windows": "observe",
    "list_apps": "observe",
    "zoom": "observe",
    "verify_state": "observe",
    "screenshot": "observe",
}
# Codex computer-use (sky) API names -> action class.
SKY_CLASSES = {
    "click": "click",
    "type_text": "type",
    "paste": "type",
    "set_value": "set_value",
    "press_key": "key",
    "scroll": "scroll",
    "drag": "drag",
    "get_app_state": "observe",
    "list_apps": "observe",
    "select_text": "other",
    "perform_secondary_action": "click",
}
SKY_CALL = re.compile(r"\bsky\.(%s)\s*\(" % "|".join(SKY_CLASSES))
CUA_CLI = re.compile(r"\bcua-driver\s+([a-z_]+)")
EVALUATOR_PATTERNS = re.compile(
    r"evaluator|oracle|hidden_tests|task\.cuabench|/tasks/(shared|macos)/|reset/(setup|verify)",
    re.IGNORECASE,
)
CONFIRMATION = re.compile(
    r"(please confirm|do you want me to|would you like me to|need your (approval|confirmation)"
    r"|waiting for (your )?(approval|confirmation)|before i proceed|can you confirm)",
    re.IGNORECASE,
)
INFRA_ERRORS = re.compile(
    r"(429|rate.?limit|usage limit|quota|unauthor|401|403|overloaded|service unavailable|"
    r"502|503|504|stream disconnected|connection (reset|refused)|timed out waiting)",
    re.IGNORECASE,
)
CODEX_BUILTIN_MCP_TOOLS = {"list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource"}
CLASSES = ("observe", "click", "type", "key", "scroll", "drag", "set_value", "other")


def _empty_by_class() -> dict[str, int]:
    return {name: 0 for name in CLASSES}


def read_events(path: Path) -> list[tuple[float, dict[str, Any]]]:
    events: list[tuple[float, dict[str, Any]]] = []
    for line in path.read_text("utf-8", errors="replace").splitlines():
        stamp, _, raw = line.partition("\t")
        try:
            events.append((float(stamp), json.loads(raw)))
        except (ValueError, json.JSONDecodeError):
            continue
    return events


def _percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round(fraction * (len(ordered) - 1))))
    return ordered[index]


def summarize(events: list[tuple[float, dict[str, Any]]]) -> dict[str, Any]:
    by_class = _empty_by_class()
    latencies: dict[str, list[float]] = {name: [] for name in CLASSES}
    started: dict[str, float] = {}
    total = failed = shell = cua_cli = 0
    tokens = {"input": 0, "cached_input": 0, "output": 0, "reasoning": 0}
    last_message = ""
    commands: list[str] = []
    error_text: list[str] = []
    turn_completed = False
    first_tool_ms: float | None = None
    first_ms = events[0][0] if events else None
    last_ms = events[-1][0] if events else None
    tool_names: dict[str, int] = {}

    for stamp, event in events:
        kind = event.get("type")
        if kind == "turn.completed":
            turn_completed = True
            usage = event.get("usage") or {}
            tokens["input"] += int(usage.get("input_tokens") or 0)
            tokens["cached_input"] += int(usage.get("cached_input_tokens") or 0)
            tokens["output"] += int(usage.get("output_tokens") or 0)
            tokens["reasoning"] += int(usage.get("reasoning_output_tokens") or 0)
        elif kind in ("error", "turn.failed"):
            error_text.append(json.dumps(event)[:300])
        elif kind == "item.started":
            item = event.get("item") or {}
            if item.get("type") in ("mcp_tool_call", "command_execution"):
                started[str(item.get("id"))] = stamp
        elif kind == "item.completed":
            item = event.get("item") or {}
            itype = item.get("type")
            if itype == "agent_message":
                last_message = str(item.get("text") or "")
            elif itype == "mcp_tool_call" and str(item.get("tool")) in CODEX_BUILTIN_MCP_TOOLS:
                continue
            elif itype == "mcp_tool_call":
                total += 1
                if first_tool_ms is None:
                    first_tool_ms = stamp
                if item.get("status") != "completed":
                    failed += 1
                name = str(item.get("tool"))
                tool_names[name] = tool_names.get(name, 0) + 1
                latency = stamp - started.get(str(item.get("id")), stamp)
                server = str(item.get("server"))
                arguments = item.get("arguments") or {}
                if server == "node_repl" and name == "js":
                    code = str(arguments.get("code") or "")
                    hits = [SKY_CLASSES[m] for m in SKY_CALL.findall(code)]
                    if not hits:
                        hits = ["other"]
                    for cls in hits:
                        by_class[cls] += 1
                        latencies[cls].append(latency / len(hits))
                else:
                    cls = CUA_CLASSES.get(name, "other")
                    by_class[cls] += 1
                    latencies[cls].append(latency)
            elif itype == "command_execution":
                command = str(item.get("command") or "")
                commands.append(command)
                match = CUA_CLI.search(command)
                if match and match.group(1) in CUA_CLASSES:
                    cua_cli += 1
                    cls = CUA_CLASSES[match.group(1)]
                    by_class[cls] += 1
                    latencies[cls].append(stamp - started.get(str(item.get("id")), stamp))
                else:
                    shell += 1

    latency_summary = {
        cls: {
            "n": len(values),
            "median": round(statistics.median(values), 1),
            "p90": round(_percentile(values, 0.9), 1),
        }
        for cls, values in latencies.items()
        if values
    }
    error_blob = " ".join(error_text)
    return {
        "turn_completed": turn_completed,
        "tool_calls": {
            "total": total + cua_cli,
            "mcp": total,
            "cua_cli_via_shell": cua_cli,
            "by_class": by_class,
            "failed": failed,
            "by_tool": tool_names,
        },
        "shell_commands": shell,
        "steps": total + cua_cli + shell,
        "action_latency_ms": latency_summary,
        "tokens": tokens,
        "last_message": last_message,
        "confirmation_requested": bool(CONFIRMATION.search(last_message)),
        "evaluator_read_suspected": any(EVALUATOR_PATTERNS.search(c) for c in commands),
        "error_events": error_text[:5],
        "infra_error_suspected": bool(error_text) and bool(INFRA_ERRORS.search(error_blob)),
        "first_tool_after_ms": (
            round(first_tool_ms - first_ms, 1)
            if first_tool_ms is not None and first_ms is not None
            else None
        ),
        "log_span_ms": round(last_ms - first_ms, 1) if first_ms is not None else None,
    }


def rollout_usage(codex_home: Path) -> dict[str, int] | None:
    """Cumulative token usage from the session rollout (works for timed-out runs too)."""
    files = sorted(
        (codex_home / "sessions").rglob("rollout-*.jsonl"), key=lambda f: f.stat().st_mtime
    )
    last = None
    for path in files:
        for line in path.read_text("utf-8", errors="replace").splitlines():
            if '"token_count"' not in line:
                continue
            try:
                payload = json.loads(line).get("payload", {})
            except json.JSONDecodeError:
                continue
            info = payload.get("info") if payload.get("type") == "token_count" else None
            if info and info.get("total_token_usage"):
                last = info["total_token_usage"]
    if not last:
        return None
    return {
        "input": int(last.get("input_tokens") or 0),
        "cached_input": int(last.get("cached_input_tokens") or 0),
        "output": int(last.get("output_tokens") or 0),
        "reasoning": int(last.get("reasoning_output_tokens") or 0),
    }
