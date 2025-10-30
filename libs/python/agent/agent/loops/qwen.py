"""
Qwen3-VL agent loop implementation using litellm with function/tool calling.
- Passes a ComputerUse tool schema to acompletion
- Converts between Responses items and completion messages using helpers
"""

from __future__ import annotations

import json
import re
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
)
from ..types import AgentCapability

# ComputerUse tool schema (OpenAI function tool format)
QWEN3_COMPUTER_TOOL: Dict[str, Any] = {
    "type": "function",
    "function": {
        "name": "computer",
        "description": (
            "Use a mouse and keyboard to interact with a computer, and take screenshots.\n"
            "* This is an interface to a desktop GUI. You do not have access to a terminal or applications menu. You must click on desktop icons to start applications.\n"
            "* Some applications may take time to start or process actions, so you may need to wait and take successive screenshots to see the results of your actions. E.g. if you click on Firefox and a window doesn't open, try wait and taking another screenshot.\n"
            "* The screen's resolution is 1000x1000.\n"
            "* Whenever you intend to move the cursor to click on an element like an icon, you should consult a screenshot to determine the coordinates of the element before moving the cursor.\n"
            "* If you tried clicking on a program or link but it failed to load, even after waiting, try adjusting your cursor position so that the tip of the cursor visually falls on the element that you want to click.\n"
            "* Make sure to click any buttons, links, icons, etc with the cursor tip in the center of the element. Don't click boxes on their edges."
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
                        # "terminate",
                        # "answer",
                    ],
                    "type": "string",
                },
                "keys": {
                    "description": "Required only by action=key.",
                    "type": "array",
                    "items": {"type": "string"},
                },
                "text": {
                    "description": "Required only by action=type and action=answer.",
                    "type": "string",
                },
                "coordinate": {
                    "description": "(x, y): Pixel coordinates from top-left.",
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
                # "status": {
                #     "description": "Task status (action=terminate).",
                #     "type": "string",
                #     "enum": ["success", "failure"],
                # },
            },
            "required": ["action"],
        },
    },
}


def _build_nous_system(functions: List[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
    """Use qwen-agent NousFnCallPrompt to generate a system message embedding tool schema."""
    try:
        from qwen_agent.llm.fncall_prompts.nous_fncall_prompt import (
            ContentItem as NousContentItem,
        )
        from qwen_agent.llm.fncall_prompts.nous_fncall_prompt import (
            Message as NousMessage,
        )
        from qwen_agent.llm.fncall_prompts.nous_fncall_prompt import (
            NousFnCallPrompt,
        )
    except ImportError:
        raise ImportError(
            "qwen-agent not installed. Please install it with `pip install cua-agent[qwen]`."
        )
    msgs = NousFnCallPrompt().preprocess_fncall_messages(
        messages=[
            NousMessage(
                role="system", content=[NousContentItem(text="You are a helpful assistant.")]
            )
        ],
        functions=functions,
        lang="en",
    )
    sys = msgs[0].model_dump()
    # Convert qwen-agent structured content to OpenAI-style content list
    content = [{"type": "text", "text": c["text"]} for c in sys.get("content", [])]
    return {"role": "system", "content": content}


def _parse_tool_call_from_text(text: str) -> Optional[Dict[str, Any]]:
    """Extract JSON object within <tool_call>...</tool_call> from model text."""
    m = re.search(r"<tool_call>\s*(\{[\s\S]*?\})\s*</tool_call>", text)
    if not m:
        return None
    try:
        return json.loads(m.group(1))
    except Exception:
        return None


async def _unnormalize_coordinate(args: Dict[str, Any], dims: Tuple[int, int]) -> Dict[str, Any]:
    """Coordinates appear in 0..1000 space, scale to actual screen size using dims if provided."""
    coord = args.get("coordinate")
    if not coord or not isinstance(coord, (list, tuple)) or len(coord) < 2:
        return args
    x, y = float(coord[0]), float(coord[1])
    width, height = float(dims[0]), float(dims[1])
    x_abs = max(0.0, min(width, (x / 1000.0) * width))
    y_abs = max(0.0, min(height, (y / 1000.0) * height))
    args = {**args, "coordinate": [round(x_abs), round(y_abs)]}
    return args


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

    # Coordinates helper
    coord = args.get("coordinate")
    x = y = None
    if isinstance(coord, (list, tuple)) and len(coord) >= 2:
        try:
            x = int(round(float(coord[0])))
            y = int(round(float(coord[1])))
        except Exception:
            x = y = None

    # Map actions
    a = action.lower()
    if a in {"left_click", "right_click", "middle_click", "double_click"}:
        if x is None or y is None:
            return None
        return {"action": a, "x": x, "y": y}
    if a == "triple_click":
        # Approximate as double_click
        if x is None or y is None:
            return None
        return {"action": "double_click", "x": x, "y": y}
    if a == "mouse_move":
        if x is None or y is None:
            return None
        return {"action": "move", "x": x, "y": y}
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
        # Include cursor position if available (optional)
        out: Dict[str, Any] = {"action": "scroll", "scroll_x": scroll_x, "scroll_y": scroll_y}
        if x is not None and y is not None:
            out.update({"x": x, "y": y})
        return out
    if a == "wait":
        return {"action": "wait"}

    # Non-UI or terminal actions: terminate/answer -> not mapped here
    return None


@register_agent(models=r"(?i).*qwen.*", priority=-1)
class Qwen3VlConfig(AsyncAgentConfig):
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
        # Build messages using NousFnCallPrompt system with tool schema in text
        # Start with converted conversation (images/text preserved)
        converted_msgs = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=False,
        )

        # Prepend Nous-generated system if available
        nous_system = _build_nous_system([QWEN3_COMPUTER_TOOL["function"]])
        completion_messages = ([nous_system] if nous_system else []) + converted_msgs

        # If there is no screenshot in the conversation, take one now and inject it.
        # Also record a pre_output_items assistant message to reflect action.
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
            # Inject a user message with the screenshot so the model can see current context
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
            # Add assistant message to outputs to reflect the action, similar to composed_grounded.py
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

        # Smart-resize all screenshots and attach min/max pixel hints. Fail fast if deps missing.
        # Also record the last resized width/height to unnormalize coordinates later.
        last_rw: Optional[int] = None
        last_rh: Optional[int] = None
        MIN_PIXELS = 3136
        MAX_PIXELS = 12845056
        try:
            import base64
            import io

            from PIL import Image  # type: ignore
            from qwen_vl_utils import smart_resize  # type: ignore
        except Exception:
            raise ImportError(
                "qwen-vl-utils not installed. Please install it with `pip install cua-agent[qwen]`."
            )

        for msg in completion_messages:
            content = msg.get("content")
            if not isinstance(content, list):
                continue
            for part in content:
                if isinstance(part, dict) and part.get("type") == "image_url":
                    url = ((part.get("image_url") or {}).get("url")) or ""
                    # Expect data URL like data:image/png;base64,<b64>
                    if url.startswith("data:") and "," in url:
                        b64 = url.split(",", 1)[1]
                        img_bytes = base64.b64decode(b64)
                        im = Image.open(io.BytesIO(img_bytes))
                        h, w = im.height, im.width
                        rh, rw = smart_resize(
                            h, w, factor=32, min_pixels=MIN_PIXELS, max_pixels=MAX_PIXELS
                        )
                        # Attach hints on this image block
                        part["min_pixels"] = MIN_PIXELS
                        part["max_pixels"] = MAX_PIXELS
                        last_rw, last_rh = rw, rh

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "max_retries": max_retries,
            "stream": stream,
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

        # Parse tool call from text; then convert to responses items via fake tool_calls
        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        content_text = ((choice.get("message") or {}).get("content")) or ""
        tool_call = _parse_tool_call_from_text(content_text)

        output_items: List[Dict[str, Any]] = []
        if tool_call and isinstance(tool_call, dict):
            fn_name = tool_call.get("name") or "computer"
            raw_args = tool_call.get("arguments") or {}
            # Unnormalize coordinates to actual screen size using last resized dims
            if last_rw is None or last_rh is None:
                raise RuntimeError(
                    "No screenshots found to derive dimensions for coordinate unnormalization."
                )
            args = await _unnormalize_coordinate(raw_args, (last_rw, last_rh))

            # Build an OpenAI-style tool call so we can reuse the converter
            fake_cm = {
                "role": "assistant",
                "tool_calls": [
                    {
                        "type": "function",
                        "id": "call_0",
                        "function": {
                            "name": fn_name,
                            "arguments": json.dumps(args),
                        },
                    }
                ],
            }
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))
        else:
            # Fallback: just return assistant text
            fake_cm = {"role": "assistant", "content": content_text}
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))

        # Prepend any pre_output_items (e.g., simulated screenshot-taking message)
        return {"output": (pre_output_items + output_items), "usage": usage}

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using Qwen3-VL via litellm.acompletion.

        Only exposes a reduced tool schema with left_click to bias model to output a single click.
        Returns (x, y) absolute pixels when screen dimensions can be obtained; otherwise normalized 0..1000 integers.
        """
        # Reduced tool
        reduced_tool = {
            "type": "function",
            "function": {
                **QWEN3_COMPUTER_TOOL["function"],
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

        # Build Nous system (lazy import inside helper already raises clear guidance if missing)
        nous_system = _build_nous_system([reduced_tool["function"]])

        # Pre-process using smart_resize
        min_pixels = 3136
        max_pixels = 12845056
        try:
            # Lazy import to avoid hard dependency
            import base64
            import io

            # If PIL is available, estimate size from image to derive smart bounds
            from PIL import Image
            from qwen_vl_utils import smart_resize  # type: ignore

            img_bytes = base64.b64decode(image_b64)
            im = Image.open(io.BytesIO(img_bytes))
            h, w = im.height, im.width
            # Qwen notebook suggests factor=32 and a wide min/max range
            rh, rw = smart_resize(h, w, factor=32, min_pixels=min_pixels, max_pixels=max_pixels)
        except Exception:
            raise ImportError(
                "qwen-vl-utils not installed. Please install it with `pip install cua-agent[qwen]`."
            )

        messages = []
        if nous_system:
            messages.append(nous_system)
        image_block: Dict[str, Any] = {
            "type": "image_url",
            "image_url": {"url": f"data:image/png;base64,{image_b64}"},
            "min_pixels": min_pixels,
            "max_pixels": max_pixels,
        }
        # Single user message with image and instruction, matching OpenAI-style content blocks
        messages.append(
            {
                "role": "user",
                "content": [
                    image_block,
                    {"type": "text", "text": instruction},
                ],
            }
        )

        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": messages,
            **{k: v for k, v in kwargs.items()},
        }
        response = await litellm.acompletion(**api_kwargs)
        resp = response.model_dump()  # type: ignore
        choice = (resp.get("choices") or [{}])[0]
        content_text = ((choice.get("message") or {}).get("content")) or ""
        tool_call = _parse_tool_call_from_text(content_text) or {}
        args = tool_call.get("arguments") or {}
        args = await _unnormalize_coordinate(args, (rh, rw))
        coord = args.get("coordinate")
        if isinstance(coord, (list, tuple)) and len(coord) >= 2:
            return int(coord[0]), int(coord[1])
        return None
