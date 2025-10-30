"""
Functions for making various Responses API items from different types of responses.
Based on the OpenAI spec for Responses API items.
"""

import base64
import json
import uuid
from typing import Any, Dict, List, Literal, Optional, Union

from openai.types.responses.easy_input_message_param import EasyInputMessageParam
from openai.types.responses.response_computer_tool_call_param import (
    ActionClick,
    ActionDoubleClick,
    ActionDrag,
    ActionDragPath,
    ActionKeypress,
    ActionMove,
    ActionScreenshot,
    ActionScroll,
)
from openai.types.responses.response_computer_tool_call_param import (
    ActionType as ActionTypeAction,
)
from openai.types.responses.response_computer_tool_call_param import (
    ActionWait,
    PendingSafetyCheck,
    ResponseComputerToolCallParam,
)
from openai.types.responses.response_function_tool_call_param import (
    ResponseFunctionToolCallParam,
)
from openai.types.responses.response_input_image_param import ResponseInputImageParam
from openai.types.responses.response_output_message_param import (
    ResponseOutputMessageParam,
)
from openai.types.responses.response_output_text_param import ResponseOutputTextParam
from openai.types.responses.response_reasoning_item_param import (
    ResponseReasoningItemParam,
    Summary,
)


def random_id():
    return str(uuid.uuid4())


# User message items
def make_input_image_item(image_data: Union[str, bytes]) -> EasyInputMessageParam:
    return EasyInputMessageParam(
        content=[
            ResponseInputImageParam(
                type="input_image",
                image_url=f"data:image/png;base64,{base64.b64encode(image_data).decode('utf-8') if isinstance(image_data, bytes) else image_data}",
            )  # type: ignore
        ],
        role="user",
        type="message",
    )


# Text items
def make_reasoning_item(reasoning: str) -> ResponseReasoningItemParam:
    return ResponseReasoningItemParam(
        id=random_id(), summary=[Summary(text=reasoning, type="summary_text")], type="reasoning"
    )


def make_output_text_item(content: str) -> ResponseOutputMessageParam:
    return ResponseOutputMessageParam(
        id=random_id(),
        content=[ResponseOutputTextParam(text=content, type="output_text", annotations=[])],
        role="assistant",
        status="completed",
        type="message",
    )


# Function call items
def make_function_call_item(
    function_name: str, arguments: Dict[str, Any], call_id: Optional[str] = None
) -> ResponseFunctionToolCallParam:
    return ResponseFunctionToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        name=function_name,
        arguments=json.dumps(arguments),
        status="completed",
        type="function_call",
    )


# Computer tool call items
def make_click_item(
    x: int,
    y: int,
    button: Literal["left", "right", "wheel", "back", "forward"] = "left",
    call_id: Optional[str] = None,
) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionClick(button=button, type="click", x=x, y=y),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_double_click_item(
    x: int, y: int, call_id: Optional[str] = None
) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionDoubleClick(type="double_click", x=x, y=y),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_drag_item(
    path: List[Dict[str, int]], call_id: Optional[str] = None
) -> ResponseComputerToolCallParam:
    drag_path = [ActionDragPath(x=point["x"], y=point["y"]) for point in path]
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionDrag(path=drag_path, type="drag"),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_keypress_item(
    keys: List[str], call_id: Optional[str] = None
) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionKeypress(keys=keys, type="keypress"),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_move_item(x: int, y: int, call_id: Optional[str] = None) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionMove(type="move", x=x, y=y),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_screenshot_item(call_id: Optional[str] = None) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionScreenshot(type="screenshot"),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_scroll_item(
    x: int, y: int, scroll_x: int, scroll_y: int, call_id: Optional[str] = None
) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionScroll(scroll_x=scroll_x, scroll_y=scroll_y, type="scroll", x=x, y=y),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_type_item(text: str, call_id: Optional[str] = None) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionTypeAction(text=text, type="type"),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


def make_wait_item(call_id: Optional[str] = None) -> ResponseComputerToolCallParam:
    return ResponseComputerToolCallParam(
        id=random_id(),
        call_id=call_id if call_id else random_id(),
        action=ActionWait(type="wait"),
        pending_safety_checks=[],
        status="completed",
        type="computer_call",
    )


# Extra anthropic computer calls
def make_left_mouse_down_item(
    x: Optional[int] = None, y: Optional[int] = None, call_id: Optional[str] = None
) -> Dict[str, Any]:
    return {
        "id": random_id(),
        "call_id": call_id if call_id else random_id(),
        "action": {"type": "left_mouse_down", "x": x, "y": y},
        "pending_safety_checks": [],
        "status": "completed",
        "type": "computer_call",
    }


def make_left_mouse_up_item(
    x: Optional[int] = None, y: Optional[int] = None, call_id: Optional[str] = None
) -> Dict[str, Any]:
    return {
        "id": random_id(),
        "call_id": call_id if call_id else random_id(),
        "action": {"type": "left_mouse_up", "x": x, "y": y},
        "pending_safety_checks": [],
        "status": "completed",
        "type": "computer_call",
    }


def make_failed_tool_call_items(
    tool_name: str, tool_kwargs: Dict[str, Any], error_message: str, call_id: Optional[str] = None
) -> List[Dict[str, Any]]:
    call_id = call_id if call_id else random_id()
    return [
        {
            "type": "function_call",
            "id": random_id(),
            "call_id": call_id,
            "name": tool_name,
            "arguments": json.dumps(tool_kwargs),
        },
        {
            "type": "function_call_output",
            "call_id": call_id,
            "output": json.dumps({"error": error_message}),
        },
    ]


def make_tool_error_item(error_message: str, call_id: Optional[str] = None) -> Dict[str, Any]:
    call_id = call_id if call_id else random_id()
    return {
        "type": "function_call_output",
        "call_id": call_id,
        "output": json.dumps({"error": error_message}),
    }


def replace_failed_computer_calls_with_function_calls(
    messages: List[Dict[str, Any]],
) -> List[Dict[str, Any]]:
    """
    Replace computer_call items with function_call items if they share a call_id with a function_call_output.
    This indicates the computer call failed and should be treated as a function call instead.
    We do this because the computer_call_output items do not support text output.

    Args:
        messages: List of message items to process
    """
    messages = messages.copy()

    # Find all call_ids that have function_call_output items
    failed_call_ids = set()
    for msg in messages:
        if msg.get("type") == "function_call_output":
            call_id = msg.get("call_id")
            if call_id:
                failed_call_ids.add(call_id)

    # Replace computer_call items that have matching call_ids
    for i, msg in enumerate(messages):
        if msg.get("type") == "computer_call" and msg.get("call_id") in failed_call_ids:

            # Extract action from computer_call
            action = msg.get("action", {})
            call_id = msg.get("call_id")

            # Create function_call replacement
            messages[i] = {
                "type": "function_call",
                "id": msg.get("id", random_id()),
                "call_id": call_id,
                "name": "computer",
                "arguments": json.dumps(action),
            }

    return messages


# Conversion functions between element descriptions and coordinates
def convert_computer_calls_desc2xy(
    responses_items: List[Dict[str, Any]], desc2xy: Dict[str, tuple]
) -> List[Dict[str, Any]]:
    """
    Convert computer calls from element descriptions to x,y coordinates.

    Args:
        responses_items: List of response items containing computer calls with element_description
        desc2xy: Dictionary mapping element descriptions to (x, y) coordinate tuples

    Returns:
        List of response items with element_description replaced by x,y coordinates
    """
    converted_items = []

    for item in responses_items:
        if item.get("type") == "computer_call" and "action" in item:
            action = item["action"].copy()

            # Handle single element_description
            if "element_description" in action:
                desc = action["element_description"]
                if desc in desc2xy:
                    x, y = desc2xy[desc]
                    action["x"] = x
                    action["y"] = y
                    del action["element_description"]

            # Handle start_element_description and end_element_description for drag operations
            elif "start_element_description" in action and "end_element_description" in action:
                start_desc = action["start_element_description"]
                end_desc = action["end_element_description"]

                if start_desc in desc2xy and end_desc in desc2xy:
                    start_x, start_y = desc2xy[start_desc]
                    end_x, end_y = desc2xy[end_desc]
                    action["path"] = [{"x": start_x, "y": start_y}, {"x": end_x, "y": end_y}]
                    del action["start_element_description"]
                    del action["end_element_description"]

            converted_item = item.copy()
            converted_item["action"] = action
            converted_items.append(converted_item)
        else:
            converted_items.append(item)

    return converted_items


def convert_computer_calls_xy2desc(
    responses_items: List[Dict[str, Any]], desc2xy: Dict[str, tuple]
) -> List[Dict[str, Any]]:
    """
    Convert computer calls from x,y coordinates to element descriptions.

    Args:
        responses_items: List of response items containing computer calls with x,y coordinates
        desc2xy: Dictionary mapping element descriptions to (x, y) coordinate tuples

    Returns:
        List of response items with x,y coordinates replaced by element_description
    """
    # Create reverse mapping from coordinates to descriptions
    xy2desc = {coords: desc for desc, coords in desc2xy.items()}

    converted_items = []

    for item in responses_items:
        if item.get("type") == "computer_call" and "action" in item:
            action = item["action"].copy()

            # Handle single x,y coordinates
            if "x" in action and "y" in action:
                coords = (action["x"], action["y"])
                if coords in xy2desc:
                    action["element_description"] = xy2desc[coords]
                    del action["x"]
                    del action["y"]

            # Handle path for drag operations
            elif "path" in action and isinstance(action["path"], list) and len(action["path"]) == 2:
                start_point = action["path"][0]
                end_point = action["path"][1]

                if (
                    "x" in start_point
                    and "y" in start_point
                    and "x" in end_point
                    and "y" in end_point
                ):

                    start_coords = (start_point["x"], start_point["y"])
                    end_coords = (end_point["x"], end_point["y"])

                    if start_coords in xy2desc and end_coords in xy2desc:
                        action["start_element_description"] = xy2desc[start_coords]
                        action["end_element_description"] = xy2desc[end_coords]
                        del action["path"]

            converted_item = item.copy()
            converted_item["action"] = action
            converted_items.append(converted_item)
        else:
            converted_items.append(item)

    return converted_items


def get_all_element_descriptions(responses_items: List[Dict[str, Any]]) -> List[str]:
    """
    Extract all element descriptions from computer calls in responses items.

    Args:
        responses_items: List of response items containing computer calls

    Returns:
        List of unique element descriptions found in computer calls
    """
    descriptions = set()

    for item in responses_items:
        if item.get("type") == "computer_call" and "action" in item:
            action = item["action"]

            # Handle single element_description
            if "element_description" in action:
                descriptions.add(action["element_description"])

            # Handle start_element_description and end_element_description for drag operations
            if "start_element_description" in action:
                descriptions.add(action["start_element_description"])

            if "end_element_description" in action:
                descriptions.add(action["end_element_description"])

    return list(descriptions)


# Conversion functions between responses_items and completion messages formats
def convert_responses_items_to_completion_messages(
    messages: List[Dict[str, Any]], allow_images_in_tool_results: bool = True
) -> List[Dict[str, Any]]:
    """Convert responses_items message format to liteLLM completion format.

    Args:
        messages: List of responses_items format messages
        allow_images_in_tool_results: If True, include images in tool role messages.
                                    If False, send tool message + separate user message with image.
    """
    completion_messages = []

    for message in messages:
        msg_type = message.get("type")
        role = message.get("role")

        # Handle user messages (both with and without explicit type)
        if role == "user" or msg_type == "user":
            content = message.get("content", "")
            if isinstance(content, list):
                # Handle list content (images, text blocks)
                completion_content = []
                for item in content:
                    if item.get("type") == "input_image":
                        completion_content.append(
                            {"type": "image_url", "image_url": {"url": item.get("image_url")}}
                        )
                    elif item.get("type") == "input_text":
                        completion_content.append({"type": "text", "text": item.get("text")})
                    elif item.get("type") == "text":
                        completion_content.append({"type": "text", "text": item.get("text")})

                completion_messages.append({"role": "user", "content": completion_content})
            elif isinstance(content, str):
                # Handle string content
                completion_messages.append({"role": "user", "content": content})

        # Handle assistant messages
        elif role == "assistant" or msg_type == "message":
            content = message.get("content", [])
            if isinstance(content, list):
                text_parts = []
                for item in content:
                    if item.get("type") == "output_text":
                        text_parts.append(item.get("text", ""))
                    elif item.get("type") == "text":
                        text_parts.append(item.get("text", ""))

                if text_parts:
                    completion_messages.append(
                        {"role": "assistant", "content": "\n".join(text_parts)}
                    )

        # Handle reasoning items (convert to assistant message)
        elif msg_type == "reasoning":
            summary = message.get("summary", [])
            text_parts = []
            for item in summary:
                if item.get("type") == "summary_text":
                    text_parts.append(item.get("text", ""))

            if text_parts:
                completion_messages.append({"role": "assistant", "content": "\n".join(text_parts)})

        # Handle function calls
        elif msg_type == "function_call":
            # Add tool call to last assistant message or create new one
            if not completion_messages or completion_messages[-1]["role"] != "assistant":
                completion_messages.append({"role": "assistant", "content": "", "tool_calls": []})

            if "tool_calls" not in completion_messages[-1]:
                completion_messages[-1]["tool_calls"] = []

            completion_messages[-1]["tool_calls"].append(
                {
                    "id": message.get("call_id"),
                    "type": "function",
                    "function": {
                        "name": message.get("name"),
                        "arguments": message.get("arguments"),
                    },
                }
            )

        # Handle computer calls
        elif msg_type == "computer_call":
            # Add tool call to last assistant message or create new one
            if not completion_messages or completion_messages[-1]["role"] != "assistant":
                completion_messages.append({"role": "assistant", "content": "", "tool_calls": []})

            if "tool_calls" not in completion_messages[-1]:
                completion_messages[-1]["tool_calls"] = []

            action = message.get("action", {})
            completion_messages[-1]["tool_calls"].append(
                {
                    "id": message.get("call_id"),
                    "type": "function",
                    "function": {"name": "computer", "arguments": json.dumps(action)},
                }
            )

        # Handle function/computer call outputs
        elif msg_type in ["function_call_output", "computer_call_output"]:
            output = message.get("output")
            call_id = message.get("call_id")

            if isinstance(output, dict) and output.get("type") == "input_image":
                if allow_images_in_tool_results:
                    # Handle image output as tool response (may not work with all APIs)
                    completion_messages.append(
                        {
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": [
                                {"type": "image_url", "image_url": {"url": output.get("image_url")}}
                            ],
                        }
                    )
                else:
                    # Send tool message + separate user message with image (OpenAI compatible)
                    completion_messages += [
                        {
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": "[Execution completed. See screenshot below]",
                        },
                        {
                            "role": "user",
                            "content": [
                                {"type": "image_url", "image_url": {"url": output.get("image_url")}}
                            ],
                        },
                    ]
            else:
                # Handle text output as tool response
                completion_messages.append(
                    {"role": "tool", "tool_call_id": call_id, "content": str(output)}
                )

    return completion_messages


def convert_completion_messages_to_responses_items(
    completion_messages: List[Dict[str, Any]],
) -> List[Dict[str, Any]]:
    """Convert completion messages format to responses_items message format."""
    responses_items = []
    skip_next = False

    for i, message in enumerate(completion_messages):
        if skip_next:
            skip_next = False
            continue

        role = message.get("role")
        content = message.get("content")
        tool_calls = message.get("tool_calls", [])

        # Handle assistant messages with text content
        if role == "assistant" and content and isinstance(content, str):
            responses_items.append(
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": content}],
                }
            )

        # Handle tool calls
        if tool_calls:
            for tool_call in tool_calls:
                if tool_call.get("type") == "function":
                    function = tool_call.get("function", {})
                    function_name = function.get("name")

                    if function_name == "computer":
                        # Parse computer action
                        try:
                            action = json.loads(function.get("arguments", "{}"))
                            # Change key from "action" -> "type"
                            if action.get("action"):
                                action["type"] = action["action"]
                                del action["action"]
                            responses_items.append(
                                {
                                    "type": "computer_call",
                                    "call_id": tool_call.get("id"),
                                    "action": action,
                                    "status": "completed",
                                }
                            )
                        except json.JSONDecodeError:
                            # Fallback to function call format
                            responses_items.append(
                                {
                                    "type": "function_call",
                                    "call_id": tool_call.get("id"),
                                    "name": function_name,
                                    "arguments": function.get("arguments", "{}"),
                                    "status": "completed",
                                }
                            )
                    else:
                        # Regular function call
                        responses_items.append(
                            {
                                "type": "function_call",
                                "call_id": tool_call.get("id"),
                                "name": function_name,
                                "arguments": function.get("arguments", "{}"),
                                "status": "completed",
                            }
                        )

        # Handle tool messages (function/computer call outputs)
        elif role == "tool" and content:
            tool_call_id = message.get("tool_call_id")
            if isinstance(content, str):
                # Check if this is the "[Execution completed. See screenshot below]" pattern
                if content == "[Execution completed. See screenshot below]":
                    # Look ahead for the next user message with image
                    next_idx = i + 1
                    if (
                        next_idx < len(completion_messages)
                        and completion_messages[next_idx].get("role") == "user"
                        and isinstance(completion_messages[next_idx].get("content"), list)
                    ):
                        # Found the pattern - extract image from next message
                        next_content = completion_messages[next_idx]["content"]
                        for item in next_content:
                            if item.get("type") == "image_url":
                                responses_items.append(
                                    {
                                        "type": "computer_call_output",
                                        "call_id": tool_call_id,
                                        "output": {
                                            "type": "input_image",
                                            "image_url": item.get("image_url", {}).get("url"),
                                        },
                                    }
                                )
                                # Skip the next user message since we processed it
                                skip_next = True
                                break
                    else:
                        # No matching user message, treat as regular text
                        responses_items.append(
                            {
                                "type": "computer_call_output",
                                "call_id": tool_call_id,
                                "output": content,
                            }
                        )
                else:
                    # Determine if this is a computer call or function call output
                    try:
                        # Try to parse as structured output
                        parsed_content = json.loads(content)
                        if parsed_content.get("type") == "input_image":
                            responses_items.append(
                                {
                                    "type": "computer_call_output",
                                    "call_id": tool_call_id,
                                    "output": parsed_content,
                                }
                            )
                        else:
                            responses_items.append(
                                {
                                    "type": "computer_call_output",
                                    "call_id": tool_call_id,
                                    "output": content,
                                }
                            )
                    except json.JSONDecodeError:
                        # Plain text output - could be function or computer call
                        responses_items.append(
                            {
                                "type": "function_call_output",
                                "call_id": tool_call_id,
                                "output": content,
                            }
                        )
            elif isinstance(content, list):
                # Handle structured content (e.g., images)
                for item in content:
                    if item.get("type") == "image_url":
                        responses_items.append(
                            {
                                "type": "computer_call_output",
                                "call_id": tool_call_id,
                                "output": {
                                    "type": "input_image",
                                    "image_url": item.get("image_url", {}).get("url"),
                                },
                            }
                        )
                    elif item.get("type") == "text":
                        responses_items.append(
                            {
                                "type": "function_call_output",
                                "call_id": tool_call_id,
                                "output": item.get("text"),
                            }
                        )

        # Handle actual user messages
        elif role == "user" and content:
            if isinstance(content, list):
                # Handle structured user content (e.g., text + images)
                user_content = []
                for item in content:
                    if item.get("type") == "image_url":
                        user_content.append(
                            {
                                "type": "input_image",
                                "image_url": item.get("image_url", {}).get("url"),
                            }
                        )
                    elif item.get("type") == "text":
                        user_content.append({"type": "input_text", "text": item.get("text")})

                if user_content:
                    responses_items.append(
                        {"role": "user", "type": "message", "content": user_content}
                    )
            elif isinstance(content, str):
                # Handle simple text user message
                responses_items.append({"role": "user", "content": content})

    return responses_items
