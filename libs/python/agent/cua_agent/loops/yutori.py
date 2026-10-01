"""
Yutori Navigator (n1.5) agent loop implementation using litellm.

Navigator is a browser-use model that outputs actions via tool_calls in OpenAI
chat completions format. Coordinates are in a 1000x1000 normalized space, and
screenshots are best sent as 1280x800 WebP images.

The default model is ``yutori/n1.5-latest``; dated snapshots such as
``yutori/n1.5-20260428`` are also supported. (Navigator n1 is deprecated and
rejected by the Yutori API.)

Notes (see https://docs.yutori.com reference/n1-5):
- key_press takes ``key`` in a lowercase key space, where "+" joins
  simultaneous keys and spaces separate sequential presses
  (e.g. "ctrl+c", "down down enter").
- Click/scroll actions accept optional ``ref`` (DOM element reference) and
  ``modifier`` (held modifier key) arguments.
- Optional request params tool_set, disable_tools, json_schema and
  prev_request_id are forwarded via extra_body, e.g.::

      agent = ComputerAgent(
          model="yutori/n1.5-latest",
          tools=[computer],
          tool_set="browser_tools_expanded-20260403",
      )

- The expanded tool set adds DOM tools (extract_elements, find,
  set_element_value, execute_js). This loop executes them inline through the
  computer handler's evaluate_js method, which requires a computer-server
  recent enough to support the ``evaluate`` browser command.

The Yutori API injects the browser tool definitions and the system prompt
server-side, so this loop only forwards custom function tools.
"""

from __future__ import annotations

import base64
import io
import json
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)
from PIL import Image

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
    make_function_call_item,
    make_output_text_item,
    make_reasoning_item,
)
from ..types import AgentCapability
from .yutori_js import (
    EXECUTE_JS_SCRIPT,
    EXTRACT_ELEMENTS_SCRIPT,
    FIND_SCRIPT,
    GET_ELEMENT_BY_REF_SCRIPT,
    SET_ELEMENT_VALUE_SCRIPT,
    build_tool_expression,
    coerce_result,
)

# Target resolution for Navigator models (docs recommend 1280x800 WebP)
NAVIGATOR_TARGET_WIDTH = 1280
NAVIGATOR_TARGET_HEIGHT = 800
NAVIGATOR_COORD_SPACE = 1000

# DOM tools from the expanded tool set, executed inline by this loop
DOM_TOOL_NAMES = {"extract_elements", "find", "set_element_value", "execute_js"}

# Yutori-specific request params forwarded to the API via extra_body
YUTORI_EXTRA_BODY_PARAMS = ("tool_set", "disable_tools", "json_schema", "prev_request_id")

# Map the Navigator key space to the pynput-style names understood by the
# computer-server keyboard handlers, plus a few tolerant synonyms.
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
}


def _map_key_token(token: str) -> str:
    """Map a single Navigator key name to a pynput-style key name."""
    token = token.strip()
    if len(token) == 1:
        return token
    lowered = token.lower()
    return _KEY_ALIASES.get(lowered, lowered)


def _convert_key_expression(expr: str) -> List[List[str]]:
    """Convert a Navigator key expression to a list of key combos.

    "+" joins keys pressed simultaneously; whitespace separates sequential
    presses. E.g. "down down enter" -> [["down"], ["down"], ["enter"]] and
    "ctrl+shift+t" -> [["ctrl", "shift", "t"]].
    """
    combos = []
    for combo in expr.split():
        keys = [_map_key_token(k) for k in combo.split("+") if k.strip()]
        if keys:
            combos.append(keys)
    return combos


def _prepare_image_for_navigator(image_b64: str) -> str:
    """Convert a base64 PNG screenshot to WebP at 1280x800 for optimal performance."""
    try:
        img_bytes = base64.b64decode(image_b64)
        img = Image.open(io.BytesIO(img_bytes))

        # Resize to the recommended resolution
        if img.size != (NAVIGATOR_TARGET_WIDTH, NAVIGATOR_TARGET_HEIGHT):
            img = img.resize((NAVIGATOR_TARGET_WIDTH, NAVIGATOR_TARGET_HEIGHT), Image.LANCZOS)

        # Convert to WebP
        buf = io.BytesIO()
        img.save(buf, format="WEBP", quality=85)
        return base64.b64encode(buf.getvalue()).decode("utf-8")
    except Exception:
        # Fallback: return original image if conversion fails
        return image_b64


def _unnormalize_coordinates(
    coords: List[int], screen_width: int, screen_height: int
) -> Tuple[int, int]:
    """Scale coordinates from the 1000x1000 normalized space to actual screen pixels."""
    x = max(0, min(screen_width, round((coords[0] / NAVIGATOR_COORD_SPACE) * screen_width)))
    y = max(0, min(screen_height, round((coords[1] / NAVIGATOR_COORD_SPACE) * screen_height)))
    return x, y


def _convert_navigator_action(
    fn_name: str,
    args: Dict[str, Any],
    screen_width: int,
    screen_height: int,
    ref_pixels: Optional[Tuple[int, int]] = None,
) -> Optional[List[Dict[str, Any]]]:
    """
    Convert a Navigator tool call to a list of internal computer_call actions.

    ref_pixels, when provided, are viewport pixel coordinates already resolved
    from the action's ``ref`` argument and take precedence over the normalized
    ``coordinates``.

    Returns None for tool calls that are not computer actions (custom function
    tools), so the caller can emit them as function_calls instead.
    """
    x, y = None, None
    if ref_pixels is not None:
        x, y = ref_pixels
    else:
        coords = args.get("coordinates")
        if isinstance(coords, (list, tuple)) and len(coords) >= 2:
            x, y = _unnormalize_coordinates(coords, screen_width, screen_height)

    modifier = args.get("modifier")
    modifier = _map_key_token(modifier) if isinstance(modifier, str) and modifier else None

    def _with_modifier(action: Dict[str, Any]) -> Dict[str, Any]:
        if modifier:
            action["modifier"] = modifier
        return action

    if fn_name in ("left_click", "right_click", "middle_click"):
        if x is None or y is None:
            return None
        button = fn_name.split("_")[0]
        return [_with_modifier({"action": "click", "button": button, "x": x, "y": y})]

    if fn_name == "double_click":
        if x is None or y is None:
            return None
        return [_with_modifier({"action": "double_click", "x": x, "y": y})]

    if fn_name == "triple_click":
        if x is None or y is None:
            return None
        return [_with_modifier({"action": "triple_click", "x": x, "y": y})]

    if fn_name == "mouse_move":
        if x is None or y is None:
            return None
        return [{"action": "move", "x": x, "y": y}]

    if fn_name in ("mouse_down", "mouse_up"):
        action: Dict[str, Any] = {"action": fn_name}
        if x is not None and y is not None:
            action["x"] = x
            action["y"] = y
        return [action]

    if fn_name == "drag":
        start_coords = args.get("start_coordinates")
        if (
            not isinstance(start_coords, (list, tuple))
            or len(start_coords) < 2
            or x is None
            or y is None
        ):
            return None
        sx, sy = _unnormalize_coordinates(start_coords, screen_width, screen_height)
        return [
            {
                "action": "left_click_drag",
                "start_coordinate": [sx, sy],
                "end_coordinate": [x, y],
            }
        ]

    if fn_name == "scroll":
        direction = args.get("direction", "down")
        amount = int(args.get("amount", 3))
        # Convert direction + amount to scroll_x/scroll_y pixels
        # Use ~100 pixels per scroll unit as a reasonable default
        pixels_per_unit = 100
        scroll_x, scroll_y = 0, 0
        if direction == "down":
            scroll_y = amount * pixels_per_unit
        elif direction == "up":
            scroll_y = -(amount * pixels_per_unit)
        elif direction == "right":
            scroll_x = amount * pixels_per_unit
        elif direction == "left":
            scroll_x = -(amount * pixels_per_unit)
        out: Dict[str, Any] = {"action": "scroll", "scroll_x": scroll_x, "scroll_y": scroll_y}
        if x is not None and y is not None:
            out["x"] = x
            out["y"] = y
        return [out]

    if fn_name == "type":
        return [{"action": "type", "text": args.get("text", "")}]

    if fn_name == "key_press":
        combos = _convert_key_expression(args.get("key") or "")
        if not combos:
            return None
        return [{"action": "keypress", "keys": keys} for keys in combos]

    if fn_name == "hold_key":
        key = args.get("key", "")
        if not key:
            return None
        action = {"action": "hold_key", "key": _map_key_token(key)}
        if args.get("duration") is not None:
            action["duration"] = float(args["duration"])
        return [action]

    if fn_name == "wait":
        action = {"action": "wait"}
        if args.get("duration") is not None:
            action["time"] = float(args["duration"])
        return [action]

    if fn_name == "go_back":
        return [{"action": "history_back"}]

    if fn_name == "go_forward":
        return [{"action": "history_forward"}]

    if fn_name == "refresh":
        return [{"action": "refresh"}]

    if fn_name == "goto_url":
        return [{"action": "visit_url", "url": args.get("url", "")}]

    return None


def _convert_images_to_navigator_format(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Convert all images in messages to WebP format optimized for Navigator models."""
    for msg in messages:
        content = msg.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if isinstance(part, dict) and part.get("type") == "image_url":
                url = ((part.get("image_url") or {}).get("url")) or ""
                if url.startswith("data:") and "," in url:
                    b64 = url.split(",", 1)[1]
                    converted = _prepare_image_for_navigator(b64)
                    part["image_url"]["url"] = f"data:image/webp;base64,{converted}"
    return messages


def _message_has_image(msg: Dict[str, Any]) -> bool:
    content = msg.get("content")
    if isinstance(content, list):
        for part in content:
            if isinstance(part, dict) and part.get("type") == "image_url":
                return True
    return False


async def _evaluate_tool_script(computer_handler, script: str, *args: Any) -> Dict[str, Any]:
    """Run a bundled JS tool script in the page via the computer handler.

    Raises RuntimeError when the handler or the computer-server cannot
    evaluate JS (e.g. servers predating the 'evaluate' browser command).
    """
    if not hasattr(computer_handler, "evaluate_js"):
        raise RuntimeError(
            "This computer handler does not support JavaScript evaluation "
            "(expanded browser tools require a BrowserTool handler)."
        )
    result = await computer_handler.evaluate_js(build_tool_expression(script, *args))
    if not isinstance(result, dict) or not result.get("success", False):
        error = result.get("error") if isinstance(result, dict) else str(result)
        raise RuntimeError(error or "JavaScript evaluation failed.")
    return coerce_result(result.get("result"))


async def _resolve_ref(computer_handler, ref: str) -> Optional[Tuple[int, int]]:
    """Resolve a DOM element ref to viewport pixel coordinates.

    Also scrolls the element into view. Returns None when resolution fails so
    the caller can fall back to the action's normalized coordinates.
    """
    try:
        result = await _evaluate_tool_script(computer_handler, GET_ELEMENT_BY_REF_SCRIPT, ref)
    except Exception:
        return None
    if not result.get("success"):
        return None
    px = result.get("coordinates")
    if not isinstance(px, (list, tuple)) or len(px) < 2:
        return None
    return int(px[0]), int(px[1])


async def _execute_dom_tool(computer_handler, fn_name: str, args: Dict[str, Any]) -> str:
    """Execute an expanded-tool-set DOM tool inline and format its result."""
    try:
        if fn_name == "extract_elements":
            data = await _evaluate_tool_script(
                computer_handler, EXTRACT_ELEMENTS_SCRIPT, args.get("filter", "visible")
            )
            result = data.get("pageContent", "")
        elif fn_name == "find":
            text = args.get("text", "")
            data = await _evaluate_tool_script(computer_handler, FIND_SCRIPT, text)
            if not data.get("success", False):
                result = f"[ERROR] {data.get('message', 'find failed')}"
            else:
                matches = data.get("matches", [])
                total_matches = int(data.get("totalMatches", len(matches)))
                if total_matches:
                    result = f'Found {total_matches} element(s) matching "{text}":\n' + "\n".join(
                        matches[:20]
                    )
                else:
                    result = f'No elements matching "{text}" found on the page.'
        elif fn_name == "set_element_value":
            data = await _evaluate_tool_script(
                computer_handler,
                SET_ELEMENT_VALUE_SCRIPT,
                args.get("ref", ""),
                args.get("value", ""),
            )
            result = data.get("message", "set_element_value completed")
        elif fn_name == "execute_js":
            data = await _evaluate_tool_script(
                computer_handler, EXECUTE_JS_SCRIPT, args.get("text", "")
            )
            if not data.get("success", False):
                result = f"[ERROR] {data.get('message', 'execute_js failed')}"
            elif not data.get("hasResult"):
                result = "undefined"
            else:
                result = str(data.get("result"))
        else:
            result = f"[ERROR] Unknown DOM tool: {fn_name}"
    except Exception as e:
        result = f"[ERROR] Error executing {fn_name}: {e}"

    # Match the reference client: append the current URL to tool results
    try:
        current_url = await computer_handler.get_current_url()
        if current_url:
            result = f"{result}\nCurrent URL: {current_url}"
    except Exception:
        pass

    return result


@register_agent(models=r"(yutori/)?n1\.\d+(-.*)?$", tool_type="browser")
class YutoriNavigatorConfig(AsyncAgentConfig):
    """
    Yutori Navigator (n1.5) browser-use agent loop.

    Navigator is a browser-only model that outputs actions as tool_calls.
    Coordinates use a 1000x1000 normalized space. Browser tool definitions and
    the system prompt are injected server-side by the Yutori API.
    """

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
        """Predict the next browser action using a Yutori Navigator model."""
        tools = tools or []

        # Get screen dimensions for coordinate denormalization
        screen_width, screen_height = NAVIGATOR_TARGET_WIDTH, NAVIGATOR_TARGET_HEIGHT
        if computer_handler:
            try:
                screen_width, screen_height = await computer_handler.get_dimensions()
            except Exception:
                # BrowserTool doesn't have get_dimensions() but has viewport attrs
                vw = getattr(computer_handler, "viewport_width", None)
                vh = getattr(computer_handler, "viewport_height", None)
                if vw and vh:
                    screen_width, screen_height = vw, vh

        # Convert messages from Responses API format to chat completions format
        completion_messages = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=True,
        )

        # Convert images to WebP at 1280x800
        completion_messages = _convert_images_to_navigator_format(completion_messages)

        # Navigator predicts actions from the current screen. If the latest
        # message carries no screenshot (start of a task, a new user
        # instruction, or a text-only DOM tool result), take one and inject it
        # for this request only.
        if not completion_messages or not _message_has_image(completion_messages[-1]):
            if computer_handler is None or not hasattr(computer_handler, "screenshot"):
                raise RuntimeError(
                    "No current screenshot and computer_handler.screenshot is not available."
                )
            screenshot_b64 = await computer_handler.screenshot()
            if not screenshot_b64:
                raise RuntimeError("Failed to capture screenshot from computer_handler.")
            if _on_screenshot:
                await _on_screenshot(screenshot_b64, "screenshot_before")

            converted = _prepare_image_for_navigator(screenshot_b64)
            completion_messages.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:image/webp;base64,{converted}"},
                        },
                        {"type": "text", "text": "Current browser screen"},
                    ],
                }
            )

        # Build tool list: pass through any custom function tools.
        # Browser tools are injected server-side by the Yutori API.
        custom_tools = []
        custom_tool_names = set()
        for tool in tools:
            if tool.get("type") == "function":
                func = tool.get("function")
                if func:
                    custom_tools.append({"type": "function", "function": func})
                    if func.get("name"):
                        custom_tool_names.add(func["name"])

        # Collect Yutori-specific request params into extra_body
        extra_body = dict(kwargs.pop("extra_body", None) or {})
        for key in YUTORI_EXTRA_BODY_PARAMS:
            value = kwargs.pop(key, None)
            if value is not None:
                extra_body[key] = value

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "max_retries": max_retries,
            "stream": False,  # Navigator models do not support streaming
            "temperature": kwargs.pop("temperature", 0.3),
        }

        if custom_tools:
            api_kwargs["tools"] = custom_tools
        if extra_body:
            api_kwargs["extra_body"] = extra_body

        # Pass through remaining kwargs (api_key, api_base, etc.)
        api_kwargs.update({k: v for k, v in kwargs.items()})

        if _on_api_start:
            await _on_api_start(api_kwargs)

        response = await litellm.acompletion(**api_kwargs)

        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # Extract usage
        usage = {
            **LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(  # type: ignore
                response.usage
            ).model_dump(),
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(usage)

        # Parse response
        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        content_text = message.get("content") or ""
        tool_calls_array = message.get("tool_calls") or []
        reasoning_text = message.get("reasoning_content") or message.get("reasoning") or ""

        output_items: List[Dict[str, Any]] = []

        # Add reasoning if present
        if reasoning_text:
            output_items.append(make_reasoning_item(reasoning_text))

        if tool_calls_array:
            for tc in tool_calls_array:
                function = tc.get("function", {})
                fn_name = function.get("name", "")
                args_str = function.get("arguments", "{}")
                tc_id = tc.get("id", "call_0")

                try:
                    args = json.loads(args_str) if isinstance(args_str, str) else args_str
                except json.JSONDecodeError:
                    args = {}

                # Expanded tool set DOM tools are executed inline; emitting the
                # call and its output together makes the agent skip re-execution.
                # A user-provided function tool with the same name wins.
                if fn_name in DOM_TOOL_NAMES and fn_name not in custom_tool_names:
                    dom_result = await _execute_dom_tool(computer_handler, fn_name, args)
                    output_items.append(make_function_call_item(fn_name, args, call_id=tc_id))
                    output_items.append(
                        {
                            "type": "function_call_output",
                            "call_id": tc_id,
                            "output": dom_result,
                        }
                    )
                    continue

                # Resolve a DOM element ref to pixel coordinates when present;
                # fall back to the action's normalized coordinates.
                ref_pixels = None
                ref = args.get("ref")
                if ref and computer_handler is not None:
                    ref_pixels = await _resolve_ref(computer_handler, ref)

                computer_actions = _convert_navigator_action(
                    fn_name, args, screen_width, screen_height, ref_pixels=ref_pixels
                )

                if computer_actions:
                    for i, computer_action in enumerate(computer_actions):
                        # A single Navigator tool call (e.g. the key sequence
                        # "down down enter") may expand into several actions;
                        # each needs a unique call_id.
                        call_id = tc_id if i == 0 else f"{tc_id}-{i}"
                        # Build a fake completion message for the converter
                        fake_cm = {
                            "role": "assistant",
                            "content": content_text or "",
                            "tool_calls": [
                                {
                                    "type": "function",
                                    "id": call_id,
                                    "function": {
                                        "name": "computer",
                                        "arguments": json.dumps(computer_action),
                                    },
                                }
                            ],
                        }
                        output_items.extend(
                            convert_completion_messages_to_responses_items([fake_cm])
                        )
                        # Only use content_text once
                        content_text = ""
                else:
                    # Custom tool — emit as function_call
                    output_items.append(make_function_call_item(fn_name, args, call_id=tc_id))
        else:
            # No tool calls — task is complete
            if content_text:
                output_items.append(make_output_text_item(content_text))
            else:
                output_items.append(make_output_text_item("Task completed."))

        return {"output": output_items, "usage": usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        raise NotImplementedError(
            "Yutori Navigator models do not support standalone click prediction. "
            "Use predict_step for full browser automation."
        )

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]
