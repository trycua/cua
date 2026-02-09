"""Response handler for the OpenAI provider."""

import asyncio
import base64
import logging
import traceback
from typing import TYPE_CHECKING, Any, AsyncGenerator, Dict, List, Optional

from ...core.types import AgentResponse
from .types import ResponseItemType

if TYPE_CHECKING:
    from .loop import OpenAILoop

logger = logging.getLogger(__name__)


class OpenAIResponseHandler:
    """Handler for OpenAI API responses."""

    def __init__(self, loop: "OpenAILoop"):
        """Initialize the response handler.

        Args:
            loop: OpenAI loop instance
        """
        self.loop = loop
        logger.info("Initialized OpenAI response handler")

    async def process_response(self, response: Dict[str, Any], queue: asyncio.Queue) -> None:
        """Process the response from the OpenAI API.

        Args:
            response: Response from the API
            queue: Queue for response streaming
        """
        try:
            # Get output items
            output_items = response.get("output", []) or []

            # Process each output item
            for item in output_items:
                if not isinstance(item, dict):
                    continue

                item_type = item.get("type")

                # For computer_call items, we only need to add to the queue
                # The loop is now handling executing the action and creating the computer_call_output
                if item_type == ResponseItemType.COMPUTER_CALL:
                    # Send computer_call to queue so it can be processed
                    await queue.put(item)

                elif item_type == ResponseItemType.MESSAGE:
                    # Send message to queue
                    await queue.put(item)

                elif item_type == ResponseItemType.REASONING:
                    # Process reasoning summary
                    summary = None
                    if "summary" in item and isinstance(item["summary"], list):
                        for summary_item in item["summary"]:
                            if (
                                isinstance(summary_item, dict)
                                and summary_item.get("type") == "summary_text"
                            ):
                                summary = summary_item.get("text")
                                break

                    if summary:
                        # Log the reasoning summary
                        logger.info(f"Reasoning summary: {summary}")

                        # Send reasoning summary to queue with a special format
                        await queue.put(
                            {
                                "role": "assistant",
                                "content": f"[Reasoning: {summary}]",
                                "metadata": {"title": "💭 Reasoning", "is_summary": True},
                            }
                        )

                    # Also pass the original reasoning item to the queue for complete context
                    await queue.put(item)

        except Exception as e:
            logger.error(f"Error processing response: {str(e)}")
            await queue.put(
                {
                    "role": "assistant",
                    "content": f"Error processing response: {str(e)}",
                    "metadata": {"title": "❌ Error"},
                }
            )

    def _process_message_item(self, item: Dict[str, Any]) -> AgentResponse:
        """Process a message item from the response.

        Args:
            item: Message item from the response

        Returns:
            Processed message in AgentResponse format
        """
        # Extract content items - add null check
        content_items = item.get("content", []) or []

        # Extract text from content items - use output_text type from OpenAI
        text = ""
        for content_item in content_items:
            # Skip if content_item is None or not a dict
            if content_item is None or not isinstance(content_item, dict):
                continue

            # In OpenAI Agent Response API, text content is in "output_text" type items
            if content_item.get("type") == "output_text":
                text += content_item.get("text", "")

        # Create agent response
        return {
            "role": "assistant",
            "content": text
            or "I don't have a response for that right now.",  # Provide fallback when text is empty
            "metadata": {"title": "💬 Response"},
        }

    async def _process_computer_call(self, item: Dict[str, Any], queue: asyncio.Queue) -> None:
        """Process a computer call item from the response.

        Args:
            item: Computer call item
            queue: Queue to add responses to
        """
        try:
            # Log the computer call
            action = item.get("action", {}) or {}
            if not isinstance(action, dict):
                logger.warning(f"Expected dict for action, got {type(action)}")
                action = {}

            action_type = action.get("type", "unknown")
            logger.info(f"Processing computer call: {action_type}")

            # Execute the tool call
            result = await self.loop.tool_manager.execute_tool("computer", action)

            # Add any message to the conversation history and queue
            if result and result.base64_image:
                # Update message history with the call output
                self.loop.message_manager.add_user_message(
                    [{"type": "text", "text": f"[Computer action completed: {action_type}]"}]
                )

                # Add image to messages (using correct content types for Agent Response API)
                self.loop.message_manager.add_user_message(
                    [
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": result.base64_image,
                            },
                        }
                    ]
                )

                # If browser environment, include URL if available
                # if (
                #     hasattr(self.loop.computer, "environment")
                #     and self.loop.computer.environment == "browser"
                # ):
                #     try:
                #         if hasattr(self.loop.computer.interface, "get_current_url"):
                #             current_url = await self.loop.computer.interface.get_current_url()
                #             self.loop.message_manager.add_user_message(
                #                 [
                #                     {
                #                         "type": "text",
                #                         "text": f"Current URL: {current_url}",
                #                     }
                #                 ]
                #             )
                #     except Exception as e:
                #         logger.warning(f"Failed to get current URL: {str(e)}")

            # Log successful completion
            logger.info(f"Computer call {action_type} executed successfully")

        except Exception as e:
            logger.error(f"Error executing computer call: {str(e)}")
            logger.debug(traceback.format_exc())

            # Add error to conversation
            self.loop.message_manager.add_user_message(
                [{"type": "text", "text": f"Error executing computer action: {str(e)}"}]
            )

            # Send error to queue
            error_response = {
                "role": "assistant",
                "content": f"Error executing computer action: {str(e)}",
                "metadata": {"title": "❌ Error"},
            }
            await queue.put(error_response)
