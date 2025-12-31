"""FARA VLM agent configuration."""

from __future__ import annotations

import ast
import json
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)

from ...decorators import register_agent
from ...loops.base import AsyncAgentConfig
from ...responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
    make_reasoning_item,
)
from ...types import AgentCapability
from .helpers import (
    build_nous_system,
    convert_qwen_tool_args_to_computer_action,
    parse_tool_call_from_text,
    unnormalize_coordinate,
)


@register_agent(models=r"(?i).*fara-7b.*")
class FaraVlmConfig(AsyncAgentConfig):
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
        # Check if the last message is a terminate function_call_output
        # If so, return a final assistant message to stop the loop
        if messages:
            last_msg = messages[-1]
            if last_msg.get("type") in ("function_call_output", "computer_call_output"):
                output_data = last_msg.get("output")

                # Parse string if needed (could be JSON or Python dict literal)
                if isinstance(output_data, str):
                    try:
                        output_data = json.loads(output_data)
                    except:
                        try:
                            output_data = ast.literal_eval(output_data)
                        except:
                            pass

                # Check if it's a terminate action output (contains "terminated": True)
                if isinstance(output_data, dict) and output_data.get("terminated") is True:
                    return {
                        "output": [
                            {
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": "Task completed."}],
                            }
                        ],
                        "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                    }

        # Build messages using NousFnCallPrompt system with tool schema in text
        # Start with converted conversation (images/text preserved)
        converted_msgs = convert_responses_items_to_completion_messages(
            messages, allow_images_in_tool_results=False, use_xml_tools=True
        )

        # Build function schemas from tools array
        function_schemas = []
        if tools:
            from ...computers import is_agent_computer

            for tool in tools:
                tool_type = tool.get("type")

                if tool_type == "computer":
                    # For computer tools, use QWEN3_COMPUTER_TOOL schema
                    computer = tool.get("computer")
                    if computer and is_agent_computer(computer):
                        function_schemas.append(QWEN3_COMPUTER_TOOL["function"])
                elif tool_type == "function":
                    # For function tools, use the provided function schema
                    function_schema = tool.get("function")
                    if function_schema:
                        function_schemas.append(function_schema)

        # If no tools provided or no computer tool found, use default QWEN3_COMPUTER_TOOL
        if not function_schemas:
            function_schemas = [QWEN3_COMPUTER_TOOL["function"]]

        # Prepend Nous-generated system if available
        nous_system = build_nous_system(function_schemas)
        completion_messages = ([nous_system] if nous_system else []) + converted_msgs

        # If there is no screenshot in the conversation, take one now and inject it.
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

            await _on_screenshot(screenshot_b64, "screenshot_before")

            # Check if computer_handler has get_current_url method
            screenshot_text = "Here is the next screenshot. Think about what to do next."
            if hasattr(computer_handler, "get_current_url"):
                try:
                    current_url = await computer_handler.get_current_url()
                    screenshot_text = f"Current URL: {current_url[:100]}\nHere is the next screenshot. Think about what to do next."
                except Exception:
                    # If get_current_url fails, fall back to default text
                    pass

            # Inject a user message with the screenshot so the model can see current context
            screenshot_msg = {
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{screenshot_b64}"},
                    },
                    {"type": "text", "text": screenshot_text},
                ],
            }
            completion_messages.append(screenshot_msg)

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
                            h, w, factor=28, min_pixels=MIN_PIXELS, max_pixels=MAX_PIXELS
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

        # Extract response data
        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        content_text = message.get("content") or ""
        tool_calls_array = message.get("tool_calls") or []
        reasoning_text = message.get("reasoning") or ""

        output_items: List[Dict[str, Any]] = []

        # Add reasoning if present (Ollama Cloud format)
        if reasoning_text:
            output_items.append(make_reasoning_item(reasoning_text))

        # Priority 1: Try to parse tool call from content text (OpenRouter format)
        tool_call = parse_tool_call_from_text(content_text)

        if tool_call and isinstance(tool_call, dict):
            fn_name = tool_call.get("name") or "computer"
            raw_args = tool_call.get("arguments") or {}
            # Unnormalize coordinates to actual screen size using last resized dims
            if last_rw is None or last_rh is None:
                raise RuntimeError(
                    "No screenshots found to derive dimensions for coordinate unnormalization."
                )
            args = await unnormalize_coordinate(raw_args, (last_rw, last_rh))

            # Extract thoughts (text before <tool_call> tag)
            thoughts = ""
            if "<tool_call>" in content_text:
                thoughts = content_text.split("<tool_call>")[0].strip()

            # Build an OpenAI-style tool call so we can reuse the converter
            fake_cm = {
                "role": "assistant",
                "content": thoughts,  # Preserve thoughts before tool call
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
        elif tool_calls_array:
            # Priority 2: Use tool_calls field if present (Ollama Cloud format)
            # Process and unnormalize coordinates in tool calls
            processed_tool_calls = []
            for tc in tool_calls_array:
                function = tc.get("function", {})
                fn_name = function.get("name", "computer")
                args_str = function.get("arguments", "{}")

                try:
                    args = json.loads(args_str)

                    # Unnormalize coordinates if present
                    if "coordinate" in args and last_rw is not None and last_rh is not None:
                        args = await unnormalize_coordinate(args, (last_rw, last_rh))

                    # Convert Qwen format to Computer Calls format if this is a computer tool
                    if fn_name == "computer":
                        converted_action = convert_qwen_tool_args_to_computer_action(args)
                        if converted_action:
                            args = converted_action

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
                    # Keep original if parsing fails
                    processed_tool_calls.append(tc)

            fake_cm = {
                "role": "assistant",
                "content": content_text if content_text else "",
                "tool_calls": processed_tool_calls,
            }
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))
        else:
            # No tool calls found in either format, return text response
            fake_cm = {"role": "assistant", "content": content_text}
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))

        # Check if this is a terminate action - if so, add a final assistant message to stop the loop
        has_terminate = False
        for item in output_items:
            if item.get("type") == "computer_call":
                action = item.get("action", {})
                if action.get("type") == "terminate":
                    has_terminate = True
                    break
            elif item.get("type") == "function_call":
                try:
                    args = json.loads(item.get("arguments", "{}"))
                    if args.get("action") == "terminate":
                        has_terminate = True
                        break
                except:
                    pass

        # If terminate detected, ensure LAST item is an assistant message to exit the loop
        # The generic agent loop checks: while new_items[-1].get("role") != "assistant"
        if has_terminate:
            output_items.append(
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": ""}],
                }
            )

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
        nous_system = build_nous_system([reduced_tool["function"]])

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
            rh, rw = smart_resize(h, w, factor=28, min_pixels=min_pixels, max_pixels=max_pixels)
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
        tool_call = parse_tool_call_from_text(content_text) or {}
        args = tool_call.get("arguments") or {}
        args = await unnormalize_coordinate(args, (rh, rw))
        coord = args.get("coordinate")
        if isinstance(coord, (list, tuple)) and len(coord) >= 2:
            return int(coord[0]), int(coord[1])
        return None


# ComputerUse tool schema (OpenAI function tool format)
QWEN3_COMPUTER_TOOL: dict[str, Any] = {
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
            },
            "required": ["action"],
        },
    },
}
