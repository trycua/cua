"""Utility functions for the OpenAI provider."""

import base64
import json
import logging
from typing import Any, Dict, List, Optional

from ...core.types import AgentResponse

logger = logging.getLogger(__name__)


def format_images_for_openai(images_base64: List[str]) -> List[Dict[str, Any]]:
    """Format images for OpenAI Agent Response API.

    Args:
        images_base64: List of base64 encoded images

    Returns:
        List of formatted image items for Agent Response API
    """
    return [
        {"type": "input_image", "image_url": f"data:image/png;base64,{image}"}
        for image in images_base64
    ]


def extract_message_content(message: Dict[str, Any]) -> str:
    """Extract text content from a message.

    Args:
        message: Message to extract content from

    Returns:
        Text content from the message
    """
    if isinstance(message.get("content"), str):
        return message["content"]

    if isinstance(message.get("content"), list):
        text = ""
        role = message.get("role", "user")

        for item in message["content"]:
            if isinstance(item, dict):
                # For user messages
                if role == "user" and item.get("type") == "input_text":
                    text += item.get("text", "")
                # For standard format
                elif item.get("type") == "text":
                    text += item.get("text", "")
                # For assistant messages in Agent Response API format
                elif item.get("type") == "output_text":
                    text += item.get("text", "")
        return text

    return ""


def sanitize_message(msg: Dict[str, Any]) -> Dict[str, Any]:
    """Sanitize a message for logging by removing large image data.

    Args:
        msg: Message to sanitize

    Returns:
        Sanitized message
    """
    if not isinstance(msg, dict):
        return msg

    sanitized = msg.copy()

    # Handle message content
    if isinstance(sanitized.get("content"), list):
        sanitized_content = []
        for item in sanitized["content"]:
            if isinstance(item, dict):
                # Handle various image types
                if item.get("type") == "image_url" and "image_url" in item:
                    sanitized_content.append({"type": "image_url", "image_url": "[omitted]"})
                elif item.get("type") == "input_image" and "image_url" in item:
                    sanitized_content.append({"type": "input_image", "image_url": "[omitted]"})
                elif item.get("type") == "image" and "source" in item:
                    sanitized_content.append({"type": "image", "source": "[omitted]"})
                else:
                    sanitized_content.append(item)
            else:
                sanitized_content.append(item)
        sanitized["content"] = sanitized_content

    # Handle computer_call_output
    if sanitized.get("type") == "computer_call_output" and "output" in sanitized:
        output = sanitized["output"]
        if isinstance(output, dict) and "image_url" in output:
            sanitized["output"] = {**output, "image_url": "[omitted]"}

    return sanitized
