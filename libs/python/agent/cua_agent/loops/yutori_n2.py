"""Yutori N2 assistant-text tool-call parsing utilities."""

from __future__ import annotations

import json
import re
import textwrap
from typing import Any, Dict, List, Mapping, Optional, Sequence, Tuple

_TOOL_CALL_RE = re.compile(r"<tool_call>\s*([\s\S]*?)\s*</tool_call>")
_FUNCTION_RE = re.compile(r"<function=([A-Za-z_][\w.-]*)>\s*([\s\S]*?)\s*</function>")
_MALFORMED_FUNCTION_RE = re.compile(
    r'\{\s*"function(?:"\s*:|=)\s*([A-Za-z_][\w.-]*)>\s*([\s\S]*?)\s*</function>'
)
_UNCLOSED_FUNCTION_RE = re.compile(r"<function=([A-Za-z_][\w.-]*)>\s*([\s\S]*)")
_PARAMETER_OPEN_RE = re.compile(r"<parameter=([A-Za-z_][\w.-]*)>")
_MALFORMED_PARAMETER_RE = re.compile(
    r"<parameter=([A-Za-z_][\w.-]*)\"?\s*:\s*([\s\S]*?)\s*</parameter>"
)
_MALFORMED_ACTION_ARGUMENTS_RE = re.compile(
    r"<parameter=([A-Za-z_][\w.-]*)\s*,\s*\"arguments\"\s*:\s*"
)
_MALFORMED_ACTION_FLAG_RE = re.compile(r"<parameter=([A-Za-z_][\w.-]*)\s*</parameter>")

_FUNCTION_TOOLS = {"bash", "read", "write", "edit"}
_COMPUTER_FUNCTIONS = {
    "click",
    "double_click",
    "drag",
    "hover",
    "hscroll",
    "key",
    "key_press",
    "keypress",
    "left_click",
    "left_click_drag",
    "left_mouse_down",
    "left_mouse_up",
    "middle_click",
    "mouse_down",
    "mouse_move",
    "mouse_up",
    "move",
    "right_click",
    "screenshot",
    "scroll",
    "text",
    "triple_click",
    "type",
    "wait",
    "write_text",
}
_ACTION_PARAMETER_NAMES = {
    "click",
    "double_click",
    "drag",
    "hover",
    "hscroll",
    "left_click",
    "left_click_drag",
    "left_mouse_down",
    "left_mouse_up",
    "middle_click",
    "mouse_down",
    "mouse_move",
    "mouse_up",
    "move",
    "right_click",
    "screenshot",
    "scroll",
    "triple_click",
    "wait",
}
_TEXT_PARAMETER_NAMES = {
    "command",
    "content",
    "input",
    "new",
    "old",
    "text",
    "value",
}


def _parse_json_value(value: str) -> Any:
    stripped = value.strip()
    try:
        return json.loads(stripped)
    except (json.JSONDecodeError, ValueError):
        return stripped


def _complete_json_object(value: str) -> Optional[str]:
    stack: List[str] = []
    in_string = False
    escaped = False

    for char in value:
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue

        if char == '"':
            in_string = True
        elif char in "{[":
            stack.append(char)
        elif char in "}]":
            if not stack:
                return None
            opener = stack.pop()
            if (opener, char) not in {("{", "}"), ("[", "]")}:
                return None

    if in_string or any(opener == "[" for opener in stack):
        return None
    if not stack:
        return value
    return value + ("}" * len(stack))


def _parse_text_parameter_value(value: str) -> str:
    stripped = value.strip()
    try:
        parsed = json.loads(stripped)
    except (json.JSONDecodeError, ValueError):
        parsed = None
    if isinstance(parsed, str):
        return parsed

    if "\n" not in value and "\r" not in value:
        return value

    if value.startswith("\r\n"):
        value = value[2:]
    elif value.startswith("\n"):
        value = value[1:]
    else:
        return value

    value = re.sub(r"\r?\n[ \t]*\Z", "", value)
    return textwrap.dedent(value).removesuffix("\n")


def _parse_parameter_value(param_name: str, value: str) -> Any:
    if param_name in _TEXT_PARAMETER_NAMES:
        return _parse_text_parameter_value(value)
    return _parse_json_value(value)


def _parse_json_tool_call(inner_text: str) -> Optional[Dict[str, Any]]:
    stripped = inner_text.strip()
    if not stripped.startswith("{"):
        return None

    candidates = [stripped]
    completed = _complete_json_object(stripped)
    if completed and completed != stripped:
        candidates.append(completed)

    for candidate in candidates:
        try:
            tool_call, _ = json.JSONDecoder().raw_decode(candidate)
        except Exception:
            continue

        if isinstance(tool_call, dict):
            return tool_call
    return None


def _with_raw_span(tool_call: Dict[str, Any], match: re.Match[str]) -> Dict[str, Any]:
    return {
        **tool_call,
        "_raw_text": match.group(0),
        "_raw_index": match.start(),
        "_raw_end": match.end(),
    }


def _parse_parameters(params_block: str) -> Dict[str, Any]:
    params: Dict[str, Any] = {}
    param_openings = list(_PARAMETER_OPEN_RE.finditer(params_block))

    for i, param_match in enumerate(param_openings):
        param_name = param_match.group(1)
        value_start = param_match.end()
        close_match = re.search(r"</parameter>", params_block[value_start:])
        next_open = param_openings[i + 1] if i + 1 < len(param_openings) else None

        if close_match is None:
            if param_name in _ACTION_PARAMETER_NAMES and "action" not in params:
                params["action"] = param_name
            continue

        value_end = value_start + close_match.start()

        if (
            param_name in _ACTION_PARAMETER_NAMES
            and next_open is not None
            and next_open.start() < value_end
            and "action" not in params
        ):
            params["action"] = param_name
            continue

        value = params_block[value_start:value_end]
        if param_name in _ACTION_PARAMETER_NAMES:
            parsed_value = _parse_json_value(value)
            if "action" not in params:
                params["action"] = param_name
            if (
                _as_coordinates(parsed_value) is not None
                and "coordinate" not in params
                and "coordinates" not in params
            ):
                params["coordinate"] = parsed_value
            elif isinstance(parsed_value, Mapping):
                for key, parsed_arg in parsed_value.items():
                    params.setdefault(key, parsed_arg)
            continue

        params[param_name] = _parse_parameter_value(param_name, value)

    for malformed_match in _MALFORMED_PARAMETER_RE.finditer(params_block):
        param_name = malformed_match.group(1)
        if param_name in params:
            continue
        params[param_name] = _parse_parameter_value(param_name, malformed_match.group(2))

    for malformed_match in _MALFORMED_ACTION_FLAG_RE.finditer(params_block):
        param_name = malformed_match.group(1)
        if param_name in _ACTION_PARAMETER_NAMES and "action" not in params:
            params["action"] = param_name

    for malformed_match in _MALFORMED_ACTION_ARGUMENTS_RE.finditer(params_block):
        param_name = malformed_match.group(1)
        if param_name in _ACTION_PARAMETER_NAMES and "action" not in params:
            params["action"] = param_name

        try:
            parsed_args, _ = json.JSONDecoder().raw_decode(
                params_block[malformed_match.end() :].lstrip()
            )
        except Exception:
            continue
        if not isinstance(parsed_args, Mapping):
            continue
        for key, value in parsed_args.items():
            params.setdefault(key, value)

    for unclosed_match in _PARAMETER_OPEN_RE.finditer(params_block):
        param_name = unclosed_match.group(1)
        if param_name in params:
            continue
        if param_name in _ACTION_PARAMETER_NAMES:
            continue

        raw_value = params_block[unclosed_match.end() :]
        if param_name == "action":
            action_value, separator, extra_fields = raw_value.strip().partition(",")
            action = action_value.strip().strip('{}"')
            if action:
                params["action"] = action
            if separator:
                parsed_extra = _parse_json_value("{" + extra_fields.strip())
                if isinstance(parsed_extra, Mapping):
                    for key, parsed_value in parsed_extra.items():
                        params.setdefault(key, parsed_value)
            continue

        params[param_name] = _parse_parameter_value(param_name, raw_value)

    return params


def _parse_xml_tool_calls(inner_text: str) -> List[Dict[str, Any]]:
    tool_calls: List[Dict[str, Any]] = []
    function_matches = [
        *list(_FUNCTION_RE.finditer(inner_text)),
        *list(_MALFORMED_FUNCTION_RE.finditer(inner_text)),
    ]
    if not function_matches:
        function_matches = list(_UNCLOSED_FUNCTION_RE.finditer(inner_text))

    for fn_match in function_matches:
        fn_name = fn_match.group(1)
        params = _parse_parameters(fn_match.group(2))

        if fn_name in {"computer", "computer_use"} and "type" in params:
            if "action" not in params:
                type_value = params["type"]
                if (
                    isinstance(type_value, str)
                    and type_value.lower() not in _COMPUTER_FUNCTIONS
                    and "text" not in params
                ):
                    params["action"] = "type"
                    params["text"] = type_value
                else:
                    params["action"] = type_value
            del params["type"]
        if (
            fn_name in {"computer", "computer_use"}
            and "action" not in params
            and isinstance(params.get("button"), str)
            and str(params["button"]).lower() in _COMPUTER_FUNCTIONS
        ):
            params["action"] = str(params.pop("button")).lower()

        tool_calls.append({"name": fn_name, "arguments": params})
    return tool_calls


def parse_yutori_n2_tool_calls_from_text(text: str) -> List[Dict[str, Any]]:
    """Extract Yutori N2 tool calls from assistant text."""
    tool_calls: List[Dict[str, Any]] = []
    for match in _TOOL_CALL_RE.finditer(text):
        inner_text = match.group(1)
        json_tool_call = _parse_json_tool_call(inner_text)
        if json_tool_call:
            tool_calls.append(_with_raw_span(json_tool_call, match))
            continue

        for tool_call in _parse_xml_tool_calls(inner_text):
            tool_calls.append(_with_raw_span(tool_call, match))

    return tool_calls


def parse_yutori_n2_tool_call_from_text(text: str) -> Optional[Dict[str, Any]]:
    """Extract the first Yutori N2 tool call from assistant text."""
    tool_calls = parse_yutori_n2_tool_calls_from_text(text)
    if not tool_calls:
        return None

    tool_call = dict(tool_calls[0])
    tool_call.pop("_raw_text", None)
    tool_call.pop("_raw_index", None)
    tool_call.pop("_raw_end", None)
    return tool_call


def strip_parsed_yutori_n2_tool_calls_from_text(
    text: str,
    tool_calls: List[Dict[str, Any]],
) -> str:
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


def _as_int(value: Any) -> Optional[int]:
    if value is None:
        return None
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return round(value)
    if isinstance(value, str):
        try:
            return round(float(value.strip()))
        except ValueError:
            return None
    return None


def _as_coordinates(value: Any) -> Optional[Tuple[int, int]]:
    if isinstance(value, str):
        parsed = _parse_json_value(value)
        if parsed == value:
            return None
        value = parsed

    if not isinstance(value, Sequence) or isinstance(value, str) or len(value) < 2:
        return None

    x = _as_int(value[0])
    y = _as_int(value[1])
    if x is None or y is None:
        return None
    return x, y


def _scale_normalized_coordinates(
    x: int,
    y: int,
    dimensions: Optional[Tuple[int, int]],
) -> Tuple[int, int]:
    if dimensions is None:
        return x, y

    width, height = dimensions
    return (
        max(0, min(width - 1, round((x / 1000) * width))),
        max(0, min(height - 1, round((y / 1000) * height))),
    )


def _coordinates_from_args(
    args: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
) -> Optional[Tuple[int, int]]:
    x = _as_int(args.get("x"))
    y = _as_int(args.get("y"))
    if x is not None and y is not None:
        return _scale_normalized_coordinates(x, y, dimensions)

    for key in ("coordinate", "coordinates"):
        parsed = _as_coordinates(args.get(key))
        if parsed is not None:
            return _scale_normalized_coordinates(parsed[0], parsed[1], dimensions)
    return None


def _text_from_args(args: Mapping[str, Any]) -> Optional[str]:
    for key in ("text", "content", "value", "input"):
        value = args.get(key)
        if isinstance(value, str):
            return value
    return None


def _scroll_delta(
    action_name: str,
    args: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
) -> Tuple[int, int]:
    pixels = _as_int(args.get("pixels"))
    if pixels is not None:
        if action_name == "hscroll":
            return pixels, 0
        return 0, pixels

    amount = _as_int(args.get("amount")) or 3
    direction = str(args.get("direction") or "down").lower()
    width, height = dimensions or (1000, 1000)
    step_x = max(1, round(width * 0.1))
    step_y = max(1, round(height * 0.1))
    if direction == "up":
        return 0, -amount * step_y
    if direction == "left":
        return -amount * step_x, 0
    if direction == "right":
        return amount * step_x, 0
    return 0, amount * step_y


def convert_yutori_n2_tool_call_to_completion_tool_calls(
    tool_call: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]] = None,
) -> List[Dict[str, Any]]:
    """Expand a parsed Yutori N2 tool call into OpenAI-style function calls."""
    function_name = str(tool_call.get("name") or "")
    raw_args = tool_call.get("arguments") or {}
    args = _normalize_args(raw_args)

    if function_name == "computer_batch":
        actions = args.get("actions")
        if not isinstance(actions, Sequence) or isinstance(actions, str):
            return []

        expanded_calls: List[Dict[str, Any]] = []
        for action in actions:
            if not isinstance(action, Mapping):
                break
            action_args = action.get("arguments") or {}
            nested_call = {
                "name": action.get("name") or "",
                "arguments": _normalize_args(action_args),
            }
            nested_calls = convert_yutori_n2_tool_call_to_completion_tool_calls(
                nested_call,
                dimensions=dimensions,
            )
            if not nested_calls:
                break
            expanded_calls.extend(nested_calls)
        return expanded_calls

    if function_name in _FUNCTION_TOOLS:
        return [{"name": function_name, "arguments": dict(args)}]

    computer_actions = _convert_args_to_computer_actions(function_name, args, dimensions)
    if computer_actions:
        return [{"name": "computer", "arguments": action} for action in computer_actions]

    if function_name not in {"computer", "computer_use", *_COMPUTER_FUNCTIONS}:
        return [{"name": function_name, "arguments": dict(args)}]

    return []


def _normalize_args(raw_args: Any) -> Dict[str, Any]:
    if not isinstance(raw_args, Mapping):
        return {}

    args = dict(raw_args)
    nested_args = args.get("arguments")
    if isinstance(nested_args, Mapping):
        outer_args = {key: value for key, value in args.items() if key != "arguments"}
        return {**nested_args, **outer_args}
    return args


def _computer_action_name(function_name: str, args: Mapping[str, Any]) -> str:
    action_name = str(args.get("type") or args.get("action") or function_name).lower()
    if action_name in {"computer", "computer_use"} and any(
        key in args for key in ("key", "keys", "key_comb")
    ):
        return "keypress"
    return action_name


def _keypress_actions_from_args(args: Mapping[str, Any]) -> List[Dict[str, Any]]:
    value = args.get("keys") or args.get("key") or args.get("key_comb")
    if isinstance(value, str):
        stripped = value.strip()
        if not stripped:
            return []
        if stripped.startswith("["):
            parsed = _parse_json_value(stripped)
            if isinstance(parsed, list):
                return [{"action": "keypress", "keys": [str(item) for item in parsed]}]
        if "+" in stripped:
            keys = [part.strip() for part in stripped.split("+") if part.strip()]
            return [{"action": "keypress", "keys": keys}] if keys else []

        keys = [part.strip() for part in stripped.split() if part.strip()]
        if len(keys) > 1:
            return [{"action": "keypress", "keys": [key]} for key in keys]
        return [{"action": "keypress", "keys": [stripped]}]

    if isinstance(value, Sequence):
        return [{"action": "keypress", "keys": [str(item) for item in value]}]
    return []


def _convert_args_to_computer_actions(
    function_name: str,
    args: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
) -> List[Dict[str, Any]]:
    action_name = _computer_action_name(function_name, args)
    if action_name in {"key", "key_press", "keypress"}:
        return _keypress_actions_from_args(args)

    computer_action = _convert_args_to_computer_action(
        function_name,
        args,
        dimensions,
        action_name=action_name,
    )
    return [computer_action] if computer_action else []


def _convert_args_to_computer_action(
    function_name: str,
    args: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
    *,
    action_name: Optional[str] = None,
) -> Optional[Dict[str, Any]]:
    action_name = action_name or _computer_action_name(function_name, args)
    coords = _coordinates_from_args(args, dimensions)

    if action_name == "screenshot":
        return {"action": "screenshot"}
    if action_name in {"left_click", "click"} and coords is not None:
        return {
            "action": "click",
            "button": str(args.get("button") or "left"),
            "x": coords[0],
            "y": coords[1],
        }
    if action_name == "right_click" and coords is not None:
        return {"action": "click", "button": "right", "x": coords[0], "y": coords[1]}
    if action_name == "middle_click" and coords is not None:
        return {"action": "click", "button": "middle", "x": coords[0], "y": coords[1]}
    if action_name in {"double_click", "triple_click"} and coords is not None:
        return {"action": "double_click", "x": coords[0], "y": coords[1]}
    if action_name in {"mouse_move", "hover", "move"} and coords is not None:
        return {"action": "move", "x": coords[0], "y": coords[1]}
    if action_name in {"left_mouse_down", "mouse_down"}:
        action = {"action": "left_mouse_down"}
        if coords is not None:
            action.update({"x": coords[0], "y": coords[1]})
        return action
    if action_name in {"left_mouse_up", "mouse_up"}:
        action = {"action": "left_mouse_up"}
        if coords is not None:
            action.update({"x": coords[0], "y": coords[1]})
        return action
    if action_name in {"left_click_drag", "drag"}:
        start = _as_coordinates(args.get("start_coordinates"))
        end = _as_coordinates(args.get("end_coordinates")) or coords
        if start is None or end is None:
            return None

        start = _scale_normalized_coordinates(start[0], start[1], dimensions)
        end = _scale_normalized_coordinates(end[0], end[1], dimensions)
        return {
            "action": "drag",
            "path": [{"x": start[0], "y": start[1]}, {"x": end[0], "y": end[1]}],
        }
    if action_name in {"scroll", "hscroll"}:
        x, y = coords or (0, 0)
        scroll_x, scroll_y = _scroll_delta(action_name, args, dimensions)
        return {
            "action": "scroll",
            "x": x,
            "y": y,
            "scroll_x": scroll_x,
            "scroll_y": scroll_y,
        }
    if action_name in {"type", "text", "write_text"}:
        text = _text_from_args(args)
        if text is not None:
            return {"action": "type", "text": text}
        return None
    if action_name in {"key", "key_press", "keypress"}:
        keypress_actions = _keypress_actions_from_args(args)
        if len(keypress_actions) == 1:
            return keypress_actions[0]
        return None
    if action_name == "wait":
        return {"action": "wait"}

    return None
