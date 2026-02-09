"""Utility functions for Anthropic message handling."""

import logging
import re
from datetime import datetime
from typing import Any, Dict, List, Optional, Tuple, cast

from anthropic.types.beta import BetaMessage

from ...core.types import AgentResponse

# Configure module logger
logger = logging.getLogger(__name__)


def to_anthropic_format(
    messages: List[Dict[str, Any]],
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
    previous_assistant_tool_use_ids = set()  # Track tool_use_ids in the previous assistant message

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
                    if isinstance(item, dict) and item.get("type") == "tool_use" and "id" in item:
                        previous_assistant_tool_use_ids.add(item["id"])

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
                        else:
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


def from_anthropic_format(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
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


async def to_agent_response_format(
    response: BetaMessage,
    messages: List[Dict[str, Any]],
    parsed_screen: Optional[dict] = None,
    parser: Optional[Any] = None,
    model: Optional[str] = None,
) -> AgentResponse:
    """Convert an Anthropic response to the standard agent response format.

    Args:
        response: The Anthropic API response (BetaMessage)
        messages: List of messages in standard format
        parsed_screen: Optional pre-parsed screen information
        parser: Optional parser instance for coordinate calculation
        model: Optional model name

    Returns:
        A response formatted according to the standard agent response format
    """
    # Create unique IDs for this response
    response_id = f"resp_{datetime.now().strftime('%Y%m%d%H%M%S')}_{id(response)}"
    reasoning_id = f"rs_{response_id}"
    action_id = f"cu_{response_id}"
    call_id = f"call_{response_id}"

    # Extract content and reasoning from Anthropic response
    content = []
    reasoning_text = None
    action_details = None

    for block in response.content:
        if block.type == "text":
            # Use the first text block as reasoning
            if reasoning_text is None:
                reasoning_text = block.text
            content.append({"type": "text", "text": block.text})
        elif block.type == "tool_use" and block.name == "computer":
            try:
                input_dict = cast(Dict[str, Any], block.input)
                action = input_dict.get("action", "").lower()

                # Extract coordinates from coordinate list if provided
                coordinates = input_dict.get("coordinate", [100, 100])
                x, y = coordinates if len(coordinates) == 2 else (100, 100)

                if action == "screenshot":
                    action_details = {
                        "type": "screenshot",
                    }
                elif action in ["click", "left_click", "right_click", "double_click"]:
                    action_details = {
                        "type": "click",
                        "button": "left" if action in ["click", "left_click"] else "right",
                        "double": action == "double_click",
                        "x": x,
                        "y": y,
                    }
                elif action == "type":
                    action_details = {
                        "type": "type",
                        "text": input_dict.get("text", ""),
                    }
                elif action == "key":
                    action_details = {
                        "type": "hotkey",
                        "keys": [input_dict.get("text", "")],
                    }
                elif action == "scroll":
                    scroll_amount = input_dict.get("scroll_amount", 1)
                    scroll_direction = input_dict.get("scroll_direction", "down")
                    delta_y = scroll_amount if scroll_direction == "down" else -scroll_amount
                    action_details = {
                        "type": "scroll",
                        "x": x,
                        "y": y,
                        "delta_x": 0,
                        "delta_y": delta_y,
                    }
                elif action == "move":
                    action_details = {
                        "type": "move",
                        "x": x,
                        "y": y,
                    }
            except Exception as e:
                logger.error(f"Error extracting action details: {str(e)}")

    # Create output items with reasoning
    output_items = []
    if reasoning_text:
        output_items.append(
            {
                "type": "reasoning",
                "id": reasoning_id,
                "summary": [
                    {
                        "type": "summary_text",
                        "text": reasoning_text,
                    }
                ],
            }
        )

    # Add computer_call item with extracted or default action
    computer_call = {
        "type": "computer_call",
        "id": action_id,
        "call_id": call_id,
        "action": action_details or {"type": "none", "description": "No action specified"},
        "pending_safety_checks": [],
        "status": "completed",
    }
    output_items.append(computer_call)

    # Create the standard response format
    standard_response = {
        "id": response_id,
        "object": "response",
        "created_at": int(datetime.now().timestamp()),
        "status": "completed",
        "error": None,
        "incomplete_details": None,
        "instructions": None,
        "max_output_tokens": None,
        "model": model or "anthropic-default",
        "output": output_items,
        "parallel_tool_calls": True,
        "previous_response_id": None,
        "reasoning": {"effort": "medium", "generate_summary": "concise"},
        "store": True,
        "temperature": 1.0,
        "text": {"format": {"type": "text"}},
        "tool_choice": "auto",
        "tools": [
            {
                "type": "computer_use_preview",
                "display_height": 768,
                "display_width": 1024,
                "environment": "mac",
            }
        ],
        "top_p": 1.0,
        "truncation": "auto",
        "usage": {
            "input_tokens": 0,
            "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": 0,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": 0,
        },
        "user": None,
        "metadata": {},
        "response": {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": content,
                        "tool_calls": [],
                    },
                    "finish_reason": response.stop_reason or "stop",
                }
            ]
        },
    }

    if response.usage:
        standard_response["usage"] = {
            "input_tokens": response.usage.input_tokens,
            "input_tokens_details": {
                "cached_tokens": response.usage.cache_creation_input_tokens,
            },
            "output_tokens": response.usage.output_tokens,
            "output_tokens_details": {
                "reasoning_tokens": 0
            },
            "total_tokens": response.usage.input_tokens + response.usage.output_tokens,
            **dict(response.usage),
        }
    
    # Add tool calls if present
    tool_calls = []
    for block in response.content:
        if hasattr(block, "type") and block.type == "tool_use":
            tool_calls.append(
                {
                    "id": f"call_{block.id}",
                    "type": "function",
                    "function": {"name": block.name, "arguments": block.input},
                }
            )
    if tool_calls:
        standard_response["response"]["choices"][0]["message"]["tool_calls"] = tool_calls

    return cast(AgentResponse, standard_response)
