"""Message handling utilities for agent."""

import json
import logging
import re
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple, Union

logger = logging.getLogger(__name__)


@dataclass
class ImageRetentionConfig:
    """Configuration for image retention in messages."""

    num_images_to_keep: Optional[int] = None
    min_removal_threshold: int = 1
    enable_caching: bool = True

    def should_retain_images(self) -> bool:
        """Check if image retention is enabled."""
        return self.num_images_to_keep is not None and self.num_images_to_keep > 0

class StandardMessageManager:
    """Manages messages in a standardized OpenAI format across different providers."""

    def __init__(self, config: Optional[ImageRetentionConfig] = None):
        """Initialize message manager.

        Args:
            config: Configuration for image retention
        """
        self.messages: List[Dict[str, Any]] = []
        self.config = config or ImageRetentionConfig()

    def add_user_message(self, content: Union[str, List[Dict[str, Any]]]) -> None:
        """Add a user message.

        Args:
            content: Message content (text or multimodal content)
        """
        self.messages.append({"role": "user", "content": content})

    def add_assistant_message(self, content: Union[str, List[Dict[str, Any]]]) -> None:
        """Add an assistant message.

        Args:
            content: Message content (text or multimodal content)
        """
        self.messages.append({"role": "assistant", "content": content})

    def add_system_message(self, content: str) -> None:
        """Add a system message.

        Args:
            content: System message content
        """
        self.messages.append({"role": "system", "content": content})

    def get_messages(self) -> List[Dict[str, Any]]:
        """Get all messages in standard format.
        This method applies image retention policy if configured.

        Returns:
            List of messages
        """
        # If image retention is configured, apply it
        if self.config.num_images_to_keep is not None:
            return self._apply_image_retention(self.messages)
        return self.messages

    def _apply_image_retention(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Apply image retention policy to messages.

        Args:
            messages: List of messages

        Returns:
            List of messages with image retention applied
        """
        if not self.config.num_images_to_keep:
            return messages

        # Find messages with images (both user messages and tool call outputs)
        image_messages = []
        for msg in messages:
            has_image = False
            
            # Check user messages with images
            if msg["role"] == "user" and isinstance(msg["content"], list):
                has_image = any(
                    item.get("type") == "image_url" or item.get("type") == "image"
                    for item in msg["content"]
                )
            
            # Check assistant messages with tool calls that have images
            elif msg["role"] == "assistant" and isinstance(msg["content"], list):
                for item in msg["content"]:
                    if item.get("type") == "tool_result" and "base64_image" in item:
                        has_image = True
                        break
            
            if has_image:
                image_messages.append(msg)

        # If we don't have more images than the limit, return all messages
        if len(image_messages) <= self.config.num_images_to_keep:
            return messages

        # Get the most recent N images to keep
        images_to_keep = image_messages[-self.config.num_images_to_keep :]
        images_to_remove = image_messages[: -self.config.num_images_to_keep]

        # Create a new message list, removing images from older messages
        result = []
        for msg in messages:
            if msg in images_to_remove:
                # Remove images from this message but keep the text content
                if msg["role"] == "user" and isinstance(msg["content"], list):
                    # Keep only text content, remove images
                    new_content = [
                        item for item in msg["content"] 
                        if item.get("type") not in ["image_url", "image"]
                    ]
                    if new_content:  # Only add if there's still content
                        result.append({"role": msg["role"], "content": new_content})
                elif msg["role"] == "assistant" and isinstance(msg["content"], list):
                    # Remove base64_image from tool_result items
                    new_content = []
                    for item in msg["content"]:
                        if item.get("type") == "tool_result" and "base64_image" in item:
                            # Create a copy without the base64_image
                            new_item = {k: v for k, v in item.items() if k != "base64_image"}
                            new_content.append(new_item)
                        else:
                            new_content.append(item)
                    result.append({"role": msg["role"], "content": new_content})
                else:
                    # For other message types, keep as is
                    result.append(msg)
            else:
                result.append(msg)

        return result

    def to_anthropic_format(
        self, messages: List[Dict[str, Any]]
    ) -> Tuple[List[Dict[str, Any]], str]:
        """Convert standard OpenAI format messages to Anthropic format.

        Args:
            messages: List of messages in OpenAI format

        Returns:
            Tuple containing (anthropic_messages, system_content)
        """
        result = []
        system_content = ""

        # Process messages in order to maintain conversation flow
        previous_assistant_tool_use_ids = (
            set()
        )  # Track tool_use_ids in the previous assistant message

        for i, msg in enumerate(messages):
            role = msg.get("role", "")
            content = msg.get("content", "")

            if role == "system":
                # Collect system messages for later use
                system_content += content + "\n"
                continue

            if role == "assistant":
                # Track tool_use_ids in this assistant message for the next user message
                previous_assistant_tool_use_ids = set()
                if isinstance(content, list):
                    for item in content:
                        if (
                            isinstance(item, dict)
                            and item.get("type") == "tool_use"
                            and "id" in item
                        ):
                            previous_assistant_tool_use_ids.add(item["id"])

                logger.info(
                    f"Tool use IDs in assistant message #{i}: {previous_assistant_tool_use_ids}"
                )

            if role in ["user", "assistant"]:
                anthropic_msg = {"role": role}

                # Convert content based on type
                if isinstance(content, str):
                    # Simple text content
                    anthropic_msg["content"] = [{"type": "text", "text": content}]
                elif isinstance(content, list):
                    # Convert complex content
                    anthropic_content = []
                    for item in content:
                        item_type = item.get("type", "")

                        if item_type == "text":
                            anthropic_content.append({"type": "text", "text": item.get("text", "")})
                        elif item_type == "image_url":
                            # Convert OpenAI image format to Anthropic
                            image_url = item.get("image_url", {}).get("url", "")
                            if image_url.startswith("data:"):
                                # Extract base64 data and media type
                                match = re.match(r"data:(.+);base64,(.+)", image_url)
                                if match:
                                    media_type, data = match.groups()
                                    anthropic_content.append(
                                        {
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": media_type,
                                                "data": data,
                                            },
                                        }
                                    )
                            else:
                                # Regular URL
                                anthropic_content.append(
                                    {
                                        "type": "image",
                                        "source": {
                                            "type": "url",
                                            "url": image_url,
                                        },
                                    }
                                )
                        elif item_type == "tool_use":
                            # Always include tool_use blocks
                            anthropic_content.append(item)
                        elif item_type == "tool_result":
                            # Check if this is a user message AND if the tool_use_id exists in the previous assistant message
                            tool_use_id = item.get("tool_use_id")

                            # Only include tool_result if it references a tool_use from the immediately preceding assistant message
                            if (
                                role == "user"
                                and tool_use_id
                                and tool_use_id in previous_assistant_tool_use_ids
                            ):
                                anthropic_content.append(item)
                                logger.info(
                                    f"Including tool_result with tool_use_id: {tool_use_id}"
                                )
                            else:
                                # Convert to text to preserve information
                                logger.warning(
                                    f"Converting tool_result to text. Tool use ID {tool_use_id} not found in previous assistant message"
                                )
                                content_text = "Tool Result: "
                                if "content" in item:
                                    if isinstance(item["content"], list):
                                        for content_item in item["content"]:
                                            if (
                                                isinstance(content_item, dict)
                                                and content_item.get("type") == "text"
                                            ):
                                                content_text += content_item.get("text", "")
                                    elif isinstance(item["content"], str):
                                        content_text += item["content"]
                                anthropic_content.append({"type": "text", "text": content_text})

                    anthropic_msg["content"] = anthropic_content

                result.append(anthropic_msg)

        return result, system_content

    def from_anthropic_format(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Convert Anthropic format messages to standard OpenAI format.

        Args:
            messages: List of messages in Anthropic format

        Returns:
            List of messages in OpenAI format
        """
        result = []

        for msg in messages:
            role = msg.get("role", "")
            content = msg.get("content", [])

            if role in ["user", "assistant"]:
                openai_msg = {"role": role}

                # Simple case: single text block
                if len(content) == 1 and content[0].get("type") == "text":
                    openai_msg["content"] = content[0].get("text", "")
                else:
                    # Complex case: multiple blocks or non-text
                    openai_content = []
                    for item in content:
                        item_type = item.get("type", "")

                        if item_type == "text":
                            openai_content.append({"type": "text", "text": item.get("text", "")})
                        elif item_type == "image":
                            # Convert Anthropic image to OpenAI format
                            source = item.get("source", {})
                            if source.get("type") == "base64":
                                media_type = source.get("media_type", "image/png")
                                data = source.get("data", "")
                                openai_content.append(
                                    {
                                        "type": "image_url",
                                        "image_url": {"url": f"data:{media_type};base64,{data}"},
                                    }
                                )
                            else:
                                # URL
                                openai_content.append(
                                    {
                                        "type": "image_url",
                                        "image_url": {"url": source.get("url", "")},
                                    }
                                )
                        elif item_type in ["tool_use", "tool_result"]:
                            # Pass through tool-related content
                            openai_content.append(item)

                    openai_msg["content"] = openai_content

                result.append(openai_msg)

        return result
