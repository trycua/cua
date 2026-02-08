"""Main entry point for computer agents."""

import asyncio
import json
import logging
import os
from typing import Any, Dict, List, Optional

from som.models import ParseResult

from ...core.types import AgentResponse

logger = logging.getLogger(__name__)


async def to_openai_agent_response_format(
    response: Any,
    messages: List[Dict[str, Any]],
    parsed_screen: Optional[ParseResult] = None,
    parser: Optional[Any] = None,
    model: Optional[str] = None,
) -> AgentResponse:
    """Create an OpenAI computer use agent compatible response format.

    Args:
        response: The original API response
        messages: List of messages in standard OpenAI format
        parsed_screen: Optional pre-parsed screen information
        parser: Optional parser instance for coordinate calculation
        model: Optional model name

    Returns:
        A response formatted according to OpenAI's computer use agent standard, including:
        - All standard OpenAI computer use agent fields
        - Original response in response.choices[0].message
        - Full message history in messages field
    """
    import time
    from datetime import datetime

    # Create a unique ID for this response
    response_id = f"resp_{datetime.now().strftime('%Y%m%d%H%M%S')}_{id(response)}"
    reasoning_id = f"rs_{response_id}"
    action_id = f"cu_{response_id}"
    call_id = f"call_{response_id}"

    # Extract the last assistant message
    assistant_msg = None
    for msg in reversed(messages):
        if msg["role"] == "assistant":
            assistant_msg = msg
            break

    if not assistant_msg:
        # If no assistant message found, create a default one
        assistant_msg = {"role": "assistant", "content": "No response available"}

    # Initialize output array
    output_items = []

    # Extract reasoning and action details from the response
    content = assistant_msg["content"]
    reasoning_text = None
    action_details = None

    for item in content:
        if isinstance(item, dict) and item.get("type") == "text":
            try:
                # Try to parse JSON from text block
                text_content = item.get("text", "")
                parsed_json = json.loads(text_content)

                # Get reasoning text
                if reasoning_text is None:
                    reasoning_text = parsed_json.get("Explanation", "")

                # Extract action details
                action = parsed_json.get("Action", "").lower()
                text_input = parsed_json.get("Text", "")
                value = parsed_json.get("Value", "")  # Also handle Value field
                box_id = parsed_json.get("Box ID")  # Extract Box ID

                if action in ["click", "left_click"]:
                    # Always calculate coordinates from Box ID for click actions
                    x, y = 100, 100  # Default fallback values

                    if parsed_screen and box_id is not None and parser is not None:
                        try:
                            box_id_int = (
                                box_id
                                if isinstance(box_id, int)
                                else int(str(box_id)) if str(box_id).isdigit() else None
                            )
                            if box_id_int is not None:
                                # Use the parser's method to calculate coordinates
                                x, y = await parser.calculate_click_coordinates(
                                    box_id_int, parsed_screen
                                )
                        except Exception as e:
                            logger.error(
                                f"Error extracting coordinates for Box ID {box_id}: {str(e)}"
                            )

                    action_details = {
                        "type": "click",
                        "button": "left",
                        "box_id": (
                            (
                                box_id
                                if isinstance(box_id, int)
                                else int(box_id) if str(box_id).isdigit() else None
                            )
                            if box_id is not None
                            else None
                        ),
                        "x": x,
                        "y": y,
                    }
                elif action in ["type", "type_text"] and (text_input or value):
                    action_details = {
                        "type": "type",
                        "text": text_input or value,
                    }
                elif action == "hotkey" and value:
                    action_details = {
                        "type": "hotkey",
                        "keys": value,
                    }
                elif action == "scroll":
                    # Use default coordinates for scrolling
                    delta_x = 0
                    delta_y = 0
                    # Try to extract scroll delta values from content if available
                    scroll_data = parsed_json.get("Scroll", {})
                    if scroll_data:
                        delta_x = scroll_data.get("delta_x", 0)
                        delta_y = scroll_data.get("delta_y", 0)
                    action_details = {
                        "type": "scroll",
                        "x": 100,
                        "y": 100,
                        "scroll_x": delta_x,
                        "scroll_y": delta_y,
                    }
                elif action == "none":
                    # Handle case when action is None (task completion)
                    action_details = {"type": "none", "description": "Task completed"}
            except json.JSONDecodeError:
                # If not JSON, just use as reasoning text
                if reasoning_text is None:
                    reasoning_text = ""
                reasoning_text += item.get("text", "")

    # Add reasoning item if we have text content
    if reasoning_text:
        output_items.append(
            {
                "type": "reasoning",
                "id": reasoning_id,
                "summary": [
                    {
                        "type": "summary_text",
                        "text": reasoning_text[:200],  # Truncate to reasonable length
                    }
                ],
            }
        )

    # If no action details extracted, use default
    if not action_details:
        action_details = {
            "type": "click",
            "button": "left",
            "x": 100,
            "y": 100,
        }

    # Add computer_call item
    computer_call = {
        "type": "computer_call",
        "id": action_id,
        "call_id": call_id,
        "action": action_details,
        "pending_safety_checks": [],
        "status": "completed",
    }
    output_items.append(computer_call)

    # Extract user and assistant messages from the history
    user_messages = []
    assistant_messages = []
    for msg in messages:
        if msg["role"] == "user":
            user_messages.append(msg)
        elif msg["role"] == "assistant":
            assistant_messages.append(msg)

    # Create the OpenAI-compatible response format with all expected fields
    return {
        "id": response_id,
        "object": "response",
        "created_at": int(time.time()),
        "status": "completed",
        "error": None,
        "incomplete_details": None,
        "instructions": None,
        "max_output_tokens": None,
        "model": model or "unknown",
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
            "input_tokens": 0,  # Placeholder values
            "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": 0,  # Placeholder values
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": 0,  # Placeholder values
        },
        "user": None,
        "metadata": {},
        # Include the original response for backward compatibility
        "response": {"choices": [{"message": assistant_msg, "finish_reason": "stop"}]},
    }
