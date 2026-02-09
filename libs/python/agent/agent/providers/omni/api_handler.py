"""API handling for Omni provider."""

import logging
from typing import Any, Dict, List

from .prompts import SYSTEM_PROMPT

logger = logging.getLogger(__name__)


class OmniAPIHandler:
    """Handler for Omni API calls."""

    def __init__(self, loop):
        """Initialize the API handler.

        Args:
            loop: Parent loop instance
        """
        self.loop = loop

    async def make_api_call(
        self, messages: List[Dict[str, Any]], system_prompt: str = SYSTEM_PROMPT
    ) -> Any:
        """Make an API call to the appropriate provider.

        Args:
            messages: List of messages in standard OpenAI format
            system_prompt: System prompt to use

        Returns:
            API response
        """
        if not self.loop._make_api_call:
            raise RuntimeError("Loop does not have _make_api_call method")

        try:
            # Use the loop's _make_api_call method with standard messages
            return await self.loop._make_api_call(messages=messages, system_prompt=system_prompt)
        except Exception as e:
            logger.error(f"Error making API call: {str(e)}")
            raise
