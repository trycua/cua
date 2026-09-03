"""Utilities for parsing Qwen-style tool calls from assistant text."""

from __future__ import annotations

import json
import re
from typing import Any, Dict, List, Literal, Optional

ToolCallParser = Literal["qwen_xml"]

_TOOL_CALL_RE = re.compile(r"<tool_call>\s*([\s\S]*?)\s*</tool_call>")
_FUNCTION_RE = re.compile(r"<function=([A-Za-z_][\w.-]*)>\s*([\s\S]*?)\s*</function>")
_PARAMETER_OPEN_RE = re.compile(r"<parameter=([A-Za-z_][\w.-]*)>")

_DIRECT_COMPUTER_ACTIONS = {
    "click",
    "double_click",
    "drag",
    "hscroll",
    "key",
    "keypress",
    "left_click",
    "left_click_drag",
    "middle_click",
    "mouse_move",
    "move",
    "right_click",
    "screenshot",
    "scroll",
    "text",
    "triple_click",
    "type",
    "wait",
}


def _parse_json_value(value: str) -> Any:
    stripped = value.strip()
    try:
        return json.loads(stripped)
    except (json.JSONDecodeError, ValueError):
        return stripped


def _parse_json_tool_call(inner_text: str) -> Optional[Dict[str, Any]]:
    json_start = inner_text.find("{")
    if json_start == -1:
        return None

    brace_count = 0
    json_end = json_start
    for i in range(json_start, len(inner_text)):
        if inner_text[i] == "{":
            brace_count += 1
        elif inner_text[i] == "}":
            brace_count -= 1
            if brace_count == 0:
                json_end = i + 1
                break

    if brace_count != 0:
        return None

    try:
        tool_call = json.loads(inner_text[json_start:json_end])
    except Exception:
        return None

    if not isinstance(tool_call, dict):
        return None
    return tool_call


def _parse_qwen_xml_parameters(params_block: str) -> Dict[str, Any]:
    params: Dict[str, Any] = {}
    param_openings = list(_PARAMETER_OPEN_RE.finditer(params_block))

    for i, param_match in enumerate(param_openings):
        param_name = param_match.group(1)
        value_start = param_match.end()
        close_match = re.search(r"</parameter>", params_block[value_start:])
        next_open = param_openings[i + 1] if i + 1 < len(param_openings) else None

        if close_match is None:
            if param_name in _DIRECT_COMPUTER_ACTIONS and "action" not in params:
                params["action"] = param_name
            continue

        value_end = value_start + close_match.start()

        if (
            param_name in _DIRECT_COMPUTER_ACTIONS
            and next_open is not None
            and next_open.start() < value_end
            and "action" not in params
        ):
            params["action"] = param_name
            continue

        value = params_block[value_start:value_end]
        if param_name in _DIRECT_COMPUTER_ACTIONS and not value.strip():
            if "action" not in params:
                params["action"] = param_name
            continue
        params[param_name] = _parse_json_value(value)

    return params


def _parse_qwen_xml_tool_calls(inner_text: str) -> List[Dict[str, Any]]:
    tool_calls: List[Dict[str, Any]] = []
    for fn_match in _FUNCTION_RE.finditer(inner_text):
        fn_name = fn_match.group(1)
        params_block = fn_match.group(2)

        params = _parse_qwen_xml_parameters(params_block)

        if fn_name in _DIRECT_COMPUTER_ACTIONS:
            if "action" not in params and "type" not in params:
                params["action"] = fn_name
            tool_name = "computer"
        else:
            tool_name = fn_name

        if tool_name in {"computer", "computer_use"} and "type" in params:
            if "action" not in params:
                params["action"] = params["type"]
            del params["type"]

        tool_calls.append({"name": tool_name, "arguments": params})
    return tool_calls


def parse_tool_calls_from_text(
    text: str,
    tool_call_parser: Optional[ToolCallParser] = None,
) -> List[Dict[str, Any]]:
    """Extract tool calls from assistant text.

    With no parser selected, only JSON inside ``<tool_call>`` is parsed. The
    optional ``qwen_xml`` parser additionally accepts Qwen's XML-style function
    and parameter tags.
    """
    if tool_call_parser not in (None, "qwen_xml"):
        raise ValueError(f"Unsupported tool_call_parser: {tool_call_parser!r}")

    tool_calls: List[Dict[str, Any]] = []
    for match in _TOOL_CALL_RE.finditer(text):
        inner_text = match.group(1)
        json_tool_call = _parse_json_tool_call(inner_text)
        if json_tool_call:
            tool_calls.append(
                {
                    **json_tool_call,
                    "_raw_text": match.group(0),
                    "_raw_index": match.start(),
                    "_raw_end": match.end(),
                }
            )
            continue

        if tool_call_parser == "qwen_xml":
            for tool_call in _parse_qwen_xml_tool_calls(inner_text):
                tool_calls.append(
                    {
                        **tool_call,
                        "_raw_text": match.group(0),
                        "_raw_index": match.start(),
                        "_raw_end": match.end(),
                    }
                )

    return tool_calls


def parse_tool_call_from_text(
    text: str,
    tool_call_parser: Optional[ToolCallParser] = None,
) -> Optional[Dict[str, Any]]:
    """Extract the first tool call from assistant text."""
    tool_calls = parse_tool_calls_from_text(text, tool_call_parser=tool_call_parser)
    if not tool_calls:
        return None

    tool_call = dict(tool_calls[0])
    tool_call.pop("_raw_text", None)
    tool_call.pop("_raw_index", None)
    tool_call.pop("_raw_end", None)
    return tool_call


def strip_parsed_tool_calls_from_text(text: str, tool_calls: List[Dict[str, Any]]) -> str:
    """Remove parsed tool-call blocks from assistant text, preserving surrounding text."""
    stripped = text
    spans = set()
    for tool_call in tool_calls:
        raw_text = tool_call.get("_raw_text")
        raw_index = tool_call.get("_raw_index")
        raw_end = tool_call.get("_raw_end")
        if not isinstance(raw_text, str):
            continue

        if (
            isinstance(raw_index, int)
            and isinstance(raw_end, int)
            and raw_index >= 0
            and raw_end >= raw_index
            and text[raw_index:raw_end] == raw_text
        ):
            spans.add((raw_index, raw_end))
            continue

        start = text.find(raw_text)
        if start != -1:
            spans.add((start, start + len(raw_text)))

    for start, end in sorted(spans, reverse=True):
        stripped = stripped[:start] + stripped[end:]
    return stripped.strip()


def convert_qwen_tool_args_to_computer_action(args: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """
    Convert Qwen computer tool arguments to the Computer Calls action schema.

    Qwen (example):
        {"action": "left_click", "coordinate": [114, 68]}

    Target (example):
        {"action": "left_click", "x": 114, "y": 68}

    Other mappings:
    - right_click, middle_click, double_click (triple_click -> double_click)
    - mouse_move -> { action: "move", x, y }
    - key -> { action: "keypress", keys: [...] }
    - type -> { action: "type", text }
    - scroll/hscroll -> { action: "scroll", scroll_x, scroll_y, x, y }
    - wait -> { action: "wait" }
    - terminate/answer are not direct UI actions; return None for now
    """
    if not isinstance(args, dict):
        return None

    action = args.get("action")
    if not isinstance(action, str):
        return None

    coord = args.get("coordinate")
    if coord is None and args.get("x") is not None and args.get("y") is not None:
        coord = [args.get("x"), args.get("y")]

    x = y = None
    if isinstance(coord, (list, tuple)) and len(coord) >= 2:
        try:
            x = int(round(float(coord[0])))
            y = int(round(float(coord[1])))
        except Exception:
            x = y = None

    a = action.lower()
    if a == "click":
        if x is None or y is None:
            return None
        button = args.get("button", "left")
        if button == "right":
            return {"action": "right_click", "x": x, "y": y}
        if button in {"middle", "wheel"}:
            return {"action": "middle_click", "x": x, "y": y}
        return {"action": "left_click", "x": x, "y": y}
    if a in {"left_click", "right_click", "middle_click", "double_click"}:
        if x is None or y is None:
            return None
        return {"action": a, "x": x, "y": y}
    if a == "triple_click":
        if x is None or y is None:
            return None
        return {"action": "double_click", "x": x, "y": y}
    if a in {"mouse_move", "move"}:
        if x is None or y is None:
            return None
        return {"action": "move", "x": x, "y": y}
    if a in {"key", "keypress"}:
        keys = args.get("keys")
        if isinstance(keys, list) and all(isinstance(k, str) for k in keys):
            return {"action": "keypress", "keys": keys}
        return None
    if a in {"type", "text"}:
        text = args.get("text")
        if isinstance(text, str):
            return {"action": "type", "text": text}
        return None
    if a in {"scroll", "hscroll"}:
        pixels = args.get("pixels") or 0
        try:
            pixels_val = int(round(float(pixels)))
        except Exception:
            pixels_val = 0
        scroll_x = pixels_val if a == "hscroll" else 0
        scroll_y = pixels_val if a == "scroll" else 0
        out: Dict[str, Any] = {"action": "scroll", "scroll_x": scroll_x, "scroll_y": scroll_y}
        if x is not None and y is not None:
            out.update({"x": x, "y": y})
        return out
    if a == "wait":
        return {"action": "wait"}
    if a == "screenshot":
        return {"action": "screenshot"}

    return None
