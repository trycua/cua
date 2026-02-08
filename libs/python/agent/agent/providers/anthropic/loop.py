"""Anthropic-specific agent loop implementation."""

import asyncio
import base64
import logging
from datetime import datetime
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, cast

from anthropic.types.beta import (
    BetaContentBlockParam,
    BetaMessage,
    BetaMessageParam,
    BetaTextBlock,
)

# Computer
from computer import Computer

# Base imports
from ...core.base import BaseLoop
from ...core.messages import ImageRetentionConfig, StandardMessageManager
from ...core.types import AgentResponse

# Anthropic provider-specific imports
from .api.client import AnthropicClientFactory, BaseAnthropicClient

# Import the new modules we created
from .api_handler import AnthropicAPIHandler
from .callbacks.manager import CallbackManager
from .prompts import SYSTEM_PROMPT
from .response_handler import AnthropicResponseHandler
from .tools import ToolResult
from .tools.manager import ToolManager
from .types import LLMProvider
from .utils import to_agent_response_format, to_anthropic_format

# Constants
COMPUTER_USE_BETA_FLAG = "computer-use-2025-01-24"
PROMPT_CACHING_BETA_FLAG = "prompt-caching-2024-07-31"

logger = logging.getLogger(__name__)


class AnthropicLoop(BaseLoop):
    """Anthropic-specific implementation of the agent loop.

    This class extends BaseLoop to provide specialized support for Anthropic's Claude models
    with their unique tool-use capabilities, custom message formatting, and
    callback-driven approach to handling responses.
    """

    ###########################################
    # INITIALIZATION AND CONFIGURATION
    ###########################################

    def __init__(
        self,
        api_key: str,
        computer: Computer,
        model: str = "claude-3-7-sonnet-20250219",
        only_n_most_recent_images: Optional[int] = 2,
        base_dir: Optional[str] = "trajectories",
        max_retries: int = 3,
        retry_delay: float = 1.0,
        save_trajectory: bool = True,
        **kwargs,
    ):
        """Initialize the Anthropic loop.

        Args:
            api_key: Anthropic API key
            model: Model name (fixed to claude-3-7-sonnet-20250219)
            computer: Computer instance
            only_n_most_recent_images: Maximum number of recent screenshots to include in API requests
            base_dir: Base directory for saving experiment data
            max_retries: Maximum number of retries for API calls
            retry_delay: Delay between retries in seconds
            save_trajectory: Whether to save trajectory data
        """
        # Initialize base class with core config
        super().__init__(
            computer=computer,
            model=model,
            api_key=api_key,
            max_retries=max_retries,
            retry_delay=retry_delay,
            base_dir=base_dir,
            save_trajectory=save_trajectory,
            only_n_most_recent_images=only_n_most_recent_images,
            **kwargs,
        )

        # Initialize message manager
        self.message_manager = StandardMessageManager(
            config=ImageRetentionConfig(num_images_to_keep=only_n_most_recent_images)
        )

        # Anthropic-specific attributes
        self.provider = LLMProvider.ANTHROPIC
        self.client = None
        self.retry_count = 0
        self.tool_manager = None
        self.callback_manager = None
        self.queue = asyncio.Queue()  # Initialize queue
        self.loop_task = None  # Store the loop task for cancellation

        # Initialize handlers
        self.api_handler = AnthropicAPIHandler(self)
        self.response_handler = AnthropicResponseHandler(self)

    ###########################################
    # CLIENT INITIALIZATION - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def initialize_client(self) -> None:
        """Initialize the Anthropic API client and tools.

        Implements abstract method from BaseLoop to set up the Anthropic-specific
        client, tool manager, message manager, and callback handlers.
        """
        try:
            logger.info(f"Initializing Anthropic client with model {self.model}...")

            # Initialize client
            self.client = AnthropicClientFactory.create_client(
                provider=self.provider, api_key=self.api_key, model=self.model
            )

            # Initialize callback manager with our callback handlers
            self.callback_manager = CallbackManager(
                content_callback=self._handle_content,
                tool_callback=self._handle_tool_result,
                api_callback=self._handle_api_interaction,
            )

            # Initialize tool manager
            self.tool_manager = ToolManager(self.computer)
            await self.tool_manager.initialize()

            logger.info(f"Initialized Anthropic client with model {self.model}")
        except Exception as e:
            logger.error(f"Error initializing Anthropic client: {str(e)}")
            self.client = None
            raise RuntimeError(f"Failed to initialize Anthropic client: {str(e)}")

    ###########################################
    # MAIN LOOP - IMPLEMENTING ABSTRACT METHOD
    ###########################################

    async def run(self, messages: List[Dict[str, Any]]) -> AsyncGenerator[AgentResponse, None]:
        """Run the agent loop with provided messages.

        Args:
            messages: List of message objects in standard OpenAI format

        Yields:
            Agent response format
        """
        try:
            logger.info("Starting Anthropic loop run")

            # Create queue for response streaming
            queue = asyncio.Queue()

            # Ensure client is initialized
            if self.client is None or self.tool_manager is None:
                logger.info("Initializing client...")
                await self.initialize_client()
                if self.client is None:
                    raise RuntimeError("Failed to initialize client")
                logger.info("Client initialized successfully")

            # Start loop in background task
            self.loop_task = asyncio.create_task(self._run_loop(queue, messages))

            # Process and yield messages as they arrive
            while True:
                try:
                    item = await queue.get()
                    if item is None:  # Stop signal
                        break
                    yield item
                    queue.task_done()
                except Exception as e:
                    logger.error(f"Error processing queue item: {str(e)}")
                    continue

            # Wait for loop to complete
            await self.loop_task

            # Send completion message
            yield {
                "role": "assistant",
                "content": "Task completed successfully.",
                "metadata": {"title": "✅ Complete"},
            }

        except Exception as e:
            logger.error(f"Error executing task: {str(e)}")
            yield {
                "role": "assistant",
                "content": f"Error: {str(e)}",
                "metadata": {"title": "❌ Error"},
            }
            
    async def cancel(self) -> None:
        """Cancel the currently running agent loop task.
        
        This method stops the ongoing processing in the agent loop
        by cancelling the loop_task if it exists and is running.
        """
        if self.loop_task and not self.loop_task.done():
            logger.info("Cancelling Anthropic loop task")
            self.loop_task.cancel()
            try:
                # Wait for the task to be cancelled with a timeout
                await asyncio.wait_for(self.loop_task, timeout=2.0)
            except asyncio.TimeoutError:
                logger.warning("Timeout while waiting for loop task to cancel")
            except asyncio.CancelledError:
                logger.info("Loop task cancelled successfully")
            except Exception as e:
                logger.error(f"Error while cancelling loop task: {str(e)}")
            finally:
                # Put None in the queue to signal any waiting consumers to stop
                await self.queue.put(None)
                logger.info("Anthropic loop task cancelled")
        else:
            logger.info("No active Anthropic loop task to cancel")

    ###########################################
    # AGENT LOOP IMPLEMENTATION
    ###########################################

    async def _run_loop(self, queue: asyncio.Queue, messages: List[Dict[str, Any]]) -> None:
        """Run the agent loop with provided messages.

        Args:
            queue: Queue for response streaming
            messages: List of messages in standard OpenAI format
        """
        try:
            while True:
                # Capture screenshot
                try:
                    # Take screenshot - always returns raw PNG bytes
                    screenshot = await self.computer.interface.screenshot()
                    logger.info("Screenshot captured successfully")

                    # Convert PNG bytes to base64
                    base64_image = base64.b64encode(screenshot).decode("utf-8")
                    logger.info(f"Screenshot converted to base64 (size: {len(base64_image)} bytes)")

                    # Save screenshot if requested
                    if self.save_trajectory and self.experiment_manager:
                        try:
                            self._save_screenshot(base64_image, action_type="state")
                            logger.info("Screenshot saved to trajectory")
                        except Exception as e:
                            logger.error(f"Error saving screenshot: {str(e)}")

                    # Create screenshot message
                    screen_info_msg = {
                        "role": "user",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": base64_image,
                                },
                            }
                        ],
                    }
                    # Add screenshot to messages
                    messages.append(screen_info_msg)
                    logger.info("Screenshot message added to conversation")

                except Exception as e:
                    logger.error(f"Error capturing or processing screenshot: {str(e)}")
                    raise

                # Create new turn directory for this API call
                self._create_turn_dir()


                # Apply image retention policy
                self.message_manager.messages = messages.copy()
                prepared_messages = self.message_manager.get_messages()
                # Convert standard messages to Anthropic format using utility function
                anthropic_messages, system_content = to_anthropic_format(prepared_messages)

                # Use API handler to make API call with Anthropic format
                response = await self.api_handler.make_api_call(
                    messages=cast(List[BetaMessageParam], anthropic_messages),
                    system_prompt=system_content or SYSTEM_PROMPT,
                )

                # Use response handler to handle the response and get new messages
                new_messages, should_continue = await self.response_handler.handle_response(
                    response, messages
                )

                # Add new messages to the parent's message history
                messages.extend(new_messages)

                openai_compatible_response = await to_agent_response_format(
                    response,
                    messages,
                    model=self.model,
                )
                # Log standardized response for ease of parsing
                self._log_api_call("agent_response", request=None, response=openai_compatible_response)
                await queue.put(openai_compatible_response)

                if not should_continue:
                    break

            # Signal completion
            await queue.put(None)

        except Exception as e:
            logger.error(f"Error in _run_loop: {str(e)}")
            await queue.put(
                {
                    "role": "assistant",
                    "content": f"Error in agent loop: {str(e)}",
                    "metadata": {"title": "❌ Error"},
                }
            )
            await queue.put(None)

    ###########################################
    # RESPONSE AND CALLBACK HANDLING
    ###########################################

    async def _handle_response(self, response: BetaMessage, messages: List[Dict[str, Any]]) -> bool:
        """Handle a response from the Anthropic API.

        Args:
            response: The response from the Anthropic API
            messages: The message history

        Returns:
            bool: Whether to continue the conversation
        """
        try:
            # Convert response to standard format
            openai_compatible_response = await to_agent_response_format(
                response,
                messages,
                model=self.model,
            )

            # Put the response on the queue
            await self.queue.put(openai_compatible_response)

            if self.callback_manager is None:
                raise RuntimeError(
                    "Callback manager not initialized. Call initialize_client() first."
                )

            # Handle tool use blocks and collect ALL results before adding to messages
            tool_result_content = []
            has_tool_use = False

            for content_block in response.content:
                # Notify callback of content
                self.callback_manager.on_content(cast(BetaContentBlockParam, content_block))

                # Handle tool use - carefully check and access attributes
                if hasattr(content_block, "type") and content_block.type == "tool_use":
                    has_tool_use = True
                    if self.tool_manager is None:
                        raise RuntimeError(
                            "Tool manager not initialized. Call initialize_client() first."
                        )

                    # Safely get attributes
                    tool_name = getattr(content_block, "name", "")
                    tool_input = getattr(content_block, "input", {})
                    tool_id = getattr(content_block, "id", "")

                    result = await self.tool_manager.execute_tool(
                        name=tool_name,
                        tool_input=cast(Dict[str, Any], tool_input),
                    )

                    # Create tool result
                    tool_result = self._make_tool_result(cast(ToolResult, result), tool_id)
                    tool_result_content.append(tool_result)

                    # Notify callback of tool result
                    self.callback_manager.on_tool_result(cast(ToolResult, result), tool_id)

            # If we had any tool_use blocks, we MUST add the tool_result message
            # even if there were errors or no actual results
            if has_tool_use:
                # If somehow we have no tool results but had tool uses, add synthetic error results
                if not tool_result_content:
                    logger.warning(
                        "Had tool uses but no tool results, adding synthetic error results"
                    )
                    for content_block in response.content:
                        if hasattr(content_block, "type") and content_block.type == "tool_use":
                            tool_id = getattr(content_block, "id", "")
                            if tool_id:
                                tool_result_content.append(
                                    {
                                        "type": "tool_result",
                                        "tool_use_id": tool_id,
                                        "content": {
                                            "type": "error",
                                            "text": "Tool execution was skipped or failed",
                                        },
                                        "is_error": True,
                                    }
                                )

                # Add ALL tool results as a SINGLE user message
                messages.append({"role": "user", "content": tool_result_content})
                return True
            else:
                # No tool uses, we're done
                self.callback_manager.on_content({"type": "text", "text": "<DONE>"})
                return False

        except Exception as e:
            logger.error(f"Error handling response: {str(e)}")
            messages.append(
                {
                    "role": "assistant",
                    "content": f"Error: {str(e)}",
                }
            )
            return False

    def _response_to_blocks(self, response: BetaMessage) -> List[Dict[str, Any]]:
        """Convert Anthropic API response to standard blocks format.

        Args:
            response: API response message

        Returns:
            List of content blocks in standard format
        """
        result = []
        for block in response.content:
            if isinstance(block, BetaTextBlock):
                result.append({"type": "text", "text": block.text})
            elif hasattr(block, "type") and block.type == "tool_use":
                # Safely access attributes after confirming it's a tool_use
                result.append(
                    {
                        "type": "tool_use",
                        "id": getattr(block, "id", ""),
                        "name": getattr(block, "name", ""),
                        "input": getattr(block, "input", {}),
                    }
                )
            else:
                # For other block types, convert to dict
                block_dict = {}
                for key, value in vars(block).items():
                    if not key.startswith("_"):
                        block_dict[key] = value
                result.append(block_dict)

        return result

    def _make_tool_result(self, result: ToolResult, tool_use_id: str) -> Dict[str, Any]:
        """Convert a tool result to standard format.

        Args:
            result: Tool execution result
            tool_use_id: ID of the tool use

        Returns:
            Formatted tool result
        """
        if result.content:
            return {
                "type": "tool_result",
                "content": result.content,
                "tool_use_id": tool_use_id,
                "is_error": bool(result.error),
            }

        tool_result_content = []
        is_error = False

        if result.error:
            is_error = True
            tool_result_content = [
                {
                    "type": "text",
                    "text": self._maybe_prepend_system_tool_result(result, result.error),
                }
            ]
        else:
            if result.output:
                tool_result_content.append(
                    {
                        "type": "text",
                        "text": self._maybe_prepend_system_tool_result(result, result.output),
                    }
                )
            if result.base64_image:
                tool_result_content.append(
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{result.base64_image}"},
                    }
                )

        return {
            "type": "tool_result",
            "content": tool_result_content,
            "tool_use_id": tool_use_id,
            "is_error": is_error,
        }

    def _maybe_prepend_system_tool_result(self, result: ToolResult, result_text: str) -> str:
        """Prepend system information to tool result if available.

        Args:
            result: Tool execution result
            result_text: Text to prepend to

        Returns:
            Text with system information prepended if available
        """
        if result.system:
            result_text = f"<s>{result.system}</s>\n{result_text}"
        return result_text

    ###########################################
    # CALLBACK HANDLERS
    ###########################################

    def _handle_content(self, content):
        """Handle content updates from the assistant."""
        if content.get("type") == "text":
            text = content.get("text", "")
            if text == "<DONE>":
                return
            logger.info(f"Assistant: {text}")

    def _handle_tool_result(self, result, tool_id):
        """Handle tool execution results."""
        if result.error:
            logger.error(f"Tool {tool_id} error: {result.error}")
        else:
            logger.info(f"Tool {tool_id} output: {result.output}")

    def _handle_api_interaction(
        self, request: Any, response: Any, error: Optional[Exception]
    ) -> None:
        """Handle API interactions."""
        if error:
            logger.error(f"API error: {error}")
            self._log_api_call("error", request, error=error)
        else:
            logger.debug(f"API request: {request}")
            if response:
                self._log_api_call("response", request, response)
            else:
                self._log_api_call("request", request)
