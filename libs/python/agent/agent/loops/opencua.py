"""
OpenCUA agent loop implementation for click prediction using litellm.acompletion
Based on OpenCUA model for GUI grounding tasks.
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
from .composed_grounded import ComposedGroundedConfig


def extract_coordinates_from_pyautogui(text: str) -> Optional[Tuple[int, int]]:
    """Extract coordinates from pyautogui.click(x=..., y=...) format."""
    try:
        # Look for pyautogui.click(x=1443, y=343) pattern
        pattern = r"pyautogui\.click\(x=(\d+),\s*y=(\d+)\)"
        match = re.search(pattern, text)
        if match:
            x, y = int(match.group(1)), int(match.group(2))
            return (x, y)
        return None
    except Exception:
        return None


@register_agent(models=r"(?i).*OpenCUA.*")
class OpenCUAConfig(ComposedGroundedConfig):
    """OpenCUA agent configuration implementing AsyncAgentConfig protocol for click prediction."""

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
            "You need to perform a series of pyautogui actions to complete the task."
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
        # print(output_text)

        # Extract coordinates from pyautogui format
        coordinates = extract_coordinates_from_pyautogui(output_text)

        return coordinates

    def get_capabilities(self) -> List[AgentCapability]:
        """Return the capabilities supported by this agent."""
        return ["click"]
