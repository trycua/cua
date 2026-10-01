"""
Composed-grounded agent loop implementation that combines grounding and thinking models.
Uses a two-stage approach: grounding model for element detection, thinking model for reasoning.
"""

import asyncio
import base64
import json
import uuid
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

import litellm
from PIL import Image

from ..agent import find_agent_config
from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_computer_calls_desc2xy,
    convert_computer_calls_xy2desc,
    convert_responses_items_to_completion_messages,
    get_all_element_descriptions,
)
from ..types import AgentCapability, AgentResponse, Messages, Tools

GROUNDED_COMPUTER_TOOL_SCHEMA = {
    "type": "function",
    "function": {
        "name": "computer",
        "description": "Control a computer by taking screenshots and interacting with UI elements. This tool uses element descriptions to locate and interact with UI elements on the screen (e.g., 'red submit button', 'search text field', 'hamburger menu icon', 'close button in top right corner').",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "screenshot",
                        "click",
                        "double_click",
                        "drag",
                        "type",
                        "keypress",
                        "scroll",
                        "move",
                        "wait",
                        "get_current_url",
                        "get_dimensions",
                        "get_environment",
                    ],
                    "description": "The action to perform (required for all actions)",
                },
                "element_description": {
                    "type": "string",
                    "description": "Description of the element to interact with (required for click, double_click, move, scroll actions)",
                },
                "start_element_description": {
                    "type": "string",
                    "description": "Description of the element to start dragging from (required for drag action)",
                },
                "end_element_description": {
                    "type": "string",
                    "description": "Description of the element to drag to (required for drag action)",
                },
                "text": {
                    "type": "string",
                    "description": "The text to type (required for type action)",
                },
                "keys": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Key(s) to press (required for keypress action)",
                },
                "button": {
                    "type": "string",
                    "enum": ["left", "right", "wheel", "back", "forward"],
                    "description": "The mouse button to use for click action (required for click and double_click action)",
                },
                "scroll_x": {
                    "type": "integer",
                    "description": "Horizontal scroll amount for scroll action (required for scroll action)",
                },
                "scroll_y": {
                    "type": "integer",
                    "description": "Vertical scroll amount for scroll action (required for scroll action)",
                },
            },
            "required": ["action"],
        },
    },
}


def _prepare_tools_for_grounded(tool_schemas: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Prepare tools for grounded API format"""
    grounded_tools = []

    for schema in tool_schemas:
        if schema["type"] == "computer":
            grounded_tools.append(GROUNDED_COMPUTER_TOOL_SCHEMA)
        else:
            grounded_tools.append(schema)

    return grounded_tools


def get_last_computer_call_image(messages: List[Dict[str, Any]]) -> Optional[str]:
    """Get the last computer call output image from messages."""
    for message in reversed(messages):
        if (
            isinstance(message, dict)
            and message.get("type") == "computer_call_output"
            and isinstance(message.get("output"), dict)
            and message["output"].get("type") == "input_image"
        ):
            image_url = message["output"].get("image_url", "")
            if image_url.startswith("data:image/png;base64,"):
                return image_url.split(",", 1)[1]
    return None


@register_agent(r".*\+.*", priority=1)
class ComposedGroundedConfig(AsyncAgentConfig):
    """
    Composed-grounded agent configuration that uses both grounding and thinking models.

    The model parameter should be in format: "grounding_model+thinking_model"
    e.g., "huggingface-local/HelloKKMe/GTA1-7B+gemini/gemini-1.5-pro"
    """

    def __init__(self):
        self.desc2xy: Dict[str, Tuple[float, float]] = {}

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
        Composed-grounded predict step implementation.

        Process:
        0. Store last computer call image, if none then take a screenshot
        1. Convert computer calls from xy to descriptions
        2. Convert responses items to completion messages
        3. Call thinking model with litellm.acompletion
        4. Convert completion messages to responses items
        5. Get all element descriptions and populate desc2xy mapping
        6. Convert computer calls from descriptions back to xy coordinates
        7. Return output and usage
        """
        # Parse the composed model
        if "+" not in model:
            raise ValueError(
                f"Composed model must be in format 'grounding_model+thinking_model', got: {model}"
            )
        grounding_model, thinking_model = model.split("+", 1)

        pre_output_items = []

        # Step 0: Store last computer call image, if none then take a screenshot
        last_image_b64 = get_last_computer_call_image(messages)
        if last_image_b64 is None:
            # Take a screenshot
            screenshot_b64 = await computer_handler.screenshot()  # type: ignore
            if screenshot_b64:

                call_id = uuid.uuid4().hex
                pre_output_items += [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "output_text",
                                "text": "Taking a screenshot to see the current computer screen.",
                            }
                        ],
                    },
                    {
                        "action": {"type": "screenshot"},
                        "call_id": call_id,
                        "status": "completed",
                        "type": "computer_call",
                    },
                    {
                        "type": "computer_call_output",
                        "call_id": call_id,
                        "output": {
                            "type": "input_image",
                            "image_url": f"data:image/png;base64,{screenshot_b64}",
                        },
                    },
                ]
                last_image_b64 = screenshot_b64

                # Call screenshot callback if provided
                if _on_screenshot:
                    await _on_screenshot(screenshot_b64)

        tool_schemas = _prepare_tools_for_grounded(tools)  # type: ignore

        # Step 1: Convert computer calls from xy to descriptions
        input_messages = messages + pre_output_items
        messages_with_descriptions = convert_computer_calls_xy2desc(input_messages, self.desc2xy)

        # Step 2: Convert responses items to completion messages
        completion_messages = convert_responses_items_to_completion_messages(
            messages_with_descriptions, allow_images_in_tool_results=False
        )

        # Step 3: Call thinking model with litellm.acompletion
        api_kwargs = {
            "model": thinking_model,
            "messages": completion_messages,
            "tools": tool_schemas,
            "max_retries": max_retries,
            "stream": stream,
            **kwargs,
        }

        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = use_prompt_caching

        # Call API start hook
        if _on_api_start:
            await _on_api_start(api_kwargs)

        # Make the completion call
        response = await litellm.acompletion(**api_kwargs)

        # Call API end hook
        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # Extract usage information
        usage = {
            **response.usage.model_dump(),  # type: ignore
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(usage)

        # Step 4: Convert completion messages back to responses items format
        response_dict = response.model_dump()  # type: ignore
        choice_messages = [choice["message"] for choice in response_dict["choices"]]
        thinking_output_items = []

        for choice_message in choice_messages:
            thinking_output_items.extend(
                convert_completion_messages_to_responses_items([choice_message])
            )

        # Step 5: Get all element descriptions and populate desc2xy mapping
        element_descriptions = get_all_element_descriptions(thinking_output_items)

        if element_descriptions and last_image_b64:
            # Use grounding model to predict coordinates for each description
            grounding_agent_conf = find_agent_config(grounding_model)
            if grounding_agent_conf:
                grounding_agent = grounding_agent_conf.agent_class()

                for desc in element_descriptions:
                    for _ in range(3):  # try 3 times
                        coords = await grounding_agent.predict_click(
                            model=grounding_model, image_b64=last_image_b64, instruction=desc
                        )
                        if coords:
                            self.desc2xy[desc] = coords
                            break

        # Step 6: Convert computer calls from descriptions back to xy coordinates
        final_output_items = convert_computer_calls_desc2xy(thinking_output_items, self.desc2xy)

        # Step 7: Return output and usage
        return {"output": pre_output_items + final_output_items, "usage": usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using the grounding model.

        For composed models, uses only the grounding model part for click prediction.
        """
        # Parse the composed model to get grounding model
        if "+" not in model:
            raise ValueError(
                f"Composed model must be in format 'grounding_model+thinking_model', got: {model}"
            )
        grounding_model, thinking_model = model.split("+", 1)

        # Find and use the grounding agent
        grounding_agent_conf = find_agent_config(grounding_model)
        if grounding_agent_conf:
            grounding_agent = grounding_agent_conf.agent_class()
            return await grounding_agent.predict_click(
                model=grounding_model, image_b64=image_b64, instruction=instruction, **kwargs
            )

        return None

    def get_capabilities(self) -> List[AgentCapability]:
        """Return the capabilities supported by this agent."""
        return ["click", "step"]
