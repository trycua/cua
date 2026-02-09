"""OpenAI client implementation."""

import logging
import os
import re
from datetime import datetime
from typing import Any, Dict, List, Optional

import aiohttp

from .base import BaseOmniClient

logger = logging.getLogger(__name__)

# OpenAI specific client for the OmniLoop


class OpenAIClient(BaseOmniClient):
    """OpenAI vision API client implementation."""

    def __init__(
        self,
        api_key: Optional[str] = None,
        model: str = "gpt-4o",
        provider_base_url: str = "https://api.openai.com/v1",
        max_tokens: int = 4096,
        temperature: float = 0.0,
    ):
        """Initialize the OpenAI client.

        Args:
            api_key: OpenAI API key
            model: Model to use
            provider_base_url: API endpoint
            max_tokens: Maximum tokens to generate
            temperature: Generation temperature
        """
        super().__init__(api_key=api_key, model=model)
        self.api_key = api_key or os.getenv("OPENAI_API_KEY")
        if not self.api_key:
            raise ValueError("No OpenAI API key provided")

        self.model = model
        self.provider_base_url = provider_base_url
        self.max_tokens = max_tokens
        self.temperature = temperature

    def _extract_base64_image(self, text: str) -> Optional[str]:
        """Extract base64 image data from an HTML img tag."""
        pattern = r'data:image/[^;]+;base64,([^"]+)'
        match = re.search(pattern, text)
        return match.group(1) if match else None

    def _get_loggable_messages(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Create a loggable version of messages with image data truncated."""
        loggable_messages = []
        for msg in messages:
            if isinstance(msg.get("content"), list):
                new_content = []
                for content in msg["content"]:
                    if content.get("type") == "image":
                        new_content.append(
                            {"type": "image", "image_url": {"url": "[BASE64_IMAGE_DATA]"}}
                        )
                    else:
                        new_content.append(content)
                loggable_messages.append({"role": msg["role"], "content": new_content})
            else:
                loggable_messages.append(msg)
        return loggable_messages

    async def run_interleaved(
        self, messages: List[Dict[str, Any]], system: str, max_tokens: Optional[int] = None
    ) -> Dict[str, Any]:
        """Run interleaved chat completion.

        Args:
            messages: List of message dicts
            system: System prompt
            max_tokens: Optional max tokens override

        Returns:
            Response dict
        """
        headers = {"Content-Type": "application/json", "Authorization": f"Bearer {self.api_key}"}

        final_messages = [{"role": "system", "content": system}]

        # Process messages
        for item in messages:
            if isinstance(item, dict):
                if isinstance(item["content"], list):
                    # Content is already in the correct format
                    final_messages.append(item)
                else:
                    # Single string content, check for image
                    base64_img = self._extract_base64_image(item["content"])
                    if base64_img:
                        message = {
                            "role": item["role"],
                            "content": [
                                {
                                    "type": "image_url",
                                    "image_url": {"url": f"data:image/jpeg;base64,{base64_img}"},
                                }
                            ],
                        }
                    else:
                        message = {
                            "role": item["role"],
                            "content": [{"type": "text", "text": item["content"]}],
                        }
                    final_messages.append(message)
            else:
                # String content, check for image
                base64_img = self._extract_base64_image(item)
                if base64_img:
                    message = {
                        "role": "user",
                        "content": [
                            {
                                "type": "image_url",
                                "image_url": {"url": f"data:image/jpeg;base64,{base64_img}"},
                            }
                        ],
                    }
                else:
                    message = {"role": "user", "content": [{"type": "text", "text": item}]}
                final_messages.append(message)

        payload = {"model": self.model, "messages": final_messages, "temperature": self.temperature}

        if "o1" in self.model or "o3-mini" in self.model:
            payload["reasoning_effort"] = "low"
            payload["max_completion_tokens"] = max_tokens or self.max_tokens
        else:
            payload["max_tokens"] = max_tokens or self.max_tokens

        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f"{self.provider_base_url}/chat/completions", headers=headers, json=payload
                ) as response:
                    response_json = await response.json()

                    if response.status != 200:
                        error_msg = response_json.get("error", {}).get(
                            "message", str(response_json)
                        )
                        logger.error(f"Error in OpenAI API call: {error_msg}")
                        raise Exception(f"OpenAI API error: {error_msg}")

                    return response_json

        except Exception as e:
            logger.error(f"Error in OpenAI API call: {str(e)}")
            raise
