"""
GTA1 agent loop implementation for click prediction using litellm.acompletion
Paper: https://arxiv.org/pdf/2507.05791
Code: https://github.com/Yan98/GTA1
"""

import asyncio
import base64
import json
import math
import re
import uuid
from io import BytesIO
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, Union

import litellm
from PIL import Image

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..types import AgentCapability, AgentResponse, Messages, Tools

SYSTEM_PROMPT = """
You are an expert UI element locator. Given a GUI image and a user's element description, provide the coordinates of the specified element as a single (x,y) point. The image resolution is height {height} and width {width}. For elements with area, return the center point.

Output the coordinate pair exactly:
(x,y)
""".strip()


def extract_coordinates(raw_string: str) -> Tuple[float, float]:
    """Extract coordinates from model output."""
    try:
        matches = re.findall(r"\((-?\d*\.?\d+),\s*(-?\d*\.?\d+)\)", raw_string)
        return tuple(map(float, matches[0]))  # type: ignore
    except:
        return (0.0, 0.0)


def smart_resize(
    height: int, width: int, factor: int = 28, min_pixels: int = 3136, max_pixels: int = 8847360
) -> Tuple[int, int]:
    """Smart resize function similar to qwen_vl_utils."""
    # Calculate the total pixels
    total_pixels = height * width

    # If already within bounds, return original dimensions
    if min_pixels <= total_pixels <= max_pixels:
        # Round to nearest factor
        new_height = (height // factor) * factor
        new_width = (width // factor) * factor
        return new_height, new_width

    # Calculate scaling factor
    if total_pixels > max_pixels:
        scale = (max_pixels / total_pixels) ** 0.5
    else:
        scale = (min_pixels / total_pixels) ** 0.5

    # Apply scaling
    new_height = int(height * scale)
    new_width = int(width * scale)

    # Round to nearest factor
    new_height = (new_height // factor) * factor
    new_width = (new_width // factor) * factor

    # Ensure minimum size
    new_height = max(new_height, factor)
    new_width = max(new_width, factor)

    return new_height, new_width


@register_agent(models=r".*GTA1.*")
class GTA1Config(AsyncAgentConfig):
    """GTA1 agent configuration implementing AsyncAgentConfig protocol for click prediction."""

    def __init__(self):
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
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **kwargs,
    ) -> Dict[str, Any]:
        raise NotImplementedError()

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[float, float]]:
        """
        Predict click coordinates using GTA1 model via litellm.acompletion.

        Args:
            model: The GTA1 model name
            image_b64: Base64 encoded image
            instruction: Instruction for where to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        # Decode base64 image
        image_data = base64.b64decode(image_b64)
        image = Image.open(BytesIO(image_data))
        width, height = image.width, image.height

        # Smart resize the image (similar to qwen_vl_utils)
        resized_height, resized_width = smart_resize(
            height,
            width,
            factor=28,  # Default factor for Qwen models
            min_pixels=3136,
            max_pixels=4096 * 2160,
        )
        resized_image = image.resize((resized_width, resized_height))
        scale_x, scale_y = width / resized_width, height / resized_height

        # Convert resized image back to base64
        buffered = BytesIO()
        resized_image.save(buffered, format="PNG")
        resized_image_b64 = base64.b64encode(buffered.getvalue()).decode()

        # Prepare system and user messages
        system_message = {
            "role": "system",
            "content": SYSTEM_PROMPT.format(height=resized_height, width=resized_width),
        }

        user_message = {
            "role": "user",
            "content": [
                {
                    "type": "image_url",
                    "image_url": {"url": f"data:image/png;base64,{resized_image_b64}"},
                },
                {"type": "text", "text": instruction},
            ],
        }

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "messages": [system_message, user_message],
            "max_tokens": 2056,
            "temperature": 0.0,
            **kwargs,
        }

        # Use liteLLM acompletion
        response = await litellm.acompletion(**api_kwargs)

        # Extract response text
        output_text = response.choices[0].message.content  # type: ignore

        # Extract and rescale coordinates
        pred_x, pred_y = extract_coordinates(output_text)  # type: ignore
        pred_x *= scale_x
        pred_y *= scale_y

        return (math.floor(pred_x), math.floor(pred_y))

    def get_capabilities(self) -> List[AgentCapability]:
        """Return the capabilities supported by this agent."""
        return ["click"]
