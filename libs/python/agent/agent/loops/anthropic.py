"""
Anthropic hosted tools agent loop implementation using liteLLM
"""

import asyncio
import json
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, Union

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    make_click_item,
    make_double_click_item,
    make_drag_item,
    make_failed_tool_call_items,
    make_input_image_item,
    make_keypress_item,
    make_left_mouse_down_item,
    make_left_mouse_up_item,
    make_move_item,
    make_output_text_item,
    make_reasoning_item,
    make_screenshot_item,
    make_scroll_item,
    make_type_item,
    make_wait_item,
)
from ..types import AgentCapability, AgentResponse, Messages, Tools

# Model version mapping to tool version and beta flag
MODEL_TOOL_MAPPING = [
    # Claude 4 models
    {
        "pattern": r"claude-4|claude-opus-4|claude-sonnet-4|claude-haiku-4",
        "tool_version": "computer_20250124",
        "beta_flag": "computer-use-2025-01-24",
    },
    # Claude 3.7 models
    {
        "pattern": r"claude-3\.?7|claude-3-7",
        "tool_version": "computer_20250124",
        "beta_flag": "computer-use-2025-01-24",
    },
    # Claude 3.5 models (fallback)
    {
        "pattern": r"claude-3\.?5|claude-3-5",
        "tool_version": "computer_20241022",
        "beta_flag": "computer-use-2024-10-22",
    },
]


def _get_tool_config_for_model(model: str) -> Dict[str, str]:
    """Get tool version and beta flag for the given model."""
    import re

    for mapping in MODEL_TOOL_MAPPING:
        if re.search(mapping["pattern"], model, re.IGNORECASE):
            return {"tool_version": mapping["tool_version"], "beta_flag": mapping["beta_flag"]}

    # Default to Claude 3.5 configuration
    return {"tool_version": "computer_20241022", "beta_flag": "computer-use-2024-10-22"}


async def _map_computer_tool_to_anthropic(computer_tool: Any, tool_version: str) -> Dict[str, Any]:
    """Map a computer tool to Anthropic's hosted tool schema."""
    # Get dimensions from the computer handler
    try:
        width, height = await computer_tool.get_dimensions()
    except Exception:
        # Fallback to default dimensions if method fails
        width, height = 1024, 768

    return {
        "type": tool_version,
        "function": {
            "name": "computer",
            "parameters": {
                "display_height_px": height,
                "display_width_px": width,
                "display_number": 1,
            },
        },
    }


async def _prepare_tools_for_anthropic(tool_schemas: List[Dict[str, Any]], model: str) -> Tools:
    """Prepare tools for Anthropic API format."""
    tool_config = _get_tool_config_for_model(model)
    anthropic_tools = []

    for schema in tool_schemas:
        if schema["type"] == "computer":
            # Map computer tool to Anthropic format
            anthropic_tools.append(
                await _map_computer_tool_to_anthropic(
                    schema["computer"], tool_config["tool_version"]
                )
            )
        elif schema["type"] == "function":
            # Function tools - convert to Anthropic format
            function_schema = schema["function"]
            anthropic_tools.append(
                {
                    "type": "function",
                    "function": {
                        "name": function_schema["name"],
                        "description": function_schema.get("description", ""),
                        "parameters": function_schema.get("parameters", {}),
                    },
                }
            )

    return anthropic_tools


def _convert_responses_items_to_completion_messages(messages: Messages) -> List[Dict[str, Any]]:
    """Convert responses_items message format to liteLLM completion format."""
    completion_messages = []
    call_id_to_fn_name = {}

    for message in messages:
        msg_type = message.get("type")
        role = message.get("role")

        # Handle user messages (both with and without explicit type)
        if role == "user" or msg_type == "user":
            content = message.get("content", "")
            if isinstance(content, list):
                # Multi-modal content - convert input_image to image format
                converted_content = []
                for item in content:
                    if isinstance(item, dict) and item.get("type") == "input_image":
                        # Convert input_image to OpenAI image format
                        image_url = item.get("image_url", "")
                        if image_url and image_url != "[omitted]":
                            converted_content.append(
                                {"type": "image_url", "image_url": {"url": image_url}}
                            )
                    elif isinstance(item, dict) and item.get("type") == "input_text":
                        # Convert input_text to OpenAI text format
                        text = item.get("text", "")
                        converted_content.append({"type": "text", "text": text})
                    else:
                        # Keep other content types as-is
                        converted_content.append(item)

                completion_messages.append(
                    {"role": "user", "content": converted_content if converted_content else content}
                )
            else:
                # Text content
                completion_messages.append({"role": "user", "content": content})

        # Handle assistant messages
        elif role == "assistant":
            content = message.get("content", [])
            if isinstance(content, str):
                content = [{"type": "output_text", "text": content}]

            content = "\n".join(item.get("text", "") for item in content)
            completion_messages.append({"role": "assistant", "content": content})

        elif msg_type == "reasoning":
            # Reasoning becomes part of assistant message
            summary = message.get("summary", [])
            reasoning_text = ""

            if isinstance(summary, list) and summary:
                # Extract text from summary items
                for item in summary:
                    if isinstance(item, dict) and item.get("type") == "summary_text":
                        reasoning_text = item.get("text", "")
                        break
            else:
                # Fallback to direct reasoning field
                reasoning_text = message.get("reasoning", "")

            if reasoning_text:
                completion_messages.append({"role": "assistant", "content": reasoning_text})

        elif msg_type == "function_call":
            fn_name = message.get("name")
            fn_args = message.get("arguments", "{}")
            call_id = message.get("call_id", "call_1")
            call_id_to_fn_name[call_id] = fn_name
            openai_tool_calls = [
                {
                    "id": call_id,
                    "type": "function",
                    "function": {"name": fn_name, "arguments": fn_args},
                }
            ]  # If the last completion message is an assistant message, extend the tool_calls
            if completion_messages and completion_messages[-1].get("role") == "assistant":
                if "tool_calls" not in completion_messages[-1]:
                    completion_messages[-1]["tool_calls"] = []
                completion_messages[-1]["tool_calls"].extend(openai_tool_calls)
            else:
                # Create new assistant message with tool calls
                completion_messages.append(
                    {"role": "assistant", "content": None, "tool_calls": openai_tool_calls}
                )

        elif msg_type == "function_call_output":
            call_id = message.get("call_id", "call_1")
            fn_output = message.get("output", "")
            fn_name = call_id_to_fn_name.get(call_id, "computer")

            completion_messages.append(
                {
                    "role": "function",
                    "name": fn_name,
                    "tool_call_id": call_id,
                    "content": str(fn_output),
                }
            )

        elif msg_type == "computer_call":
            # Computer call becomes tool use in assistant message
            action = message.get("action", {})
            action_type = action.get("type")
            call_id = message.get("call_id", "call_1")

            tool_use_content = []

            # Basic actions (all versions)
            if action_type == "click":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "click",
                #         "x": 100,
                #         "y": 200
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "click",
                #             "coordinate": [100, 200]
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                button = action.get("button", "left")
                action_name = (
                    "right_click"
                    if button == "right"
                    else "middle_click" if button == "wheel" else "left_click"
                )
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": action_name,
                            "coordinate": [action.get("x", 0), action.get("y", 0)],
                        },
                    }
                )
            elif action_type == "double_click":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "double_click",
                #         "x": 160,
                #         "y": 240
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "double_click",
                #             "coordinate": [160, 240]
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": "double_click",
                            "coordinate": [action.get("x", 0), action.get("y", 0)],
                        },
                    }
                )
            elif action_type == "type":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "type",
                #         "text": "Hello World"
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "type",
                #             "text": "Hello World"
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {"action": "type", "text": action.get("text", "")},
                    }
                )
            elif action_type == "keypress":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "keypress",
                #         "keys": ["ctrl", "c"]
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "key",
                #             "text": "ctrl+c"
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {"action": "key", "text": "+".join(action.get("keys", []))},
                    }
                )
            elif action_type in ["mouse_move", "move"]:
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "move",
                #         "x": 150,
                #         "y": 250
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "mouse_move",
                #             "coordinate": [150, 250]
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": "mouse_move",
                            "coordinate": [action.get("x", 0), action.get("y", 0)],
                        },
                    }
                )
            elif action_type == "scroll":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "scroll",
                #         "x": 300,
                #         "y": 400,
                #         "scroll_x": 0,
                #         "scroll_y": -5
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "scroll",
                #             "coordinate": [300, 400],
                #             "scroll_direction": "down",
                #             "scroll_amount": 5
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                scroll_x = action.get("scroll_x", 0)
                scroll_y = action.get("scroll_y", 0)
                # Determine direction and amount from scroll values
                if scroll_x > 0:
                    direction = "left"
                    amount = scroll_x
                elif scroll_x < 0:
                    direction = "right"
                    amount = -scroll_x
                elif scroll_y > 0:
                    direction = "up"
                    amount = scroll_y
                elif scroll_y < 0:
                    direction = "down"
                    amount = -scroll_y
                else:
                    direction = "down"
                    amount = 3

                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": "scroll",
                            "coordinate": [action.get("x", 0), action.get("y", 0)],
                            "scroll_direction": direction,
                            "scroll_amount": amount,
                        },
                    }
                )
            elif action_type == "drag":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "drag",
                #         "path": [
                #             {"x": 100, "y": 150},
                #             {"x": 200, "y": 250}
                #         ]
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "left_click_drag",
                #             "start_coordinate": [100, 150],
                #             "end_coordinate": [200, 250]
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                path = action.get("path", [])
                start_coord = [0, 0]
                end_coord = [0, 0]
                if isinstance(path, list) and len(path) >= 2:
                    start_coord = [path[0].get("x", 0), path[0].get("y", 0)]
                    end_coord = [path[-1].get("x", 0), path[-1].get("y", 0)]

                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": "left_click_drag",
                            "start_coordinate": start_coord,
                            "end_coordinate": end_coord,
                        },
                    }
                )
            elif action_type == "wait":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "wait"
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "wait"
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {"action": "wait"},
                    }
                )
            elif action_type == "screenshot":
                # Input:
                # {
                #     "type": "computer_call",
                #     "call_id": "call_1",
                #     "action": {
                #         "type": "screenshot"
                #     }
                # }

                # Output:
                # {
                #     "function": {
                #         "name": "computer",
                #         "arguments": json.dumps({
                #             "action": "screenshot"
                #         })
                #     },
                #     "id": "call_1",
                #     "type": "function"
                # }
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {"action": "screenshot"},
                    }
                )
            elif action_type == "left_mouse_down":
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": "left_mouse_down",
                            "coordinate": [action.get("x", None), action.get("y", None)],
                        },
                    }
                )
            elif action_type == "left_mouse_up":
                tool_use_content.append(
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": "computer",
                        "input": {
                            "action": "left_mouse_up",
                            "coordinate": [action.get("x", None), action.get("y", None)],
                        },
                    }
                )

            # Convert tool_use_content to OpenAI tool_calls format
            openai_tool_calls = []
            for tool_use in tool_use_content:
                openai_tool_calls.append(
                    {
                        "id": tool_use["id"],
                        "type": "function",
                        "function": {
                            "name": tool_use["name"],
                            "arguments": json.dumps(tool_use["input"]),
                        },
                    }
                )

            # If the last completion message is an assistant message, extend the tool_calls
            if completion_messages and completion_messages[-1].get("role") == "assistant":
                if "tool_calls" not in completion_messages[-1]:
                    completion_messages[-1]["tool_calls"] = []
                completion_messages[-1]["tool_calls"].extend(openai_tool_calls)
            else:
                # Create new assistant message with tool calls
                completion_messages.append(
                    {"role": "assistant", "content": None, "tool_calls": openai_tool_calls}
                )

        elif msg_type == "computer_call_output":
            # Computer call output becomes OpenAI function result
            output = message.get("output", {})
            call_id = message.get("call_id", "call_1")

            if output.get("type") == "input_image":
                # Screenshot result - convert to OpenAI format with image_url content
                image_url = output.get("image_url", "")
                completion_messages.append(
                    {
                        "role": "function",
                        "name": "computer",
                        "tool_call_id": call_id,
                        "content": [{"type": "image_url", "image_url": {"url": image_url}}],
                    }
                )
            else:
                # Text result - convert to OpenAI format
                completion_messages.append(
                    {
                        "role": "function",
                        "name": "computer",
                        "tool_call_id": call_id,
                        "content": str(output),
                    }
                )

    return completion_messages


def _convert_completion_to_responses_items(response: Any) -> List[Dict[str, Any]]:
    """Convert liteLLM completion response to responses_items message format."""
    responses_items = []

    if not response or not hasattr(response, "choices") or not response.choices:
        return responses_items

    choice = response.choices[0]
    message = choice.message

    # Handle text content
    if hasattr(message, "content") and message.content:
        if isinstance(message.content, str):
            responses_items.append(make_output_text_item(message.content))
        elif isinstance(message.content, list):
            for content_item in message.content:
                if isinstance(content_item, dict):
                    if content_item.get("type") == "text":
                        responses_items.append(make_output_text_item(content_item.get("text", "")))
                    elif content_item.get("type") == "tool_use":
                        # Convert tool use to computer call
                        tool_input = content_item.get("input", {})
                        action_type = tool_input.get("action")
                        call_id = content_item.get("id")

                        # Action reference:
                        # https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/computer-use-tool#available-actions

                        try:
                            # Basic actions (all versions)
                            if action_type == "screenshot":
                                responses_items.append(make_screenshot_item(call_id=call_id))
                            elif action_type in ["click", "left_click"]:
                                coordinate = tool_input.get("coordinate", [0, 0])
                                responses_items.append(
                                    make_click_item(
                                        x=coordinate[0] if len(coordinate) > 0 else 0,
                                        y=coordinate[1] if len(coordinate) > 1 else 0,
                                        call_id=call_id,
                                    )
                                )
                            elif action_type in ["type", "type_text"]:
                                responses_items.append(
                                    make_type_item(text=tool_input.get("text", ""), call_id=call_id)
                                )
                            elif action_type in ["key", "keypress", "hotkey"]:
                                responses_items.append(
                                    make_keypress_item(
                                        keys=tool_input.get("text", "")
                                        .replace("+", "-")
                                        .split("-"),
                                        call_id=call_id,
                                    )
                                )
                            elif action_type in ["mouse_move", "move_cursor", "move"]:
                                # Mouse move - create a custom action item
                                coordinate = tool_input.get("coordinate", [0, 0])
                                responses_items.append(
                                    make_move_item(
                                        x=coordinate[0] if len(coordinate) > 0 else 0,
                                        y=coordinate[1] if len(coordinate) > 1 else 0,
                                        call_id=call_id,
                                    )
                                )

                            # Enhanced actions (computer_20250124) Available in Claude 4 and Claude Sonnet 3.7
                            elif action_type == "scroll":
                                coordinate = tool_input.get("coordinate", [0, 0])
                                scroll_amount = tool_input.get("scroll_amount", 3)
                                scroll_x = (
                                    scroll_amount
                                    if tool_input.get("scroll_direction", "down") == "right"
                                    else (
                                        -scroll_amount
                                        if tool_input.get("scroll_direction", "down") == "left"
                                        else 0
                                    )
                                )
                                scroll_y = (
                                    scroll_amount
                                    if tool_input.get("scroll_direction", "down") == "down"
                                    else (
                                        -scroll_amount
                                        if tool_input.get("scroll_direction", "down") == "up"
                                        else 0
                                    )
                                )
                                responses_items.append(
                                    make_scroll_item(
                                        x=coordinate[0] if len(coordinate) > 0 else 0,
                                        y=coordinate[1] if len(coordinate) > 1 else 0,
                                        scroll_x=scroll_x,
                                        scroll_y=scroll_y,
                                        call_id=call_id,
                                    )
                                )
                            elif action_type in ["left_click_drag", "drag"]:
                                start_coord = tool_input.get("start_coordinate", [0, 0])
                                end_coord = tool_input.get("end_coordinate", [0, 0])
                                responses_items.append(
                                    make_drag_item(
                                        path=[
                                            {
                                                "x": start_coord[0] if len(start_coord) > 0 else 0,
                                                "y": start_coord[1] if len(start_coord) > 1 else 0,
                                            },
                                            {
                                                "x": end_coord[0] if len(end_coord) > 0 else 0,
                                                "y": end_coord[1] if len(end_coord) > 1 else 0,
                                            },
                                        ],
                                        call_id=call_id,
                                    )
                                )
                            elif action_type == "right_click":
                                coordinate = tool_input.get("coordinate", [0, 0])
                                responses_items.append(
                                    make_click_item(
                                        x=coordinate[0] if len(coordinate) > 0 else 0,
                                        y=coordinate[1] if len(coordinate) > 1 else 0,
                                        button="right",
                                        call_id=call_id,
                                    )
                                )
                            elif action_type == "middle_click":
                                coordinate = tool_input.get("coordinate", [0, 0])
                                responses_items.append(
                                    make_click_item(
                                        x=coordinate[0] if len(coordinate) > 0 else 0,
                                        y=coordinate[1] if len(coordinate) > 1 else 0,
                                        button="wheel",
                                        call_id=call_id,
                                    )
                                )
                            elif action_type == "double_click":
                                coordinate = tool_input.get("coordinate", [0, 0])
                                responses_items.append(
                                    make_double_click_item(
                                        x=coordinate[0] if len(coordinate) > 0 else 0,
                                        y=coordinate[1] if len(coordinate) > 1 else 0,
                                        call_id=call_id,
                                    )
                                )
                            elif action_type == "triple_click":
                                # coordinate = tool_input.get("coordinate", [0, 0])
                                # responses_items.append({
                                #     "type": "computer_call",
                                #     "call_id": call_id,
                                #     "action": {
                                #         "type": "triple_click",
                                #         "x": coordinate[0] if len(coordinate) > 0 else 0,
                                #         "y": coordinate[1] if len(coordinate) > 1 else 0
                                #     }
                                # })
                                raise NotImplementedError("triple_click")
                            elif action_type == "left_mouse_down":
                                # coordinate = tool_input.get("coordinate", [0, 0])
                                # responses_items.append({
                                #     "type": "computer_call",
                                #     "call_id": call_id,
                                #     "action": {
                                #         "type": "mouse_down",
                                #         "button": "left",
                                #         "x": coordinate[0] if len(coordinate) > 0 else 0,
                                #         "y": coordinate[1] if len(coordinate) > 1 else 0
                                #     }
                                # })
                                coordinate = tool_input.get("coordinate", [None, None])
                                responses_items.append(
                                    make_left_mouse_down_item(
                                        x=coordinate[0] if len(coordinate) > 0 else None,
                                        y=coordinate[1] if len(coordinate) > 1 else None,
                                        call_id=call_id,
                                    )
                                )
                            elif action_type == "left_mouse_up":
                                # coordinate = tool_input.get("coordinate", [0, 0])
                                # responses_items.append({
                                #     "type": "computer_call",
                                #     "call_id": call_id,
                                #     "action": {
                                #         "type": "mouse_up",
                                #         "button": "left",
                                #         "x": coordinate[0] if len(coordinate) > 0 else 0,
                                #         "y": coordinate[1] if len(coordinate) > 1 else 0
                                #     }
                                # })
                                coordinate = tool_input.get("coordinate", [None, None])
                                responses_items.append(
                                    make_left_mouse_up_item(
                                        x=coordinate[0] if len(coordinate) > 0 else None,
                                        y=coordinate[1] if len(coordinate) > 1 else None,
                                        call_id=call_id,
                                    )
                                )
                            elif action_type == "hold_key":
                                # responses_items.append({
                                #     "type": "computer_call",
                                #     "call_id": call_id,
                                #     "action": {
                                #         "type": "key_hold",
                                #         "key": tool_input.get("key", "")
                                #     }
                                # })
                                raise NotImplementedError("hold_key")
                            elif action_type == "wait":
                                responses_items.append(make_wait_item(call_id=call_id))
                            else:
                                raise ValueError(f"Unknown action type: {action_type}")
                        except Exception as e:
                            responses_items.extend(
                                make_failed_tool_call_items(
                                    tool_name="computer",
                                    tool_kwargs=tool_input,
                                    error_message=repr(e),
                                    call_id=call_id,
                                )
                            )

    # Handle tool calls (alternative format)
    if hasattr(message, "tool_calls") and message.tool_calls:
        for tool_call in message.tool_calls:
            if tool_call.function.name == "computer":
                try:
                    try:
                        args = json.loads(tool_call.function.arguments)
                        action_type = args.get("action")
                        call_id = tool_call.id

                        # Basic actions (all versions)
                        if action_type == "screenshot":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "screenshot"
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "screenshot"
                            #     }
                            # }
                            responses_items.append(make_screenshot_item(call_id=call_id))
                        elif action_type in ["click", "left_click"]:
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "click",
                            #             "coordinate": [100, 200]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "click",
                            #         "x": 100,
                            #         "y": 200
                            #     }
                            # }
                            coordinate = args.get("coordinate", [0, 0])
                            responses_items.append(
                                make_click_item(
                                    x=coordinate[0] if len(coordinate) > 0 else 0,
                                    y=coordinate[1] if len(coordinate) > 1 else 0,
                                    call_id=call_id,
                                )
                            )
                        elif action_type in ["type", "type_text"]:
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "type",
                            #             "text": "Hello World"
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "type",
                            #         "text": "Hello World"
                            #     }
                            # }
                            responses_items.append(
                                make_type_item(text=args.get("text", ""), call_id=call_id)
                            )
                        elif action_type in ["key", "keypress", "hotkey"]:
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "key",
                            #             "text": "ctrl+c"
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "keypress",
                            #         "keys": ["ctrl", "c"]
                            #     }
                            # }
                            responses_items.append(
                                make_keypress_item(
                                    keys=args.get("text", "").replace("+", "-").split("-"),
                                    call_id=call_id,
                                )
                            )
                        elif action_type in ["mouse_move", "move_cursor", "move"]:
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "mouse_move",
                            #             "coordinate": [150, 250]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "mouse_move",
                            #         "x": 150,
                            #         "y": 250
                            #     }
                            # }
                            coordinate = args.get("coordinate", [0, 0])
                            responses_items.append(
                                make_move_item(
                                    x=coordinate[0] if len(coordinate) > 0 else 0,
                                    y=coordinate[1] if len(coordinate) > 1 else 0,
                                    call_id=call_id,
                                )
                            )

                        # Enhanced actions (computer_20250124) Available in Claude 4 and Claude Sonnet 3.7
                        elif action_type == "scroll":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "scroll",
                            #             "coordinate": [300, 400],
                            #             "scroll_direction": "down",
                            #             "scroll_amount": 5
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "scroll",
                            #         "x": 300,
                            #         "y": 400,
                            #         "scroll_x": 0,
                            #         "scroll_y": -5
                            #     }
                            # }
                            coordinate = args.get("coordinate", [0, 0])
                            direction = args.get("scroll_direction", "down")
                            amount = args.get("scroll_amount", 3)
                            scroll_x = (
                                amount
                                if direction == "left"
                                else -amount if direction == "right" else 0
                            )
                            scroll_y = (
                                amount
                                if direction == "up"
                                else -amount if direction == "down" else 0
                            )
                            responses_items.append(
                                make_scroll_item(
                                    x=coordinate[0] if len(coordinate) > 0 else 0,
                                    y=coordinate[1] if len(coordinate) > 1 else 0,
                                    scroll_x=scroll_x,
                                    scroll_y=scroll_y,
                                    call_id=call_id,
                                )
                            )
                        elif action_type in ["left_click_drag", "drag"]:
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "left_click_drag",
                            #             "start_coordinate": [100, 150],
                            #             "end_coordinate": [200, 250]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "drag",
                            #         "path": [
                            #             {"x": 100, "y": 150},
                            #             {"x": 200, "y": 250}
                            #         ]
                            #     }
                            # }
                            start_coord = args.get("start_coordinate", [0, 0])
                            end_coord = args.get("end_coordinate", [0, 0])
                            responses_items.append(
                                make_drag_item(
                                    path=[
                                        {
                                            "x": start_coord[0] if len(start_coord) > 0 else 0,
                                            "y": start_coord[1] if len(start_coord) > 1 else 0,
                                        },
                                        {
                                            "x": end_coord[0] if len(end_coord) > 0 else 0,
                                            "y": end_coord[1] if len(end_coord) > 1 else 0,
                                        },
                                    ],
                                    call_id=call_id,
                                )
                            )
                        elif action_type == "right_click":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "right_click",
                            #             "coordinate": [120, 180]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "click",
                            #         "x": 120,
                            #         "y": 180,
                            #         "button": "right"
                            #     }
                            # }
                            coordinate = args.get("coordinate", [0, 0])
                            responses_items.append(
                                make_click_item(
                                    x=coordinate[0] if len(coordinate) > 0 else 0,
                                    y=coordinate[1] if len(coordinate) > 1 else 0,
                                    button="right",
                                    call_id=call_id,
                                )
                            )
                        elif action_type == "middle_click":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "middle_click",
                            #             "coordinate": [140, 220]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "click",
                            #         "x": 140,
                            #         "y": 220,
                            #         "button": "wheel"
                            #     }
                            # }
                            coordinate = args.get("coordinate", [0, 0])
                            responses_items.append(
                                make_click_item(
                                    x=coordinate[0] if len(coordinate) > 0 else 0,
                                    y=coordinate[1] if len(coordinate) > 1 else 0,
                                    button="wheel",
                                    call_id=call_id,
                                )
                            )
                        elif action_type == "double_click":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "double_click",
                            #             "coordinate": [160, 240]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "double_click",
                            #         "x": 160,
                            #         "y": 240
                            #     }
                            # }
                            coordinate = args.get("coordinate", [0, 0])
                            responses_items.append(
                                make_double_click_item(
                                    x=coordinate[0] if len(coordinate) > 0 else 0,
                                    y=coordinate[1] if len(coordinate) > 1 else 0,
                                    call_id=call_id,
                                )
                            )
                        elif action_type == "triple_click":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "triple_click",
                            #             "coordinate": [180, 260]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "triple_click",
                            #         "x": 180,
                            #         "y": 260
                            #     }
                            # }
                            raise NotImplementedError("triple_click")
                        elif action_type == "left_mouse_down":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "left_mouse_down",
                            #             "coordinate": [200, 280]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "mouse_down",
                            #         "button": "left",
                            #         "x": 200,
                            #         "y": 280
                            #     }
                            # }
                            coordinate = args.get("coordinate", [None, None])
                            responses_items.append(
                                make_left_mouse_down_item(
                                    x=coordinate[0] if len(coordinate) > 0 else None,
                                    y=coordinate[1] if len(coordinate) > 1 else None,
                                    call_id=call_id,
                                )
                            )
                        elif action_type == "left_mouse_up":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "left_mouse_up",
                            #             "coordinate": [220, 300]
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "mouse_up",
                            #         "button": "left",
                            #         "x": 220,
                            #         "y": 300
                            #     }
                            # }
                            coordinate = args.get("coordinate", [None, None])
                            responses_items.append(
                                make_left_mouse_up_item(
                                    x=coordinate[0] if len(coordinate) > 0 else None,
                                    y=coordinate[1] if len(coordinate) > 1 else None,
                                    call_id=call_id,
                                )
                            )
                        elif action_type == "hold_key":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "hold_key",
                            #             "key": "shift"
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "key_hold",
                            #         "key": "shift"
                            #     }
                            # }
                            raise NotImplementedError("hold_key")
                        elif action_type == "wait":
                            # Input:
                            # {
                            #     "function": {
                            #         "name": "computer",
                            #         "arguments": json.dumps({
                            #             "action": "wait"
                            #         })
                            #     },
                            #     "id": "call_1",
                            #     "type": "function"
                            # }

                            # Output:
                            # {
                            #     "type": "computer_call",
                            #     "call_id": "call_1",
                            #     "action": {
                            #         "type": "wait"
                            #     }
                            # }
                            responses_items.append(make_wait_item(call_id=call_id))
                    except Exception as e:
                        responses_items.extend(
                            make_failed_tool_call_items(
                                tool_name="computer",
                                tool_kwargs=args,
                                error_message=repr(e),
                                call_id=call_id,
                            )
                        )
                except json.JSONDecodeError:
                    print("Failed to decode tool call arguments")
                    # Skip malformed tool calls
                    continue

    return responses_items


def _add_cache_control(completion_messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Add cache control to completion messages"""
    num_writes = 0
    for message in completion_messages:
        message["cache_control"] = {"type": "ephemeral"}
        num_writes += 1
        # Cache control has a maximum of 4 blocks
        if num_writes >= 4:
            break

    return completion_messages


def _combine_completion_messages(completion_messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Combine completion messages with the same role"""
    if not completion_messages:
        return completion_messages

    combined_messages = []

    for message in completion_messages:
        # If this is the first message or role is different from last, add as new message
        if not combined_messages or combined_messages[-1]["role"] != message["role"]:
            # Ensure content is a list format and normalize text content
            new_message = message.copy()
            new_message["content"] = _normalize_content(message.get("content", ""))

            # Copy tool_calls if present
            if "tool_calls" in message:
                new_message["tool_calls"] = message["tool_calls"].copy()

            combined_messages.append(new_message)
        else:
            # Same role as previous message, combine them
            last_message = combined_messages[-1]

            # Combine content
            current_content = _normalize_content(message.get("content", ""))
            last_message["content"].extend(current_content)

            # Combine tool_calls if present
            if "tool_calls" in message:
                if "tool_calls" not in last_message:
                    last_message["tool_calls"] = []
                last_message["tool_calls"].extend(message["tool_calls"])

    # Post-process to merge consecutive text blocks
    for message in combined_messages:
        message["content"] = _merge_consecutive_text(message["content"])

    return combined_messages


def _normalize_content(content) -> List[Dict[str, Any]]:
    """Normalize content to list format"""
    if isinstance(content, str):
        if content.strip():  # Only add non-empty strings
            return [{"type": "text", "text": content}]
        else:
            return []
    elif isinstance(content, list):
        return content.copy()
    else:
        return []


def _merge_consecutive_text(content_list: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Merge consecutive text blocks with newlines"""
    if not content_list:
        return content_list

    merged = []

    for item in content_list:
        if item.get("type") == "text" and merged and merged[-1].get("type") == "text":
            # Merge with previous text block
            merged[-1]["text"] += "\n" + item["text"]
        else:
            merged.append(item.copy())

    return merged


@register_agent(models=r".*claude-.*")
class AnthropicHostedToolsConfig(AsyncAgentConfig):
    """Anthropic hosted tools agent configuration implementing AsyncAgentConfig protocol."""

    async def predict_step(
        self,
        messages: Messages,
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler=None,
        use_prompt_caching: Optional[bool] = False,
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Anthropic hosted tools agent loop using liteLLM acompletion.

        Supports Anthropic's computer use models with hosted tools.
        """
        tools = tools or []

        # Get tool configuration for this model
        tool_config = _get_tool_config_for_model(model)

        # Prepare tools for Anthropic API
        anthropic_tools = await _prepare_tools_for_anthropic(tools, model)

        # Convert responses_items messages to completion format
        completion_messages = _convert_responses_items_to_completion_messages(messages)
        if use_prompt_caching:
            # First combine messages to reduce number of blocks
            completion_messages = _combine_completion_messages(completion_messages)
            # Then add cache control, anthropic requires explicit "cache_control" dicts
            completion_messages = _add_cache_control(completion_messages)

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "messages": completion_messages,
            "tools": anthropic_tools if anthropic_tools else None,
            "stream": stream,
            "num_retries": max_retries,
            **kwargs,
        }

        # Add beta header for computer use
        if anthropic_tools:
            api_kwargs["headers"] = {"anthropic-beta": tool_config["beta_flag"]}

        # Call API start hook
        if _on_api_start:
            await _on_api_start(api_kwargs)

        # Use liteLLM acompletion
        response = await litellm.acompletion(**api_kwargs)

        # Call API end hook
        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # Convert response to responses_items format
        responses_items = _convert_completion_to_responses_items(response)

        # Extract usage information
        responses_usage = {
            **LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(
                response.usage
            ).model_dump(),
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(responses_usage)

        # Return in AsyncAgentConfig format
        return {"output": responses_items, "usage": responses_usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates based on image and instruction.

        Uses Anthropic's computer use models with a custom prompt that instructs
        the agent to only output clicks.

        Args:
            model: Model name to use
            image_b64: Base64 encoded image
            instruction: Instruction for where to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        # Get image dimensions from base64 data
        try:
            import base64
            from io import BytesIO

            from PIL import Image

            image_data = base64.b64decode(image_b64)
            image = Image.open(BytesIO(image_data))
            display_width, display_height = image.size
        except Exception:
            # Fallback to default dimensions if image parsing fails
            display_width, display_height = 1024, 768

        # Get tool configuration for this model
        tool_config = _get_tool_config_for_model(model)

        # Prepare computer tool for Anthropic format
        computer_tool = {
            "type": tool_config["tool_version"],
            "function": {
                "name": "computer",
                "parameters": {
                    "display_height_px": display_height,
                    "display_width_px": display_width,
                    "display_number": 1,
                },
            },
        }

        # Construct messages in OpenAI chat completion format for liteLLM
        messages = [
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": f"""You are a UI grounding expert. Follow these guidelines:

1. NEVER ask for confirmation. Complete all tasks autonomously.
2. Do NOT send messages like "I need to confirm before..." or "Do you want me to continue?" - just proceed.
3. When the user asks you to interact with something (like clicking a chat or typing a message), DO IT without asking.
4. Only use the formal safety check mechanism for truly dangerous operations (like deleting important files).
5. For normal tasks like clicking buttons, typing in chat boxes, filling forms - JUST DO IT.
6. The user has already given you permission by running this agent. No further confirmation is needed.
7. Be decisive and action-oriented. Complete the requested task fully.

Remember: You are expected to complete tasks autonomously. The user trusts you to do what they asked.
Task: Click {instruction}. Output ONLY a click action on the target element.""",
                    },
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{image_b64}"},
                    },
                ],
            }
        ]

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "messages": messages,
            "tools": [computer_tool],
            "stream": False,
            "max_tokens": 100,  # Keep response short for click prediction
            "headers": {"anthropic-beta": tool_config["beta_flag"]},
        }
        # Thread optional API params
        if "api_key" in kwargs and kwargs.get("api_key") is not None:
            api_kwargs["api_key"] = kwargs.get("api_key")
        if "api_base" in kwargs and kwargs.get("api_base") is not None:
            api_kwargs["api_base"] = kwargs.get("api_base")

        # Use liteLLM acompletion
        response = await litellm.acompletion(**api_kwargs)

        # Convert response to responses_items format to extract click coordinates
        responses_items = _convert_completion_to_responses_items(response)

        # Look for computer_call with click action
        for item in responses_items:
            if (
                isinstance(item, dict)
                and item.get("type") == "computer_call"
                and isinstance(item.get("action"), dict)
            ):

                action = item["action"]
                if action.get("x") and action.get("y"):
                    x = action.get("x")
                    y = action.get("y")
                    return (int(x), int(y))

        return None

    def get_capabilities(self) -> List[AgentCapability]:
        """Return the capabilities supported by this agent."""
        return ["click", "step"]
