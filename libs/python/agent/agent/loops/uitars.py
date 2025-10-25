"""
UITARS agent loop implementation using liteLLM for ByteDance-Seed/UI-TARS-1.5-7B
Paper: https://arxiv.org/abs/2501.12326
Code: https://github.com/bytedance/UI-TARS
"""

import ast
import asyncio
import base64
import json
import math
import re
from ctypes import cast
from io import BytesIO
from typing import Any, AsyncGenerator, Dict, List, Optional, Tuple, Union

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)
from litellm.responses.utils import Usage
from litellm.types.utils import ModelResponse
from openai.types.responses.response_computer_tool_call_param import (
    ActionType,
    ResponseComputerToolCallParam,
)
from openai.types.responses.response_input_param import ComputerCallOutput
from openai.types.responses.response_output_message_param import (
    ResponseOutputMessageParam,
)
from openai.types.responses.response_reasoning_item_param import (
    ResponseReasoningItemParam,
    Summary,
)
from PIL import Image

from ..decorators import register_agent
from ..responses import (
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

# Constants from reference code
IMAGE_FACTOR = 28
MIN_PIXELS = 100 * 28 * 28
MAX_PIXELS = 16384 * 28 * 28
MAX_RATIO = 200

FINISH_WORD = "finished"
WAIT_WORD = "wait"
ENV_FAIL_WORD = "error_env"
CALL_USER = "call_user"

# Action space prompt for UITARS
UITARS_ACTION_SPACE = """
click(start_box='<|box_start|>(x1,y1)<|box_end|>')
left_double(start_box='<|box_start|>(x1,y1)<|box_end|>')
right_single(start_box='<|box_start|>(x1,y1)<|box_end|>')
drag(start_box='<|box_start|>(x1,y1)<|box_end|>', end_box='<|box_start|>(x3,y3)<|box_end|>')
hotkey(key='')
type(content='') #If you want to submit your input, use "\\n" at the end of `content`.
scroll(start_box='<|box_start|>(x1,y1)<|box_end|>', direction='down or up or right or left')
wait() #Sleep for 5s and take a screenshot to check for any changes.
finished(content='xxx') # Use escape characters \\', \\", and \\n in content part to ensure we can parse the content in normal python string format.
"""

UITARS_PROMPT_TEMPLATE = """You are a GUI agent. You are given a task and your action history, with screenshots. You need to perform the next action to complete the task. 

## Output Format
```
Thought: ...
Action: ...
```

## Action Space
{action_space}

## Note
- Use {language} in `Thought` part.
- Write a small plan and finally summarize your next action (with its target element) in one sentence in `Thought` part.

## User Instruction
{instruction}
"""

GROUNDING_UITARS_PROMPT_TEMPLATE = """You are a GUI agent. You are given a task and your action history, with screenshots. You need to perform the next action to complete the task. 

## Output Format

Action: ...


## Action Space
click(point='<|box_start|>(x1,y1)<|box_end|>')

## User Instruction
{instruction}"""


def round_by_factor(number: float, factor: int) -> int:
    """Returns the closest integer to 'number' that is divisible by 'factor'."""
    return round(number / factor) * factor


def ceil_by_factor(number: float, factor: int) -> int:
    """Returns the smallest integer greater than or equal to 'number' that is divisible by 'factor'."""
    return math.ceil(number / factor) * factor


def floor_by_factor(number: float, factor: int) -> int:
    """Returns the largest integer less than or equal to 'number' that is divisible by 'factor'."""
    return math.floor(number / factor) * factor


def smart_resize(
    height: int,
    width: int,
    factor: int = IMAGE_FACTOR,
    min_pixels: int = MIN_PIXELS,
    max_pixels: int = MAX_PIXELS,
) -> tuple[int, int]:
    """
    Rescales the image so that the following conditions are met:
    1. Both dimensions (height and width) are divisible by 'factor'.
    2. The total number of pixels is within the range ['min_pixels', 'max_pixels'].
    3. The aspect ratio of the image is maintained as closely as possible.
    """
    if max(height, width) / min(height, width) > MAX_RATIO:
        raise ValueError(
            f"absolute aspect ratio must be smaller than {MAX_RATIO}, got {max(height, width) / min(height, width)}"
        )
    h_bar = max(factor, round_by_factor(height, factor))
    w_bar = max(factor, round_by_factor(width, factor))
    if h_bar * w_bar > max_pixels:
        beta = math.sqrt((height * width) / max_pixels)
        h_bar = floor_by_factor(height / beta, factor)
        w_bar = floor_by_factor(width / beta, factor)
    elif h_bar * w_bar < min_pixels:
        beta = math.sqrt(min_pixels / (height * width))
        h_bar = ceil_by_factor(height * beta, factor)
        w_bar = ceil_by_factor(width * beta, factor)
    return h_bar, w_bar


def escape_single_quotes(text):
    """Escape single quotes in text for safe string formatting."""
    pattern = r"(?<!\\)'"
    return re.sub(pattern, r"\\'", text)


def parse_action(action_str):
    """Parse action string into structured format."""
    try:
        node = ast.parse(action_str, mode="eval")
        if not isinstance(node, ast.Expression):
            raise ValueError("Not an expression")

        call = node.body
        if not isinstance(call, ast.Call):
            raise ValueError("Not a function call")

        # Get function name
        if isinstance(call.func, ast.Name):
            func_name = call.func.id
        elif isinstance(call.func, ast.Attribute):
            func_name = call.func.attr
        else:
            func_name = None

        # Get keyword arguments
        kwargs = {}
        for kw in call.keywords:
            key = kw.arg
            if isinstance(kw.value, ast.Constant):
                value = kw.value.value
            elif isinstance(kw.value, ast.Str):  # Compatibility with older Python
                value = kw.value.s
            else:
                value = None
            kwargs[key] = value

        return {"function": func_name, "args": kwargs}

    except Exception as e:
        print(f"Failed to parse action '{action_str}': {e}")
        return None


def parse_uitars_response(text: str, image_width: int, image_height: int) -> List[Dict[str, Any]]:
    """Parse UITARS model response into structured actions."""
    text = text.strip()

    # Extract thought
    thought = None
    if text.startswith("Thought:"):
        thought_match = re.search(r"Thought: (.+?)(?=\s*Action:|$)", text, re.DOTALL)
        if thought_match:
            thought = thought_match.group(1).strip()

    # Extract action
    if "Action:" not in text:
        raise ValueError("No Action found in response")

    action_str = text.split("Action:")[-1].strip()

    # Handle special case for type actions
    if "type(content" in action_str:

        def escape_quotes(match):
            return match.group(1)

        pattern = r"type\(content='(.*?)'\)"
        content = re.sub(pattern, escape_quotes, action_str)
        action_str = escape_single_quotes(content)
        action_str = "type(content='" + action_str + "')"

    # Parse the action
    parsed_action = parse_action(action_str.replace("\n", "\\n").lstrip())
    if parsed_action is None:
        raise ValueError(f"Action can't parse: {action_str}")

    action_type = parsed_action["function"]
    params = parsed_action["args"]

    # Process parameters
    action_inputs = {}
    for param_name, param in params.items():
        if param == "":
            continue
        param = str(param).lstrip()
        action_inputs[param_name.strip()] = param

        # Handle coordinate parameters
        if "start_box" in param_name or "end_box" in param_name:
            # Parse coordinates like '<|box_start|>(x,y)<|box_end|>' or '(x,y)'
            # First, remove special tokens
            clean_param = param.replace("<|box_start|>", "").replace("<|box_end|>", "")
            # Then remove parentheses and split
            numbers = clean_param.replace("(", "").replace(")", "").split(",")

            try:
                float_numbers = [
                    float(num.strip()) / 1000 for num in numbers
                ]  # Normalize to 0-1 range

                if len(float_numbers) == 2:
                    # Single point, duplicate for box format
                    float_numbers = [
                        float_numbers[0],
                        float_numbers[1],
                        float_numbers[0],
                        float_numbers[1],
                    ]

                action_inputs[param_name.strip()] = str(float_numbers)
            except ValueError as e:
                # If parsing fails, keep the original parameter value
                print(f"Warning: Could not parse coordinates '{param}': {e}")
                action_inputs[param_name.strip()] = param

    return [
        {
            "thought": thought,
            "action_type": action_type,
            "action_inputs": action_inputs,
            "text": text,
        }
    ]


def convert_to_computer_actions(
    parsed_responses: List[Dict[str, Any]], image_width: int, image_height: int
) -> List[ResponseComputerToolCallParam | ResponseOutputMessageParam]:
    """Convert parsed UITARS responses to computer actions."""
    computer_actions = []

    for response in parsed_responses:
        action_type = response.get("action_type")
        action_inputs = response.get("action_inputs", {})

        if action_type == "finished":
            finished_text = action_inputs.get("content", "Task completed successfully.")
            computer_actions.append(make_output_text_item(finished_text))
            break

        elif action_type == "wait":
            computer_actions.append(make_wait_item())

        elif action_type == "call_user":
            computer_actions.append(
                make_output_text_item("I need assistance from the user to proceed with this task.")
            )

        elif action_type in ["click", "left_single"]:
            start_box = action_inputs.get("start_box")
            if start_box:
                coords = eval(start_box)
                x = int((coords[0] + coords[2]) / 2 * image_width)
                y = int((coords[1] + coords[3]) / 2 * image_height)

                computer_actions.append(make_click_item(x, y, "left"))

        elif action_type == "double_click":
            start_box = action_inputs.get("start_box")
            if start_box:
                coords = eval(start_box)
                x = int((coords[0] + coords[2]) / 2 * image_width)
                y = int((coords[1] + coords[3]) / 2 * image_height)

                computer_actions.append(make_double_click_item(x, y))

        elif action_type == "right_click":
            start_box = action_inputs.get("start_box")
            if start_box:
                coords = eval(start_box)
                x = int((coords[0] + coords[2]) / 2 * image_width)
                y = int((coords[1] + coords[3]) / 2 * image_height)

                computer_actions.append(make_click_item(x, y, "right"))

        elif action_type == "type":
            content = action_inputs.get("content", "")
            computer_actions.append(make_type_item(content))

        elif action_type == "hotkey":
            key = action_inputs.get("key", "")
            keys = key.split()
            computer_actions.append(make_keypress_item(keys))

        elif action_type == "press":
            key = action_inputs.get("key", "")
            computer_actions.append(make_keypress_item([key]))

        elif action_type == "scroll":
            start_box = action_inputs.get("start_box")
            direction = action_inputs.get("direction", "down")

            if start_box:
                coords = eval(start_box)
                x = int((coords[0] + coords[2]) / 2 * image_width)
                y = int((coords[1] + coords[3]) / 2 * image_height)
            else:
                x, y = image_width // 2, image_height // 2

            scroll_y = 5 if "up" in direction.lower() else -5
            computer_actions.append(make_scroll_item(x, y, 0, scroll_y))

        elif action_type == "drag":
            start_box = action_inputs.get("start_box")
            end_box = action_inputs.get("end_box")

            if start_box and end_box:
                start_coords = eval(start_box)
                end_coords = eval(end_box)

                start_x = int((start_coords[0] + start_coords[2]) / 2 * image_width)
                start_y = int((start_coords[1] + start_coords[3]) / 2 * image_height)
                end_x = int((end_coords[0] + end_coords[2]) / 2 * image_width)
                end_y = int((end_coords[1] + end_coords[3]) / 2 * image_height)

                path = [{"x": start_x, "y": start_y}, {"x": end_x, "y": end_y}]
                computer_actions.append(make_drag_item(path))

    return computer_actions


def pil_to_base64(image: Image.Image) -> str:
    """Convert PIL image to base64 string."""
    buffer = BytesIO()
    image.save(buffer, format="PNG")
    return base64.b64encode(buffer.getvalue()).decode("utf-8")


def process_image_for_uitars(
    image_data: str, max_pixels: int = MAX_PIXELS, min_pixels: int = MIN_PIXELS
) -> tuple[Image.Image, int, int]:
    """Process image for UITARS model input."""
    # Decode base64 image
    if image_data.startswith("data:image"):
        image_data = image_data.split(",")[1]

    image_bytes = base64.b64decode(image_data)
    image = Image.open(BytesIO(image_bytes))

    original_width, original_height = image.size

    # Resize image according to UITARS requirements
    if image.width * image.height > max_pixels:
        resize_factor = math.sqrt(max_pixels / (image.width * image.height))
        width = int(image.width * resize_factor)
        height = int(image.height * resize_factor)
        image = image.resize((width, height))

    if image.width * image.height < min_pixels:
        resize_factor = math.sqrt(min_pixels / (image.width * image.height))
        width = math.ceil(image.width * resize_factor)
        height = math.ceil(image.height * resize_factor)
        image = image.resize((width, height))

    if image.mode != "RGB":
        image = image.convert("RGB")

    return image, original_width, original_height


def sanitize_message(msg: Any) -> Any:
    """Return a copy of the message with image_url ommited within content parts"""
    if isinstance(msg, dict):
        result = {}
        for key, value in msg.items():
            if key == "content" and isinstance(value, list):
                result[key] = [
                    (
                        {k: v for k, v in item.items() if k != "image_url"}
                        if isinstance(item, dict)
                        else item
                    )
                    for item in value
                ]
            else:
                result[key] = value
        return result
    elif isinstance(msg, list):
        return [sanitize_message(item) for item in msg]
    else:
        return msg


def convert_uitars_messages_to_litellm(messages: Messages) -> List[Dict[str, Any]]:
    """
    Convert UITARS internal message format back to LiteLLM format.

    This function processes reasoning, computer_call, and computer_call_output messages
    and converts them to the appropriate LiteLLM assistant message format.

    Args:
        messages: List of UITARS internal messages

    Returns:
        List of LiteLLM formatted messages
    """
    litellm_messages = []
    current_assistant_content = []

    for message in messages:
        if isinstance(message, dict):
            message_type = message.get("type")

            if message_type == "reasoning":
                # Extract reasoning text from summary
                summary = message.get("summary", [])
                if summary and isinstance(summary, list):
                    for summary_item in summary:
                        if (
                            isinstance(summary_item, dict)
                            and summary_item.get("type") == "summary_text"
                        ):
                            reasoning_text = summary_item.get("text", "")
                            if reasoning_text:
                                current_assistant_content.append(f"Thought: {reasoning_text}")

            elif message_type == "computer_call":
                # Convert computer action to UITARS action format
                action = message.get("action", {})
                action_type = action.get("type")

                if action_type == "click":
                    x, y = action.get("x", 0), action.get("y", 0)
                    button = action.get("button", "left")
                    if button == "left":
                        action_text = f"Action: click(start_box='({x},{y})')"
                    elif button == "right":
                        action_text = f"Action: right_single(start_box='({x},{y})')"
                    else:
                        action_text = f"Action: click(start_box='({x},{y})')"

                elif action_type == "double_click":
                    x, y = action.get("x", 0), action.get("y", 0)
                    action_text = f"Action: left_double(start_box='({x},{y})')"

                elif action_type == "drag":
                    start_x, start_y = action.get("start_x", 0), action.get("start_y", 0)
                    end_x, end_y = action.get("end_x", 0), action.get("end_y", 0)
                    action_text = f"Action: drag(start_box='({start_x},{start_y})', end_box='({end_x},{end_y})')"

                elif action_type == "key":
                    key = action.get("key", "")
                    action_text = f"Action: hotkey(key='{key}')"

                elif action_type == "type":
                    text = action.get("text", "")
                    # Escape single quotes in the text
                    escaped_text = escape_single_quotes(text)
                    action_text = f"Action: type(content='{escaped_text}')"

                elif action_type == "scroll":
                    x, y = action.get("x", 0), action.get("y", 0)
                    direction = action.get("direction", "down")
                    action_text = f"Action: scroll(start_box='({x},{y})', direction='{direction}')"

                elif action_type == "wait":
                    action_text = "Action: wait()"

                else:
                    # Fallback for unknown action types
                    action_text = f"Action: {action_type}({action})"

                current_assistant_content.append(action_text)

                # When we hit a computer_call_output, finalize the current assistant message
                if current_assistant_content:
                    litellm_messages.append(
                        {
                            "role": "assistant",
                            "content": [
                                {"type": "text", "text": "\n".join(current_assistant_content)}
                            ],
                        }
                    )
                    current_assistant_content = []

            elif message_type == "computer_call_output":
                # Add screenshot from computer call output
                output = message.get("output", {})
                if isinstance(output, dict) and output.get("type") == "input_image":
                    image_url = output.get("image_url", "")
                    if image_url:
                        litellm_messages.append(
                            {
                                "role": "user",
                                "content": [{"type": "image_url", "image_url": {"url": image_url}}],
                            }
                        )

            elif message.get("role") == "user":
                # # Handle user messages
                # content = message.get("content", "")
                # if isinstance(content, str):
                #     litellm_messages.append({
                #         "role": "user",
                #         "content": content
                #     })
                # elif isinstance(content, list):
                #     litellm_messages.append({
                #         "role": "user",
                #         "content": content
                #     })
                pass

    # Add any remaining assistant content
    if current_assistant_content:
        litellm_messages.append({"role": "assistant", "content": current_assistant_content})

    return litellm_messages


@register_agent(models=r"(?i).*ui-?tars.*")
class UITARSConfig:
    """
    UITARS agent configuration using liteLLM for ByteDance-Seed/UI-TARS-1.5-7B model.

    Supports UITARS vision-language models for computer control.
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
        Predict the next step based on input messages.

        Args:
            messages: Input messages following Responses format
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

        # Create response items
        response_items = []

        # Find computer tool for screen dimensions
        computer_tool = None
        for tool_schema in tools:
            if tool_schema["type"] == "computer":
                computer_tool = tool_schema["computer"]
                break

        # Get screen dimensions
        screen_width, screen_height = 1024, 768
        if computer_tool:
            try:
                screen_width, screen_height = await computer_tool.get_dimensions()
            except:
                pass

        # Process messages to extract instruction and image
        instruction = ""
        image_data = None

        # Convert messages to list if string
        if isinstance(messages, str):
            messages = [{"role": "user", "content": messages}]

        # Extract instruction and latest screenshot
        for message in reversed(messages):
            if isinstance(message, dict):
                content = message.get("content", "")

                # Handle different content formats
                if isinstance(content, str):
                    if not instruction and message.get("role") == "user":
                        instruction = content
                elif isinstance(content, list):
                    for item in content:
                        if isinstance(item, dict):
                            if item.get("type") == "text" and not instruction:
                                instruction = item.get("text", "")
                            elif item.get("type") == "image_url" and not image_data:
                                image_url = item.get("image_url", {})
                                if isinstance(image_url, dict):
                                    image_data = image_url.get("url", "")
                                else:
                                    image_data = image_url

            # Also check for computer_call_output with screenshots
            if message.get("type") == "computer_call_output" and not image_data:
                output = message.get("output", {})
                if isinstance(output, dict) and output.get("type") == "input_image":
                    image_data = output.get("image_url", "")

            if instruction and image_data:
                break

        if not instruction:
            instruction = (
                "Help me complete this task by analyzing the screen and taking appropriate actions."
            )

        # Create prompt
        user_prompt = UITARS_PROMPT_TEMPLATE.format(
            instruction=instruction, action_space=UITARS_ACTION_SPACE, language="English"
        )

        # Convert conversation history to LiteLLM format
        history_messages = convert_uitars_messages_to_litellm(messages)

        # Prepare messages for liteLLM
        litellm_messages = [{"role": "system", "content": "You are a helpful assistant."}]

        # Add current user instruction with screenshot
        current_user_message = {
            "role": "user",
            "content": [
                {"type": "text", "text": user_prompt},
            ],
        }
        litellm_messages.append(current_user_message)

        # Process image for UITARS
        if not image_data:
            # Take screenshot if none found in messages
            if computer_handler:
                image_data = await computer_handler.screenshot()
                await _on_screenshot(image_data, "screenshot_before")

                # Add screenshot to output items so it can be retained in history
                response_items.append(make_input_image_item(image_data))
            else:
                raise ValueError("No screenshot found in messages and no computer_handler provided")
        processed_image, original_width, original_height = process_image_for_uitars(image_data)
        encoded_image = pil_to_base64(processed_image)

        # Add conversation history
        if history_messages:
            litellm_messages.extend(history_messages)
        else:
            litellm_messages.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:image/png;base64,{encoded_image}"},
                        }
                    ],
                }
            )

        # Prepare API call kwargs
        api_kwargs = {
            "model": model,
            "messages": litellm_messages,
            "max_tokens": kwargs.get("max_tokens", 500),
            "temperature": kwargs.get("temperature", 0.0),
            "do_sample": kwargs.get("temperature", 0.0) > 0.0,
            "num_retries": max_retries,
            **{k: v for k, v in kwargs.items() if k not in ["max_tokens", "temperature"]},
        }

        # Call API start hook
        if _on_api_start:
            await _on_api_start(api_kwargs)

        # Call liteLLM with UITARS model
        response = await litellm.acompletion(**api_kwargs)

        # Call API end hook
        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # Extract response content
        response_content = response.choices[0].message.content.strip()  # type: ignore

        # Parse UITARS response
        parsed_responses = parse_uitars_response(response_content, original_width, original_height)

        # Convert to computer actions
        computer_actions = convert_to_computer_actions(
            parsed_responses, original_width, original_height
        )

        # Add computer actions to response items
        thought = parsed_responses[0].get("thought", "")
        if thought:
            response_items.append(make_reasoning_item(thought))
        response_items.extend(computer_actions)

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
        Predict click coordinates based on image and instruction.

        UITARS supports click prediction through its action parsing.

        Args:
            model: Model name to use
            image_b64: Base64 encoded image
            instruction: Instruction for where to click

        Returns:
            Tuple with (x, y) coordinates or None
        """
        try:
            # Create prompt using grounding template
            user_prompt = GROUNDING_UITARS_PROMPT_TEMPLATE.format(instruction=instruction)

            # Process image for UITARS
            processed_image, original_width, original_height = process_image_for_uitars(image_b64)
            encoded_image = pil_to_base64(processed_image)

            # Prepare messages for liteLLM
            litellm_messages = [
                {"role": "system", "content": "You are a helpful assistant."},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": user_prompt},
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:image/png;base64,{encoded_image}"},
                        },
                    ],
                },
            ]

            # Prepare API call kwargs
            api_kwargs = {
                "model": model,
                "messages": litellm_messages,
                "max_tokens": 2056,
                "temperature": 0.0,
                "do_sample": False,
            }
            api_kwargs.update({k: v for k, v in (kwargs or {}).items()})

            # Call liteLLM with UITARS model
            response = await litellm.acompletion(**api_kwargs)

            # Extract response content
            response_content = response.choices[0].message.content.strip()  # type: ignore

            print(response_content)

            # Parse the response to extract click coordinates
            # Look for click action with coordinates (with special tokens)
            click_pattern = r"click\(point='<\|box_start\|>\((\d+),(\d+)\)<\|box_end\|>'\)"
            match = re.search(click_pattern, response_content)

            # Fallback: Look for simpler format without special tokens
            if not match:
                # Pattern for: click(start_box='(x,y)') or click(point='(x,y)')
                fallback_pattern = r"click\((?:start_box|point)='\((\d+),(\d+)\)'\)"
                match = re.search(fallback_pattern, response_content)

            if match:
                x, y = int(match.group(1)), int(match.group(2))
                # Scale coordinates back to original image dimensions
                scale_x = original_width / processed_image.width
                scale_y = original_height / processed_image.height

                scaled_x = int(x * scale_x)
                scaled_y = int(y * scale_y)

                return (scaled_x, scaled_y)

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
