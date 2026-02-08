import asyncio
import logging
import random
from typing import Any, Dict, List, cast

import httpx
from anthropic import Anthropic, AnthropicBedrock, AnthropicVertex
from anthropic.types.beta import BetaMessage, BetaMessageParam, BetaToolUnionParam

from ..types import LLMProvider
from .logging import log_api_interaction

logger = logging.getLogger(__name__)


class APIConnectionError(Exception):
    """Error raised when there are connection issues with the API."""

    pass


class BaseAnthropicClient:
    """Base class for Anthropic API clients."""

    MAX_RETRIES = 10
    INITIAL_RETRY_DELAY = 1.0
    MAX_RETRY_DELAY = 60.0
    JITTER_FACTOR = 0.1

    async def create_message(
        self,
        *,
        messages: list[BetaMessageParam],
        system: list[Any],
        tools: list[BetaToolUnionParam],
        max_tokens: int,
        betas: list[str],
    ) -> BetaMessage:
        """Create a message using the Anthropic API."""
        raise NotImplementedError

    async def _make_api_call_with_retries(self, api_call):
        """Make an API call with exponential backoff retry logic.

        Args:
            api_call: Async function that makes the actual API call

        Returns:
            API response

        Raises:
            APIConnectionError: If all retries fail
        """
        retry_count = 0
        last_error = None

        while retry_count < self.MAX_RETRIES:
            try:
                return await api_call()
            except Exception as e:
                last_error = e
                retry_count += 1

                if retry_count == self.MAX_RETRIES:
                    break

                # Calculate delay with exponential backoff and jitter
                delay = min(
                    self.INITIAL_RETRY_DELAY * (2 ** (retry_count - 1)), self.MAX_RETRY_DELAY
                )
                # Add jitter to avoid thundering herd
                jitter = delay * self.JITTER_FACTOR * (2 * random.random() - 1)
                final_delay = delay + jitter

                logger.info(
                    f"Retrying request (attempt {retry_count}/{self.MAX_RETRIES}) "
                    f"in {final_delay:.2f} seconds after error: {str(e)}"
                )
                await asyncio.sleep(final_delay)

        raise APIConnectionError(
            f"Failed after {self.MAX_RETRIES} retries. " f"Last error: {str(last_error)}"
        )

    async def run_interleaved(
        self, messages: List[Dict[str, Any]], system: str, max_tokens: int = 4096
    ) -> Any:
        """Run the Anthropic API with the Claude model, supports interleaved tool calling.

        Args:
            messages: List of message objects
            system: System prompt
            max_tokens: Maximum tokens to generate

        Returns:
            API response
        """
        # Add the tool_result check/fix logic here
        fixed_messages = self._fix_missing_tool_results(messages)

        # Get model name from concrete implementation if available
        model_name = getattr(self, "model", "unknown model")
        logger.info(f"Running Anthropic API call with model {model_name}")

        retry_count = 0

        while retry_count < self.MAX_RETRIES:
            try:
                # Call the Anthropic API through create_message which is implemented by subclasses
                # Convert system str to the list format expected by create_message
                system_list = [system]

                # Convert message format if needed - concrete implementations may do further conversion
                response = await self.create_message(
                    messages=cast(list[BetaMessageParam], fixed_messages),
                    system=system_list,
                    tools=[],  # Tools are included in the messages
                    max_tokens=max_tokens,
                    betas=["tools-2023-12-13"],
                )
                logger.info(f"Anthropic API call successful")
                return response
            except Exception as e:
                retry_count += 1
                wait_time = self.INITIAL_RETRY_DELAY * (
                    2 ** (retry_count - 1)
                )  # Exponential backoff
                logger.info(
                    f"Retrying request (attempt {retry_count}/{self.MAX_RETRIES}) in {wait_time:.2f} seconds after error: {str(e)}"
                )
                await asyncio.sleep(wait_time)

        # If we get here, all retries failed
        raise RuntimeError(f"Failed to call Anthropic API after {self.MAX_RETRIES} attempts")

    def _fix_missing_tool_results(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Check for and fix any missing tool_result blocks after tool_use blocks.

        Args:
            messages: List of message objects

        Returns:
            Fixed messages with proper tool_result blocks
        """
        fixed_messages = []
        pending_tool_uses = {}  # Map of tool_use IDs to their details

        for i, message in enumerate(messages):
            # Track any tool_use blocks in this message
            if message.get("role") == "assistant" and "content" in message:
                content = message.get("content", [])
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_use":
                        tool_id = block.get("id")
                        if tool_id:
                            pending_tool_uses[tool_id] = {
                                "name": block.get("name", ""),
                                "input": block.get("input", {}),
                            }

            # Check if this message handles any pending tool_use blocks
            if message.get("role") == "user" and "content" in message:
                # Check for tool_result blocks in this message
                content = message.get("content", [])
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_result":
                        tool_id = block.get("tool_use_id")
                        if tool_id in pending_tool_uses:
                            # This tool_result handles a pending tool_use
                            pending_tool_uses.pop(tool_id)

            # Add the message to our fixed list
            fixed_messages.append(message)

            # If this is an assistant message with tool_use blocks and there are
            # pending tool uses that need to be resolved before the next assistant message
            if (
                i + 1 < len(messages)
                and message.get("role") == "assistant"
                and messages[i + 1].get("role") == "assistant"
                and pending_tool_uses
            ):

                # We need to insert a user message with tool_results for all pending tool_uses
                tool_results = []
                for tool_id, tool_info in pending_tool_uses.items():
                    tool_results.append(
                        {
                            "type": "tool_result",
                            "tool_use_id": tool_id,
                            "content": {
                                "type": "error",
                                "message": "Tool execution was skipped or failed",
                            },
                        }
                    )

                # Insert a synthetic user message with the tool results
                if tool_results:
                    fixed_messages.append({"role": "user", "content": tool_results})

                # Clear pending tools since we've added results for them
                pending_tool_uses = {}

        # Check if there are any remaining pending tool_uses at the end of the conversation
        if pending_tool_uses and fixed_messages and fixed_messages[-1].get("role") == "assistant":
            # Add a final user message with tool results for any pending tool_uses
            tool_results = []
            for tool_id, tool_info in pending_tool_uses.items():
                tool_results.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": tool_id,
                        "content": {
                            "type": "error",
                            "message": "Tool execution was skipped or failed",
                        },
                    }
                )

            if tool_results:
                fixed_messages.append({"role": "user", "content": tool_results})

        return fixed_messages


class AnthropicDirectClient(BaseAnthropicClient):
    """Direct Anthropic API client implementation."""

    def __init__(self, api_key: str, model: str):
        self.model = model
        self.client = Anthropic(api_key=api_key, http_client=self._create_http_client())

    def _create_http_client(self) -> httpx.Client:
        """Create an HTTP client with appropriate settings."""
        return httpx.Client(
            verify=True,
            timeout=httpx.Timeout(connect=30.0, read=300.0, write=30.0, pool=30.0),
            transport=httpx.HTTPTransport(
                retries=3,
                verify=True,
                limits=httpx.Limits(max_keepalive_connections=5, max_connections=10),
            ),
        )

    async def create_message(
        self,
        *,
        messages: list[BetaMessageParam],
        system: list[Any],
        tools: list[BetaToolUnionParam],
        max_tokens: int,
        betas: list[str],
    ) -> BetaMessage:
        """Create a message using the direct Anthropic API with retry logic."""

        async def api_call():
            response = self.client.beta.messages.with_raw_response.create(
                max_tokens=max_tokens,
                messages=messages,
                model=self.model,
                system=system,
                tools=tools,
                betas=betas,
            )
            log_api_interaction(response.http_response.request, response.http_response, None)
            return response.parse()

        try:
            return await self._make_api_call_with_retries(api_call)
        except Exception as e:
            log_api_interaction(None, None, e)
            raise


class AnthropicVertexClient(BaseAnthropicClient):
    """Google Cloud Vertex AI implementation of Anthropic client."""

    def __init__(self, model: str):
        self.model = model
        self.client = AnthropicVertex()

    async def create_message(
        self,
        *,
        messages: list[BetaMessageParam],
        system: list[Any],
        tools: list[BetaToolUnionParam],
        max_tokens: int,
        betas: list[str],
    ) -> BetaMessage:
        """Create a message using Vertex AI with retry logic."""

        async def api_call():
            response = self.client.beta.messages.with_raw_response.create(
                max_tokens=max_tokens,
                messages=messages,
                model=self.model,
                system=system,
                tools=tools,
                betas=betas,
            )
            log_api_interaction(response.http_response.request, response.http_response, None)
            return response.parse()

        try:
            return await self._make_api_call_with_retries(api_call)
        except Exception as e:
            log_api_interaction(None, None, e)
            raise


class AnthropicBedrockClient(BaseAnthropicClient):
    """AWS Bedrock implementation of Anthropic client."""

    def __init__(self, model: str):
        self.model = model
        self.client = AnthropicBedrock()

    async def create_message(
        self,
        *,
        messages: list[BetaMessageParam],
        system: list[Any],
        tools: list[BetaToolUnionParam],
        max_tokens: int,
        betas: list[str],
    ) -> BetaMessage:
        """Create a message using AWS Bedrock with retry logic."""

        async def api_call():
            response = self.client.beta.messages.with_raw_response.create(
                max_tokens=max_tokens,
                messages=messages,
                model=self.model,
                system=system,
                tools=tools,
                betas=betas,
            )
            log_api_interaction(response.http_response.request, response.http_response, None)
            return response.parse()

        try:
            return await self._make_api_call_with_retries(api_call)
        except Exception as e:
            log_api_interaction(None, None, e)
            raise


class AnthropicClientFactory:
    """Factory for creating appropriate Anthropic client implementations."""

    @staticmethod
    def create_client(provider: LLMProvider, api_key: str, model: str) -> BaseAnthropicClient:
        """Create an appropriate client based on the provider."""
        if provider == LLMProvider.ANTHROPIC:
            return AnthropicDirectClient(api_key, model)
        elif provider == LLMProvider.VERTEX:
            return AnthropicVertexClient(model)
        elif provider == LLMProvider.BEDROCK:
            return AnthropicBedrockClient(model)
        raise ValueError(f"Unsupported provider: {provider}")
