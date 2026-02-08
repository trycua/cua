"""Anthropic API client implementation."""

import asyncio
import logging
from typing import Any, Dict, List, Optional, Tuple, cast

from anthropic import Anthropic, AsyncAnthropic
from anthropic.types import MessageParam
from httpx import ConnectError, ReadTimeout

from .base import BaseOmniClient

logger = logging.getLogger(__name__)


class AnthropicClient(BaseOmniClient):
    """Client for making calls to Anthropic API."""

    def __init__(self, api_key: str, model: str, max_retries: int = 3, retry_delay: float = 1.0):
        """Initialize the Anthropic client.

        Args:
            api_key: Anthropic API key
            model: Anthropic model name (e.g. "claude-3-opus-20240229")
            max_retries: Maximum number of retries for API calls
            retry_delay: Base delay between retries in seconds
        """
        if not model:
            raise ValueError("Model name must be provided")

        self.client = AsyncAnthropic(api_key=api_key)
        self.model: str = model  # Add explicit type annotation
        self.max_retries = max_retries
        self.retry_delay = retry_delay

    def _convert_message_format(self, messages: List[Dict[str, Any]]) -> List[MessageParam]:
        """Convert messages from standard format to Anthropic format.

        Args:
            messages: Messages in standard format

        Returns:
            Messages in Anthropic format
        """
        anthropic_messages = []

        for message in messages:
            # Skip messages with empty content
            if not message.get("content"):
                continue

            if message["role"] == "user":
                anthropic_messages.append({"role": "user", "content": message["content"]})
            elif message["role"] == "assistant":
                anthropic_messages.append({"role": "assistant", "content": message["content"]})

        # Cast the list to the correct type expected by Anthropic
        return cast(List[MessageParam], anthropic_messages)

    async def run_interleaved(
        self, messages: List[Dict[str, Any]], system: str, max_tokens: int
    ) -> Any:
        """Run model with interleaved conversation format.

        Args:
            messages: List of messages to process
            system: System prompt
            max_tokens: Maximum tokens to generate

        Returns:
            Model response
        """
        last_error = None

        for attempt in range(self.max_retries):
            try:
                # Convert messages to Anthropic format
                anthropic_messages = self._convert_message_format(messages)

                response = await self.client.messages.create(
                    model=self.model,
                    max_tokens=max_tokens,
                    temperature=0,
                    system=system,
                    messages=anthropic_messages,
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
                logger.error(f"Unexpected error in Anthropic API call: {str(e)}")
                raise RuntimeError(f"Anthropic API call failed: {str(e)}")

        # If we get here, all retries failed
        raise RuntimeError(f"Connection error after {self.max_retries} retries: {str(last_error)}")
