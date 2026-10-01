"""
GLM-4.5V agent loop implementation using liteLLM for GLM-4.5V model.
Supports vision-language models for computer control with bounding box parsing.
"""

import asyncio
import base64
import json
import re
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)
from litellm.types.utils import ModelResponse
from PIL import Image

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
    make_click_item,
    make_double_click_item,
    make_drag_item,
    make_input_image_item,
    make_keypress_item,
    make_output_text_item,
    make_reasoning_item,
    make_scroll_item,
    make_type_item,
    make_wait_item,
)
from ..types import AgentCapability, AgentResponse, Messages, Tools

# GLM-4.5V specific constants
GLM_ACTION_SPACE = """
### {left,right,middle}_click

Call rule: `{left,right,middle}_click(start_box='[x,y]', element_info='')`
{
    'name': ['left_click', 'right_click', 'middle_click'],
    'description': 'Perform a left/right/middle mouse click at the specified coordinates on the screen.',
    'parameters': {
        'type': 'object',
        'properties': {
            'start_box': {
                'type': 'array',
                'items': {
                    'type': 'integer'
                },
                'description': 'Coordinates [x,y] where to perform the click, normalized to 0-999 range.'
            },
            'element_info': {
                'type': 'string',
                'description': 'Optional text description of the UI element being clicked.'
            }
        },
        'required': ['start_box']
    }
}

### hover

Call rule: `hover(start_box='[x,y]', element_info='')`
{
    'name': 'hover',
    'description': 'Move the mouse pointer to the specified coordinates without performing any click action.',
    'parameters': {
        'type': 'object',
        'properties': {
            'start_box': {
                'type': 'array',
                'items': {
                    'type': 'integer'
                },
                'description': 'Coordinates [x,y] where to move the mouse pointer, normalized to 0-999 range.'
            },
            'element_info': {
                'type': 'string',
                'description': 'Optional text description of the UI element being hovered over.'
            }
        },
        'required': ['start_box']
    }
}

### left_double_click

Call rule: `left_double_click(start_box='[x,y]', element_info='')`
{
    'name': 'left_double_click',
    'description': 'Perform a left mouse double-click at the specified coordinates on the screen.',
    'parameters': {
        'type': 'object',
        'properties': {
            'start_box': {
                'type': 'array',
                'items': {
                    'type': 'integer'
                },
                'description': 'Coordinates [x,y] where to perform the double-click, normalized to 0-999 range.'
            },
            'element_info': {
                'type': 'string',
                'description': 'Optional text description of the UI element being double-clicked.'
            }
        },
        'required': ['start_box']
    }
}

### left_drag

Call rule: `left_drag(start_box='[x1,y1]', end_box='[x2,y2]', element_info='')`
{
    'name': 'left_drag',
    'description': 'Drag the mouse from starting coordinates to ending coordinates while holding the left mouse button.',
    'parameters': {
        'type': 'object',
        'properties': {
            'start_box': {
                'type': 'array',
                'items': {
                    'type': 'integer'
                },
                'description': 'Starting coordinates [x1,y1] for the drag operation, normalized to 0-999 range.'
            },
            'end_box': {
                'type': 'array',
                'items': {
                    'type': 'integer'
                },
                'description': 'Ending coordinates [x2,y2] for the drag operation, normalized to 0-999 range.'
            },
            'element_info': {
                'type': 'string',
                'description': 'Optional text description of the UI element being dragged.'
            }
        },
        'required': ['start_box', 'end_box']
    }
}

### key

Call rule: `key(keys='')`
{
    'name': 'key',
    'description': 'Simulate pressing a single key or combination of keys on the keyboard.',
    'parameters': {
        'type': 'object',
        'properties': {
            'keys': {
                'type': 'string',
                'description': 'The key or key combination to press. Use '+' to separate keys in combinations (e.g., 'ctrl+c', 'alt+tab').'
            }
        },
        'required': ['keys']
    }
}

### type

Call rule: `type(content='')`
{
    'name': 'type',
    'description': 'Type text content into the currently focused text input field. This action only performs typing and does not handle field activation or clearing.',
    'parameters': {
        'type': 'object',
        'properties': {
            'content': {
                'type': 'string',
                'description': 'The text content to be typed into the active text field.'
            }
        },
        'required': ['content']
    }
}

### scroll

Call rule: `scroll(start_box='[x,y]', direction='', step=5, element_info='')`
{
    'name': 'scroll',
    'description': 'Scroll an element at the specified coordinates in the specified direction by a given number of wheel steps.',
    'parameters': {
        'type': 'object',
        'properties': {
            'start_box': {
                'type': 'array',
                'items': {
                    'type': 'integer'
                },
                'description': 'Coordinates [x,y] of the element or area to scroll, normalized to 0-999 range.'
            },
            'direction': {
                'type': 'string',
                'enum': ['down', 'up'],
                'description': 'The direction to scroll: 'down' or 'up'.'
            },
            'step': {
                'type': 'integer',
                'default': 5,
                'description': 'Number of wheel steps to scroll, default is 5.'
            },
            'element_info': {
                'type': 'string',
                'description': 'Optional text description of the UI element being scrolled.'
            }
        },
        'required': ['start_box', 'direction']
    }
}

### WAIT

Call rule: `WAIT()`
{
    'name': 'WAIT',
    'description': 'Wait for 5 seconds before proceeding to the next action.',
    'parameters': {
        'type': 'object',
        'properties': {},
        'required': []
    }
}

### DONE

Call rule: `DONE()`
{
    'name': 'DONE',
    'description': 'Indicate that the current task has been completed successfully and no further actions are needed.',
    'parameters': {
        'type': 'object',
        'properties': {},
        'required': []
    }
}

### FAIL

Call rule: `FAIL()`
{
    'name': 'FAIL',
    'description': 'Indicate that the current task cannot be completed or is impossible to accomplish.',
    'parameters': {
        'type': 'object',
        'properties': {},
        'required': []
    }
}"""


def encode_image_to_base64(image_path: str) -> str:
    """Encode image file to base64 string with data URI."""
    with open(image_path, "rb") as image_file:
        encoded_string = base64.b64encode(image_file.read()).decode("utf-8")
        return f"data:image/png;base64,{encoded_string}"


def parse_glm_response(response: str) -> Dict[str, Any]:
    """
    Parse GLM-4.5V response to extract action and memory.

    The special tokens <|begin_of_box|> and <|end_of_box|> mark bounding boxes.
    Coordinates are normalized values between 0 and 1000.
    """
    # Extract action from between special tokens
    pattern = r"<\|begin_of_box\|>(.*?)<\|end_of_box\|>"
    match = re.search(pattern, response)
    if match:
        action = match.group(1).strip()
    else:
        # Fallback: look for function call patterns
        action_pattern = r"[\w_]+\([^)]*\)"
        matches = re.findall(action_pattern, response)
        action = matches[0] if matches else None

    # Extract memory section
    memory_pattern = r"Memory:(.*?)$"
    memory_match = re.search(memory_pattern, response, re.DOTALL)
    memory = memory_match.group(1).strip() if memory_match else "[]"

    # Extract action text (everything before Memory:)
    action_text_pattern = r"^(.*?)Memory:"
    action_text_match = re.search(action_text_pattern, response, re.DOTALL)
    action_text = action_text_match.group(1).strip() if action_text_match else response

    # Clean up action text by removing special tokens
    if action_text:
        action_text = action_text.replace("<|begin_of_box|>", "").replace("<|end_of_box|>", "")

    return {"action": action, "action_text": action_text, "memory": memory}


def get_last_image_from_messages(messages: Messages) -> Optional[str]:
    """Extract the last image from messages for processing."""
    for message in reversed(messages):
        if isinstance(message, dict):
            if message.get("type") == "computer_call_output":
                output = message.get("output", {})
                if isinstance(output, dict) and output.get("type") == "input_image":
                    image_url = output.get("image_url", "")
                    if isinstance(image_url, str) and image_url.startswith("data:image/"):
                        # Extract base64 part
                        return image_url.split(",", 1)[1]
            elif message.get("role") == "user":
                content = message.get("content", [])
                if isinstance(content, list):
                    for item in reversed(content):
                        if isinstance(item, dict) and item.get("type") == "image_url":
                            image_url_obj = item.get("image_url", {})
                            if isinstance(image_url_obj, dict):
                                image_url = image_url_obj.get("url", "")
                                if isinstance(image_url, str) and image_url.startswith(
                                    "data:image/"
                                ):
                                    return image_url.split(",", 1)[1]
    return None


def convert_responses_items_to_glm45v_pc_prompt(
    messages: Messages, task: str, memory: str = ""
) -> List[Dict[str, Any]]:
    """Convert responses items to GLM-4.5V PC prompt format with historical actions.

    Args:
        messages: List of message items from the conversation
        task: The task description
        memory: Current memory state

    Returns:
        List of content items for the prompt (text and image_url items)
    """
    action_space = GLM_ACTION_SPACE

    # Template head
    head_text = f"""You are a GUI Agent, and your primary task is to respond accurately to user requests or questions. In addition to directly answering the user's queries, you can also use tools or perform GUI operations directly until you fulfill the user's request or provide a correct answer. You should carefully read and understand the images and questions provided by the user, and engage in thinking and reflection when appropriate. The coordinates involved are all represented in thousandths (0-999).

# Task:
{task}

# Task Platform
Ubuntu

# Action Space
{action_space}

# Historical Actions and Current Memory
History:"""

    # Template tail
    tail_text = f"""
Memory:
{memory}
# Output Format
Plain text explanation with action(param='...')
Memory:
[{{"key": "value"}}, ...]

# Some Additional Notes
- I'll give you the most recent 4 history screenshots(shrunked to 50%*50%) along with the historical action steps.
- You should put the key information you *have to remember* in a seperated memory part and I'll give it to you in the next round. The content in this part should be a dict list. If you no longer need some given information, you should remove it from the memory. Even if you don't need to remember anything, you should also output an empty list.
- My computer's password is "password", feel free to use it when you need sudo rights.
- For the thunderbird account "anonym-x2024@outlook.com", the password is "gTCI";=@y7|QJ0nDa_kN3Sb&>".

Current Screenshot:
"""

    # Build history from messages
    history = []
    history_images = []

    # Group messages into steps
    current_step = []
    step_num = 0

    for message in messages:
        msg_type = message.get("type")

        if msg_type == "reasoning":
            current_step.append(message)
        elif msg_type == "message" and message.get("role") == "assistant":
            current_step.append(message)
        elif msg_type == "computer_call":
            current_step.append(message)
        elif msg_type == "computer_call_output":
            current_step.append(message)
            # End of step - process it
            if current_step:
                step_num += 1

                # Extract bot thought from message content
                bot_thought = ""
                for item in current_step:
                    if item.get("type") == "message" and item.get("role") == "assistant":
                        content = item.get("content", [])
                        for content_item in content:
                            if content_item.get("type") == "output_text":
                                bot_thought = content_item.get("text", "")
                                break
                        break

                # Extract action from computer_call
                action_text = ""
                for item in current_step:
                    if item.get("type") == "computer_call":
                        action = item.get("action", {})
                        action_type = action.get("type", "")

                        if action_type == "click":
                            x, y = action.get("x", 0), action.get("y", 0)
                            # Convert to 0-999 range (assuming screen dimensions)
                            # For now, use direct coordinates - this may need adjustment
                            action_text = f"left_click(start_box='[{x},{y}]')"
                        elif action_type == "double_click":
                            x, y = action.get("x", 0), action.get("y", 0)
                            action_text = f"left_double_click(start_box='[{x},{y}]')"
                        elif action_type == "right_click":
                            x, y = action.get("x", 0), action.get("y", 0)
                            action_text = f"right_click(start_box='[{x},{y}]')"
                        elif action_type == "drag":
                            # Handle drag with path
                            path = action.get("path", [])
                            if len(path) >= 2:
                                start = path[0]
                                end = path[-1]
                                action_text = f"left_drag(start_box='[{start.get('x', 0)},{start.get('y', 0)}]', end_box='[{end.get('x', 0)},{end.get('y', 0)}]')"
                        elif action_type == "keypress":
                            key = action.get("key", "")
                            action_text = f"key(keys='{key}')"
                        elif action_type == "type":
                            text = action.get("text", "")
                            action_text = f"type(content='{text}')"
                        elif action_type == "scroll":
                            x, y = action.get("x", 0), action.get("y", 0)
                            direction = action.get("direction", "down")
                            action_text = f"scroll(start_box='[{x},{y}]', direction='{direction}')"
                        elif action_type == "wait":
                            action_text = "WAIT()"
                        break

                # Extract screenshot from computer_call_output
                screenshot_url = None
                for item in current_step:
                    if item.get("type") == "computer_call_output":
                        output = item.get("output", {})
                        if output.get("type") == "input_image":
                            screenshot_url = output.get("image_url", "")
                            break

                # Store step info
                step_info = {
                    "step_num": step_num,
                    "bot_thought": bot_thought,
                    "action_text": action_text,
                    "screenshot_url": screenshot_url,
                }
                history.append(step_info)

                # Store screenshot for last 4 steps
                if screenshot_url:
                    history_images.append(screenshot_url)

                current_step = []

    # Build content array with head, history, and tail
    content = []
    current_text = head_text

    total_history_steps = len(history)
    history_image_count = min(4, len(history_images))  # Last 4 images

    for step_idx, step_info in enumerate(history):
        step_num = step_info["step_num"]
        bot_thought = step_info["bot_thought"]
        action_text = step_info["action_text"]

        if step_idx < total_history_steps - history_image_count:
            # For steps beyond the last 4, use text placeholder
            current_text += f"\nstep {step_num}: Screenshot:(Omitted in context.) Thought: {bot_thought}\nAction: {action_text}"
        else:
            # For the last 4 steps, insert images
            current_text += f"\nstep {step_num}: Screenshot:"
            content.append({"type": "text", "text": current_text})

            # Add image
            img_idx = step_idx - (total_history_steps - history_image_count)
            if img_idx < len(history_images):
                content.append({"type": "image_url", "image_url": {"url": history_images[img_idx]}})

            current_text = f" Thought: {bot_thought}\nAction: {action_text}"

    # Add tail
    current_text += tail_text
    content.append({"type": "text", "text": current_text})

    return content


def model_dump(obj) -> Dict[str, Any]:
    if isinstance(obj, dict):
        return {k: model_dump(v) for k, v in obj.items()}
    elif hasattr(obj, "model_dump"):
        return obj.model_dump()
    else:
        return obj


def convert_glm_completion_to_responses_items(
    response: ModelResponse, image_width: int, image_height: int
) -> List[Dict[str, Any]]:
    """
    Convert GLM-4.5V completion response to responses items format.

    Args:
        response: LiteLLM ModelResponse from GLM-4.5V
        image_width: Original image width for coordinate scaling
        image_height: Original image height for coordinate scaling

    Returns:
        List of response items in the proper format
    """
    import uuid

    response_items = []

    if not response.choices or not response.choices[0].message:
        return response_items

    message = response.choices[0].message
    content = message.content or ""
    reasoning_content = getattr(message, "reasoning_content", None)

    # Add reasoning item if present
    if reasoning_content:
        reasoning_item = model_dump(make_reasoning_item(reasoning_content))
        response_items.append(reasoning_item)

    # Parse the content to extract action and text
    parsed_response = parse_glm_response(content)
    action = parsed_response.get("action", "")
    action_text = parsed_response.get("action_text", "")

    # Add message item with text content (excluding action and memory)
    if action_text:
        # Remove action from action_text if it's there
        clean_text = action_text
        if action and action in clean_text:
            clean_text = clean_text.replace(action, "").strip()

        # Remove memory section
        memory_pattern = r"Memory:\s*\[.*?\]\s*$"
        clean_text = re.sub(memory_pattern, "", clean_text, flags=re.DOTALL).strip()

        if clean_text:
            message_item = model_dump(make_output_text_item(clean_text))
            response_items.append(message_item)

    # Convert action to computer call if present
    if action:
        call_id = f"call_{uuid.uuid4().hex[:8]}"

        # Parse different action types and create appropriate computer calls
        if action.startswith("left_click"):
            coord_match = re.search(r"start_box='?\[(\d+),\s*(\d+)\]'?", action)
            if coord_match:
                x, y = int(coord_match.group(1)), int(coord_match.group(2))
                # Convert from 0-999 to actual pixel coordinates
                actual_x = int((x / 999.0) * image_width)
                actual_y = int((y / 999.0) * image_height)
                computer_call = model_dump(make_click_item(actual_x, actual_y))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action.startswith("right_click"):
            coord_match = re.search(r"start_box='?\[(\d+),\s*(\d+)\]'?", action)
            if coord_match:
                x, y = int(coord_match.group(1)), int(coord_match.group(2))
                actual_x = int((x / 999.0) * image_width)
                actual_y = int((y / 999.0) * image_height)
                computer_call = model_dump(make_click_item(actual_x, actual_y, button="right"))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action.startswith("left_double_click"):
            coord_match = re.search(r"start_box='?\[(\d+),\s*(\d+)\]'?", action)
            if coord_match:
                x, y = int(coord_match.group(1)), int(coord_match.group(2))
                actual_x = int((x / 999.0) * image_width)
                actual_y = int((y / 999.0) * image_height)
                computer_call = model_dump(make_double_click_item(actual_x, actual_y))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action.startswith("left_drag"):
            start_match = re.search(r"start_box='?\[(\d+),\s*(\d+)\]'?", action)
            end_match = re.search(r"end_box='?\[(\d+),\s*(\d+)\]'?", action)
            if start_match and end_match:
                x1, y1 = int(start_match.group(1)), int(start_match.group(2))
                x2, y2 = int(end_match.group(1)), int(end_match.group(2))
                actual_x1 = int((x1 / 999.0) * image_width)
                actual_y1 = int((y1 / 999.0) * image_height)
                actual_x2 = int((x2 / 999.0) * image_width)
                actual_y2 = int((y2 / 999.0) * image_height)
                # Create path for drag operation
                drag_path = [{"x": actual_x1, "y": actual_y1}, {"x": actual_x2, "y": actual_y2}]
                computer_call = model_dump(make_drag_item(drag_path))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action.startswith("key"):
            key_match = re.search(r"keys='([^']+)'", action)
            if key_match:
                keys = key_match.group(1)
                # Split keys by '+' for key combinations, or use as single key
                key_list = keys.split("+") if "+" in keys else [keys]
                computer_call = model_dump(make_keypress_item(key_list))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action.startswith("type"):
            content_match = re.search(r"content='([^']*)'", action)
            if content_match:
                content = content_match.group(1)
                computer_call = model_dump(make_type_item(content))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action.startswith("scroll"):
            coord_match = re.search(r"start_box='?\[(\d+),\s*(\d+)\]'?", action)
            direction_match = re.search(r"direction='([^']+)'", action)
            if coord_match and direction_match:
                x, y = int(coord_match.group(1)), int(coord_match.group(2))
                direction = direction_match.group(1)
                actual_x = int((x / 999.0) * image_width)
                actual_y = int((y / 999.0) * image_height)
                # Convert direction to scroll amounts
                scroll_x, scroll_y = 0, 0
                if direction == "up":
                    scroll_y = -5
                elif direction == "down":
                    scroll_y = 5
                elif direction == "left":
                    scroll_x = -5
                elif direction == "right":
                    scroll_x = 5
                computer_call = model_dump(make_scroll_item(actual_x, actual_y, scroll_x, scroll_y))
                computer_call["call_id"] = call_id
                computer_call["status"] = "completed"
                response_items.append(computer_call)

        elif action == "WAIT()":
            computer_call = model_dump(make_wait_item())
            computer_call["call_id"] = call_id
            computer_call["status"] = "completed"
            response_items.append(computer_call)

    return response_items


@register_agent(models=r"(?i).*GLM-4\.5V.*")
class Glm4vConfig(AsyncAgentConfig):
    """GLM-4.5V agent configuration using liteLLM."""

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
        Predict the next step using GLM-4.5V model.

        Args:
            messages: Input messages following Responses format
            model: Model name to use
            tools: Optional list of tool schemas
            max_retries: Maximum number of retries for API calls
            stream: Whether to stream the response
            computer_handler: Computer handler for taking screenshots
            use_prompt_caching: Whether to use prompt caching
            _on_api_start: Callback for API start
            _on_api_end: Callback for API end
            _on_usage: Callback for usage tracking
            _on_screenshot: Callback for screenshot events

        Returns:
            Dict with "output" and "usage" keys
        """
        # Get the user instruction from the last user message
        user_instruction = ""
        for message in reversed(messages):
            if isinstance(message, dict) and message.get("role") == "user":
                content = message.get("content", "")
                if isinstance(content, str):
                    user_instruction = content
                elif isinstance(content, list):
                    for item in content:
                        if isinstance(item, dict) and item.get("type") == "text":
                            user_instruction = item.get("text", "")
                            break
                break

        # Get the last image for processing
        last_image_b64 = get_last_image_from_messages(messages)
        if not last_image_b64 and computer_handler:
            # Take a screenshot if no image available
            screenshot_b64 = await computer_handler.screenshot()
            if screenshot_b64:
                last_image_b64 = screenshot_b64
                if _on_screenshot:
                    await _on_screenshot(screenshot_b64)

        if not last_image_b64:
            raise ValueError("No image available for GLM-4.5V processing")

        # Convert responses items to GLM-4.5V PC prompt format with historical actions
        prompt_content = convert_responses_items_to_glm45v_pc_prompt(
            messages=messages,
            task=user_instruction,
            memory="[]",  # Initialize with empty memory for now
        )

        # Add the current screenshot to the end
        prompt_content.append(
            {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{last_image_b64}"}}
        )

        # Prepare messages for liteLLM
        litellm_messages = [
            {"role": "system", "content": "You are a helpful GUI agent assistant."},
            {"role": "user", "content": prompt_content},
        ]

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "messages": litellm_messages,
            # "max_tokens": 2048,
            # "temperature": 0.001,
            # "extra_body": {
            #     "skip_special_tokens": False,
            # }
        }
        api_kwargs.update({k: v for k, v in (kwargs or {}).items()})

        # Add API callbacks
        if _on_api_start:
            await _on_api_start(api_kwargs)

        # Call liteLLM
        response = await litellm.acompletion(**api_kwargs)

        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # Get image dimensions for coordinate scaling
        image_width, image_height = 1920, 1080  # Default dimensions

        # Try to get actual dimensions from the image
        try:
            image_data = base64.b64decode(last_image_b64)
            image = Image.open(BytesIO(image_data))
            image_width, image_height = image.size
        except Exception:
            pass  # Use default dimensions

        # Convert GLM completion response to responses items
        response_items = convert_glm_completion_to_responses_items(
            response, image_width, image_height
        )

        # Extract usage information
        response_usage = {
            **LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(
                response.usage
            ).model_dump(),
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(response_usage)

        # Create agent response
        agent_response = {"output": response_items, "usage": response_usage}

        return agent_response

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using GLM-4.5V model.

        Args:
            model: Model name to use
            image_b64: Base64 encoded image
            instruction: Instruction for where to click

        Returns:
            Tuple with (x, y) coordinates or None
        """
        try:
            # Create a simple click instruction prompt
            click_prompt = f"""You are a GUI agent. Look at the screenshot and identify where to click for: {instruction}

Respond with a single click action in this format:
left_click(start_box='[x,y]')

Where x,y are coordinates normalized to 0-999 range."""

            # Prepare messages for liteLLM
            litellm_messages = [
                {"role": "system", "content": "You are a helpful GUI agent assistant."},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": click_prompt},
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:image/png;base64,{image_b64}"},
                        },
                    ],
                },
            ]

            # Prepare API call kwargs
            api_kwargs = {
                "model": model,
                "messages": litellm_messages,
                "max_tokens": 2056,
                "temperature": 0.001,
                "extra_body": {
                    "skip_special_tokens": False,
                },
            }
            api_kwargs.update({k: v for k, v in (kwargs or {}).items()})

            # Call liteLLM
            response = await litellm.acompletion(**api_kwargs)

            # Extract response content
            response_content = response.choices[0].message.content.strip()
            print(response)

            # Parse response for click coordinates
            # Look for coordinates in the response, handling special tokens
            coord_pattern = r"<\|begin_of_box\|>.*?left_click\(start_box='?\[(\d+),(\d+)\]'?\).*?<\|end_of_box\|>"
            match = re.search(coord_pattern, response_content)

            if not match:
                # Fallback: look for coordinates without special tokens
                coord_pattern = r"left_click\(start_box='?\[(\d+),(\d+)\]'?\)"
                match = re.search(coord_pattern, response_content)

            if match:
                x, y = int(match.group(1)), int(match.group(2))

                # Get actual image dimensions for scaling
                try:
                    image_data = base64.b64decode(image_b64)
                    image = Image.open(BytesIO(image_data))
                    image_width, image_height = image.size
                except Exception:
                    # Use default dimensions
                    image_width, image_height = 1920, 1080

                # Convert from 0-999 normalized coordinates to actual pixel coordinates
                actual_x = int((x / 999.0) * image_width)
                actual_y = int((y / 999.0) * image_height)

                return (actual_x, actual_y)

            return None

        except Exception as e:
            # Log error and return None
            print(f"Error in predict_click: {e}")
            return None

    def get_capabilities(self) -> List[AgentCapability]:
        """
        Get list of capabilities supported by this agent config.

        Returns:
            List of capability strings
        """
        return ["step", "click"]
