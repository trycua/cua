"""
Moondream3+ composed-grounded agent loop implementation.
Grounding is handled by a local Moondream3 preview model via Transformers.
Thinking is delegated to the trailing LLM in the composed model string: "moondream3+<thinking_model>".

Differences from composed_grounded:
- Provides a singleton Moondream3 client outside the class.
- predict_click uses model.point(image, instruction, settings={"max_objects": 1}) and returns pixel coordinates.
- If the last image was a screenshot (or we take one), run model.detect(image, "all form ui") to get bboxes, then
  run model.caption on each cropped bbox to label it. Overlay labels on the screenshot and emit via _on_screenshot.
- Add a user message listing all detected form UI names so the thinker can reference them.
- If the thinking model doesn't support vision, filter out image content before calling litellm.
"""

from __future__ import annotations

import base64
import io
import uuid
from typing import Any, Dict, List, Optional, Tuple

import litellm
from PIL import Image, ImageDraw, ImageFont

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_computer_calls_desc2xy,
    convert_computer_calls_xy2desc,
    convert_responses_items_to_completion_messages,
    get_all_element_descriptions,
)
from ..types import AgentCapability

_MOONDREAM_SINGLETON = None


def get_moondream_model() -> Any:
    """Get a singleton instance of the Moondream3 preview model."""
    global _MOONDREAM_SINGLETON
    if _MOONDREAM_SINGLETON is None:
        try:
            import torch
            from transformers import AutoModelForCausalLM

            _MOONDREAM_SINGLETON = AutoModelForCausalLM.from_pretrained(
                "moondream/moondream3-preview",
                trust_remote_code=True,
                torch_dtype=torch.bfloat16,
                device_map="cuda",
            )
        except ImportError as e:
            raise RuntimeError(
                "moondream3 requires torch and transformers. Install with: pip install cua-agent[moondream3]"
            ) from e
    return _MOONDREAM_SINGLETON


def _decode_image_b64(image_b64: str) -> Image.Image:
    data = base64.b64decode(image_b64)
    return Image.open(io.BytesIO(data)).convert("RGB")


def _image_to_b64(img: Image.Image) -> str:
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode("utf-8")


def _supports_vision(model: str) -> bool:
    """Heuristic vision support detection for thinking model."""
    m = model.lower()
    vision_markers = [
        "gpt-4o",
        "gpt-4.1",
        "o1",
        "o3",
        "claude-3",
        "claude-3.5",
        "sonnet",
        "haiku",
        "opus",
        "gemini-1.5",
        "llava",
    ]
    return any(v in m for v in vision_markers)


def _filter_images_from_completion_messages(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    filtered: List[Dict[str, Any]] = []
    for msg in messages:
        msg_copy = {**msg}
        content = msg_copy.get("content")
        if isinstance(content, list):
            msg_copy["content"] = [c for c in content if c.get("type") != "image_url"]
        filtered.append(msg_copy)
    return filtered


def _annotate_detect_and_label_ui(base_img: Image.Image, model_md) -> Tuple[str, List[str]]:
    """Detect UI elements with Moondream, caption each, draw labels with backgrounds.

    Args:
        base_img: PIL image of the screenshot (RGB or RGBA). Will be copied/converted internally.
        model_md: Moondream model instance with .detect() and .query() methods.

    Returns:
        A tuple of (annotated_image_base64_png, detected_names)
    """
    # Ensure RGBA for semi-transparent fills
    if base_img.mode != "RGBA":
        base_img = base_img.convert("RGBA")
    W, H = base_img.width, base_img.height

    # Detect objects
    try:
        detect_result = model_md.detect(base_img, "all ui elements")
        objects = detect_result.get("objects", []) if isinstance(detect_result, dict) else []
    except Exception:
        objects = []

    draw = ImageDraw.Draw(base_img)
    try:
        font = ImageFont.load_default()
    except Exception:
        font = None

    detected_names: List[str] = []

    for i, obj in enumerate(objects):
        try:
            # Clamp normalized coords and crop
            x_min = max(0.0, min(1.0, float(obj.get("x_min", 0.0))))
            y_min = max(0.0, min(1.0, float(obj.get("y_min", 0.0))))
            x_max = max(0.0, min(1.0, float(obj.get("x_max", 0.0))))
            y_max = max(0.0, min(1.0, float(obj.get("y_max", 0.0))))
            left, top, right, bottom = (
                int(x_min * W),
                int(y_min * H),
                int(x_max * W),
                int(y_max * H),
            )
            left, top = max(0, left), max(0, top)
            right, bottom = min(W - 1, right), min(H - 1, bottom)
            crop = base_img.crop((left, top, right, bottom))

            # Prompted short caption
            try:
                result = model_md.query(crop, "Caption this UI element in few words.")
                caption_text = (result or {}).get("answer", "")
            except Exception:
                caption_text = ""

            name = (caption_text or "").strip() or f"element_{i+1}"
            detected_names.append(name)

            # Draw bbox
            draw.rectangle([left, top, right, bottom], outline=(255, 215, 0, 255), width=2)

            # Label background with padding and rounded corners
            label = f"{i+1}. {name}"
            padding = 3
            if font:
                text_bbox = draw.textbbox((0, 0), label, font=font)
            else:
                text_bbox = draw.textbbox((0, 0), label)
            text_w = text_bbox[2] - text_bbox[0]
            text_h = text_bbox[3] - text_bbox[1]

            tx = left + 3
            ty = top - (text_h + 2 * padding + 4)
            if ty < 0:
                ty = top + 3

            bg_left = tx - padding
            bg_top = ty - padding
            bg_right = tx + text_w + padding
            bg_bottom = ty + text_h + padding
            try:
                draw.rounded_rectangle(
                    [bg_left, bg_top, bg_right, bg_bottom],
                    radius=4,
                    fill=(0, 0, 0, 160),
                    outline=(255, 215, 0, 200),
                    width=1,
                )
            except Exception:
                draw.rectangle(
                    [bg_left, bg_top, bg_right, bg_bottom],
                    fill=(0, 0, 0, 160),
                    outline=(255, 215, 0, 200),
                    width=1,
                )

            text_fill = (255, 255, 255, 255)
            if font:
                draw.text((tx, ty), label, fill=text_fill, font=font)
            else:
                draw.text((tx, ty), label, fill=text_fill)
        except Exception:
            continue

    # Encode PNG base64
    annotated = base_img
    if annotated.mode not in ("RGBA", "RGB"):
        annotated = annotated.convert("RGBA")
    annotated_b64 = _image_to_b64(annotated)
    return annotated_b64, detected_names


GROUNDED_COMPUTER_TOOL_SCHEMA = {
    "type": "function",
    "function": {
        "name": "computer",
        "description": (
            "Control a computer by taking screenshots and interacting with UI elements. "
            "The screenshot action will include a list of detected form UI element names when available. "
            "Use element descriptions to locate and interact with UI elements on the screen."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "screenshot",
                        "click",
                        "double_click",
                        "drag",
                        "type",
                        "keypress",
                        "scroll",
                        "move",
                        "wait",
                        "get_current_url",
                        "get_dimensions",
                        "get_environment",
                    ],
                    "description": "The action to perform (required for all actions)",
                },
                "element_description": {
                    "type": "string",
                    "description": "Description of the element to interact with (required for click/double_click/move/scroll)",
                },
                "start_element_description": {
                    "type": "string",
                    "description": "Description of the element to start dragging from (required for drag)",
                },
                "end_element_description": {
                    "type": "string",
                    "description": "Description of the element to drag to (required for drag)",
                },
                "text": {
                    "type": "string",
                    "description": "The text to type (required for type)",
                },
                "keys": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Key(s) to press (required for keypress)",
                },
                "button": {
                    "type": "string",
                    "enum": ["left", "right", "wheel", "back", "forward"],
                    "description": "The mouse button to use for click/double_click",
                },
                "scroll_x": {
                    "type": "integer",
                    "description": "Horizontal scroll amount (required for scroll)",
                },
                "scroll_y": {
                    "type": "integer",
                    "description": "Vertical scroll amount (required for scroll)",
                },
            },
            "required": ["action"],
        },
    },
}


@register_agent(r"moondream3\+.*", priority=2)
class Moondream3PlusConfig(AsyncAgentConfig):
    def __init__(self):
        self.desc2xy: Dict[str, Tuple[float, float]] = {}

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
        # Parse composed model: moondream3+<thinking_model>
        if "+" not in model:
            raise ValueError(f"Composed model must be 'moondream3+<thinking_model>', got: {model}")
        _, thinking_model = model.split("+", 1)

        pre_output_items: List[Dict[str, Any]] = []

        # Acquire last screenshot; if missing, take one
        last_image_b64: Optional[str] = None
        for message in reversed(messages):
            if (
                isinstance(message, dict)
                and message.get("type") == "computer_call_output"
                and isinstance(message.get("output"), dict)
                and message["output"].get("type") == "input_image"
            ):
                image_url = message["output"].get("image_url", "")
                if image_url.startswith("data:image/png;base64,"):
                    last_image_b64 = image_url.split(",", 1)[1]
                    break

        if last_image_b64 is None and computer_handler is not None:
            # Take a screenshot
            screenshot_b64 = await computer_handler.screenshot()  # type: ignore
            if screenshot_b64:
                call_id = uuid.uuid4().hex
                pre_output_items += [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "output_text",
                                "text": "Taking a screenshot to analyze the current screen.",
                            }
                        ],
                    },
                    {
                        "type": "computer_call",
                        "call_id": call_id,
                        "status": "completed",
                        "action": {"type": "screenshot"},
                    },
                    {
                        "type": "computer_call_output",
                        "call_id": call_id,
                        "output": {
                            "type": "input_image",
                            "image_url": f"data:image/png;base64,{screenshot_b64}",
                        },
                    },
                ]
                last_image_b64 = screenshot_b64
                if _on_screenshot:
                    await _on_screenshot(screenshot_b64)

        # If we have a last screenshot, run Moondream detection and labeling
        detected_names: List[str] = []
        if last_image_b64 is not None:
            base_img = _decode_image_b64(last_image_b64)
            model_md = get_moondream_model()
            annotated_b64, detected_names = _annotate_detect_and_label_ui(base_img, model_md)
            if _on_screenshot:
                await _on_screenshot(annotated_b64, "annotated_form_ui")

            # Also push a user message listing all detected names
            if detected_names:
                names_text = "\n".join(f"- {n}" for n in detected_names)
                pre_output_items.append(
                    {
                        "type": "message",
                        "role": "user",
                        "content": [
                            {"type": "input_text", "text": "Detected form UI elements on screen:"},
                            {"type": "input_text", "text": names_text},
                            {
                                "type": "input_text",
                                "text": "Please continue with the next action needed to perform your task.",
                            },
                        ],
                    }
                )

        tool_schemas = []
        for schema in tools or []:
            if schema.get("type") == "computer":
                tool_schemas.append(GROUNDED_COMPUTER_TOOL_SCHEMA)
            else:
                tool_schemas.append(schema)

        # Step 1: Convert computer calls from xy to descriptions
        input_messages = messages + pre_output_items
        messages_with_descriptions = convert_computer_calls_xy2desc(input_messages, self.desc2xy)

        # Step 2: Convert responses items to completion messages
        completion_messages = convert_responses_items_to_completion_messages(
            messages_with_descriptions,
            allow_images_in_tool_results=False,
        )

        # Optionally filter images if model lacks vision
        if not _supports_vision(thinking_model):
            completion_messages = _filter_images_from_completion_messages(completion_messages)

        # Step 3: Call thinking model with litellm.acompletion
        api_kwargs = {
            "model": thinking_model,
            "messages": completion_messages,
            "tools": tool_schemas,
            "max_retries": max_retries,
            "stream": stream,
            **kwargs,
        }
        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = use_prompt_caching

        if _on_api_start:
            await _on_api_start(api_kwargs)

        response = await litellm.acompletion(**api_kwargs)

        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        usage = {
            **response.usage.model_dump(),  # type: ignore
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(usage)

        # Step 4: Convert completion messages back to responses items format
        response_dict = response.model_dump()  # type: ignore
        choice_messages = [choice["message"] for choice in response_dict["choices"]]
        thinking_output_items: List[Dict[str, Any]] = []
        for choice_message in choice_messages:
            thinking_output_items.extend(
                convert_completion_messages_to_responses_items([choice_message])
            )

        # Step 5: Use Moondream to get coordinates for each description
        element_descriptions = get_all_element_descriptions(thinking_output_items)
        if element_descriptions and last_image_b64:
            for desc in element_descriptions:
                for _ in range(3):  # try 3 times
                    coords = await self.predict_click(
                        model=model,
                        image_b64=last_image_b64,
                        instruction=desc,
                    )
                    if coords:
                        self.desc2xy[desc] = coords
                        break

        # Step 6: Convert computer calls from descriptions back to xy coordinates
        final_output_items = convert_computer_calls_desc2xy(thinking_output_items, self.desc2xy)

        # Step 7: Return output and usage
        return {"output": pre_output_items + final_output_items, "usage": usage}

    async def predict_click(
        self,
        model: str,
        image_b64: str,
        instruction: str,
        **kwargs,
    ) -> Optional[Tuple[float, float]]:
        """Predict click coordinates using Moondream3's point API.

        Returns pixel coordinates (x, y) as floats.
        """
        img = _decode_image_b64(image_b64)
        W, H = img.width, img.height
        model_md = get_moondream_model()
        try:
            result = model_md.point(img, instruction, settings={"max_objects": 1})
        except Exception:
            return None

        try:
            pt = (result or {}).get("points", [])[0]
            x_norm = float(pt.get("x", 0.0))
            y_norm = float(pt.get("y", 0.0))
            x_px = max(0.0, min(float(W - 1), x_norm * W))
            y_px = max(0.0, min(float(H - 1), y_norm * H))
            return (x_px, y_px)
        except Exception:
            return None

    def get_capabilities(self) -> List[AgentCapability]:
        return ["click", "step"]
