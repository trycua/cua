"""
Yutori n1 agent loop implementation using litellm.

n1 is a browser-use model that outputs actions via tool_calls in OpenAI chat
completions format. Coordinates are in a 1000x1000 normalized space.
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

# Target resolution for n1 (docs recommend 1280x800 WebP)
N1_TARGET_WIDTH = 1280
N1_TARGET_HEIGHT = 800
N1_COORD_SPACE = 1000


def _prepare_image_for_n1(image_b64: str) -> str:
    """Convert a base64 PNG screenshot to WebP at 1280x800 for optimal n1 performance."""
    try:
        img_bytes = base64.b64decode(image_b64)
        img = Image.open(io.BytesIO(img_bytes))

        # Resize to n1's recommended resolution
        if img.size != (N1_TARGET_WIDTH, N1_TARGET_HEIGHT):
            img = img.resize((N1_TARGET_WIDTH, N1_TARGET_HEIGHT), Image.LANCZOS)

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
    """Scale coordinates from n1's 1000x1000 space to actual screen pixels."""
    x = max(0, min(screen_width, round((coords[0] / N1_COORD_SPACE) * screen_width)))
    y = max(0, min(screen_height, round((coords[1] / N1_COORD_SPACE) * screen_height)))
    return x, y


def _convert_n1_action_to_computer_action(
    fn_name: str, args: Dict[str, Any], screen_width: int, screen_height: int
) -> Optional[Dict[str, Any]]:
    """
    Convert an n1 tool call to the internal computer_call action schema.

    Returns None for actions that should be emitted as function_calls instead
    (goto_url, go_back, refresh).
    """
    # Actions with coordinates
    coords = args.get("coordinates")
    x, y = None, None
    if isinstance(coords, (list, tuple)) and len(coords) >= 2:
        x, y = _unnormalize_coordinates(coords, screen_width, screen_height)

    if fn_name == "left_click":
        if x is None or y is None:
            return None
        return {"action": "left_click", "x": x, "y": y}

    if fn_name == "double_click":
        if x is None or y is None:
            return None
        return {"action": "double_click", "x": x, "y": y}

    if fn_name == "triple_click":
        # Approximate as double_click
        if x is None or y is None:
            return None
        return {"action": "double_click", "x": x, "y": y}

    if fn_name == "right_click":
        if x is None or y is None:
            return None
        return {"action": "right_click", "x": x, "y": y}

    if fn_name == "hover":
        if x is None or y is None:
            return None
        return {"action": "move", "x": x, "y": y}

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
        return {
            "action": "drag",
            "start_x": sx,
            "start_y": sy,
            "end_x": x,
            "end_y": y,
        }

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
        return out

    if fn_name == "type":
        text = args.get("text", "")
        if args.get("press_enter_after"):
            text = text + "\n"
        # Note: clear_before_typing is not supported by the framework's type action.
        # n1 rarely emits this flag; when it does, the field may already be empty.
        return {"action": "type", "text": text}

    if fn_name == "key_press":
        key_comb = args.get("key_comb", "")
        # n1 uses Playwright-compatible key combos like "Control+a", "Escape"
        keys = [k.strip() for k in key_comb.split("+")]
        return {"action": "keypress", "keys": keys}

    if fn_name == "wait":
        return {"action": "wait"}

    if fn_name == "go_back":
        return {"action": "history_back"}

    if fn_name == "refresh":
        return {"action": "keypress", "keys": ["F5"]}

    if fn_name == "goto_url":
        return {"action": "visit_url", "url": args.get("url", "")}

    return None


def _convert_images_to_n1_format(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Convert all images in messages to WebP format optimized for n1."""
    for msg in messages:
        content = msg.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if isinstance(part, dict) and part.get("type") == "image_url":
                url = ((part.get("image_url") or {}).get("url")) or ""
                if url.startswith("data:") and "," in url:
                    b64 = url.split(",", 1)[1]
                    converted = _prepare_image_for_n1(b64)
                    part["image_url"]["url"] = f"data:image/webp;base64,{converted}"
    return messages


@register_agent(models=r"(yutori/)?n1(-.*)?$", tool_type="browser")
class YutoriN1Config(AsyncAgentConfig):
    """
    Yutori n1 browser-use agent loop.

    n1 is a browser-only model that outputs actions as tool_calls.
    Coordinates use a 1000x1000 normalized space.
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
        """Predict the next browser action using Yutori n1."""
        tools = tools or []

        # Get screen dimensions for coordinate denormalization
        screen_width, screen_height = N1_TARGET_WIDTH, N1_TARGET_HEIGHT
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
        completion_messages = _convert_images_to_n1_format(completion_messages)

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

            converted = _prepare_image_for_n1(screenshot_b64)
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
            pre_output_items.append(
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "text",
                            "text": "Taking a screenshot to see the current browser screen.",
                        }
                    ],
                }
            )

        # Build tool list: pass through any custom function tools
        n1_tools = []
        for tool in tools:
            if tool.get("type") == "function":
                func = tool.get("function")
                if func:
                    n1_tools.append({"type": "function", "function": func})
            # Skip computer tools — n1 has built-in browser actions

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "max_retries": max_retries,
            "stream": False,  # n1 does not support streaming
            "temperature": kwargs.pop("temperature", 0.3),
        }

        if n1_tools:
            api_kwargs["tools"] = n1_tools

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
        reasoning_text = message.get("reasoning") or ""

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

                # Try converting to a computer action
                computer_action = _convert_n1_action_to_computer_action(
                    fn_name, args, screen_width, screen_height
                )

                if computer_action is not None:
                    # Build a fake completion message for the converter
                    fake_cm = {
                        "role": "assistant",
                        "content": content_text or "",
                        "tool_calls": [
                            {
                                "type": "function",
                                "id": tc_id,
                                "function": {
                                    "name": "computer",
                                    "arguments": json.dumps(computer_action),
                                },
                            }
                        ],
                    }
                    output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))
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

        return {"output": (pre_output_items + output_items), "usage": usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        raise NotImplementedError(
            "Yutori n1 does not support standalone click prediction. "
            "Use predict_step for full browser automation."
        )

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]
