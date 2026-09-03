"""Yutori N2 agent loop and assistant-text tool-call parsing utilities."""

from __future__ import annotations

import asyncio
import base64
import io
import json
import os
import re
import textwrap
from pathlib import Path
from typing import Any, Awaitable, Callable, Dict, List, Mapping, Optional, Sequence, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
    make_function_call_item,
    make_reasoning_item,
    random_id,
)
from ..types import AgentCapability

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
    "file_path",
    "input",
    "new",
    "new_string",
    "old",
    "old_string",
    "text",
    "value",
}

YUTORI_N2_TOOL_SET = "computer_use_tools-20260830"
YUTORI_N2_COORD_SPACE = 1000
YUTORI_N2_MAX_BATCH_ACTIONS = 100
YUTORI_N2_MAX_WAIT_SECONDS = 300
YUTORI_N2_MAX_SCROLL_AMOUNT = 50
YUTORI_N2_DEFAULT_READ_LIMIT = 2000
YUTORI_N2_WRITE_CONTENT_MAX_CHARS = 256000
YUTORI_N2_BASH_DEFAULT_TIMEOUT_SECONDS = 120
YUTORI_N2_BASH_MAX_TIMEOUT_SECONDS = 600
YUTORI_N2_SKIPPED_TOOL_CALL_MESSAGE = "Skipped malformed Yutori N2 tool call."

_YUTORI_N2_SYSTEM_PROMPT = (
    "You are a computer use agent. Use the provided desktop tools to complete the "
    "user request. Output normalized coordinates in a 1000x1000 space. Use "
    "computer_batch for GUI actions so the actions are validated together and "
    "the computer returns one screenshot after the batch."
)

_MODIFIER_ALIASES = {
    "alt": "alt",
    "cmd": "cmd",
    "command": "cmd",
    "control": "ctrl",
    "ctrl": "ctrl",
    "meta": "cmd",
    "shift": "shift",
    "super": "cmd",
    "win": "cmd",
}

_BATCH_ACTION_FIELDS = {
    "left_click": {"action", "coordinates", "modifier"},
    "double_click": {"action", "coordinates", "modifier"},
    "triple_click": {"action", "coordinates", "modifier"},
    "middle_click": {"action", "coordinates", "modifier"},
    "right_click": {"action", "coordinates", "modifier"},
    "mouse_move": {"action", "coordinates"},
    "drag": {"action", "start_coordinates", "coordinates"},
    "scroll": {"action", "coordinates", "direction", "amount", "modifier"},
    "type": {"action", "text"},
    "key_press": {"action", "key"},
    "wait": {"action", "duration"},
    "mouse_down": {"action", "coordinates"},
    "mouse_up": {"action", "coordinates"},
    "hold_key": {"action", "key", "duration"},
    "screenshot": {"action"},
}

_OPTIONAL_COORDINATE_ACTIONS = {"mouse_down", "mouse_up"}

_YUTORI_N2_BATCH_ACTION_NAMES = [
    "left_click",
    "double_click",
    "triple_click",
    "middle_click",
    "right_click",
    "mouse_move",
    "drag",
    "scroll",
    "type",
    "key_press",
    "wait",
    "mouse_down",
    "mouse_up",
    "hold_key",
    "screenshot",
]


def _coordinate_schema(description: str = "Normalized [x, y] coordinates.") -> Dict[str, Any]:
    return {
        "description": description,
        "type": "array",
        "items": {"type": "integer", "minimum": 0, "maximum": 1000},
        "minItems": 2,
        "maxItems": 2,
    }


YUTORI_N2_TOOLS: List[Dict[str, Any]] = [
    {
        "type": "function",
        "function": {
            "name": "computer_batch",
            "description": (
                "Execute a sequence of desktop GUI actions in one logical call. "
                "The batch is validated before execution, members run in order, "
                "execution stops on the first runtime error, and one screenshot is "
                "returned after the batch."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "items": {
                            "anyOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "name": {
                                            "type": "string",
                                            "enum": _YUTORI_N2_BATCH_ACTION_NAMES,
                                        },
                                        "arguments": {"type": "object"},
                                    },
                                    "required": ["name", "arguments"],
                                    "additionalProperties": False,
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "action": {
                                            "type": "string",
                                            "enum": _YUTORI_N2_BATCH_ACTION_NAMES,
                                        },
                                        "action_type": {
                                            "type": "string",
                                            "enum": _YUTORI_N2_BATCH_ACTION_NAMES,
                                        },
                                        "coordinates": _coordinate_schema(),
                                        "start_coordinates": _coordinate_schema(
                                            "Normalized drag start coordinates."
                                        ),
                                        "direction": {"type": "string", "enum": ["up", "down"]},
                                        "amount": {
                                            "type": "integer",
                                            "minimum": 1,
                                            "maximum": YUTORI_N2_MAX_SCROLL_AMOUNT,
                                        },
                                        "modifier": {"type": "string"},
                                        "text": {"type": "string"},
                                        "key": {"type": "string"},
                                        "duration": {
                                            "type": "number",
                                            "minimum": 0,
                                            "maximum": YUTORI_N2_MAX_WAIT_SECONDS,
                                        },
                                    },
                                    "additionalProperties": False,
                                },
                            ]
                        },
                    }
                },
                "required": ["actions"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "edit",
            "description": "Replace exact text in a local file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean", "default": False},
                },
                "required": ["file_path", "old_string", "new_string"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "read",
            "description": "Read a local text file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 1, "default": 1},
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "default": YUTORI_N2_DEFAULT_READ_LIMIT,
                    },
                },
                "required": ["file_path"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "write",
            "description": "Write content to a local file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "content": {
                        "type": "string",
                        "maxLength": YUTORI_N2_WRITE_CONTENT_MAX_CHARS,
                    },
                },
                "required": ["file_path", "content"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "bash",
            "description": "Run a local shell command.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": YUTORI_N2_BASH_MAX_TIMEOUT_SECONDS,
                        "default": YUTORI_N2_BASH_DEFAULT_TIMEOUT_SECONDS,
                    },
                    "run_in_background": {"type": "boolean", "default": False},
                },
                "required": ["command"],
                "additionalProperties": False,
            },
        },
    },
]


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
) -> Optional[Tuple[int, int]]:
    if not (0 <= x <= YUTORI_N2_COORD_SPACE and 0 <= y <= YUTORI_N2_COORD_SPACE):
        return None

    if dimensions is None:
        return x, y

    width, height = dimensions
    return (
        max(0, min(width - 1, round((x / YUTORI_N2_COORD_SPACE) * width))),
        max(0, min(height - 1, round((y / YUTORI_N2_COORD_SPACE) * height))),
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
        for index, action in enumerate(actions):
            try:
                action_name, action_args = _normalize_batch_member(action, index)
            except ValueError:
                break
            nested_call = {
                "name": action_name,
                "arguments": action_args,
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


def _normalize_batch_member(value: Any, index: int) -> Tuple[str, Dict[str, Any]]:
    path = f"computer_batch.actions[{index}]"
    if not isinstance(value, Mapping):
        raise ValueError(f"{path} must be an object")

    member = dict(value)
    if "name" in member:
        unknown_field = next(
            (key for key in member if key not in {"name", "arguments"}),
            None,
        )
        if unknown_field:
            raise ValueError(f"{path}.{unknown_field} is not allowed")
        name = member.get("name")
        if not isinstance(name, str):
            raise ValueError(f"{path}.name must be a string")
        arguments = member.get("arguments")
        if not isinstance(arguments, Mapping):
            raise ValueError(f"{path}.arguments must be an object")
        return name, _normalize_args(arguments)

    if "action" in member and "action_type" in member:
        raise ValueError(f"{path}.action_type is not allowed when action is present")

    action_field = "action" if "action" in member else "action_type"
    name = member.get(action_field)
    if not isinstance(name, str):
        raise ValueError(f"{path}.action is unsupported")

    return name, _normalize_args({key: val for key, val in member.items() if key != action_field})


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
    if action_name == "triple_click":
        coords = _coordinates_from_args(args, dimensions)
        if coords is None:
            return []
        return [
            {"action": "click", "button": "left", "x": coords[0], "y": coords[1]} for _ in range(3)
        ]

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
    if action_name == "double_click" and coords is not None:
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
        if start is None or end is None:
            return None
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
        duration = args.get("duration", args.get("time"))
        if duration is None:
            return {"action": "wait"}
        seconds = _as_int(duration)
        if seconds is None:
            return None
        return {"action": "wait", "ms": seconds * 1000}

    return None


def _completion_content_to_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""

    text_parts = []
    for part in content:
        if isinstance(part, Mapping) and part.get("type") in {"text", "output_text"}:
            text = part.get("text")
            if isinstance(text, str):
                text_parts.append(text)
    return "\n".join(text_parts)


def _completion_tool_calls_to_yutori_tool_calls(
    tool_calls: Sequence[Mapping[str, Any]],
) -> List[Dict[str, Any]]:
    parsed: List[Dict[str, Any]] = []
    for index, tool_call in enumerate(tool_calls):
        function = tool_call.get("function") or {}
        if not isinstance(function, Mapping):
            continue

        name = function.get("name")
        if not isinstance(name, str):
            continue

        raw_arguments = function.get("arguments") or {}
        arguments = (
            _parse_json_value(raw_arguments) if isinstance(raw_arguments, str) else raw_arguments
        )
        parsed.append(
            {
                "name": name,
                "arguments": _normalize_args(arguments),
                "_call_id": tool_call.get("id") or tool_call.get("call_id") or f"call_{index}",
            }
        )
    return parsed


def _function_call_with_output(
    name: str,
    arguments: Mapping[str, Any],
    output: str,
    call_id: Optional[str],
) -> List[Dict[str, Any]]:
    resolved_call_id = call_id or random_id()
    return [
        make_function_call_item(name, dict(arguments), call_id=resolved_call_id),
        {
            "type": "function_call_output",
            "call_id": resolved_call_id,
            "output": output,
        },
    ]


def _screenshot_message(screenshot_b64: str) -> Dict[str, Any]:
    return {
        "type": "message",
        "role": "user",
        "content": [
            {
                "type": "input_image",
                "image_url": f"data:image/png;base64,{screenshot_b64}",
            },
            {"type": "input_text", "text": "Current screen"},
        ],
    }


async def _get_computer_dimensions(computer_handler: Any) -> Optional[Tuple[int, int]]:
    if computer_handler is None:
        return None

    try:
        if hasattr(computer_handler, "get_dimensions"):
            dimensions = await computer_handler.get_dimensions()
        elif hasattr(computer_handler, "interface") and hasattr(
            computer_handler.interface, "get_screen_size"
        ):
            dimensions = await computer_handler.interface.get_screen_size()
        else:
            return None

        if isinstance(dimensions, Mapping):
            width = dimensions.get("width")
            height = dimensions.get("height")
        else:
            width, height = dimensions

        return int(width), int(height)
    except Exception:
        return None


def _messages_have_image(messages: Sequence[Mapping[str, Any]]) -> bool:
    for message in messages:
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if isinstance(part, Mapping) and part.get("type") == "image_url":
                return True
    return False


def _messages_have_screenshot_notice(messages: Sequence[Mapping[str, Any]]) -> bool:
    screenshot_text = "Taking a screenshot to see the current computer screen."
    for message in messages:
        content = message.get("content")
        if isinstance(content, str) and screenshot_text in content:
            return True
        if not isinstance(content, list):
            continue
        for part in content:
            if isinstance(part, Mapping) and screenshot_text in str(part.get("text") or ""):
                return True
    return False


async def _ensure_initial_screenshot(
    completion_messages: List[Dict[str, Any]],
    response_messages: Sequence[Mapping[str, Any]],
    computer_handler: Any,
    on_screenshot: Optional[Callable[[str, str], Awaitable[None]]],
) -> List[Dict[str, Any]]:
    pre_output_items: List[Dict[str, Any]] = []
    if _messages_have_image(completion_messages):
        return pre_output_items

    if computer_handler is None or not hasattr(computer_handler, "screenshot"):
        raise RuntimeError(
            "No screenshots present and computer_handler.screenshot is not available."
        )

    screenshot_b64 = await computer_handler.screenshot()
    if not screenshot_b64:
        raise RuntimeError("Failed to capture screenshot from computer_handler.")
    if on_screenshot:
        await on_screenshot(screenshot_b64, "screenshot")

    completion_messages.append(
        {
            "role": "user",
            "content": [
                {
                    "type": "image_url",
                    "image_url": {"url": f"data:image/png;base64,{screenshot_b64}"},
                },
                {"type": "text", "text": "Current screen"},
            ],
        }
    )
    if not _messages_have_screenshot_notice(response_messages):
        pre_output_items.append(
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "Taking a screenshot to see the current computer screen.",
                    }
                ],
            }
        )
    return pre_output_items


def _add_image_resize_hints(
    completion_messages: Sequence[Mapping[str, Any]],
) -> Optional[Tuple[int, int]]:
    min_pixels = 3136
    max_pixels = 12845056
    last_dimensions: Optional[Tuple[int, int]] = None

    try:
        from PIL import Image  # type: ignore
        from qwen_vl_utils import smart_resize  # type: ignore
    except Exception:
        return None

    for message in completion_messages:
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if not isinstance(part, dict) or part.get("type") != "image_url":
                continue
            url = ((part.get("image_url") or {}).get("url")) or ""
            if not (url.startswith("data:") and "," in url):
                continue

            try:
                image_bytes = base64.b64decode(url.split(",", 1)[1])
                image = Image.open(io.BytesIO(image_bytes))
            except Exception:
                continue

            resized_height, resized_width = smart_resize(
                image.height,
                image.width,
                factor=32,
                min_pixels=min_pixels,
                max_pixels=max_pixels,
            )
            part["min_pixels"] = min_pixels
            part["max_pixels"] = max_pixels
            last_dimensions = (resized_width, resized_height)
    return last_dimensions


def _extra_function_tools(tools: Optional[Sequence[Mapping[str, Any]]]) -> List[Dict[str, Any]]:
    extra_tools = []
    if not tools:
        return extra_tools

    native_names = {tool["function"]["name"] for tool in YUTORI_N2_TOOLS}
    for tool in tools:
        if tool.get("type") != "function":
            continue
        function = tool.get("function")
        if not isinstance(function, Mapping):
            continue
        name = function.get("name")
        if isinstance(name, str) and name not in native_names:
            extra_tools.append({"type": "function", "function": dict(function)})
    return extra_tools


def _uses_yutori_api(api_base: Optional[str]) -> bool:
    if not api_base:
        return True
    return "api.yutori.com" in api_base


def _as_number(value: Any) -> Optional[float]:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value.strip())
        except ValueError:
            return None
    return None


def _as_positive_int(value: Any) -> Optional[int]:
    number = _as_number(value)
    if number is None or not number.is_integer():
        return None
    return int(number)


def _canonicalize_action_args(
    action_name: str,
    raw_args: Mapping[str, Any],
) -> Tuple[str, Dict[str, Any]]:
    args = _normalize_args(raw_args)

    if action_name in {"computer", "computer_use"}:
        action_name = _computer_action_name(action_name, args)
    if action_name in {"click"}:
        button = str(args.get("button") or "left").lower()
        if button == "right":
            action_name = "right_click"
        elif button in {"middle", "wheel"}:
            action_name = "middle_click"
        else:
            action_name = "left_click"
    elif action_name in {"move", "hover"}:
        action_name = "mouse_move"
    elif action_name in {"key", "keypress"}:
        action_name = "key_press"
    elif action_name in {"text", "write_text"}:
        action_name = "type"
    elif action_name == "left_click_drag":
        action_name = "drag"
    elif action_name == "left_mouse_down":
        action_name = "mouse_down"
    elif action_name == "left_mouse_up":
        action_name = "mouse_up"

    canonical_args = dict(args)
    canonical_args.pop("button", None)
    if "coordinate" in canonical_args and "coordinates" not in canonical_args:
        canonical_args["coordinates"] = canonical_args.pop("coordinate")
    if "end_coordinates" in canonical_args and "coordinates" not in canonical_args:
        canonical_args["coordinates"] = canonical_args.pop("end_coordinates")
    if "x" in canonical_args and "y" in canonical_args and "coordinates" not in canonical_args:
        canonical_args["coordinates"] = [canonical_args.pop("x"), canonical_args.pop("y")]
    if "keys" in canonical_args and "key" not in canonical_args:
        keys = canonical_args.pop("keys")
        if isinstance(keys, Sequence) and not isinstance(keys, str):
            canonical_args["key"] = "+".join(str(key) for key in keys)
        else:
            canonical_args["key"] = keys
    if "key_comb" in canonical_args and "key" not in canonical_args:
        canonical_args["key"] = canonical_args.pop("key_comb")
    if "time" in canonical_args and "duration" not in canonical_args:
        canonical_args["duration"] = canonical_args.pop("time")

    canonical_args.pop("type", None)
    action_arg = canonical_args.get("action")
    if isinstance(action_arg, str) and action_arg:
        canonical_args.pop("action", None)

    return action_name, canonical_args


def _require_normalized_coordinates(value: Any, path: str) -> Tuple[int, int]:
    if (
        not isinstance(value, Sequence)
        or isinstance(value, str)
        or len(value) != 2
        or any(isinstance(component, bool) for component in value)
        or not all(isinstance(component, int) for component in value)
        or not all(0 <= component <= YUTORI_N2_COORD_SPACE for component in value)
    ):
        raise ValueError(f"{path} must be two integers in the inclusive 0-1000 range")
    return int(value[0]), int(value[1])


def _scale_required_coordinates(
    value: Any,
    path: str,
    dimensions: Optional[Tuple[int, int]],
) -> Tuple[int, int]:
    x, y = _require_normalized_coordinates(value, path)
    scaled = _scale_normalized_coordinates(x, y, dimensions)
    if scaled is None:
        raise ValueError(f"{path} must be two integers in the inclusive 0-1000 range")
    return scaled


def _parse_modifier_keys(value: Any) -> Optional[List[str]]:
    if value is None or value == "":
        return []
    if not isinstance(value, str):
        return None

    keys = []
    for part in re.split(r"[+\s]+", value.strip()):
        if not part:
            continue
        normalized = _MODIFIER_ALIASES.get(part.lower())
        if normalized is None:
            return None
        keys.append(normalized)
    return keys


def _validate_computer_action(
    action_name: str,
    args: Mapping[str, Any],
    index: int,
    dimensions: Optional[Tuple[int, int]],
) -> Dict[str, Any]:
    action_name, args = _canonicalize_action_args(action_name, args)
    path = f"computer_batch.actions[{index}]"

    if action_name == "computer_batch":
        raise ValueError(f"{path}.action cannot be computer_batch")
    if action_name in _FUNCTION_TOOLS:
        raise ValueError(f"{path}.action cannot execute shell commands")
    if action_name not in _BATCH_ACTION_FIELDS:
        raise ValueError(f"{path}.action is unsupported")

    allowed_fields = _BATCH_ACTION_FIELDS[action_name]
    unknown_field = next((key for key in args if key not in allowed_fields), None)
    if unknown_field:
        raise ValueError(f"{path}.{unknown_field} is not allowed for {action_name}")

    modifier_keys = _parse_modifier_keys(args.get("modifier"))
    if modifier_keys is None:
        allowed_modifiers = ", ".join(sorted(_MODIFIER_ALIASES))
        raise ValueError(
            f"{path}.modifier must be one or more of {allowed_modifiers}, "
            'e.g. "ctrl" or "ctrl+shift"'
        )

    action: Dict[str, Any] = {"action": action_name}
    if "modifier" in args:
        action["modifier"] = args["modifier"]

    if action_name in {
        "left_click",
        "double_click",
        "triple_click",
        "middle_click",
        "right_click",
        "mouse_move",
    }:
        x, y = _scale_required_coordinates(
            args.get("coordinates"),
            f"{path}.coordinates",
            dimensions,
        )
        action.update({"x": x, "y": y})
    elif action_name == "drag":
        start_x, start_y = _scale_required_coordinates(
            args.get("start_coordinates"),
            f"{path}.start_coordinates",
            dimensions,
        )
        end_x, end_y = _scale_required_coordinates(
            args.get("coordinates"),
            f"{path}.coordinates",
            dimensions,
        )
        action["path"] = [{"x": start_x, "y": start_y}, {"x": end_x, "y": end_y}]
    elif action_name == "scroll":
        x, y = _scale_required_coordinates(
            args.get("coordinates"),
            f"{path}.coordinates",
            dimensions,
        )
        direction = args.get("direction")
        if direction not in {"up", "down"}:
            raise ValueError(f"{path}.direction must be up or down")
        amount = _as_positive_int(args.get("amount"))
        if amount is None or amount < 1 or amount > YUTORI_N2_MAX_SCROLL_AMOUNT:
            raise ValueError(
                f"{path}.amount must be an integer between 1 and {YUTORI_N2_MAX_SCROLL_AMOUNT}"
            )
        action.update({"x": x, "y": y, "direction": direction, "amount": amount})
    elif action_name == "type":
        text = args.get("text")
        if not isinstance(text, str) or not text:
            raise ValueError(f"{path}.text must be a non-empty string")
        action["text"] = text
    elif action_name in {"key_press", "hold_key"}:
        key = args.get("key")
        if not isinstance(key, str) or not key.strip():
            raise ValueError(f"{path}.key must be a non-empty string")
        action["key"] = key
    elif action_name in _OPTIONAL_COORDINATE_ACTIONS:
        if "coordinates" in args:
            x, y = _scale_required_coordinates(
                args.get("coordinates"),
                f"{path}.coordinates",
                dimensions,
            )
            action.update({"x": x, "y": y})

    if action_name in {"wait", "hold_key"}:
        duration = args.get("duration", 1)
        duration_number = _as_number(duration)
        if (
            duration_number is None
            or duration_number < 0
            or duration_number > YUTORI_N2_MAX_WAIT_SECONDS
        ):
            raise ValueError(
                f"{path}.duration must be between 0 and {YUTORI_N2_MAX_WAIT_SECONDS} seconds"
            )
        action["duration"] = duration_number

    return action


def _validate_computer_batch(
    args: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
) -> List[Dict[str, Any]]:
    args = _normalize_args(args)
    unknown_root_field = next((key for key in args if key != "actions"), None)
    if unknown_root_field:
        raise ValueError(f"computer_batch.{unknown_root_field} is not allowed")

    actions = args.get("actions")
    if not isinstance(actions, Sequence) or isinstance(actions, str):
        raise ValueError("computer_batch.actions must be an array")
    if not actions:
        raise ValueError("computer_batch.actions must not be empty")
    if len(actions) > YUTORI_N2_MAX_BATCH_ACTIONS:
        raise ValueError(
            "computer_batch.actions must contain at most "
            f"{YUTORI_N2_MAX_BATCH_ACTIONS} actions, got {len(actions)}"
        )

    validated = []
    for index, member in enumerate(actions):
        action_name, action_args = _normalize_batch_member(member, index)
        validated.append(_validate_computer_action(action_name, action_args, index, dimensions))
    return validated


def _single_computer_actions(
    name: str,
    args: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
) -> List[Dict[str, Any]]:
    action_name, action_args = _canonicalize_action_args(name, args)
    return [_validate_computer_action(action_name, action_args, 0, dimensions)]


def _key_sequences(key: str) -> List[List[str]]:
    stripped = key.strip()
    if not stripped:
        return []

    if stripped.startswith("["):
        parsed = _parse_json_value(stripped)
        if isinstance(parsed, Sequence) and not isinstance(parsed, str):
            return [[str(item) for item in parsed]]
    if "+" in stripped:
        keys = [part.strip() for part in stripped.split("+") if part.strip()]
        return [keys] if keys else []

    parts = [part.strip() for part in stripped.split() if part.strip()]
    if len(parts) > 1:
        return [[part] for part in parts]
    return [[stripped]]


def _key_target(computer_handler: Any) -> Any:
    if hasattr(computer_handler, "key_down") and hasattr(computer_handler, "key_up"):
        return computer_handler
    interface = getattr(computer_handler, "interface", None)
    if interface is not None and hasattr(interface, "key_down") and hasattr(interface, "key_up"):
        return interface
    return None


async def _hold_keys_around(
    computer_handler: Any,
    keys: Sequence[str],
    operation: Callable[[], Awaitable[None]],
) -> None:
    if not keys:
        await operation()
        return

    target = _key_target(computer_handler)
    if target is None:
        raise RuntimeError("computer handler does not support held modifier keys")

    pressed: List[str] = []
    try:
        for key in keys:
            await target.key_down(key)
            pressed.append(key)
        await operation()
    finally:
        for key in reversed(pressed):
            await target.key_up(key)


async def _execute_hold_key(computer_handler: Any, key: str, duration: float) -> None:
    target = _key_target(computer_handler)
    if target is None:
        raise RuntimeError("computer handler does not support hold_key")

    keys = _key_sequences(key)
    if len(keys) != 1:
        raise RuntimeError("hold_key requires one key or one key combination")

    pressed: List[str] = []
    try:
        for key_name in keys[0]:
            await target.key_down(key_name)
            pressed.append(key_name)
        await asyncio.sleep(duration)
    finally:
        for key_name in reversed(pressed):
            await target.key_up(key_name)


async def _execute_computer_action(
    computer_handler: Any,
    action: Mapping[str, Any],
    dimensions: Optional[Tuple[int, int]],
) -> None:
    if computer_handler is None:
        raise RuntimeError("computer handler is required for computer_batch")

    action_name = str(action["action"])
    modifier_keys = _parse_modifier_keys(action.get("modifier")) or []

    async def run_without_modifier() -> None:
        if action_name == "screenshot":
            return
        if action_name == "left_click":
            await computer_handler.click(action["x"], action["y"], button="left")
            return
        if action_name == "right_click":
            await computer_handler.click(action["x"], action["y"], button="right")
            return
        if action_name == "middle_click":
            await computer_handler.click(action["x"], action["y"], button="middle")
            return
        if action_name == "double_click":
            await computer_handler.double_click(action["x"], action["y"])
            return
        if action_name == "triple_click":
            for _ in range(3):
                await computer_handler.click(action["x"], action["y"], button="left")
            return
        if action_name == "mouse_move":
            await computer_handler.move(action["x"], action["y"])
            return
        if action_name == "drag":
            await computer_handler.drag(action["path"])
            return
        if action_name == "scroll":
            width, height = dimensions or (YUTORI_N2_COORD_SPACE, YUTORI_N2_COORD_SPACE)
            step_y = max(1, round(height * 0.1))
            scroll_y = (
                -action["amount"] * step_y
                if action["direction"] == "up"
                else action["amount"] * step_y
            )
            await computer_handler.scroll(action["x"], action["y"], scroll_x=0, scroll_y=scroll_y)
            return
        if action_name == "type":
            await computer_handler.type(action["text"])
            return
        if action_name == "key_press":
            for keys in _key_sequences(action["key"]):
                await computer_handler.keypress(keys)
            return
        if action_name == "wait":
            await computer_handler.wait(ms=round(float(action["duration"]) * 1000))
            return
        if action_name == "mouse_down":
            await computer_handler.left_mouse_down(action.get("x"), action.get("y"))
            return
        if action_name == "mouse_up":
            await computer_handler.left_mouse_up(action.get("x"), action.get("y"))
            return
        if action_name == "hold_key":
            await _execute_hold_key(computer_handler, action["key"], float(action["duration"]))
            return
        raise RuntimeError(f"unsupported computer action: {action_name}")

    await _hold_keys_around(computer_handler, modifier_keys, run_without_modifier)


async def _take_post_tool_screenshot(
    computer_handler: Any,
    on_screenshot: Optional[Callable[[str, str], Awaitable[None]]],
) -> Optional[Dict[str, Any]]:
    if computer_handler is None or not hasattr(computer_handler, "screenshot"):
        return None

    screenshot_b64 = await computer_handler.screenshot()
    if not screenshot_b64:
        return None
    if on_screenshot:
        await on_screenshot(screenshot_b64, "screenshot_after")
    return _screenshot_message(screenshot_b64)


async def _execute_computer_batch(
    actions: Sequence[Mapping[str, Any]],
    computer_handler: Any,
    dimensions: Optional[Tuple[int, int]],
    on_screenshot: Optional[Callable[[str, str], Awaitable[None]]],
) -> Tuple[str, Optional[Dict[str, Any]]]:
    completed = 0
    result_lines = []

    for index, action in enumerate(actions):
        action_name = str(action["action"])
        try:
            await _execute_computer_action(computer_handler, action, dimensions)
        except Exception as exc:
            skipped = len(actions) - index - 1
            screenshot = await _take_post_tool_screenshot(computer_handler, on_screenshot)
            return (
                f"[ERROR] computer_batch failed at member index {index}: {exc}; "
                f"completed={completed} skipped={skipped}. Completed members are not rolled back.",
                screenshot,
            )
        completed += 1
        result_lines.append(f"[{index}:{action_name}] success")

    screenshot = await _take_post_tool_screenshot(computer_handler, on_screenshot)
    return "\n".join(result_lines), screenshot


def _resolve_file_path(args: Mapping[str, Any], cwd: Path) -> Path:
    raw_path = args.get("file_path", args.get("path"))
    if not isinstance(raw_path, str) or not raw_path:
        raise ValueError("file_path must be a non-empty string")
    path = Path(raw_path).expanduser()
    if not path.is_absolute():
        path = cwd / path
    return path


async def _execute_bash(args: Mapping[str, Any], cwd: Path) -> str:
    command = args.get("command")
    if not isinstance(command, str) or not command:
        raise ValueError("command must be a non-empty string")

    timeout = _as_positive_int(args.get("timeout", YUTORI_N2_BASH_DEFAULT_TIMEOUT_SECONDS))
    if timeout is None or timeout < 1 or timeout > YUTORI_N2_BASH_MAX_TIMEOUT_SECONDS:
        raise ValueError(
            f"timeout must be an integer between 1 and {YUTORI_N2_BASH_MAX_TIMEOUT_SECONDS}"
        )

    if bool(args.get("run_in_background", False)):
        proc = await asyncio.create_subprocess_shell(
            command,
            cwd=str(cwd),
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        return f"Started background command with pid {proc.pid}."

    proc = await asyncio.create_subprocess_shell(
        command,
        cwd=str(cwd),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=timeout)
    except asyncio.TimeoutError:
        proc.kill()
        await proc.wait()
        raise TimeoutError(f"bash command timed out after {timeout} seconds")

    stdout_text = stdout.decode("utf-8", errors="replace")
    stderr_text = stderr.decode("utf-8", errors="replace")
    parts = [f"exit_code={proc.returncode}"]
    if stdout_text:
        parts.append(f"stdout:\n{stdout_text}")
    if stderr_text:
        parts.append(f"stderr:\n{stderr_text}")
    return "\n".join(parts)


async def _execute_read(args: Mapping[str, Any], cwd: Path) -> str:
    path = _resolve_file_path(args, cwd)
    offset = _as_positive_int(args.get("offset", 1))
    limit = _as_positive_int(args.get("limit", YUTORI_N2_DEFAULT_READ_LIMIT))
    if offset is None or offset < 1:
        raise ValueError("offset must be an integer greater than or equal to 1")
    if limit is None or limit < 1:
        raise ValueError("limit must be an integer greater than or equal to 1")

    text = await asyncio.to_thread(path.read_text, encoding="utf-8", errors="replace")
    lines = text.splitlines(keepends=True)
    return "".join(lines[offset - 1 : offset - 1 + limit])


async def _execute_write(args: Mapping[str, Any], cwd: Path) -> str:
    path = _resolve_file_path(args, cwd)
    content = args.get("content")
    if not isinstance(content, str):
        raise ValueError("content must be a string")
    if len(content) > YUTORI_N2_WRITE_CONTENT_MAX_CHARS:
        raise ValueError(f"content must be at most {YUTORI_N2_WRITE_CONTENT_MAX_CHARS} characters")

    await asyncio.to_thread(path.parent.mkdir, parents=True, exist_ok=True)
    await asyncio.to_thread(path.write_text, content, encoding="utf-8")
    return f"Wrote {len(content)} characters to {path}."


async def _execute_edit(args: Mapping[str, Any], cwd: Path) -> str:
    path = _resolve_file_path(args, cwd)
    old_string = args.get("old_string", args.get("old"))
    new_string = args.get("new_string", args.get("new"))
    replace_all = bool(args.get("replace_all", False))
    if not isinstance(old_string, str) or old_string == "":
        raise ValueError("old_string must be a non-empty string")
    if not isinstance(new_string, str):
        raise ValueError("new_string must be a string")

    text = await asyncio.to_thread(path.read_text, encoding="utf-8", errors="replace")
    occurrences = text.count(old_string)
    if occurrences == 0:
        raise ValueError("old_string was not found")
    if occurrences > 1 and not replace_all:
        raise ValueError("old_string appears multiple times; set replace_all to true")

    count = -1 if replace_all else 1
    updated = text.replace(old_string, new_string, count)
    await asyncio.to_thread(path.write_text, updated, encoding="utf-8")
    replaced = occurrences if replace_all else 1
    return f"Replaced {replaced} occurrence{'s' if replaced != 1 else ''} in {path}."


async def _execute_file_or_shell_tool(name: str, args: Mapping[str, Any], cwd: Path) -> str:
    if name == "bash":
        return await _execute_bash(args, cwd)
    if name == "read":
        return await _execute_read(args, cwd)
    if name == "write":
        return await _execute_write(args, cwd)
    if name == "edit":
        return await _execute_edit(args, cwd)
    raise ValueError(f"unsupported Yutori N2 tool: {name}")


async def _handle_yutori_n2_tool_call(
    tool_call: Mapping[str, Any],
    computer_handler: Any,
    dimensions: Optional[Tuple[int, int]],
    cwd: Path,
    on_screenshot: Optional[Callable[[str, str], Awaitable[None]]],
) -> List[Dict[str, Any]]:
    name = str(tool_call.get("name") or "")
    args = _normalize_args(tool_call.get("arguments") or {})
    call_id = tool_call.get("_call_id")
    if not isinstance(call_id, str):
        call_id = None

    if name == "computer_batch":
        try:
            actions = _validate_computer_batch(args, dimensions)
        except ValueError as exc:
            return _function_call_with_output(
                name,
                args,
                f"[ERROR] Batch validation failed: {exc}; completed=0 failed_index=none skipped=0.",
                call_id,
            )
        result_text, screenshot_message = await _execute_computer_batch(
            actions,
            computer_handler,
            dimensions,
            on_screenshot,
        )
        output = _function_call_with_output(name, args, result_text, call_id)
        if screenshot_message:
            output.append(screenshot_message)
        return output

    if name in _FUNCTION_TOOLS:
        try:
            result_text = await _execute_file_or_shell_tool(name, args, cwd)
        except Exception as exc:
            result_text = f"[ERROR] {name} failed: {exc}"
        return _function_call_with_output(name, args, result_text, call_id)

    if name in {"computer", "computer_use", *_COMPUTER_FUNCTIONS}:
        try:
            actions = _single_computer_actions(name, args, dimensions)
        except ValueError as exc:
            return _function_call_with_output(
                name,
                args,
                f"[ERROR] Computer action validation failed: {exc}",
                call_id,
            )
        result_text, screenshot_message = await _execute_computer_batch(
            actions,
            computer_handler,
            dimensions,
            on_screenshot,
        )
        output = _function_call_with_output(name, args, result_text, call_id)
        if screenshot_message:
            output.append(screenshot_message)
        return output

    return [make_function_call_item(name, args, call_id=call_id)]


@register_agent(models=r"^yutori/.*(?:n2|n2os).*", priority=10)
class YutoriN2Config(AsyncAgentConfig):
    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler=None,
        use_prompt_caching: Optional[bool] = False,
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **kwargs,
    ) -> Dict[str, Any]:
        generation_kwargs = dict(kwargs)
        cwd = Path(str(generation_kwargs.pop("n2_cwd", os.getcwd()))).expanduser()
        tool_set = generation_kwargs.pop("tool_set", YUTORI_N2_TOOL_SET)
        api_base = generation_kwargs.get("api_base")

        converted_messages = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=False,
        )
        completion_messages: List[Dict[str, Any]] = [
            {"role": "system", "content": _YUTORI_N2_SYSTEM_PROMPT},
            *converted_messages,
        ]

        pre_output_items = await _ensure_initial_screenshot(
            completion_messages,
            messages,
            computer_handler,
            _on_screenshot,
        )
        image_dimensions = _add_image_resize_hints(completion_messages)
        dimensions = await _get_computer_dimensions(computer_handler) or image_dimensions
        model_tools = _extra_function_tools(tools)

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "tool_choice": "auto",
            "parallel_tool_calls": True,
            "max_retries": max_retries,
            "stream": stream,
            **generation_kwargs,
        }
        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = use_prompt_caching

        if _uses_yutori_api(api_base):
            api_kwargs["tool_set"] = tool_set
            if model_tools:
                api_kwargs["tools"] = model_tools
        else:
            api_kwargs["tools"] = [*YUTORI_N2_TOOLS, *model_tools]

        if _on_api_start:
            await _on_api_start(api_kwargs)

        response = await litellm.acompletion(**api_kwargs)

        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        usage = {
            **LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(  # type: ignore
                response.usage
            ).model_dump(),
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(usage)

        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        content_text = _completion_content_to_text(message.get("content"))
        tool_calls_array = message.get("tool_calls") or []
        reasoning_text = message.get("reasoning") or ""

        output_items: List[Dict[str, Any]] = []
        if reasoning_text:
            output_items.append(make_reasoning_item(reasoning_text))

        structured_tool_calls = _completion_tool_calls_to_yutori_tool_calls(tool_calls_array)
        text_tool_calls = (
            [] if structured_tool_calls else parse_yutori_n2_tool_calls_from_text(content_text)
        )
        assistant_text = (
            strip_parsed_yutori_n2_tool_calls_from_text(content_text, text_tool_calls)
            if text_tool_calls
            else content_text
        )

        if assistant_text:
            output_items.extend(
                convert_completion_messages_to_responses_items(
                    [{"role": "assistant", "content": assistant_text}]
                )
            )

        yutori_tool_calls = structured_tool_calls or text_tool_calls
        for tool_call in yutori_tool_calls:
            output_items.extend(
                await _handle_yutori_n2_tool_call(
                    tool_call,
                    computer_handler,
                    dimensions,
                    cwd,
                    _on_screenshot,
                )
            )

        if text_tool_calls and not output_items:
            output_items.extend(
                convert_completion_messages_to_responses_items(
                    [{"role": "assistant", "content": YUTORI_N2_SKIPPED_TOOL_CALL_MESSAGE}]
                )
            )
        elif not yutori_tool_calls and not output_items:
            output_items.extend(
                convert_completion_messages_to_responses_items(
                    [{"role": "assistant", "content": content_text}]
                )
            )

        return {"output": pre_output_items + output_items, "usage": usage}

    async def predict_click(
        self,
        model: str,
        image_b64: str,
        instruction: str,
        **generation_config,
    ) -> Optional[Tuple[int, int]]:
        return None

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]
