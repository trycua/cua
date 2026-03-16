"""
Moonshot Kimi K2.5 agent loop implementation using litellm with function/tool calling.

Kimi K2.5 is an OpenAI-compatible VLM with strong visual grounding and computer use
capabilities. It uses the standard `computer` function tool schema with 0..1000
normalized coordinates, and returns actions via native OpenAI function calling.

No extra dependencies required beyond litellm and PIL.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
    make_output_text_item,
    make_reasoning_item,
)
from ..types import AgentCapability

# ComputerUse tool schema (OpenAI function tool format) — same actions as generic_vlm
# but passed via native function calling instead of embedded in system prompt
COMPUTER_TOOL: Dict[str, Any] = {
    "type": "function",
    "function": {
        "name": "computer",
        "description": (
            "Use a mouse and keyboard to interact with a computer, and take screenshots.\n"
            "* This is an interface to a desktop GUI. You do not have access to a terminal or applications menu. "
            "You must click on desktop icons to start applications.\n"
            "* Some applications may take time to start or process actions, so you may need to wait and take "
            "successive screenshots to see the results of your actions.\n"
            "* The screen's resolution is 1000x1000.\n"
            "* Whenever you intend to move the cursor to click on an element like an icon, you should consult "
            "a screenshot to determine the coordinates of the element before moving the cursor.\n"
            "* If you tried clicking on a program or link but it failed to load, even after waiting, try "
            "adjusting your cursor position so that the tip of the cursor visually falls on the element.\n"
            "* Make sure to click any buttons, links, icons, etc with the cursor tip in the center of the "
            "element. Don't click boxes on their edges."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "description": "The action to perform.",
                    "enum": [
                        "key",
                        "type",
                        "mouse_move",
                        "left_click",
                        "left_click_drag",
                        "right_click",
                        "middle_click",
                        "double_click",
                        "triple_click",
                        "scroll",
                        "hscroll",
                        "screenshot",
                        "wait",
                    ],
                    "type": "string",
                },
                "keys": {
                    "description": "Required only by action=key.",
                    "type": "array",
                    "items": {"type": "string"},
                },
                "text": {
                    "description": "Required only by action=type.",
                    "type": "string",
                },
                "coordinate": {
                    "description": "(x, y): Pixel coordinates from top-left in 0..1000 space.",
                    "type": "array",
                    "items": {"type": ["number", "integer"]},
                    "minItems": 2,
                    "maxItems": 2,
                },
                "pixels": {
                    "description": "Scroll amount. Positive=up, negative=down. For scroll/hscroll.",
                    "type": "number",
                },
                "time": {
                    "description": "Seconds to wait (action=wait).",
                    "type": "number",
                },
            },
            "required": ["action"],
        },
    },
}


async def _unnormalize_coordinate(
    args: Dict[str, Any], dims: Tuple[int, int]
) -> Dict[str, Any]:
    """Scale coordinates from 0..1000 space to actual screen size."""
    coord = args.get("coordinate")
    if not coord or not isinstance(coord, (list, tuple)) or len(coord) < 2:
        return args
    x, y = float(coord[0]), float(coord[1])
    width, height = float(dims[0]), float(dims[1])
    x_abs = max(0.0, min(width, (x / 1000.0) * width))
    y_abs = max(0.0, min(height, (y / 1000.0) * height))
    return {**args, "coordinate": [round(x_abs), round(y_abs)]}


def _convert_tool_args_to_computer_action(args: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """
    Convert computer tool arguments to the internal Computer Calls action schema.

    Input (from model):  {"action": "left_click", "coordinate": [114, 68]}
    Output (internal):   {"action": "left_click", "x": 114, "y": 68}
    """
    if not isinstance(args, dict):
        return None

    action = args.get("action")
    if not isinstance(action, str):
        return None

    coord = args.get("coordinate")
    x = y = None
    if isinstance(coord, (list, tuple)) and len(coord) >= 2:
        try:
            x = int(round(float(coord[0])))
            y = int(round(float(coord[1])))
        except Exception:
            x = y = None

    a = action.lower()

    if a in {"left_click", "right_click", "middle_click", "double_click"}:
        if x is None or y is None:
            return None
        return {"action": a, "x": x, "y": y}

    if a == "triple_click":
        if x is None or y is None:
            return None
        return {"action": "double_click", "x": x, "y": y}

    if a == "mouse_move":
        if x is None or y is None:
            return None
        return {"action": "move", "x": x, "y": y}

    if a == "left_click_drag":
        if x is None or y is None:
            return None
        # Drag from current position to coordinate
        return {"action": "drag", "end_x": x, "end_y": y}

    if a == "key":
        keys = args.get("keys")
        if isinstance(keys, list) and all(isinstance(k, str) for k in keys):
            return {"action": "keypress", "keys": keys}
        return None

    if a == "type":
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


@register_agent(models=r"(?i)(moonshot/)?(kimi[_-]?k2\.?5)(-.*)?$", priority=10)
class MoonshotConfig(AsyncAgentConfig):
    """
    Moonshot Kimi K2.5 computer use agent loop.

    Uses the standard `computer` function tool with 0..1000 normalized coordinates.
    Actions are returned via native OpenAI function calling.
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
        tools = tools or []

        # Get screen dimensions for coordinate denormalization
        screen_width, screen_height = 1280, 800
        if computer_handler:
            try:
                dims = await computer_handler.get_dimensions()
                screen_width, screen_height = dims
            except Exception:
                vw = getattr(computer_handler, "viewport_width", None)
                vh = getattr(computer_handler, "viewport_height", None)
                if vw and vh:
                    screen_width, screen_height = vw, vh

        # Convert messages from Responses API format to chat completions format
        completion_messages = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=False,
        )

        # If there's no screenshot, take one and inject it
        def _has_any_image(msgs: List[Dict[str, Any]]) -> bool:
            for m in msgs:
                content = m.get("content")
                if isinstance(content, list):
                    for p in content:
                        if isinstance(p, dict) and p.get("type") == "image_url":
                            return True
            return False

        pre_output_items: List[Dict[str, Any]] = []
        if not _has_any_image(completion_messages):
            if computer_handler is None or not hasattr(computer_handler, "screenshot"):
                raise RuntimeError(
                    "No screenshots present and computer_handler.screenshot is not available."
                )
            screenshot_b64 = await computer_handler.screenshot()
            if not screenshot_b64:
                raise RuntimeError("Failed to capture screenshot from computer_handler.")
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

        # Build tool list: computer tool + any custom function tools
        api_tools = [COMPUTER_TOOL]
        for tool in tools:
            if tool.get("type") == "function":
                func = tool.get("function")
                if func:
                    api_tools.append({"type": "function", "function": func})

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "tools": api_tools,
            "max_retries": max_retries,
            "stream": False,
            **{k: v for k, v in kwargs.items()},
        }
        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = use_prompt_caching

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

        # Parse response
        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        content_text = message.get("content") or ""
        tool_calls_array = message.get("tool_calls") or []
        reasoning_text = message.get("reasoning") or ""

        output_items: List[Dict[str, Any]] = []

        if reasoning_text:
            output_items.append(make_reasoning_item(reasoning_text))

        if tool_calls_array:
            processed_tool_calls = []
            for tc in tool_calls_array:
                function = tc.get("function", {})
                fn_name = function.get("name", "computer")
                args_str = function.get("arguments", "{}")

                try:
                    args = json.loads(args_str)

                    # Unnormalize coordinates if present
                    if "coordinate" in args:
                        args = await _unnormalize_coordinate(
                            args, (screen_width, screen_height)
                        )

                    # Convert to Computer Calls format if this is the computer tool
                    if fn_name == "computer":
                        converted = _convert_tool_args_to_computer_action(args)
                        if converted:
                            args = converted

                    processed_tool_calls.append(
                        {
                            "type": tc.get("type", "function"),
                            "id": tc.get("id", "call_0"),
                            "function": {
                                "name": fn_name,
                                "arguments": json.dumps(args),
                            },
                        }
                    )
                except json.JSONDecodeError:
                    processed_tool_calls.append(tc)

            fake_cm = {
                "role": "assistant",
                "content": content_text or "",
                "tool_calls": processed_tool_calls,
            }
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))
        else:
            # No tool calls — return text response
            if content_text:
                output_items.append(make_output_text_item(content_text))
            else:
                output_items.append(make_output_text_item("Task completed."))

        return {"output": (pre_output_items + output_items), "usage": usage}

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using Kimi K2.5 via litellm.acompletion.

        Uses a reduced tool schema with only left_click to bias the model
        toward outputting a single click. Returns (x, y) absolute pixels.
        """
        reduced_tool = {
            "type": "function",
            "function": {
                **COMPUTER_TOOL["function"],
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["left_click"]},
                        "coordinate": {
                            "description": "(x, y) in 0..1000 reference space",
                            "type": "array",
                            "items": {"type": ["number", "integer"]},
                            "minItems": 2,
                            "maxItems": 2,
                        },
                    },
                    "required": ["action", "coordinate"],
                },
            },
        }

        messages = [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{image_b64}"},
                    },
                    {"type": "text", "text": instruction},
                ],
            }
        ]

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": messages,
            "tools": [reduced_tool],
            "stream": False,
            **{k: v for k, v in kwargs.items()},
        }
        response = await litellm.acompletion(**api_kwargs)
        resp = response.model_dump()  # type: ignore
        choice = (resp.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        tool_calls = message.get("tool_calls") or []

        # Try to extract coordinate from tool call
        for tc in tool_calls:
            fn = tc.get("function", {})
            try:
                args = json.loads(fn.get("arguments", "{}"))
            except json.JSONDecodeError:
                continue
            coord = args.get("coordinate")
            if isinstance(coord, (list, tuple)) and len(coord) >= 2:
                # Denormalize from 0..1000 — caller doesn't provide screen dims,
                # so return normalized coordinates as integers (0..1000)
                return int(round(float(coord[0]))), int(round(float(coord[1])))

        return None
