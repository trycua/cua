"""
InternVL agent loop implementation for click prediction using litellm.acompletion.

Implements the ScreenSpot InternVL grounding baseline behavior:
- Uses the exact grounding prompt format with <image> and <ref> tags
- Expects coordinates in 0-1000 normalized range in formats [[x1,y1,x2,y2]] or [[x,y]]
- Converts to pixel coordinates relative to the original screenshot size

Note: We do NOT manually load the InternVL model; acompletions (via HuggingFaceLocalAdapter)
will handle loading based on the provided model name.
"""

from __future__ import annotations

import base64
import math
import re
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

import litellm
from PIL import Image

from ..decorators import register_agent
from ..types import AgentCapability
from .composed_grounded import ComposedGroundedConfig

# Regex patterns for extracting coordinates
# Accept optional whitespace and optional decimal fractions
_NUM = r"(\d+(?:\.\d+)?)"
_POINT_PATTERN = re.compile(r"\[\[\s*" + _NUM + r"\s*,\s*" + _NUM + r"\s*\]\]")
_BBOX_PATTERN = re.compile(
    r"\[\[\s*" + _NUM + r"\s*,\s*" + _NUM + r"\s*,\s*" + _NUM + r"\s*,\s*" + _NUM + r"\s*\]\]"
)


def _extract_first_point(text: str) -> Optional[Tuple[float, float]]:
    """Extract the first [[x,y]] as normalized (0-1000) floats."""
    m = _POINT_PATTERN.search(text)
    if not m:
        return None
    try:
        x = float(m.group(1))
        y = float(m.group(2))
        return x, y
    except Exception:
        return None


def _extract_last_bbox(text: str) -> Optional[Tuple[float, float, float, float]]:
    """Extract the last [[x1,y1,x2,y2]] as normalized (0-1000) floats."""
    matches = list(_BBOX_PATTERN.finditer(text))
    if not matches:
        return None
    m = matches[-1]
    try:
        x1 = float(m.group(1))
        y1 = float(m.group(2))
        x2 = float(m.group(3))
        y2 = float(m.group(4))
        return x1, y1, x2, y2
    except Exception:
        return None


def _scale_norm_to_pixels(x_norm: float, y_norm: float, width: int, height: int) -> Tuple[int, int]:
    """Scale 0-1000 normalized coordinates to pixel coordinates for given image size."""
    x_px = int(math.floor((x_norm / 1000.0) * width))
    y_px = int(math.floor((y_norm / 1000.0) * height))
    # Clamp to image bounds just in case
    x_px = max(0, min(width - 1, x_px))
    y_px = max(0, min(height - 1, y_px))
    return x_px, y_px


@register_agent(models=r"(?i).*InternVL.*")
class InternVLConfig(ComposedGroundedConfig):
    """InternVL agent configuration reusing ComposedGroundedConfig for steps and
    overriding predict_click to implement ScreenSpot InternVL grounding baseline."""

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
        """Fallback to a self-composed model"""
        return await super().predict_step(
            messages=messages,
            model=f"{model}+{model}",
            tools=tools,
            max_retries=max_retries,
            stream=stream,
            computer_handler=computer_handler,
            _on_api_start=_on_api_start,
            _on_api_end=_on_api_end,
            _on_usage=_on_usage,
            _on_screenshot=_on_screenshot,
            **kwargs,
        )

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using InternVL via litellm.acompletion.

        Behavior mirrors the ScreenSpot InternVL baseline:
        - Prompt: "<image>\nPlease provide the bounding box coordinate of the UI element this user instruction describes: <ref>{instruction}</ref>. Answer in the format of [[x1, y1, x2, y2]]"
        - Parse either [[x,y]] point or [[x1,y1,x2,y2]] bbox, using bbox center if point missing
        - Coordinates are 0-1000 normalized; convert to pixel coordinates for the original screenshot
        """
        try:
            # Decode image dimensions to scale the normalized outputs
            img_bytes = base64.b64decode(image_b64)
            image = Image.open(BytesIO(img_bytes))
            width, height = image.size
        except Exception:
            # If decoding fails, proceed with a safe default size to avoid crash
            width, height = 1920, 1080

        # Build grounding prompt exactly like the baseline
        grounding_prompt = (
            f"Please provide the bounding box coordinate of the UI element this user instruction describes: <ref>{instruction}</ref>. "
            f"Answer in the format of [[x1, y1, x2, y2]]"
        )

        # Prepare messages for LiteLLM
        messages = [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{image_b64}"},
                    },
                    {"type": "text", "text": grounding_prompt},
                ],
            }
        ]

        # Call acompletion; HuggingFaceLocalAdapter/model handler will handle InternVL loading
        api_kwargs = {
            "model": model,
            "messages": messages,
            # Conservative generation params akin to baseline (deterministic)
            "max_tokens": kwargs.get("max_tokens", 256),
            "temperature": kwargs.get("temperature", 0.0),
        }

        response = await litellm.acompletion(**api_kwargs)
        output_text = (response.choices[0].message.content or "").strip()  # type: ignore

        # print(f"InternVL output: {output_text}")

        # Try to parse a point first; if absent, parse bbox and take center
        point = _extract_first_point(output_text)
        if point is None:
            bbox = _extract_last_bbox(output_text)
            if bbox is None:
                return None
            x1, y1, x2, y2 = bbox
            cx = (x1 + x2) / 2.0
            cy = (y1 + y2) / 2.0
            point = (cx, cy)

        x_norm, y_norm = point
        x_px, y_px = _scale_norm_to_pixels(x_norm, y_norm, width, height)
        return (x_px, y_px)

    def get_capabilities(self) -> List[AgentCapability]:
        return ["click", "step"]
