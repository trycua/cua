"""
OpenCUA agent loop implementation for click prediction and step execution using litellm.acompletion.
Based on OpenCUA model for GUI grounding tasks.
"""

import base64
import io
import json
import re
import uuid
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
    make_reasoning_item,
)
from ..types import AgentCapability
from .composed_grounded import ComposedGroundedConfig
from .generic_vlm import (
    QWEN3_COMPUTER_TOOL,
    _build_nous_system,
    _parse_tool_call_from_text,
    convert_qwen_tool_args_to_computer_action,
)


def extract_coordinates_from_click(text: str) -> Optional[Tuple[int, int]]:
    """Extract coordinates from click(x=..., y=...) or pyautogui.click(x=..., y=...) format.

    This function supports parsing both generic click() and legacy pyautogui.click() formats
    for backwards compatibility with models that may still output pyautogui format.
    """
    try:
        # Look for click(x=1443, y=343) or pyautogui.click(x=1443, y=343) pattern
        pattern = r"(?:pyautogui\.)?click\(x=(\d+),\s*y=(\d+)\)"
        match = re.search(pattern, text)
        if match:
            x, y = int(match.group(1)), int(match.group(2))
            return (x, y)
        return None
    except Exception:
        return None


def _rescale_coordinate(
    x: int,
    y: int,
    orig_w: int,
    orig_h: int,
    resized_w: int,
    resized_h: int,
) -> Tuple[int, int]:
    """Rescale coordinates from resized image space back to original image space."""
    if resized_w == 0 or resized_h == 0:
        return (x, y)
    return (round(x * orig_w / resized_w), round(y * orig_h / resized_h))


@register_agent(models=r"(?i).*OpenCUA.*")
class OpenCUAConfig(ComposedGroundedConfig):
    """OpenCUA agent configuration implementing AsyncAgentConfig protocol for click prediction and step execution."""

    def __init__(self):
        super().__init__()
        self.current_model = None
        self.last_screenshot_b64 = None

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
        """Predict the next step using the OpenCUA model with smart resize (factor=28)."""

        # Convert responses items to completion messages
        converted_msgs = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=False,
        )

        # Build function schemas from tools array
        function_schemas: List[Dict[str, Any]] = []
        if tools:
            from ..computers import is_agent_computer

            for tool in tools:
                tool_type = tool.get("type")
                if tool_type == "computer":
                    computer = tool.get("computer")
                    if computer and is_agent_computer(computer):
                        function_schemas.append(QWEN3_COMPUTER_TOOL["function"])
                elif tool_type == "function":
                    function_schema = tool.get("function")
                    if function_schema:
                        function_schemas.append(function_schema)

        if not function_schemas:
            function_schemas = [QWEN3_COMPUTER_TOOL["function"]]

        # Prepend Nous-generated system prompt with tool schema
        nous_system = _build_nous_system(function_schemas)
        completion_messages = ([nous_system] if nous_system else []) + converted_msgs

        # ------------------------------------------------------------------
        # If there are no screenshots in the conversation, take one now
        # ------------------------------------------------------------------
        def _has_any_image(msgs: List[Dict[str, Any]]) -> bool:
            for m in msgs:
                content = m.get("content")
                if isinstance(content, list):
                    for p in content:
                        if isinstance(p, dict) and p.get("type") == "image_url":
                            return True
            return False

        def _has_screenshot_message(msgs: List[Dict[str, Any]]) -> bool:
            screenshot_text = "Taking a screenshot to see the current computer screen."
            for m in msgs:
                content = m.get("content")
                if isinstance(content, str) and screenshot_text in content:
                    return True
                if isinstance(content, list):
                    for p in content:
                        if isinstance(p, dict) and p.get("type") == "text":
                            if screenshot_text in (p.get("text") or ""):
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
            if not _has_screenshot_message(messages):
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

        # ------------------------------------------------------------------
        # Smart-resize all screenshots with factor=28
        # Unlike generic_vlm (which sets min/max pixel hints for the provider),
        # OpenCUA uses an OpenAI-compatible endpoint that does not honour those
        # hints, so we actually resize the image before sending.
        # ------------------------------------------------------------------
        MIN_PIXELS = 3136
        MAX_PIXELS = 12845056
        FACTOR = 28

        try:
            from qwen_vl_utils import smart_resize  # type: ignore
        except ImportError:
            raise ImportError(
                "qwen-vl-utils not installed. Please install it with: pip install qwen-vl-utils"
            )

        last_orig_w: Optional[int] = None
        last_orig_h: Optional[int] = None
        last_rw: Optional[int] = None
        last_rh: Optional[int] = None

        for msg in completion_messages:
            content = msg.get("content")
            if not isinstance(content, list):
                continue
            for part in content:
                if isinstance(part, dict) and part.get("type") == "image_url":
                    url = ((part.get("image_url") or {}).get("url")) or ""
                    if url.startswith("data:") and "," in url:
                        b64 = url.split(",", 1)[1]
                        img_bytes = base64.b64decode(b64)
                        im = Image.open(io.BytesIO(img_bytes))
                        orig_h, orig_w = im.height, im.width
                        rh, rw = smart_resize(
                            orig_h,
                            orig_w,
                            factor=FACTOR,
                            min_pixels=MIN_PIXELS,
                            max_pixels=MAX_PIXELS,
                        )

                        # Actually resize the image
                        resized_im = im.resize((rw, rh))
                        buf = io.BytesIO()
                        resized_im.save(buf, format="PNG")
                        new_b64 = base64.b64encode(buf.getvalue()).decode("utf-8")
                        part["image_url"]["url"] = f"data:image/png;base64,{new_b64}"

                        last_orig_w, last_orig_h = orig_w, orig_h
                        last_rw, last_rh = rw, rh

        # ------------------------------------------------------------------
        # Call litellm
        # ------------------------------------------------------------------
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

        # ------------------------------------------------------------------
        # Parse response
        # ------------------------------------------------------------------
        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        content_text = message.get("content") or ""
        tool_calls_array = message.get("tool_calls") or []
        reasoning_text = message.get("reasoning") or ""

        output_items: List[Dict[str, Any]] = []

        if reasoning_text:
            output_items.append(make_reasoning_item(reasoning_text))

        # Helper: rescale coordinates from resized space to original space
        def _rescale(x: int, y: int) -> Tuple[int, int]:
            if last_orig_w and last_orig_h and last_rw and last_rh:
                return _rescale_coordinate(x, y, last_orig_w, last_orig_h, last_rw, last_rh)
            return (x, y)

        # Priority 1: OpenCUA native click(x=..., y=...) format
        coords = extract_coordinates_from_click(content_text)
        if coords:
            x, y = _rescale(coords[0], coords[1])
            fake_cm: Dict[str, Any] = {
                "role": "assistant",
                "tool_calls": [
                    {
                        "type": "function",
                        "id": "call_0",
                        "function": {
                            "name": "computer",
                            "arguments": json.dumps({"action": "left_click", "x": x, "y": y}),
                        },
                    }
                ],
            }
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))

        # Priority 2: <tool_call>...</tool_call> XML format
        elif not tool_calls_array:
            tool_call = _parse_tool_call_from_text(content_text)
            if tool_call and isinstance(tool_call, dict):
                fn_name = tool_call.get("name") or "computer"
                raw_args = tool_call.get("arguments") or {}

                # Rescale any coordinate field
                coord = raw_args.get("coordinate")
                if coord and isinstance(coord, (list, tuple)) and len(coord) >= 2:
                    rx, ry = _rescale(int(round(float(coord[0]))), int(round(float(coord[1]))))
                    raw_args = {**raw_args, "coordinate": [rx, ry]}

                fake_cm = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "type": "function",
                            "id": "call_0",
                            "function": {
                                "name": fn_name,
                                "arguments": json.dumps(raw_args),
                            },
                        }
                    ],
                }
                output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))
            else:
                # Plain text response
                fake_cm = {"role": "assistant", "content": content_text}
                output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))

        # Priority 3: tool_calls array from response
        else:
            processed_tool_calls = []
            for tc in tool_calls_array:
                function = tc.get("function", {})
                fn_name = function.get("name", "computer")
                args_str = function.get("arguments", "{}")

                try:
                    args = json.loads(args_str)

                    # Rescale coordinates if present
                    coord = args.get("coordinate")
                    if coord and isinstance(coord, (list, tuple)) and len(coord) >= 2:
                        rx, ry = _rescale(
                            int(round(float(coord[0]))), int(round(float(coord[1])))
                        )
                        args = {**args, "coordinate": [rx, ry]}

                    # Convert Qwen format to Computer Calls format
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
                    processed_tool_calls.append(tc)

            fake_cm = {
                "role": "assistant",
                "content": content_text if content_text else "",
                "tool_calls": processed_tool_calls,
            }
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))

        return {"output": (pre_output_items + output_items), "usage": usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using OpenCUA model via litellm.acompletion.

        Args:
            model: The OpenCUA model name
            image_b64: Base64 encoded image
            instruction: Instruction for where to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        # Prepare system message
        system_prompt = (
            "You are a GUI agent. You are given a task and a screenshot of the screen. "
            "You need to perform a series of click actions to complete the task."
        )

        system_message = {"role": "system", "content": system_prompt}

        # Prepare user message with image and instruction
        user_message = {
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{image_b64}"}},
                {"type": "text", "text": f"Click on {instruction}"},
            ],
        }

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "messages": [system_message, user_message],
            "max_new_tokens": 2056,
            "temperature": 0,
            **kwargs,
        }

        # Use liteLLM acompletion
        response = await litellm.acompletion(**api_kwargs)

        # Extract response text
        output_text = response.choices[0].message.content

        # Extract coordinates from click format
        coordinates = extract_coordinates_from_click(output_text)

        return coordinates

    def get_capabilities(self) -> List[AgentCapability]:
        """Return the capabilities supported by this agent."""
        return ["click", "step"]
