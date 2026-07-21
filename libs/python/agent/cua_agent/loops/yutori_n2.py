"""Yutori Navigator n2-preview computer-use loop.

The Yutori API injects its versioned computer-use tools server-side. This loop
keeps the public Chat Completions tool-call/result shape intact while attaching
validated, executor-only actions to each internal function-call item.
"""

from __future__ import annotations

import base64
import copy
import io
import json
import time
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)
from PIL import Image

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_responses_items_to_completion_messages,
    make_function_call_item,
    make_output_text_item,
    make_reasoning_item,
)
from ..types import AgentCapability

N2_COORDINATE_SCALE = 1000
N2_MODEL_IMAGE_MAX_WIDTH = 1280
N2_MODEL_IMAGE_MAX_HEIGHT = 800
N2_MODEL_IMAGE_QUALITY = 75
N2_MAX_BATCH_ACTIONS = 20
N2_MAX_WAIT_SECONDS = 10

MAX_REQUEST_BODY_BYTES = 10_000_000
REQUEST_ENVELOPE_ALLOWANCE_BYTES = 500_000
DEFAULT_MAX_MESSAGES_BYTES = MAX_REQUEST_BODY_BYTES - REQUEST_ENVELOPE_ALLOWANCE_BYTES

TOOL_SET_COMPUTER_USE = "computer_use_tools-20260708"
TOOL_SET_COMPUTER_USE_BATCH = "computer_use_tools-20260716"
_SUPPORTED_TOOL_SETS = {TOOL_SET_COMPUTER_USE, TOOL_SET_COMPUTER_USE_BATCH}

_SAFE_WITHOUT_CONFIRMATION = {"screenshot", "wait", "mouse_move", "scroll"}

_PUNCTUATION_KEYS = {
    "minus": "-",
    "plus": "+",
    "equal": "=",
    "comma": ",",
    "period": ".",
    "slash": "/",
    "backslash": "\\",
    "semicolon": ";",
    "quote": "'",
    "backquote": "`",
    "bracketleft": "[",
    "bracketright": "]",
}

_KEY_ALIASES = {
    "meta": "cmd",
    "command": "cmd",
    "super": "cmd",
    "win": "cmd",
    "control": "ctrl",
    "escape": "esc",
    "pageup": "page_up",
    "pagedown": "page_down",
    "return": "enter",
    **_PUNCTUATION_KEYS,
}

_NAMED_KEYS = {
    "ctrl",
    "shift",
    "alt",
    "cmd",
    "enter",
    "esc",
    "tab",
    "space",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "page_up",
    "page_down",
    *{f"f{i}" for i in range(1, 13)},
}

_ACTION_FIELDS = {
    "left_click": {"coordinates"},
    "double_click": {"coordinates"},
    "triple_click": {"coordinates"},
    "middle_click": {"coordinates"},
    "right_click": {"coordinates"},
    "mouse_move": {"coordinates"},
    "drag": {"start_coordinates", "coordinates"},
    "scroll": {"coordinates", "direction", "amount"},
    "type": {"text"},
    "key_press": {"key"},
    "wait": {"duration"},
    "screenshot": set(),
}


class N2ActionValidationError(ValueError):
    """Raised when an n2 action cannot be safely executed."""


def _map_key_token(token: str) -> str:
    token = token.strip().lower()
    if not token:
        raise N2ActionValidationError("key expressions cannot contain empty keys")
    if len(token) == 1:
        return token
    mapped = _KEY_ALIASES.get(token, token)
    if mapped not in _NAMED_KEYS and mapped not in _PUNCTUATION_KEYS.values():
        raise N2ActionValidationError(f"unknown key name: {token}")
    return mapped


def parse_n2_key_expression(expression: str) -> List[List[str]]:
    """Parse Yutori's ``+`` combo and space-separated sequence grammar."""
    if not isinstance(expression, str) or not expression.strip():
        raise N2ActionValidationError("key must be a non-empty string")
    sequence: List[List[str]] = []
    for raw_combo in expression.split():
        raw_keys = raw_combo.split("+")
        if any(not key.strip() for key in raw_keys):
            raise N2ActionValidationError(f"invalid key combination: {raw_combo}")
        sequence.append([_map_key_token(key) for key in raw_keys])
    return sequence


def _decode_data_url(url: str) -> Tuple[bytes, str]:
    if not isinstance(url, str) or not url.startswith("data:") or "," not in url:
        raise ValueError("n2 screenshots must be base64 data URLs")
    header, encoded = url.split(",", 1)
    if ";base64" not in header:
        raise ValueError("n2 screenshots must use base64 data URLs")
    return base64.b64decode(encoded), header[5:].split(";", 1)[0]


def _image_dimensions(url: str) -> Tuple[int, int]:
    image_bytes, _ = _decode_data_url(url)
    with Image.open(io.BytesIO(image_bytes)) as image:
        return image.size


def prepare_n2_image_data_url(url: str) -> str:
    """Return a full-frame, aspect-preserving JPEG bounded by 1280x800."""
    image_bytes, _ = _decode_data_url(url)
    with Image.open(io.BytesIO(image_bytes)) as source:
        image = source.convert("RGB")
        image.thumbnail(
            (N2_MODEL_IMAGE_MAX_WIDTH, N2_MODEL_IMAGE_MAX_HEIGHT),
            Image.Resampling.LANCZOS,
        )
        output = io.BytesIO()
        image.save(output, format="JPEG", quality=N2_MODEL_IMAGE_QUALITY)
    return f"data:image/jpeg;base64,{base64.b64encode(output.getvalue()).decode('ascii')}"


def _message_image_parts(message: Dict[str, Any]) -> List[Dict[str, Any]]:
    content = message.get("content")
    if not isinstance(content, list):
        return []
    return [part for part in content if isinstance(part, dict) and part.get("type") == "image_url"]


def _latest_image_url(messages: List[Dict[str, Any]]) -> Optional[str]:
    for message in reversed(messages):
        for part in reversed(_message_image_parts(message)):
            image_url = part.get("image_url")
            if isinstance(image_url, dict) and isinstance(image_url.get("url"), str):
                return image_url["url"]
    return None


def _strip_images_from_message(message: Dict[str, Any]) -> None:
    content = message.get("content")
    if not isinstance(content, list):
        return
    message["content"] = [
        part for part in content if not (isinstance(part, dict) and part.get("type") == "image_url")
    ]


def _serialized_messages_bytes(messages: List[Dict[str, Any]]) -> int:
    return len(json.dumps(messages, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))


def retain_n2_request_images(
    messages: List[Dict[str, Any]],
    max_messages_bytes: int = DEFAULT_MAX_MESSAGES_BYTES,
) -> List[Dict[str, Any]]:
    """Return a request-local copy with images in at most two newest image messages."""
    request_messages = copy.deepcopy(messages)
    image_indices = [
        index for index, message in enumerate(request_messages) if _message_image_parts(message)
    ]
    for index in image_indices[:-2]:
        _strip_images_from_message(request_messages[index])

    if _serialized_messages_bytes(request_messages) <= max_messages_bytes:
        return request_messages

    retained_indices = image_indices[-2:]
    if len(retained_indices) == 2:
        _strip_images_from_message(request_messages[retained_indices[0]])
    if _serialized_messages_bytes(request_messages) <= max_messages_bytes:
        return request_messages

    raise ValueError(
        "The newest n2 screenshot message cannot fit within the serialized messages budget. "
        "Reduce screenshot dimensions/quality or shorten non-image request content."
    )


def _convert_request_images(messages: List[Dict[str, Any]]) -> None:
    for message in messages:
        for part in _message_image_parts(message):
            image_url = part.get("image_url")
            if not isinstance(image_url, dict) or not isinstance(image_url.get("url"), str):
                raise ValueError("n2 image_url content must contain a string url")
            image_url["url"] = prepare_n2_image_data_url(image_url["url"])


def _coordinate(value: Any, path: str) -> Tuple[int, int]:
    if (
        not isinstance(value, (list, tuple))
        or len(value) != 2
        or any(isinstance(component, bool) or not isinstance(component, int) for component in value)
        or any(component < 0 or component > N2_COORDINATE_SCALE for component in value)
    ):
        raise N2ActionValidationError(f"{path} must be two integers in the inclusive 0-1000 range")
    return int(value[0]), int(value[1])


def _native_point(value: Any, path: str, width: int, height: int) -> Tuple[int, int]:
    if width <= 0 or height <= 0:
        raise N2ActionValidationError("native screenshot dimensions must be positive")
    x, y = _coordinate(value, path)
    return (
        min(width - 1, round((x / N2_COORDINATE_SCALE) * width)),
        min(height - 1, round((y / N2_COORDINATE_SCALE) * height)),
    )


def _validate_fields(action: str, args: Dict[str, Any]) -> None:
    if action not in _ACTION_FIELDS:
        raise N2ActionValidationError(f"unsupported n2 action: {action}")
    unknown = set(args) - _ACTION_FIELDS[action]
    if unknown:
        raise N2ActionValidationError(
            f"{action} received unsupported field(s): {', '.join(sorted(unknown))}"
        )


def translate_n2_action(
    action: str,
    args: Dict[str, Any],
    native_width: int,
    native_height: int,
    *,
    batch_index: Optional[int] = None,
) -> List[Dict[str, Any]]:
    """Strictly validate and translate one Yutori action to Cua handler calls."""
    if not isinstance(args, dict):
        raise N2ActionValidationError(f"{action} arguments must be an object")
    _validate_fields(action, args)

    def internal(action_type: str, **kwargs: Any) -> Dict[str, Any]:
        result = {"type": action_type, **kwargs}
        if batch_index is not None:
            result["batch_index"] = batch_index
        return result

    if action in {
        "left_click",
        "double_click",
        "triple_click",
        "middle_click",
        "right_click",
        "mouse_move",
    }:
        x, y = _native_point(
            args.get("coordinates"), f"{action}.coordinates", native_width, native_height
        )
        if action == "left_click":
            return [internal("click", x=x, y=y, button="left")]
        if action == "right_click":
            return [internal("click", x=x, y=y, button="right")]
        if action == "middle_click":
            return [internal("click", x=x, y=y, button="middle")]
        if action == "double_click":
            return [internal("double_click", x=x, y=y)]
        if action == "triple_click":
            return [
                internal("double_click", x=x, y=y),
                internal("click", x=x, y=y, button="left"),
            ]
        return [internal("move", x=x, y=y)]

    if action == "drag":
        start_x, start_y = _native_point(
            args.get("start_coordinates"),
            "drag.start_coordinates",
            native_width,
            native_height,
        )
        end_x, end_y = _native_point(
            args.get("coordinates"), "drag.coordinates", native_width, native_height
        )
        return [
            internal(
                "drag",
                path=[{"x": start_x, "y": start_y}, {"x": end_x, "y": end_y}],
            )
        ]

    if action == "scroll":
        x, y = _native_point(
            args.get("coordinates"), "scroll.coordinates", native_width, native_height
        )
        direction = args.get("direction")
        if direction not in {"up", "down", "left", "right"}:
            raise N2ActionValidationError("scroll.direction must be up, down, left, or right")
        amount = args.get("amount")
        if isinstance(amount, bool) or not isinstance(amount, int):
            raise N2ActionValidationError("scroll.amount must be an integer")
        scroll_x = scroll_y = 0
        if direction in {"left", "right"}:
            scroll_x = round(amount * native_width * 0.1) * (1 if direction == "right" else -1)
        else:
            scroll_y = round(amount * native_height * 0.1) * (1 if direction == "down" else -1)
        return [internal("scroll", x=x, y=y, scroll_x=scroll_x, scroll_y=scroll_y)]

    if action == "type":
        text = args.get("text")
        if not isinstance(text, str):
            raise N2ActionValidationError("type.text must be a string")
        return [internal("type", text=text)]

    if action == "key_press":
        return [
            internal("keypress", keys=keys) for keys in parse_n2_key_expression(args.get("key"))
        ]

    if action == "wait":
        duration = args.get("duration", 1)
        if (
            isinstance(duration, bool)
            or not isinstance(duration, (int, float))
            or not 0 <= duration <= N2_MAX_WAIT_SECONDS
        ):
            raise N2ActionValidationError(
                f"wait.duration must be between 0 and {N2_MAX_WAIT_SECONDS} seconds"
            )
        return [internal("wait", ms=round(float(duration) * 1000))]

    if action == "screenshot":
        return [internal("screenshot")]

    raise N2ActionValidationError(f"unsupported n2 action: {action}")


def translate_n2_batch(
    args: Dict[str, Any], native_width: int, native_height: int
) -> Tuple[List[Dict[str, Any]], List[Dict[str, Any]]]:
    """Validate a complete batch before returning any executable actions."""
    if not isinstance(args, dict) or set(args) != {"actions"}:
        raise N2ActionValidationError("computer_batch requires exactly one actions field")
    raw_actions = args.get("actions")
    if not isinstance(raw_actions, list) or not 1 <= len(raw_actions) <= N2_MAX_BATCH_ACTIONS:
        raise N2ActionValidationError(
            f"computer_batch.actions must contain 1-{N2_MAX_BATCH_ACTIONS} actions"
        )

    translated: List[Dict[str, Any]] = []
    validated: List[Dict[str, Any]] = []
    for index, raw_action in enumerate(raw_actions):
        if not isinstance(raw_action, dict):
            raise N2ActionValidationError(f"computer_batch.actions[{index}] must be an object")
        action = raw_action.get("action")
        if not isinstance(action, str):
            raise N2ActionValidationError(f"computer_batch.actions[{index}].action is required")
        if action == "screenshot":
            raise N2ActionValidationError("screenshot is not allowed inside computer_batch")
        member_args = {key: value for key, value in raw_action.items() if key != "action"}
        translated.extend(
            translate_n2_action(
                action,
                member_args,
                native_width,
                native_height,
                batch_index=index,
            )
        )
        validated.append(copy.deepcopy(raw_action))
    return validated, translated


def _function_call_with_execution(
    name: str,
    args: Dict[str, Any],
    call_id: str,
    actions: List[Dict[str, Any]],
    *,
    batch_actions: Optional[List[Dict[str, Any]]] = None,
    execution_deadline: Optional[float] = None,
) -> Dict[str, Any]:
    item = dict(make_function_call_item(name, args, call_id=call_id))
    item["_computer_actions"] = actions
    item["_requires_confirmation"] = (
        any(action.get("action") not in _SAFE_WITHOUT_CONFIRMATION for action in batch_actions)
        if batch_actions is not None
        else name not in _SAFE_WITHOUT_CONFIRMATION
    )
    if batch_actions is not None:
        item["_batch_actions"] = batch_actions
    if execution_deadline is not None:
        item["_execution_deadline"] = execution_deadline
    return item


def _request_body_bytes(api_kwargs: Dict[str, Any]) -> int:
    body_fields = {
        key: value
        for key, value in api_kwargs.items()
        if key
        not in {
            "api_key",
            "api_base",
            "extra_headers",
            "headers",
            "max_retries",
        }
    }
    extra_body = body_fields.pop("extra_body", None)
    if isinstance(extra_body, dict):
        body_fields.update(extra_body)
    return len(json.dumps(body_fields, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))


@register_agent(models=r"^(yutori/)?n2-preview$", tool_type="computer", priority=10)
class YutoriN2Config(AsyncAgentConfig):
    """Yutori n2-preview desktop computer-use loop."""

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
        del tools, stream, use_prompt_caching

        completion_messages = convert_responses_items_to_completion_messages(
            copy.deepcopy(messages), allow_images_in_tool_results=True
        )

        latest_url = _latest_image_url(completion_messages)
        if latest_url is None:
            if computer_handler is None or not hasattr(computer_handler, "screenshot"):
                raise RuntimeError(
                    "No current screenshot and computer_handler.screenshot is unavailable."
                )
            screenshot_b64 = await computer_handler.screenshot()
            if not screenshot_b64:
                raise RuntimeError("Failed to capture screenshot from computer_handler.")
            if _on_screenshot:
                await _on_screenshot(screenshot_b64, "screenshot_before")
            latest_url = f"data:image/png;base64,{screenshot_b64}"
            completion_messages.append(
                {
                    "role": "user",
                    "content": [
                        {"type": "image_url", "image_url": {"url": latest_url}},
                        {"type": "text", "text": "Current desktop screen"},
                    ],
                }
            )

        native_width, native_height = _image_dimensions(latest_url)
        _convert_request_images(completion_messages)
        completion_messages = retain_n2_request_images(completion_messages)

        extra_body = dict(kwargs.pop("extra_body", None) or {})
        tool_set = kwargs.pop("tool_set", None)
        if tool_set is not None:
            if tool_set not in _SUPPORTED_TOOL_SETS:
                raise ValueError(f"Unsupported n2 tool_set: {tool_set}")
            extra_body["tool_set"] = tool_set

        execution_deadline = kwargs.pop("execution_deadline", None)
        if execution_deadline is not None and not isinstance(execution_deadline, (int, float)):
            raise ValueError("execution_deadline must be a monotonic timestamp in seconds")

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "max_retries": max_retries,
            "stream": False,
            "temperature": kwargs.pop("temperature", 0.0),
        }
        if extra_body:
            api_kwargs["extra_body"] = extra_body
        api_kwargs.update(kwargs)

        request_size = _request_body_bytes(api_kwargs)
        if request_size > MAX_REQUEST_BODY_BYTES:
            raise ValueError(
                f"Serialized n2 request is {request_size} bytes, above the "
                f"{MAX_REQUEST_BODY_BYTES}-byte request limit."
            )

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

        response_dict = response.model_dump()  # type: ignore
        message = (response_dict.get("choices") or [{}])[0].get("message") or {}
        content_text = message.get("content") or ""
        reasoning_text = message.get("reasoning_content") or message.get("reasoning") or ""
        tool_calls = message.get("tool_calls") or []
        output: List[Dict[str, Any]] = []

        if reasoning_text:
            output.append(dict(make_reasoning_item(reasoning_text)))
        if tool_calls and content_text:
            output.append(dict(make_output_text_item(content_text)))

        for tool_call in tool_calls:
            function = tool_call.get("function") or {}
            name = function.get("name") or ""
            arguments = function.get("arguments", "{}")
            call_id = tool_call.get("id") or "call_0"
            call_item: Dict[str, Any] = {
                "type": "function_call",
                "id": tool_call.get("id") or call_id,
                "call_id": call_id,
                "name": name,
                "arguments": arguments if isinstance(arguments, str) else json.dumps(arguments),
                "status": "completed",
            }
            try:
                args = json.loads(arguments) if isinstance(arguments, str) else arguments
                if not isinstance(args, dict):
                    raise N2ActionValidationError(f"{name} arguments must be an object")
                if name == "computer_batch":
                    batch_actions, translated = translate_n2_batch(
                        args, native_width, native_height
                    )
                    call_item = _function_call_with_execution(
                        name,
                        args,
                        call_id,
                        translated,
                        batch_actions=batch_actions,
                        execution_deadline=execution_deadline,
                    )
                else:
                    translated = translate_n2_action(name, args, native_width, native_height)
                    call_item = _function_call_with_execution(
                        name,
                        args,
                        call_id,
                        translated,
                        execution_deadline=execution_deadline,
                    )
                output.append(call_item)
            except (json.JSONDecodeError, N2ActionValidationError, TypeError, ValueError) as error:
                output.append(call_item)
                output.append(
                    {
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": f"[ERROR] Invalid {name} call: {error}",
                    }
                )

        if not tool_calls:
            output.append(dict(make_output_text_item(content_text or "Task completed.")))

        return {"output": output, "usage": usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        raise NotImplementedError("Yutori n2-preview supports step prediction only.")

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]
