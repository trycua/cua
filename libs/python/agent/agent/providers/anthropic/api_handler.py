"""API call handling for Anthropic provider."""

import asyncio
import logging
from typing import List

from anthropic.types.beta import (
    BetaMessage,
    BetaMessageParam,
    BetaTextBlockParam,
)

from .prompts import SYSTEM_PROMPT
from .types import LLMProvider

# Constants
COMPUTER_USE_BETA_FLAG = "computer-use-2025-01-24"
PROMPT_CACHING_BETA_FLAG = "prompt-caching-2024-07-31"

logger = logging.getLogger(__name__)


class AnthropicAPIHandler:
    """Handles API calls to Anthropic's API with structured error handling and retries."""

    def __init__(self, loop):
        """Initialize the API handler.

        Args:
            loop: Reference to the parent loop instance that provides context
        """
        self.loop = loop

    async def make_api_call(
        self, messages: List[BetaMessageParam], system_prompt: str = SYSTEM_PROMPT
    ) -> BetaMessage:
        """Make API call to Anthropic with retry logic.

        Args:
            messages: List of messages to send to the API
            system_prompt: System prompt to use (default: SYSTEM_PROMPT)

        Returns:
            API response

        Raises:
            RuntimeError: If API call fails after all retries
        """
        if self.loop.client is None:
            raise RuntimeError("Client not initialized. Call initialize_client() first.")
        if self.loop.tool_manager is None:
            raise RuntimeError("Tool manager not initialized. Call initialize_client() first.")

        last_error = None

        # Add detailed debug logging to examine messages
        logger.info(f"Sending {len(messages)} messages to Anthropic API")

        # Log tool use IDs and tool result IDs for debugging
        tool_use_ids = set()
        tool_result_ids = set()

        for i, msg in enumerate(messages):
            logger.info(f"Message {i}: role={msg.get('role')}")
            if isinstance(msg.get("content"), list):
                for content_block in msg.get("content", []):
                    if isinstance(content_block, dict):
                        block_type = content_block.get("type")
                        if block_type == "tool_use" and "id" in content_block:
                            tool_id = content_block.get("id")
                            tool_use_ids.add(tool_id)
                            logger.info(f"  - Found tool_use with ID: {tool_id}")
                        elif block_type == "tool_result" and "tool_use_id" in content_block:
                            result_id = content_block.get("tool_use_id")
                            tool_result_ids.add(result_id)
                            logger.info(f"  - Found tool_result referencing ID: {result_id}")

        # Check for mismatches
        missing_tool_uses = tool_result_ids - tool_use_ids
        if missing_tool_uses:
            logger.warning(
                f"Found tool_result IDs without matching tool_use IDs: {missing_tool_uses}"
            )

        for attempt in range(self.loop.max_retries):
            try:
                # Log request
                request_data = {
                    "messages": messages,
                    "max_tokens": self.loop.max_tokens,
                    "system": system_prompt,
                }
                # Let ExperimentManager handle sanitization
                self.loop._log_api_call("request", request_data)

                # Setup betas and system
                system = BetaTextBlockParam(
                    type="text",
                    text=system_prompt,
                )

                betas = [COMPUTER_USE_BETA_FLAG]
                # Add prompt caching if enabled in the message manager's config
                if self.loop.message_manager.config.enable_caching:
                    betas.append(PROMPT_CACHING_BETA_FLAG)
                    system["cache_control"] = {"type": "ephemeral"}

                # Make API call
                response = await self.loop.client.create_message(
                    messages=messages,
                    system=[system],
                    tools=self.loop.tool_manager.get_tool_params(),
                    max_tokens=self.loop.max_tokens,
                    betas=betas,
                )

                # Let ExperimentManager handle sanitization
                self.loop._log_api_call("response", request_data, response)

                return response
            except Exception as e:
                last_error = e
                logger.error(
                    f"Error in API call (attempt {attempt + 1}/{self.loop.max_retries}): {str(e)}"
                )
                self.loop._log_api_call("error", {"messages": messages}, error=e)

                if attempt < self.loop.max_retries - 1:
                    await asyncio.sleep(
                        self.loop.retry_delay * (attempt + 1)
                    )  # Exponential backoff
                continue

        # If we get here, all retries failed
        error_message = f"API call failed after {self.loop.max_retries} attempts"
        if last_error:
            error_message += f": {str(last_error)}"

        logger.error(error_message)
        raise RuntimeError(error_message)
