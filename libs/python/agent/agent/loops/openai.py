"""
OpenAI computer-use-preview agent loop implementation using liteLLM
"""

import asyncio
import base64
import json
from io import BytesIO
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, Union

import litellm
from PIL import Image

from ..decorators import register_agent
from ..types import AgentCapability, AgentResponse, Messages, Tools


async def _map_computer_tool_to_openai(
    computer_handler: Any, use_native_tool: bool = True
) -> Dict[str, Any]:
    """Map a computer tool to OpenAI's tool schema.

    Args:
        computer_handler: The computer handler instance
        use_native_tool: If True, use native computer_use_preview format (for computer-use-preview model).
                        If False, use standard function calling format (for GPT-5.4 etc).
    """
    # Get dimensions from the computer handler
    try:
        width, height = await computer_handler.get_dimensions()
    except Exception:
        # Fallback to default dimensions if method fails
        width, height = 1024, 768

    # Get environment from the computer handler
    try:
        environment = await computer_handler.get_environment()
    except Exception:
        # Fallback to default environment if method fails
        environment = "linux"

    if use_native_tool:
        # Native computer_use_preview format (for computer-use-preview model)
        return {
            "type": "computer_use_preview",
            "display_width": width,
            "display_height": height,
            "environment": environment,  # mac, windows, linux, browser
        }
    else:
        # Standard function calling format (for GPT-5.4 etc)
        # Responses API requires: {type, name, description, parameters} at root level
        return {
            "type": "function",
            "name": "computer",
            "description": (
                f"Use a mouse and keyboard to interact with a computer, and take screenshots.\n"
                f"Screen resolution: {width}x{height} pixels.\n"
                f"Environment: {environment}."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "description": "The action to perform.",
                        "type": "string",
                        "enum": [
                            "click",
                            "double_click",
                            "right_click",
                            "type",
                            "keypress",
                            "scroll",
                            "move",
                            "drag",
                            "screenshot",
                            "wait",
                            "terminate",
                        ],
                    },
                    "x": {
                        "description": "X coordinate for click/move/scroll actions.",
                        "type": "integer",
                    },
                    "y": {
                        "description": "Y coordinate for click/move/scroll actions.",
                        "type": "integer",
                    },
                    "text": {
                        "description": "Text to type (for action=type).",
                        "type": "string",
                    },
                    "keys": {
                        "description": "Keys to press (for action=keypress). Example: ['ctrl', 'c']",
                        "type": "array",
                        "items": {"type": "string"},
                    },
                    "scroll_x": {
                        "description": "Horizontal scroll amount. Positive=right, negative=left.",
                        "type": "integer",
                    },
                    "scroll_y": {
                        "description": "Vertical scroll amount. Positive=down, negative=up.",
                        "type": "integer",
                    },
                    "button": {
                        "description": "Mouse button for click action.",
                        "type": "string",
                        "enum": ["left", "right", "middle"],
                    },
                    "start_x": {
                        "description": "Starting X coordinate for drag action.",
                        "type": "integer",
                    },
                    "start_y": {
                        "description": "Starting Y coordinate for drag action.",
                        "type": "integer",
                    },
                    "end_x": {
                        "description": "Ending X coordinate for drag action.",
                        "type": "integer",
                    },
                    "end_y": {
                        "description": "Ending Y coordinate for drag action.",
                        "type": "integer",
                    },
                    "status": {
                        "description": "Status for terminate action.",
                        "type": "string",
                        "enum": ["success", "failure"],
                    },
                },
                "required": ["action"],
            },
        }


def _is_native_computer_use_model(model: str) -> bool:
    """Check if the model supports native computer_use_preview tool format."""
    import re

    # Only computer-use-preview models support native computer_use_preview tool
    # GPT 5.4 does NOT support computer_use_preview - it uses function calling
    return bool(re.search(r"computer-use-preview", model, re.IGNORECASE))


async def _prepare_tools_for_openai(tool_schemas: List[Dict[str, Any]], model: str = "") -> Tools:
    """Prepare tools for OpenAI API format.

    Args:
        tool_schemas: List of tool schemas to prepare
        model: Model name to determine tool format
    """
    openai_tools = []
    use_native = _is_native_computer_use_model(model)

    for schema in tool_schemas:
        if schema["type"] == "computer":
            # Map computer tool to OpenAI format (native or function based on model)
            computer_tool = await _map_computer_tool_to_openai(
                schema["computer"], use_native_tool=use_native
            )
            openai_tools.append(computer_tool)
        elif schema["type"] == "function":
            # Function tools for Responses API need: {type, name, description, parameters}
            # Note: parameters are at the root level, NOT nested under 'function'
            func = schema["function"]
            openai_tools.append(
                {
                    "type": "function",
                    "name": func["name"],
                    "description": func.get("description", ""),
                    "parameters": func.get("parameters", {}),
                }
            )

    return openai_tools


@register_agent(models=r".*(computer-use-preview|gpt-?5\.?4)")
class OpenAIComputerUseConfig:
    """
    OpenAI computer-use-preview agent configuration using liteLLM responses.

    Supports OpenAI's computer use preview models.
    """

    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
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
        Predict the next step based on input items.

        Args:
            messages: Input items following Responses format
            model: Model name to use
            tools: Optional list of tool schemas
            max_retries: Maximum number of retries
            stream: Whether to stream responses
            computer_handler: Computer handler instance
            _on_api_start: Callback for API start
            _on_api_end: Callback for API end
            _on_usage: Callback for usage tracking
            _on_screenshot: Callback for screenshot events
            **kwargs: Additional arguments

        Returns:
            Dictionary with "output" (output items) and "usage" array
        """
        tools = tools or []

        # Prepare tools for OpenAI API
        openai_tools = await _prepare_tools_for_openai(tools, model=model)

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "input": messages,
            "tools": openai_tools if openai_tools else None,
            "stream": stream,
            "reasoning": {"summary": "concise"},
            "truncation": "auto",
            "num_retries": max_retries,
            **kwargs,
        }

        # Call API start hook
        if _on_api_start:
            await _on_api_start(api_kwargs)

        # Use liteLLM responses
        response = await litellm.aresponses(**api_kwargs)

        # Call API end hook
        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # Extract usage information - handle both dict and Pydantic model responses
        if isinstance(response, dict):
            response_usage = response.get("usage", {})
            usage = response_usage if isinstance(response_usage, dict) else {}
            output_dict = response
        else:
            # Response is a Pydantic model - but usage might be dict or model
            response_usage = response.usage
            if hasattr(response_usage, "model_dump"):
                usage = response_usage.model_dump()
            elif isinstance(response_usage, dict):
                usage = response_usage
            else:
                usage = {}
            output_dict = response.model_dump()

        # Add response cost if available
        if hasattr(response, "_hidden_params"):
            usage["response_cost"] = response._hidden_params.get("response_cost", 0.0)
        elif isinstance(response, dict):
            usage["response_cost"] = response.get("_hidden_params", {}).get("response_cost", 0.0)

        if _on_usage:
            await _on_usage(usage)

        # Return in the expected format
        output_dict["usage"] = usage
        return output_dict

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates based on image and instruction.

        Uses OpenAI computer-use-preview with manually constructed input items
        and a prompt that instructs the agent to only output clicks.

        Args:
            model: Model name to use
            image_b64: Base64 encoded image
            instruction: Instruction for where to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        # TODO: use computer tool to get dimensions + environment
        # Manually construct input items with image and click instruction
        input_items = [
            {
                "role": "user",
                "content": f"""You are a UI grounding expert. Follow these guidelines:

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
                "role": "user",
                "content": [
                    {"type": "input_image", "image_url": f"data:image/png;base64,{image_b64}"}
                ],
            },
        ]

        # Get image dimensions from base64 data
        try:
            image_data = base64.b64decode(image_b64)
            image = Image.open(BytesIO(image_data))
            display_width, display_height = image.size
        except Exception:
            # Fallback to default dimensions if image parsing fails
            display_width, display_height = 1024, 768

        # Prepare computer tool for click actions - use native format only for models that support it
        use_native = _is_native_computer_use_model(model)
        if use_native:
            # Native computer_use_preview format (for computer-use-preview model)
            computer_tool = {
                "type": "computer_use_preview",
                "display_width": display_width,
                "display_height": display_height,
                "environment": "windows",
            }
        else:
            # Standard function calling format (for GPT-5.4 etc)
            computer_tool = {
                "type": "function",
                "name": "computer",
                "description": (
                    f"Use a mouse and keyboard to interact with a computer, and take screenshots.\n"
                    f"Screen resolution: {display_width}x{display_height} pixels.\n"
                    f"Environment: windows."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "description": "The action to perform.",
                            "type": "string",
                            "enum": ["click"],
                        },
                        "x": {
                            "description": "X coordinate for click action.",
                            "type": "integer",
                        },
                        "y": {
                            "description": "Y coordinate for click action.",
                            "type": "integer",
                        },
                    },
                    "required": ["action", "x", "y"],
                },
            }

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "input": input_items,
            "tools": [computer_tool],
            "stream": False,
            "reasoning": {"summary": "concise"},
            "truncation": "auto",
            "max_tokens": 200,  # Keep response short for click prediction
            **kwargs,
        }

        # Use liteLLM responses
        response = await litellm.aresponses(**api_kwargs)

        # Extract click coordinates from response output - handle both dict and Pydantic model
        output_dict = response if isinstance(response, dict) else response.model_dump()
        output_items = output_dict.get("output", [])

        # Look for click coordinates in the response
        for item in output_items:
            if not isinstance(item, dict):
                continue

            # Native format: computer_call with action dict
            if item.get("type") == "computer_call" and isinstance(item.get("action"), dict):
                action = item["action"]
                if action.get("x") is not None and action.get("y") is not None:
                    return (int(action.get("x")), int(action.get("y")))

            # Function calling format: function_call with arguments
            if item.get("type") == "function_call" and item.get("name") == "computer":
                try:
                    arguments = item.get("arguments", "{}")
                    if isinstance(arguments, str):
                        args = json.loads(arguments)
                    else:
                        args = arguments
                    if args.get("x") is not None and args.get("y") is not None:
                        return (int(args.get("x")), int(args.get("y")))
                except (json.JSONDecodeError, TypeError):
                    continue

        return None

    def get_capabilities(self) -> List[AgentCapability]:
        """
        Get list of capabilities supported by this agent config.

        Returns:
            List of capability strings
        """
        return ["click", "step"]
