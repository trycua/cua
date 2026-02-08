"""Response and tool handling for Anthropic provider."""

import logging
from typing import Any, Dict, List, Tuple, cast

from anthropic.types.beta import (
    BetaContentBlockParam,
    BetaMessage,
    BetaTextBlock,
)

from .tools import ToolResult

logger = logging.getLogger(__name__)


class AnthropicResponseHandler:
    """Handles Anthropic API responses and tool execution results."""

    def __init__(self, loop):
        """Initialize the response handler.

        Args:
            loop: Reference to the parent loop instance that provides context
        """
        self.loop = loop

    async def handle_response(
        self, response: BetaMessage, messages: List[Dict[str, Any]]
    ) -> Tuple[List[Dict[str, Any]], bool]:
        """Handle the Anthropic API response.

        Args:
            response: API response
            messages: List of messages for context

        Returns:
            Tuple containing:
            - List of new messages to be added
            - Boolean indicating if the loop should continue
        """
        try:
            new_messages = []

            # Convert response to parameter format
            response_params = self.response_to_params(response)

            # Collect all existing tool_use IDs from previous messages for validation
            existing_tool_use_ids = set()
            for msg in messages:
                if msg.get("role") == "assistant" and isinstance(msg.get("content"), list):
                    for block in msg.get("content", []):
                        if (
                            isinstance(block, dict)
                            and block.get("type") == "tool_use"
                            and "id" in block
                        ):
                            existing_tool_use_ids.add(block["id"])

            # Also add new tool_use IDs from the current response
            current_tool_use_ids = set()
            for block in response_params:
                if isinstance(block, dict) and block.get("type") == "tool_use" and "id" in block:
                    current_tool_use_ids.add(block["id"])
                    existing_tool_use_ids.add(block["id"])

            logger.info(f"Existing tool_use IDs in conversation: {existing_tool_use_ids}")
            logger.info(f"New tool_use IDs in current response: {current_tool_use_ids}")

            # Create assistant message
            new_messages.append(
                {
                    "role": "assistant",
                    "content": response_params,
                }
            )

            if self.loop.callback_manager is None:
                raise RuntimeError(
                    "Callback manager not initialized. Call initialize_client() first."
                )

            # Handle tool use blocks and collect results
            tool_result_content = []
            for content_block in response_params:
                # Notify callback of content
                self.loop.callback_manager.on_content(cast(BetaContentBlockParam, content_block))

                # Handle tool use
                if content_block.get("type") == "tool_use":
                    if self.loop.tool_manager is None:
                        raise RuntimeError(
                            "Tool manager not initialized. Call initialize_client() first."
                        )

                    # Execute the tool
                    result = await self.loop.tool_manager.execute_tool(
                        name=content_block["name"],
                        tool_input=cast(Dict[str, Any], content_block["input"]),
                    )

                    # Verify the tool_use ID exists in the conversation (which it should now)
                    tool_use_id = content_block["id"]
                    if tool_use_id in existing_tool_use_ids:
                        # Create tool result and add to content
                        tool_result = self.make_tool_result(cast(ToolResult, result), tool_use_id)
                        tool_result_content.append(tool_result)

                        # Notify callback of tool result
                        self.loop.callback_manager.on_tool_result(
                            cast(ToolResult, result), content_block["id"]
                        )
                    else:
                        logger.warning(
                            f"Tool use ID {tool_use_id} not found in previous messages. Skipping tool result."
                        )

            # If no tool results, we're done
            if not tool_result_content:
                # Signal completion
                self.loop.callback_manager.on_content({"type": "text", "text": "<DONE>"})
                return new_messages, False

            # Add tool results as user message
            new_messages.append({"content": tool_result_content, "role": "user"})
            return new_messages, True

        except Exception as e:
            logger.error(f"Error handling response: {str(e)}")
            new_messages.append(
                {
                    "role": "assistant",
                    "content": f"Error: {str(e)}",
                }
            )
            return new_messages, False

    def response_to_params(
        self,
        response: BetaMessage,
    ) -> List[Dict[str, Any]]:
        """Convert API response to message parameters.

        Args:
            response: API response message

        Returns:
            List of content blocks
        """
        result = []
        for block in response.content:
            if isinstance(block, BetaTextBlock):
                result.append({"type": "text", "text": block.text})
            else:
                result.append(cast(Dict[str, Any], block.model_dump()))
        return result

    def make_tool_result(self, result: ToolResult, tool_use_id: str) -> Dict[str, Any]:
        """Convert a tool result to API format.

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
                    "text": self.maybe_prepend_system_tool_result(result, result.error),
                }
            ]
        else:
            if result.output:
                tool_result_content.append(
                    {
                        "type": "text",
                        "text": self.maybe_prepend_system_tool_result(result, result.output),
                    }
                )
            if result.base64_image:
                tool_result_content.append(
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": result.base64_image,
                        },
                    }
                )

        return {
            "type": "tool_result",
            "content": tool_result_content,
            "tool_use_id": tool_use_id,
            "is_error": is_error,
        }

    def maybe_prepend_system_tool_result(self, result: ToolResult, result_text: str) -> str:
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
