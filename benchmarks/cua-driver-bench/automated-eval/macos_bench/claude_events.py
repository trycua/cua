"""Metrics from a Claude Code `--output-format stream-json --verbose` log.

The runner stores each stdout line as ``<epoch_ms>\\t<json>`` (a bare JSON line is accepted
too) so tool latency can be measured from one clock for both arms. Only counts, names,
timings, usage and error text are copied into the summary; tool inputs, tool results and
screen content are not.
"""

from __future__ import annotations

import json
import re
import statistics
from collections import Counter
from pathlib import Path
from typing import Any, Iterable

from codex_events import CUA_CLASSES

# Tool-name suffix -> action class for the Cua Driver MCP surface (shared with the Codex pilot).
# The Codex computer-use arm exposes `js` / `js_reset` only; its calls are classed by what the
# JS source calls on the `cua` object (see CUA_REPL_CALL).
CUA_REPL_CLASSES = {
    "click": "click",
    "doubleClick": "click",
    "rightClick": "click",
    "typeText": "type",
    "type": "type",
    "pressKey": "key",
    "press": "key",
    "hotkey": "key",
    "scroll": "scroll",
    "drag": "drag",
    "setValue": "set_value",
    "getApp": "observe",
    "getAppState": "observe",
    "listApps": "observe",
    "screenshot": "observe",
    "snapshot": "observe",
}
CUA_REPL_CALL = re.compile(r"\.(%s)\s*\(" % "|".join(CUA_REPL_CLASSES))

RATE_LIMIT_TEXT = re.compile(
    r"rate.?limit|usage limit|limit reached|\b429\b|too many requests|quota|"
    r"reached your .*limit|out of extra usage|credit balance|resets? (at|in|on)",
    re.IGNORECASE,
)
OVERLOAD_TEXT = re.compile(
    r"overloaded|\b(500|502|503|504|529)\b|internal server error|service unavailable|"
    r"api error|temporarily unavailable|timed? ?out|econnreset|socket hang up",
    re.IGNORECASE,
)
AUTH_TEXT = re.compile(
    r"authentication_error|invalid api key|please run /login|oauth token|"
    r"not logged in|invalid x-api-key|credentials",
    re.IGNORECASE,
)


def read_events(path: Path) -> list[dict[str, Any]]:
    """Read a stream log: ``<ms>\\t<json>`` lines or bare JSON lines. Adds ``_ms`` when stamped."""
    events: list[dict[str, Any]] = []
    if not path.exists():
        return events
    for raw in path.read_text("utf-8", "replace").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        stamp = None
        if not raw.startswith("{") and "\t" in raw:
            head, _, raw = raw.partition("\t")
            try:
                stamp = float(head)
            except ValueError:
                continue
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue  # a torn final line after a kill is not an error
        if isinstance(obj, dict):
            if stamp is not None:
                obj["_ms"] = stamp
            events.append(obj)
    return events


def strip_mcp(name: str) -> tuple[str | None, str]:
    """``mcp__server__tool`` -> (server, tool); a built-in name -> (None, name)."""
    if name.startswith("mcp__"):
        parts = name.split("__", 2)
        if len(parts) == 3:
            return parts[1], parts[2]
    return None, name


def tool_class(name: str, tool_input: dict[str, Any] | None = None) -> str:
    server, tool = strip_mcp(name)
    if server is None:
        return "builtin"
    if tool in CUA_CLASSES:
        return CUA_CLASSES[tool]
    if tool == "js":
        source = str((tool_input or {}).get("code") or (tool_input or {}).get("source") or "")
        classes = [CUA_REPL_CLASSES[m] for m in CUA_REPL_CALL.findall(source)]
        for preferred in ("drag", "click", "type", "key", "scroll", "set_value", "observe"):
            if preferred in classes:
                return preferred
        return "other"
    return "other"


def quota_from_event(event: dict[str, Any]) -> dict[str, Any] | None:
    """Normalise one ``rate_limit_event`` into utilisation numbers and a status."""
    if event.get("type") != "rate_limit_event":
        return None
    info = event.get("rate_limit_info") or {}
    windows = info.get("unifiedWindows") or {}

    def util(key: str) -> float | None:
        value = (windows.get(key) or {}).get("utilization")
        return float(value) if isinstance(value, (int, float)) else None

    def reset(key: str) -> float | None:
        value = (windows.get(key) or {}).get("resetsAt")
        return float(value) if isinstance(value, (int, float)) else None

    five, seven = util("five_hour"), util("seven_day")
    kind = info.get("rateLimitType")
    if kind == "five_hour" and five is None and isinstance(info.get("utilization"), (int, float)):
        five = float(info["utilization"])
    if kind == "seven_day" and seven is None and isinstance(info.get("utilization"), (int, float)):
        seven = float(info["utilization"])
    resets_at = info.get("resetsAt")
    return {
        "status": info.get("status"),
        "type": kind,
        "five_hour": five,
        "seven_day": seven,
        "five_hour_resets_at": reset("five_hour"),
        "seven_day_resets_at": reset("seven_day"),
        "resets_at": float(resets_at) if isinstance(resets_at, (int, float)) else None,
        "is_using_overage": bool(info.get("isUsingOverage", False)),
    }


def _usage_of(message: dict[str, Any]) -> dict[str, int]:
    usage = message.get("usage") or {}
    return {
        "input": int(usage.get("input_tokens") or 0),
        "output": int(usage.get("output_tokens") or 0),
        "cache_read": int(usage.get("cache_read_input_tokens") or 0),
        "cache_write": int(usage.get("cache_creation_input_tokens") or 0),
    }


def _text_of(content: Any) -> str:
    if isinstance(content, str):
        return content
    parts: list[str] = []
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(str(block.get("text", "")))
    return "\n".join(parts)


def summarize(events: Iterable[dict[str, Any]]) -> dict[str, Any]:
    """Counts, usage, cost, errors and quota state from one trial's stream."""
    events = list(events)
    init: dict[str, Any] = {}
    result: dict[str, Any] | None = None
    uses: dict[str, dict[str, Any]] = {}
    results: dict[str, dict[str, Any]] = {}
    per_message: dict[str, dict[str, int]] = {}
    message_order: list[str] = []
    quota_events: list[dict[str, Any]] = []
    control_requests = 0
    api_retries: list[dict[str, Any]] = []
    stamps: list[float] = []
    for event in events:
        kind = event.get("type")
        if "_ms" in event:
            stamps.append(event["_ms"])
        if kind == "system" and event.get("subtype") == "init":
            init = event
        elif kind == "system" and event.get("subtype") in ("api_retry", "api_error"):
            api_retries.append(
                {
                    k: event.get(k)
                    for k in (
                        "subtype",
                        "error",
                        "error_status",
                        "attempt",
                        "max_retries",
                        "retry_delay_ms",
                    )
                    if k in event
                }
            )
        elif kind == "assistant":
            message = event.get("message") or {}
            message_id = message.get("id") or f"anon-{len(message_order)}"
            if message_id not in per_message:
                message_order.append(message_id)
            per_message[message_id] = _usage_of(
                message
            )  # the last chunk of a message carries the final usage
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    uses[block["id"]] = {
                        "name": block.get("name", "?"),
                        "input": block.get("input") or {},
                        "ms": event.get("_ms"),
                        "turn": len(message_order),
                    }
        elif kind == "user":
            for block in (event.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    results[block.get("tool_use_id", "")] = {
                        "is_error": bool(block.get("is_error")),
                        "ms": event.get("_ms"),
                        "text": _text_of(block.get("content"))[:300]
                        if block.get("is_error")
                        else "",
                    }
        elif kind == "rate_limit_event":
            q = quota_from_event(event)
            if q:
                quota_events.append(q)
        elif kind == "control_request":
            control_requests += 1
        elif kind == "result":
            result = event

    by_name: Counter[str] = Counter()
    by_class: Counter[str] = Counter()
    errors_by_name: Counter[str] = Counter()
    latencies: dict[str, list[float]] = {}
    mcp_total = builtin_total = failed = 0
    for tool_id, use in uses.items():
        name = use["name"]
        by_name[name] += 1
        server, _ = strip_mcp(name)
        cls = tool_class(name, use["input"])
        by_class[cls] += 1
        if server is None:
            builtin_total += 1
        else:
            mcp_total += 1
        res = results.get(tool_id)
        if res and res["is_error"]:
            failed += 1
            errors_by_name[name] += 1
        if res and use.get("ms") is not None and res.get("ms") is not None and server is not None:
            latencies.setdefault(cls, []).append(res["ms"] - use["ms"])
    latency = {
        cls: {
            "n": len(vals),
            "median": round(statistics.median(vals), 1),
            "p90": round(sorted(vals)[min(len(vals) - 1, int(0.9 * len(vals)))], 1),
        }
        for cls, vals in latencies.items()
    }

    summed = {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0}
    for usage in per_message.values():
        for key, value in usage.items():
            summed[key] += value
    tokens = dict(summed)
    token_source = "per_message_sum"
    cost: float | None = None
    stats: dict[str, Any] = {}
    if result:
        usage = _usage_of(result)
        if any(usage.values()):
            tokens = usage
            token_source = "result"
        if isinstance(result.get("total_cost_usd"), (int, float)):
            cost = float(result["total_cost_usd"])
        stats = {
            "duration_ms": result.get("duration_ms"),
            "duration_api_ms": result.get("duration_api_ms"),
            "num_turns": result.get("num_turns"),
            "stop_reason": result.get("stop_reason"),
            "terminal_reason": result.get("terminal_reason"),
            "subtype": result.get("subtype"),
            "is_error": bool(result.get("is_error")),
            "api_error_status": result.get("api_error_status"),
            "permission_denials": len(result.get("permission_denials") or []),
            "model_usage": result.get("modelUsage"),
        }
    first_call = (
        per_message[message_order[0]]
        if message_order
        else {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0}
    )
    baseline_prompt_tokens = (
        first_call["input"] + first_call["cache_read"] + first_call["cache_write"]
    )

    tools = list(init.get("tools") or [])
    mcp_tools = [t for t in tools if t.startswith("mcp__")]
    summary = {
        "has_init": bool(init),
        "has_result": result is not None,
        "init": {
            "tools_builtin": sorted(t for t in tools if not t.startswith("mcp__")),
            "mcp_tool_count": len(mcp_tools),
            "mcp_tools": sorted(mcp_tools),
            "mcp_servers": init.get("mcp_servers") or [],
            "skills": init.get("skills") or [],
            "plugins": [
                p.get("name") if isinstance(p, dict) else p for p in (init.get("plugins") or [])
            ],
            "agents": init.get("agents") or [],
            "slash_command_count": len(init.get("slash_commands") or []),
            "model": init.get("model"),
            "permission_mode": init.get("permissionMode"),
            "claude_code_version": init.get("claude_code_version"),
            "cwd": init.get("cwd"),
            "memory_paths": init.get("memory_paths"),
            # Deferral shows up as a ToolSearch tool in the built-in list plus MCP tools absent from it.
            "tool_search_available": "ToolSearch" in tools,
        },
        "mcp_deferred": ("ToolSearch" in tools) and not mcp_tools,
        "turns": len(message_order),
        "api_calls": len(message_order),
        "tool_calls": {
            "total": len(uses),
            "mcp": mcp_total,
            "builtin": builtin_total,
            "by_name": dict(by_name),
            "by_class": dict(by_class),
            "failed": failed,
            "failed_by_name": dict(errors_by_name),
        },
        "action_latency_ms": latency,
        "tokens": tokens,
        "token_source": token_source,
        "tokens_summed_messages": summed,
        "baseline_prompt_tokens": baseline_prompt_tokens,
        "total_cost_usd": cost,
        "result": stats,
        "final_text": str((result or {}).get("result") or "")[:2000],
        "quota_events": quota_events,
        "quota_last": quota_events[-1] if quota_events else None,
        "control_requests": control_requests,
        "api_retries": api_retries,
        "first_ms": stamps[0] if stamps else None,
        "last_ms": stamps[-1] if stamps else None,
    }
    return summary


def classify_failure(
    summary: dict[str, Any], stderr_text: str = "", returncode: int | None = None
) -> dict[str, Any]:
    """Name the kind of failure, if any: ``rate_limit``, ``overloaded``, ``auth``, ``mcp_start``,
    ``harness_crash`` or ``None``. Also returns a reset epoch when the message carries one."""
    result = summary.get("result") or {}
    text = " ".join([summary.get("final_text") or "", stderr_text or ""])
    status = result.get("api_error_status")
    out: dict[str, Any] = {"kind": None, "message": "", "reset_epoch": None}
    quota = summary.get("quota_last") or {}
    rejected = [q for q in summary.get("quota_events", []) if q.get("status") == "rejected"]
    if rejected:
        last = rejected[-1]
        out.update(
            kind="rate_limit",
            message=f"rate_limit_event rejected ({last.get('type')})",
            reset_epoch=last.get("resets_at"),
            window=last.get("type"),
        )
        return out
    errored = result.get("is_error") or (
        summary.get("has_result") is False and returncode not in (0, None)
    )
    if result.get("is_error") or errored:
        if status == 429 or RATE_LIMIT_TEXT.search(text):
            out.update(kind="rate_limit", message=text.strip()[:300])
            epoch = re.search(r"\|(\d{10})\b", text)
            if epoch:
                out["reset_epoch"] = float(epoch.group(1))
            elif quota.get("resets_at"):
                out["reset_epoch"] = quota["resets_at"]
            return out
        if (
            status in (500, 502, 503, 504, 529)
            or OVERLOAD_TEXT.search(text)
            and result.get("is_error")
        ):
            out.update(kind="overloaded", message=text.strip()[:300])
            return out
        if status in (401, 403) or AUTH_TEXT.search(text):
            out.update(kind="auth", message=text.strip()[:300])
            return out
    servers = (summary.get("init") or {}).get("mcp_servers") or []
    bad = [
        s for s in servers if isinstance(s, dict) and s.get("status") not in ("connected", "ready")
    ]
    if bad:
        out.update(kind="mcp_start", message="mcp server not connected: " + json.dumps(bad)[:300])
        return out
    if not summary.get("has_init"):
        out.update(kind="harness_crash", message=(stderr_text or "no init event")[:300])
        return out
    return out
