"""Ollama API client implementation."""

import asyncio
import logging
from typing import Any, Dict, List, Optional, Tuple, cast

from httpx import ConnectError, ReadTimeout
from ollama import AsyncClient, Message, Options

from .base import BaseOmniClient

logger = logging.getLogger(__name__)


class OllamaClient(BaseOmniClient):
    """Client for making calls to Ollama API."""

    def __init__(self, api_key: str, model: str, max_retries: int = 3, retry_delay: float = 1.0):
        """Initialize the Ollama client.

        Args:
            api_key: Not used
            model: Ollama model name (e.g. "gemma3:4b-it-q4_K_M")
            max_retries: Maximum number of retries for API calls
            retry_delay: Base delay between retries in seconds
        """
        if not model:
            raise ValueError("Model name must be provided")

        self.client = AsyncClient(
            host="http://localhost:11434",
        )
        self.model: str = model  # Add explicit type annotation
        self.max_retries = max_retries
        self.retry_delay = retry_delay

    def _convert_message_format(self, system: str, messages: List[Dict[str, Any]]) -> List[Any]:
        """Convert messages from standard format to Ollama format.

        Args:
            messages: Messages in standard format

        Returns:
            Messages in Ollama format
        """
        ollama_messages = []

        # Add system message
        ollama_messages.append(
            {
                "role": "system",
                "content": system,
            }
        )

        for message in messages:
            # Skip messages with empty content
            if not message.get("content"):
                continue
            content = message.get("content", [{}])[0]
            isImage = content.get("type", "") == "image_url"
            isText = content.get("type", "") == "text"
            if isText:
                data = content.get("text", "")
                ollama_messages.append({"role": message["role"], "content": data})
            if isImage:
                data = content.get("image_url", {}).get("url", "")
                # remove header
                data = data.removeprefix("data:image/png;base64,")
                ollama_messages.append(
                    {"role": message["role"], "content": "Use this image", "images": [data]}
                )

        # Cast the list to the correct type expected by Ollama
        return cast(List[Any], ollama_messages)

    async def run_interleaved(
        self, messages: List[Dict[str, Any]], system: str, max_tokens: int
    ) -> Any:
        """Run model with interleaved conversation format.

        Args:
            messages: List of messages to process
            system: System prompt
            max_tokens: Not used

        Returns:
            Model response
        """
        last_error = None

        for attempt in range(self.max_retries):
            try:
                # Convert messages to Ollama format
                ollama_messages = self._convert_message_format(system, messages)

                response = await self.client.chat(
                    model=self.model,
                    options=Options(
                        temperature=0,
                    ),
                    messages=ollama_messages,
                    format="json",
                )

                return response

            except (ConnectError, ReadTimeout) as e:
                last_error = e
                logger.warning(
                    f"Connection error on attempt {attempt + 1}/{self.max_retries}: {str(e)}"
                )
                if attempt < self.max_retries - 1:
                    await asyncio.sleep(self.retry_delay * (attempt + 1))  # Exponential backoff
                continue

            except Exception as e:
                logger.error(f"Unexpected error in Ollama API call: {str(e)}")
                raise RuntimeError(f"Ollama API call failed: {str(e)}")

        # If we get here, all retries failed
        raise RuntimeError(f"Connection error after {self.max_retries} retries: {str(last_error)}")
